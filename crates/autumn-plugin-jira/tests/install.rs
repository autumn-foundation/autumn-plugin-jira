//! AC24: install with one call, the declared route and the conformance
//! checks.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use autumn_jira_fake::FakeJira;
use autumn_plugin_jira::{
    DEFAULT_PREFIX, Delivery, EventSink, HandlerError, JiraPlugin, JiraService, PLUGIN_NAME,
};
use autumn_web::plugin::Plugin;
use autumn_web::route_listing::{RouteClassification, RouteInfo, RouteSource};
use autumn_web::test::TestApp;
use common::*;

/// Records the delivery ids that it gets.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl EventSink for Recorder {
    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        self.0.lock().unwrap().push(delivery.meta.id.to_string());
        Ok(())
    }
}

fn plugin_routes(p: JiraPlugin) -> Vec<RouteInfo> {
    autumn_web::app()
        .plugin(p)
        .plugin_route_infos()
        .unwrap()
        .into_iter()
        .filter(|r| r.source == RouteSource::Plugin(PLUGIN_NAME.to_owned()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_installs_with_one_call() {
    let fake = FakeJira::new();
    let client = TestApp::new()
        .config(autumn_config())
        .plugin(JiraPlugin::new().client(client_on(Arc::new(fake.clone()))))
        .build();
    let jira = JiraService::from_state(client.state()).expect("the service is in the state");
    let me = jira.myself().await.unwrap();
    assert_eq!(me.account_id.as_str(), autumn_jira_fake::BOT);
}

#[test]
fn name_contract_and_config_section() {
    let p = JiraPlugin::new();
    assert_eq!(p.name(), PLUGIN_NAME);
    assert_eq!(p.route_prefix(), DEFAULT_PREFIX);
    let contract = p.contract().unwrap();
    assert_eq!(contract.autumn_web.as_deref(), Some("0.9"));
    let app = autumn_web::app().plugin(JiraPlugin::default());
    assert!(app.has_plugin(PLUGIN_NAME));
    assert!(app.has_config_section("jira"));
}

#[test]
fn plugin_passes_the_autumn_conformance_checks() {
    use autumn_web::plugin_conformance::{ConformanceConfig, run_conformance};
    for p in [
        JiraPlugin::new().webhook_route(),
        JiraPlugin::new().sink(Recorder::default()),
    ] {
        let contract = p.contract().unwrap();
        let routes = autumn_web::app().plugin(p).plugin_route_infos().unwrap();
        let cfg = ConformanceConfig::new(PLUGIN_NAME)
            .prefix(DEFAULT_PREFIX)
            .contract(contract);
        let report = run_conformance(&cfg, &routes);
        assert!(report.passed(), "{}", report.to_text_report());
    }
}

#[test]
fn webhook_route_is_public_and_attributed() {
    let routes = plugin_routes(JiraPlugin::new().webhook_route());
    let [route] = routes.as_slice() else {
        panic!("one route: {routes:?}");
    };
    assert_eq!(route.method, "POST");
    assert_eq!(route.path, "/jira/webhook");
    assert_eq!(route.classification, RouteClassification::Public);
    assert_eq!(route.module.as_deref(), Some("autumn_plugin_jira::webhook"));

    // A sink mounts the route too, at the full path of the prefix.
    let routes = plugin_routes(
        JiraPlugin::new()
            .prefix("/hooks/jira")
            .sink(Recorder::default()),
    );
    assert_eq!(routes.len(), 1, "{routes:?}");
    assert_eq!(routes[0].path, "/hooks/jira/webhook");
    let routes = plugin_routes(JiraPlugin::new().on_event(|_d, _s| async { Ok(()) }));
    assert_eq!(routes.len(), 1, "{routes:?}");

    // No sink and no `webhook_route()`: no route.
    assert!(plugin_routes(JiraPlugin::new()).is_empty());
}

/// Spike S1 on the real plugin: a signed delivery reaches the sink. The
/// route is merged at the full path, so autumn finds the endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn signed_delivery_reaches_the_handler_not_500() {
    let fake = FakeJira::new();
    let recorder = Recorder::default();
    let client = app(plugin(&fake).sink(recorder.clone()));
    let body = issue_updated(Some(now_ms()), "acct-ann");
    deliver(&client, "s1-1", &body).await.assert_status(200);
    assert_eq!(*recorder.0.lock().unwrap(), ["s1-1"]);
    client
        .post(PATH)
        .header("X-Hub-Signature", "sha256=00")
        .header("X-Atlassian-Webhook-Identifier", "s1-2")
        .body(body)
        .send()
        .await
        .assert_status(401);
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_plugin_is_a_no_op() {
    let app = autumn_web::app()
        .plugin(JiraPlugin::new().webhook_route())
        .plugin(JiraPlugin::new().webhook_route());
    let ours: Vec<_> = app
        .plugin_route_infos()
        .unwrap()
        .into_iter()
        .filter(|r| r.source == RouteSource::Plugin(PLUGIN_NAME.to_owned()))
        .collect();
    assert_eq!(ours.len(), 1, "{ours:?}");

    // In a `TestApp`, the second plugin does not mount a second route or
    // start a second time.
    let fake = FakeJira::new();
    let recorder = Recorder::default();
    let client = TestApp::new()
        .config(autumn_config())
        .plugin(plugin(&fake).sink(recorder.clone()))
        .plugin(plugin(&fake).sink(recorder.clone()))
        .build();
    let body = issue_updated(Some(now_ms()), "acct-ann");
    deliver(&client, "dup-1", &body).await.assert_status(200);
    assert_eq!(recorder.0.lock().unwrap().len(), 1);
}
