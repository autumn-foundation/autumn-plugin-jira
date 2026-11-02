//! The webhook-to-signal bridge (plan 9.4).
//!
//! [`HarvestSignalBridge`] is an [`EventSink`]. For a delivery that needs
//! routing, the bridge reads the route property of the issue. Then it reads
//! the status, the id and the labels of the issue. It checks the tag of
//! each route against that issue, and the allowlist. It sends [`SIGNAL`] to
//! the run with `send_signal_idempotent`. The delivery id is the
//! idempotency key, so a repeated delivery gives one signal. Webhooks are at
//! least once.
//!
//! Two sites can have the same issue id, and one webhook secret can serve
//! two sites by mistake. So the bridge does not trust the payload for the
//! issue. It reads the status and the key from the site of its own client
//! (R2-webhook-security-1). One bridge serves one site. The plugin takes
//! the webhooks of one Jira site for each app (plan V114), so an app has
//! one webhook endpoint, one webhook secret and one bridge. A wait on an
//! issue of another site ends by poll.
//!
//! When the bridge knows its site host, it also refuses a delivery whose
//! `issue.self` names another host or no host (R2F-fixcheck-a-2). It makes
//! no call for such a delivery and counts it in `other_site`. A client of a
//! subdomain site tells its host. For a san-mateo client or a cloud id
//! client, give the host with [`HarvestSignalBridge::site_host`].
//!
//! [`wants`] and [`routes_from_property`] are pure. They do no I/O.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use autumn_harvest::HarvestError;
use autumn_harvest::erase::is_terminal_state;
use autumn_harvest::resolve_execution_id_by_workflow_id;
use autumn_harvest::signal::send_signal_idempotent;
use autumn_harvest::worker::DbPool;
use autumn_jira::backend::{BackendInfo, BackendMode};
use autumn_jira::ids::{FieldId, SiteHost};
use autumn_jira::model::Issue;
use autumn_jira::request::FieldSet;
use autumn_jira::{Delivery, EventSink, HandlerError, IssueId, JiraClient, JiraEvent};
use serde::Serialize;
use serde_json::Value;

use crate::route::{ROUTES_PROPERTY, RouteBinding, RouteKey, SignalRoute, TRACKED_LABEL};
use crate::wait::{IssueSignal, SIGNAL};

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
    /// Deliveries of another site, or of no named site. The bridge made no
    /// call for them.
    pub other_site: u64,
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
    other_site: AtomicU64,
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
            other_site: read(&self.other_site),
        }
    }
}

/// The sink that turns Jira deliveries into harvest signals.
///
/// Limits (ADR 0006): the bridge uses one shard. A multi-shard engine needs
/// a fan-out lookup. The workflow checks the issue id of each signal, so a
/// route on another issue cannot move a workflow. The bridge serves the
/// site of its client only. The status in a signal comes from a read on
/// that site, not from the payload. So a delivery of another site with the
/// same issue id cannot give a false status.
#[derive(Clone)]
pub struct HarvestSignalBridge {
    pool: DbPool,
    client: JiraClient,
    site: Option<SiteHost>,
    key: RouteKey,
    allowed: BTreeSet<String>,
    accept_v1: bool,
    counters: Arc<Counters>,
}

impl fmt::Debug for HarvestSignalBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HarvestSignalBridge")
            .field("client", &self.client)
            .field("site", &self.site)
            .field("key", &self.key)
            .field("allowed", &self.allowed)
            .field("accept_v1", &self.accept_v1)
            .field("counters", &self.counters.snapshot())
            .finish_non_exhaustive()
    }
}

impl HarvestSignalBridge {
    /// Makes a bridge with an empty allowlist. An empty allowlist allows no
    /// workflow, so call [`Self::allow_workflow`] for each workflow name.
    ///
    /// A client of a subdomain site gives the site host of the bridge. Other
    /// clients give no host. Then the bridge does not check the site of a
    /// delivery until you call [`Self::site_host`].
    #[must_use]
    pub fn new(pool: DbPool, client: JiraClient, key: RouteKey) -> Self {
        let site = client_host(&client.info());
        Self {
            pool,
            client,
            site,
            key,
            allowed: BTreeSet::new(),
            accept_v1: false,
            counters: Arc::default(),
        }
    }

    /// Allows routes to the workflow with this name.
    #[must_use]
    pub fn allow_workflow(mut self, workflow_name: &str) -> Self {
        self.allowed.insert(workflow_name.to_owned());
        self
    }

    /// Sets the site host of the bridge, for example `acme.atlassian.net`.
    /// The bridge then refuses each delivery whose `issue.self` names
    /// another host, or names no host. For a san-mateo client, use the host
    /// of the site URL in `accessible-resources`:
    /// `SiteHost::from_url(&resource.url)`.
    #[must_use]
    pub fn site_host(mut self, host: SiteHost) -> Self {
        self.site = Some(host);
        self
    }

    /// Opt-in for an upgrade: the bridge also takes a v1 tag.
    ///
    /// A v1 tag binds the run, not the issue. A person who can edit issue
    /// properties can copy a v1 entry to another issue. Then each status
    /// change of that issue signals the run (R2F-fixcheck-a-3). A run
    /// that started before the upgrade has only v1 entries. Turn this on
    /// until those runs end, then turn it off.
    #[must_use]
    pub const fn accept_unbound_v1_tags(mut self) -> Self {
        self.accept_v1 = true;
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
        bump(&self.counters.deliveries);
        if let Some(own) = &self.site
            && delivery.site() != Some(own)
        {
            bump(&self.counters.other_site);
            tracing::warn!(
                site = %own,
                "jira bridge: a delivery names another site or no site; no call"
            );
            return Ok(Vec::new());
        }
        let result = self.route_inner(delivery).await;
        match &result {
            Ok(outcomes) if outcomes.is_empty() => bump(&self.counters.skipped),
            Ok(outcomes) => {
                for outcome in outcomes {
                    bump(self.counter(*outcome));
                }
            }
            Err(HandlerError::Transient(_)) => bump(&self.counters.transient),
            Err(_) => {}
        }
        result
    }

    /// The counter of one route outcome.
    fn counter(&self, outcome: RouteOutcome) -> &AtomicU64 {
        let c = &self.counters;
        match outcome {
            RouteOutcome::Signalled => &c.signalled,
            RouteOutcome::Duplicate => &c.duplicate,
            RouteOutcome::NoRun => &c.no_run,
            RouteOutcome::Finished => &c.finished,
            RouteOutcome::NotAllowed => &c.not_allowed,
            RouteOutcome::BadTag => &c.bad_tag,
        }
    }

    /// Steps 1 to 6 of plan 9.4. The tag binds the issue, so the issue read
    /// comes before the tag check. A forged route makes no database call.
    async fn route_inner(&self, delivery: &Delivery) -> Result<Vec<RouteOutcome>, HandlerError> {
        let event = &delivery.event;
        let Some(issue) = event.issue().filter(|_| wants(event)) else {
            return Ok(Vec::new());
        };
        // A payload with labels and no tracked label: nothing to read.
        if issue
            .fields
            .labels
            .as_ref()
            .is_some_and(|labels| !labels.iter().any(|l| l == TRACKED_LABEL))
        {
            return Ok(Vec::new());
        }
        let Some(property) = self
            .client
            .get_issue_property(issue.id.clone(), ROUTES_PROPERTY)
            .await?
        else {
            return Ok(Vec::new());
        };
        if route_entries(&property).is_empty() {
            return Ok(Vec::new());
        }
        let (current, bindings) = self.read_issue(&issue.id).await?;
        let routes = routes_from_property(
            &property,
            &self.key,
            &self.allowed,
            &bindings,
            self.accept_v1,
        );
        let signal = routes
            .iter()
            .any(|(_, checked)| *checked == RouteOutcome::Signalled)
            .then(|| signal_of(delivery, &current, self.site.is_some()));
        let payload = signal
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| HandlerError::permanent("the signal payload does not encode"))?;
        let mut outcomes = Vec::new();
        for (route, checked) in routes {
            let outcome = match (checked, &signal, &payload) {
                (RouteOutcome::Signalled, Some(signal), Some(payload)) => {
                    self.signal(&route, payload, signal).await?
                }
                (RouteOutcome::BadTag, _, _) => {
                    tracing::warn!(
                        issue_id = %issue.id,
                        "jira bridge: a route tag does not verify; no signal"
                    );
                    RouteOutcome::BadTag
                }
                (other, _, _) => other,
            };
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Reads the status and the labels of the issue on the site of the
    /// client. Returns the issue and the bindings that a tag can have on it:
    /// its id and each of its labels.
    async fn read_issue(
        &self,
        issue_id: &IssueId,
    ) -> Result<(Issue, Vec<RouteBinding>), HandlerError> {
        let fields = FieldSet::new(
            ["status", "labels"]
                .into_iter()
                .filter_map(|f| FieldId::parse(f).ok()),
        );
        let current = self
            .client
            .get_issue_with(issue_id.clone(), &fields)
            .await?;
        let bindings = std::iter::once(RouteBinding::Issue(current.id.clone()))
            .chain(
                current
                    .fields
                    .labels
                    .iter()
                    .cloned()
                    .map(RouteBinding::MarkerLabel),
            )
            .collect();
        Ok((current, bindings))
    }

    /// Steps 5 and 6: resolve the run and send the signal once for the
    /// delivery id.
    async fn signal(
        &self,
        route: &SignalRoute,
        payload: &Value,
        signal: &IssueSignal,
    ) -> Result<RouteOutcome, HandlerError> {
        let mut conn = self.pool.get().await.map_err(|_| {
            HandlerError::transient("jira bridge: the harvest database pool has no connection")
        })?;
        let run = match resolve_execution_id_by_workflow_id(
            &mut conn,
            &route.workflow_name,
            &route.workflow_id,
        )
        .await
        {
            Ok(Some(run)) => run,
            Ok(None) => return Ok(RouteOutcome::NoRun),
            Err(error) => return error_outcome(&error),
        };
        if is_terminal_state(&run.state) {
            return Ok(RouteOutcome::Finished);
        }
        match send_signal_idempotent(
            &mut conn,
            run.exec_id,
            SIGNAL,
            payload.clone(),
            Some(signal.delivery_id.as_str()),
        )
        .await
        {
            Ok(true) => Ok(RouteOutcome::Signalled),
            Ok(false) => Ok(RouteOutcome::Duplicate),
            Err(error) => error_outcome(&error),
        }
    }
}

/// The site host of a client: the host of a subdomain site. A cloud id
/// site and a san-mateo connection give `None`.
fn client_host(info: &BackendInfo) -> Option<SiteHost> {
    if info.mode != BackendMode::ApiToken {
        return None;
    }
    SiteHost::parse(&info.site).ok()
}

/// Makes the signal of a delivery. The id and the key come from a read of
/// the issue on the site of the client, not from the payload.
///
/// The status depends on `site_checked`: `true` when the bridge knows its
/// site host and the delivery names that host. Then the delivery comes from
/// the site of the bridge. The signal gets the status that the payload
/// reports, together with the time of the payload. An issue can pass a
/// status in a few seconds, and the bridge can get the delivery after the
/// next move. The reported status still ends a wait for it
/// (R2C-harvest-4). Else, or when the payload has no status, the signal
/// gets the status of the read. A bridge with no site host loses a brief
/// status in that case, so give each bridge its site host.
fn signal_of(delivery: &Delivery, current: &Issue, site_checked: bool) -> IssueSignal {
    let reported = delivery
        .event
        .issue()
        .and_then(|issue| issue.fields.status.clone())
        .filter(|_| site_checked);
    IssueSignal {
        delivery_id: delivery.meta.id.clone(),
        event: delivery.event.name().to_owned(),
        issue_id: current.id.clone(),
        issue_key: Some(current.key.clone()),
        status: reported.or_else(|| current.status()),
        timestamp_ms: delivery.timestamp.map(|t| t.epoch_ms()),
    }
}

/// Adds one to a counter.
fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Maps a harvest error of one route to its outcome (plan 9.4 step 7, W19,
/// B4). A run that cannot take a signal is finished: 200. A database, pool
/// or shard error is transient: 503, so Jira sends the delivery again.
fn error_outcome(error: &HarvestError) -> Result<RouteOutcome, HandlerError> {
    match error {
        HarvestError::Config(_) | HarvestError::Cancelled(_) | HarvestError::NotFound(_) => {
            Ok(RouteOutcome::Finished)
        }
        // The message names the class only: a database error can hold a
        // host name or SQL text.
        HarvestError::ShardUnavailable { .. } => Err(HandlerError::transient(
            "jira bridge: the harvest shard is not available",
        )),
        _ => Err(HandlerError::transient(
            "jira bridge: the harvest database call failed",
        )),
    }
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
/// `bindings` are the bindings of the issue that the bridge read: its id
/// and its labels. A v2 tag verifies when it is the tag of one of them. A
/// v1 tag binds no issue, so it verifies only with `accept_v1`.
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
/// Some entries are not routes, and the result does not hold them. Such an
/// entry has no `wf`, `id` or `tag` string. Or its name or id breaks the
/// [`SignalRoute::new`] rule. A value with no `routes` array gives no
/// routes.
#[must_use]
pub fn routes_from_property(
    value: &Value,
    key: &RouteKey,
    allowed: &BTreeSet<String>,
    bindings: &[RouteBinding],
    accept_v1: bool,
) -> Vec<(SignalRoute, RouteOutcome)> {
    route_entries(value)
        .into_iter()
        .map(|(route, tag)| {
            let verified = bindings
                .iter()
                .any(|binding| route.verify(key, binding, tag))
                || (accept_v1 && route.verify_v1(key, tag));
            let outcome = if !verified {
                RouteOutcome::BadTag
            } else if !allowed.contains(&route.workflow_name) {
                RouteOutcome::NotAllowed
            } else {
                RouteOutcome::Signalled
            };
            (route, outcome)
        })
        .collect()
}

/// The route entries of a property value, with their tag text. An entry
/// with no `wf`, `id` or `tag` string, or with a bad name or id, is left
/// out.
fn route_entries(value: &Value) -> Vec<(SignalRoute, &str)> {
    let Some(entries) = value.get("routes").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let name = entry.get("wf")?.as_str()?;
            let id = entry.get("id")?.as_str()?;
            let tag = entry.get("tag")?.as_str()?;
            Some((SignalRoute::new(name, id).ok()?, tag))
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

    fn bindings() -> Vec<RouteBinding> {
        vec![
            RouteBinding::Issue(IssueId::parse("99291").unwrap()),
            RouteBinding::MarkerLabel("autumn-tracked".into()),
            RouteBinding::MarkerLabel("autumn-idem-x".into()),
        ]
    }

    #[test]
    fn routes_need_tag_and_allowlist() {
        let key = key();
        let issue = RouteBinding::Issue(IssueId::parse("99291").unwrap());
        let marker = RouteBinding::MarkerLabel("autumn-idem-x".into());
        let good = SignalRoute::new("incident_flow", "A-1").unwrap();
        let other = SignalRoute::new("other_flow", "B-2").unwrap();
        let forged = SignalRoute::new("incident_flow", "A-9").unwrap();
        let allowed: BTreeSet<String> = ["incident_flow".to_owned()].into();
        let value = json!({"routes": [
            good.property_entry(&key, &issue),
            other.property_entry(&key, &marker),
            {"wf": "incident_flow", "id": "A-9", "tag": good.tag(&key, &issue)},
            {"wf": "incident_flow", "id": "A-1"},
            {"wf": "a b", "id": "A-1", "tag": good.tag(&key, &issue)},
            {"wf": 7, "id": "A-1", "tag": good.tag(&key, &issue)},
            "not an object"
        ]});
        assert_eq!(
            routes_from_property(&value, &key, &allowed, &bindings(), false),
            vec![
                (good.clone(), RouteOutcome::Signalled),
                (other, RouteOutcome::NotAllowed),
                (forged, RouteOutcome::BadTag),
            ]
        );
        // A copy on another issue: no binding of that issue verifies.
        let elsewhere = [
            RouteBinding::Issue(IssueId::parse("99292").unwrap()),
            RouteBinding::MarkerLabel("autumn-tracked".into()),
        ];
        assert!(
            routes_from_property(&value, &key, &allowed, &elsewhere, false)
                .iter()
                .all(|(_, outcome)| *outcome == RouteOutcome::BadTag)
        );
        // A v1 tag verifies only with the opt-in.
        let v1 = json!({"routes": [
            {"wf": "incident_flow", "id": "A-1", "tag": "df60e928addb470f158ced8d9f527e82"}
        ]});
        assert_eq!(
            routes_from_property(&v1, &key, &allowed, &bindings(), false),
            vec![(good.clone(), RouteOutcome::BadTag)]
        );
        assert_eq!(
            routes_from_property(&v1, &key, &allowed, &[], true),
            vec![(good, RouteOutcome::Signalled)]
        );
        // An empty allowlist allows no name. A bad tag is still a bad tag.
        let none = BTreeSet::new();
        let outcomes: Vec<RouteOutcome> =
            routes_from_property(&value, &key, &none, &bindings(), false)
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
            routes_from_property(&value, &other_key, &allowed, &bindings(), true)
                .iter()
                .all(|(_, outcome)| *outcome == RouteOutcome::BadTag)
        );
        for empty in [json!(null), json!({}), json!({"routes": {}}), json!([])] {
            assert_eq!(
                routes_from_property(&empty, &key, &allowed, &bindings(), true),
                vec![]
            );
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
        assert!(code.contains("route.verify(key, binding, tag)"));
        assert!(code.contains("route.verify_v1(key, tag)"));
        assert!(!code.contains("tag =="));
        assert!(!code.contains("== tag"));
        // The behaviour: a tag that differs in one char fails.
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let binding = RouteBinding::MarkerLabel("autumn-idem-x".into());
        let tag = route.tag(&key(), &binding);
        let mut flipped = tag.clone().into_bytes();
        flipped[0] = if flipped[0] == b'a' { b'b' } else { b'a' };
        assert!(!route.verify(&key(), &binding, std::str::from_utf8(&flipped).unwrap()));
        assert!(route.verify(&key(), &binding, &tag));
    }
}
