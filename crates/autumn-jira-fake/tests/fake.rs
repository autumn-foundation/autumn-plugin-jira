//! Behaviour tests of `FakeJira` (AC22).

use autumn_jira_fake::sign;

#[test]
fn own_hmac_matches_the_vector() {
    // The Atlassian webhook page vector.
    assert_eq!(
        sign(b"It's a Secret to Everybody", b"Hello World!"),
        "sha256=a4771c39fbe90f317c7824e83ddef3caae9cb3d976c214ace1f2937e133263c9"
    );
    // RFC 4231 test case 1: a short key.
    assert_eq!(
        sign(&[0x0b; 20], b"Hi There"),
        "sha256=b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    // RFC 4231 test case 6: a key longer than one block.
    assert_eq!(
        sign(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        ),
        "sha256=60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
    // An empty key and an empty body.
    assert_eq!(
        sign(b"", b""),
        "sha256=b613679a0814d9ec772f95d778c35fc5ff1697c493715653c6c712144292c5ad"
    );
}

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use autumn_jira_fake::{
    ANN, BOT, DEFAULT_CLOUD_ID, FakeError, FakeJira, FakeOp, FakeProject, Fault,
};
use common::{GATEWAY, SITE, api, bearer, call, create, json_of, search, send_with, transition};
use san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use serde_json::{Value, json};

#[test]
fn every_endpoint() {
    let fake = FakeJira::new()
        .with_project(FakeProject::new("WEB", "10100", "Web shop").issue_type("10001", "Task"));

    // Account and site.
    let (status, me) = api(&fake, "GET", "/rest/api/3/myself", None);
    assert_eq!(status, 200);
    assert_eq!(me["accountId"], BOT);
    let (status, info) = api(&fake, "GET", "/rest/api/3/serverInfo", None);
    assert_eq!(status, 200);
    assert_eq!(info["baseUrl"], SITE);
    assert_eq!(info["deploymentType"], "Cloud");

    // Projects.
    let (status, ops) = api(&fake, "GET", "/rest/api/3/project/OPS", None);
    assert_eq!(status, 200);
    assert_eq!(ops["key"], "OPS");
    assert_eq!(ops["id"], "10000");
    assert_eq!(ops["issueTypes"].as_array().unwrap().len(), 3);
    assert!(ops.get("lead").is_none(), "lead needs expand=lead");
    let (_, ops) = api(&fake, "GET", "/rest/api/3/project/OPS?expand=lead", None);
    assert_eq!(ops["lead"]["accountId"], ANN);
    let (_, by_id) = api(&fake, "GET", "/rest/api/3/project/10100", None);
    assert_eq!(by_id["key"], "WEB");
    let (status, missing) = api(&fake, "GET", "/rest/api/3/project/NOPE", None);
    assert_eq!(status, 404);
    assert!(missing["errorMessages"][0].is_string());

    let (status, page) = api(
        &fake,
        "GET",
        "/rest/api/3/project/search?maxResults=1",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(page["values"].as_array().unwrap().len(), 1);
    assert_eq!(page["values"][0]["key"], "OPS");
    assert_eq!(page["total"], 2);
    assert_eq!(page["isLast"], false);
    assert!(page["nextPage"].is_string());
    let (_, page) = api(
        &fake,
        "GET",
        "/rest/api/3/project/search?startAt=1&maxResults=1",
        None,
    );
    assert_eq!(page["values"][0]["key"], "WEB");
    assert_eq!(page["startAt"], 1);
    assert_eq!(page["isLast"], true);
    assert!(page.get("nextPage").is_none());

    // Issue types for create.
    let (status, meta) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/createmeta/OPS/issuetypes?maxResults=2",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(meta["total"], 3);
    let names: Vec<&str> = meta["issueTypes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Task", "Incident"]);
    let (_, meta) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/createmeta/OPS/issuetypes?startAt=2&maxResults=2",
        None,
    );
    assert_eq!(meta["issueTypes"][0]["id"], "10003");
    let (status, _) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/createmeta/NOPE/issuetypes",
        None,
    );
    assert_eq!(status, 400);

    // Create.
    let (status, created) = api(
        &fake,
        "POST",
        "/rest/api/3/issue",
        Some(json!({"fields": {
            "project": {"key": "OPS"},
            "issuetype": {"name": "Incident"},
            "summary": "Disk full",
            "labels": ["ops"],
            "description": {"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "Body"}]}]},
            "customfield_10010": "kept",
        }, "properties": [{"key": "p1", "value": {"a": 1}}]})),
    );
    assert_eq!(status, 201);
    assert_eq!(created["key"], "OPS-1");
    assert_eq!(created["id"], "10000");
    assert_eq!(created["self"], format!("{SITE}/rest/api/3/issue/10000"));
    assert_eq!(create(&fake, "OPS", "Second", &[]).0, "OPS-2");
    assert_eq!(create(&fake, "WEB", "Web one", &[]).0, "WEB-1");

    // Create validation.
    let bad = |fields: Value| {
        api(
            &fake,
            "POST",
            "/rest/api/3/issue",
            Some(json!({"fields": fields})),
        )
    };
    let (status, err) = bad(json!({"project": {"key": "OPS"}, "issuetype": {"id": "10001"}}));
    assert_eq!(status, 400);
    assert!(err["errors"]["summary"].is_string());
    let (status, err) =
        bad(json!({"project": {"key": "NOPE"}, "issuetype": {"id": "10001"}, "summary": "x"}));
    assert_eq!(status, 400);
    assert!(err["errors"]["project"].is_string());
    let (status, err) =
        bad(json!({"project": {"key": "OPS"}, "issuetype": {"name": "Epic"}, "summary": "x"}));
    assert_eq!(status, 400);
    assert!(err["errors"]["issuetype"].is_string());
    let (status, err) = bad(
        json!({"project": {"id": "10000"}, "issuetype": {"id": "10001"}, "summary": "x", "colour": "red"}),
    );
    assert_eq!(status, 400);
    assert!(err["errors"]["colour"].is_string());
    let (status, err) = bad(
        json!({"project": {"key": "OPS"}, "issuetype": {"id": "10001"}, "summary": "x", "labels": ["two words"]}),
    );
    assert_eq!(status, 400);
    assert!(err["errors"]["labels"].is_string());
    let (status, _) = api(
        &fake,
        "POST",
        "/rest/api/3/issue",
        Some(json!("not an object")),
    );
    assert_eq!(status, 400);

    // Get.
    let (status, issue) = api(&fake, "GET", "/rest/api/3/issue/OPS-1", None);
    assert_eq!(status, 200);
    assert_eq!(issue["key"], "OPS-1");
    assert_eq!(issue["fields"]["summary"], "Disk full");
    assert_eq!(issue["fields"]["status"]["name"], "To Do");
    assert_eq!(issue["fields"]["status"]["statusCategory"]["key"], "new");
    assert_eq!(issue["fields"]["issuetype"]["name"], "Incident");
    assert_eq!(issue["fields"]["project"]["key"], "OPS");
    assert_eq!(issue["fields"]["labels"], json!(["ops"]));
    assert_eq!(issue["fields"]["reporter"]["accountId"], BOT);
    assert_eq!(issue["fields"]["customfield_10010"], "kept");
    assert!(issue["fields"]["assignee"].is_null());
    let created_at = issue["fields"]["created"].as_str().unwrap();
    assert!(created_at.ends_with("+0000"), "{created_at}");
    assert_eq!(created_at.len(), "2026-11-02T10:00:00.000+0000".len());
    assert!(issue.get("properties").is_none());
    let (_, by_id) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/10000?fields=summary,labels&properties=p1",
        None,
    );
    let mut keys: Vec<&String> = by_id["fields"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["labels", "summary"]);
    assert_eq!(by_id["properties"]["p1"], json!({"a": 1}));
    let (_, lower) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/ops-1?fields=-comment,-description",
        None,
    );
    assert_eq!(lower["key"], "OPS-1");
    assert!(lower["fields"].get("comment").is_none());
    assert!(lower["fields"].get("summary").is_some());
    let (_, expanded) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/OPS-1?expand=transitions",
        None,
    );
    assert_eq!(expanded["transitions"][0]["id"], "21");
    let (status, missing) = api(&fake, "GET", "/rest/api/3/issue/OPS-99", None);
    assert_eq!(status, 404);
    assert!(missing["errorMessages"][0].is_string());

    // Edit.
    let (status, _) = api(
        &fake,
        "PUT",
        "/rest/api/3/issue/OPS-1",
        Some(json!({
            "fields": {"summary": "Disk almost full", "assignee": {"accountId": ANN}},
            "update": {"labels": [{"add": "sev1"}, {"remove": "ops"}]},
        })),
    );
    assert_eq!(status, 204);
    let (_, issue) = api(&fake, "GET", "/rest/api/3/issue/OPS-1", None);
    assert_eq!(issue["fields"]["summary"], "Disk almost full");
    assert_eq!(issue["fields"]["labels"], json!(["sev1"]));
    assert_eq!(issue["fields"]["assignee"]["accountId"], ANN);
    let (status, returned) = api(
        &fake,
        "PUT",
        "/rest/api/3/issue/OPS-1?returnIssue=true&notifyUsers=false",
        Some(
            json!({"update": {"summary": [{"set": "Disk full again"}], "labels": [{"set": ["a", "b"]}]}}),
        ),
    );
    assert_eq!(status, 200);
    assert_eq!(returned["fields"]["summary"], "Disk full again");
    assert_eq!(returned["fields"]["labels"], json!(["a", "b"]));
    let (status, err) = api(
        &fake,
        "PUT",
        "/rest/api/3/issue/OPS-1",
        Some(json!({"fields": {"summary": "x"}, "update": {"summary": [{"set": "y"}]}})),
    );
    assert_eq!(status, 400);
    assert!(err["errors"]["summary"].is_string());
    let (status, _) = api(
        &fake,
        "PUT",
        "/rest/api/3/issue/OPS-1",
        Some(json!({"fields": {"status": {"name": "Done"}}})),
    );
    assert_eq!(status, 400);
    let (status, _) = api(
        &fake,
        "PUT",
        "/rest/api/3/issue/OPS-99",
        Some(json!({"fields": {}})),
    );
    assert_eq!(status, 404);

    // Properties.
    let path = "/rest/api/3/issue/OPS-1/properties/autumn-jira";
    assert_eq!(api(&fake, "GET", path, None).0, 404);
    assert_eq!(
        api(&fake, "PUT", path, Some(json!({"idempotencyKey": "k1"}))).0,
        201
    );
    assert_eq!(
        api(&fake, "PUT", path, Some(json!({"idempotencyKey": "k2"}))).0,
        200
    );
    let (status, prop) = api(&fake, "GET", path, None);
    assert_eq!(status, 200);
    assert_eq!(
        prop,
        json!({"key": "autumn-jira", "value": {"idempotencyKey": "k2"}})
    );
    let encoded = "/rest/api/3/issue/OPS-1/properties/my%20key";
    assert_eq!(api(&fake, "PUT", encoded, Some(json!(true))).0, 201);
    assert_eq!(api(&fake, "GET", encoded, None).1["key"], "my key");
    let raw = call(
        &fake,
        "PUT",
        &format!("{SITE}{path}"),
        Some(b"{not json".to_vec()),
    );
    assert_eq!(raw.status, 400);
    assert_eq!(
        api(
            &fake,
            "PUT",
            "/rest/api/3/issue/OPS-99/properties/x",
            Some(json!(1))
        )
        .0,
        404
    );

    // Transitions.
    let (status, list) = api(&fake, "GET", "/rest/api/3/issue/OPS-1/transitions", None);
    assert_eq!(status, 200);
    assert_eq!(list["transitions"][0]["id"], "21");
    assert_eq!(list["transitions"][0]["to"]["name"], "In Progress");
    assert_eq!(transition(&fake, "OPS-1", "21", None), 204);
    let (_, issue) = api(&fake, "GET", "/rest/api/3/issue/OPS-1?fields=status", None);
    assert_eq!(issue["fields"]["status"]["name"], "In Progress");

    // Comments.
    for text in ["first", "second", "third"] {
        let (status, comment) = api(
            &fake,
            "POST",
            "/rest/api/3/issue/OPS-1/comment",
            Some(json!({"body": {"type": "doc", "version": 1, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": text}]}]}})),
        );
        assert_eq!(status, 201);
        assert_eq!(comment["author"]["accountId"], BOT);
        assert!(comment["id"].is_string());
    }
    let texts = |page: &Value| -> Vec<String> {
        page["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                c["body"]["content"][0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    };
    let (status, page) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/OPS-1/comment?orderBy=created",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(texts(&page), ["first", "second", "third"]);
    assert_eq!(page["total"], 3);
    let (_, page) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/OPS-1/comment?orderBy=-created&maxResults=2",
        None,
    );
    assert_eq!(texts(&page), ["third", "second"]);
    assert_eq!(page["maxResults"], 2);
    let (_, page) = api(
        &fake,
        "GET",
        "/rest/api/3/issue/OPS-1/comment?orderBy=%2Bcreated&startAt=2",
        None,
    );
    assert_eq!(texts(&page), ["third"]);
    assert_eq!(page["startAt"], 2);
    assert_eq!(
        api(
            &fake,
            "GET",
            "/rest/api/3/issue/OPS-1/comment?orderBy=updated",
            None
        )
        .0,
        400
    );
    assert_eq!(
        api(
            &fake,
            "POST",
            "/rest/api/3/issue/OPS-1/comment",
            Some(json!({"body": "plain"}))
        )
        .0,
        400
    );
    assert_eq!(
        api(&fake, "GET", "/rest/api/3/issue/OPS-99/comment", None).0,
        404
    );

    // Search and count.
    let (status, page) = search(
        &fake,
        json!({"jql": "project = OPS", "fields": ["summary"]}),
    );
    assert_eq!(status, 200);
    assert_eq!(page["issues"].as_array().unwrap().len(), 2);
    assert_eq!(page["isLast"], true);
    assert!(page["issues"][0]["fields"]["summary"].is_string());
    let (status, page) = api(
        &fake,
        "GET",
        "/rest/api/3/search/jql?jql=project%20%3D%20WEB&fields=summary,status",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(page["issues"][0]["key"], "WEB-1");
    assert!(page["issues"][0]["fields"]["status"].is_object());
    let (_, ids_only) = search(&fake, json!({"jql": "project = WEB"}));
    assert_eq!(ids_only["issues"][0]["fields"], json!({}));
    let (status, count) = api(
        &fake,
        "POST",
        "/rest/api/3/search/approximate-count",
        Some(json!({"jql": "project = OPS"})),
    );
    assert_eq!(status, 200);
    assert_eq!(count, json!({"count": 2}));
    assert_eq!(
        api(
            &fake,
            "POST",
            "/rest/api/3/search/approximate-count",
            Some(json!({"jql": ""}))
        )
        .0,
        400
    );
    assert_eq!(
        api(&fake, "GET", "/rest/api/3/search?jql=project%3DOPS", None).0,
        410
    );
    let (status, gone) = api(
        &fake,
        "POST",
        "/rest/api/3/search",
        Some(json!({"jql": "project = OPS"})),
    );
    assert_eq!(status, 410);
    assert!(
        gone["errorMessages"][0]
            .as_str()
            .unwrap()
            .contains("/rest/api/3/search/jql")
    );

    // Delete.
    assert_eq!(
        api(
            &fake,
            "DELETE",
            "/rest/api/3/issue/OPS-2?deleteSubtasks=maybe",
            None
        )
        .0,
        400
    );
    assert_eq!(
        api(
            &fake,
            "DELETE",
            "/rest/api/3/issue/OPS-2?deleteSubtasks=false",
            None
        )
        .0,
        204
    );
    assert_eq!(api(&fake, "GET", "/rest/api/3/issue/OPS-2", None).0, 404);
    assert_eq!(api(&fake, "DELETE", "/rest/api/3/issue/OPS-2", None).0, 404);
    assert_eq!(
        create(&fake, "OPS", "After delete", &[]).0,
        "OPS-3",
        "keys are never reused"
    );

    // Labels that are not on the create screen.
    fake.labels_not_on_screen("OPS");
    let (status, err) = api(
        &fake,
        "POST",
        "/rest/api/3/issue",
        Some(
            json!({"fields": {"project": {"key": "OPS"}, "issuetype": {"id": "10001"},
            "summary": "x", "labels": ["autumn-idem-abc"]}}),
        ),
    );
    assert_eq!(status, 400);
    assert!(err["errors"]["labels"].is_string());
    assert_eq!(create(&fake, "OPS", "No labels", &[]).0, "OPS-4");
    assert_eq!(create(&fake, "WEB", "Other project", &["fine"]).0, "WEB-2");

    // The gateway host serves the same site.
    let reply = call(
        &fake,
        "GET",
        &format!("{GATEWAY}/rest/api/3/issue/OPS-1"),
        None,
    );
    assert_eq!(reply.status, 200);
    assert_eq!(json_of(&reply)["key"], "OPS-1");

    // Unknown paths and methods.
    assert_eq!(api(&fake, "GET", "/rest/api/3/nothing/here", None).0, 404);
    assert_eq!(api(&fake, "PATCH", "/rest/api/3/issue/OPS-1", None).0, 405);
    assert_eq!(
        call(&fake, "GET", &format!("{SITE}/wiki/home"), None).status,
        404
    );

    // Counts and the request log.
    assert_eq!(fake.count(FakeOp::Myself), 1);
    assert_eq!(fake.count(FakeOp::CreateIssue), 13);
    assert_eq!(fake.count(FakeOp::Count), 2);
    let requests = fake.requests();
    let first = &requests[0];
    assert_eq!(first.method, "GET");
    assert_eq!(first.host, "acme.atlassian.net");
    assert_eq!(first.path, "/rest/api/3/myself");
    assert_eq!(first.query, "");
    assert!(requests.iter().any(|r| r.query == "maxResults=1"));
    let log = format!("{requests:?}");
    assert!(!log.contains("Basic "), "the request log holds no auth");
}

#[test]
fn jql_subset() {
    let fake = FakeJira::new();
    let a = create(&fake, "OPS", "one", &["a", "x"]).0;
    let b = create(&fake, "OPS", "two", &["b"]).0;
    let c = create(&fake, "OPS", "three", &[]).0;
    let odd = create(&fake, "OPS", "odd", &["q\"\\b"]).0;
    assert_eq!(transition(&fake, &b, "21", None), 204);
    assert_eq!(
        transition(
            &fake,
            &b,
            "31",
            Some(json!({"resolution": {"name": "Done"}}))
        ),
        204
    );
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{c}"),
            Some(json!({"fields": {"assignee": {"accountId": BOT}}}))
        )
        .0,
        204
    );
    let (_, bug) = api(
        &fake,
        "POST",
        "/rest/api/3/issue",
        Some(
            json!({"fields": {"project": {"key": "OPS"}, "issuetype": {"name": "Bug"}, "summary": "bug"}}),
        ),
    );
    let bug = bug["key"].as_str().unwrap().to_owned();

    let keys = |jql: &str| -> Vec<String> {
        let (status, page) = search(&fake, json!({"jql": jql, "maxResults": 100}));
        assert_eq!(status, 200, "{jql}: {page}");
        page["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };

    assert_eq!(
        keys("project = OPS ORDER BY key ASC"),
        [
            a.as_str(),
            b.as_str(),
            c.as_str(),
            odd.as_str(),
            bug.as_str()
        ]
    );
    assert_eq!(
        keys("project = \"OPS\" order by key desc"),
        [
            bug.as_str(),
            odd.as_str(),
            c.as_str(),
            b.as_str(),
            a.as_str()
        ]
    );
    assert_eq!(keys("project = 10000 AND labels = \"a\""), [a.as_str()]);
    assert_eq!(keys("project = Operations and LABELS = x"), [a.as_str()]);
    assert_eq!(
        sorted(keys("project = OPS AND labels != \"a\"")),
        [b.as_str(), odd.as_str()]
    );
    assert_eq!(
        keys("project = OPS AND labels = \"q\\\"\\\\b\""),
        [odd.as_str()]
    );
    assert_eq!(
        keys("project = OPS AND labels IN (\"b\", \"x\") ORDER BY key"),
        [a.as_str(), b.as_str()]
    );
    assert_eq!(keys("issuetype in (\"Bug\", Incident)"), [bug.as_str()]);
    assert_eq!(keys("project = OPS AND issuetype = 10003"), [bug.as_str()]);
    assert_eq!(keys("status = Done"), [b.as_str()]);
    assert_eq!(keys("project = OPS AND status = \"10002\""), [b.as_str()]);
    assert_eq!(keys("statusCategory = done"), [b.as_str()]);
    assert_eq!(
        sorted(keys("project = OPS AND statusCategory != \"Done\"")),
        [a.as_str(), c.as_str(), odd.as_str(), bug.as_str()]
    );
    assert_eq!(keys("statusCategory = 3"), [b.as_str()]);
    assert_eq!(keys(&format!("key = {}", a.to_lowercase())), [a.as_str()]);
    assert_eq!(
        keys(&format!("issuekey in ({a}, {c}) ORDER BY key ASC")),
        [a.as_str(), c.as_str()]
    );
    assert_eq!(keys("key = 10001"), [b.as_str()]);
    assert_eq!(keys(&format!("assignee = \"{BOT}\"")), [c.as_str()]);
    assert_eq!(keys("assignee = currentUser()"), [c.as_str()]);
    assert_eq!(
        keys("project = OPS AND assignee != currentUser()"),
        Vec::<String>::new()
    );
    assert_eq!(
        sorted(keys("project = OPS AND assignee is EMPTY")),
        [a.as_str(), b.as_str(), odd.as_str(), bug.as_str()]
    );
    assert_eq!(
        keys("project = OPS AND assignee IS NOT EMPTY"),
        [c.as_str()]
    );
    assert_eq!(
        keys("project = OPS AND labels is empty"),
        [bug.as_str(), c.as_str()]
    );
    assert_eq!(
        keys("project = OPS AND labels NOT IN (a, b) AND labels is not empty"),
        [odd.as_str()]
    );
    assert_eq!(
        keys("project = OPS ORDER BY created ASC, key DESC").len(),
        5
    );
    assert_eq!(keys("project = OPS ORDER BY updated DESC, status").len(), 5);

    // The default order is newest first.
    assert_eq!(keys("project = OPS")[0], bug);

    // Errors: unbounded, unknown field, syntax, unknown project.
    let error = |jql: &str| -> String {
        let (status, body) = search(&fake, json!({"jql": jql}));
        assert_eq!(status, 400, "{jql}");
        assert!(body["errors"].is_object(), "{jql}: {body}");
        body["errorMessages"][0].as_str().unwrap().to_owned()
    };
    assert!(error("").contains("Unbounded"));
    assert!(error("ORDER BY key DESC").contains("Unbounded"));
    assert!(error("foo = 1").contains("Field 'foo' does not exist"));
    assert!(error("cf[10010] = 1").contains("does not exist"));
    assert!(error("summary ~ x").contains("does not exist"));
    assert!(error("project = OPS ORDER BY colour").contains("colour"));
    assert!(!error("project =").is_empty());
    assert!(!error("project = OPS OR labels = a").is_empty());
    assert!(!error("project = \"OPS").is_empty());
    assert!(!error("project in (OPS").is_empty());
    assert!(!error("project = OPS AND").is_empty());
    assert!(!error("project = OPS extra").is_empty());
    assert!(error("project = NOPE").contains("NOPE"));

    // Token pages.
    let (_, first) = search(
        &fake,
        json!({"jql": "project = OPS ORDER BY key ASC", "maxResults": 2}),
    );
    assert_eq!(first["issues"].as_array().unwrap().len(), 2);
    assert_eq!(first["isLast"], false);
    let token = first["nextPageToken"].as_str().unwrap().to_owned();
    let (_, second) = search(
        &fake,
        json!({"jql": "project = OPS ORDER BY key ASC", "maxResults": 2, "nextPageToken": token}),
    );
    assert_eq!(second["issues"][0]["key"], c.as_str());
    let token = second["nextPageToken"].as_str().unwrap().to_owned();
    let (_, last) = search(
        &fake,
        json!({"jql": "project = OPS ORDER BY key ASC", "maxResults": 2, "nextPageToken": token}),
    );
    assert_eq!(last["issues"].as_array().unwrap().len(), 1);
    assert_eq!(last["isLast"], true);
    assert!(last.get("nextPageToken").is_none());
    let (status, _) = search(
        &fake,
        json!({"jql": "project = OPS", "nextPageToken": "forged"}),
    );
    assert_eq!(status, 400);
    let (status, _) = search(
        &fake,
        json!({"jql": "status = Done", "nextPageToken": first["nextPageToken"]}),
    );
    assert_eq!(status, 400, "a token belongs to one query");
    let (status, page) = api(
        &fake,
        "GET",
        "/rest/api/3/search/jql?jql=project%3DOPS&maxResults=1&fields=summary",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(page["isLast"], false);

    // Issue properties in the search reply.
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{a}/properties/autumn-jira"),
            Some(json!({"k": 1}))
        )
        .0,
        201
    );
    let (_, page) = search(
        &fake,
        json!({"jql": "labels = a", "fields": ["summary"], "properties": ["autumn-jira", "other"]}),
    );
    assert_eq!(
        page["issues"][0]["properties"],
        json!({"autumn-jira": {"k": 1}})
    );
}

#[test]
fn auth_rules() {
    let fake = FakeJira::new().with_oauth("cid", "csecret", "at-0", "rt-0");
    let url = format!("{SITE}/rest/api/3/myself");

    let anonymous = fake.handle(&HttpRequest::new("GET", url.clone())).unwrap();
    assert_eq!(anonymous.status, 401);
    assert!(json_of(&anonymous)["errorMessages"][0].is_string());
    let wrong = HttpRequest::new("GET", url.clone()).header(
        "authorization",
        common::basic_header("bot@acme.example", "wrong"),
    );
    assert_eq!(fake.handle(&wrong).unwrap().status, 401);
    let junk = HttpRequest::new("GET", url.clone()).header("authorization", "Basic !!!");
    assert_eq!(fake.handle(&junk).unwrap().status, 401);
    let no_colon = HttpRequest::new("GET", url.clone()).header("authorization", "Basic Ym90");
    assert_eq!(fake.handle(&no_colon).unwrap().status, 401);
    assert_eq!(call(&fake, "GET", &url, None).status, 200);

    // Bearer tokens work on the gateway only.
    assert_eq!(
        fake.handle(&bearer("GET", &url, "at-0")).unwrap().status,
        401
    );
    let gw = format!("{GATEWAY}/rest/api/3/myself");
    assert_eq!(
        fake.handle(&bearer("GET", &gw, "at-0")).unwrap().status,
        200
    );
    assert_eq!(
        fake.handle(&bearer("GET", &gw, "at-9")).unwrap().status,
        401
    );
    assert_eq!(
        call(&fake, "GET", &gw, None).status,
        200,
        "a scoped API token"
    );
    let other_scheme = HttpRequest::new("GET", gw.clone()).header("authorization", "Digest x");
    assert_eq!(fake.handle(&other_scheme).unwrap().status, 401);

    // An expired access token gives 401.
    fake.expire_access_token();
    assert_eq!(
        fake.handle(&bearer("GET", &gw, "at-0")).unwrap().status,
        401
    );
    assert_eq!(
        call(&fake, "GET", &gw, None).status,
        200,
        "Basic does not expire"
    );

    // New Basic credentials replace the old ones.
    let fake = fake.with_basic_auth("me@example.com", "tok");
    assert_eq!(call(&fake, "GET", &url, None).status, 401);
    let mine = HttpRequest::new("GET", url.clone()).header(
        "authorization",
        common::basic_header("me@example.com", "tok"),
    );
    assert_eq!(fake.handle(&mine).unwrap().status, 200);

    // A 401 consumes no fault and still counts as a call.
    fake.fault(FakeOp::Myself, Fault::Status(500));
    assert_eq!(call(&fake, "GET", &url, None).status, 401);
    assert_eq!(fake.handle(&mine).unwrap().status, 500);
    assert_eq!(fake.count(FakeOp::Myself), 16);

    // Without OAuth, a Bearer token never works.
    let plain = FakeJira::new();
    assert_eq!(
        plain.handle(&bearer("GET", &gw, "at-0")).unwrap().status,
        401
    );

    // A request that is not https is not valid.
    assert!(matches!(
        fake.handle(&HttpRequest::new(
            "GET",
            "acme.atlassian.net/rest/api/3/myself"
        )),
        Err(TransportError::Invalid(_))
    ));
}

#[test]
fn oauth_refresh_rotates() {
    let fake = FakeJira::new().with_oauth("cid", "csecret", "at-0", "rt-0");
    let token_url = "https://auth.atlassian.com/oauth/token";
    let refresh_json = |refresh: &str, secret: &str| {
        let body = json!({"grant_type": "refresh_token", "client_id": "cid",
            "client_secret": secret, "refresh_token": refresh});
        HttpRequest::new("POST", token_url)
            .header("content-type", "application/json")
            .body(body.to_string().into_bytes())
    };

    let reply = fake.handle(&refresh_json("rt-0", "csecret")).unwrap();
    assert_eq!(reply.status, 200);
    let tokens = json_of(&reply);
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["token_type"], "Bearer");
    assert!(tokens["scope"].as_str().unwrap().contains("read:jira-work"));
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(access, "at-0");
    assert_ne!(refresh, "rt-0");
    assert_eq!(fake.token_refreshes(), 1);

    // The new access token works. The old refresh token does not.
    let gw = format!("{GATEWAY}/rest/api/3/myself");
    assert_eq!(
        fake.handle(&bearer("GET", &gw, &access)).unwrap().status,
        200
    );
    let reused = fake.handle(&refresh_json("rt-0", "csecret")).unwrap();
    assert_eq!(reused.status, 403);
    assert_eq!(json_of(&reused)["error"], "invalid_grant");
    let bad_client = fake.handle(&refresh_json(&refresh, "nope")).unwrap();
    assert_eq!(bad_client.status, 401);
    assert_eq!(json_of(&bad_client)["error"], "invalid_client");
    assert_eq!(fake.token_refreshes(), 1);

    // A form body with client credentials in Basic auth also works.
    let form = format!(
        "grant_type=refresh_token&refresh_token={}",
        refresh.replace('-', "%2D")
    );
    let basic = HttpRequest::new("POST", token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", common::basic_header("cid", "csecret"))
        .body(form.into_bytes());
    let reply = fake.handle(&basic).unwrap();
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let third = json_of(&reply);
    assert_ne!(third["refresh_token"].as_str().unwrap(), refresh);
    assert_eq!(fake.token_refreshes(), 2);

    // After expiry, a refresh gives a working token again.
    fake.expire_access_token();
    assert_eq!(
        fake.handle(&bearer("GET", &gw, &access)).unwrap().status,
        401
    );
    let next = third["refresh_token"].as_str().unwrap();
    let reply = json_of(&fake.handle(&refresh_json(next, "csecret")).unwrap());
    let fresh = reply["access_token"].as_str().unwrap();
    assert_eq!(fake.handle(&bearer("GET", &gw, fresh)).unwrap().status, 200);
    assert_eq!(fake.token_refreshes(), 3);

    // Other grants and bad bodies.
    let code = HttpRequest::new("POST", token_url)
        .header("content-type", "application/json")
        .body(
            json!({"grant_type": "authorization_code", "client_id": "cid",
            "client_secret": "csecret", "code": "c"})
            .to_string()
            .into_bytes(),
        );
    let reply = fake.handle(&code).unwrap();
    assert_eq!(reply.status, 400);
    assert_eq!(json_of(&reply)["error"], "unsupported_grant_type");
    let get = HttpRequest::new("GET", token_url);
    assert_eq!(fake.handle(&get).unwrap().status, 405);
    let empty = HttpRequest::new("POST", token_url);
    assert_eq!(fake.handle(&empty).unwrap().status, 401);
    assert_eq!(fake.count(FakeOp::Token), 8);

    // A fake without OAuth refuses every client.
    let plain = FakeJira::new();
    assert_eq!(
        plain
            .handle(&refresh_json("rt-0", "csecret"))
            .unwrap()
            .status,
        401
    );
}

#[test]
fn accessible_resources() {
    let fake = FakeJira::new()
        .with_site("beta", "22222222-3333-4444-8555-666666666666")
        .with_oauth("cid", "csecret", "at-0", "rt-0");
    let url = "https://api.atlassian.com/oauth/token/accessible-resources";
    let reply = fake.handle(&bearer("GET", url, "at-0")).unwrap();
    assert_eq!(reply.status, 200);
    let list = json_of(&reply);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    let jira = &list[0];
    assert_eq!(jira["id"], "22222222-3333-4444-8555-666666666666");
    assert_eq!(jira["url"], "https://beta.atlassian.net");
    assert_eq!(jira["name"], "beta");
    let scopes: Vec<&str> = jira["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    assert!(scopes.contains(&"read:jira-work"));
    assert!(scopes.contains(&"write:jira-work"));
    assert!(jira["avatarUrl"].is_string());
    let wiki = &list[1];
    assert_ne!(wiki["id"], jira["id"]);
    assert!(
        wiki["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| !s.as_str().unwrap().contains(":jira-")),
        "the second entry is Confluence only"
    );

    // Basic auth, a wrong token and an expired token give 401.
    assert_eq!(call(&fake, "GET", url, None).status, 401);
    assert_eq!(
        fake.handle(&bearer("GET", url, "at-1")).unwrap().status,
        401
    );
    fake.expire_access_token();
    assert_eq!(
        fake.handle(&bearer("GET", url, "at-0")).unwrap().status,
        401
    );
    assert_eq!(
        fake.handle(&bearer("POST", url, "at-0")).unwrap().status,
        405
    );
    assert_eq!(fake.count(FakeOp::AccessibleResources), 5);

    // The site follows `with_site`.
    let gw = "https://api.atlassian.com/ex/jira/22222222-3333-4444-8555-666666666666/rest/api/3/serverInfo";
    let reply = call(&fake, "GET", gw, None);
    assert_eq!(json_of(&reply)["baseUrl"], "https://beta.atlassian.net");
    let old = format!("{GATEWAY}/rest/api/3/serverInfo");
    assert_eq!(call(&fake, "GET", &old, None).status, 404);
    assert!(matches!(
        fake.handle(&HttpRequest::new(
            "GET",
            format!("{SITE}/rest/api/3/myself")
        )),
        Err(TransportError::Connect(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn faults_table() {
    struct Case {
        fault: Fault,
        status: Option<u16>,
        error: Option<TransportError>,
        written: bool,
    }
    let connect = Some(TransportError::Connect(String::new()));
    let cases = [
        Case {
            fault: Fault::Status(503),
            status: Some(503),
            error: None,
            written: false,
        },
        Case {
            fault: Fault::Status(400),
            status: Some(400),
            error: None,
            written: false,
        },
        Case {
            fault: Fault::StatusAfterCommit(502),
            status: Some(502),
            error: None,
            written: true,
        },
        Case {
            fault: Fault::Network,
            status: None,
            error: connect.clone(),
            written: false,
        },
        Case {
            fault: Fault::NetworkAfterCommit,
            status: None,
            error: connect,
            written: true,
        },
        Case {
            fault: Fault::Timeout,
            status: None,
            error: Some(TransportError::Timeout),
            written: false,
        },
        Case {
            fault: Fault::Redirect,
            status: Some(302),
            error: None,
            written: false,
        },
        Case {
            fault: Fault::BadJson,
            status: Some(201),
            error: None,
            written: true,
        },
    ];
    for case in cases {
        let fake = FakeJira::new();
        fake.fault(FakeOp::CreateIssue, case.fault.clone());
        let reply = send_with(&fake, common::create_request("OPS", "faulted")).await;
        match (&reply, &case.error) {
            (Ok(r), None) => assert_eq!(Some(r.status), case.status, "{:?}", case.fault),
            (Err(e), Some(want)) => assert_eq!(
                std::mem::discriminant(e),
                std::mem::discriminant(want),
                "{:?}",
                case.fault
            ),
            other => panic!("{:?}: {other:?}", case.fault),
        }
        assert_eq!(
            fake.issues().len(),
            usize::from(case.written),
            "{:?}",
            case.fault
        );
        if case.fault == Fault::Redirect {
            let r = reply.as_ref().unwrap();
            assert_eq!(r.header_value("location"), Some("https://evil.example/"));
        }
        if case.fault == Fault::BadJson {
            let r = reply.as_ref().unwrap();
            assert!(serde_json::from_slice::<Value>(&r.body).is_err());
        }
        if case.status.is_some_and(|s| s >= 400) {
            let r = send_with(&fake, common::create_request("OPS", "again"))
                .await
                .unwrap();
            assert_eq!(r.status, 201, "a fault fires once");
        }
    }

    // The write happens after the delay, in tokio time.
    let fake = FakeJira::new();
    fake.fault(
        FakeOp::CreateIssue,
        Fault::TimeoutThenCommit(Duration::from_secs(5)),
    );
    let reply = send_with(&fake, common::create_request("OPS", "late")).await;
    assert_eq!(reply, Err(TransportError::Timeout));
    assert!(fake.issues().is_empty());
    tokio::time::advance(Duration::from_secs(4)).await;
    assert!(fake.issue("OPS-1").is_none());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(fake.issue("OPS-1").unwrap()["fields"]["summary"], "late");

    // A bad JSON body on a read.
    let fake = FakeJira::new();
    let key = create(&fake, "OPS", "one", &[]).0;
    fake.fault(FakeOp::GetIssue, Fault::BadJson);
    let reply = call(
        &fake,
        "GET",
        &format!("{SITE}/rest/api/3/issue/{key}"),
        None,
    );
    assert_eq!(reply.status, 200);
    assert!(serde_json::from_slice::<Value>(&reply.body).is_err());
    fake.fault(FakeOp::EditIssue, Fault::BadJson);
    let edit = api(
        &fake,
        "PUT",
        &format!("/rest/api/3/issue/{key}"),
        Some(json!({"fields": {"summary": "two"}})),
    );
    assert_eq!(edit.0, 200);
    assert_eq!(fake.issue(&key).unwrap()["fields"]["summary"], "two");

    // Faults are FIFO for each op and do not touch other ops.
    fake.fault(FakeOp::GetIssue, Fault::Status(500));
    fake.fault(FakeOp::GetIssue, Fault::Status(502));
    assert_eq!(api(&fake, "GET", "/rest/api/3/myself", None).0, 200);
    let url = format!("{SITE}/rest/api/3/issue/{key}");
    assert_eq!(call(&fake, "GET", &url, None).status, 500);
    assert_eq!(call(&fake, "GET", &url, None).status, 502);
    assert_eq!(call(&fake, "GET", &url, None).status, 200);

    // A rate limit gives 429 with Retry-After, then the normal reply.
    fake.rate_limit(FakeOp::Search, 2, 7);
    for _ in 0..2 {
        let (status, body, headers) = common::search_raw(&fake, json!({"jql": "project = OPS"}));
        assert_eq!(status, 429);
        assert_eq!(common::header(&headers, "retry-after"), Some("7"));
        assert!(common::header(&headers, "ratelimit-reason").is_some());
        assert!(body["errorMessages"][0].is_string());
    }
    assert_eq!(search(&fake, json!({"jql": "project = OPS"})).0, 200);

    // A repeated page token.
    for n in 0..3 {
        create(&fake, "OPS", &format!("page {n}"), &[]);
    }
    let (_, first) = search(&fake, json!({"jql": "project = OPS", "maxResults": 1}));
    let token = first["nextPageToken"].as_str().unwrap().to_owned();
    fake.fault(FakeOp::Search, Fault::RepeatToken);
    let (status, second) = search(
        &fake,
        json!({"jql": "project = OPS", "maxResults": 1, "nextPageToken": token}),
    );
    assert_eq!(status, 200);
    assert_eq!(second["nextPageToken"], token.as_str());
    assert_eq!(second["isLast"], false);
    // RepeatToken on a call with no page token gives the last token again.
    fake.fault(FakeOp::Search, Fault::RepeatToken);
    let (_, again) = search(&fake, json!({"jql": "project = OPS", "maxResults": 100}));
    assert_eq!(again["nextPageToken"], token.as_str());
    assert_eq!(again["isLast"], false);

    // Faults on the OAuth hosts.
    let fake = FakeJira::new().with_oauth("cid", "csecret", "at-0", "rt-0");
    fake.fault(FakeOp::Token, Fault::Network);
    fake.fault(FakeOp::AccessibleResources, Fault::Status(503));
    let token = HttpRequest::new("POST", "https://auth.atlassian.com/oauth/token")
        .header("content-type", "application/json")
        .body(
            json!({"grant_type": "refresh_token", "client_id": "cid",
            "client_secret": "csecret", "refresh_token": "rt-0"})
            .to_string()
            .into_bytes(),
        );
    assert!(matches!(
        send_with(&fake, token.clone()).await,
        Err(TransportError::Connect(_))
    ));
    assert_eq!(fake.token_refreshes(), 0);
    assert_eq!(send_with(&fake, token).await.unwrap().status, 200);
    let resources = bearer(
        "GET",
        "https://api.atlassian.com/oauth/token/accessible-resources",
        "at-0",
    );
    assert_eq!(send_with(&fake, resources).await.unwrap().status, 503);
}

#[tokio::test(start_paused = true)]
async fn index_lag_in_virtual_time() {
    let fake = FakeJira::new();
    fake.index_lag(Duration::from_secs(2));
    let (key, id) = create(&fake, "OPS", "lagging", &["old"]);
    let found = |jql: &str| {
        search(&fake, json!({"jql": jql})).1["issues"]
            .as_array()
            .unwrap()
            .len()
    };
    let count = || {
        api(
            &fake,
            "POST",
            "/rest/api/3/search/approximate-count",
            Some(json!({"jql": "project = OPS"})),
        )
        .1["count"]
            .clone()
    };

    // Get, properties and comments are current at once.
    assert_eq!(found("project = OPS"), 0);
    assert_eq!(count(), 0);
    assert_eq!(
        api(&fake, "GET", &format!("/rest/api/3/issue/{key}"), None).0,
        200
    );
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{key}/properties/p"),
            Some(json!(1))
        )
        .0,
        201
    );
    assert_eq!(
        api(
            &fake,
            "GET",
            &format!("/rest/api/3/issue/{key}/properties/p"),
            None
        )
        .0,
        200
    );
    assert_eq!(
        api(
            &fake,
            "GET",
            &format!("/rest/api/3/issue/{key}/comment"),
            None
        )
        .0,
        200
    );

    // `reconcileIssues` reads the current issue.
    let (_, page) = search(
        &fake,
        json!({"jql": "project = OPS", "reconcileIssues": [id]}),
    );
    assert_eq!(page["issues"].as_array().unwrap().len(), 1);

    tokio::time::advance(Duration::from_millis(1999)).await;
    assert_eq!(found("project = OPS"), 0);
    tokio::time::advance(Duration::from_millis(1)).await;
    assert_eq!(found("project = OPS"), 1);
    assert_eq!(count(), 1);

    // A change is visible to search only after the lag.
    let edit = json!({"update": {"labels": [{"add": "new"}, {"remove": "old"}]}});
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{key}"),
            Some(edit)
        )
        .0,
        204
    );
    assert_eq!(found("labels = new"), 0);
    assert_eq!(found("labels = old"), 1);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(found("labels = new"), 1);
    assert_eq!(found("labels = old"), 0);

    // With no lag, search is current.
    fake.index_lag(Duration::ZERO);
    let (second, _) = create(&fake, "OPS", "fresh", &[]);
    assert_eq!(found(&format!("key = {second}")), 1);
}

#[test]
fn workflow_needs_resolution() {
    let fake = FakeJira::new();
    let (key, _) = create(&fake, "OPS", "flow", &[]);
    let ids = |fake: &FakeJira| -> Vec<String> {
        let (_, list) = api(
            fake,
            "GET",
            &format!("/rest/api/3/issue/{key}/transitions"),
            None,
        );
        list["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let status_of = |fake: &FakeJira| -> Value {
        api(
            fake,
            "GET",
            &format!("/rest/api/3/issue/{key}?fields=status,resolution,resolutiondate"),
            None,
        )
        .1["fields"]
            .clone()
    };

    assert_eq!(ids(&fake), ["21"]);
    assert_eq!(
        transition(&fake, &key, "31", None),
        400,
        "no path from To Do to Done"
    );
    assert_eq!(transition(&fake, &key, "21", None), 204);
    assert_eq!(status_of(&fake)["status"]["name"], "In Progress");
    assert_eq!(
        status_of(&fake)["status"]["statusCategory"]["key"],
        "indeterminate"
    );
    assert_eq!(ids(&fake), ["31"]);
    let (_, filtered) = api(
        &fake,
        "GET",
        &format!("/rest/api/3/issue/{key}/transitions?transitionId=99"),
        None,
    );
    assert_eq!(filtered["transitions"], json!([]));
    let (_, with_fields) = api(
        &fake,
        "GET",
        &format!("/rest/api/3/issue/{key}/transitions?expand=transitions.fields"),
        None,
    );
    assert_eq!(
        with_fields["transitions"][0]["fields"]["resolution"]["required"],
        true
    );
    assert_eq!(with_fields["transitions"][0]["to"]["name"], "Done");
    assert_ne!(
        with_fields["transitions"][0]["name"], "Done",
        "a transition name is not the status name"
    );

    // Done needs a resolution.
    let (status, err) = common::transition_raw(&fake, &key, json!({"transition": {"id": "31"}}));
    assert_eq!(status, 400);
    assert!(err["errors"]["resolution"].is_string());
    let (status, err) = common::transition_raw(
        &fake,
        &key,
        json!({"transition": {"id": "31"}, "fields": {"resolution": {"name": "Maybe"}}}),
    );
    assert_eq!(status, 400);
    assert!(err["errors"]["resolution"].is_string());
    assert_eq!(
        common::transition_raw(&fake, &key, json!({"fields": {}})).0,
        400
    );
    assert_eq!(
        transition(
            &fake,
            &key,
            "31",
            Some(json!({"resolution": {"name": "Done"}}))
        ),
        204
    );
    let fields = status_of(&fake);
    assert_eq!(fields["status"]["name"], "Done");
    assert_eq!(fields["status"]["statusCategory"]["key"], "done");
    assert_eq!(fields["resolution"]["name"], "Done");
    assert!(
        fields["resolutiondate"]
            .as_str()
            .unwrap()
            .ends_with("+0000")
    );

    // Done goes back to To Do. Reopen clears the resolution.
    assert_eq!(ids(&fake), ["11"]);
    let (status, _) = common::transition_raw(
        &fake,
        &key,
        json!({"transition": {"id": "11"}, "update": {"comment": [{"add": {"body": {
            "type": "doc", "version": 1, "content": [{"type": "paragraph", "content": [
                {"type": "text", "text": "reopened"}]}]}}}]}}),
    );
    assert_eq!(status, 204);
    let fields = status_of(&fake);
    assert_eq!(fields["status"]["name"], "To Do");
    assert!(fields["resolution"].is_null());
    assert_eq!(
        api(
            &fake,
            "GET",
            &format!("/rest/api/3/issue/{key}/comment"),
            None
        )
        .1["total"],
        1
    );
    assert_eq!(transition(&fake, "OPS-99", "21", None), 404);

    // A person in the Jira UI moves along the same graph.
    assert!(matches!(
        fake.human_transition(&key, "Done", "ann"),
        Err(FakeError::NoTransition(s)) if s == "Done"
    ));
    fake.human_transition(&key, "in progress", "ann").unwrap();
    fake.human_transition(&key, "Done", ANN).unwrap();
    let fields = status_of(&fake);
    assert_eq!(fields["status"]["name"], "Done");
    assert_eq!(fields["resolution"]["name"], "Done");
    assert!(
        matches!(fake.human_transition("OPS-99", "Done", "ann"), Err(FakeError::NoIssue(k)) if k == "OPS-99")
    );
    assert!(matches!(
        fake.human_comment("OPS-99", "x", "ann"),
        Err(FakeError::NoIssue(_))
    ));
    assert!(matches!(
        fake.human_edit_labels("OPS-99", &[], &[], "ann"),
        Err(FakeError::NoIssue(_))
    ));
    assert_eq!(
        FakeError::NoIssue("OPS-9".into()).to_string(),
        "no issue OPS-9"
    );
    assert_eq!(
        FakeError::NoTransition("Done".into()).to_string(),
        "no transition to Done"
    );
}

#[test]
fn deliveries_are_signed() {
    let secret = "whsec-test";
    let fake = FakeJira::new().with_webhook_secret(secret);
    let check = |d: &autumn_jira_fake::FakeDelivery| {
        assert_eq!(
            d.header("x-hub-signature"),
            Some(autumn_jira_fake::sign(secret.as_bytes(), &d.body).as_str())
        );
        assert_eq!(d.header("X-Atlassian-Webhook-Flow"), Some("Primary"));
        assert_eq!(d.header("X-Atlassian-Webhook-Identifier"), Some(d.id()));
        assert!(
            d.header("content-type")
                .unwrap()
                .starts_with("application/json")
        );
        let body: Value = serde_json::from_slice(&d.body).unwrap();
        assert!(body["timestamp"].is_i64());
        body
    };

    // An API create by the bot queues a delivery too (the loop case).
    let (key, id) = create(&fake, "OPS", "watched", &["a"]);
    let created = fake.take_deliveries();
    assert_eq!(created.len(), 1);
    let body = check(&created[0]);
    assert_eq!(body["webhookEvent"], "jira:issue_created");
    assert_eq!(body["issue_event_type_name"], "issue_created");
    assert_eq!(body["user"]["accountId"], BOT);
    assert_eq!(body["issue"]["key"], key.as_str());
    assert_eq!(body["issue"]["id"], id.to_string());
    assert_eq!(body["issue"]["fields"]["status"]["name"], "To Do");
    assert!(fake.take_deliveries().is_empty(), "take drains the queue");

    // A person moves the issue.
    fake.human_transition(&key, "In Progress", "ann").unwrap();
    fake.human_transition(&key, "Done", "ann").unwrap();
    let moved = fake.take_deliveries();
    assert_eq!(moved.len(), 2);
    let body = check(&moved[1]);
    assert_eq!(body["webhookEvent"], "jira:issue_updated");
    assert_eq!(body["user"]["accountId"], ANN);
    assert_eq!(body["issue"]["fields"]["status"]["name"], "Done");
    let items = body["changelog"]["items"].as_array().unwrap();
    let status = items.iter().find(|i| i["field"] == "status").unwrap();
    assert_eq!(status["fromString"], "In Progress");
    assert_eq!(status["toString"], "Done");
    assert_eq!(status["from"], "10001");
    assert_eq!(status["to"], "10002");
    assert!(
        items
            .iter()
            .any(|i| i["field"] == "resolution" && i["toString"] == "Done")
    );
    assert!(body["changelog"]["id"].is_string());

    // Comments and labels.
    fake.human_comment(&key, "looks good\nsecond line", "ann")
        .unwrap();
    fake.human_edit_labels(&key, &["b"], &["a"], ANN).unwrap();
    let more = fake.take_deliveries();
    assert_eq!(more.len(), 2);
    let comment = check(&more[0]);
    assert_eq!(comment["webhookEvent"], "comment_created");
    assert_eq!(comment["comment"]["author"]["accountId"], ANN);
    assert_eq!(
        comment["comment"]["body"]["content"][1]["content"][0]["text"],
        "second line"
    );
    assert_eq!(comment["issue"]["key"], key.as_str());
    let labels = check(&more[1]);
    let item = &labels["changelog"]["items"][0];
    assert_eq!(item["field"], "labels");
    assert_eq!(item["fromString"], "a");
    assert_eq!(item["toString"], "b");
    let (_, page) = api(
        &fake,
        "GET",
        &format!("/rest/api/3/issue/{key}/comment"),
        None,
    );
    assert_eq!(page["comments"][0]["author"]["accountId"], ANN);

    // API edit, API comment, API transition and delete.
    let edit = json!({"fields": {"summary": "renamed"}});
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{key}"),
            Some(edit)
        )
        .0,
        204
    );
    assert_eq!(transition(&fake, &key, "11", None), 204);
    let comment = json!({"body": {"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "bot note"}]}]}});
    assert_eq!(
        api(
            &fake,
            "POST",
            &format!("/rest/api/3/issue/{key}/comment"),
            Some(comment)
        )
        .0,
        201
    );
    assert_eq!(
        api(&fake, "DELETE", &format!("/rest/api/3/issue/{key}"), None).0,
        204
    );
    let rest = fake.take_deliveries();
    let events: Vec<String> = rest
        .iter()
        .map(|d| check(d)["webhookEvent"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        events,
        [
            "jira:issue_updated",
            "jira:issue_updated",
            "comment_created",
            "jira:issue_deleted"
        ]
    );
    let summary = check(&rest[0]);
    assert_eq!(summary["changelog"]["items"][0]["field"], "summary");
    assert_eq!(summary["changelog"]["items"][0]["toString"], "renamed");
    assert_eq!(summary["user"]["accountId"], BOT);

    // Every id is unique.
    let mut ids: Vec<String> = created
        .iter()
        .chain(&moved)
        .chain(&more)
        .chain(&rest)
        .map(|d| d.id().to_owned())
        .collect();
    let all = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all);

    // Tampered and re-signed copies.
    let original = &moved[1];
    let tampered = original.tampered();
    assert_ne!(tampered.body, original.body);
    assert_eq!(
        tampered.header("x-hub-signature"),
        original.header("x-hub-signature")
    );
    assert_ne!(
        tampered.header("x-hub-signature"),
        Some(autumn_jira_fake::sign(secret.as_bytes(), &tampered.body).as_str())
    );
    let as_bot = original.with_actor(BOT);
    let body = check(&as_bot);
    assert_eq!(body["user"]["accountId"], BOT);
    assert_ne!(as_bot.id(), original.id());
    let comment_as = more[0].with_actor("acct-other");
    assert_eq!(
        check(&comment_as)["comment"]["author"]["accountId"],
        "acct-other"
    );
    assert!(!format!("{original:?}").contains(secret));
    assert_eq!(original.header("x-missing"), None);

    // No secret: no deliveries.
    let quiet = FakeJira::new();
    create(&quiet, "OPS", "quiet", &[]);
    quiet.human_comment("OPS-1", "hello", "bot").unwrap();
    assert!(quiet.take_deliveries().is_empty());
}

#[tokio::test]
async fn fallback_serves_other_hosts() {
    struct Chat;
    #[async_trait]
    impl HttpTransport for Chat {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            Ok(HttpResponse::new(201, request.url.into_bytes()))
        }
    }

    let fake = FakeJira::new().with_fallback(Arc::new(Chat));
    let chat = HttpRequest::new("POST", "https://chat.example/v1/messages");
    let reply = fake.send(chat.clone()).await.unwrap();
    assert_eq!(reply.status, 201);
    assert_eq!(reply.body, b"https://chat.example/v1/messages");
    // The sans-IO core never calls the fallback.
    assert!(matches!(
        fake.handle(&chat),
        Err(TransportError::Connect(_))
    ));
    // Without a fallback, another host fails to connect.
    let alone = FakeJira::new();
    assert!(matches!(
        alone.send(chat).await,
        Err(TransportError::Connect(_))
    ));
    // The fake still serves its own hosts.
    let own = common::with_auth(HttpRequest::new("GET", format!("{SITE}/rest/api/3/myself")));
    assert_eq!(fake.send(own).await.unwrap().status, 200);

    // Two tenants: one fake falls back to the other.
    let beta = FakeJira::new()
        .with_site("beta", "22222222-3333-4444-8555-666666666666")
        .with_oauth("cid-b", "secret-b", "at-b", "rt-b");
    let acme = FakeJira::new()
        .with_oauth("cid-a", "secret-a", "at-a", "rt-a")
        .with_fallback(Arc::new(beta.clone()));
    let beta_site = common::with_auth(HttpRequest::new(
        "GET",
        "https://beta.atlassian.net/rest/api/3/serverInfo",
    ));
    let reply = acme.send(beta_site).await.unwrap();
    assert_eq!(json_of(&reply)["baseUrl"], "https://beta.atlassian.net");
    let beta_gw = bearer(
        "GET",
        "https://api.atlassian.com/ex/jira/22222222-3333-4444-8555-666666666666/rest/api/3/myself",
        "at-b",
    );
    assert_eq!(acme.send(beta_gw).await.unwrap().status, 200);
    let resources = bearer(
        "GET",
        "https://api.atlassian.com/oauth/token/accessible-resources",
        "at-b",
    );
    let list = json_of(&acme.send(resources).await.unwrap());
    assert_eq!(list[0]["url"], "https://beta.atlassian.net");
    let refresh = HttpRequest::new("POST", "https://auth.atlassian.com/oauth/token")
        .header("content-type", "application/json")
        .body(
            json!({"grant_type": "refresh_token", "client_id": "cid-b",
            "client_secret": "secret-b", "refresh_token": "rt-b"})
            .to_string()
            .into_bytes(),
        );
    assert_eq!(acme.send(refresh).await.unwrap().status, 200);
    assert_eq!(beta.token_refreshes(), 1);
    assert_eq!(acme.token_refreshes(), 0);
    assert!(
        acme.requests()
            .iter()
            .all(|r| r.host != "beta.atlassian.net")
    );
    assert_eq!(beta.requests().len(), 4);
    // In the sans-IO core, the other cloud id is not found.
    let other = format!(
        "https://api.atlassian.com/ex/jira/22222222-3333-4444-8555-666666666666/rest/api/3/myself"
    );
    assert_eq!(call(&acme, "GET", &other, None).status, 404);
    assert_eq!(DEFAULT_CLOUD_ID, "11111111-2222-4333-8444-555555555555");
}

#[test]
fn snapshot_shape() {
    let fake = FakeJira::new();
    let (key, _) = create(&fake, "OPS", "snap", &["b", "a"]);
    create(&fake, "OPS", "other", &[]);
    assert_eq!(
        api(
            &fake,
            "PUT",
            &format!("/rest/api/3/issue/{key}/properties/autumn-jira"),
            Some(json!({"k": "v"}))
        )
        .0,
        201
    );
    fake.human_comment(&key, "hello\nworld", "ann").unwrap();
    fake.human_transition(&key, "In Progress", "ann").unwrap();

    let snap = fake.snapshot();
    let issues = snap["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    let first = &issues[0];
    let mut keys: Vec<&String> = first.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "assignee",
            "comments",
            "id",
            "issueType",
            "key",
            "labels",
            "project",
            "properties",
            "resolution",
            "status",
            "summary"
        ]
    );
    assert_eq!(first["key"], key.as_str());
    assert_eq!(first["id"], "10000");
    assert_eq!(first["project"], "OPS");
    assert_eq!(first["issueType"], "Task");
    assert_eq!(first["status"], "In Progress");
    assert_eq!(first["labels"], json!(["a", "b"]));
    assert_eq!(first["properties"], json!({"autumn-jira": {"k": "v"}}));
    assert!(first["assignee"].is_null());
    assert!(first["resolution"].is_null());
    assert_eq!(
        first["comments"],
        json!([{"id": "10000", "author": ANN, "text": "hello\nworld"}])
    );
    assert_eq!(issues[1]["key"], "OPS-2");
    assert_eq!(snap["site"], "acme");
    assert_eq!(snap["cloudId"], DEFAULT_CLOUD_ID);
    // The snapshot has no times, so two equal histories give equal snapshots.
    let twin = FakeJira::new();
    let (twin_key, _) = create(&twin, "OPS", "snap", &["b", "a"]);
    create(&twin, "OPS", "other", &[]);
    api(
        &twin,
        "PUT",
        &format!("/rest/api/3/issue/{twin_key}/properties/autumn-jira"),
        Some(json!({"k": "v"})),
    );
    twin.human_comment(&twin_key, "hello\nworld", "ann")
        .unwrap();
    twin.human_transition(&twin_key, "In Progress", "ann")
        .unwrap();
    assert_eq!(twin.snapshot(), snap);

    // `issue` and `issues` give full issue beans with all properties.
    let bean = fake.issue(&key).unwrap();
    assert_eq!(bean["key"], key.as_str());
    assert_eq!(bean["properties"]["autumn-jira"], json!({"k": "v"}));
    assert_eq!(bean["fields"]["comment"]["total"], 1);
    assert_eq!(
        fake.issue(&key.to_lowercase()).unwrap()["key"],
        key.as_str()
    );
    assert!(fake.issue("OPS-99").is_none());
    assert_eq!(fake.issues().len(), 2);
    assert_eq!(fake.issues()[1]["key"], "OPS-2");
}
