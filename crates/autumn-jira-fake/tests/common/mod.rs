//! Helpers for the `FakeJira` tests.

#![allow(dead_code, reason = "each test binary uses a part of the helpers")]

use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use serde_json::{Value, json};

/// The site base URL of the default fake.
pub const SITE: &str = "https://acme.atlassian.net";
/// The gateway base URL of the default fake.
pub const GATEWAY: &str = "https://api.atlassian.com/ex/jira/11111111-2222-4333-8444-555555555555";

/// Returns a Basic auth header value.
pub fn basic_header(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// Adds the default Basic auth to a request.
pub fn with_auth(request: HttpRequest) -> HttpRequest {
    request.header(
        "authorization",
        basic_header(DEFAULT_EMAIL, DEFAULT_API_TOKEN),
    )
}

/// Returns a request with a Bearer token.
pub fn bearer(method: &str, url: &str, token: &str) -> HttpRequest {
    HttpRequest::new(method, url).header("authorization", format!("Bearer {token}"))
}

/// Sends one request with the default Basic auth through the sans-IO core.
pub fn call(fake: &FakeJira, method: &str, url: &str, body: Option<Vec<u8>>) -> HttpResponse {
    let mut request = with_auth(HttpRequest::new(method, url));
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body);
    }
    fake.handle(&request).unwrap()
}

/// Parses a reply body as JSON. A body that is not JSON gives `Null`.
pub fn json_of(reply: &HttpResponse) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or(Value::Null)
}

/// Calls a site path with JSON and returns the status and the JSON body.
pub fn api(fake: &FakeJira, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let reply = call(
        fake,
        method,
        &format!("{SITE}{path}"),
        body.map(|b| b.to_string().into_bytes()),
    );
    (reply.status, json_of(&reply))
}

/// Returns the request that creates a Task.
pub fn create_request(project: &str, summary: &str) -> HttpRequest {
    let body = json!({"fields": {
        "project": {"key": project},
        "issuetype": {"name": "Task"},
        "summary": summary,
    }});
    with_auth(HttpRequest::new("POST", format!("{SITE}/rest/api/3/issue")))
        .header("content-type", "application/json")
        .body(body.to_string().into_bytes())
}

/// Creates a Task and returns its key and id.
pub fn create(fake: &FakeJira, project: &str, summary: &str, labels: &[&str]) -> (String, u64) {
    let body = json!({"fields": {
        "project": {"key": project},
        "issuetype": {"name": "Task"},
        "summary": summary,
        "labels": labels,
    }});
    let (status, created) = api(fake, "POST", "/rest/api/3/issue", Some(body));
    assert_eq!(status, 201, "{created}");
    (
        created["key"].as_str().unwrap().to_owned(),
        created["id"].as_str().unwrap().parse().unwrap(),
    )
}

/// Posts a transition with optional fields and returns the status.
pub fn transition(fake: &FakeJira, key: &str, id: &str, fields: Option<Value>) -> u16 {
    let mut body = json!({"transition": {"id": id}});
    if let Some(fields) = fields {
        body["fields"] = fields;
    }
    transition_raw(fake, key, body).0
}

/// Posts a raw transition body.
pub fn transition_raw(fake: &FakeJira, key: &str, body: Value) -> (u16, Value) {
    api(
        fake,
        "POST",
        &format!("/rest/api/3/issue/{key}/transitions"),
        Some(body),
    )
}

/// Posts a search body.
pub fn search(fake: &FakeJira, body: Value) -> (u16, Value) {
    api(fake, "POST", "/rest/api/3/search/jql", Some(body))
}

/// Posts a search body and returns the headers too.
pub fn search_raw(fake: &FakeJira, body: Value) -> (u16, Value, Vec<(String, String)>) {
    let reply = call(
        fake,
        "POST",
        &format!("{SITE}/rest/api/3/search/jql"),
        Some(body.to_string().into_bytes()),
    );
    (reply.status, json_of(&reply), reply.headers)
}

/// Returns a header value. The name match ignores case.
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Sends a request through the async transport.
pub async fn send_with(
    fake: &FakeJira,
    request: HttpRequest,
) -> Result<HttpResponse, TransportError> {
    fake.send(request).await
}
