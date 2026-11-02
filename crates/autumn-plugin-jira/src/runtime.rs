//! The started plugin: the service, the webhook handlers and the counters.

use std::sync::Arc;

use serde::Serialize;

use crate::health::JiraHealth;
use crate::service::JiraService;

/// The webhook counters of one process.
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
    /// Deliveries from an ignored actor.
    pub ignored_self: u64,
    /// Deliveries of an event that the plugin does not dispatch.
    pub ignored_unknown: u64,
    /// Deliveries that do not parse.
    pub undecodable: u64,
    /// Permanent handler errors.
    pub permanent: u64,
    /// Transient handler errors.
    pub handler_failed: u64,
    /// Handlers that did not finish in time.
    pub timed_out: u64,
}

/// A started plugin.
#[derive(Debug)]
pub struct JiraRuntime {
    service: JiraService,
    health: Arc<JiraHealth>,
}

impl JiraRuntime {
    /// The service.
    #[must_use]
    pub const fn service(&self) -> &JiraService {
        &self.service
    }

    /// The health indicator.
    #[must_use]
    pub fn health(&self) -> Arc<JiraHealth> {
        Arc::clone(&self.health)
    }

    /// The webhook counters.
    #[must_use]
    pub fn counters(&self) -> WebhookCounters {
        WebhookCounters::default()
    }
}
