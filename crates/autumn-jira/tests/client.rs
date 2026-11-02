//! Protocol cases of the client over `MemoryTransport` and `FakeJira`
//! (AC2, AC3, AC6 decode cases).

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

mod common;

use std::sync::Arc;

use autumn_jira::client::{ClientSettings, DeleteOutcome};
use autumn_jira::error::{Op, SCOPE_HINT};
use autumn_jira::request::{EditOptions, IssueEdit, IssueTypeRef};
use autumn_jira::san_mateo::transport::HttpResponse;
use autumn_jira::san_mateo::{MemoryTransport, Secret};
use autumn_jira::{BasicAuth, IssueKey, JiraClient, JiraError, NewIssue, ProjectKey};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp, Fault};
use common::{api_token_client, auth, fast_settings, site};
use serde_json::json;

const BASE: &str = "https://acme.atlassian.net/rest/api/3";

fn memory_client(transport: &MemoryTransport) -> JiraClient {
    JiraClient::api_token(site(), auth(), Arc::new(transport.clone()))
        .with_settings(fast_settings())
}

fn key(text: &str) -> IssueKey {
    IssueKey::parse(text).unwrap()
}

fn task() -> NewIssue {
    NewIssue::new(
        ProjectKey::parse("OPS").unwrap(),
        IssueTypeRef::Name("Task".to_owned()),
        "s",
    )
}

#[tokio::test(start_paused = true)]
async fn a_redirect_is_not_followed() {
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/myself"),
        HttpResponse::new(302, "").header("location", "https://evil.example/steal"),
    );
    let client = memory_client(&transport);
    let err = client.myself().await.unwrap_err();
    assert_eq!(
        err,
        JiraError::Redirect {
            op: Op::Myself,
            status: 302
        }
    );
    let sent = transport.requests();
    assert_eq!(sent.len(), 1);
    assert!(sent.iter().all(|r| !r.url.contains("evil.example")));

    // The fake: a redirect on a write is never sent again.
    let fake = FakeJira::new();
    fake.fault(FakeOp::CreateIssue, Fault::Redirect);
    let err = api_token_client(&fake)
        .create_issue(&task())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        JiraError::Redirect {
            op: Op::CreateIssue,
            ..
        }
    ));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    assert_eq!(fake.issues().len(), 0);
}

#[tokio::test]
async fn dot_segment_property_keys_are_rejected() {
    // A URL parser drops `.` and `..` segments: `..` would edit the issue.
    let transport = MemoryTransport::new();
    let client = memory_client(&transport);
    for bad in [".", ".."] {
        let set = client
            .set_issue_property(&key("OPS-1"), bad, &json!({"fields": {"summary": "x"}}))
            .await
            .unwrap_err();
        assert!(
            matches!(set, JiraError::InvalidInput { .. }),
            "{bad}: {set:?}"
        );
        let get = client
            .get_issue_property(&key("OPS-1"), bad)
            .await
            .unwrap_err();
        assert!(
            matches!(get, JiraError::InvalidInput { .. }),
            "{bad}: {get:?}"
        );
    }
    assert!(transport.requests().is_empty(), "no request");
}

#[tokio::test]
async fn path_segments_are_encoded() {
    let transport = MemoryTransport::new();
    let url = format!("{BASE}/issue/OPS-1/properties/a%20b%2Fc%3Fd%23e");
    transport.reply("PUT", &url, HttpResponse::new(200, ""));
    transport.reply(
        "GET",
        &url,
        HttpResponse::json(200, &json!({"key": "a b/c?d#e", "value": 7})),
    );
    let client = memory_client(&transport);
    client
        .set_issue_property(&key("OPS-1"), "a b/c?d#e", &json!(7))
        .await
        .unwrap();
    let value = client
        .get_issue_property(&key("OPS-1"), "a b/c?d#e")
        .await
        .unwrap();
    assert_eq!(value, Some(json!(7)));
    let sent = transport.requests();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].url, url);
    assert_eq!(sent[0].body, b"7");
    // Every URL starts with the one base URL.
    assert!(
        sent.iter()
            .all(|r| r.url.starts_with("https://acme.atlassian.net/rest/api/3/"))
    );
}

#[tokio::test]
async fn basic_header_is_sent() {
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/myself"),
        HttpResponse::json(200, &json!({"accountId": "acct-bot"})),
    );
    memory_client(&transport).myself().await.unwrap();
    let expected = format!(
        "Basic {}",
        data_encoding::BASE64.encode(format!("{DEFAULT_EMAIL}:{DEFAULT_API_TOKEN}").as_bytes())
    );
    let sent = transport.requests();
    assert_eq!(
        sent[0].header_value("authorization"),
        Some(expected.as_str())
    );
    // The fake checks the same header.
    let fake = FakeJira::new();
    let me = api_token_client(&fake).myself().await.unwrap();
    assert_eq!(me.account_id.as_str(), autumn_jira_fake::BOT);
}

#[tokio::test(start_paused = true)]
async fn unauthorized_is_not_retried() {
    let fake = FakeJira::new();
    let wrong = BasicAuth::new(DEFAULT_EMAIL, Secret::new("wrong-token")).unwrap();
    let client =
        JiraClient::api_token(site(), wrong, Arc::new(fake.clone())).with_settings(fast_settings());
    let err = client.myself().await.unwrap_err();
    assert_eq!(
        err,
        JiraError::Unauthorized {
            op: Op::Myself,
            status: 401,
            hint: SCOPE_HINT
        }
    );
    assert_eq!(fake.count(FakeOp::Myself), 1);
    assert!(err.to_string().contains("read:jira-work"));
    assert!(!err.to_string().contains("wrong-token"));

    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/myself"),
        HttpResponse::new(403, "no"),
    );
    let err = memory_client(&transport).myself().await.unwrap_err();
    assert!(matches!(err, JiraError::Unauthorized { status: 403, .. }));
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn captcha_is_typed() {
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/myself"),
        HttpResponse::new(401, "").header("X-Seraph-LoginReason", "AUTHENTICATION_DENIED"),
    );
    let err = memory_client(&transport).myself().await.unwrap_err();
    assert_eq!(err, JiraError::CaptchaRequired { op: Op::Myself });
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn moved_issue_key_is_kept() {
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/issue/OLD-1"),
        HttpResponse::json(
            200,
            &json!({"id": "10001", "key": "NEW-7", "fields": {"summary": "moved"}}),
        ),
    );
    let issue = memory_client(&transport)
        .get_issue(&key("OLD-1"))
        .await
        .unwrap();
    assert_eq!(issue.key.as_str(), "NEW-7");
    assert_eq!(issue.id.as_str(), "10001");
    assert!(transport.requests()[0].url.ends_with(
        "?fields=summary%2Cstatus%2Cissuetype%2Cproject%2Cassignee%2Clabels%2Ccreated%2Cupdated%2Cdescription"
    ));
}

#[tokio::test]
async fn one_search_page_at_a_token() {
    use autumn_jira::request::SearchRequest;
    let fake = FakeJira::new();
    let client = api_token_client(&fake);
    for n in 0..3 {
        client
            .create_issue(&task().labels([format!("page-{n}"), "paged".to_owned()]))
            .await
            .unwrap();
    }
    let jql = autumn_jira::Jql::raw("labels = paged ORDER BY key ASC");
    let request = SearchRequest::new(jql).max_results(2);
    let first = client.search_page_at(&request, None).await.unwrap();
    assert_eq!(first.issues.len(), 2);
    let token = first.next_token.expect("a next page");
    let second = client.search_page_at(&request, Some(&token)).await.unwrap();
    assert_eq!(second.issues.len(), 1);
    assert_eq!(second.next_token, None);
    let mut keys: Vec<String> = first
        .issues
        .iter()
        .chain(&second.issues)
        .map(|i| i.key.to_string())
        .collect();
    keys.dedup();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn short_non_last_comment_page() {
    let url = format!("{BASE}/issue/OPS-1/comment");
    let comment =
        |id: &str| json!({"id": id, "body": {"type": "doc", "version": 1, "content": []}});
    // The first page is short but not the last: `total` says 3.
    let transport = MemoryTransport::new();
    let pages = [
        json!({"startAt": 0, "total": 3, "comments": [comment("1")]}),
        json!({"startAt": 1, "total": 3, "comments": [comment("2"), comment("3")]}),
    ];
    for page in &pages {
        transport.reply("GET", &url, HttpResponse::json(200, page));
    }
    let comments = memory_client(&transport)
        .all_comments(&key("OPS-1"))
        .await
        .unwrap();
    let ids: Vec<&str> = comments.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["1", "2", "3"]);
    let sent = transport.requests();
    assert_eq!(sent.len(), 2);
    assert!(sent[1].url.contains("startAt=1"), "{}", sent[1].url);

    // An empty page ends the list, even when `total` says more.
    let empty = MemoryTransport::new();
    empty.reply(
        "GET",
        &url,
        HttpResponse::json(200, &json!({"startAt": 0, "total": 9, "comments": []})),
    );
    assert_eq!(
        memory_client(&empty)
            .all_comments(&key("OPS-1"))
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(empty.requests().len(), 1);
}

#[tokio::test]
async fn empty_204_is_not_parsed_and_statuses_are_checked() {
    let transport = MemoryTransport::new();
    transport.reply(
        "PUT",
        &format!("{BASE}/issue/OPS-1"),
        HttpResponse::new(204, ""),
    );
    transport.reply(
        "DELETE",
        &format!("{BASE}/issue/OPS-1"),
        HttpResponse::new(204, ""),
    );
    transport.reply(
        "POST",
        &format!("{BASE}/issue"),
        HttpResponse::json(200, &json!({"id": "1", "key": "OPS-1"})),
    );
    let client = memory_client(&transport);
    client
        .edit_issue(
            &key("OPS-1"),
            &IssueEdit::new().set_summary("x"),
            EditOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        client.delete_issue(&key("OPS-1"), true).await.unwrap(),
        DeleteOutcome::Deleted
    );
    // Create expects 201. A 200 is not a success.
    let err = client.create_issue(&task()).await.unwrap_err();
    assert_eq!(
        err,
        JiraError::UnexpectedStatus {
            op: Op::CreateIssue,
            status: 200
        }
    );
    assert!(
        transport.requests()[1]
            .url
            .ends_with("?deleteSubtasks=true")
    );
}

#[test]
fn debug_has_no_token() {
    let client = JiraClient::api_token(site(), auth(), Arc::new(MemoryTransport::new()));
    let text = format!("{client:?}");
    assert!(text.contains("acme.atlassian.net"), "{text}");
    assert!(
        !text.contains(DEFAULT_API_TOKEN) && !text.contains(DEFAULT_EMAIL),
        "{text}"
    );
    assert_eq!(client.settings(), &ClientSettings::default());
    assert_eq!(client.info().site, "acme.atlassian.net");
}

/// Starts a server on 127.0.0.1 that reads each request in full. It gives
/// an empty search page, and it drops the socket after a create POST, with
/// no reply. Returns `http://127.0.0.1:<port>`.
fn reset_after_write_server() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0_usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0_u8; length];
            let _ = reader.read_exact(&mut body);
            if head.starts_with("POST /rest/api/3/issue ") {
                // Jira has the whole request. The reply is lost.
                drop(stream);
                continue;
            }
            let page = br#"{"issues":[],"isLast":true}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                page.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.write_all(page);
        }
    });
    base
}

/// `ReqwestTransport` with the Jira host replaced by a local server.
struct ToLocal {
    inner: autumn_jira::san_mateo::ReqwestTransport,
    base: String,
}

#[async_trait::async_trait]
impl autumn_jira::san_mateo::transport::HttpTransport for ToLocal {
    async fn send(
        &self,
        mut request: autumn_jira::san_mateo::transport::HttpRequest,
    ) -> Result<HttpResponse, autumn_jira::san_mateo::transport::TransportError> {
        request.url = request
            .url
            .replace("https://acme.atlassian.net", &self.base);
        self.inner.send(request).await
    }
}

#[tokio::test]
async fn reset_after_write_is_maybe_sent() {
    use autumn_jira::backend::{ApiRequest, BackendError, JiraBackend, Method};
    use autumn_jira::san_mateo::transport::TransportConfig;
    let inner = autumn_jira::san_mateo::ReqwestTransport::new(TransportConfig::default()).unwrap();
    let local = Arc::new(ToLocal {
        inner,
        base: reset_after_write_server(),
    });
    let backend = common::direct(local.clone());
    let post = ApiRequest::new(Op::CreateIssue, Method::Post, "/rest/api/3/issue")
        .json(&json!({"fields": {}}));
    let err = backend.send(post).await.unwrap_err();
    assert!(matches!(err, BackendError::MaybeSent { .. }), "{err:?}");
    // The idempotent create looks up the marker and does not say "not sent".
    let client = JiraClient::new(Arc::new(backend)).with_settings(fast_settings());
    let k = autumn_jira::IdempotencyKey::derive(&["reset"]);
    let err = client
        .create_issue_idempotent(&task(), &k)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            JiraError::OutcomeUnknown {
                marker: Some(_),
                ..
            }
        ),
        "{err:?}"
    );
    assert!(err.may_have_run());
}

#[tokio::test]
async fn offset_pager_reads_the_first_page_like_the_token_pager() {
    let transport = MemoryTransport::new();
    let first = format!("{BASE}/project/search?startAt=0&maxResults=50");
    let page = |is_last: bool| {
        HttpResponse::json(
            200,
            &json!({"values": [{"id": "10000", "key": "OPS", "name": "Ops"}],
                "startAt": 0, "maxResults": 50, "total": 2, "isLast": is_last}),
        )
    };
    transport.reply("GET", &first, page(true));
    let mut settings = fast_settings();
    settings.max_pages = 0;
    let client = memory_client(&transport).with_settings(settings.clone());
    // One page that is the last page: no page limit.
    let projects = client.all_projects().await.unwrap();
    assert_eq!(projects.len(), 1);
    // More pages after the cap: PageLimit after the first page.
    let more = MemoryTransport::new();
    more.reply("GET", &first, page(false));
    let client = memory_client(&more).with_settings(settings);
    assert!(matches!(
        client.all_projects().await.unwrap_err(),
        JiraError::PageLimit { max: 0, .. }
    ));
    assert_eq!(more.requests().len(), 1);
}

/// AC8: an empty transitions list gives `TransitionNotAvailable` with no
/// names, and the client sends no POST.
#[tokio::test]
async fn an_empty_transitions_list_names_no_transition() {
    use autumn_jira::StatusTarget;
    let transport = MemoryTransport::new();
    transport.reply(
        "GET",
        &format!("{BASE}/issue/OPS-1"),
        HttpResponse::json(
            200,
            &json!({"id": "10001", "key": "OPS-1", "fields": {"status": {
                "id": "10000", "name": "To Do", "statusCategory": {"key": "new"}
            }}}),
        ),
    );
    transport.reply(
        "GET",
        &format!("{BASE}/issue/OPS-1/transitions"),
        HttpResponse::json(200, &json!({"transitions": []})),
    );
    let client = memory_client(&transport);
    let err = client
        .transition_to_status(&key("OPS-1"), &StatusTarget::done())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        JiraError::TransitionNotAvailable {
            target: StatusTarget::done().to_string(),
            available: vec![],
        }
    );
    let sent = transport.requests();
    assert_eq!(sent.len(), 2, "one status read and one transitions read");
    assert!(sent.iter().all(|r| r.method == "GET"), "no POST");
    assert!(sent[0].url.contains("fields=status"), "{}", sent[0].url);
}

/// A reconnect can give the same connection another Atlassian user. The
/// marker lookup compares the comment author with the account id. So
/// `refresh_site` drops the cached account id, and the next call reads
/// `GET /myself` again.
#[tokio::test(start_paused = true)]
async fn refresh_site_reads_the_account_id_again() {
    let transport = MemoryTransport::new();
    for account in ["acct-old", "acct-new"] {
        transport.reply(
            "GET",
            &format!("{BASE}/myself"),
            HttpResponse::json(200, &json!({"accountId": account})),
        );
    }
    let client = memory_client(&transport);
    assert_eq!(client.account_id().await.unwrap().as_str(), "acct-old");
    assert_eq!(client.account_id().await.unwrap().as_str(), "acct-old");
    assert_eq!(transport.requests().len(), 1, "the second call uses the cache");
    client.refresh_site().await.unwrap();
    assert_eq!(client.account_id().await.unwrap().as_str(), "acct-new");
    assert_eq!(transport.requests().len(), 2);
}
