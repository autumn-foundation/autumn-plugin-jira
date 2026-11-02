//! The activities over `FakeJira` (AC32, AC35).
//!
//! The work methods run with an explicit durable client and a test
//! `ActivityContext`, so no worker is needed. A second attempt is a new
//! context with the same idempotency key, `attempt = 2` and the failure
//! payload of the first attempt, as the worker gives it.

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

use autumn_harvest::context::{ActivityContext, WorkflowContext};
use autumn_harvest::failure::{IntoActivityErrorString, WorkflowFailure, parse_typed_payload};
use autumn_harvest::info::ActivityInfo;
use autumn_harvest::policy::RetryPolicy;
use autumn_harvest::prelude::workflow;
use autumn_harvest::{HarvestBuilder, WorkflowSimulator};
use autumn_jira::backend::BackendMode;
use autumn_jira::client::ClientSettings;
use autumn_jira::error::{MarkerRef, Op, UnknownCause};
use autumn_jira::ids::SiteName;
use autumn_jira::policy::OpClass;
use autumn_jira::request::IssueTypeRef;
use autumn_jira::san_mateo::{EnvironmentConfig, KeyRing, SanMateo, Secret};
use autumn_jira::{
    BasicAuth, IssueKey, IssueRef, JiraClient, JiraError, JiraSite, NewIssue, ProjectKey,
    StatusTarget,
};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
use autumn_jira_harvest::activities::{
    COMMENT_LOOKUP_PAGES, COMMENT_START_TO_CLOSE, LONG_WRITE_START_TO_CLOSE, READ_START_TO_CLOSE,
    WRITE_START_TO_CLOSE,
};
use autumn_jira_harvest::{
    AddCommentInput, CONFIG_ERROR, ClientRef, CreateIssueInput, CreateIssueOutput, CreateKind,
    DeleteIssueInput, DeleteKind, ERROR_PREFIX, EditIssueInput, GetIssueInput, JiraClients,
    OUTCOME_PENDING_ERROR, OUTCOME_UNKNOWN_ERROR, ROUTES_PROPERTY, RouteKey, SETTLE_SECS,
    SearchPageInput, SignalRoute, TRACKED_LABEL, TransitionInput, TransitionKind, WatchIssueInput,
    activities, client_for, failure_for, jira_create_issue_info,
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
            Some(json!({
                "code": err.code(),
                "status": err.status(),
                "sent": sent,
                "retryable": err.is_retryable(),
            })),
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

// ------------------------------------------------------------ helpers

/// The route key of the tests.
const ROUTE_SECRET: &str = "route-key-secret-0123456789-abcdef";

fn route_key() -> RouteKey {
    RouteKey::new(Secret::new(ROUTE_SECRET)).unwrap()
}

/// Fast settings for tests: short waits, no jitter.
fn fast() -> ClientSettings {
    let mut settings = ClientSettings::default();
    settings.base_backoff_ms = 50;
    settings.jitter = false;
    settings
}

/// The durable client of the fake, as `client_for` gives it.
fn durable(fake: &FakeJira) -> JiraClient {
    client(fake, "acme").with_settings(fast()).durable()
}

/// The state of a worker: the clients and the route key.
fn state(fake: &FakeJira) -> Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>> {
    let clients = JiraClients::new(client(fake, "acme").with_settings(fast()));
    let mut state: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
    state.insert(TypeId::of::<JiraClients>(), Box::new(clients));
    state.insert(TypeId::of::<RouteKey>(), Box::new(route_key()));
    Arc::new(state)
}

/// Attempt 1 with the route key in the state.
fn first_ctx() -> ActivityContext {
    ctx_with(vec![(TypeId::of::<RouteKey>(), Box::new(route_key()))]).with_max_attempts(5)
}

/// A later attempt of `first`: the same key, the previous failure payload.
fn retry_of(first: &ActivityContext, attempt: u32, previous: Option<String>) -> ActivityContext {
    ctx_with(vec![(TypeId::of::<RouteKey>(), Box::new(route_key()))])
        .with_idempotency_key(first.idempotency_key().unwrap().clone())
        .with_attempt(attempt)
        .with_max_attempts(5)
        .with_previous_failure(previous)
}

fn ops() -> ProjectKey {
    ProjectKey::parse("OPS").unwrap()
}

fn create_input() -> CreateIssueInput {
    let mut input = CreateIssueInput::new(ops(), IssueTypeRef::Name("Task".into()), "Disk full");
    input.description_text = Some("db-1 has no free space".into());
    input.labels = vec!["ops".into()];
    input.settle_secs = Some(3);
    input
}

/// A person makes an issue: a plain client, no marker.
async fn human_issue(fake: &FakeJira, summary: &str) -> IssueKey {
    let created = client(fake, "acme")
        .create_issue(&NewIssue::new(
            ops(),
            IssueTypeRef::Name("Task".into()),
            summary,
        ))
        .await
        .unwrap();
    created.key
}

fn key_ref(key: &IssueKey) -> IssueRef {
    IssueRef::Key(key.clone())
}

/// Every text that must never be in an output, a failure or a history.
fn assert_no_secrets(text: &str) {
    for secret in [DEFAULT_API_TOKEN, DEFAULT_EMAIL, ROUTE_SECRET, "Basic "] {
        assert!(!text.contains(secret), "a secret leaked: {text}");
    }
}

// ------------------------------------------------------------ AC32

#[test]
fn names_are_pinned() {
    let names: Vec<&str> = activities().iter().map(|info| info.name).collect();
    assert_eq!(
        names,
        [
            "jira_create_issue",
            "jira_get_issue",
            "jira_search_page",
            "jira_edit_issue",
            "jira_transition_to_status",
            "jira_add_comment",
            "jira_delete_issue",
            "jira_watch_issue",
        ]
    );
    assert_eq!(autumn_jira_harvest::GET_ISSUE, names[1]);
    // The activities register on a builder with the state.
    let fake = FakeJira::new();
    let built = HarvestBuilder::new()
        .activities(activities())
        .state(JiraClients::new(client(&fake, "acme")))
        .state(route_key())
        .try_build()
        .unwrap();
    assert!(built.state::<JiraClients>().is_some());
    assert!(built.state::<RouteKey>().is_some());
}

/// The client calls of one activity and the class of each, for its worst case.
fn calls(name: &str) -> Vec<OpClass> {
    match name {
        // The lookup-only path is one read. The send path is one marked write.
        "jira_create_issue" => vec![OpClass::MarkedWrite],
        // The marked write counts one page for the lookup before the send and
        // one page for the lookup after it. The comment lookup also reads
        // `GET /myself` one time (a cold cache), and up to
        // `COMMENT_LOOKUP_PAGES - 1` more pages in each of the two lookups.
        "jira_add_comment" => {
            let extra = 2 * (usize::try_from(COMMENT_LOOKUP_PAGES).unwrap() - 1);
            let mut calls = vec![OpClass::MarkedWrite, OpClass::Read];
            calls.extend(std::iter::repeat_n(OpClass::Read, extra));
            calls
        }
        "jira_get_issue" | "jira_search_page" => vec![OpClass::Read],
        "jira_edit_issue" | "jira_delete_issue" => vec![OpClass::IdempotentWrite],
        "jira_transition_to_status" => vec![OpClass::CheckedWrite],
        // GET issue, GET property, PUT property, PUT edit.
        "jira_watch_issue" => vec![
            OpClass::Read,
            OpClass::Read,
            OpClass::IdempotentWrite,
            OpClass::IdempotentWrite,
        ],
        other => panic!("no budget row for {other}"),
    }
}

#[test]
fn start_to_close_covers_the_client_budget() {
    // The budgets hold for a transport timeout of 60 s or less (R9-test-quality-6).
    // A `ClientRef::Connection` client sends through san-mateo. One client
    // send there can be four transport calls: a refresh before the send, the
    // send, a refresh after a 401 and the send again (R4-harvest-determinism-6).
    let settings = ClientSettings::for_durable_host();
    let max = autumn_jira_harvest::activities::MAX_TRANSPORT_TIMEOUT.as_secs();
    let san_mateo = u64::from(autumn_jira_harvest::activities::SAN_MATEO_CALLS_PER_SEND);
    assert_eq!((max, san_mateo), (60, 4));
    // The plugin caps `[jira.http] timeout_secs` at the core value. The
    // plugin does not depend on harvest, so this test keeps the two equal.
    assert_eq!(max, autumn_jira::client::MAX_TRANSPORT_TIMEOUT_SECS);
    for timeout in [1, 30, max] {
        for (path, calls_per_send) in [("direct", 1), ("san-mateo", san_mateo)] {
            let per_request = Duration::from_secs(timeout * calls_per_send);
            for info in activities() {
                let budget: Duration = calls(info.name)
                    .into_iter()
                    .map(|class| settings.worst_case(class, per_request))
                    .sum();
                let start_to_close = info.default_start_to_close.expect("a start_to_close");
                assert!(
                    start_to_close > budget,
                    "{} ({path}, {timeout} s): {start_to_close:?} <= {budget:?}",
                    info.name
                );
                let retry = info.default_retry_policy.clone().expect("a retry policy");
                assert_eq!(retry.max_attempts, 5, "{}", info.name);
            }
        }
    }
    let per_request = Duration::from_secs(60);
    let marked = settings.worst_case(OpClass::MarkedWrite, per_request);
    let checked = settings.worst_case(OpClass::CheckedWrite, per_request);
    assert_eq!(marked, Duration::from_secs(390));
    assert_eq!(checked, Duration::from_secs(650));
    let by_name = |name: &str| {
        activities()
            .into_iter()
            .find(|info| info.name == name)
            .and_then(|info| info.default_start_to_close)
    };
    assert_eq!(
        by_name("jira_create_issue"),
        Some(LONG_WRITE_START_TO_CLOSE)
    );
    assert_eq!(by_name("jira_add_comment"), Some(COMMENT_START_TO_CLOSE));
    assert_eq!(
        by_name("jira_transition_to_status"),
        Some(LONG_WRITE_START_TO_CLOSE)
    );
    assert_eq!(by_name("jira_watch_issue"), Some(LONG_WRITE_START_TO_CLOSE));
    assert_eq!(by_name("jira_edit_issue"), Some(WRITE_START_TO_CLOSE));
    assert_eq!(by_name("jira_delete_issue"), Some(WRITE_START_TO_CLOSE));
    assert_eq!(by_name("jira_get_issue"), Some(READ_START_TO_CLOSE));
    assert_eq!(by_name("jira_search_page"), Some(READ_START_TO_CLOSE));
    assert_eq!(
        (READ_START_TO_CLOSE, LONG_WRITE_START_TO_CLOSE),
        (Duration::from_secs(540), Duration::from_secs(2_520))
    );
    assert_eq!(
        (COMMENT_LOOKUP_PAGES, COMMENT_START_TO_CLOSE),
        (3, Duration::from_secs(4_200))
    );
}

#[tokio::test(start_paused = true)]
async fn lost_create_is_pending_then_found() {
    let fake = FakeJira::new();
    // The search index lags, so the lookup in attempt 1 finds nothing.
    fake.index_lag(Duration::from_secs(2));
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let client = durable(&fake);
    let input = create_input();
    let first = first_ctx();
    let failure = input.run(&client, &first).await.unwrap_err();
    assert_eq!(failure.error_type, OUTCOME_PENDING_ERROR);
    assert!(!failure.non_retryable);
    // `settle_secs` overrides SETTLE_SECS.
    assert_eq!(failure.retry_after, Some(Duration::from_secs(3)));
    assert_no_secrets(&serde_json::to_string(&failure).unwrap());

    // The worker waits `retry_after`. The index catches up.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let second = retry_of(&first, 2, Some(failure.into_error_payload()));
    let output = input.run(&client, &second).await.unwrap();
    assert_eq!(output.outcome, CreateKind::Found);
    assert_eq!(fake.count(FakeOp::CreateIssue), 1, "no second POST");
    assert_eq!(fake.issues().len(), 1);
    assert_eq!(output.key.as_str(), "OPS-1");
}

#[tokio::test(start_paused = true)]
async fn maybe_sent_attempt_looks_up_only() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let input = create_input();
    let first = first_ctx();
    let pending = failure_for(&JiraError::OutcomeUnknown {
        op: Op::CreateIssue,
        cause: UnknownCause::ServerError(503),
        marker: Some(MarkerRef::label("autumn-idem-x".into())),
    })
    .into_error_payload();
    // After "maybe sent", and after a crash (no typed failure): lookup only.
    for (attempt, previous) in [(2, Some(pending.clone())), (3, None)] {
        let ctx = retry_of(&first, attempt, previous);
        let failure = input.run(&client, &ctx).await.unwrap_err();
        assert_eq!(failure.error_type, OUTCOME_UNKNOWN_ERROR, "{attempt}");
        assert!(failure.non_retryable, "{attempt}");
        assert!(
            failure.message.contains("resend_after_negative_lookup"),
            "{}",
            failure.message
        );
    }
    assert_eq!(fake.count(FakeOp::CreateIssue), 0, "a lookup only");
    assert_eq!(fake.count(FakeOp::Search), 2);

    // The same rule for a comment.
    let key = human_issue(&fake, "Printer").await;
    // The person's issue is one create POST.
    let posts = fake.count(FakeOp::CreateIssue);
    let comment = AddCommentInput::new(key_ref(&key), "Fixed it");
    let ctx = retry_of(&first, 2, Some(pending.clone()));
    let failure = comment.run(&client, &ctx).await.unwrap_err();
    assert_eq!(failure.error_type, OUTCOME_UNKNOWN_ERROR);
    assert_eq!(fake.count(FakeOp::AddComment), 0);

    // A failed lookup keeps the attempt "maybe sent": the next attempt
    // looks up again.
    fake.fault(FakeOp::Search, Fault::Status(503));
    fake.fault(FakeOp::Search, Fault::Status(503));
    let ctx = retry_of(&first, 2, Some(pending));
    let failure = input.run(&client, &ctx).await.unwrap_err();
    assert_eq!(failure.error_type, OUTCOME_PENDING_ERROR);
    assert_eq!(failure.retry_after, Some(Duration::from_secs(3)));
    let next = retry_of(&first, 3, Some(failure.into_error_payload()));
    assert_eq!(
        autumn_jira_harvest::previous_attempt(&next),
        autumn_jira::policy::Previous::MaybeSent
    );
    assert_eq!(fake.count(FakeOp::CreateIssue), posts);

    // A found write needs no POST.
    let made = input.run(&client, &first).await.unwrap();
    assert_eq!(made.outcome, CreateKind::Created);
    let ctx = retry_of(&first, 2, None);
    let found = input.run(&client, &ctx).await.unwrap();
    assert_eq!(found.outcome, CreateKind::Found);
    assert_eq!(found.id, made.id);
    assert_eq!(fake.count(FakeOp::CreateIssue), posts + 1);
}

#[tokio::test(start_paused = true)]
async fn opt_in_resends_after_negative_lookup() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let mut input = create_input();
    input.resend_after_negative_lookup = true;
    let first = first_ctx();
    let ctx = retry_of(&first, 2, None);
    let output = input.run(&client, &ctx).await.unwrap();
    assert_eq!(output.outcome, CreateKind::Created);
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);

    let key = human_issue(&fake, "Printer").await;
    let mut comment = AddCommentInput::new(key_ref(&key), "Fixed it");
    comment.resend_after_negative_lookup = true;
    let made = comment.run(&client, &ctx).await.unwrap();
    assert_eq!(made.outcome, CreateKind::Created);
    // The same key again finds the comment.
    let again = comment
        .run(&client, &retry_of(&first, 3, None))
        .await
        .unwrap();
    assert_eq!(again.outcome, CreateKind::Found);
    assert_eq!(again.id, made.id);
    assert_eq!(fake.count(FakeOp::AddComment), 1);
}

#[tokio::test(start_paused = true)]
async fn key_is_stable_across_attempts() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let input = create_input();
    let first = first_ctx();
    let later = retry_of(&first, 4, None);
    assert_eq!(
        first.idempotency_key().unwrap().as_str(),
        later.idempotency_key().unwrap().as_str()
    );
    let made = input.run(&client, &first).await.unwrap();
    let found = input.run(&client, &later).await.unwrap();
    assert_eq!(made.id, found.id);
    // The issue has the marker of the activity key.
    let key =
        autumn_jira::IdempotencyKey::parse(first.idempotency_key().unwrap().as_str()).unwrap();
    let issue = fake.issue(made.key.as_str()).unwrap();
    let labels = issue["fields"]["labels"].as_array().unwrap();
    assert!(
        labels.contains(&json!(key.marker_label("autumn-idem-"))),
        "{labels:?}"
    );
    // Another activity key makes another issue.
    let other = input.run(&client, &first_ctx()).await.unwrap();
    assert_ne!(other.id, made.id);
}

#[tokio::test(start_paused = true)]
async fn create_with_a_route_tracks_the_issue_in_one_call() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let mut input = create_input();
    let route = SignalRoute::new("incident_flow", "incident:A-1").unwrap();
    input.route = Some(route.clone());
    let made = input.run(&client, &first_ctx()).await.unwrap();
    let issue = fake.issue(made.key.as_str()).unwrap();
    let labels = issue["fields"]["labels"].as_array().unwrap();
    assert!(labels.contains(&json!(TRACKED_LABEL)), "{labels:?}");
    assert!(labels.contains(&json!("ops")), "{labels:?}");
    let property = &issue["properties"][ROUTES_PROPERTY];
    assert_eq!(
        property,
        &json!({"routes": [route.property_entry(&route_key())]})
    );
    assert_eq!(fake.count(FakeOp::SetProperty), 0, "no second call");

    // A route with no key in the state is a config error.
    let failure = input
        .run(&client, &ActivityContext::new_test())
        .await
        .unwrap_err();
    assert_eq!(failure.error_type, CONFIG_ERROR);
    assert!(failure.non_retryable);
}

#[tokio::test(start_paused = true)]
async fn reads_and_idempotent_writes() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let ctx = first_ctx();
    let mut keys = Vec::new();
    for n in 0..3 {
        keys.push(human_issue(&fake, &format!("Issue {n}")).await);
    }
    // Get.
    let summary = GetIssueInput::new(key_ref(&keys[0]))
        .run(&client, &ctx)
        .await
        .unwrap();
    assert_eq!(summary.key, keys[0]);
    assert_eq!(summary.status.as_ref().unwrap().name, "To Do");

    // Search: one page for each call.
    let mut search = SearchPageInput::new("project = OPS ORDER BY key ASC", 2);
    let page = search.run(&client, &ctx).await.unwrap();
    assert_eq!(page.issues.len(), 2);
    search.token = page.next_token.clone();
    assert!(search.token.is_some());
    let last = search.run(&client, &ctx).await.unwrap();
    assert_eq!(last.issues.len(), 1);
    assert_eq!(last.next_token, None);
    search.token = Some(String::new());
    let failure = search.run(&client, &ctx).await.unwrap_err();
    assert_eq!(failure.error_type, "jira.invalid_input");

    // Edit.
    let mut edit = EditIssueInput::new(key_ref(&keys[1]));
    edit.set_summary = Some("New title".into());
    edit.add_labels = vec!["triage".into()];
    edit.notify_users = Some(false);
    edit.run(&client, &ctx).await.unwrap();
    let issue = fake.issue(keys[1].as_str()).unwrap();
    assert_eq!(issue["fields"]["summary"], json!("New title"));
    edit.add_labels.clear();
    edit.remove_labels = vec!["triage".into()];
    edit.run(&client, &ctx).await.unwrap();
    let issue = fake.issue(keys[1].as_str()).unwrap();
    assert_eq!(issue["fields"]["labels"], json!([]));

    // Transition: moved, then already there.
    let transition = TransitionInput::new(key_ref(&keys[2]), StatusTarget::in_progress());
    let moved = transition.run(&client, &ctx).await.unwrap();
    assert_eq!(moved.outcome, TransitionKind::Moved);
    assert!(moved.via.is_some());
    assert_eq!(moved.status.name, "In Progress");
    let again = transition.run(&client, &ctx).await.unwrap();
    assert_eq!(again.outcome, TransitionKind::AlreadyThere);
    assert_eq!(again.via, None);
    let mut to_done = TransitionInput::new(key_ref(&keys[2]), StatusTarget::done());
    to_done
        .fields
        .insert("resolution".into(), json!({"name": "Done"}));
    assert_eq!(
        to_done.run(&client, &ctx).await.unwrap().status.name,
        "Done"
    );

    // Delete: deleted; then 404 after a "maybe sent" attempt is gone.
    let delete = DeleteIssueInput::new(key_ref(&keys[0]));
    let deleted = delete.run(&client, &ctx).await.unwrap();
    assert_eq!(deleted.outcome, DeleteKind::Deleted);
    let lost_reply = failure_for(&JiraError::Upstream {
        op: Op::DeleteIssue,
        status: 503,
    })
    .into_error_payload();
    let gone = delete
        .run(&client, &retry_of(&ctx, 2, Some(lost_reply)))
        .await
        .unwrap();
    assert_eq!(gone.outcome, DeleteKind::GoneAfterRetry);
    // A 404 on attempt 1 is a typed failure.
    let missing = delete.run(&client, &ctx).await.unwrap_err();
    assert_eq!(missing.error_type, "jira.not_found_or_forbidden");
    assert!(missing.non_retryable);
}

#[tokio::test(start_paused = true)]
async fn handler_call_parses_typed_failure() {
    let fake = FakeJira::new();
    fake.fault(FakeOp::CreateIssue, Fault::Status(400));
    let ctx = ActivityContext::new_test_with_state(state(&fake));
    let input = serde_json::to_value(create_input()).unwrap();
    let payload = (jira_create_issue_info().handler)(&ctx, input)
        .await
        .unwrap_err();
    let failure = parse_typed_payload(&payload).expect("a typed failure");
    assert_eq!(failure.error_type, "jira.validation");
    assert!(failure.non_retryable);
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    assert_no_secrets(&payload);
    // A good call through the handler.
    let ok =
        (jira_create_issue_info().handler)(&ctx, serde_json::to_value(create_input()).unwrap())
            .await
            .unwrap();
    let output: CreateIssueOutput = serde_json::from_value(ok).unwrap();
    assert_eq!(output.outcome, CreateKind::Created);
}

/// Each activity with no state gives `jira.config`.
#[tokio::test]
async fn every_activity_without_state_is_config() {
    let issue = json!("OPS-1");
    let route = json!({"workflow_name": "w", "workflow_id": "1"});
    let inputs: Vec<(ActivityInfo, Value)> = activities()
        .into_iter()
        .map(|info| {
            let input = match info.name {
                "jira_create_issue" => serde_json::to_value(create_input()).unwrap(),
                "jira_search_page" => json!({"jql": "project = OPS", "max_results": 10}),
                "jira_transition_to_status" => json!({"issue": issue, "target": {"name": "Done"}}),
                "jira_add_comment" => json!({"issue": issue, "text": "x"}),
                "jira_watch_issue" => json!({"issue": issue, "route": route}),
                _ => json!({"issue": issue}),
            };
            (info, input)
        })
        .collect();
    assert_eq!(inputs.len(), 8);
    for (info, input) in inputs {
        let payload = (info.handler)(&ActivityContext::new_test(), input)
            .await
            .unwrap_err();
        let failure = parse_typed_payload(&payload).expect("a typed failure");
        assert_eq!(failure.error_type, CONFIG_ERROR, "{}", info.name);
        assert!(failure.non_retryable, "{}", info.name);
    }
}

// ------------------------------------------------------------ AC35

#[tokio::test(start_paused = true)]
async fn watch_is_idempotent() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let key = human_issue(&fake, "A person made it").await;
    let route = SignalRoute::new("incident_flow", "incident:A-7").unwrap();
    let watch = WatchIssueInput::new(key_ref(&key), route.clone());
    let ctx = first_ctx();

    let first = watch.run(&client, &ctx).await.unwrap();
    assert_eq!(first.issue.key, key);
    assert_eq!(first.issue.status.as_ref().unwrap().name, "To Do");
    assert!(first.issue.labels.contains(&TRACKED_LABEL.to_owned()));
    let puts = (
        fake.count(FakeOp::SetProperty),
        fake.count(FakeOp::EditIssue),
    );
    assert_eq!(puts, (1, 1));

    // The second call changes nothing and sends no PUT.
    let second = watch.run(&client, &ctx).await.unwrap();
    assert_eq!(second, first);
    assert_eq!(
        (
            fake.count(FakeOp::SetProperty),
            fake.count(FakeOp::EditIssue)
        ),
        puts
    );
    let issue = fake.issue(key.as_str()).unwrap();
    let labels = issue["fields"]["labels"].as_array().unwrap();
    assert_eq!(
        labels
            .iter()
            .filter(|l| *l == &json!(TRACKED_LABEL))
            .count(),
        1
    );
    assert_eq!(
        issue["properties"][ROUTES_PROPERTY],
        json!({"routes": [route.property_entry(&route_key())]})
    );

    // A second route merges. The first route stays.
    let other = SignalRoute::new("audit_flow", "audit:9").unwrap();
    WatchIssueInput::new(key_ref(&key), other.clone())
        .run(&client, &ctx)
        .await
        .unwrap();
    let issue = fake.issue(key.as_str()).unwrap();
    assert_eq!(
        issue["properties"][ROUTES_PROPERTY],
        json!({"routes": [route.property_entry(&route_key()), other.property_entry(&route_key())]})
    );
    // A stale tag of the same route is replaced, not kept twice.
    client
        .set_issue_property(
            key_ref(&key),
            ROUTES_PROPERTY,
            &json!({"routes": [{"wf": "incident_flow", "id": "incident:A-7", "tag": "0".repeat(32)}]}),
        )
        .await
        .unwrap();
    watch.run(&client, &ctx).await.unwrap();
    let issue = fake.issue(key.as_str()).unwrap();
    assert_eq!(
        issue["properties"][ROUTES_PROPERTY],
        json!({"routes": [route.property_entry(&route_key())]})
    );

    // No route key in the state: a config error.
    let failure = watch
        .run(&client, &ActivityContext::new_test())
        .await
        .unwrap_err();
    assert_eq!(failure.error_type, CONFIG_ERROR);
}

// ------------------------------------------------------------ history

/// The input of the history workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TourInput {
    create: CreateIssueInput,
}

/// A workflow that runs each of the eight activities once.
#[workflow]
async fn tour(ctx: &WorkflowContext, input: TourInput) -> Result<Value, WorkflowFailure> {
    let fail =
        |e: autumn_harvest::HarvestError| WorkflowFailure::new("test.activity", e.to_string());
    let queue = ctx.queue_name().to_owned();
    let created: CreateIssueOutput = ctx
        .execute_activity_with_opts(
            &jira_create_issue_info(),
            &input.create,
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let issue = IssueRef::Id(created.id.clone());
    let got: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_get_issue_info(),
            GetIssueInput::new(issue.clone()),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let page: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_search_page_info(),
            SearchPageInput::new("project = OPS", 10),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let mut edit = EditIssueInput::new(issue.clone());
    edit.add_labels = vec!["seen".into()];
    let edited: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_edit_issue_info(),
            edit,
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let moved: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_transition_to_status_info(),
            TransitionInput::new(issue.clone(), StatusTarget::in_progress()),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let comment: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_add_comment_info(),
            AddCommentInput::new(issue.clone(), "On it"),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let route = SignalRoute::new("tour", "tour-1")
        .map_err(|e| WorkflowFailure::new("test.route", e.to_string()))?;
    let watched: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_watch_issue_info(),
            WatchIssueInput::new(issue.clone(), route),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    let deleted: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_delete_issue_info(),
            DeleteIssueInput::new(issue),
            Some(&queue),
            None,
            None,
        )
        .await
        .map_err(fail)?;
    Ok(json!({
        "created": created, "got": got, "page": page, "edited": edited, "moved": moved,
        "comment": comment, "watched": watched, "deleted": deleted
    }))
}

/// Runs `future` on the multi-thread runtime from a sync mock.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

/// A mock that runs the real activity handler over the fake.
fn real(
    info: &ActivityInfo,
    fake: &FakeJira,
) -> impl Fn(Value, u32) -> Result<Value, String> + Send + Sync + 'static {
    let state = state(fake);
    let handler = info.handler;
    move |input, attempt| {
        let ctx = ActivityContext::new_test_with_state(Arc::clone(&state)).with_attempt(attempt);
        block_on(handler(&ctx, input))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_secret_in_outputs_or_history() {
    let fake = FakeJira::new();
    // One lost reply on the comment: the retry finds it by its marker.
    fake.fault(FakeOp::AddComment, Fault::StatusAfterCommit(503));
    let mut sim = WorkflowSimulator::new(tour_info().handler);
    for info in activities() {
        let name = info.name;
        sim = sim
            .with_activity_info(&info)
            .with_retry_policy(name, RetryPolicy::fixed(3, Duration::from_millis(100)))
            .mock_activity_with_attempt(name, real(&info, &fake));
    }
    let mut create = create_input();
    create.settle_secs = Some(1);
    let result = sim
        .run(serde_json::to_value(TourInput { create }).unwrap())
        .await;
    let output = result.final_output.clone().expect("the tour ends");
    assert_eq!(output["created"]["outcome"], json!("created"));
    assert_eq!(output["deleted"]["outcome"], json!("deleted"));
    assert_eq!(fake.count(FakeOp::AddComment), 1, "one comment POST");
    let history = serde_json::to_string(&result.history).unwrap();
    assert_no_secrets(&history);
    assert_no_secrets(&output.to_string());
    assert!(history.contains("jira_watch_issue"), "{history}");
}

// ------------------------------------------------------------ review fixes

/// R5-spec-proof-1: a failed pre-lookup is a read. It cannot have run the
/// write, so the next attempt sends the write.
#[tokio::test(start_paused = true)]
async fn a_failed_pre_lookup_is_not_sent() {
    for comment in [false, true] {
        let fake = FakeJira::new();
        let client = durable(&fake);
        let key = human_issue(&fake, "Printer").await;
        let first = first_ctx();
        // The durable client sends a read two times. Both fail.
        let lookup = if comment {
            FakeOp::GetComments
        } else {
            FakeOp::Search
        };
        fake.fault(lookup, Fault::Status(503));
        fake.fault(lookup, Fault::Status(503));
        let failure = if comment {
            AddCommentInput::new(key_ref(&key), "Fixed it")
                .run(&client, &first)
                .await
                .unwrap_err()
        } else {
            create_input().run(&client, &first).await.unwrap_err()
        };
        assert!(!failure.non_retryable, "{failure:?}");
        assert_eq!(
            failure.details.as_ref().and_then(|d| d.get("sent")),
            Some(&json!(false)),
            "{failure:?}"
        );
        let second = retry_of(&first, 2, Some(failure.into_error_payload()));
        assert_eq!(
            autumn_jira_harvest::previous_attempt(&second),
            autumn_jira::policy::Previous::NotSent
        );
        if comment {
            let made = AddCommentInput::new(key_ref(&key), "Fixed it")
                .run(&client, &second)
                .await
                .unwrap();
            assert_eq!(made.outcome, CreateKind::Created);
            assert_eq!(fake.count(FakeOp::AddComment), 1);
        } else {
            let made = create_input().run(&client, &second).await.unwrap();
            assert_eq!(made.outcome, CreateKind::Created);
            // The person's issue and this one.
            assert_eq!(fake.count(FakeOp::CreateIssue), 2);
        }
    }
}

/// R1-write-safety-6: the `sent` flag of a failure tells only the last
/// client attempt. An earlier client attempt of the same activity attempt
/// can have deleted the issue. So a 404 on any later attempt is gone.
#[tokio::test(start_paused = true)]
async fn a_delete_404_after_any_failed_attempt_is_gone() {
    let fake = FakeJira::new();
    let client = durable(&fake);
    let key = human_issue(&fake, "Old").await;
    let delete = DeleteIssueInput::new(key_ref(&key));
    let ctx = first_ctx();
    delete.run(&client, &ctx).await.unwrap();
    let store_error = failure_for(&JiraError::Network {
        op: Op::DeleteIssue,
        code: "store_error",
    })
    .into_error_payload();
    let rate_limited = failure_for(&JiraError::RateLimited {
        op: Op::DeleteIssue,
        retry_after: None,
        reason: None,
    })
    .into_error_payload();
    for (attempt, previous) in [(2, store_error), (3, rate_limited)] {
        let gone = delete
            .run(&client, &retry_of(&ctx, attempt, Some(previous)))
            .await
            .unwrap();
        assert_eq!(gone.outcome, DeleteKind::GoneAfterRetry, "{attempt}");
    }
}

/// R1-write-safety-9 and the B2 budget: the marker lookup of
/// `jira_add_comment` reads at most `COMMENT_LOOKUP_PAGES` pages. At the cap
/// it fails closed: the lookup gives no "not found", so no attempt sends the
/// comment.
#[tokio::test(start_paused = true)]
async fn the_comment_lookup_reads_at_most_its_page_cap() {
    let fake = FakeJira::new();
    let durable = durable(&fake);
    let key = human_issue(&fake, "Printer").await;
    // 100 comments for each page. The person adds one page more than the cap.
    let person = client(&fake, "acme");
    let notes = 100 * (COMMENT_LOOKUP_PAGES + 1);
    for n in 0..notes {
        person
            .add_comment(key_ref(&key), &autumn_jira::Adf::text(&format!("note {n}")))
            .await
            .unwrap();
    }
    let posts = fake.count(FakeOp::AddComment);
    let first = first_ctx();
    let comment = AddCommentInput::new(key_ref(&key), "Fixed it");
    // The send path: the lookup before the send stops at the cap.
    let failure = comment.run(&durable, &first).await.unwrap_err();
    assert_ne!(failure.error_type, OUTCOME_UNKNOWN_ERROR, "{failure:?}");
    let cap = usize::try_from(COMMENT_LOOKUP_PAGES).unwrap();
    assert_eq!(fake.count(FakeOp::GetComments), cap);
    // The lookup-only path stops at the cap too.
    let ctx = retry_of(&first, 2, None);
    let failure = comment.run(&durable, &ctx).await.unwrap_err();
    assert!(failure.error_type.starts_with(ERROR_PREFIX), "{failure:?}");
    assert_eq!(fake.count(FakeOp::GetComments), 2 * cap);
    assert_eq!(fake.count(FakeOp::AddComment), posts, "no comment is sent");
}

/// R2A-write-safety-3: on attempt 1 no earlier attempt can have sent the
/// comment. So a busy issue with more comments than the lookup cap still
/// gets the comment. Only a lookup after a possible send fails closed.
#[tokio::test(start_paused = true)]
async fn add_comment_on_an_issue_with_301_comments_works_on_attempt_1() {
    let fake = FakeJira::new();
    let durable = durable(&fake);
    let key = human_issue(&fake, "Printer").await;
    let person = client(&fake, "acme");
    for n in 0..=(100 * COMMENT_LOOKUP_PAGES) {
        person
            .add_comment(key_ref(&key), &autumn_jira::Adf::text(&format!("note {n}")))
            .await
            .unwrap();
    }
    let posts = fake.count(FakeOp::AddComment);
    let reads = fake.count(FakeOp::GetComments);
    let first = first_ctx();
    let comment = AddCommentInput::new(key_ref(&key), "Fixed it");
    let made = comment.run(&durable, &first).await.unwrap();
    assert_eq!(made.outcome, CreateKind::Created);
    assert_eq!(fake.count(FakeOp::AddComment), posts + 1);
    assert_eq!(fake.count(FakeOp::GetComments), reads, "no pre-lookup");

    // After a failure that tells "not sent", the next attempt also sends
    // with no pre-lookup.
    let not_sent = failure_for(&JiraError::Network {
        op: Op::AddComment,
        code: "store_error",
    })
    .into_error_payload();
    let other = first_ctx();
    let ctx = retry_of(&other, 2, Some(not_sent));
    let made = comment.run(&durable, &ctx).await.unwrap();
    assert_eq!(made.outcome, CreateKind::Created);
    assert_eq!(fake.count(FakeOp::AddComment), posts + 2);
    assert_eq!(fake.count(FakeOp::GetComments), reads, "no pre-lookup");
}

/// The core gives `resume_transition_to_status` for a host retry
/// (R2A-write-safety-1). After an attempt that can have run, the activity
/// reads the status and sends no transition.
#[tokio::test(start_paused = true)]
async fn a_transition_after_a_maybe_sent_attempt_sends_nothing() {
    let fake = FakeJira::new();
    let durable = durable(&fake);
    let key = human_issue(&fake, "Printer").await;
    let first = first_ctx();
    let transition = TransitionInput::new(key_ref(&key), StatusTarget::in_progress());
    let pending = failure_for(&JiraError::OutcomeUnknown {
        op: Op::DoTransition,
        cause: UnknownCause::ServerError(503),
        marker: Some(MarkerRef::label("autumn-idem-x".into())),
    })
    .into_error_payload();
    // A crash or a timeout (no typed failure), and "maybe sent".
    for (attempt, previous) in [(2, None), (3, Some(pending))] {
        let ctx = retry_of(&first, attempt, previous);
        let failure = transition.run(&durable, &ctx).await.unwrap_err();
        assert_eq!(failure.error_type, OUTCOME_UNKNOWN_ERROR, "{attempt}");
        assert!(failure.non_retryable, "{attempt}");
    }
    assert_eq!(fake.count(FakeOp::DoTransition), 0, "no transition is sent");

    // At the target, the resumed attempt gives AlreadyThere.
    fake.human_transition(key.as_str(), "In Progress", autumn_jira_fake::ANN)
        .unwrap();
    let ctx = retry_of(&first, 2, None);
    let output = transition.run(&durable, &ctx).await.unwrap();
    assert_eq!(output.outcome, TransitionKind::AlreadyThere);
    assert_eq!(fake.count(FakeOp::DoTransition), 0);

    // After a failure that tells "not sent", the attempt moves the issue.
    let back = TransitionInput::new(key_ref(&key), StatusTarget::todo());
    let not_sent = failure_for(&JiraError::Network {
        op: Op::GetTransitions,
        code: "store_error",
    })
    .into_error_payload();
    let ctx = retry_of(&first, 2, Some(not_sent));
    let output = back.run(&durable, &ctx).await.unwrap();
    assert_eq!(output.outcome, TransitionKind::Moved);
    assert_eq!(fake.count(FakeOp::DoTransition), 1);
}
