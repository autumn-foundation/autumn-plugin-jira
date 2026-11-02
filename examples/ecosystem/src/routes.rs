//! camel: the edges. Alert in, chat out, and one content enricher.
//!
//! ```text
//! direct:alerts ─► harvest:alerts?idHeader=AlertId          (alert in: one run per alert id)
//! direct:notify ─► jira:getIssue ─► san-mateo:team-chat     (chat out: POST /v1/messages)
//! ```
//!
//! The routes have no camel redelivery: harvest retries the activity that
//! calls a route.

use camel_api::{Body, CamelError, Exchange, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_core::RouteDefinition;
use harvest_camel::HEADER_IDEMPOTENCY_KEY;
use serde_json::json;

/// The inbound route. In production, use a broker consumer such as
/// `from("kafka:alerts")`.
pub const ALERTS: &str = "direct:alerts";
/// The header with the alert id. A broker sends the same id again on a
/// redelivery, so harvest uses it to find duplicates.
pub const ALERT_ID: &str = "AlertId";
/// The harvest stream that the connector reads.
pub const STREAM: &str = "alerts";
/// The outbound route that the `notify_chat` activity calls.
pub const NOTIFY: &str = "direct:notify";

/// The step that enriches the exchange with the issue. The `JiraIssueKey`
/// header gives the issue, so the step needs `allowHeaderOverrides=true`.
/// Only the activity can send to this route, so no outside caller can set
/// that header.
const ENRICH: &str = "jira:getIssue?allowHeaderOverrides=true";
/// The chat step. The URI fixes the connection: a header cannot change it.
const CHAT: &str = "san-mateo:team-chat?connectionId=team&method=POST&path=/v1/messages";

/// The JSON body of an exchange. `Value::Null` for other bodies.
fn json_body(ex: &Exchange) -> Value {
    match &ex.input.body {
        Body::Json(value) => value.clone(),
        _ => Value::Null,
    }
}

/// The two routes of the example.
///
/// # Errors
///
/// The error of a route build.
pub fn routes() -> Result<Vec<RouteDefinition>, CamelError> {
    let alerts = RouteBuilder::from(ALERTS)
        .route_id("alerts-in")
        .to(format!("harvest:{STREAM}?idHeader={ALERT_ID}"))
        .build()?;
    let notify = RouteBuilder::from(NOTIFY)
        .route_id("notify-chat")
        // The enricher reads the issue key from the header. A GET sends no
        // body.
        .process(|mut ex: Exchange| async move {
            let key = json_body(&ex)["issue"].clone();
            if !key.is_string() {
                return Err(CamelError::ValidationError(
                    "the notify body has no issue key".to_owned(),
                ));
            }
            ex.input
                .set_header(autumn_jira_camel::HEADER_ISSUE_KEY, key);
            ex.input.body = Body::Empty;
            Ok(ex)
        })
        .to(ENRICH)
        // harvest-camel can send this route again after a 5xx or a network
        // error, also for a POST. So the chat API gets the harvest key (the
        // same for each attempt) as `Idempotency-Key`. With no key, the
        // route sends no POST.
        .process(|mut ex: Exchange| async move {
            let Some(key) = ex.input.header(HEADER_IDEMPOTENCY_KEY).cloned() else {
                return Err(CamelError::ValidationError(
                    "no HarvestIdempotencyKey: call this route from a harvest activity".to_owned(),
                ));
            };
            ex.input.set_header("SanMateoProxy-Idempotency-Key", key);
            let issue = json_body(&ex);
            let text = format!(
                "{} is {}: {}",
                issue["key"].as_str().unwrap_or_default(),
                issue["fields"]["status"]["name"]
                    .as_str()
                    .unwrap_or_default(),
                issue["fields"]["summary"].as_str().unwrap_or_default(),
            );
            ex.input.body = Body::Json(json!({ "text": text }));
            Ok(ex)
        })
        .to(CHAT)
        // The reply of the activity: the chat status and the message.
        .process(|mut ex: Exchange| async move {
            let status = ex
                .input
                .header(san_mateo_camel::HEADER_RESPONSE_CODE)
                .cloned()
                .unwrap_or(Value::Null);
            let message = json_body(&ex);
            ex.input.body = Body::Json(json!({ "status": status, "message": message }));
            Ok(ex)
        })
        .build()?;
    Ok(vec![alerts, notify])
}
