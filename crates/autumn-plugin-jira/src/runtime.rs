//! The started plugin: the service, the webhook handlers and the counters.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use autumn_jira::{Delivery, EventSink, HandlerError, JiraClient};
use autumn_web::AppState;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::config::{JiraHealthConfig, JiraWebhookConfig};
use crate::health::JiraHealth;
use crate::service::JiraService;

/// The runtime, set one time at startup. The route answers 503 before that.
pub(crate) type Cell = Arc<OnceLock<Arc<Runtime>>>;

/// The future of an `on_event` closure.
pub(crate) type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), HandlerError>> + Send>>;

/// An `on_event` closure, with its future boxed.
pub(crate) type OnEvent = Arc<dyn Fn(Arc<Delivery>, AppState) -> HandlerFuture + Send + Sync>;

/// One webhook handler: a sink, or an `on_event` closure.
#[derive(Clone)]
pub(crate) enum Handler {
    /// An [`EventSink`].
    Sink(Arc<dyn EventSink>),
    /// An `on_event` closure.
    OnEvent(OnEvent),
    /// An [`EventSink`] that also gets the events of the ignored actors.
    AllActors(Arc<dyn EventSink>),
}

impl Handler {
    /// A name for logs.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Sink(sink) | Self::AllActors(sink) => sink.name(),
            Self::OnEvent(_) => "on_event",
        }
    }

    /// `true` when the handler also gets the events of the ignored actors.
    pub(crate) const fn all_actors(&self) -> bool {
        matches!(self, Self::AllActors(_))
    }

    /// Runs the handler on one delivery.
    pub(crate) fn run(&self, delivery: Arc<Delivery>, state: AppState) -> HandlerFuture {
        match self {
            Self::Sink(sink) | Self::AllActors(sink) => {
                let sink = Arc::clone(sink);
                Box::pin(async move { sink.deliver(delivery).await })
            }
            Self::OnEvent(f) => f(delivery, state),
        }
    }
}

/// The webhook counters of one process.
///
/// Each delivery that passes the autumn signature check adds one to
/// exactly one of `accepted`, `ignored_*`, `undecodable`, `handler_failed`
/// and `timed_out`. `permanent` counts the permanent handler errors of
/// accepted deliveries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct WebhookCounters {
    /// Deliveries that the handlers took.
    pub accepted: u64,
    /// Deliveries older than `max_age_secs`.
    pub ignored_stale: u64,
    /// Deliveries more than `max_future_skew_secs` in the future.
    pub ignored_future: u64,
    /// Deliveries with no `timestamp`.
    pub ignored_no_timestamp: u64,
    /// Deliveries from an actor in `ignore_actor_account_ids`, when no
    /// sink for all actors exists. With such a sink, the delivery goes to
    /// that sink only, and it counts as the other deliveries do.
    pub ignored_self: u64,
    /// Deliveries of an event that the plugin does not dispatch.
    pub ignored_unknown: u64,
    /// Deliveries that do not parse.
    pub undecodable: u64,
    /// Permanent handler errors. The route answers 200 for them.
    pub permanent: u64,
    /// Deliveries with a transient handler error. The route answers 503.
    pub handler_failed: u64,
    /// Deliveries with a handler that did not finish in time, that found
    /// no free handler slot (`max_handler_tasks`), or that autumn stopped
    /// at its request timeout. The route answers 503.
    pub timed_out: u64,
}

/// The live counters.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    pub(crate) accepted: AtomicU64,
    pub(crate) ignored_stale: AtomicU64,
    pub(crate) ignored_future: AtomicU64,
    pub(crate) ignored_no_timestamp: AtomicU64,
    pub(crate) ignored_self: AtomicU64,
    pub(crate) ignored_unknown: AtomicU64,
    pub(crate) undecodable: AtomicU64,
    pub(crate) permanent: AtomicU64,
    pub(crate) handler_failed: AtomicU64,
    pub(crate) timed_out: AtomicU64,
}

impl Counters {
    /// Adds one.
    pub(crate) fn add(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// The values now.
    pub(crate) fn snapshot(&self) -> WebhookCounters {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        WebhookCounters {
            accepted: get(&self.accepted),
            ignored_stale: get(&self.ignored_stale),
            ignored_future: get(&self.ignored_future),
            ignored_no_timestamp: get(&self.ignored_no_timestamp),
            ignored_self: get(&self.ignored_self),
            ignored_unknown: get(&self.ignored_unknown),
            undecodable: get(&self.undecodable),
            permanent: get(&self.permanent),
            handler_failed: get(&self.handler_failed),
            timed_out: get(&self.timed_out),
        }
    }
}

/// The started state that the route, the health indicator and the startup
/// hook share.
pub(crate) struct Runtime {
    /// The service in the app state.
    pub(crate) service: JiraService,
    /// The client of the health check: one attempt for each call. `None`
    /// when `[jira.san_mateo]` names no `connection_id`: then there is no
    /// default connection to probe.
    pub(crate) health_client: Option<JiraClient>,
    /// `[jira.webhook]`.
    pub(crate) webhook: JiraWebhookConfig,
    /// `[jira.health]`.
    pub(crate) health: JiraHealthConfig,
    /// The webhook handlers, in order.
    pub(crate) handlers: Vec<Handler>,
    /// The webhook counters.
    pub(crate) counters: Counters,
    /// `api_token` or `san_mateo`.
    pub(crate) mode: &'static str,
    /// `max_body_bytes` of the webhook endpoint, when the route is mounted.
    pub(crate) max_body_bytes: Option<usize>,
    /// One permit for each handler task that can run:
    /// `[jira.webhook] max_handler_tasks`.
    pub(crate) slots: Arc<Semaphore>,
}

impl Runtime {
    /// The handler tasks that run now, also the tasks that timed out.
    pub(crate) fn handler_tasks(&self) -> usize {
        slot_count(self.webhook.max_handler_tasks, Semaphore::MAX_PERMITS)
            .saturating_sub(self.slots.available_permits())
    }
}

/// The number of handler slots: `max_handler_tasks`, but not more than
/// `cap`. Give `Semaphore::MAX_PERMITS` as `cap`. tokio panics for a
/// semaphore with more permits than that. On a 32-bit target, a `u32`
/// value can be more than that limit.
pub(crate) fn slot_count(max_handler_tasks: u32, cap: usize) -> usize {
    usize::try_from(max_handler_tasks).unwrap_or(cap)
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("service", &self.service)
            .field("mode", &self.mode)
            .field("handlers", &self.handlers.len())
            .field("counters", &self.counters.snapshot())
            .finish_non_exhaustive()
    }
}

/// A started plugin. [`JiraPlugin::start`](crate::JiraPlugin::start) gives
/// it, for tests and for apps that build no autumn app.
#[derive(Debug)]
pub struct JiraRuntime {
    runtime: Arc<Runtime>,
    health: Arc<JiraHealth>,
}

impl JiraRuntime {
    /// Wraps the started state.
    pub(crate) const fn new(runtime: Arc<Runtime>, health: Arc<JiraHealth>) -> Self {
        Self { runtime, health }
    }

    /// The service.
    #[must_use]
    pub fn service(&self) -> &JiraService {
        &self.runtime.service
    }

    /// The health indicator.
    #[must_use]
    pub fn health(&self) -> Arc<JiraHealth> {
        Arc::clone(&self.health)
    }

    /// The webhook counters now.
    #[must_use]
    pub fn counters(&self) -> WebhookCounters {
        self.runtime.counters.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R2B-webhooks-secrets-3, R2D-integration-6: a large
    /// `max_handler_tasks` does not make tokio panic at the start.
    #[test]
    fn the_slot_count_stays_at_the_semaphore_cap() {
        // `Semaphore::MAX_PERMITS` on a 32-bit target.
        let cap_32 = usize::try_from(u32::MAX >> 3).unwrap();
        assert_eq!(slot_count(u32::MAX, cap_32), cap_32);
        assert_eq!(slot_count(64, cap_32), 64);
        let n = slot_count(u32::MAX, Semaphore::MAX_PERMITS);
        assert!(n <= Semaphore::MAX_PERMITS);
        assert_eq!(Semaphore::new(n).available_permits(), n);
    }
}
