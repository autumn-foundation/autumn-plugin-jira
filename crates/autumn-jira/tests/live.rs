//! Live cases against a real Jira Cloud site (plan 13, item 8).
//!
//! Each case is `#[ignore]`. CI never runs them. Run them by hand:
//!
//! ```text
//! JIRA_LIVE_SITE=acme JIRA_LIVE_EMAIL=bot@acme.example \
//! JIRA_LIVE_TOKEN=... JIRA_LIVE_PROJECT=SANDBOX \
//! cargo test -p autumn-jira --test live -- --ignored --test-threads=1
//! ```
//!
//! `JIRA_LIVE_ISSUE_TYPE` sets the issue type (default `Task`). Each case
//! deletes the issues that it makes. No case logs a value of the env.
//! These cases check the `[U]` assumptions of plan section 15: A2, A3, A7.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use autumn_jira::client::{CommentOutcome, CreateOutcome, DeleteOutcome};
use autumn_jira::ids::SiteName;
use autumn_jira::request::{EditOptions, IssueEdit, IssueTypeRef, SearchRequest};
use autumn_jira::san_mateo::transport::TransportConfig;
use autumn_jira::san_mateo::{ReqwestTransport, Secret};
use autumn_jira::{
    Adf, BasicAuth, IdempotencyKey, IssueKey, JiraClient, JiraSite, Jql, NewIssue, ProjectKey,
};

/// The live setup from the env, or a panic that names the missing variable.
struct Live {
    client: JiraClient,
    project: ProjectKey,
    issue_type: String,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name} to run the live cases"))
}

fn live() -> Live {
    let site = JiraSite::Subdomain(SiteName::parse(&env("JIRA_LIVE_SITE")).unwrap());
    let auth = BasicAuth::new(env("JIRA_LIVE_EMAIL"), Secret::new(env("JIRA_LIVE_TOKEN"))).unwrap();
    let transport = ReqwestTransport::new(TransportConfig::default()).unwrap();
    Live {
        client: JiraClient::api_token(site, auth, Arc::new(transport)),
        project: ProjectKey::parse(&env("JIRA_LIVE_PROJECT")).unwrap(),
        issue_type: std::env::var("JIRA_LIVE_ISSUE_TYPE").unwrap_or_else(|_| "Task".to_owned()),
    }
}

impl Live {
    fn issue(&self, summary: &str) -> NewIssue {
        NewIssue::new(
            self.project.clone(),
            IssueTypeRef::Name(self.issue_type.clone()),
            format!("autumn-jira live: {summary}"),
        )
    }

    async fn delete(&self, key: &IssueKey) {
        let outcome = self.client.delete_issue(key, true).await.unwrap();
        assert_eq!(outcome, DeleteOutcome::Deleted);
    }

    /// Polls until `f` gives `Some`, at most 60 s.
    async fn eventually<T, F: AsyncFnMut() -> Option<T>>(mut f: F) -> T {
        for _ in 0..30 {
            if let Some(value) = f().await {
                return value;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        panic!("no result after 60 s");
    }
}

#[tokio::test]
#[ignore = "needs a real Jira site"]
async fn live_account_project_and_issue_types() {
    let live = live();
    let me = live.client.myself().await.unwrap();
    assert_ne!(me.account_id.as_str().len(), 0);
    live.client.server_info().await.unwrap();
    let project = live.client.get_project(&live.project).await.unwrap();
    assert_eq!(project.key, live.project);
    let types = live.client.issue_types(&live.project).await.unwrap();
    assert!(types.iter().any(|t| t.name == live.issue_type));
    assert_ne!(live.client.all_projects().await.unwrap().len(), 0);
}

#[tokio::test]
#[ignore = "needs a real Jira site"]
async fn live_create_get_edit_search_delete() {
    let live = live();
    let created = live
        .client
        .create_issue(&live.issue("crud").description(Adf::text("line 1\nline 2")))
        .await
        .unwrap();
    let issue = live.client.get_issue(&created.key).await.unwrap();
    assert_eq!(issue.id, created.id);
    let edit = IssueEdit::new().add_label("autumn-jira-live");
    live.client
        .edit_issue(&created.key, &edit, EditOptions::notify_users(false))
        .await
        .unwrap();
    let request = SearchRequest::new(Jql::project(&live.project)).max_results(5);
    let mut pager = live.client.search(request);
    assert!(pager.next_page().await.unwrap().is_some());
    live.client
        .approximate_count(&Jql::project(&live.project))
        .await
        .unwrap();
    live.delete(&created.key).await;
}

/// A2: JQL `labels = "<marker>"` finds the exact marker label.
#[tokio::test]
#[ignore = "needs a real Jira site"]
async fn marker_label_is_searchable() {
    let live = live();
    let key = IdempotencyKey::derive(&["live", &format!("{:?}", std::time::SystemTime::now())]);
    let outcome = live
        .client
        .create_issue_idempotent(&live.issue("marker"), &key)
        .await
        .unwrap();
    let CreateOutcome::Created(created) = outcome else {
        panic!("a fresh key found an issue");
    };
    let found = Live::eventually(async || {
        live.client
            .find_by_idempotency_key(&live.project, &key)
            .await
            .unwrap()
    })
    .await;
    assert_eq!(found.key, created.key);
    live.delete(&created.key).await;
}

/// A3: the comment list sees a new comment at once, newest first.
#[tokio::test]
#[ignore = "needs a real Jira site"]
async fn comment_lookup_sees_a_new_comment() {
    let live = live();
    let created = live
        .client
        .create_issue(&live.issue("comment"))
        .await
        .unwrap();
    let key = IdempotencyKey::derive(&["live-comment", created.id.as_str()]);
    let first = live
        .client
        .add_comment_idempotent(&created.key, &Adf::text("first"), &key)
        .await
        .unwrap();
    assert!(matches!(first, CommentOutcome::Created(_)));
    let found = live
        .client
        .find_comment_by_idempotency_key(&created.key, &key)
        .await
        .unwrap();
    assert!(found.is_some());
    live.delete(&created.key).await;
}

/// A7: a property set in the create body is readable at once.
#[tokio::test]
#[ignore = "needs a real Jira site"]
async fn create_body_property_is_readable() {
    let live = live();
    let value = serde_json::json!({"probe": 1});
    let created = live
        .client
        .create_issue(
            &live
                .issue("property")
                .property("autumn-jira-live", value.clone()),
        )
        .await
        .unwrap();
    let read = live
        .client
        .get_issue_property(&created.key, "autumn-jira-live")
        .await
        .unwrap();
    assert_eq!(read, Some(value));
    live.delete(&created.key).await;
}
