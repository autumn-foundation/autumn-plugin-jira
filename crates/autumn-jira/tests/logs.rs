//! No secret and no body in any log line (AC21).

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use autumn_jira::backend::SanMateoTarget;
use autumn_jira::request::{FieldSet, IssueTypeRef, SearchRequest};
use autumn_jira::webhook::parse_event;
use autumn_jira::{Adf, IdempotencyKey, JiraClient, Jql, NewIssue, ProjectKey};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
use common::{
    ACCESS_TOKEN, CLIENT_SECRET, ENV, REFRESH_TOKEN, api_token_client, fast_settings,
    oauth_connection, san_mateo, with_oauth,
};
use serde_json::Map;
use tracing_subscriber::fmt::MakeWriter;

/// The summary text: a body value that must not reach a log line.
const SUMMARY: &str = "Body-Secret-Summary-7731";
/// The comment text: a body value that must not reach a log line.
const COMMENT: &str = "Body-Secret-Comment-9902";

/// A writer that keeps every log line.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

async fn flow(fake: &FakeJira, client: &JiraClient) {
    let ops = ProjectKey::parse("OPS").unwrap();
    let issue = NewIssue::new(ops.clone(), IssueTypeRef::Name("Task".to_owned()), SUMMARY)
        .description(Adf::text(SUMMARY));
    // A retry, a lost reply with a lookup, and a plain create.
    fake.fault(FakeOp::Search, Fault::Status(503));
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let key = IdempotencyKey::derive(&["logs", "1"]);
    client.create_issue_idempotent(&issue, &key).await.unwrap();
    let created = client.create_issue(&issue).await.unwrap();
    client.get_issue(&created.key).await.unwrap();
    let request = SearchRequest::new(Jql::project(&ops)).fields(FieldSet::default());
    client.search_all(request, 10).await.unwrap();
    client
        .add_comment(&created.key, &Adf::text(COMMENT))
        .await
        .unwrap();
    fake.fault(FakeOp::AddComment, Fault::StatusAfterCommit(500));
    let _ = client.add_comment(&created.key, &Adf::text(COMMENT)).await;
    let _ = client.myself().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn no_secret_or_body_in_logs() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let fake = FakeJira::new().with_webhook_secret("whsec-77");
    flow(&fake, &api_token_client(&fake)).await;
    for delivery in fake.take_deliveries() {
        parse_event(&delivery.body).unwrap();
    }

    let oauth = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(oauth.clone()));
    let conn = oauth_connection(&san, Map::new()).await;
    oauth.expire_access_token();
    let client =
        JiraClient::san_mateo(san, SanMateoTarget::new(ENV, conn)).with_settings(fast_settings());
    flow(&oauth, &client).await;

    let text = capture.text();
    assert!(
        text.contains("jira attempt"),
        "the client logs each attempt"
    );
    assert!(text.contains("outcome of a write is unknown"), "{text}");
    let basic =
        data_encoding::BASE64.encode(format!("{DEFAULT_EMAIL}:{DEFAULT_API_TOKEN}").as_bytes());
    for needle in [
        DEFAULT_API_TOKEN,
        DEFAULT_EMAIL,
        basic.as_str(),
        "Basic ",
        "Bearer ",
        ACCESS_TOKEN,
        REFRESH_TOKEN,
        "acme-at-",
        "acme-rt-",
        CLIENT_SECRET,
        "whsec-77",
        SUMMARY,
        COMMENT,
        "authorization",
    ] {
        assert!(!text.contains(needle), "a log line holds {needle}:\n{text}");
    }
}
