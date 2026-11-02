//! Activity glue over the public API (AC32). The activity tests that need
//! the client come with WP5 part H1.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    reason = "tests with tables"
)]

use std::time::Duration;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use autumn_harvest::context::ActivityContext;
use autumn_harvest::failure::{IntoActivityErrorString, parse_typed_payload};
use autumn_jira::backend::BackendMode;
use autumn_jira::client::ClientSettings;
use autumn_jira::error::{MarkerRef, Op, UnknownCause};
use autumn_jira::ids::SiteName;
use autumn_jira::san_mateo::{EnvironmentConfig, KeyRing, SanMateo, Secret};
use autumn_jira::{BasicAuth, JiraClient, JiraError, JiraSite, ProjectKey};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira};
use autumn_jira_harvest::{
    CONFIG_ERROR, ClientRef, ERROR_PREFIX, JiraClients, OUTCOME_PENDING_ERROR,
    OUTCOME_UNKNOWN_ERROR, SETTLE_SECS, client_for, failure_for,
};
use serde_json::{Value, json};

/// A client of the fake at `site`, with the default (not durable) settings.
fn client(fake: &FakeJira, site: &str) -> JiraClient {
    JiraClient::api_token(
        JiraSite::Subdomain(SiteName::parse(site).unwrap()),
        BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).unwrap(),
        Arc::new(fake.clone()),
    )
}

/// A san-mateo facade over the fake. It needs no integration to make a
/// client: the backend reads san-mateo only when it sends.
fn san_mateo(fake: &FakeJira) -> SanMateo {
    SanMateo::builder()
        .environment(EnvironmentConfig::new("dev", "sm-secret-key"))
        .key_ring(KeyRing::new("k1", [42; 32]).unwrap())
        .transport(Arc::new(fake.clone()))
        .build()
        .unwrap()
}

/// An activity context with these state values.
fn ctx_with(values: Vec<(TypeId, Box<dyn Any + Send + Sync>)>) -> ActivityContext {
    let state: HashMap<TypeId, Box<dyn Any + Send + Sync>> = values.into_iter().collect();
    ActivityContext::new_test_with_state(Arc::new(state))
}

#[test]
fn client_ref_named_and_connection() {
    let fake = FakeJira::new();
    let clients = JiraClients::new(client(&fake, "acme")).with("ops", client(&fake, "ops"));
    assert!(!format!("{clients:?}").contains(DEFAULT_API_TOKEN));
    let ctx = ctx_with(vec![
        (TypeId::of::<JiraClients>(), Box::new(clients)),
        (TypeId::of::<SanMateo>(), Box::new(san_mateo(&fake))),
    ]);
    let durable = ClientSettings::for_durable_host();

    let default = client_for(&ctx, &ClientRef::Default).unwrap();
    assert_eq!(default.info().site, "acme.atlassian.net");
    assert_eq!(default.settings(), &durable);

    let named = client_for(&ctx, &ClientRef::Named { name: "ops".into() }).unwrap();
    assert_eq!(named.info().site, "ops.atlassian.net");
    assert_eq!(named.settings(), &durable);

    let connection = ClientRef::Connection {
        environment: "dev".into(),
        integration: "jira".into(),
        connection_id: "acme".into(),
    };
    let over_san = client_for(&ctx, &connection).unwrap();
    assert_eq!(over_san.info().mode, BackendMode::SanMateo);
    assert_eq!(over_san.info().site, "san-mateo:jira/acme");
    assert_eq!(over_san.settings(), &durable);

    let missing = client_for(
        &ctx,
        &ClientRef::Named {
            name: "nope".into(),
        },
    )
    .unwrap_err();
    assert_eq!(missing.error_type, CONFIG_ERROR);
    assert!(missing.non_retryable);
    let bad = ClientRef::Connection {
        environment: "dev".into(),
        integration: "bad key".into(),
        connection_id: "acme".into(),
    };
    let invalid = client_for(&ctx, &bad).unwrap_err();
    assert_eq!(invalid.error_type, "jira.invalid_input");
    assert!(invalid.non_retryable);
}

#[test]
fn missing_state_is_config() {
    let ctx = ActivityContext::new_test();
    let connection = ClientRef::Connection {
        environment: "dev".into(),
        integration: "jira".into(),
        connection_id: "acme".into(),
    };
    for client in [
        ClientRef::Default,
        ClientRef::Named { name: "ops".into() },
        connection,
    ] {
        let failure = client_for(&ctx, &client).unwrap_err();
        assert_eq!(failure.error_type, CONFIG_ERROR, "{client:?}");
        assert!(failure.non_retryable, "{client:?}");
        assert!(failure.message.contains("state"), "{}", failure.message);
    }
}

/// One sample of each `JiraError` variant, with the `sent` value and the
/// `retry_after` of its failure.
fn samples() -> Vec<(JiraError, bool, Option<Duration>)> {
    let op = Op::CreateIssue;
    let secret = "s3cr3t-token-value";
    vec![
        (
            JiraError::InvalidInput {
                what: "issue key",
                reason: "must match [A-Z]".into(),
            },
            false,
            None,
        ),
        (JiraError::NotFoundOrForbidden { op }, false, None),
        (
            JiraError::Unauthorized {
                op,
                status: 401,
                hint: autumn_jira::error::SCOPE_HINT,
            },
            false,
            None,
        ),
        (JiraError::CaptchaRequired { op }, false, None),
        (
            JiraError::Validation {
                op,
                status: 400,
                messages: vec![format!("bad value {secret}")],
                fields: vec!["summary".into()],
            },
            false,
            None,
        ),
        (JiraError::Conflict { op }, false, None),
        (JiraError::Gone { op }, false, None),
        (JiraError::PayloadTooLarge { op }, false, None),
        (
            JiraError::RateLimited {
                op,
                retry_after: Some(Duration::from_secs(7)),
                reason: Some("jira-burst-based".into()),
            },
            false,
            Some(Duration::from_secs(7)),
        ),
        (
            JiraError::RateLimited {
                op,
                retry_after: None,
                reason: None,
            },
            false,
            None,
        ),
        (JiraError::Upstream { op, status: 503 }, true, None),
        (
            JiraError::OutcomeUnknown {
                op,
                cause: UnknownCause::ServerError(503),
                marker: Some(MarkerRef::label("autumn-idem-abc".into())),
            },
            true,
            Some(Duration::from_secs(SETTLE_SECS)),
        ),
        (
            JiraError::OutcomeUnknown {
                op,
                cause: UnknownCause::Network,
                marker: None,
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "connect",
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "timeout",
            },
            true,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "store_error",
            },
            false,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "connection_refresh_backoff",
            },
            false,
            None,
        ),
        (
            JiraError::Network {
                op,
                code: "provider",
            },
            false,
            None,
        ),
        (JiraError::Redirect { op, status: 302 }, true, None),
        (JiraError::UnexpectedStatus { op, status: 202 }, true, None),
        (
            JiraError::Decode {
                op,
                reason: "missing field `id` at line 1 column 2".into(),
            },
            true,
            None,
        ),
        (
            JiraError::PaginationStalled {
                op: Op::Search,
                pages: 3,
            },
            true,
            None,
        ),
        (
            JiraError::PageLimit {
                op: Op::Search,
                max: 10,
            },
            true,
            None,
        ),
        (
            JiraError::TransitionNotAvailable {
                target: "category done".into(),
                available: vec!["Start work".into()],
            },
            false,
            None,
        ),
        (
            JiraError::LabelsNotOnScreen {
                project: ProjectKey::parse("OPS").unwrap(),
            },
            false,
            None,
        ),
        (JiraError::CloudId("no Jira site".into()), false, None),
        (JiraError::Config { code: "no_client" }, false, None),
        (
            JiraError::Auth {
                code: "refresh_exhausted",
            },
            false,
            None,
        ),
    ]
}

#[test]
fn failure_for_table() {
    for (err, sent, retry_after) in samples() {
        let failure = failure_for(&err);
        let name = format!("{err:?}");
        assert_eq!(
            failure.error_type,
            format!("{ERROR_PREFIX}{}", err.code()),
            "{name}"
        );
        assert_eq!(failure.non_retryable, !err.is_retryable(), "{name}");
        assert_eq!(failure.retry_after, retry_after, "{name}");
        assert_eq!(failure.message, err.to_string(), "{name}");
        assert_eq!(
            failure.details,
            Some(json!({"code": err.code(), "status": err.status(), "sent": sent})),
            "{name}"
        );
        // The wire payload keeps the type, the class and the hint.
        let wire = parse_typed_payload(&failure.clone().into_error_payload()).unwrap();
        assert_eq!(wire.error_type, failure.error_type, "{name}");
        assert_eq!(wire.non_retryable, failure.non_retryable, "{name}");
        assert_eq!(wire.retry_after, failure.retry_after, "{name}");
        // No Jira message and no secret in the failure.
        let text = serde_json::to_string(&failure).unwrap();
        assert!(!text.contains("s3cr3t"), "{name}: {text}");
        assert!(!text.contains("bad value"), "{name}: {text}");
    }
}

#[test]
fn outcome_codes_have_their_constants() {
    let op = Op::AddComment;
    let pending = failure_for(&JiraError::OutcomeUnknown {
        op,
        cause: UnknownCause::Timeout,
        marker: Some(MarkerRef::comment("autumn-ref: k".into())),
    });
    assert_eq!(pending.error_type, OUTCOME_PENDING_ERROR);
    assert!(!pending.non_retryable);
    let unknown = failure_for(&JiraError::OutcomeUnknown {
        op,
        cause: UnknownCause::BadReply,
        marker: None,
    });
    assert_eq!(unknown.error_type, OUTCOME_UNKNOWN_ERROR);
    assert!(unknown.non_retryable);
    assert_eq!(
        unknown.details.as_ref().and_then(|d| d.get("status")),
        Some(&Value::Null)
    );
}
