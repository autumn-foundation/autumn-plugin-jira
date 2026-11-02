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

/// The module name in the route listing.
pub(crate) const MODULE: &str = "autumn_plugin_jira::webhook";

/// The router to merge: one `POST` route at the full webhook path.
pub(crate) fn webhook_router(webhook_path: &str) -> Router<AppState> {
    Router::new().route(webhook_path, post(log_only))
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

/// The handler when no sink is set. autumn has checked the signature and
/// the delivery id before this runs. The handler logs the delivery id,
/// never the body.
async fn log_only(hook: SignedWebhook) -> StatusCode {
    tracing::debug!(
        delivery_id = hook.delivery_id().unwrap_or("none"),
        bytes = hook.raw_body().len(),
        "jira webhook received; no sink is set"
    );
    StatusCode::OK
}
