//! The webhook route: the pipeline after the autumn signature check.
//!
//! autumn finds the `[[security.webhooks.endpoints]]` entry by the request
//! path that `SignedWebhook` sees. `nest` strips the prefix from that path,
//! so the lookup fails with 500. So the plugin uses `merge` at the full
//! path. The `spike_mount` tests show both cases.
//!
//! The pipeline (plan 8.4):
//!
//! 1. autumn checks the HMAC in constant time over the raw body, and the
//!    delivery id in its replay store. It answers 400, 401 or 409 itself.
//! 2. The route reads the retry and flow headers and parses the body.
//! 3. It ignores a delivery with no timestamp, a stale or a future one, one
//!    from an ignored actor, and an event that it does not dispatch. Each
//!    gets 200 and a counter.
//! 4. It runs each handler in order, each in its own task, under
//!    `handler_timeout_ms`. A transient error or a timeout gives 503, and
//!    the later handlers do not run. autumn then frees the replay keys, so
//!    Jira can send the delivery again.
//!
//! After step 1 the route answers 200 or 503 only. Webhooks are at least
//! once: a handler can get the same delivery again, also when another
//! handler fails.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::policy::{self, Freshness, Verdict};
use autumn_jira::webhook::{DeliveryMeta, FLOW_HEADER, Flow, RETRY_HEADER, parse_event};
use autumn_jira::{Delivery, HandlerError, JiraEvent};
use autumn_web::AppState;
use autumn_web::reexports::axum::Router;
use autumn_web::reexports::axum::extract::State;
use autumn_web::reexports::axum::routing::post;
use autumn_web::reexports::http::{HeaderMap, StatusCode};
use autumn_web::route_listing::{RouteClassification, RouteInfo};
use autumn_web::webhook::SignedWebhook;

use crate::runtime::{Cell, Counters, Runtime};

/// The module name in the route listing.
pub(crate) const MODULE: &str = "autumn_plugin_jira::webhook";

/// The router to merge: one `POST` route at the full webhook path. The
/// route answers 503 until the plugin starts.
pub(crate) fn router(webhook_path: &str, cell: Cell) -> Router<AppState> {
    Router::new().route(
        webhook_path,
        post(
            move |State(state): State<AppState>, headers: HeaderMap, hook: SignedWebhook| {
                let runtime = cell.get().cloned();
                async move {
                    let verdict = match runtime {
                        Some(runtime) => runtime.process(&state, &headers, &hook).await,
                        // Not started: Jira sends the delivery again.
                        None => Verdict::TimedOut,
                    };
                    StatusCode::from_u16(policy::webhook_status(verdict))
                        .unwrap_or(StatusCode::SERVICE_UNAVAILABLE)
                }
            },
        ),
    )
}

/// The route for `autumn routes` and the routes audit.
///
/// The route is `Public`: autumn checks the HMAC signature, not a session.
pub(crate) fn route_info(webhook_path: &str) -> RouteInfo {
    RouteInfo {
        method: "POST".to_owned(),
        path: webhook_path.to_owned(),
        handler: format!("{MODULE} POST {webhook_path}"),
        classification: RouteClassification::Public,
        module: Some(MODULE.to_owned()),
        ..RouteInfo::default()
    }
}

/// A header value as text.
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The ms since the epoch, or 0 before it.
fn epoch_ms(time: std::time::SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Runtime {
    /// Runs the pipeline on one verified delivery. It logs ids and names,
    /// never the body.
    pub(crate) async fn process(
        &self,
        state: &AppState,
        headers: &HeaderMap,
        hook: &SignedWebhook,
    ) -> Verdict {
        let c = &self.counters;
        let ignore = |counter, reason: &str| {
            Counters::add(counter);
            tracing::debug!(
                delivery_id = hook.delivery_id().unwrap_or("none"),
                reason,
                "jira webhook ignored"
            );
            Verdict::Ignored
        };
        let Some(id) = hook
            .delivery_id()
            .and_then(|id| WebhookDeliveryId::parse(id).ok())
        else {
            return ignore(&c.undecodable, "no delivery id");
        };
        let retry = header(headers, RETRY_HEADER)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let flow = header(headers, FLOW_HEADER).map_or(Flow::Unknown, Flow::parse);
        let Ok(parsed) = parse_event(hook.raw_body()) else {
            return ignore(&c.undecodable, "the body does not parse");
        };
        let Some(timestamp) = parsed.timestamp else {
            return ignore(&c.ignored_no_timestamp, "no timestamp");
        };
        let event_ms = u64::try_from(timestamp.epoch_ms()).unwrap_or(0);
        let settings = &self.webhook;
        match policy::freshness(
            event_ms,
            epoch_ms(hook.received_at()),
            settings.max_age_secs.saturating_mul(1000),
            settings.max_future_skew_secs.saturating_mul(1000),
        ) {
            Freshness::Stale => return ignore(&c.ignored_stale, "stale"),
            Freshness::Future => return ignore(&c.ignored_future, "future"),
            Freshness::Fresh => {}
        }
        if parsed.actor.as_ref().is_some_and(|actor| {
            settings
                .ignore_actor_account_ids
                .iter()
                .any(|ignored| ignored == actor.as_str())
        }) {
            return ignore(&c.ignored_self, "ignored actor");
        }
        if matches!(parsed.event, JiraEvent::Other { .. }) {
            return ignore(
                &c.ignored_unknown,
                "an event that the plugin does not dispatch",
            );
        }
        let delivery = Arc::new(Delivery::new(DeliveryMeta::new(id, retry, flow), parsed));
        let verdict = self.dispatch(state, &delivery).await;
        let counter = match verdict {
            Verdict::Accepted | Verdict::Ignored => &c.accepted,
            Verdict::HandlerFailed => &c.handler_failed,
            Verdict::TimedOut => &c.timed_out,
        };
        Counters::add(counter);
        verdict
    }

    /// Runs each handler in order, each in a spawned task, and waits for
    /// each under `handler_timeout_ms`. A task that times out runs to its
    /// end.
    async fn dispatch(&self, state: &AppState, delivery: &Arc<Delivery>) -> Verdict {
        let limit = Duration::from_millis(self.webhook.handler_timeout_ms);
        let id = delivery.meta.id.as_str();
        let event = delivery.event.name();
        for handler in &self.handlers {
            let name = handler.name();
            let task = tokio::spawn(handler.run(Arc::clone(delivery), state.clone()));
            match tokio::time::timeout(limit, task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(HandlerError::Permanent(reason)))) => {
                    Counters::add(&self.counters.permanent);
                    tracing::error!(
                        delivery_id = id,
                        event,
                        handler = name,
                        reason,
                        "jira webhook handler failed; Jira does not send it again"
                    );
                }
                Ok(Ok(Err(HandlerError::Transient(reason)))) => {
                    tracing::warn!(
                        delivery_id = id,
                        event,
                        handler = name,
                        reason,
                        "jira webhook handler failed; Jira sends it again"
                    );
                    return Verdict::HandlerFailed;
                }
                Ok(Err(join)) => {
                    tracing::error!(
                        delivery_id = id,
                        event,
                        handler = name,
                        panicked = join.is_panic(),
                        "jira webhook handler stopped; Jira sends it again"
                    );
                    return Verdict::HandlerFailed;
                }
                Err(_) => {
                    tracing::warn!(
                        delivery_id = id,
                        event,
                        handler = name,
                        timeout_ms = self.webhook.handler_timeout_ms,
                        "jira webhook handler did not finish in time; it runs to its end, and Jira sends the delivery again"
                    );
                    return Verdict::TimedOut;
                }
            }
        }
        tracing::debug!(delivery_id = id, event, "jira webhook accepted");
        Verdict::Accepted
    }
}
