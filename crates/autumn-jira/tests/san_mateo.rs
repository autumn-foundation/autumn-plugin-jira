//! `SanMateoBackend` over a san-mateo facade (AC4, spike S5).

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;

use autumn_jira::backend::{
    ApiRequest, BackendInfo, BackendMode, JiraBackend, Method, SanMateoBackend, SanMateoTarget,
};
use autumn_jira::error::Op;
use autumn_jira::san_mateo::transport::HttpResponse;
use autumn_jira::san_mateo::{GetOptions, MemoryTransport, ProxyRequest};
use autumn_jira_fake::{DEFAULT_CLOUD_ID, FakeJira, FakeOp, Fault};
use common::{ENV, basic_connection, cloud_id, oauth_connection, san_mateo, with_oauth};
use serde_json::{Map, json};

const MYSELF: &str = "https://acme.atlassian.net/rest/api/3/myself";
const CREATE: &str = "https://acme.atlassian.net/rest/api/3/issue";

fn myself() -> ApiRequest {
    ApiRequest::new(Op::Myself, Method::Get, "/rest/api/3/myself")
}

/// Spike S5: `SanMateo::proxy` with `retries: 0` sends one request for a
/// 503 and one for a 429. The backend keeps this rule.
#[tokio::test(start_paused = true)]
async fn one_send_for_a_503_and_for_a_429() {
    let transport = MemoryTransport::new();
    transport.reply("GET", MYSELF, HttpResponse::new(503, "busy"));
    transport.reply(
        "POST",
        CREATE,
        HttpResponse::new(429, "slow down").header("retry-after", "1"),
    );
    let san = san_mateo(Arc::new(transport.clone()));
    let key = basic_connection(&san).await;

    // The spike: the san-mateo proxy itself.
    let get = san
        .proxy(
            ENV,
            &key,
            ProxyRequest::get("/rest/api/3/myself").retries(0),
        )
        .await
        .unwrap();
    assert_eq!(get.status, 503);
    let post = san
        .proxy(
            ENV,
            &key,
            ProxyRequest::new("POST", "/rest/api/3/issue")
                .json(&json!({"fields": {}}))
                .retries(0),
        )
        .await
        .unwrap();
    assert_eq!(post.status, 429);
    assert_eq!(transport.requests_to(MYSELF).len(), 1);
    assert_eq!(transport.requests_to(CREATE).len(), 1);

    // The backend: one send for each attempt.
    let backend = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    assert_eq!(backend.send(myself()).await.unwrap().status, 503);
    let create = ApiRequest::new(Op::CreateIssue, Method::Post, "/rest/api/3/issue")
        .json(&json!({"fields": {}}));
    let reply = backend.send(create).await.unwrap();
    assert_eq!(reply.status, 429);
    assert_eq!(reply.header("Retry-After"), Some("1"));
    assert_eq!(transport.requests_to(MYSELF).len(), 2);
    assert_eq!(transport.requests_to(CREATE).len(), 2);
    let sent = &transport.requests_to(CREATE)[1];
    assert_eq!(sent.header_value("content-type"), Some("application/json"));
    assert_eq!(sent.header_value("accept"), Some("application/json"));
    assert!(
        sent.header_value("authorization")
            .unwrap()
            .starts_with("Basic ")
    );
    assert_eq!(
        backend.info(),
        BackendInfo {
            mode: BackendMode::SanMateo,
            site: "san-mateo:jira/acme".to_owned()
        }
    );
}

#[tokio::test(start_paused = true)]
async fn discovery_stores_the_cloud_id() {
    let fake = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fake.clone()));
    let key = oauth_connection(&san, Map::new()).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    let id = backend.discover_cloud_id().await.unwrap();
    assert_eq!(id, cloud_id());
    let conn = san
        .get_connection(ENV, &key, GetOptions::default())
        .await
        .unwrap();
    assert_eq!(conn.metadata["cloudId"], json!(DEFAULT_CLOUD_ID));
    // The next call reads the stored id: no second discovery.
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 1);
    let last = fake.requests().pop().unwrap();
    assert_eq!(last.host, "api.atlassian.com");
    assert_eq!(
        last.path,
        format!("/ex/jira/{DEFAULT_CLOUD_ID}/rest/api/3/myself")
    );

    // With no stored id, the first call discovers it.
    let fresh = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fresh.clone()));
    let key = oauth_connection(&san, Map::new()).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fresh.count(FakeOp::AccessibleResources), 1);
    let conn = san
        .get_connection(ENV, &key, GetOptions::default())
        .await
        .unwrap();
    assert_eq!(conn.metadata["cloudId"], json!(DEFAULT_CLOUD_ID));
}

#[tokio::test(start_paused = true)]
async fn metadata_cloud_id_skips_discovery() {
    let fake = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fake.clone()));
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(DEFAULT_CLOUD_ID))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 0);
    assert_eq!(fake.count(FakeOp::Myself), 2);

    // A cloud id in the target skips the metadata and the discovery.
    let target = SanMateoTarget::new(ENV, key).cloud_id(cloud_id());
    let backend = SanMateoBackend::new(san, target);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 0);
}

#[tokio::test(start_paused = true)]
async fn a_401_refreshes_once() {
    let fake = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fake.clone()));
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(DEFAULT_CLOUD_ID))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    fake.expire_access_token();
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.token_refreshes(), 1);
    assert_eq!(fake.count(FakeOp::Myself), 2);
    // The new token works: no second refresh.
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.token_refreshes(), 1);

    // A 503 after commit on a write: one send, the 503 comes back.
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let create = ApiRequest::new(Op::CreateIssue, Method::Post, "/rest/api/3/issue").json(
        &json!({"fields": {"project": {"key": "OPS"}, "issuetype": {"name": "Task"}, "summary": "x"}}),
    );
    assert_eq!(backend.send(create).await.unwrap().status, 503);
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
}

#[tokio::test]
async fn backend_errors_are_typed() {
    // A connection that does not exist: not sent, not transient.
    let san = san_mateo(Arc::new(MemoryTransport::new()));
    let key = basic_connection(&san).await;
    let mut missing = key.clone();
    missing.connection_id = "nobody".to_owned();
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, missing));
    assert_eq!(
        backend.send(myself()).await.unwrap_err(),
        autumn_jira::backend::BackendError::NotSent {
            transient: false,
            code: "unknown_connection"
        }
    );
    // `jira-basic` has no cloud id to discover.
    let basic = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    assert!(matches!(
        basic.discover_cloud_id().await,
        Err(autumn_jira::JiraError::CloudId(_))
    ));
}

/// A cloud id of a site that the token does not cover any more.
const OLD_SITE: &str = "99999999-2222-4333-8444-555555555555";

#[tokio::test(start_paused = true)]
async fn a_stale_metadata_cloud_id_is_checked_after_a_404() {
    // The tenant connected site OLD, then reconnected to the fake site.
    // san-mateo keeps the old metadata.
    let fake = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fake.clone()));
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(OLD_SITE))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    // The first call goes to the old site and gets 404. The backend does
    // not send it again.
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(fake.count(FakeOp::Myself), 0);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 1);
    let conn = san
        .get_connection(ENV, &key, GetOptions::default())
        .await
        .unwrap();
    assert_eq!(conn.metadata["cloudId"], json!(DEFAULT_CLOUD_ID));
    // The next call goes to the site of the token.
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(fake.count(FakeOp::Myself), 1);

    // A discovery confirms the id: a 404 now does not check it again.
    let issue = ApiRequest::new(Op::GetIssue, Method::Get, "/rest/api/3/issue/OPS-404");
    assert_eq!(backend.send(issue.clone()).await.unwrap().status, 404);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 1);

    // A new backend reads the stored id. A 404 under a cloud id that the
    // token covers changes nothing, and the backend checks it one time only.
    let backend = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    for _ in 0..3 {
        assert_eq!(backend.send(issue.clone()).await.unwrap().status, 404);
    }
    assert_eq!(fake.count(FakeOp::AccessibleResources), 2);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
}

/// The cloud id of the site that the tenant moves to.
const NEW_SITE: &str = "77777777-2222-4333-8444-555555555555";

/// A transport that serves fake `a` until `moved`, then fake `b`: the
/// tenant reconnects the same san-mateo connection to another site.
struct Moving {
    a: FakeJira,
    b: FakeJira,
    moved: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl autumn_jira::san_mateo::transport::HttpTransport for Moving {
    async fn send(
        &self,
        request: autumn_jira::san_mateo::transport::HttpRequest,
    ) -> Result<HttpResponse, autumn_jira::san_mateo::transport::TransportError> {
        if self.moved.load(std::sync::atomic::Ordering::SeqCst) {
            self.b.send(request).await
        } else {
            self.a.send(request).await
        }
    }
}

fn moving() -> Arc<Moving> {
    Arc::new(Moving {
        a: with_oauth(FakeJira::new()),
        b: with_oauth(FakeJira::new().with_site("beta", NEW_SITE)),
        moved: std::sync::atomic::AtomicBool::new(false),
    })
}

/// Moves the tenant to site B, then waits out the check interval.
async fn move_site(sites: &Moving) {
    sites.moved.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::advance(autumn_jira::backend::CLOUD_ID_RECHECK_INTERVAL).await;
}

#[tokio::test(start_paused = true)]
async fn a_reconnect_after_confirmation_is_seen() {
    let sites = moving();
    let san = san_mateo(sites.clone());
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(DEFAULT_CLOUD_ID))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    // An ordinary 404 confirms the stored id. More 404s in the interval do
    // not check again.
    let issue = ApiRequest::new(Op::GetIssue, Method::Get, "/rest/api/3/issue/OPS-404");
    for _ in 0..3 {
        assert_eq!(backend.send(issue.clone()).await.unwrap().status, 404);
    }
    assert_eq!(sites.a.count(FakeOp::AccessibleResources), 1);
    move_site(&sites).await;
    // The old id gets a 404 now. The backend checks again and stores B.
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(sites.b.count(FakeOp::AccessibleResources), 1);
    let conn = san
        .get_connection(ENV, &key, GetOptions::default())
        .await
        .unwrap();
    assert_eq!(conn.metadata["cloudId"], json!(NEW_SITE));
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    let last = sites.b.requests().pop().unwrap();
    assert_eq!(last.path, format!("/ex/jira/{NEW_SITE}/rest/api/3/myself"));
}

#[tokio::test(start_paused = true)]
async fn a_reconnect_after_discovery_is_seen() {
    let sites = moving();
    let san = san_mateo(sites.clone());
    let key = oauth_connection(&san, Map::new()).await;
    let backend = SanMateoBackend::new(san.clone(), SanMateoTarget::new(ENV, key.clone()));
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    assert_eq!(sites.a.count(FakeOp::AccessibleResources), 1);
    move_site(&sites).await;
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    let conn = san
        .get_connection(ENV, &key, GetOptions::default())
        .await
        .unwrap();
    assert_eq!(conn.metadata["cloudId"], json!(NEW_SITE));
}

/// R3-webhooks-site-auth-1: a failed check counts as a check. While the
/// failure lasts, the 401 and 404 replies make at most one
/// `accessible-resources` call in each interval.
#[tokio::test(start_paused = true)]
async fn a_failed_check_waits_for_the_interval() {
    // Case 1: after the move, no site of the token has the site URL, so
    // the pick fails.
    let sites = moving();
    let san = san_mateo(sites.clone());
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(DEFAULT_CLOUD_ID))]);
    let key = oauth_connection(&san, metadata).await;
    let target = SanMateoTarget::new(ENV, key).site_url("https://acme.atlassian.net");
    let backend = SanMateoBackend::new(san, target);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    move_site(&sites).await;
    for _ in 0..5 {
        assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    }
    assert_eq!(sites.b.count(FakeOp::AccessibleResources), 1);
    tokio::time::advance(autumn_jira::backend::CLOUD_ID_RECHECK_INTERVAL).await;
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(sites.b.count(FakeOp::AccessibleResources), 2);

    // Case 2: `accessible-resources` answers 503. The next check comes
    // after the interval, and then it repairs the id.
    let fake = with_oauth(FakeJira::new());
    fake.fault(FakeOp::AccessibleResources, Fault::Status(503));
    let san = san_mateo(Arc::new(fake.clone()));
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(OLD_SITE))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    for _ in 0..3 {
        assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    }
    assert_eq!(fake.count(FakeOp::AccessibleResources), 1);
    tokio::time::advance(autumn_jira::backend::CLOUD_ID_RECHECK_INTERVAL).await;
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(fake.count(FakeOp::AccessibleResources), 2);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
}

/// R3-webhooks-site-auth-2: an explicit `discover_cloud_id` also resets the
/// cached prefix. The next request goes to the new cloud id.
#[tokio::test(start_paused = true)]
async fn discover_cloud_id_resets_the_cached_prefix() {
    let sites = moving();
    let san = san_mateo(sites.clone());
    let metadata = Map::from_iter([("cloudId".to_owned(), json!(DEFAULT_CLOUD_ID))]);
    let key = oauth_connection(&san, metadata).await;
    let backend = SanMateoBackend::new(san, SanMateoTarget::new(ENV, key));
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    move_site(&sites).await;
    let id = backend.discover_cloud_id().await.unwrap();
    assert_eq!(id.as_str(), NEW_SITE);
    assert_eq!(backend.send(myself()).await.unwrap().status, 200);
    let last = sites.b.requests().pop().unwrap();
    assert_eq!(last.path, format!("/ex/jira/{NEW_SITE}/rest/api/3/myself"));

    // A cloud id of the target stays fixed: the discovery stores the id,
    // and the requests keep the id of the target.
    let fake = with_oauth(FakeJira::new());
    let san = san_mateo(Arc::new(fake.clone()));
    let key = oauth_connection(&san, Map::new()).await;
    let target =
        SanMateoTarget::new(ENV, key).cloud_id(autumn_jira::ids::CloudId::parse(OLD_SITE).unwrap());
    let backend = SanMateoBackend::new(san, target);
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(
        backend.discover_cloud_id().await.unwrap().as_str(),
        DEFAULT_CLOUD_ID
    );
    assert_eq!(backend.send(myself()).await.unwrap().status, 404);
    assert_eq!(fake.count(FakeOp::Myself), 0);
}
