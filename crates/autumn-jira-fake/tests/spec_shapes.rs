//! Each fake reply uses only the keys and the JSON types of its spec
//! schema (AC23). `scripts/extract-jira-schemas.py` writes the fixture from
//! the Jira Cloud REST v3 `OpenAPI` spec.

#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    reason = "a test: a failure is a test failure, and one walk covers every reply"
)]

mod common;

use autumn_jira_fake::{FakeJira, FakeOp, FakeProject, Fault};
use common::{SITE, api, call, json_of, search, transition};
use serde_json::{Value, json};

/// The fixture: property names and JSON types of the response schemas.
const FIXTURE: &str = include_str!("fixtures/schemas.json");

/// Checks values against the schemas of the fixture.
struct Checker {
    schemas: Value,
    errors: Vec<String>,
    checked: usize,
}

impl Checker {
    fn new() -> Self {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        Self {
            schemas: fixture["schemas"].clone(),
            errors: Vec::new(),
            checked: 0,
        }
    }

    /// Checks an object against a schema. Every key must be in the schema.
    fn object(&mut self, schema: &str, value: &Value, path: &str) {
        self.checked += 1;
        let Some(props) = self.schemas.get(schema).and_then(Value::as_object).cloned() else {
            self.errors
                .push(format!("{path}: the fixture has no schema {schema}"));
            return;
        };
        let Some(object) = value.as_object() else {
            self.errors
                .push(format!("{path}: {schema} must be an object, not {value}"));
            return;
        };
        for (key, item) in object {
            let at = format!("{path}.{key}");
            match props.get(key) {
                None => self.errors.push(format!("{at}: {schema} has no key {key}")),
                Some(entry) => self.typed(entry, item, &at),
            }
        }
    }

    /// Checks one value against a type entry: type, ref, items, nullable.
    fn typed(&mut self, entry: &Value, value: &Value, path: &str) {
        let kind = entry["type"].as_str().unwrap_or("any");
        if value.is_null() {
            if kind != "any" && entry["nullable"] != true {
                self.errors.push(format!("{path}: null for a {kind}"));
            }
            return;
        }
        let ok = match kind {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !ok {
            self.errors.push(format!("{path}: {value} is not a {kind}"));
            return;
        }
        if let Some(name) = entry["ref"].as_str() {
            self.object(name, value, path);
        }
        if let (Some(items), Some(list)) = (entry.get("items"), value.as_array()) {
            for (n, item) in list.iter().enumerate() {
                self.typed(items, item, &format!("{path}[{n}]"));
            }
        }
    }

    /// Checks an issue bean, and the known fields inside `fields`.
    fn issue(&mut self, value: &Value, path: &str) {
        self.object("IssueBean", value, path);
        let fields = &value["fields"];
        let known = [
            ("status", "StatusDetails"),
            ("issuetype", "IssueTypeDetails"),
            ("assignee", "UserDetails"),
            ("reporter", "UserDetails"),
            ("creator", "UserDetails"),
            ("comment", "PageOfComments"),
        ];
        for (field, schema) in known {
            if let Some(v) = fields.get(field).filter(|v| !v.is_null()) {
                self.object(schema, v, &format!("{path}.fields.{field}"));
            }
        }
        if let Some(labels) = fields.get("labels") {
            assert!(
                labels.as_array().unwrap().iter().all(Value::is_string),
                "{path}"
            );
        }
    }
}

/// Calls a path and returns the JSON body after a status check.
fn get(fake: &FakeJira, method: &str, path: &str, body: Option<Value>, status: u16) -> Value {
    let (got, value) = api(fake, method, path, body);
    assert_eq!(got, status, "{method} {path}: {value}");
    value
}

#[test]
fn fake_replies_match_the_spec_shapes() {
    let mut c = Checker::new();

    // The checker itself finds an unknown key, a wrong type and a null.
    c.object(
        "CreatedIssue",
        &json!({"id": 1, "key": "A-1", "extra": true, "self": null}),
        "probe",
    );
    assert_eq!(c.errors.len(), 3, "{:?}", c.errors);
    c.errors.clear();

    let fake = FakeJira::new()
        .with_webhook_secret("shape-secret")
        .with_project(FakeProject::new("WEB", "10100", "Web").issue_type("10001", "Task"));

    c.object(
        "User",
        &get(&fake, "GET", "/rest/api/3/myself", None, 200),
        "myself",
    );
    c.object(
        "ServerInformation",
        &get(&fake, "GET", "/rest/api/3/serverInfo", None, 200),
        "serverInfo",
    );
    for path in [
        "/rest/api/3/project/OPS",
        "/rest/api/3/project/OPS?expand=lead",
    ] {
        c.object("Project", &get(&fake, "GET", path, None, 200), path);
    }
    for path in [
        "/rest/api/3/project/search",
        "/rest/api/3/project/search?maxResults=1&expand=lead",
    ] {
        c.object("PageBeanProject", &get(&fake, "GET", path, None, 200), path);
    }
    c.object(
        "PageOfCreateMetaIssueTypes",
        &get(
            &fake,
            "GET",
            "/rest/api/3/issue/createmeta/OPS/issuetypes?maxResults=2",
            None,
            200,
        ),
        "createmeta",
    );

    let body = json!({"fields": {
        "project": {"key": "OPS"},
        "issuetype": {"id": "10002"},
        "summary": "Shape",
        "labels": ["shape"],
        "assignee": {"accountId": autumn_jira_fake::ANN},
        "description": {"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "Body"}]}]},
    }, "properties": [{"key": "autumn-jira", "value": {"idempotencyKey": "k"}}]});
    c.object(
        "CreatedIssue",
        &get(&fake, "POST", "/rest/api/3/issue", Some(body), 201),
        "create",
    );
    common::create(&fake, "OPS", "Second", &[]);
    let comment_body = json!({"body": {"type": "doc", "version": 1, "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "note"}]}]}});
    c.object(
        "Comment",
        &get(
            &fake,
            "POST",
            "/rest/api/3/issue/OPS-1/comment",
            Some(comment_body),
            201,
        ),
        "add comment",
    );
    for path in [
        "/rest/api/3/issue/OPS-1",
        "/rest/api/3/issue/OPS-1?expand=transitions&properties=autumn-jira",
        "/rest/api/3/issue/OPS-2?fields=summary,status",
    ] {
        c.issue(&get(&fake, "GET", path, None, 200), path);
    }
    let edit = json!({"update": {"labels": [{"add": "more"}]}});
    c.issue(
        &get(
            &fake,
            "PUT",
            "/rest/api/3/issue/OPS-1?returnIssue=true",
            Some(edit),
            200,
        ),
        "edit",
    );
    c.object(
        "EntityProperty",
        &get(
            &fake,
            "GET",
            "/rest/api/3/issue/OPS-1/properties/autumn-jira",
            None,
            200,
        ),
        "property",
    );
    for path in [
        "/rest/api/3/issue/OPS-1/transitions",
        "/rest/api/3/issue/OPS-1/transitions?expand=transitions.fields",
    ] {
        c.object("Transitions", &get(&fake, "GET", path, None, 200), path);
    }
    assert_eq!(transition(&fake, "OPS-1", "21", None), 204);
    c.object(
        "Transitions",
        &get(
            &fake,
            "GET",
            "/rest/api/3/issue/OPS-1/transitions?expand=transitions.fields",
            None,
            200,
        ),
        "transitions to Done",
    );
    for path in [
        "/rest/api/3/issue/OPS-1/comment",
        "/rest/api/3/issue/OPS-1/comment?orderBy=-created&maxResults=1",
    ] {
        c.object("PageOfComments", &get(&fake, "GET", path, None, 200), path);
    }

    // Search pages: the first page has a token, the last page has none.
    let first = json!({"jql": "project = OPS", "maxResults": 1,
        "fields": ["summary", "status", "assignee", "comment"],
        "properties": ["autumn-jira"], "expand": "transitions"});
    let (status, page) = search(&fake, first);
    assert_eq!(status, 200);
    check_page(&mut c, &page, "search page 1");
    let token = page["nextPageToken"].clone();
    let (_, page) = search(
        &fake,
        json!({"jql": "project = OPS", "maxResults": 1, "nextPageToken": token}),
    );
    assert_eq!(page["isLast"], true);
    check_page(&mut c, &page, "search page 2");
    let page = get(
        &fake,
        "GET",
        "/rest/api/3/search/jql?jql=project%3DOPS&fields=*all&reconcileIssues=10000",
        None,
        200,
    );
    check_page(&mut c, &page, "search GET");
    c.object(
        "JQLCountResultsBean",
        &get(
            &fake,
            "POST",
            "/rest/api/3/search/approximate-count",
            Some(json!({"jql": "project = OPS"})),
            200,
        ),
        "count",
    );

    // Error replies are ErrorCollection.
    let errors = [
        get(
            &fake,
            "POST",
            "/rest/api/3/issue",
            Some(json!({"fields": {}})),
            400,
        ),
        get(&fake, "GET", "/rest/api/3/issue/OPS-99", None, 404),
        get(&fake, "GET", "/rest/api/3/project/NOPE", None, 404),
        get(
            &fake,
            "POST",
            "/rest/api/3/search/jql",
            Some(json!({"jql": "foo = 1"})),
            400,
        ),
        get(
            &fake,
            "GET",
            "/rest/api/3/search?jql=project%3DOPS",
            None,
            410,
        ),
        get(
            &fake,
            "GET",
            "/rest/api/3/issue/OPS-1/properties/none",
            None,
            404,
        ),
        get(
            &fake,
            "POST",
            "/rest/api/3/issue/OPS-1/transitions",
            Some(json!({"transition": {"id": "31"}})),
            400,
        ),
    ];
    for (n, error) in errors.iter().enumerate() {
        c.object("ErrorCollection", error, &format!("error {n}"));
    }
    let anonymous = fake
        .handle(&san_mateo::transport::HttpRequest::new(
            "GET",
            format!("{SITE}/rest/api/3/myself"),
        ))
        .unwrap();
    c.object("ErrorCollection", &json_of(&anonymous), "401");
    fake.fault(FakeOp::GetIssue, Fault::Status(503));
    fake.rate_limit(FakeOp::GetIssue, 1, 2);
    for name in ["503", "429"] {
        let reply = call(
            &fake,
            "GET",
            &format!("{SITE}/rest/api/3/issue/OPS-1"),
            None,
        );
        c.object("ErrorCollection", &json_of(&reply), name);
    }

    // The issue and the comment inside a webhook delivery have the REST shapes.
    fake.human_comment("OPS-1", "from a person", "ann").unwrap();
    for delivery in fake.take_deliveries() {
        let body: Value = serde_json::from_slice(&delivery.body).unwrap();
        if body["webhookEvent"] == "comment_created" {
            c.object("Comment", &body["comment"], "delivery comment");
        } else {
            c.issue(&body["issue"], "delivery issue");
        }
    }

    assert!(
        c.errors.is_empty(),
        "{} shape errors:\n{}",
        c.errors.len(),
        c.errors.join("\n")
    );
    assert!(c.checked > 100, "the walk checked {} objects", c.checked);
}

/// Checks a search page and each issue in it.
fn check_page(c: &mut Checker, page: &Value, path: &str) {
    c.object("SearchAndReconcileResults", page, path);
    for (n, issue) in page["issues"].as_array().unwrap().iter().enumerate() {
        c.issue(issue, &format!("{path}.issues[{n}]"));
    }
}
