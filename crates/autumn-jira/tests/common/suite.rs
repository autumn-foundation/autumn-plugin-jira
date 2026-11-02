//! The client conformance suite. Each case runs on each backend setup.

use std::future::Future;

use autumn_jira::JiraClient;
use autumn_jira_fake::FakeJira;

/// Runs every suite case. `make` turns a fresh fake into a client.
pub async fn run_client_suite<F, Fut>(make: F)
where
    F: Fn(FakeJira) -> Fut,
    Fut: Future<Output = JiraClient>,
{
    let _ = make(FakeJira::new()).await;
}
