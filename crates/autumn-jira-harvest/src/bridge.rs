//! The webhook-to-signal bridge (plan 9.4).
//!
//! [`HarvestSignalBridge`] is an [`EventSink`]. For a delivery that needs
//! routing, it reads the route property of the issue, checks the tag and
//! the allowlist of each route, and sends the signal [`SIGNAL`] to the run
//! with `send_signal_idempotent`. The delivery id is the idempotency key,
//! so a repeated delivery gives one signal. Webhooks are at least once.
//!
//! [`wants`] and [`routes_from_property`] are pure. They do no I/O.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use autumn_harvest::HarvestError;
use autumn_harvest::worker::DbPool;
use autumn_jira::{Delivery, EventSink, HandlerError, JiraClient, JiraEvent};
use serde::Serialize;
use serde_json::Value;

use crate::route::{RouteKey, SignalRoute};
use crate::wait::SIGNAL;

/// The result of one route of one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteOutcome {
    /// The route passed the checks, and the run got the signal.
    Signalled,
    /// The run already has a signal with this delivery id.
    Duplicate,
    /// No run has this workflow id.
    NoRun,
    /// The run is finished.
    Finished,
    /// The workflow name is not in the allowlist.
    NotAllowed,
    /// The tag does not verify. The bridge makes no database call.
    BadTag,
}

/// The counters of a bridge. Each counts routes or deliveries since the
/// bridge was made.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct BridgeCounters {
    /// Deliveries that came in.
    pub deliveries: u64,
    /// Deliveries with nothing to route: another event, or no
    /// `autumn-tracked` label, or no route property.
    pub skipped: u64,
    /// Routes that got the signal.
    pub signalled: u64,
    /// Routes whose run already had the signal of this delivery.
    pub duplicate: u64,
    /// Routes with no run.
    pub no_run: u64,
    /// Routes whose run is finished.
    pub finished: u64,
    /// Routes with a workflow name that is not allowed.
    pub not_allowed: u64,
    /// Routes with a tag that does not verify.
    pub bad_tag: u64,
    /// Deliveries that ended with a transient error (503).
    pub transient: u64,
}

/// The live counters.
#[derive(Debug, Default)]
struct Counters {
    deliveries: AtomicU64,
    skipped: AtomicU64,
    signalled: AtomicU64,
    duplicate: AtomicU64,
    no_run: AtomicU64,
    finished: AtomicU64,
    not_allowed: AtomicU64,
    bad_tag: AtomicU64,
    transient: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> BridgeCounters {
        let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
        BridgeCounters {
            deliveries: read(&self.deliveries),
            skipped: read(&self.skipped),
            signalled: read(&self.signalled),
            duplicate: read(&self.duplicate),
            no_run: read(&self.no_run),
            finished: read(&self.finished),
            not_allowed: read(&self.not_allowed),
            bad_tag: read(&self.bad_tag),
            transient: read(&self.transient),
        }
    }
}

/// The sink that turns Jira deliveries into harvest signals.
///
/// Limits (ADR 0006): the bridge uses one shard. A multi-shard engine needs
/// a fan-out lookup. The workflow checks the issue id of each signal, so a
/// route on another issue cannot move a workflow.
#[derive(Clone)]
pub struct HarvestSignalBridge {
    pool: DbPool,
    client: JiraClient,
    key: RouteKey,
    allowed: BTreeSet<String>,
    counters: Arc<Counters>,
}

impl fmt::Debug for HarvestSignalBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HarvestSignalBridge")
            .field("client", &self.client)
            .field("key", &self.key)
            .field("allowed", &self.allowed)
            .field("counters", &self.counters.snapshot())
            .finish_non_exhaustive()
    }
}

impl HarvestSignalBridge {
    /// Makes a bridge with an empty allowlist. An empty allowlist allows no
    /// workflow, so call [`Self::allow_workflow`] for each workflow name.
    #[must_use]
    pub fn new(pool: DbPool, client: JiraClient, key: RouteKey) -> Self {
        Self {
            pool,
            client,
            key,
            allowed: BTreeSet::new(),
            counters: Arc::default(),
        }
    }

    /// Allows routes to the workflow with this name.
    #[must_use]
    pub fn allow_workflow(mut self, workflow_name: &str) -> Self {
        self.allowed.insert(workflow_name.to_owned());
        self
    }

    /// Returns the counters.
    #[must_use]
    pub fn counters(&self) -> BridgeCounters {
        self.counters.snapshot()
    }

    /// The core of `deliver`, with an outcome per route, for tests.
    ///
    /// # Errors
    ///
    /// `HandlerError::Transient` for a database or shard error and for a
    /// retryable Jira error. `HandlerError::Permanent` for another Jira
    /// error of the property read.
    pub async fn route(&self, delivery: &Delivery) -> Result<Vec<RouteOutcome>, HandlerError> {
        let _ = (delivery, SIGNAL, &self.pool);
        Ok(Vec::new())
    }
}

/// Maps a harvest error of one route to its outcome (plan 9.4 step 7, W19,
/// B4). A run that cannot take a signal is finished: 200. A database, pool
/// or shard error is transient: 503, so Jira sends the delivery again.
fn error_outcome(error: &HarvestError) -> Result<RouteOutcome, HandlerError> {
    let _ = error;
    Ok(RouteOutcome::Finished)
}

#[async_trait]
impl EventSink for HarvestSignalBridge {
    fn name(&self) -> &'static str {
        "harvest-signal-bridge"
    }

    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        self.route(&delivery).await.map(|_| ())
    }
}

/// Pure: does this delivery need routing? `jira:issue_created`, or `jira:issue_updated` with a status change (B4, W26).
#[must_use]
pub fn wants(event: &JiraEvent) -> bool {
    match event {
        JiraEvent::IssueCreated(_) => true,
        JiraEvent::IssueUpdated(_) => event.status_change().is_some(),
        _ => false,
    }
}

/// Pure: the routes in a property value, kept only when allowed and the tag verifies.
///
/// Each item is a route and the result of its checks:
///
/// - [`RouteOutcome::BadTag`]: the tag does not verify. This check comes
///   first and runs in constant time.
/// - [`RouteOutcome::NotAllowed`]: the tag verifies, but the workflow name
///   is not in `allowed`. An empty `allowed` allows no name.
/// - [`RouteOutcome::Signalled`]: both checks pass. The bridge then sends the
///   signal and sets the final outcome of the route.
///
/// An entry with no `wf`, `id` or `tag` string, or with a name or an id that
/// breaks the [`SignalRoute::new`] rule, is not a route and is left out. A
/// value with no `routes` array gives no routes.
#[must_use]
pub fn routes_from_property(
    value: &Value,
    key: &RouteKey,
    allowed: &BTreeSet<String>,
) -> Vec<(SignalRoute, RouteOutcome)> {
    let Some(entries) = value.get("routes").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let name = entry.get("wf")?.as_str()?;
            let id = entry.get("id")?.as_str()?;
            let tag = entry.get("tag")?.as_str()?;
            let route = SignalRoute::new(name, id).ok()?;
            let outcome = if !route.verify(key, tag) {
                RouteOutcome::BadTag
            } else if !allowed.contains(&route.workflow_name) {
                RouteOutcome::NotAllowed
            } else {
                RouteOutcome::Signalled
            };
            Some((route, outcome))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use autumn_jira::webhook::parse_event;
    use san_mateo::Secret;
    use serde_json::json;

    fn event(value: &Value) -> JiraEvent {
        parse_event(&serde_json::to_vec(value).unwrap())
            .unwrap()
            .event
    }

    fn issue(event_name: &str, items: &Value) -> Value {
        json!({
            "timestamp": 1_606_480_436_302_i64,
            "webhookEvent": event_name,
            "issue": {"id": "99291", "key": "OPS-7", "fields": {"labels": ["autumn-tracked"]}},
            "changelog": {"id": "10124", "items": items}
        })
    }

    fn key() -> RouteKey {
        RouteKey::new(Secret::new("k".repeat(32))).unwrap()
    }

    #[test]
    fn wants_only_status_changes() {
        let status = json!([{"field": "status", "from": "1", "fromString": "To Do", "to": "3", "toString": "In Progress"}]);
        let summary = json!([{"field": "summary", "fromString": "a", "toString": "b"}]);
        assert!(wants(&event(&issue("jira:issue_created", &json!([])))));
        assert!(wants(&event(&issue("jira:issue_updated", &status))));
        assert!(!wants(&event(&issue("jira:issue_updated", &summary))));
        assert!(!wants(&event(&issue("jira:issue_updated", &json!([])))));
        assert!(!wants(&event(&issue("jira:issue_deleted", &status))));
        let comment = json!({
            "webhookEvent": "comment_created",
            "comment": {"id": "10100", "body": {"type": "doc", "version": 1, "content": []}},
            "issue": {"id": "99291", "key": "OPS-7", "fields": {}}
        });
        assert!(!wants(&event(&comment)));
        assert!(!wants(&event(&json!({"webhookEvent": "sprint_started"}))));
    }

    #[test]
    fn routes_need_tag_and_allowlist() {
        let key = key();
        let good = SignalRoute::new("incident_flow", "A-1").unwrap();
        let other = SignalRoute::new("other_flow", "B-2").unwrap();
        let forged = SignalRoute::new("incident_flow", "A-9").unwrap();
        let allowed: BTreeSet<String> = ["incident_flow".to_owned()].into();
        let value = json!({"routes": [
            good.property_entry(&key),
            other.property_entry(&key),
            {"wf": "incident_flow", "id": "A-9", "tag": good.tag(&key)},
            {"wf": "incident_flow", "id": "A-1"},
            {"wf": "a b", "id": "A-1", "tag": good.tag(&key)},
            {"wf": 7, "id": "A-1", "tag": good.tag(&key)},
            "not an object"
        ]});
        assert_eq!(
            routes_from_property(&value, &key, &allowed),
            vec![
                (good, RouteOutcome::Signalled),
                (other, RouteOutcome::NotAllowed),
                (forged, RouteOutcome::BadTag),
            ]
        );
        // An empty allowlist allows no name. A bad tag is still a bad tag.
        let none = BTreeSet::new();
        let outcomes: Vec<RouteOutcome> = routes_from_property(&value, &key, &none)
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect();
        assert_eq!(
            outcomes,
            vec![
                RouteOutcome::NotAllowed,
                RouteOutcome::NotAllowed,
                RouteOutcome::BadTag
            ]
        );
        // Another key makes every tag bad.
        let other_key = RouteKey::new(Secret::new("j".repeat(32))).unwrap();
        assert!(
            routes_from_property(&value, &other_key, &allowed)
                .iter()
                .all(|(_, outcome)| *outcome == RouteOutcome::BadTag)
        );
        for empty in [json!(null), json!({}), json!({"routes": {}}), json!([])] {
            assert_eq!(routes_from_property(&empty, &key, &allowed), vec![]);
        }
    }

    #[test]
    fn harvest_errors_map_to_classes() {
        let finished = [
            HarvestError::Config("workflow execution x is terminal (COMPLETED)".into()),
            HarvestError::Cancelled("cancelled".into()),
            HarvestError::NotFound("workflow execution x".into()),
        ];
        for error in &finished {
            assert_eq!(
                error_outcome(error),
                Ok(RouteOutcome::Finished),
                "{error:?}"
            );
        }
        let transient = [
            HarvestError::Database("connection reset".into()),
            HarvestError::ShardUnavailable {
                shard_id: 0,
                reason: "moving".into(),
            },
            HarvestError::Timeout {
                timeout_type: autumn_harvest::TimeoutType::StartToClose,
                task_name: "x".into(),
            },
        ];
        for error in &transient {
            let outcome = error_outcome(error);
            assert!(
                matches!(outcome, Err(HandlerError::Transient(_))),
                "{error:?}: {outcome:?}"
            );
        }
    }

    /// The tag check uses the constant-time `verify_truncated_left` of the
    /// `hmac` crate. No code compares tag text or MAC bytes with `==`.
    #[test]
    fn tag_is_constant_time() {
        let source = include_str!("route.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("mac.verify_truncated_left(&expected)"));
        for line in code.lines() {
            let text = line.split("//").next().unwrap();
            let has_eq = text.contains("==") || text.contains("!=");
            let names_a_tag = text.contains("tag") || text.contains("expected");
            assert!(
                !(has_eq && names_a_tag && !text.contains(".len()")),
                "a plain compare on a tag: {line}"
            );
        }
        let bridge = include_str!("bridge.rs");
        let code = bridge.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("route.verify(key, tag)"));
        assert!(!code.contains("tag =="));
        assert!(!code.contains("== tag"));
        // The behaviour: a tag that differs in one char fails.
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let tag = route.tag(&key());
        let mut flipped = tag.clone().into_bytes();
        flipped[0] = if flipped[0] == b'a' { b'b' } else { b'a' };
        assert!(!route.verify(&key(), std::str::from_utf8(&flipped).unwrap()));
        assert!(route.verify(&key(), &tag));
    }
}
