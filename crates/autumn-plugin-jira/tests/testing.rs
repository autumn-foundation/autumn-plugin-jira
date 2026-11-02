//! AC31: the `testing` helpers test an app handler in a few lines.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::sync::{Arc, Mutex};

use autumn_jira_fake::FakeJira;
use autumn_plugin_jira::testing::{DeliverExt, TEST_WEBHOOK_SECRET, app_config, plugin_for};
use autumn_plugin_jira::{JiraEvent, JiraService};
use autumn_web::test::TestApp;

#[tokio::test(flavor = "multi_thread")]
async fn app_handler_test_in_ten_lines() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let record = Arc::clone(&seen);
    // The ten lines: the fake, the app, one person action, the deliveries.
    let fake = FakeJira::new();
    let client = TestApp::new()
        .config(app_config())
        .plugin(plugin_for(&fake).on_event(move |d, _state| {
            let record = Arc::clone(&record);
            async move {
                record.lock().unwrap().push(d.event.name().to_owned());
                Ok(())
            }
        }))
        .build();
    let jira = JiraService::require(client.state()).unwrap();
    let issue = jira
        .create_issue(&autumn_plugin_jira::jira::NewIssue::new(
            autumn_plugin_jira::jira::ProjectKey::parse("OPS").unwrap(),
            autumn_plugin_jira::jira::request::IssueTypeRef::Name("Task".to_owned()),
            "Disk full",
        ))
        .await
        .unwrap();
    fake.human_comment(issue.key.as_str(), "On it", "ann")
        .unwrap();
    for delivery in fake.take_deliveries() {
        client.deliver(&delivery).await.assert_status(200);
    }
    assert_eq!(
        *seen.lock().unwrap(),
        ["jira:issue_created", "comment_created"]
    );
    // A tampered copy fails the autumn signature check.
    fake.human_comment(issue.key.as_str(), "Again", "ann")
        .unwrap();
    let delivery = fake.take_deliveries().remove(0);
    client
        .deliver(&delivery.tampered())
        .await
        .assert_status(401);
    assert_eq!(TEST_WEBHOOK_SECRET.len(), 36);
    assert!(app_config().security.csrf.enabled);
    assert!(matches!(
        JiraEvent::Other {
            name: "x".to_owned()
        },
        JiraEvent::Other { .. }
    ));
}
