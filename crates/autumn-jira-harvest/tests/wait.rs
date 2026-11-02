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

/// A workflow that waits 10 minutes, then calls the wait. In production the
/// start-time clock of the context is 10 minutes old at the wait.
#[workflow]
async fn sleep_then_wait(
    ctx: &WorkflowContext,
    input: WaitInput,
) -> Result<StatusWait, WorkflowFailure> {
    ctx.timer("before-the-wait", 600)
        .await
        .map_err(|e| WorkflowFailure::new("test.timer", e.to_string()))?;
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
    assert_replays_with_the_clock_off_as("wait_for_issue", handler(), events).await;
}

async fn assert_replays_with_the_clock_off_as(
    name: &str,
    handler: WorkflowHandlerFn,
    events: Vec<WorkflowEvent>,
) {
    let report = WorkflowReplayer::new()
        .register_fn(name, handler)
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
            // The first read, the poll at 60 s and the last read at 120 s. A
            // signal that lands with the poll timer wins the race, and then
            // that poll does not run.
            let reads = reads(run.events()).len();
            assert!(
                (2..=3).contains(&reads),
                "seed {}: {reads} reads",
                run.seed()
            );
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
    assert_eq!(timers(outcome.events()), Vec::<u64>::new());
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

/// The deadline counts from the start of the wait, not from the start of
/// the run. With the advancing clock OFF, a deadline from the start-time
/// clock is 10 minutes in the past, and the replay fails.
#[tokio::test]
async fn a_late_wait_keeps_its_full_timeout() {
    let handler = sleep_then_wait_info().handler;
    let env = WorkflowTestEnv::new().mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())));
    let run = env.simulation().run(3, handler, input(120, None)).await;
    let wait: StatusWait = serde_json::from_value(run.result().clone().unwrap()).unwrap();
    assert_eq!(
        wait,
        StatusWait::TimedOut {
            last: Some(in_progress())
        }
    );
    assert_eq!(timers(run.events()), vec![600, 120]);
    assert_eq!(run.elapsed().num_seconds(), 720);
    assert_replays_with_the_clock_off_as("sleep_then_wait", handler, run.events().to_vec()).await;
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
    assert_eq!(reads(outcome.events()), Vec::<GetIssueInput>::new());
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

// ------------------------------------------------------------ AC35

mod human {
    //! A workflow waits on an issue that a person made. The activities are
    //! the real handlers over `FakeJira`. No webhook comes: a poll ends the
    //! wait.

    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use autumn_harvest::context::ActivityContext;
    use autumn_harvest::info::ActivityInfo;
    use autumn_jira::ids::SiteName;
    use autumn_jira::request::IssueTypeRef;
    use autumn_jira::san_mateo::Secret;
    use autumn_jira::{BasicAuth, IssueKey, IssueRef, JiraClient, JiraSite, NewIssue, ProjectKey};
    use autumn_jira_fake::{ANN, DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp};
    use autumn_jira_harvest::{
        JiraClients, RouteKey, SignalRoute, WatchIssueInput, WatchOutput, activities,
        jira_watch_issue_info,
    };

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct WatchInput {
        issue: IssueKey,
        options: WaitOptions,
    }

    /// Watches a person's issue, then waits until it is done.
    #[workflow]
    async fn watch_and_wait(
        ctx: &WorkflowContext,
        input: WatchInput,
    ) -> Result<StatusWait, WorkflowFailure> {
        let route = SignalRoute::new("watch_and_wait", ctx.workflow_id())
            .map_err(|e| WorkflowFailure::new("test.route", e.to_string()))?;
        let watch = WatchIssueInput::new(IssueRef::Key(input.issue.clone()), route);
        let watched: WatchOutput = ctx
            .execute_activity_with_opts(
                &jira_watch_issue_info(),
                watch,
                Some(ctx.queue_name()),
                None,
                None,
            )
            .await
            .map_err(|e| WorkflowFailure::new("test.watch", e.to_string()))?;
        await_issue_status(
            ctx,
            &watched.issue.id,
            &StatusTarget::done(),
            &input.options,
        )
        .await
    }

    fn client(fake: &FakeJira) -> JiraClient {
        JiraClient::api_token(
            JiraSite::Subdomain(SiteName::parse("acme").unwrap()),
            BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN)).unwrap(),
            Arc::new(fake.clone()),
        )
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
    }

    fn info(name: &str) -> ActivityInfo {
        activities().into_iter().find(|i| i.name == name).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wait_on_a_human_issue() {
        let fake = FakeJira::new();
        let made = client(&fake)
            .create_issue(&NewIssue::new(
                ProjectKey::parse("OPS").unwrap(),
                IssueTypeRef::Name("Task".into()),
                "The printer is on fire",
            ))
            .await
            .unwrap();
        let mut state: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        state.insert(
            TypeId::of::<JiraClients>(),
            Box::new(JiraClients::new(client(&fake))),
        );
        state.insert(
            TypeId::of::<RouteKey>(),
            Box::new(RouteKey::new(Secret::new("k".repeat(32))).unwrap()),
        );
        let state = Arc::new(state);
        let reads = Arc::new(AtomicUsize::new(0));
        let watch = {
            let state = Arc::clone(&state);
            move |input: Value| {
                let ctx = ActivityContext::new_test_with_state(Arc::clone(&state));
                block_on((info("jira_watch_issue").handler)(&ctx, input))
            }
        };
        let read = {
            let (state, reads, fake) = (Arc::clone(&state), Arc::clone(&reads), fake.clone());
            let key = made.key.clone();
            move |input: Value| {
                let ctx = ActivityContext::new_test_with_state(Arc::clone(&state));
                let out = block_on((info(GET_ISSUE).handler)(&ctx, input));
                // After the first read, a person resolves the issue.
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    fake.human_transition(key.as_str(), "In Progress", ANN)
                        .unwrap();
                    fake.human_transition(key.as_str(), "Done", ANN).unwrap();
                }
                out
            }
        };
        let mut options = WaitOptions::new(Duration::from_secs(3600));
        options.poll_every = Some(Duration::from_secs(60));
        let input = serde_json::to_value(WatchInput {
            issue: made.key.clone(),
            options,
        })
        .unwrap();
        let env = WorkflowTestEnv::new()
            .with_workflow_id("watch:OPS-1")
            .mock_activity("jira_watch_issue", watch)
            .mock_activity(GET_ISSUE, read);
        let handler = watch_and_wait_info().handler;
        let outcome = env.run(handler, input).await;
        let StatusWait::Reached { status, source } = result(&outcome) else {
            panic!("the wait times out: {:?}", outcome.result);
        };
        assert_eq!(status.name, "Done");
        assert_eq!(source, WaitSource::Poll);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "the first read and one poll"
        );
        assert_eq!(timers(outcome.events()), vec![60]);
        // The watch added the route and the label. No webhook came.
        let issue = fake.issue(made.key.as_str()).unwrap();
        assert!(issue["properties"]["autumn-jira-routes"]["routes"].is_array());
        assert_eq!(fake.count(FakeOp::SetProperty), 1);
        let report = outcome.replay_check(handler).await;
        assert!(
            matches!(report.status, ReplayStatus::ReplaySucceeded),
            "{report}"
        );
        let report = WorkflowReplayer::new()
            .with_workflow_id("watch:OPS-1")
            .register_fn("watch_and_wait", handler)
            .replay_from_events(outcome.events().to_vec())
            .await;
        assert!(
            matches!(report.status, ReplayStatus::ReplaySucceeded),
            "{report}"
        );
    }
}
