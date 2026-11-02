//! Typed errors over `MemoryTransport` (AC13).

mod common;

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::error::{Op, SCOPE_HINT};
use autumn_jira::san_mateo::transport::HttpResponse;
use autumn_jira::san_mateo::MemoryTransport;
use autumn_jira::{IssueKey, JiraClient, JiraError};
use autumn_jira_fake::DEFAULT_API_TOKEN;
use common::{auth, fast_settings, site};
use serde_json::json;

const URL: &str = "https://acme.atlassian.net/rest/api/3/issue/OPS-1";

/// A client with one attempt for each call, so each status gives its error.
fn one_shot(transport: &MemoryTransport) -> JiraClient {
    let mut settings = fast_settings();
    settings.max_attempts = 1;
    JiraClient::api_token(site(), auth(), Arc::new(transport.clone())).with_settings(settings)
}

async fn get_with(reply: HttpResponse) -> JiraError {
    let transport = MemoryTransport::new();
    transport.reply("GET", URL, reply);
    let err = one_shot(&transport)
        .get_issue(&IssueKey::parse("OPS-1").unwrap())
        .await
        .unwrap_err();
    assert_eq!(transport.requests().len(), 1);
    err
}

#[tokio::test]
async fn status_table_over_memory_transport() {
    let op = Op::GetIssue;
    let errors = json!({
        "errorMessages": ["Issue has a problem"],
        "errors": {"summary": "You must specify a summary."}
    });
    let validation = get_with(HttpResponse::json(400, &errors)).await;
    assert_eq!(
        validation,
        JiraError::Validation {
            op,
            status: 400,
            messages: vec![
                "Issue has a problem".to_owned(),
                "summary: You must specify a summary.".to_owned()
            ],
            fields: vec!["summary".to_owned()],
        }
    );
    assert_eq!(validation.messages().len(), 2);
    let unprocessable = get_with(HttpResponse::json(422, &errors)).await;
    assert!(matches!(unprocessable, JiraError::Validation { status: 422, .. }));
    // A body that is not JSON gives a validation error with no messages.
    let bare = get_with(HttpResponse::new(400, "<html>bad</html>")).await;
    assert!(matches!(&bare, JiraError::Validation { messages, .. } if messages.is_empty()));

    for status in [401, 403] {
        let err = get_with(HttpResponse::new(status, "no")).await;
        assert_eq!(err, JiraError::Unauthorized { op, status, hint: SCOPE_HINT });
    }
    let html = get_with(HttpResponse::new(404, "<html>Not found</html>")).await;
    assert_eq!(html, JiraError::NotFoundOrForbidden { op });
    assert_eq!(get_with(HttpResponse::new(409, "")).await, JiraError::Conflict { op });
    assert_eq!(get_with(HttpResponse::new(410, "")).await, JiraError::Gone { op });
    assert_eq!(
        get_with(HttpResponse::new(413, "")).await,
        JiraError::PayloadTooLarge { op }
    );
    let limited = get_with(
        HttpResponse::new(429, "")
            .header("Retry-After", "7")
            .header("RateLimit-Reason", "jira-burst-based"),
    )
    .await;
    assert_eq!(
        limited,
        JiraError::RateLimited {
            op,
            retry_after: Some(Duration::from_secs(7)),
            reason: Some("jira-burst-based".to_owned()),
        }
    );
    assert_eq!(limited.retry_after(), Some(Duration::from_secs(7)));
    // A date in Retry-After is not read.
    let dated = get_with(
        HttpResponse::new(429, "").header("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT"),
    )
    .await;
    assert_eq!(dated.retry_after(), None);
    for status in [500, 502, 503, 504, 408, 425] {
        let err = get_with(HttpResponse::new(status, "")).await;
        assert_eq!(err, JiraError::Upstream { op, status });
        assert!(err.is_retryable());
    }
    assert_eq!(
        get_with(HttpResponse::new(302, "")).await,
        JiraError::Redirect { op, status: 302 }
    );
    assert_eq!(
        get_with(HttpResponse::new(418, "")).await,
        JiraError::UnexpectedStatus { op, status: 418 }
    );
    let decode = get_with(HttpResponse::new(200, "{not json")).await;
    assert!(matches!(&decode, JiraError::Decode { op: Op::GetIssue, reason } if reason.contains("line 1")));
    let wrong_shape = get_with(HttpResponse::json(200, &json!({"key": "OPS-1"}))).await;
    assert!(matches!(wrong_shape, JiraError::Decode { .. }));
}

#[tokio::test]
async fn display_has_no_secret_or_query() {
    let secret_text = "token=sk-live-123 password hunter2";
    let errors = json!({"errorMessages": [secret_text], "errors": {"customfield_1": secret_text}});
    let mut seen = Vec::new();
    seen.push(get_with(HttpResponse::json(400, &errors)).await);
    seen.push(get_with(HttpResponse::new(500, secret_text)).await);
    seen.push(get_with(HttpResponse::new(200, secret_text)).await);
    seen.push(get_with(HttpResponse::new(401, secret_text)).await);
    seen.push(
        get_with(HttpResponse::new(429, secret_text).header("RateLimit-Reason", secret_text)).await,
    );
    for err in &seen {
        let display = err.to_string();
        for needle in ["sk-live", "hunter2", DEFAULT_API_TOKEN, "fields=", "?", "Basic "] {
            assert!(!display.contains(needle), "{needle} in {display}");
        }
        let debug = format!("{err:?}");
        for needle in [DEFAULT_API_TOKEN, "fields=", "Basic "] {
            assert!(!debug.contains(needle), "{needle} in {debug}");
        }
    }
    // The messages stay reachable for the app, outside Display.
    assert_eq!(seen[0].messages()[0], secret_text);
}
