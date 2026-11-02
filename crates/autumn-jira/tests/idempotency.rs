//! Idempotent create, idempotent comment and the checked transition
//! (AC16, AC17).

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use autumn_jira::client::{CommentOutcome, CreateOutcome, MarkerRef, TransitionOutcome};
use autumn_jira::error::{Op, UnknownCause};
use autumn_jira::request::IssueTypeRef;
use autumn_jira::{Adf, IdempotencyKey, JiraClient, JiraError, NewIssue, ProjectKey, StatusTarget};
use autumn_jira_fake::{FakeJira, FakeOp, Fault};
use common::{Scripted, api_token_client, direct, fast_settings, json_reply};
use proptest::prelude::*;
use serde_json::{Value, json};

const PREFIX: &str = "autumn-idem-";

fn ops() -> ProjectKey {
    ProjectKey::parse("OPS").unwrap()
}

fn task(summary: &str) -> NewIssue {
    NewIssue::new(ops(), IssueTypeRef::Name("Task".to_owned()), summary)
}

fn key(n: u32) -> IdempotencyKey {
    IdempotencyKey::derive(&["alert", &n.to_string()])
}

/// The issues of the fake that carry the marker label of `key`.
fn marked(fake: &FakeJira, key: &IdempotencyKey) -> Vec<Value> {
    let label = key.marker_label(PREFIX);
    fake.issues()
        .into_iter()
        .filter(|i| {
            i["fields"]["labels"]
                .as_array()
                .is_some_and(|l| l.iter().any(|v| v == &json!(label)))
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn marker_goes_in_the_create_body() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(1);
    let issue = task("Disk full").labels(["alert"]);
    let outcome = client.create_issue_idempotent(&issue, &k).await.unwrap();
    let CreateOutcome::Created(created) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    let stored = fake.issue(created.key.as_str()).unwrap();
    let labels = stored["fields"]["labels"].as_array().unwrap();
    assert!(labels.contains(&json!("alert")));
    assert!(labels.contains(&json!(k.marker_label(PREFIX))));
    let property = client
        .get_issue_property(&created.key, "autumn-jira")
        .await
        .unwrap();
    assert_eq!(property, Some(json!({"idempotencyKey": k.as_str()})));
    // The marker label has 26 lower base32 chars after the prefix.
    let label = k.marker_label(PREFIX);
    assert_eq!(label.len(), PREFIX.len() + 26);
}

#[tokio::test(start_paused = true)]
async fn prelookup_hit_sends_no_post() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(2);
    let first = client
        .create_issue_idempotent(&task("a"), &k)
        .await
        .unwrap();
    let second = client
        .create_issue_idempotent(&task("a"), &k)
        .await
        .unwrap();
    let (CreateOutcome::Created(c), CreateOutcome::Found(f)) = (first, second) else {
        panic!("wrong outcomes");
    };
    assert_eq!((c.id, c.key), (f.id, f.key));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    // The lookup JQL: project and label, each value quoted.
    let search = fake
        .requests()
        .into_iter()
        .find(|r| r.path.ends_with("/search/jql"))
        .unwrap();
    assert_eq!(search.method, "POST");
}

#[tokio::test(start_paused = true)]
async fn lost_reply_is_found() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(3);
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let outcome = client
        .create_issue_idempotent(&task("lost"), &k)
        .await
        .unwrap();
    assert!(matches!(outcome, CreateOutcome::Found(ref r) if r.key.as_str() == "OPS-1"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    assert_eq!(marked(&fake, &k).len(), 1);

    // A 2xx that does not decode also goes to the lookup.
    fake.fault(FakeOp::CreateIssue, Fault::BadJson);
    let k2 = key(30);
    let outcome = client
        .create_issue_idempotent(&task("bad json"), &k2)
        .await
        .unwrap();
    assert!(matches!(outcome, CreateOutcome::Found(_)));
    // A reset after the commit too.
    fake.fault(FakeOp::CreateIssue, Fault::NetworkAfterCommit);
    let k3 = key(31);
    let outcome = client
        .create_issue_idempotent(&task("reset"), &k3)
        .await
        .unwrap();
    assert!(matches!(outcome, CreateOutcome::Found(_)));
    assert_eq!(fake.count(FakeOp::CreateIssue), 3);
    // A 429 on a marked write is retried: the marker makes it safe (A1).
    fake.rate_limit(FakeOp::CreateIssue, 1, 1);
    let k4 = key(32);
    let outcome = client
        .create_issue_idempotent(&task("429"), &k4)
        .await
        .unwrap();
    assert!(matches!(outcome, CreateOutcome::Created(_)));
    assert_eq!(fake.count(FakeOp::CreateIssue), 5);
}

#[tokio::test(start_paused = true)]
async fn lag_beyond_lookups_is_outcome_unknown() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(4);
    fake.index_lag(Duration::from_secs(60));
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let err = client
        .create_issue_idempotent(&task("lag"), &k)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        JiraError::OutcomeUnknown {
            op: Op::CreateIssue,
            cause: UnknownCause::ServerError(503),
            marker: Some(MarkerRef::label(k.marker_label(PREFIX))),
        }
    );
    assert_eq!(err.code(), "outcome_pending");
    assert!(err.is_retryable());
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    // One pre-lookup and three lookups.
    assert_eq!(fake.count(FakeOp::Search), 4);
    assert_eq!(marked(&fake, &k).len(), 1);
    // After the lag, a lookup finds it.
    tokio::time::sleep(Duration::from_secs(61)).await;
    let found = client.find_by_idempotency_key(&ops(), &k).await.unwrap();
    assert_eq!(found.unwrap().key.as_str(), "OPS-1");
}

#[tokio::test(start_paused = true)]
async fn label_only_hit_is_skipped() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(5);
    // A person copied the marker label to another issue: no property.
    let copy = task("copy").labels([k.marker_label(PREFIX)]);
    client.create_issue(&copy).await.unwrap();
    assert_eq!(
        client.find_by_idempotency_key(&ops(), &k).await.unwrap(),
        None
    );
    let outcome = client
        .create_issue_idempotent(&task("real"), &k)
        .await
        .unwrap();
    let CreateOutcome::Created(created) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(created.key.as_str(), "OPS-2");
    let found = client.find_by_idempotency_key(&ops(), &k).await.unwrap();
    assert_eq!(found.unwrap().key.as_str(), "OPS-2");
}

#[tokio::test(start_paused = true)]
async fn two_hits_pick_the_lowest_id() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let k = key(6);
    let marked_issue = |summary: &str| {
        task(summary)
            .labels([k.marker_label(PREFIX)])
            .property("autumn-jira", json!({"idempotencyKey": k.as_str()}))
    };
    let first = client.create_issue(&marked_issue("one")).await.unwrap();
    client.create_issue(&marked_issue("two")).await.unwrap();
    let found = client
        .find_by_idempotency_key(&ops(), &k)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, first.id);
}

#[tokio::test(start_paused = true)]
async fn failed_lookup_is_not_not_found() {
    let fake = FakeJira::new();
    let posted = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&posted);
    let backend = Scripted::new(Arc::new(direct(Arc::new(fake.clone()))), move |r, _| {
        if r.op == Op::CreateIssue {
            flag.store(true, Ordering::SeqCst);
            return None;
        }
        (r.op == Op::Search && flag.load(Ordering::SeqCst))
            .then(|| Ok(json_reply(500, &json!({"errorMessages": ["down"]}))))
    });
    let client = JiraClient::new(backend.clone()).with_settings(fast_settings());
    let k = key(7);
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let err = client
        .create_issue_idempotent(&task("x"), &k)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, JiraError::OutcomeUnknown { marker: Some(m), .. } if m.label.is_some()),
        "{err:?}"
    );
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    // A failed search is an error, never "not found".
    let lookup = client
        .find_by_idempotency_key(&ops(), &k)
        .await
        .unwrap_err();
    assert!(matches!(
        lookup,
        JiraError::Upstream {
            op: Op::Search,
            status: 500
        }
    ));
    assert_eq!(
        backend
            .requests()
            .iter()
            .filter(|r| r.op == Op::CreateIssue)
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn labels_not_on_screen_has_a_fix() {
    let fake = FakeJira::new();
    fake.labels_not_on_screen("OPS");
    let client = api_token_client(&fake);
    let err = client
        .create_issue_idempotent(&task("x"), &key(8))
        .await
        .unwrap_err();
    assert_eq!(err, JiraError::LabelsNotOnScreen { project: ops() });
    assert!(err.to_string().contains("add the Labels field"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    // A plain create with no labels still works.
    client.create_issue(&task("plain")).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn lost_comment_reply_is_found() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let issue = client.create_issue(&task("c")).await.unwrap().key;
    let k = key(9);
    fake.fault(FakeOp::AddComment, Fault::StatusAfterCommit(503));
    let outcome = client
        .add_comment_idempotent(&issue, &Adf::text("Fixed"), &k)
        .await
        .unwrap();
    let CommentOutcome::Found(comment) = outcome else {
        panic!("{outcome:?}");
    };
    let text = comment.body.unwrap().to_plain_text();
    assert_eq!(text, format!("Fixed\n{}", k.comment_marker()));
    assert_eq!(fake.count(FakeOp::AddComment), 1);
    // The lookup reads the newest comments first.
    let lookup = fake
        .requests()
        .into_iter()
        .rfind(|r| r.method == "GET" && r.path.ends_with("/comment"))
        .unwrap();
    assert!(
        lookup.query.contains("orderBy=-created"),
        "{}",
        lookup.query
    );
    assert_eq!(client.all_comments(&issue).await.unwrap().len(), 1);

    // A lookup that finds nothing: outcome unknown, with the marker.
    fake.fault(FakeOp::AddComment, Fault::Status(500));
    let err = client
        .add_comment_idempotent(&issue, &Adf::text("Again"), &key(10))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        JiraError::OutcomeUnknown {
            op: Op::AddComment,
            cause: UnknownCause::ServerError(500),
            marker: Some(MarkerRef::comment(key(10).comment_marker())),
        }
    );
    assert_eq!(fake.count(FakeOp::AddComment), 2);
}

#[tokio::test(start_paused = true)]
async fn transition_after_lost_reply_is_already_there() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let issue = client.create_issue(&task("t")).await.unwrap().key;
    fake.fault(FakeOp::DoTransition, Fault::StatusAfterCommit(503));
    let outcome = client
        .transition_to_status(&issue, &StatusTarget::in_progress())
        .await
        .unwrap();
    assert!(
        matches!(&outcome, TransitionOutcome::AlreadyThere(s) if s.name == "In Progress"),
        "{outcome:?}"
    );
    assert_eq!(fake.count(FakeOp::DoTransition), 1);
}

/// One fault of the create POST.
const fn create_fault(n: u8) -> Fault {
    match n {
        0 => Fault::Status(500),
        1 => Fault::StatusAfterCommit(503),
        2 => Fault::Network,
        3 => Fault::NetworkAfterCommit,
        4 => Fault::Timeout,
        5 => Fault::BadJson,
        _ => Fault::TimeoutThenCommit(Duration::from_millis(150)),
    }
}

/// One fault of the search.
const fn search_fault(n: u8) -> Fault {
    match n {
        0 => Fault::Status(500),
        1 => Fault::Network,
        _ => Fault::Status(503),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Random fault schedules, a paused clock, and a host that retries the
    /// whole call. The host follows the default policy: after an unknown
    /// outcome it only looks up. At most one issue per key, always.
    #[test]
    fn chaos_at_most_one_issue_per_key(
        create in proptest::collection::vec(0_u8..7, 0..4),
        search in proptest::collection::vec(0_u8..3, 0..4),
        limits in 0_usize..3,
        lag_ms in 0_u64..1_000,
        calls in 1_usize..4,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(async {
            let fake = FakeJira::new();
            for n in &create {
                fake.fault(FakeOp::CreateIssue, create_fault(*n));
            }
            for n in &search {
                fake.fault(FakeOp::Search, search_fault(*n));
            }
            fake.rate_limit(FakeOp::CreateIssue, limits, 1);
            fake.index_lag(Duration::from_millis(lag_ms));
            let client = api_token_client(&fake);
            let k = key(99);
            let mut maybe_sent = false;
            let mut done = false;
            for _ in 0..calls {
                if maybe_sent {
                    // The default host policy: a lookup only.
                    if client.find_by_idempotency_key(&ops(), &k).await.is_ok_and(|f| f.is_some()) {
                        done = true;
                        break;
                    }
                    continue;
                }
                match client.create_issue_idempotent(&task("chaos"), &k).await {
                    Ok(_) => {
                        done = true;
                        break;
                    }
                    Err(JiraError::OutcomeUnknown { .. }) => maybe_sent = true,
                    Err(_) => {}
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            let issues = marked(&fake, &k);
            prop_assert!(issues.len() <= 1, "{} issues", issues.len());
            if done {
                prop_assert_eq!(issues.len(), 1);
            }
            Ok(())
        })?;
    }
}

#[tokio::test(start_paused = true)]
async fn comment_lookup_finds_marker_beyond_100() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let issue = client.create_issue(&task("busy")).await.unwrap().key;
    let k = key(40);
    let first = client
        .add_comment_idempotent(&issue, &Adf::text("Escalated"), &k)
        .await
        .unwrap();
    assert!(matches!(first, CommentOutcome::Created(_)), "{first:?}");
    // A bot writes 120 newer comments.
    for n in 0..120 {
        fake.human_comment(issue.as_str(), &format!("ci note {n}"), "ann")
            .unwrap();
    }
    let found = client
        .find_comment_by_idempotency_key(&issue, &k)
        .await
        .unwrap();
    assert!(found.is_some(), "the marker is older than the 100 newest");
    let again = client
        .add_comment_idempotent(&issue, &Adf::text("Escalated"), &k)
        .await
        .unwrap();
    assert!(matches!(again, CommentOutcome::Found(_)), "{again:?}");
    assert_eq!(fake.count(FakeOp::AddComment), 1);
}

#[tokio::test(start_paused = true)]
async fn a_marker_from_another_author_is_not_found() {
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    let issue = client.create_issue(&task("forged")).await.unwrap().key;
    let k = key(41);
    // A person posts the marker of the key before the workflow runs.
    fake.human_comment(issue.as_str(), &k.comment_marker(), "ann")
        .unwrap();
    // A person quotes the marker inside a line.
    fake.human_comment(
        issue.as_str(),
        &format!("> {} was here", k.comment_marker()),
        "ann",
    )
    .unwrap();
    assert_eq!(
        client
            .find_comment_by_idempotency_key(&issue, &k)
            .await
            .unwrap(),
        None
    );
    let outcome = client
        .add_comment_idempotent(&issue, &Adf::text("Escalated"), &k)
        .await
        .unwrap();
    assert!(matches!(outcome, CommentOutcome::Created(_)), "{outcome:?}");
    assert_eq!(fake.count(FakeOp::AddComment), 1);
}
