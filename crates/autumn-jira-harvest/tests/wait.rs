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

// ------------------------------------------------------------ review fixes

/// The failure payload of a read that used every attempt on a 503.
fn transient_read_failure() -> String {
    autumn_harvest::failure::IntoActivityErrorString::into_error_payload(
        autumn_jira_harvest::failure_for(&autumn_jira::JiraError::Upstream {
            op: autumn_jira::error::Op::GetIssue,
            status: 503,
        }),
    )
}

/// The first read gives `first`. Each later read fails with a 503.
fn env_with_failed_polls(first: &StatusSummary) -> WorkflowTestEnv {
    let first = issue_at(first);
    WorkflowTestEnv::new().mock_activity_seeded(GET_ISSUE, move |_, mock| {
        if mock.call() == 1 {
            Ok(first.clone())
        } else {
            Err(transient_read_failure())
        }
    })
}

/// R4-harvest-determinism-1: a failed poll is a missed poll. The next poll
/// can still end the wait.
#[tokio::test]
async fn a_failed_poll_is_a_missed_poll() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .mock_activity_retries(GET_ISSUE, vec![Err(transient_read_failure())])
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
    assert_replays(&outcome).await;
}

/// R4-harvest-determinism-1: a failed last read gives `TimedOut` with the
/// last status that the wait saw. The simulation records a failed read
/// with no type.
#[tokio::test]
async fn a_failed_last_read_times_out() {
    let env = env_with_failed_polls(&in_progress());
    let run = env
        .simulation()
        .run(5, handler(), input(180, Some(60)))
        .await;
    let wait: StatusWait = serde_json::from_value(
        run.result()
            .clone()
            .unwrap_or_else(|e| panic!("the wait fails: {e}")),
    )
    .unwrap();
    assert_eq!(
        wait,
        StatusWait::TimedOut {
            last: Some(in_progress())
        }
    );
    assert_eq!(run.elapsed().num_seconds(), 180);
    // The first read, two polls and the last read.
    assert_eq!(reads(run.events()).len(), 4);
    assert_replays_with_the_clock_off(run.events().to_vec()).await;
}

/// R4-harvest-determinism-1: after a failed poll, a signal still ends the
/// wait. Some seeds land the signal after the first failed poll.
#[tokio::test]
async fn a_signal_after_a_failed_poll_ends_the_wait() {
    let env = env_with_failed_polls(&in_progress());
    let mut sim = env.simulation().seeds(SeedPlan {
        first: 0,
        count: 24,
    });
    for n in 0..6 {
        sim = sim.signal(SIGNAL, signal(OTHER_ISSUE, Some(done()), None, n));
    }
    let sim = sim.signal(SIGNAL, signal(ISSUE, Some(done()), Some(5_000), 9));
    let mut late = 0;
    sim.sweep(handler(), input(3_600, Some(60)), |run| {
        let wait: StatusWait = serde_json::from_value(
            run.result()
                .clone()
                .unwrap_or_else(|e| panic!("seed {}: the wait fails: {e}", run.seed())),
        )
        .unwrap();
        assert_eq!(
            wait,
            StatusWait::Reached {
                status: done(),
                source: WaitSource::Signal
            },
            "seed {}",
            run.seed()
        );
        if reads(run.events()).len() > 1 {
            late += 1;
        }
    })
    .await
    .unwrap_or_else(|failure| panic!("{failure}"));
    assert!(late > 0, "no seed landed the signal after a failed poll");
}

/// A poll that fails with a non-retryable class still ends the wait with
/// that error type.
#[tokio::test]
async fn a_non_retryable_poll_failure_ends_the_wait() {
    let not_found = autumn_harvest::failure::IntoActivityErrorString::into_error_payload(
        autumn_harvest::failure::ActivityFailure::non_retryable(
            "jira.not_found_or_forbidden",
            "jira get_issue: not found or no permission",
        ),
    );
    let outcome = WorkflowTestEnv::new()
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .mock_activity_retries(GET_ISSUE, vec![Err(not_found)])
        .run(handler(), input(3_600, Some(60)))
        .await;
    let failed = outcome.events().iter().find_map(|e| match e {
        WorkflowEvent::WorkflowFailed { error_type, .. } => Some(error_type.clone()),
        _ => None,
    });
    assert_eq!(failed, Some(Some("jira.not_found_or_forbidden".to_owned())));
    assert_replays(&outcome).await;
}

/// An issue at `status`, updated at `updated_ms`.
fn issue_updated_at(status: &StatusSummary, updated_ms: i64) -> Value {
    let mut issue = issue_at(status);
    issue["updated"] = json!(updated_ms);
    issue
}

fn reopened() -> StatusSummary {
    status("10005", "Reopened", "indeterminate")
}

/// R4-harvest-determinism-3: a read moves the newest time too. A signal
/// that is older than a read is about a past state, so the wait skips it.
#[tokio::test]
async fn a_signal_older_than_a_read_is_skipped() {
    let env = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_updated_at(&reopened(), 2_000)))
        .queue_signal(SIGNAL, signal(ISSUE, Some(done()), Some(1_000), 1));
    let run = env.simulation().run(1, handler(), input(60, None)).await;
    let wait: StatusWait = serde_json::from_value(run.result().clone().unwrap()).unwrap();
    assert_eq!(
        wait,
        StatusWait::TimedOut {
            last: Some(reopened())
        }
    );
    assert_replays_with_the_clock_off(run.events().to_vec()).await;

    // A signal at the time of the read, or later, is not older.
    let env = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_updated_at(&reopened(), 2_000)))
        .queue_signal(SIGNAL, signal(ISSUE, Some(done()), Some(2_000), 1));
    let run = env.simulation().run(1, handler(), input(60, None)).await;
    let wait: StatusWait = serde_json::from_value(run.result().clone().unwrap()).unwrap();
    assert!(
        matches!(
            wait,
            StatusWait::Reached {
                source: WaitSource::Signal,
                ..
            }
        ),
        "{wait:?}"
    );
}

/// R4-harvest-determinism-4: the poll rounds have a bound, so a valid input
/// cannot fill the history. The bound is 3 000 rounds. The default poll
/// over `MAX_WAIT` is 2 784 rounds, so it is in the bound.
#[tokio::test]
async fn too_many_poll_rounds_fail_at_once() {
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
        .run(handler(), input(MAX_WAIT.as_secs(), Some(60)))
        .await;
    let failed = outcome.events().iter().find_map(|e| match e {
        WorkflowEvent::WorkflowFailed { error_type, .. } => Some(error_type.clone()),
        _ => None,
    });
    assert_eq!(failed, Some(Some(INVALID_INPUT_ERROR.to_owned())));
    assert_eq!(reads(outcome.events()), Vec::<GetIssueInput>::new());
    // 3 000 rounds are allowed. The default poll over MAX_WAIT is allowed.
    for (timeout, poll) in [(3_000 * 60, 60), (MAX_WAIT.as_secs(), 15 * 60)] {
        let outcome = WorkflowTestEnv::new()
            .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
            .run(handler(), input(timeout, Some(poll)))
            .await;
        assert!(
            matches!(result(&outcome), StatusWait::Reached { .. }),
            "{timeout} / {poll}"
        );
    }
}

/// R4-harvest-determinism-4: the events of the largest wait stay far under
/// the hard cap of the history (50 000 events). 3 000 poll rounds and 1 000
/// skipped signals use at most half of it.
#[tokio::test]
async fn the_round_bound_fits_the_history_cap() {
    let env = WorkflowTestEnv::new().mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())));
    let base = env.simulation().run(1, handler(), input(60, None)).await;
    let polled = env
        .simulation()
        .run(1, handler(), input(660, Some(60)))
        .await;
    let poll_round = (polled.events().len() - base.events().len()).div_ceil(10);
    let skipped = (0..10).fold(WorkflowTestEnv::new(), |env, n| {
        env.queue_signal(SIGNAL, signal(OTHER_ISSUE, Some(done()), None, n))
    });
    let skipped = skipped
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
        .simulation()
        .run(1, handler(), input(60, None))
        .await;
    let signal_round = (skipped.events().len() - base.events().len()).div_ceil(10);
    let worst = poll_round * 3_000 + signal_round * 1_000 + base.events().len();
    assert!(
        worst < 25_000,
        "{poll_round} events for each poll, {signal_round} for each signal: {worst}"
    );
}

/// The input of a workflow with two waits at the same time.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TwoWaits {
    first: IssueId,
    second: IssueId,
}

/// Runs two waits at the same time in one run.
#[workflow]
async fn two_waits(
    ctx: &WorkflowContext,
    input: TwoWaits,
) -> Result<Vec<StatusWait>, WorkflowFailure> {
    let options = WaitOptions::new(Duration::from_secs(60));
    let target = StatusTarget::done();
    let (first, second) = tokio::join!(
        await_issue_status(ctx, &input.first, &target, &options),
        await_issue_status(ctx, &input.second, &target, &options),
    );
    Ok(vec![first?, second?])
}

/// R4-harvest-determinism-7: all waits share one signal name, so two waits
/// at the same time take the signals of each other. The waits of one run
/// run one at a time: the second wait starts when the first wait ends. It
/// then reads its issue first, so it sees each change that it did not get
/// as a signal. A replay gives the same order.
#[tokio::test]
async fn two_waits_at_the_same_time_run_one_after_the_other() {
    let handler = two_waits_info().handler;
    let input = serde_json::to_value(TwoWaits {
        first: issue_id(ISSUE),
        second: issue_id(OTHER_ISSUE),
    })
    .unwrap();
    let outcome = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
        .run(handler, input)
        .await;
    let both: Vec<StatusWait> =
        serde_json::from_value(outcome.result.clone().expect("the waits end")).unwrap();
    assert_eq!(both.len(), 2);
    let events = outcome.events();
    let scheduled: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(n, e)| matches!(e, WorkflowEvent::ActivityScheduled { .. }).then_some(n))
        .collect();
    let completed = events
        .iter()
        .position(|e| matches!(e, WorkflowEvent::ActivityCompleted { .. }))
        .unwrap();
    assert_eq!(scheduled.len(), 2);
    assert!(
        scheduled[1] > completed,
        "the second wait read before the first wait ended: {events:?}"
    );
    let report = outcome.replay_check(handler).await;
    assert!(
        matches!(report.status, ReplayStatus::ReplaySucceeded),
        "{report}"
    );
    assert_replays_with_the_clock_off_as("two_waits", handler, events.to_vec()).await;

    // One wait after the other is correct, and a wait after a wait starts.
    for _ in 0..2 {
        let outcome = WorkflowTestEnv::new()
            .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
            .run(wait_for_issue_info().handler, input_for(ISSUE))
            .await;
        assert!(matches!(result(&outcome), StatusWait::Reached { .. }));
    }
}

fn input_for(issue: &str) -> Value {
    serde_json::to_value(WaitInput {
        issue_id: issue_id(issue),
        target: StatusTarget::done(),
        options: WaitOptions::new(Duration::from_secs(60)),
    })
    .unwrap()
}

/// Waits, then runs an activity and waits for the signal `go`: work after
/// the wait. With `drain` set, the workflow takes the late Jira signals
/// before it returns.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkAfter {
    wait: WaitInput,
    drain: bool,
}

#[workflow]
async fn work_after_the_wait(
    ctx: &WorkflowContext,
    input: WorkAfter,
) -> Result<StatusWait, WorkflowFailure> {
    let wait = &input.wait;
    let outcome = await_issue_status(ctx, &wait.issue_id, &wait.target, &wait.options).await?;
    // Other work: one activity. A replay of the wait cannot take a signal
    // that comes after this command.
    let _: Value = ctx
        .execute_activity_with_opts(
            &autumn_jira_harvest::jira_get_issue_info(),
            GetIssueInput::new(wait.issue_id.clone()),
            Some(ctx.queue_name()),
            None,
            None,
        )
        .await
        .map_err(|e| WorkflowFailure::new("test.work", e.to_string()))?;
    ctx.wait_for_signal("go")
        .await
        .map_err(|e| WorkflowFailure::new("test.go", e.to_string()))?;
    if input.drain {
        let late = autumn_jira_harvest::drain_jira_signals(ctx)?;
        assert_eq!(late, 1, "one late signal");
    }
    Ok(outcome)
}

/// R4-harvest-determinism-2: a Jira signal can come after the wait, while
/// the workflow does other work. No code reads it, and a strict replay
/// fails on it. `drain_jira_signals` before the return takes it.
#[tokio::test]
async fn a_signal_after_the_wait_needs_the_drain() {
    let handler = work_after_the_wait_info().handler;
    for drain in [false, true] {
        let input = serde_json::to_value(WorkAfter {
            wait: serde_json::from_value(input(3_600, None)).unwrap(),
            drain,
        })
        .unwrap();
        let env = WorkflowTestEnv::new().mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())));
        let mut run = env.start(handler, input);
        run.run_until_blocked().await;
        // A late webhook of the issue, then the end of the other work.
        run.signal(SIGNAL, signal(ISSUE, Some(done()), Some(9_000), 1))
            .unwrap();
        run.signal("go", Value::Null).unwrap();
        let outcome = run.finish().await;
        assert!(
            matches!(
                result(&outcome),
                StatusWait::Reached {
                    source: WaitSource::Initial,
                    ..
                }
            ),
            "{:?}",
            outcome.result
        );
        let report = WorkflowReplayer::new()
            .register_fn("work_after_the_wait", handler)
            .replay_from_events(outcome.events().to_vec())
            .await;
        assert_eq!(
            matches!(report.status, ReplayStatus::ReplaySucceeded),
            drain,
            "drain {drain}: {report}"
        );
        let report = outcome.replay_check(handler).await;
        assert_eq!(
            matches!(report.status, ReplayStatus::ReplaySucceeded),
            drain,
            "drain {drain}: {report}"
        );
    }
}

// ------------------------------------------------------------ review round 2

/// R2C-harvest-1: after `MAX_SIGNAL_ROUNDS` signals that do not end the
/// wait, the race has no signal branch. With `poll_every: None`, the wait
/// then still polls at `DEFAULT_POLL_EVERY`, so it does not go blind until
/// its deadline.
#[tokio::test]
async fn a_wait_past_the_signal_cap_still_polls() {
    let cap = autumn_jira_harvest::MAX_SIGNAL_ROUNDS;
    let env = (0..cap).fold(WorkflowTestEnv::new(), |env, n| {
        env.queue_signal(SIGNAL, signal(OTHER_ISSUE, Some(done()), None, n))
    });
    let first = issue_at(&in_progress());
    let later = issue_at(&done());
    let env = env
        .queue_signal(SIGNAL, signal(ISSUE, Some(done()), None, cap))
        .mock_activity_seeded(GET_ISSUE, move |_, mock| {
            if mock.call() == 1 {
                Ok(first.clone())
            } else {
                Ok(later.clone())
            }
        });
    let day = 24 * 3_600;
    let run = env.simulation().run(1, handler(), input(day, None)).await;
    let wait: StatusWait = serde_json::from_value(run.result().clone().unwrap()).unwrap();
    assert_eq!(
        wait,
        StatusWait::Reached {
            status: done(),
            source: WaitSource::Poll
        }
    );
    let fallback = autumn_jira_harvest::wait::DEFAULT_POLL_EVERY.as_secs();
    assert!(
        u64::try_from(run.elapsed().num_seconds()).unwrap() <= fallback,
        "the wait ended after {} s",
        run.elapsed().num_seconds()
    );
    assert_replays_with_the_clock_off(run.events().to_vec()).await;
}

/// R2E-tests-guards-6: the bound of the poll rounds at its edge. 3 001
/// rounds fail, also when the timeout is one second over 3 000 rounds.
#[tokio::test]
async fn one_poll_round_over_the_bound_fails() {
    for timeout in [3_001 * 60, 3_000 * 60 + 1] {
        let outcome = WorkflowTestEnv::new()
            .mock_activity(GET_ISSUE, |_| Ok(issue_at(&done())))
            .run(handler(), input(timeout, Some(60)))
            .await;
        let failed = outcome.events().iter().find_map(|e| match e {
            WorkflowEvent::WorkflowFailed { error_type, .. } => Some(error_type.clone()),
            _ => None,
        });
        assert_eq!(
            failed,
            Some(Some(INVALID_INPUT_ERROR.to_owned())),
            "{timeout}"
        );
        assert_eq!(
            reads(outcome.events()),
            Vec::<GetIssueInput>::new(),
            "{timeout}"
        );
    }
}

/// R2C-harvest-3: a missed poll records more events than a good one: each
/// attempt starts, and the last one fails. The worst case of the round
/// bounds stays under 35 000 events, so 15 000 of the cap of 50 000 stay
/// for the workflow. That is the claim of `MAX_POLL_ROUNDS`.
#[tokio::test]
async fn missed_polls_fit_the_history_cap() {
    let base = WorkflowTestEnv::new()
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
        .simulation()
        .run(1, handler(), input(60, None))
        .await;
    let attempts = autumn_jira_harvest::activities::get_issue_retry().max_attempts;
    let failed = (0..10).fold(
        WorkflowTestEnv::new().mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))]),
        |env, _| {
            env.mock_activity_retries(
                GET_ISSUE,
                (0..attempts)
                    .map(|_| Err(transient_read_failure()))
                    .collect(),
            )
        },
    );
    let failed = failed
        .mock_activity_retries(GET_ISSUE, vec![Ok(issue_at(&in_progress()))])
        .simulation()
        .run(1, handler(), input(660, Some(60)))
        .await;
    assert_eq!(reads(failed.events()).len(), 12);
    let poll_round = (failed.events().len() - base.events().len()).div_ceil(10);
    let skipped = (0..10)
        .fold(WorkflowTestEnv::new(), |env, n| {
            env.queue_signal(SIGNAL, signal(OTHER_ISSUE, Some(done()), None, n))
        })
        .mock_activity(GET_ISSUE, |_| Ok(issue_at(&in_progress())))
        .simulation()
        .run(1, handler(), input(60, None))
        .await;
    let signal_round = (skipped.events().len() - base.events().len()).div_ceil(10);
    let worst = poll_round * 3_000
        + signal_round * usize::try_from(autumn_jira_harvest::MAX_SIGNAL_ROUNDS).unwrap()
        + base.events().len();
    assert!(
        worst < 35_000,
        "{poll_round} events for each missed poll, {signal_round} for each signal: {worst}"
    );
}
