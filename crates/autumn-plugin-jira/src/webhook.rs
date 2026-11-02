//! The webhook route and its declaration.
//!
//! autumn finds the `[[security.webhooks.endpoints]]` entry by the request
//! path that `SignedWebhook` sees. `nest` strips the prefix from that path,
//! so the lookup fails with 500. So the plugin uses `merge` at the full
//! path. The `spike_mount` tests show both cases.

use autumn_web::AppState;
use autumn_web::reexports::axum::Router;
use autumn_web::reexports::axum::routing::post;
use autumn_web::reexports::http::StatusCode;
use autumn_web::route_listing::{RouteClassification, RouteInfo};
use autumn_web::webhook::SignedWebhook;

use crate::runtime::Cell;

/// The module name in the route listing.
pub(crate) const MODULE: &str = "autumn_plugin_jira::webhook";

/// The router to merge: one `POST` route at the full webhook path. The
/// route answers 503 until the plugin starts.
pub(crate) fn router(webhook_path: &str, cell: Cell) -> Router<AppState> {
    Router::new().route(
        webhook_path,
        post(move |hook: SignedWebhook| {
            let started = cell.get().is_some();
            async move {
                if !started {
                    return StatusCode::SERVICE_UNAVAILABLE;
                }
                tracing::debug!(
                    delivery_id = hook.delivery_id().unwrap_or("none"),
                    bytes = hook.raw_body().len(),
                    "jira webhook received"
                );
                StatusCode::OK
            }
        }),
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
