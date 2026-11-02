//! Retry policy and unsafe writes (AC14, AC15). Every case runs on a
//! paused tokio clock: no wall-clock sleep (F37).

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::backend::BackendError;
use autumn_jira::error::{Op, UnknownCause};
use autumn_jira::ids::TransitionId;
use autumn_jira::request::{IssueTypeRef, TransitionFields};
use autumn_jira::san_mateo::MemoryTransport;
use autumn_jira::san_mateo::clock::Entropy;
use autumn_jira::san_mateo::transport::HttpResponse;
use autumn_jira::{Adf, IssueKey, JiraClient, JiraError, NewIssue, ProjectKey};
use autumn_jira_fake::{FakeJira, FakeOp, Fault};
use common::{Scripted, api_token_client, auth, direct, fast_settings, site};
use tokio::time::Instant;

const MYSELF: &str = "https://acme.atlassian.net/rest/api/3/myself";

fn task(summary: &str) -> NewIssue {
    NewIssue::new(
        ProjectKey::parse("OPS").unwrap(),
        IssueTypeRef::Name("Task".to_owned()),
        summary,
    )
}

fn ms(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

/// An entropy that always gives the same 4 bytes.
struct Fixed(u32);

impl Entropy for Fixed {
    fn fill(&self, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.0.to_le_bytes()[i % 4];
        }
    }
}

async fn issue(fake: &FakeJira) -> IssueKey {
    api_token_client(fake)
        .create_issue(&task("x"))
        .await
        .unwrap()
        .key
}

#[tokio::test(start_paused = true)]
async fn reads_retry_server_errors() {
    let fake = FakeJira::new();
    let key = issue(&fake).await;
    let client = api_token_client(&fake);
    fake.fault(FakeOp::GetIssue, Fault::Status(503));
    fake.fault(FakeOp::GetIssue, Fault::Status(500));
    let start = Instant::now();
    client.get_issue(&key).await.unwrap();
    assert_eq!(fake.count(FakeOp::GetIssue), 3);
    // Backoff 50 ms, then 100 ms.
    assert_eq!(ms(start), 150);

    // 408, 425 and a network error are retried too.
    fake.fault(FakeOp::GetIssue, Fault::Status(408));
    fake.fault(FakeOp::GetIssue, Fault::Status(425));
    fake.fault(FakeOp::GetIssue, Fault::Network);
    fake.fault(FakeOp::GetIssue, Fault::Timeout);
    client.get_issue(&key).await.unwrap();
    assert_eq!(fake.count(FakeOp::GetIssue), 8);

    // Idempotent writes retry: an edit after a 503.
    fake.fault(FakeOp::EditIssue, Fault::Status(503));
    let edit = autumn_jira::request::IssueEdit::new().set_summary("y");
    client
        .edit_issue(&key, &edit, autumn_jira::request::EditOptions::default())
        .await
        .unwrap();
    assert_eq!(fake.count(FakeOp::EditIssue), 2);

    // At most `max_attempts` attempts, then a typed error.
    for _ in 0..6 {
        fake.fault(FakeOp::GetIssue, Fault::Status(502));
    }
    let err = client.get_issue(&key).await.unwrap_err();
    assert_eq!(
        err,
        JiraError::Upstream {
            op: Op::GetIssue,
            status: 502
        }
    );
    assert_eq!(fake.count(FakeOp::GetIssue), 13);
}

#[tokio::test(start_paused = true)]
async fn retry_after_is_the_floor() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    fake.rate_limit(FakeOp::Myself, 1, 10);
    let start = Instant::now();
    client.myself().await.unwrap();
    assert_eq!(ms(start), 10_000);
    assert_eq!(fake.count(FakeOp::Myself), 2);
}

#[tokio::test(start_paused = true)]
async fn retry_after_above_the_cap_gives_up() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    fake.rate_limit(FakeOp::Myself, 1, 60);
    let start = Instant::now();
    let err = client.myself().await.unwrap_err();
    assert!(
        matches!(&err, JiraError::RateLimited { op: Op::Myself, retry_after: Some(d), reason: Some(_) } if *d == Duration::from_secs(60)),
        "{err:?}"
    );
    assert_eq!(fake.count(FakeOp::Myself), 1);
    assert_eq!(ms(start), 0);
    assert!(err.is_retryable());
}

#[tokio::test(start_paused = true)]
async fn beta_retry_after_is_ignored() {
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        MYSELF,
        HttpResponse::new(429, "").header("Beta-Retry-After", "600"),
    );
    transport.reply(
        "GET",
        MYSELF,
        HttpResponse::json(200, &serde_json::json!({"accountId": "a"})),
    );
    let client = JiraClient::api_token(site(), auth(), Arc::new(transport.clone()))
        .with_settings(fast_settings());
    let start = Instant::now();
    client.myself().await.unwrap();
    assert_eq!(ms(start), 50);
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn jitter_uses_entropy() {
    for (raw, expected) in [(0_u32, 700_u128), (600, 1_300), (300, 1_000)] {
        let fake = FakeJira::new();
        let mut settings = fast_settings();
        settings.base_backoff_ms = 1_000;
        settings.jitter = true;
        let client = api_token_client(&fake)
            .with_settings(settings)
            .with_entropy(Arc::new(Fixed(raw)));
        fake.fault(FakeOp::Myself, Fault::Status(503));
        let start = Instant::now();
        client.myself().await.unwrap();
        assert_eq!(ms(start), expected, "raw {raw}");
    }
    // A Retry-After stays the floor under jitter.
    let fake = FakeJira::new();
    let mut settings = fast_settings();
    settings.jitter = true;
    let client = api_token_client(&fake)
        .with_settings(settings)
        .with_entropy(Arc::new(Fixed(0)));
    fake.rate_limit(FakeOp::Myself, 1, 2);
    let start = Instant::now();
    client.myself().await.unwrap();
    assert_eq!(ms(start), 2_000);
}

#[tokio::test(start_paused = true)]
async fn not_sent_transient_is_retried() {
    let fake = FakeJira::new();
    let scripted = Scripted::new(Arc::new(direct(Arc::new(fake.clone()))), |_, n| {
        (n < 2).then_some(Err(BackendError::NotSent {
            transient: true,
            code: "store_error",
        }))
    });
    let client = JiraClient::new(scripted.clone()).with_settings(fast_settings());
    client.myself().await.unwrap();
    assert_eq!(scripted.requests().len(), 3);
    assert_eq!(fake.count(FakeOp::Myself), 1);

    // A plain create is retried after "not sent": it never left.
    let scripted = Scripted::new(Arc::new(direct(Arc::new(fake.clone()))), |_, n| {
        (n == 0).then_some(Err(BackendError::NotSent {
            transient: true,
            code: "store_error",
        }))
    });
    let client = JiraClient::new(scripted.clone()).with_settings(fast_settings());
    client.create_issue(&task("ns")).await.unwrap();
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);

    // Not transient: a config error, no retry. Auth: an auth error.
    let config = Scripted::new(Arc::new(direct(Arc::new(fake.clone()))), |_, _| {
        Some(Err(BackendError::NotSent {
            transient: false,
            code: "unknown_connection",
        }))
    });
    let client = JiraClient::new(config.clone()).with_settings(fast_settings());
    assert_eq!(
        client.myself().await.unwrap_err(),
        JiraError::Config {
            code: "unknown_connection"
        }
    );
    assert_eq!(config.requests().len(), 1);
    let auth = Scripted::new(Arc::new(direct(Arc::new(fake.clone()))), |_, _| {
        Some(Err(BackendError::Auth {
            code: "connection_refresh_exhausted",
        }))
    });
    let client = JiraClient::new(auth.clone()).with_settings(fast_settings());
    assert_eq!(
        client.myself().await.unwrap_err(),
        JiraError::Auth {
            code: "connection_refresh_exhausted"
        }
    );
    assert_eq!(auth.requests().len(), 1);
    // Out of attempts after "not sent": a network error.
    let always = Scripted::new(Arc::new(direct(Arc::new(fake))), |_, _| {
        Some(Err(BackendError::NotSent {
            transient: true,
            code: "store_error",
        }))
    });
    let client = JiraClient::new(always.clone()).with_settings(fast_settings());
    assert_eq!(
        client.myself().await.unwrap_err(),
        JiraError::Network {
            op: Op::Myself,
            code: "store_error"
        }
    );
    assert_eq!(always.requests().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn plain_create_is_sent_once_after_503() {
    let fake = FakeJira::new();
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let err = api_token_client(&fake)
        .create_issue(&task("a"))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        JiraError::OutcomeUnknown {
            op: Op::CreateIssue,
            cause: UnknownCause::ServerError(503),
            marker: None
        }
    );
    assert_eq!(err.code(), "outcome_unknown");
    assert!(!err.is_retryable());
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    assert_eq!(fake.issues().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn plain_create_is_sent_once_after_a_reset() {
    let fake = FakeJira::new();
    fake.fault(FakeOp::CreateIssue, Fault::NetworkAfterCommit);
    let err = api_token_client(&fake)
        .create_issue(&task("a"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::OutcomeUnknown {
            cause: UnknownCause::Network,
            marker: None,
            ..
        }
    ));
    fake.fault(FakeOp::CreateIssue, Fault::Timeout);
    let err = api_token_client(&fake)
        .create_issue(&task("b"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::OutcomeUnknown {
            cause: UnknownCause::Timeout,
            marker: None,
            ..
        }
    ));
    assert_eq!(fake.count(FakeOp::CreateIssue), 2);
    assert_eq!(fake.issues().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn plain_create_is_not_retried_after_429() {
    let fake = FakeJira::new();
    fake.rate_limit(FakeOp::CreateIssue, 1, 1);
    let err = api_token_client(&fake)
        .create_issue(&task("a"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::RateLimited {
            op: Op::CreateIssue,
            ..
        }
    ));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    assert_eq!(fake.issues().len(), 0);
}

#[tokio::test(start_paused = true)]
async fn plain_transition_is_sent_once() {
    let fake = FakeJira::new();
    let key = issue(&fake).await;
    let client = api_token_client(&fake);
    fake.fault(FakeOp::DoTransition, Fault::StatusAfterCommit(503));
    let err = client
        .transition(
            &key,
            &TransitionId::parse("21").unwrap(),
            &TransitionFields::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::OutcomeUnknown {
            op: Op::DoTransition,
            marker: None,
            ..
        }
    ));
    assert_eq!(fake.count(FakeOp::DoTransition), 1);
    // A plain comment: the same rule.
    fake.fault(FakeOp::AddComment, Fault::StatusAfterCommit(502));
    let err = client
        .add_comment(&key, &Adf::text("hi"))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::OutcomeUnknown {
            op: Op::AddComment,
            marker: None,
            ..
        }
    ));
    assert_eq!(fake.count(FakeOp::AddComment), 1);
    assert_eq!(
        fake.snapshot()["issues"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// One planned trouble of an unsafe write.
#[derive(Debug, Clone)]
enum Trouble {
    Fault(Fault),
    RateLimit,
}

/// AC15 as a table: each unsafe write, each trigger, one request.
#[tokio::test(start_paused = true)]
async fn unsafe_writes_send_once_table() {
    let troubles = [
        Trouble::Fault(Fault::Status(500)),
        Trouble::Fault(Fault::StatusAfterCommit(503)),
        Trouble::Fault(Fault::NetworkAfterCommit),
        Trouble::Fault(Fault::Timeout),
        Trouble::RateLimit,
    ];
    for op in [
        FakeOp::CreateIssue,
        FakeOp::AddComment,
        FakeOp::DoTransition,
    ] {
        for trouble in troubles.clone() {
            let fake = FakeJira::new();
            let key = issue(&fake).await;
            let client = api_token_client(&fake);
            let before = fake.count(op);
            match trouble.clone() {
                Trouble::Fault(fault) => fake.fault(op, fault),
                Trouble::RateLimit => fake.rate_limit(op, 1, 1),
            }
            let result = match op {
                FakeOp::CreateIssue => client.create_issue(&task("t")).await.map(|_| ()),
                FakeOp::AddComment => client.add_comment(&key, &Adf::text("hi")).await.map(|_| ()),
                _ => {
                    client
                        .transition(
                            &key,
                            &TransitionId::parse("21").unwrap(),
                            &TransitionFields::default(),
                        )
                        .await
                }
            };
            let err = result.unwrap_err();
            let label = format!("{op:?} {trouble:?}: {err:?}");
            match trouble {
                Trouble::RateLimit => {
                    assert!(matches!(err, JiraError::RateLimited { .. }), "{label}");
                }
                Trouble::Fault(_) => assert!(
                    matches!(err, JiraError::OutcomeUnknown { marker: None, .. }),
                    "{label}"
                ),
            }
            assert_eq!(fake.count(op) - before, 1, "{label}");
        }
    }
}
