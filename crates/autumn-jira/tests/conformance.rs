//! One suite, three setups (AC5): API token, san-mateo `jira-basic`, and
//! san-mateo `atlassian` OAuth with a 401 refresh on the first call.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::{Arc, Mutex};

use autumn_jira::JiraClient;
use autumn_jira::backend::SanMateoTarget;
use autumn_jira::client::ClientSettings;
use autumn_jira_fake::FakeJira;
use common::suite::run_client_suite;
use common::{ENV, api_token_client, basic_connection, fast_settings, oauth_connection, san_mateo};
use serde_json::Map;

fn settings() -> ClientSettings {
    fast_settings()
}

#[tokio::test(start_paused = true)]
async fn suite_on_api_token() {
    run_client_suite(|fake: FakeJira| async move { api_token_client(&fake) }).await;
}

#[tokio::test(start_paused = true)]
async fn suite_on_san_mateo_basic() {
    run_client_suite(|fake: FakeJira| async move {
        let san = san_mateo(Arc::new(fake));
        let key = basic_connection(&san).await;
        JiraClient::san_mateo(san, SanMateoTarget::new(ENV, key)).with_settings(settings())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn suite_on_san_mateo_oauth() {
    let fakes = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&fakes);
    run_client_suite(move |fake: FakeJira| {
        let fake = common::with_oauth(fake);
        seen.lock().unwrap().push(fake.clone());
        async move {
            let san = san_mateo(Arc::new(fake.clone()));
            // No cloud id: the first call discovers it.
            let key = oauth_connection(&san, Map::new()).await;
            // The first token is dead: the first call refreshes once.
            fake.expire_access_token();
            JiraClient::san_mateo(san, SanMateoTarget::new(ENV, key)).with_settings(settings())
        }
    })
    .await;
    let fakes = fakes.lock().unwrap();
    assert!(fakes.len() >= 20, "{}", fakes.len());
    for fake in fakes.iter() {
        assert_eq!(fake.token_refreshes(), 1);
        assert!(
            fake.requests()
                .iter()
                .any(|r| r.host == "api.atlassian.com")
        );
    }
}
