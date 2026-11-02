//! AC25 (builder conflicts) and AC26: the service and the lifecycle.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;

use autumn_jira::ids::ProjectKey;
use autumn_jira::request::IssueTypeRef;
use autumn_jira_fake::{FakeJira, FakeOp, Fault};
use autumn_plugin_jira::{
    IssueKey, JiraClient, JiraError, JiraPlugin, JiraResultExt, JiraService, PluginError,
    autumn_error, jira,
};
use autumn_plugin_san_mateo::{SanMateoConfig, SanMateoPlugin, SanMateoService};
use autumn_web::reexports::axum::Router;
use autumn_web::reexports::axum::routing::get;
use autumn_web::reexports::http::StatusCode;
use autumn_web::test::TestApp;
use autumn_web::{AppState, AutumnResult, Json, Path, ProcessRole};
use common::*;
use serde_json::{Value, json};

fn start_err(p: JiraPlugin) -> PluginError {
    p.start(&AppState::for_test()).unwrap_err()
}

#[tokio::test(flavor = "multi_thread")]
async fn client_and_mode_table_conflict() {
    let fake = FakeJira::new();
    let p = JiraPlugin::with_config(api_token_config("E", "T")).client(client_on(Arc::new(fake)));
    let e = start_err(p);
    assert!(
        matches!(e, PluginError::Conflict("client(..)", "[jira.api_token]")),
        "{e:?}"
    );
    assert_eq!(
        e.to_string(),
        "client(..) and [jira.api_token] cannot be used together"
    );
    let p = JiraPlugin::with_config(san_mateo_config(None))
        .client(client_on(Arc::new(FakeJira::new())));
    assert!(matches!(
        start_err(p),
        PluginError::Conflict("client(..)", "[jira.san_mateo]")
    ));
    let p = plugin(&FakeJira::new()).transport(Arc::new(FakeJira::new()));
    assert!(matches!(
        start_err(p),
        PluginError::Conflict("transport(..)", "client(..)")
    ));
    // The app start fails loudly with the same text.
    let text = boot(|| {
        app(JiraPlugin::with_config(api_token_config("E", "T"))
            .client(client_on(Arc::new(FakeJira::new()))))
    })
    .unwrap_err();
    assert!(text.contains("cannot be used together"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_in_san_mateo_mode_conflicts() {
    let state = AppState::for_test();
    state.insert_extension(SanMateoService::new(san_mateo(Arc::new(FakeJira::new()))));
    let e = JiraPlugin::with_config(san_mateo_config(Some(CONNECTION)))
        .transport(Arc::new(FakeJira::new()))
        .start(&state)
        .unwrap_err();
    assert!(
        matches!(
            e,
            PluginError::Conflict("transport(..)", "[jira.san_mateo]")
        ),
        "{e:?}"
    );
}

/// A route that needs the service.
async fn who(jira: JiraService) -> AutumnResult<String> {
    Ok(jira.account_id().await.http()?.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn service_is_503_before_start() {
    let state = AppState::for_test();
    assert!(JiraService::from_state(&state).is_none());
    let e = JiraService::require(&state).unwrap_err();
    assert_eq!(e.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        e.message().contains("Jira has not started"),
        "{}",
        e.message()
    );

    // The extractor in a route of an app with no plugin.
    let client = TestApp::new()
        .merge(Router::new().route("/who", get(who)))
        .build();
    let r = client
        .get("/who")
        .header("accept", "application/json")
        .send()
        .await;
    r.assert_status(503);
}

#[cfg(feature = "testing")]
#[tokio::test(flavor = "multi_thread")]
async fn startup_makes_no_network_call() {
    let fake = FakeJira::new();
    let counting = Counting::new(Arc::new(fake.clone()));
    let p = JiraPlugin::with_config(api_token_config("PLUGIN_T_EMAIL", "PLUGIN_T_TOKEN"))
        .transport(Arc::new(counting.clone()))
        .secret_env("PLUGIN_T_EMAIL", autumn_jira_fake::DEFAULT_EMAIL)
        .secret_env("PLUGIN_T_TOKEN", autumn_jira_fake::DEFAULT_API_TOKEN)
        .webhook_route();
    let client = app(p);
    assert_eq!(counting.count(), 0, "the start makes no call");
    let jira = JiraService::require(client.state()).unwrap();
    assert_eq!(
        jira.myself().await.unwrap().account_id.as_str(),
        autumn_jira_fake::BOT
    );
    assert_eq!(counting.count(), 1);
    assert_eq!(fake.count(FakeOp::Myself), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_secret_names_the_variable() {
    let e = start_err(JiraPlugin::with_config(api_token_config(
        "AUTUMN_PLUGIN_JIRA_TEST_NO_EMAIL",
        "AUTUMN_PLUGIN_JIRA_TEST_NO_TOKEN",
    )));
    assert!(
        matches!(&e, PluginError::MissingSecret { env } if env == "AUTUMN_PLUGIN_JIRA_TEST_NO_EMAIL"),
        "{e:?}"
    );
    assert_eq!(
        e.to_string(),
        "set the environment variable AUTUMN_PLUGIN_JIRA_TEST_NO_EMAIL"
    );
    // No mode table and no client.
    let e = start_err(JiraPlugin::with_config(
        autumn_plugin_jira::JiraConfig::default(),
    ));
    assert_eq!(
        e.to_string(),
        "jira config: set [jira.api_token] or [jira.san_mateo]"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn san_mateo_missing_has_a_fix() {
    let e = start_err(JiraPlugin::with_config(san_mateo_config(Some(CONNECTION))));
    assert!(matches!(e, PluginError::SanMateoMissing), "{e:?}");
    let text =
        boot(|| app(JiraPlugin::with_config(san_mateo_config(Some(CONNECTION))))).unwrap_err();
    assert!(
        text.contains("san_mateo mode needs SanMateoPlugin; register it before JiraPlugin"),
        "{text}"
    );
}

fn smp(fake: &FakeJira) -> SanMateoPlugin {
    SanMateoPlugin::with_config(SanMateoConfig::default())
        .service(san_mateo(Arc::new(fake.clone())))
        .background(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn san_mateo_after_jira_has_a_fix() {
    let fake = FakeJira::new();
    let text = boot(|| {
        TestApp::new()
            .config(autumn_config())
            .plugin(JiraPlugin::with_config(san_mateo_config(Some(CONNECTION))))
            .plugin(smp(&fake))
            .build()
    })
    .unwrap_err();
    assert!(
        text.contains("SanMateoPlugin is registered after JiraPlugin; register it before"),
        "{text}"
    );

    // The right order starts, and the client sends through San Mateo.
    let client = TestApp::new()
        .config(autumn_config())
        .plugin(smp(&fake))
        .plugin(JiraPlugin::with_config(san_mateo_config(Some(CONNECTION))))
        .build();
    let san = SanMateoService::require(client.state()).unwrap();
    basic_connection(&san, CONNECTION).await;
    let jira = JiraService::require(client.state()).unwrap();
    assert_eq!(
        jira.myself().await.unwrap().account_id.as_str(),
        autumn_jira_fake::BOT
    );
    assert!(
        jira.info().site.contains(CONNECTION),
        "{}",
        jira.info().site
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn for_connection_caches_clients() {
    let fake = FakeJira::new();
    let san = san_mateo(Arc::new(fake.clone()));
    basic_connection(&san, "tenant-a").await;
    basic_connection(&san, "tenant-b").await;
    let state = AppState::for_test();
    state.insert_extension(SanMateoService::new(san));
    let rt = JiraPlugin::with_config(san_mateo_config(None))
        .start(&state)
        .unwrap();
    let jira = rt.service();
    // The same id gives the cached client: the account id cache is shared.
    let a = jira.for_connection("tenant-a").unwrap();
    a.account_id().await.unwrap();
    jira.for_connection("tenant-a")
        .unwrap()
        .account_id()
        .await
        .unwrap();
    assert_eq!(fake.count(FakeOp::Myself), 1);
    let b = jira.for_connection("tenant-b").unwrap();
    assert!(b.info().site.contains("tenant-b"), "{}", b.info().site);
    b.account_id().await.unwrap();
    assert_eq!(fake.count(FakeOp::Myself), 2);
    // A bad id fails before a call.
    let e = jira.for_connection("bad id").unwrap_err();
    assert!(matches!(e, JiraError::InvalidInput { .. }), "{e:?}");
    assert!(jira.for_connection("").is_err());
    // No connection_id: the default client fails with a config error.
    let e = jira.myself().await.unwrap_err();
    assert!(matches!(e, JiraError::Config { .. }), "{e:?}");
    assert_eq!(
        JiraService::from_state(&state).unwrap().info().mode,
        jira::backend::BackendMode::SanMateo
    );

    // Not in san_mateo mode: a config error.
    let rt = plugin(&fake).start(&AppState::for_test()).unwrap();
    let e = rt.service().for_connection("tenant-a").unwrap_err();
    assert!(matches!(e, JiraError::Config { .. }), "{e:?}");
}

/// A route of the app: the service as an extractor, and `.http()?`.
async fn issue(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<Json<Value>> {
    let issue = jira.get_issue(key).await.http()?;
    Ok(Json(json!({
        "key": issue.key.to_string(),
        "summary": issue.fields.summary,
    })))
}

#[tokio::test(flavor = "multi_thread")]
async fn handler_uses_the_service() {
    let fake = FakeJira::new();
    let jira: JiraClient = client_on(Arc::new(fake.clone()));
    let created = jira
        .create_issue(&jira::NewIssue::new(
            ProjectKey::parse("OPS").unwrap(),
            IssueTypeRef::Name("Task".to_owned()),
            "Disk full",
        ))
        .await
        .unwrap();
    let client = TestApp::new()
        .config(autumn_config())
        .plugin(plugin(&fake))
        .merge(Router::new().route("/issues/{key}", get(issue)))
        .build();
    let r = client
        .get(&format!("/issues/{}", created.key))
        .header("accept", "application/json")
        .send()
        .await;
    r.assert_status(200);
    assert_eq!(
        json_of(&r),
        json!({"key": created.key.to_string(), "summary": "Disk full"})
    );
    // A missing issue: 404 from the class table.
    let r = client
        .get("/issues/OPS-999")
        .header("accept", "application/json")
        .send()
        .await;
    r.assert_status(404);
    // A bad key: the strict `Deserialize` of `IssueKey` gives 400.
    let r = client
        .get("/issues/..%2Fx")
        .header("accept", "application/json")
        .send()
        .await;
    r.assert_status(400);
    // A Jira 503 on a read: 502 to the caller after the retries.
    for _ in 0..5 {
        fake.fault(FakeOp::GetIssue, Fault::Status(503));
    }
    let r = client
        .get(&format!("/issues/{}", created.key))
        .header("accept", "application/json")
        .send()
        .await;
    assert_eq!(r.status.as_u16(), 502, "{}", r.text());
}

#[test]
fn autumn_error_keeps_the_status_and_hides_internal_text() {
    use autumn_jira::error::Op;
    let e = autumn_error(&JiraError::NotFoundOrForbidden { op: Op::GetIssue });
    assert_eq!(e.status(), StatusCode::NOT_FOUND);
    let e = autumn_error(&JiraError::RateLimited {
        op: Op::GetIssue,
        retry_after: None,
        reason: None,
    });
    assert_eq!(e.status().as_u16(), 503);
    let e = autumn_error(&JiraError::Decode {
        op: Op::GetIssue,
        reason: "missing field `secretField`".to_owned(),
    });
    assert_eq!(e.status().as_u16(), 502);
    assert!(!e.message().contains("secretField"), "{}", e.message());
    let e = autumn_error(&JiraError::Config {
        code: "no_connection_id",
    });
    assert_eq!(e.status().as_u16(), 500);
    assert!(!e.message().contains("no_connection_id"), "{}", e.message());
    let r: Result<(), JiraError> = Err(JiraError::Conflict { op: Op::EditIssue });
    assert_eq!(r.http().unwrap_err().status().as_u16(), 409);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_runtime_and_the_role() {
    let fake = FakeJira::new();
    // A worker skips the webhook boot check, so a missing endpoint passes.
    let rt = plugin(&fake)
        .webhook_route()
        .start_with_role(&AppState::for_test(), ProcessRole::Worker)
        .unwrap();
    assert_eq!(
        rt.counters(),
        autumn_plugin_jira::WebhookCounters::default()
    );
    assert!(format!("{rt:?}").contains("JiraRuntime"));
    let _ = rt.health();
    // A web process checks it.
    let e = plugin(&fake)
        .webhook_route()
        .start_with_role(&AppState::for_test(), ProcessRole::Web)
        .unwrap_err();
    assert!(matches!(e, PluginError::WebhookEndpoint { .. }), "{e:?}");
    // The started service is in the state.
    let state = AppState::for_test();
    let rt = plugin(&fake).start(&state).unwrap();
    assert!(JiraService::from_state(&state).is_some());
    assert_eq!(rt.service().info().site, "acme.atlassian.net");
    // The Debug output shows no secret.
    let p = plugin(&fake);
    let text = format!("{p:?}");
    assert!(
        text.contains("JiraPlugin") && !text.contains(autumn_jira_fake::DEFAULT_API_TOKEN),
        "{text}"
    );
}

#[cfg(feature = "testing")]
#[test]
fn debug_hides_secret_values() {
    let p = JiraPlugin::new().secret_env("JIRA_API_TOKEN", "very-secret-value");
    let text = format!("{p:?}");
    assert!(text.contains("JIRA_API_TOKEN"), "{text}");
    assert!(!text.contains("very-secret-value"), "{text}");
}

/// The cloud id of the second tenant, `globex`.
const GLOBEX_CLOUD_ID: &str = "22222222-3333-4444-8555-666666666666";

/// Two OAuth tenants, each on its own fake site. `acme` is the default
/// connection, and `[jira.san_mateo]` pins its cloud id and site URL.
async fn two_oauth_tenants() -> (FakeJira, FakeJira, autumn_plugin_jira::JiraRuntime) {
    let globex = with_oauth(FakeJira::new().with_site("globex", GLOBEX_CLOUD_ID));
    let acme = with_oauth(FakeJira::new()).with_fallback(Arc::new(globex.clone()));
    let san = san_mateo(Arc::new(acme.clone()));
    oauth_connection(&san, "acme", autumn_jira_fake::DEFAULT_CLOUD_ID).await;
    oauth_connection(&san, "globex", GLOBEX_CLOUD_ID).await;
    let state = AppState::for_test();
    state.insert_extension(SanMateoService::new(san));
    let config = autumn_plugin_jira::JiraConfig::from_toml_str(&format!(
        "[jira.san_mateo]\nenvironment = \"{ENV}\"\nintegration = \"{OAUTH_INTEGRATION}\"\nconnection_id = \"acme\"\ncloud_id = \"{}\"\nsite_url = \"https://acme.atlassian.net\"\n",
        autumn_jira_fake::DEFAULT_CLOUD_ID
    ))
    .unwrap();
    let rt = JiraPlugin::with_config(config).start(&state).unwrap();
    (acme, globex, rt)
}

/// R10-plugin-ecosystem-1: `cloud_id` and `site_url` of `[jira.san_mateo]`
/// belong to the default connection. Another tenant uses its own site.
#[tokio::test(flavor = "multi_thread")]
async fn the_configured_cloud_id_is_for_the_default_connection_only() {
    let (acme, globex, rt) = two_oauth_tenants().await;
    rt.service().myself().await.unwrap();
    assert_eq!(acme.count(FakeOp::Myself), 1, "the default goes to acme");
    rt.service()
        .for_connection("globex")
        .unwrap()
        .myself()
        .await
        .unwrap();
    assert_eq!(
        (acme.count(FakeOp::Myself), globex.count(FakeOp::Myself)),
        (1, 1),
        "the globex token goes to the globex site, not to acme"
    );
    // The default id through for_connection keeps the configured site.
    rt.service()
        .for_connection("acme")
        .unwrap()
        .myself()
        .await
        .unwrap();
    assert_eq!(acme.count(FakeOp::Myself), 2);
}
