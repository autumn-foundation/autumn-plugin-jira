//! The `jira` health indicator for `/actuator/health`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use autumn_web::actuator::{HealthCheckOutput, HealthIndicator, IndicatorGroup};
use autumn_web::reexports::chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::config::JiraHealthConfig;
use crate::runtime::{Cell, Runtime};

/// The result of the last call to Jira.
#[derive(Debug, Clone)]
struct Probe {
    /// When the plugin made the call, in tokio time.
    at: Instant,
    /// `true` when Jira answered `myself`.
    up: bool,
    /// The HTTP status of the reply, if Jira replied.
    status: Option<u16>,
    /// The wall time of the call, RFC 3339.
    checked_at: String,
    /// The error code, when the call failed.
    error: Option<&'static str>,
}

/// The `jira` health indicator.
///
/// - It is in [`IndicatorGroup::HealthOnly`]: a Jira outage shows in
///   `/actuator/health`, and `/ready` stays 200. The app can still serve
///   requests that do not need Jira.
/// - `check` calls `GET /myself` with one attempt, at most one time in
///   `[jira.health] cache_secs`. In the other checks it gives the last
///   result again.
/// - A failed call, or a call that does not finish inside the timeout,
///   gives `DOWN`.
/// - Before the start: `DOWN` with `reason = "not started"`.
///
/// The details hold the mode, the site, the last HTTP status, the time of
/// the last call, the webhook counters, `self_filter` and the
/// `max_body_bytes` of the endpoint. They never hold an email, an account
/// id or a token.
pub struct JiraHealth {
    cell: Cell,
    failed: AtomicBool,
    last: Mutex<Option<Probe>>,
}

impl std::fmt::Debug for JiraHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraHealth")
            .field("started", &self.cell.get().is_some())
            .field("failed", &self.failed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl JiraHealth {
    /// Makes the indicator of the runtime in `cell`.
    pub(crate) const fn new(cell: Cell) -> Self {
        Self {
            cell,
            failed: AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }

    /// Records that the start failed.
    pub(crate) fn mark_failed(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }

    /// The `[jira.health]` settings: the started ones, else the defaults.
    fn settings(&self) -> JiraHealthConfig {
        self.cell
            .get()
            .map_or_else(JiraHealthConfig::default, |rt| rt.health.clone())
    }

    /// Runs the check.
    async fn run(&self) -> HealthCheckOutput {
        let Some(runtime) = self.cell.get() else {
            let reason = if self.failed.load(Ordering::SeqCst) {
                "start failed"
            } else {
                "not started"
            };
            return HealthCheckOutput::down()
                .with_details(HashMap::from([("reason".to_owned(), json!(reason))]));
        };
        let probe = self.probe(runtime).await;
        output(runtime, &probe)
    }

    /// The cached probe, or a new call to Jira.
    async fn probe(&self, runtime: &Runtime) -> Probe {
        let ttl = Duration::from_secs(runtime.health.cache_secs);
        let cached = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .filter(|p| p.at.elapsed() < ttl);
        if let Some(probe) = cached {
            return probe;
        }
        // Stop before autumn does: autumn gives `UNKNOWN` at its timeout,
        // and the plugin gives `DOWN` with the reason.
        let limit =
            Duration::from_millis((runtime.health.timeout_ms.saturating_mul(9) / 10).max(1));
        let at = Instant::now();
        let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let (up, status, error) =
            match tokio::time::timeout(limit, runtime.health_client.myself()).await {
                Ok(Ok(_)) => (true, Some(200), None),
                Ok(Err(e)) => (false, e.status(), Some(e.code())),
                Err(_) => (false, None, Some("timeout")),
            };
        let probe = Probe {
            at,
            up,
            status,
            checked_at,
            error,
        };
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(probe.clone());
        probe
    }
}

/// The output of one probe, with the live counters.
fn output(runtime: &Runtime, probe: &Probe) -> HealthCheckOutput {
    let info = runtime.service.info();
    let self_filter = if runtime.webhook.ignore_actor_account_ids.is_empty() {
        "off"
    } else {
        "on"
    };
    let mut details: HashMap<String, Value> = HashMap::from([
        ("mode".to_owned(), json!(runtime.mode)),
        ("site".to_owned(), json!(info.site)),
        ("status".to_owned(), json!(probe.status)),
        ("checked_at".to_owned(), json!(probe.checked_at)),
        ("webhook".to_owned(), json!(runtime.counters.snapshot())),
        ("self_filter".to_owned(), json!(self_filter)),
        ("max_body_bytes".to_owned(), json!(runtime.max_body_bytes)),
    ]);
    if let Some(code) = probe.error {
        details.insert("error".to_owned(), json!(code));
    }
    let out = if probe.up {
        HealthCheckOutput::up()
    } else {
        HealthCheckOutput::down()
    };
    out.with_details(details)
}

impl HealthIndicator for JiraHealth {
    fn check(&self) -> Pin<Box<dyn Future<Output = HealthCheckOutput> + Send + '_>> {
        Box::pin(self.run())
    }

    fn timeout_ms(&self) -> u64 {
        self.settings().timeout_ms
    }

    fn group(&self) -> IndicatorGroup {
        IndicatorGroup::HealthOnly
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use std::sync::Arc;

    use autumn_web::actuator::HealthStatus;

    use super::*;

    #[tokio::test]
    async fn down_before_start_and_after_a_failed_start() {
        let h = JiraHealth::new(Arc::default());
        let out = h.check().await;
        assert_eq!(out.status, HealthStatus::Down);
        assert_eq!(out.details["reason"], json!("not started"));
        assert_eq!(h.timeout_ms(), JiraHealthConfig::default().timeout_ms);
        assert!(format!("{h:?}").contains("started: false"), "{h:?}");
        h.mark_failed();
        let out = h.check().await;
        assert_eq!(out.details["reason"], json!("start failed"));
    }
}
