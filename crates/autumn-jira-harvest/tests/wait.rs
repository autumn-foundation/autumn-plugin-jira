//! The durable wait on a mock `jira_get_issue` activity (AC33, spike S2).
//!
//! Two harnesses run the wait:
//!
//! - `WorkflowTestEnv` for the cases that end with no deadline. Its
//!   `system_now()` reads the wall clock, so a deadline never comes there.
//! - The seeded `Simulation` of the same env for the deadline cases. Its
//!   `system_now()` reads the virtual clock, which moves only when a durable
//!   timer fires.
//!
//! Each history then replays with `WorkflowReplayer::new()`. That replayer
//! has the advancing clock OFF: `ctx.now()` stays at the start time, as in
//! production (W2). A wait that takes its deadline from `ctx.now()` gives
//! other commands on that replay and fails it.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::time::Duration;

use autumn_harvest::WorkflowEvent;
use autumn_harvest::WorkflowHandlerFn;
use autumn_harvest::context::WorkflowContext;
use autumn_harvest::dst::SeedPlan;
use autumn_harvest::failure::WorkflowFailure;
use autumn_harvest::prelude::workflow;
use autumn_harvest::testing::{ReplayStatus, TestRunOutcome, WorkflowReplayer, WorkflowTestEnv};
use autumn_jira::model::StatusSummary;
use autumn_jira::{IssueId, StatusTarget};
use autumn_jira_harvest::{
    GET_ISSUE, GetIssueInput, INVALID_INPUT_ERROR, IssueSignal, MAX_WAIT, SIGNAL, StatusWait,
    WaitOptions, WaitSource, await_issue_status,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The input of the test workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WaitInput {
    issue_id: IssueId,
    target: StatusTarget,
    options: WaitOptions,
}

/// A real workflow that calls the wait.
#[workflow]
async fn wait_for_issue(
    ctx: &WorkflowContext,
    input: WaitInput,
) -> Result<StatusWait, WorkflowFailure> {
    await_issue_status(ctx, &input.issue_id, &input.target, &input.options).await
}

const ISSUE: &str = "10001";
const OTHER_ISSUE: &str = "10002";

fn handler() -> WorkflowHandlerFn {
    wait_for_issue_info().handler
}

fn issue_id(id: &str) -> IssueId {
    IssueId::parse(id).unwrap()
}

fn status(id: &str, name: &str, category: &str) -> StatusSummary {
    serde_json::from_value(json!({"id": id, "name": name, "category": category})).unwrap()
}

fn in_progress() -> StatusSummary {
    status("10001", "In Progress", "indeterminate")
}

fn done() -> StatusSummary {
    status("10002", "Done", "done")
}

/// An `IssueSummary` as the mock activity returns it.
fn issue_at(status: &StatusSummary) -> Value {
    json!({
        "id": ISSUE,
        "key": "OPS-1",
        "summary": "Disk full",
        "status": status,
        "labels": ["autumn-tracked"],
        "updated": null
    })
}

fn signal(issue: &str, status: Option<StatusSummary>, timestamp_ms: Option<i64>, n: u32) -> Value {
    serde_json::to_value(IssueSignal {
        delivery_id: autumn_jira::ids::WebhookDeliveryId::parse(&format!("acme-{n}")).unwrap(),
        event: "jira:issue_updated".into(),
        issue_id: issue_id(issue),
        issue_key: None,
        status,
        timestamp_ms,
    })
    .unwrap()
}

fn input(timeout_secs: u64, poll_secs: Option<u64>) -> Value {
    let mut options = WaitOptions::new(Duration::from_secs(timeout_secs));
    options.poll_every = poll_secs.map(Duration::from_secs);
    serde_json::to_value(WaitInput {
        issue_id: issue_id(ISSUE),
        target: StatusTarget::done(),
        options,
    })
    .unwrap()
}

fn result(outcome: &TestRunOutcome) -> StatusWait {
    serde_json::from_value(outcome.result.clone().expect("the wait ends")).unwrap()
}

fn reads(events: &[WorkflowEvent]) -> Vec<GetIssueInput> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkflowEvent::ActivityScheduled { name, input, .. } if name == GET_ISSUE => {
                Some(serde_json::from_value(input.clone()).unwrap())
            }
            _ => None,
        })
        .collect()
}

fn timers(events: &[WorkflowEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkflowEvent::TimerStarted { duration_secs, .. } => Some(*duration_secs),
            _ => None,
        })
        .collect()
}

/// Replays `events` with the advancing clock ON (`replay_check`) and OFF
/// (`WorkflowReplayer::new()`).
async fn assert_replays(outcome: &TestRunOutcome) {
    let report = outcome.replay_check(handler()).await;
    assert!(
        matches!(report.status, ReplayStatus::ReplaySucceeded),
        "{report}"
    );
    assert_replays_with_the_clock_off(outcome.events().to_vec()).await;
}

async fn assert_replays_with_the_clock_off(events: Vec<WorkflowEvent>) {
    let report = WorkflowReplayer::new()
        .register_fn("wait_for_issue", handler())
        .replay_from_events(events)
        .await;
    assert!(
        matches!(report.status, ReplayStatus::ReplaySucceeded),
        "{report}"
    );
}

// ------------------------------------------------------------ spike S2

/// Spike S2. Ten signals of other issues and of a status that is not the
/// target come during a 120 s wait with a poll each 60 s. The seed of the
/// simulation picks when each signal lands, so some land after the first
/// poll. The wait still ends at 120 s with `TimedOut`, on every seed.
#[tokio::test]
async fn ignored_signals_do_not_extend_the_deadline() {
    let env = WorkflowTestEnv::new().mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())));
    let mut sim = env.simulation().seeds(SeedPlan {
        first: 0,
        count: 24,
    });
    for n in 0..10 {
        let payload = if n % 2 == 0 {
            signal(OTHER_ISSUE, Some(done()), Some(1_000), n)
        } else {
            signal(ISSUE, Some(in_progress()), Some(1_000 + i64::from(n)), n)
        };
        sim = sim.signal(SIGNAL, payload);
    }
    let mut histories = Vec::new();
    let mut late_signals = 0;
    let report = sim
        .sweep(handler(), input(120, Some(60)), |run| {
            let wait: StatusWait =
                serde_json::from_value(run.result().clone().expect("the wait ends")).unwrap();
            assert_eq!(
                wait,
                StatusWait::TimedOut {
                    last: Some(in_progress())
                },
                "seed {}",
                run.seed()
            );
            assert_eq!(run.elapsed().num_seconds(), 120, "seed {}", run.seed());
            // The first read, the poll at 60 s and the last read at 120 s.
            assert_eq!(reads(run.events()).len(), 3, "seed {}", run.seed());
            let first_fire = run
                .events()
                .iter()
                .position(|e| matches!(e, WorkflowEvent::TimerFired { .. }))
                .unwrap();
            if run.events()[first_fire..]
                .iter()
                .any(|e| matches!(e, WorkflowEvent::SignalReceived { .. }))
            {
                late_signals += 1;
            }
            histories.push(run.events().to_vec());
        })
        .await
        .unwrap_or_else(|failure| panic!("{failure}"));
    assert!(report.seeds > 0);
    // Some seeds land a signal after the poll at 60 s. Those seeds show
    // that a signal does not move the deadline.
    assert!(late_signals > 0, "no seed landed a signal after 60 s");
    for events in histories {
        assert_replays_with_the_clock_off(events).await;
    }
}

// ------------------------------------------------------------ AC33

#[tokio::test]
async fn already_there_returns_at_once() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
        .run(handler(), input(3_600, Some(60)))
        .await;
    assert_eq!(
        result(&outcome),
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Initial
        }
    );
    let reads = reads(outcome.events());
    assert_eq!(reads, vec![GetIssueInput::new(issue_id(ISSUE))]);
    assert!(timers(outcome.events()).is_empty());
    assert_eq!(outcome.elapsed().num_seconds(), 0);
    assert_replays(&outcome).await;
}

#[tokio::test]
async fn signal_ends_the_wait() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
        .queue_signal(SIGNAL, signal(ISSUE, Some(done()), Some(5_000), 1))
        .run(handler(), input(3_600, None))
        .await;
    assert_eq!(
        result(&outcome),
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Signal
        }
    );
    assert_eq!(reads(outcome.events()).len(), 1);
    assert_eq!(outcome.elapsed().num_seconds(), 0);
    assert_replays(&outcome).await;
}

#[tokio::test]
async fn poll_ends_the_wait_without_webhooks() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&done()))])
        .run(handler(), input(3_600, Some(60)))
        .await;
    assert_eq!(
        result(&outcome),
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Poll
        }
    );
    assert_eq!(reads(outcome.events()).len(), 3);
    assert_eq!(timers(outcome.events()), vec![60, 60]);
    assert_eq!(outcome.elapsed().num_seconds(), 120);
    assert_replays(&outcome).await;
}

#[tokio::test]
async fn the_last_read_at_the_deadline_can_reach_the_target() {
    let env = WorkflowTestEnv::new()
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&done()))]);
    let run = env.simulation().run(7, handler(), input(90, None)).await;
    let wait: StatusWait = serde_json::from_value(run.result().clone().unwrap()).unwrap();
    assert_eq!(
        wait,
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Poll
        }
    );
    assert_eq!(timers(run.events()), vec![90]);
    assert_eq!(run.elapsed().num_seconds(), 90);
    assert_replays_with_the_clock_off(run.events().to_vec()).await;
}

#[tokio::test]
async fn other_issue_and_old_timestamp_are_skipped() {
    let closed = status("10003", "Closed", "done");
    let resolved = status("10004", "Resolved", "done");
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
        // Another issue at a done status: skipped.
        .queue_signal(SIGNAL, signal(OTHER_ISSUE, Some(resolved), Some(9_000), 1))
        // This issue, not at the target: seen, so 2 000 is the newest time.
        .queue_signal(SIGNAL, signal(ISSUE, Some(in_progress()), Some(2_000), 2))
        // This issue at a done status, but older than 2 000: skipped.
        .queue_signal(SIGNAL, signal(ISSUE, Some(closed), Some(1_000), 3))
        // A payload that is not an IssueSignal: skipped.
        .queue_signal(SIGNAL, json!({"unexpected": true}))
        // This issue at the target, and newer: the end.
        .queue_signal(SIGNAL, signal(ISSUE, Some(done()), Some(3_000), 4))
        .run(handler(), input(3_600, None))
        .await;
    assert_eq!(
        result(&outcome),
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Signal
        }
    );
    assert_eq!(reads(outcome.events()).len(), 1);
    assert_eq!(outcome.elapsed().num_seconds(), 0);
    assert_replays(&outcome).await;
}

#[tokio::test]
async fn timeout_over_max_wait_fails() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
        .run(handler(), input(MAX_WAIT.as_secs() + 1, None))
        .await;
    assert!(outcome.result.is_err());
    let failed = outcome.events().iter().find_map(|e| match e {
        WorkflowEvent::WorkflowFailed { error_type, .. } => Some(error_type.clone()),
        _ => None,
    });
    assert_eq!(failed, Some(Some(INVALID_INPUT_ERROR.to_owned())));
    assert!(reads(outcome.events()).is_empty());
    assert_replays(&outcome).await;

    // MAX_WAIT itself is allowed.
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
        .run(handler(), input(MAX_WAIT.as_secs(), None))
        .await;
    assert!(matches!(result(&outcome), StatusWait::Reached { .. }));
}

#[tokio::test]
async fn a_failed_read_keeps_its_error_type() {
    let payload = autumn_harvest::failure::ActivityFailure::non_retryable(
        "jira.not_found_or_forbidden",
        "jira get_issue: not found or no permission",
    );
    let outcome = WorkflowTestEnv::new()
        .mock_activity_retries(
            GET_ISSUE,
            vec![Err(
                autumn_harvest::failure::IntoActivityErrorString::into_error_payload(payload),
            )],
        )
        .run(handler(), input(3_600, None))
        .await;
    assert!(outcome.result.is_err());
    let failed = outcome.events().iter().find_map(|e| match e {
        WorkflowEvent::WorkflowFailed { error_type, .. } => Some(error_type.clone()),
        _ => None,
    });
    assert_eq!(failed, Some(Some("jira.not_found_or_forbidden".to_owned())));
    assert_replays(&outcome).await;
}

/// Each `WorkflowTestEnv` case again, in one place, with both replays.
#[tokio::test]
async fn every_case_replays() {
    let cases: Vec<WorkflowTestEnv> = vec![
        WorkflowTestEnv::new().mock_activity(GET_ISSUE, |_| Ok(issue_at(&done()))),
        WorkflowTestEnv::new()
            .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
            .queue_signal(SIGNAL, signal(OTHER_ISSUE, Some(done()), None, 1))
            .queue_signal(SIGNAL, signal(ISSUE, Some(done()), None, 2)),
        WorkflowTestEnv::new()
            .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
            .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&done()))]),
    ];
    for env in &cases {
        let outcome = env.run(handler(), input(3_600, Some(300))).await;
        assert!(matches!(result(&outcome), StatusWait::Reached { .. }));
        assert_replays(&outcome).await;
    }
    let outcome = WorkflowTestEnv::new()
        .run(handler(), input(MAX_WAIT.as_secs() + 1, None))
        .await;
    assert!(outcome.result.is_err());
    assert_replays(&outcome).await;
}
