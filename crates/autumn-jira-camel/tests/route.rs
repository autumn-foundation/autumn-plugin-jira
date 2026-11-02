//! AC36 and AC37: real camel routes send exchanges to the `jira:` producer.
//! `FakeJira` is the transport of the client. No test calls a real Jira.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "tests"
)]

mod common;

use std::sync::Arc;

use autumn_jira_camel::{
    HEADER_ISSUE_ID, HEADER_ISSUE_KEY, HEADER_NEXT_TOKEN, HEADER_OUTCOME, JiraComponent, SCHEME,
    uri_options,
};
use autumn_jira_fake::{FakeJira, FakeOp, Fault};
use camel_api::{Body, CamelError, Message};
use camel_component_api::{Component, NoOpComponentContext, RuntimeObservability};
use common::*;
use serde_json::json;

/// The option names of the metadata, in order.
fn option_names() -> Vec<String> {
    uri_options().into_iter().map(|o| o.name).collect()
}

/// The routes of the CRUD test.
const CRUD: &[(&str, &str)] = &[
    (
        "direct:create",
        "jira:createIssue?project=OPS&issueType=Task",
    ),
    (
        "direct:get",
        "jira:getIssue?issueKey=OPS-1&fields=summary,status,labels",
    ),
    (
        "direct:edit",
        "jira:editIssue?issueKey=OPS-1&notifyUsers=false",
    ),
    (
        "direct:move",
        "jira:transitionToStatus?issueKey=OPS-1&status=category:indeterminate",
    ),
    ("direct:comment", "jira:addComment?issueKey=OPS-1"),
    ("direct:page", "jira:search?project=OPS&maxResults=1"),
    (
        "direct:all",
        "jira:search?jql=project%20%3D%20OPS&maxResults=1&maxPages=5&fields=summary",
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_get_edit_transition_comment_search() {
    let fake = FakeJira::new();
    let camel = start(component(&fake), CRUD).await.unwrap();

    // createIssue: the body gives summary, description, labels and fields.
    let ex = send(
        &camel,
        "direct:create",
        json(json!({
            "summary": "Disk full on db-1",
            "description": "db-1\nat 95%",
            "labels": ["alert"],
            "fields": {"priority": {"name": "High"}}
        })),
    )
    .await
    .unwrap();
    let bean = fake.issue("OPS-1").unwrap();
    let id = bean["id"].as_str().unwrap().to_owned();
    assert_eq!(
        reply(&ex),
        json!({"id": id, "key": "OPS-1", "outcome": "created"})
    );
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), Some(json!("OPS-1")));
    assert_eq!(header(&ex, HEADER_ISSUE_ID), Some(json!(id)));
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("created")));
    assert_eq!(bean["fields"]["summary"], json!("Disk full on db-1"));
    assert_eq!(bean["fields"]["labels"], json!(["alert"]));
    assert_eq!(bean["fields"]["priority"]["name"], json!("High"));
    assert_eq!(
        bean["fields"]["description"]["content"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "one paragraph for each line"
    );
    // A plain create has no marker label.
    let ex = send(&camel, "direct:create", json(json!({"summary": "Second"})))
        .await
        .unwrap();
    assert_eq!(reply(&ex)["key"], json!("OPS-2"));
    assert_eq!(fake.issue("OPS-2").unwrap()["fields"]["labels"], json!([]));

    // getIssue: the body is ignored. The reply is the issue.
    let ex = send(&camel, "direct:get", json(json!({"ignored": true})))
        .await
        .unwrap();
    let issue = reply(&ex);
    assert_eq!(issue["key"], json!("OPS-1"));
    assert_eq!(issue["id"], json!(id));
    assert_eq!(issue["fields"]["summary"], json!("Disk full on db-1"));
    assert_eq!(issue["fields"]["status"]["name"], json!("To Do"));
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), Some(json!("OPS-1")));
    assert_eq!(header(&ex, HEADER_ISSUE_ID), Some(json!(id)));
    assert_eq!(header(&ex, HEADER_OUTCOME), None, "a read has no outcome");

    // editIssue: setSummary, addLabels, removeLabels and fields.
    let ex = send(
        &camel,
        "direct:edit",
        json(json!({
            "setSummary": "Disk full on db-1 (95%)",
            "addLabels": ["triaged"],
            "removeLabels": ["alert"],
            "fields": {"priority": {"name": "Low"}}
        })),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex), json!({"issue": "OPS-1"}));
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), Some(json!("OPS-1")));
    let bean = fake.issue("OPS-1").unwrap();
    assert_eq!(bean["fields"]["summary"], json!("Disk full on db-1 (95%)"));
    assert_eq!(bean["fields"]["labels"], json!(["triaged"]));
    assert_eq!(bean["fields"]["priority"]["name"], json!("Low"));
    let edit = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT")
        .unwrap();
    assert_eq!(edit.query, "notifyUsers=false");

    // transitionToStatus: moved, then already there with no POST.
    let ex = send(&camel, "direct:move", Message::default())
        .await
        .unwrap();
    let moved = reply(&ex);
    assert_eq!(moved["issue"], json!("OPS-1"));
    assert_eq!(moved["outcome"], json!("moved"));
    assert_eq!(moved["status"]["name"], json!("In Progress"));
    assert_eq!(moved["status"]["category"], json!("indeterminate"));
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("moved")));
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), Some(json!("OPS-1")));
    let ex = send(&camel, "direct:move", Message::default())
        .await
        .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("already_there"));
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("already_there")));
    assert_eq!(fake.count(FakeOp::DoTransition), 1);

    // addComment: a JSON object with text, or a plain string.
    let ex = send(
        &camel,
        "direct:comment",
        json(json!({"text": "Looking now"})),
    )
    .await
    .unwrap();
    let comment = reply(&ex);
    assert_eq!(comment["issue"], json!("OPS-1"));
    assert_eq!(comment["outcome"], json!("created"));
    assert!(comment["id"].as_str().is_some(), "{comment}");
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("created")));
    let ex = send(
        &camel,
        "direct:comment",
        Message::new(Body::Text("Fixed".into())),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("created"));
    send(&camel, "direct:comment", json(json!("Closing")))
        .await
        .unwrap();
    assert_eq!(fake.count(FakeOp::AddComment), 3);
    let snapshot = fake.snapshot().to_string();
    for text in ["Looking now", "Fixed", "Closing"] {
        assert!(snapshot.contains(text), "{text}: {snapshot}");
    }

    // search: one page of one issue gives the token of the next page.
    let ex = send(&camel, "direct:page", Message::default())
        .await
        .unwrap();
    let page = reply(&ex);
    assert_eq!(page["issues"].as_array().unwrap().len(), 1);
    assert_eq!(page["issues"][0]["key"], json!("OPS-2"), "newest first");
    let token = header(&ex, HEADER_NEXT_TOKEN).expect("a next page token");
    assert!(token.as_str().is_some_and(|t| !t.is_empty()), "{token}");
    assert_eq!(page["nextPageToken"], token);
    // maxPages=5 reads every page: no token.
    let ex = send(&camel, "direct:all", Message::default())
        .await
        .unwrap();
    let all = reply(&ex);
    let keys: Vec<&str> = all["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["OPS-2", "OPS-1"]);
    assert_eq!(all["nextPageToken"], json!(null));
    assert_eq!(header(&ex, HEADER_NEXT_TOKEN), None);
    // A stale token header of an earlier step does not stay.
    let mut stale = Message::default();
    stale.set_header(HEADER_NEXT_TOKEN, "old");
    let ex = send(&camel, "direct:all", stale).await.unwrap();
    assert_eq!(header(&ex, HEADER_NEXT_TOKEN), None);
    stop(camel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumer_is_rejected() {
    let fake = FakeJira::new();
    let component = component(&fake);
    assert_eq!(component.scheme(), SCHEME);
    let metadata = component.metadata();
    assert!(metadata.capabilities.supports_producer);
    assert!(!metadata.capabilities.supports_consumer);
    assert!(!metadata.capabilities.supports_polling_consumer);
    let names: Vec<String> = metadata
        .uri_options
        .iter()
        .map(|o| o.name.clone())
        .collect();
    assert_eq!(names, option_names());
    assert!(metadata.validate_scheme(SCHEME).is_ok());

    let endpoint = component
        .create_endpoint("jira:getIssue?issueKey=OPS-1", &NoOpComponentContext)
        .unwrap();
    assert_eq!(endpoint.uri(), "jira:getIssue?issueKey=OPS-1");
    let rt: Arc<dyn RuntimeObservability> = Arc::new(NoOpComponentContext);
    match endpoint.create_consumer(rt) {
        Err(CamelError::EndpointCreationFailed(m)) => assert_eq!(
            m,
            "jira: is producer-only; use it in to(...), not in from(...)"
        ),
        Err(other) => panic!("expected EndpointCreationFailed, got {other:?}"),
        Ok(_) => panic!("a consumer must fail"),
    }

    // In a route, `from("jira:...")` fails at the route add.
    let mut camel = camel_core::CamelContext::builder().build().await.unwrap();
    camel.register_component(component);
    let route = camel_builder::RouteBuilder::from("jira:getIssue?issueKey=OPS-1")
        .route_id("from-jira")
        .build()
        .unwrap();
    let added = camel.add_route_definition(route).await;
    let started = match added {
        Ok(()) => camel.start().await,
        Err(e) => Err(e),
    };
    let err = started.expect_err("a jira: consumer must fail");
    assert!(err.to_string().contains("producer-only"), "{err}");
    let _ = camel.stop().await;
    assert_eq!(fake.requests().len(), 0, "no call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_option_lists_names() {
    let fake = FakeJira::new();
    let m = route_add_error(
        component(&fake),
        "jira:getIssue?issueKey=OPS-1&issuekey=OPS-2",
    )
    .await;
    assert!(m.contains("unknown jira: option 'issuekey'"), "{m}");
    let listed = m.split("the options are ").nth(1).unwrap();
    assert!(listed.starts_with(&option_names().join(", ")), "{m}");

    match component(&fake).endpoint_options("jira:search?projectt=OPS") {
        Err(CamelError::InvalidUri(m)) => {
            for name in option_names() {
                assert!(m.contains(&name), "{name}: {m}");
            }
        }
        other => panic!("expected InvalidUri, got {other:?}"),
    }
    // An unknown operation lists the operations. The match is exact.
    let m = route_add_error(component(&fake), "jira:createissue?project=OPS").await;
    assert!(
        m.contains("createissue") && m.contains("createIssue"),
        "{m}"
    );
    // An unknown connection is a config error that lists the names.
    let named = component(&fake).connection("eu", client(&fake));
    match named.endpoint_options("jira:getIssue?issueKey=OPS-1&connection=us") {
        Err(CamelError::Config(m)) => assert!(m.contains("default, eu"), "{m}"),
        other => panic!("expected Config, got {other:?}"),
    }
    let options = named
        .endpoint_options("jira:getIssue?issueKey=OPS-1&connection=eu")
        .unwrap();
    assert_eq!(options.connection, "eu");
    assert!(fake.requests().is_empty(), "no call at the route add");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_and_jql_conflict() {
    let fake = FakeJira::new();
    let m = route_add_error(
        component(&fake),
        "jira:search?project=OPS&jql=status%20%3D%20Done",
    )
    .await;
    assert!(m.contains("project") && m.contains("jql"), "{m}");
    let m = route_add_error(component(&fake), "jira:search").await;
    assert!(m.contains("allowHeaderOverrides"), "{m}");
    assert_eq!(fake.requests().len(), 0, "no call");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_hold_no_body() {
    const SECRET: &str = "s3cr3t-body-text";
    let fake = FakeJira::new();
    let camel = start(
        component(&fake),
        &[
            (
                "direct:create",
                "jira:createIssue?project=OPS&issueType=Task",
            ),
            ("direct:get", "jira:getIssue?issueKey=OPS-1"),
            (
                "direct:soft",
                "jira:getIssue?issueKey=OPS-9&throwExceptionOnFailure=false",
            ),
            (
                "direct:small",
                "jira:createIssue?project=OPS&issueType=Task&maxBodySize=10",
            ),
            ("direct:edit", "jira:editIssue?issueKey=OPS-1"),
            ("direct:comment", "jira:addComment?issueKey=OPS-1"),
        ],
    )
    .await
    .unwrap();
    let no_secret = |err: &CamelError| {
        let text = format!("{err} {err:?}");
        assert!(!text.contains(SECRET), "{text}");
        assert!(
            !text.contains(autumn_jira_fake::DEFAULT_API_TOKEN),
            "{text}"
        );
    };

    // A Jira 400 on a bad field: ValidationError with the field name only.
    let err = send(
        &camel,
        "direct:create",
        json(json!({"summary": SECRET, "fields": {"nonsense": SECRET}})),
    )
    .await
    .unwrap_err();
    assert!(validation(&err).contains("jira validation"), "{err}");
    no_secret(&err);
    // A Jira 500: HttpOperationFailed with no response body.
    fake.fault(FakeOp::GetIssue, Fault::Status(500));
    fake.fault(FakeOp::GetIssue, Fault::Status(500));
    let err = send(&camel, "direct:get", Message::default())
        .await
        .unwrap_err();
    match &err {
        CamelError::HttpOperationFailed {
            status_code,
            url,
            response_body,
            ..
        } => {
            assert_eq!(*status_code, 500);
            assert_eq!(url, "jira:getIssue");
            assert_eq!(*response_body, None);
        }
        other => panic!("expected HttpOperationFailed, got {other:?}"),
    }
    no_secret(&err);
    // A bad body: the message names the rule, never the value.
    for body in [
        json!({"summary": [SECRET]}),
        json!({(SECRET): 1, "summary": "x"}),
        json!({"description": SECRET}),
        json!({"summary": "x", "labels": SECRET}),
        json!({"summary": "x", "fields": {(SECRET): 1}}),
        json!(SECRET),
    ] {
        let err = send(&camel, "direct:create", json(body.clone()))
            .await
            .unwrap_err();
        assert!(validation(&err).contains("createIssue"), "{body}: {err}");
        no_secret(&err);
    }
    let err = send(
        &camel,
        "direct:create",
        Message::new(Body::Text(SECRET.into())),
    )
    .await
    .unwrap_err();
    validation(&err);
    no_secret(&err);
    for body in [
        json!({}),
        json!({"setSummary": [SECRET]}),
        json!({(SECRET): 1}),
    ] {
        let err = send(&camel, "direct:edit", json(body.clone()))
            .await
            .unwrap_err();
        assert!(validation(&err).contains("editIssue"), "{body}: {err}");
        no_secret(&err);
    }
    for body in [json!({"text": ""}), json!({"text": [SECRET]}), json!(null)] {
        let err = send(&camel, "direct:comment", json(body.clone()))
            .await
            .unwrap_err();
        assert!(validation(&err).contains("addComment"), "{body}: {err}");
        no_secret(&err);
    }
    // maxBodySize: the limit applies before any call.
    let err = send(
        &camel,
        "direct:small",
        json(json!({"summary": "longer than ten bytes"})),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, CamelError::StreamLimitExceeded(10)),
        "{err:?}"
    );
    assert_eq!(fake.count(FakeOp::CreateIssue), 1, "only the first create");
    // throwExceptionOnFailure=false: the exchange holds the error code.
    let ex = send(&camel, "direct:soft", Message::default())
        .await
        .unwrap();
    let body = reply(&ex);
    assert_eq!(body["error"]["code"], json!("not_found_or_forbidden"));
    assert_eq!(body["error"]["retryable"], json!(false));
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), None);
    stop(camel).await;
}

/// The other component API: `connection` adds a named client, and the URI
/// option picks it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_named_connection_picks_its_client() {
    let default_site = FakeJira::new();
    let eu = FakeJira::new();
    let component = JiraComponent::new(client(&default_site)).connection("eu", client(&eu));
    let camel = start_one(
        component,
        "jira:createIssue?connection=eu&project=OPS&issueType=Task",
    )
    .await
    .unwrap();
    send(&camel, "direct:in", json(json!({"summary": "In the EU"})))
        .await
        .unwrap();
    stop(camel).await;
    assert_eq!(eu.count(FakeOp::CreateIssue), 1);
    assert_eq!(default_site.count(FakeOp::CreateIssue), 0);
    let debug = format!("{:?}", JiraComponent::new(client(&eu)));
    assert!(
        !debug.contains(autumn_jira_fake::DEFAULT_API_TOKEN),
        "{debug}"
    );
}

/// The routes of the idempotency tests.
const MARKED: &[(&str, &str)] = &[
    (
        "direct:create",
        "jira:createIssue?project=OPS&issueType=Task",
    ),
    ("direct:comment", "jira:addComment?issueKey=OPS-1"),
    (
        "direct:strict",
        "jira:createIssue?project=OPS&issueType=Task&requireIdempotencyKey=true",
    ),
    (
        "direct:strict-comment",
        "jira:addComment?issueKey=OPS-1&requireIdempotencyKey=true",
    ),
    (
        "direct:resend",
        "jira:createIssue?project=OPS&issueType=Task&resendAfterNegativeLookup=true",
    ),
    (
        "direct:resend-comment",
        "jira:addComment?issueKey=OPS-1&resendAfterNegativeLookup=true",
    ),
];

/// A message with the harvest-camel headers of one attempt.
fn attempt(body: serde_json::Value, key: &str, attempt: serde_json::Value) -> Message {
    json_with(body, &[(KEY, json!(key)), (ATTEMPT, attempt)])
}

/// The labels of an issue in the fake.
fn labels(fake: &FakeJira, key: &str) -> Vec<String> {
    serde_json::from_value(fake.issue(key).unwrap()["fields"]["labels"].clone()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_header_uses_the_idempotent_path() {
    let fake = FakeJira::new();
    let camel = start(component(&fake), MARKED).await.unwrap();

    // The first attempt creates the issue with the marker label and the
    // marker property.
    let body = json!({"summary": "Disk full", "labels": ["alert"]});
    let ex = send(
        &camel,
        "direct:create",
        attempt(body.clone(), "wf-1/a-1", json!(1)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("created"));
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("created")));
    let marked = labels(&fake, "OPS-1");
    assert!(marked.contains(&"alert".to_owned()), "{marked:?}");
    assert!(
        marked.iter().any(|l| l.starts_with("autumn-idem-")),
        "{marked:?}"
    );
    let bean = fake.issue("OPS-1").unwrap();
    assert_eq!(
        bean["properties"]["autumn-jira"]["idempotencyKey"].as_str(),
        Some(
            autumn_jira::IdempotencyKey::parse("wf-1/a-1")
                .unwrap()
                .as_str()
        )
    );
    // The same key again: found, no second POST. With no attempt header
    // the call counts as a first attempt.
    let ex = send(
        &camel,
        "direct:create",
        json_with(body, &[(KEY, json!("wf-1/a-1"))]),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["key"], json!("OPS-1"));
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(header(&ex, HEADER_OUTCOME), Some(json!("found")));
    assert_eq!(header(&ex, HEADER_ISSUE_KEY), Some(json!("OPS-1")));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);

    // The comment has the marker paragraph. The same key finds it.
    let ex = send(
        &camel,
        "direct:comment",
        attempt(json!("Looking"), "wf-1/a-2", json!(1)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("created"));
    let id = reply(&ex)["id"].clone();
    let ex = send(
        &camel,
        "direct:comment",
        attempt(json!("Looking"), "wf-1/a-2", json!(1)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(reply(&ex)["id"], id);
    assert_eq!(fake.count(FakeOp::AddComment), 1);
    assert!(fake.snapshot().to_string().contains("autumn-ref: "));

    // A lost reply (503 after the commit): the lookup finds the issue, with
    // one POST.
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let ex = send(
        &camel,
        "direct:create",
        attempt(json!({"summary": "Lost"}), "wf-2/a-1", json!(1)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["key"], json!("OPS-2"));
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 2);

    // requireIdempotencyKey: no key header fails with no call.
    let err = send(&camel, "direct:strict", json(json!({"summary": "x"})))
        .await
        .unwrap_err();
    assert!(validation(&err).contains(KEY), "{err}");
    let err = send(&camel, "direct:strict-comment", json(json!("x")))
        .await
        .unwrap_err();
    assert!(validation(&err).contains(KEY), "{err}");
    // A blank key or a key that is not a string fails too.
    for bad in [json!(" "), json!(7), json!(["k"])] {
        let message = json_with(json!({"summary": "x"}), &[(KEY, bad.clone())]);
        let err = send(&camel, "direct:create", message).await.unwrap_err();
        assert!(validation(&err).contains(KEY), "{bad}: {err}");
    }
    assert_eq!(fake.count(FakeOp::CreateIssue), 2);
    assert_eq!(fake.count(FakeOp::AddComment), 1);
    stop(camel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_attempt_looks_up_only() {
    let fake = FakeJira::new();
    let camel = start(component(&fake), MARKED).await.unwrap();
    let body = json!({"summary": "Disk full"});

    // Attempt 2 with a key that made no issue: the lookup finds nothing,
    // so the write fails closed with no POST.
    for n in [json!(2), json!("3")] {
        let err = send(
            &camel,
            "direct:create",
            attempt(body.clone(), "wf-1/a-1", n),
        )
        .await
        .unwrap_err();
        let m = validation(&err);
        assert!(m.starts_with("jira outcome_unknown: "), "{m}");
        assert!(m.contains("resendAfterNegativeLookup"), "{m}");
    }
    assert_eq!(fake.count(FakeOp::CreateIssue), 0);
    assert!(fake.count(FakeOp::Search) >= 2, "a lookup each time");

    // Attempt 1 creates. Attempt 2 finds it with no new POST.
    send(
        &camel,
        "direct:create",
        attempt(body.clone(), "wf-1/a-2", json!(1)),
    )
    .await
    .unwrap();
    let ex = send(
        &camel,
        "direct:create",
        attempt(body.clone(), "wf-1/a-2", json!(2)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["key"], json!("OPS-1"));
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);

    // The same rule for a comment.
    let err = send(
        &camel,
        "direct:comment",
        attempt(json!("x"), "wf-1/a-3", json!(2)),
    )
    .await
    .unwrap_err();
    assert!(
        validation(&err).starts_with("jira outcome_unknown: "),
        "{err}"
    );
    assert_eq!(fake.count(FakeOp::AddComment), 0);
    send(
        &camel,
        "direct:comment",
        attempt(json!("x"), "wf-1/a-4", json!(1)),
    )
    .await
    .unwrap();
    let ex = send(
        &camel,
        "direct:comment",
        attempt(json!("x"), "wf-1/a-4", json!(5)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(fake.count(FakeOp::AddComment), 1);

    // A bad attempt header fails with no call.
    for bad in [json!(0), json!("x"), json!(-1), json!(true)] {
        let err = send(
            &camel,
            "direct:create",
            attempt(body.clone(), "wf-1/a-5", bad.clone()),
        )
        .await
        .unwrap_err();
        assert!(validation(&err).contains(ATTEMPT), "{bad}: {err}");
    }
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);

    // A lost create that the lookups cannot see yet: OutcomeUnknown with
    // the marker maps to Io, so harvest retries. Without a key it maps to
    // ValidationError: harvest does not send a plain write again.
    fake.index_lag(std::time::Duration::from_secs(3600));
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let err = send(
        &camel,
        "direct:create",
        attempt(body.clone(), "wf-1/a-6", json!(1)),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CamelError::Io(_)), "{err:?}");
    assert!(err.to_string().contains("outcome_unknown"), "{err}");
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let err = send(&camel, "direct:create", json(body.clone()))
        .await
        .unwrap_err();
    assert!(validation(&err).contains("outcome_unknown"), "{err}");
    assert_eq!(fake.count(FakeOp::CreateIssue), 3);
    stop(camel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opt_in_resends() {
    let fake = FakeJira::new();
    let camel = start(component(&fake), MARKED).await.unwrap();
    // OPS-1 for the comments.
    send(&camel, "direct:create", json(json!({"summary": "Base"})))
        .await
        .unwrap();

    // Attempt 2 with the opt-in: a negative lookup sends the write.
    let ex = send(
        &camel,
        "direct:resend",
        attempt(json!({"summary": "Again"}), "wf-1/a-1", json!(2)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("created"));
    assert_eq!(reply(&ex)["key"], json!("OPS-2"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 2);
    // The opt-in still finds an issue that exists: no duplicate.
    let ex = send(
        &camel,
        "direct:resend",
        attempt(json!({"summary": "Again"}), "wf-1/a-1", json!(3)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("found"));
    assert_eq!(fake.count(FakeOp::CreateIssue), 2);

    let ex = send(
        &camel,
        "direct:resend-comment",
        attempt(json!({"text": "Again"}), "wf-1/a-2", json!(2)),
    )
    .await
    .unwrap();
    assert_eq!(reply(&ex)["outcome"], json!("created"));
    assert_eq!(fake.count(FakeOp::AddComment), 1);
    stop(camel).await;
}
