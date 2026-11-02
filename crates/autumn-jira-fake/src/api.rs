//! The Jira Cloud REST v3 endpoints of the fake.

use std::collections::BTreeSet;

use san_mateo::transport::HttpResponse;
use serde_json::{Map, Value, json};

use crate::jql::{self, Ctx};
use crate::render::{self, BeanParts, FieldFilter};
use crate::state::{Comment, Issue, PageToken, State};
use crate::types::FakeOp;
use crate::webhook::{self, Event};
use crate::wire;
use crate::workflow::{self, Transition};

/// The message of a field that the create or edit screen does not have.
const NOT_ON_SCREEN: &str = "cannot be set. It is not on the appropriate screen, or unknown.";

/// The reply to a call of a removed API.
const GONE: &str = "The requested API has been removed. Please migrate to the /rest/api/3/search/jql API. A full migration guideline is available at https://developer.atlassian.com/changelog/#CHANGE-2046";

/// One REST call after routing.
#[derive(Debug, Clone)]
pub(crate) struct Call<'a> {
    /// The upper-case method.
    pub(crate) method: &'a str,
    /// The path segments after `/rest/api/3/`, decoded.
    pub(crate) segments: Vec<String>,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: &'a [u8],
    /// The account id of the caller.
    pub(crate) actor: &'a str,
    /// `Fault::RepeatToken` is active for this call.
    pub(crate) repeat_token: bool,
}

impl Call<'_> {
    fn segment(&self, n: usize) -> &str {
        self.segments.get(n).map_or("", String::as_str)
    }

    fn query(&self, name: &str) -> Option<&str> {
        wire::first(&self.query, name)
    }

    fn query_usize(&self, name: &str, default: usize) -> usize {
        self.query(name)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }

    fn expands(&self, what: &str) -> bool {
        wire::csv(&self.query, "expand").iter().any(|e| e == what)
    }
}

/// The result of routing a REST path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    Op(FakeOp),
    /// A removed API: 410.
    Gone,
    NotFound,
    MethodNotAllowed,
}

/// Maps a method and the path segments to an op.
pub(crate) fn classify(method: &str, segments: &[String]) -> Route {
    let s: Vec<&str> = segments.iter().map(String::as_str).collect();
    let ops: &[(&str, FakeOp)] = match s.as_slice() {
        ["myself"] => &[("GET", FakeOp::Myself)],
        ["serverInfo"] => &[("GET", FakeOp::ServerInfo)],
        ["project", "search"] => &[("GET", FakeOp::ProjectSearch)],
        ["project", _] => &[("GET", FakeOp::GetProject)],
        ["issue", "createmeta", _, "issuetypes"] => &[("GET", FakeOp::CreateMeta)],
        ["issue"] => &[("POST", FakeOp::CreateIssue)],
        ["issue", _] => &[
            ("GET", FakeOp::GetIssue),
            ("PUT", FakeOp::EditIssue),
            ("DELETE", FakeOp::DeleteIssue),
        ],
        ["issue", _, "properties", _] => {
            &[("GET", FakeOp::GetProperty), ("PUT", FakeOp::SetProperty)]
        }
        ["issue", _, "transitions"] => &[
            ("GET", FakeOp::GetTransitions),
            ("POST", FakeOp::DoTransition),
        ],
        ["issue", _, "comment"] => &[("GET", FakeOp::GetComments), ("POST", FakeOp::AddComment)],
        ["search", "jql"] => &[("GET", FakeOp::Search), ("POST", FakeOp::Search)],
        ["search", "approximate-count"] => &[("POST", FakeOp::Count)],
        ["search"] if method == "GET" || method == "POST" => return Route::Gone,
        ["search"] => &[],
        _ => return Route::NotFound,
    };
    ops.iter()
        .find(|(m, _)| *m == method)
        .map_or(Route::MethodNotAllowed, |(_, op)| Route::Op(*op))
}

/// Returns a JSON reply.
fn reply(status: u16, body: &Value) -> HttpResponse {
    HttpResponse::json(status, body)
}

/// Returns a reply with no body.
fn empty(status: u16) -> HttpResponse {
    HttpResponse::new(status, Vec::new())
}

/// Returns a 400 reply with one message.
fn bad_request(message: &str) -> HttpResponse {
    reply(400, &render::errors(&[message], &[]))
}

/// The reply for a missing issue. Jira cannot tell "not found" from "no
/// permission".
fn no_issue() -> HttpResponse {
    reply(
        404,
        &render::errors(
            &["Issue does not exist or you do not have permission to see it."],
            &[],
        ),
    )
}

/// Returns the reply to a removed API.
pub(crate) fn gone() -> HttpResponse {
    reply(410, &render::errors(&[GONE], &[]))
}

/// Returns the reply to an unknown path.
pub(crate) fn not_found() -> HttpResponse {
    reply(
        404,
        &render::errors(&["The requested resource could not be found."], &[]),
    )
}

/// Returns the reply to a known path with a wrong method.
pub(crate) fn method_not_allowed() -> HttpResponse {
    reply(
        405,
        &render::errors(&["The method is not allowed for this resource."], &[]),
    )
}

/// Parses a JSON object body.
fn object_body(body: &[u8]) -> Result<Map<String, Value>, HttpResponse> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(bad_request("The request body must be a JSON object.")),
        Err(_) => Err(bad_request("The request body is not valid JSON.")),
    }
}

/// Serves one REST call of an op.
pub(crate) fn serve(state: &mut State, op: FakeOp, call: &Call<'_>) -> HttpResponse {
    match op {
        FakeOp::Myself => reply(200, &render::myself(state)),
        FakeOp::ServerInfo => reply(200, &render::server_info(state)),
        FakeOp::GetProject => get_project(state, call),
        FakeOp::ProjectSearch => project_search(state, call),
        FakeOp::CreateMeta => create_meta(state, call),
        FakeOp::CreateIssue => create_issue(state, call),
        FakeOp::GetIssue => get_issue(state, call),
        FakeOp::EditIssue => edit_issue(state, call),
        FakeOp::DeleteIssue => delete_issue(state, call),
        FakeOp::GetProperty => get_property(state, call),
        FakeOp::SetProperty => set_property(state, call),
        FakeOp::GetTransitions => get_transitions(state, call),
        FakeOp::DoTransition => do_transition(state, call),
        FakeOp::GetComments => get_comments(state, call),
        FakeOp::AddComment => post_comment(state, call),
        FakeOp::Search => search(state, call),
        FakeOp::Count => count(state, call),
        FakeOp::AccessibleResources | FakeOp::Token => not_found(),
    }
}

fn get_project(state: &State, call: &Call<'_>) -> HttpResponse {
    let key = call.segment(1);
    state.project(key).map_or_else(
        || {
            reply(
                404,
                &render::errors(
                    &[&format!("No project could be found with key '{key}'.")],
                    &[],
                ),
            )
        },
        |p| reply(200, &render::project(state, p, call.expands("lead"))),
    )
}

fn project_search(state: &State, call: &Call<'_>) -> HttpResponse {
    let start = call.query_usize("startAt", 0);
    let max = call.query_usize("maxResults", 50).clamp(1, 100);
    let total = state.projects.len();
    let lead = call.expands("lead");
    let values: Vec<Value> = state
        .projects
        .iter()
        .skip(start)
        .take(max)
        .map(|p| render::project(state, p, lead))
        .collect();
    let is_last = start + max >= total;
    let page_url = |at: usize| {
        format!(
            "{}/rest/api/3/project/search?startAt={at}&maxResults={max}",
            state.base()
        )
    };
    let mut out = json!({
        "self": page_url(start),
        "maxResults": max,
        "startAt": start,
        "total": total,
        "isLast": is_last,
        "values": values,
    });
    if !is_last {
        out["nextPage"] = Value::from(page_url(start + max));
    }
    reply(200, &out)
}

fn create_meta(state: &State, call: &Call<'_>) -> HttpResponse {
    let key = call.segment(2);
    let Some(project) = state.project(key) else {
        return bad_request(&format!("No project could be found with key '{key}'."));
    };
    let start = call.query_usize("startAt", 0);
    let max = call.query_usize("maxResults", 50).clamp(1, 200);
    let types: Vec<Value> = project
        .issue_types
        .iter()
        .skip(start)
        .take(max)
        .map(|t| render::create_meta_type(state, t))
        .collect();
    reply(
        200,
        &json!({
            "maxResults": max,
            "startAt": start,
            "total": project.issue_types.len(),
            "issueTypes": types,
        }),
    )
}

/// The screen that a field edit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Create,
    Edit,
    Transition,
}

/// Collects field errors and comments to add.
#[derive(Debug, Default)]
struct Edits {
    errors: Map<String, Value>,
    comments: Vec<Value>,
}

impl Edits {
    fn error(&mut self, field: &str, message: impl Into<String>) {
        self.errors
            .entry(field.to_owned())
            .or_insert_with(|| Value::from(message.into()));
    }
}

/// Reads `{"id": ..}`, `{"key": ..}` or `{"name": ..}`.
fn reference<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|n| value.get(*n).and_then(Value::as_str))
}

/// Sets one field on an issue.
fn set_field(issue: &mut Issue, name: &str, value: &Value, screen: Screen, edits: &mut Edits) {
    match name {
        "summary" => match value.as_str().map(str::trim) {
            Some(text) if !text.is_empty() && text.len() <= 255 => {
                text.clone_into(&mut issue.summary);
            }
            _ => edits.error(name, "You must specify a summary of the issue."),
        },
        "description" => match value {
            Value::Null => issue.description = None,
            v if render::is_adf(v) => issue.description = Some(v.clone()),
            _ => edits.error(
                name,
                "Operation value must be an Atlassian Document (see the Atlassian Document Format)",
            ),
        },
        "labels" => match value.as_array() {
            Some(items) => {
                let mut labels = BTreeSet::new();
                for item in items {
                    match item.as_str() {
                        Some(l) if label_ok(l) => {
                            labels.insert(l.to_owned());
                        }
                        Some(l) => {
                            edits.error(
                                name,
                                format!("The label '{l}' contains spaces which is invalid."),
                            );
                        }
                        None => edits.error(name, "A label must be a string."),
                    }
                }
                issue.labels = labels;
            }
            None => edits.error(name, "Labels must be a list of strings."),
        },
        "assignee" | "reporter" => {
            let account = match value {
                Value::Null if name == "assignee" => Ok(None),
                v => v
                    .get("accountId")
                    .and_then(Value::as_str)
                    .filter(|a| !a.is_empty())
                    .map(|a| Some(a.to_owned()))
                    .ok_or(()),
            };
            match (name, account) {
                ("assignee", Ok(a)) => issue.assignee = a,
                ("reporter", Ok(Some(a))) => issue.reporter = a,
                _ => edits.error(name, format!("Specify a valid value for {name}.")),
            }
        }
        "priority" => {
            let wanted = reference(value, &["id", "name"]);
            match workflow::PRIORITIES
                .iter()
                .find(|(id, n)| Some(*id) == wanted || Some(*n) == wanted)
            {
                Some((id, _)) => issue.priority = id,
                None => edits.error(name, "The priority selected is invalid."),
            }
        }
        custom if custom.starts_with("customfield_") => {
            issue.custom.insert(custom.to_owned(), value.clone());
        }
        "project" | "issuetype" if screen == Screen::Create => {}
        other => edits.error(other, format!("Field '{other}' {NOT_ON_SCREEN}")),
    }
}

/// Returns `true` for a label that Jira accepts.
fn label_ok(label: &str) -> bool {
    !label.is_empty() && label.len() <= 255 && !label.chars().any(char::is_whitespace)
}

/// Applies the `fields` and `update` maps of an `IssueUpdateDetails` body.
fn apply_details(
    issue: &mut Issue,
    body: &Map<String, Value>,
    screen: Screen,
    skip: &[&str],
    edits: &mut Edits,
) {
    let empty = Map::new();
    let fields = match body.get("fields") {
        None => &empty,
        Some(Value::Object(f)) => f,
        Some(_) => {
            edits.error("fields", "The fields must be an object.");
            &empty
        }
    };
    let update = match body.get("update") {
        None => &empty,
        Some(Value::Object(u)) => u,
        Some(_) => {
            edits.error("update", "The update must be an object.");
            &empty
        }
    };
    for (name, value) in fields {
        if skip.contains(&name.as_str()) {
            continue;
        }
        if update.contains_key(name) {
            edits.error(
                name,
                format!("Field '{name}' cannot be set in both 'fields' and 'update'."),
            );
            continue;
        }
        set_field(issue, name, value, screen, edits);
    }
    for (name, ops) in update {
        let Some(ops) = ops.as_array() else {
            edits.error(name, "An update must be a list of operations.");
            continue;
        };
        for op in ops {
            apply_update(issue, name, op, screen, edits);
        }
    }
}

/// Applies one update operation, for example `{"add": "label"}`.
fn apply_update(issue: &mut Issue, name: &str, op: &Value, screen: Screen, edits: &mut Edits) {
    let Some((verb, value)) = op.as_object().and_then(|o| o.iter().next()) else {
        edits.error(name, "An operation must be an object.");
        return;
    };
    match (name, verb.as_str()) {
        ("labels", "add") => match value.as_str() {
            Some(l) if label_ok(l) => {
                issue.labels.insert(l.to_owned());
            }
            _ => edits.error(name, "The label is not valid."),
        },
        ("labels", "remove") => {
            if let Some(l) = value.as_str() {
                issue.labels.remove(l);
            }
        }
        ("comment", "add") => match value.get("body") {
            Some(body) if render::is_adf(body) => edits.comments.push(body.clone()),
            _ => edits.error(name, "Comment body can not be empty!"),
        },
        (_, "set") if name != "comment" => set_field(issue, name, value, screen, edits),
        _ => edits.error(
            name,
            format!("The operation '{verb}' is not valid for '{name}'."),
        ),
    }
}

/// Returns the field errors reply.
fn error_reply(edits: &Edits) -> HttpResponse {
    reply(400, &render::field_errors(&edits.errors))
}

/// An empty issue for a project, before its id and key exist.
fn blank_issue(state: &State, project: &str, issue_type: &str) -> Issue {
    let now = state.clock.now();
    Issue {
        id: 0,
        key: String::new(),
        number: 0,
        project: project.to_owned(),
        type_id: issue_type.to_owned(),
        summary: String::new(),
        description: None,
        status: workflow::TODO.id,
        resolution: None,
        resolution_date: None,
        labels: BTreeSet::new(),
        assignee: None,
        reporter: State::caller().to_owned(),
        priority: workflow::MEDIUM,
        custom: std::collections::BTreeMap::new(),
        properties: std::collections::BTreeMap::new(),
        comments: Vec::new(),
        created: now,
        updated: now,
        status_changed: now,
    }
}

fn create_issue(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let body = match object_body(call.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mut edits = Edits::default();
    let fields = body.get("fields").cloned().unwrap_or(Value::Null);
    let project = fields
        .get("project")
        .and_then(|p| reference(p, &["key", "id"]))
        .and_then(|k| state.project(k))
        .cloned();
    if project.is_none() {
        edits.error("project", "Specify a valid project ID or key");
    }
    let issue_type = project.as_ref().and_then(|p| {
        let wanted = fields.get("issuetype")?;
        p.issue_types
            .iter()
            .find(|t| {
                reference(wanted, &["id"]) == Some(t.id.as_str())
                    || reference(wanted, &["name"]).is_some_and(|n| n.eq_ignore_ascii_case(&t.name))
            })
            .map(|t| t.id.clone())
    });
    if project.is_some() && issue_type.is_none() {
        edits.error("issuetype", "The issue type selected is invalid.");
    }
    let project_key = project.as_ref().map_or("", |p| p.key.as_str());
    let mut issue = blank_issue(state, project_key, issue_type.as_deref().unwrap_or(""));
    if fields.get("summary").is_none() {
        edits.error("summary", "You must specify a summary of the issue.");
    }
    apply_details(&mut issue, &body, Screen::Create, &[], &mut edits);
    if !issue.labels.is_empty() && state.labels_off.contains(project_key) {
        edits.error("labels", format!("Field 'labels' {NOT_ON_SCREEN}"));
    }
    let mut properties = Vec::new();
    match body.get("properties") {
        None => {}
        Some(Value::Array(items)) => {
            for item in items {
                match (item.get("key").and_then(Value::as_str), item.get("value")) {
                    (Some(k), Some(v)) if property_key_ok(k) => {
                        properties.push((k.to_owned(), v.clone()));
                    }
                    _ => edits.error("properties", "Each property needs a key and a value."),
                }
            }
        }
        Some(_) => edits.error("properties", "The properties must be a list."),
    }
    if !edits.errors.is_empty() || project.is_none() {
        return error_reply(&edits);
    }
    let number = {
        let counter = state
            .key_counters
            .entry(project_key.to_owned())
            .or_insert(0);
        *counter += 1;
        *counter
    };
    issue.id = state.next_issue_id;
    state.next_issue_id += 1;
    issue.number = number;
    issue.key = format!("{project_key}-{number}");
    issue.properties.extend(properties);
    let id = issue.id;
    let key = issue.key.clone();
    state.issues.insert(id, issue.clone());
    state.reindex(id);
    webhook::emit(state, &Event::Created, &issue, call.actor);
    for comment in edits.comments {
        add_comment(state, id, comment, call.actor, false);
    }
    reply(
        201,
        &json!({
            "id": id.to_string(),
            "key": key,
            "self": format!("{}/rest/api/3/issue/{id}", state.base()),
        }),
    )
}

/// Returns `true` for a property key that Jira accepts.
const fn property_key_ok(key: &str) -> bool {
    !key.is_empty() && key.len() <= 255
}

fn get_issue(state: &State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let parts = BeanParts {
        fields: FieldFilter::parse(&wire::csv(&call.query, "fields"), FieldFilter::all()),
        properties: Some(wire::csv(&call.query, "properties")).filter(|p| !p.is_empty()),
        transitions: call.expands("transitions"),
    };
    reply(200, &render::issue_bean(state, &state.issues[&id], &parts))
}

/// Stores a changed issue, reindexes it, and queues one delivery.
fn commit_change(state: &mut State, old: &Issue, mut new: Issue, actor: &str, type_name: &str) {
    let changes = webhook::diff(old, &new);
    if changes.is_empty() {
        return;
    }
    new.updated = state.clock.now();
    let id = new.id;
    state.issues.insert(id, new.clone());
    state.reindex(id);
    webhook::emit(state, &Event::Updated { type_name, changes }, &new, actor);
}

fn edit_issue(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let body = match object_body(call.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let old = state.issues[&id].clone();
    let mut new = old.clone();
    let mut edits = Edits::default();
    apply_details(&mut new, &body, Screen::Edit, &[], &mut edits);
    if !edits.errors.is_empty() {
        return error_reply(&edits);
    }
    commit_change(state, &old, new, call.actor, "issue_updated");
    for comment in edits.comments {
        add_comment(state, id, comment, call.actor, false);
    }
    if call.query("returnIssue") == Some("true") {
        let parts = BeanParts {
            fields: FieldFilter::all(),
            ..BeanParts::default()
        };
        return reply(200, &render::issue_bean(state, &state.issues[&id], &parts));
    }
    empty(204)
}

fn delete_issue(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    match call.query("deleteSubtasks") {
        None | Some("true" | "false") => {}
        Some(_) => return bad_request("The value of deleteSubtasks must be 'true' or 'false'."),
    }
    let Some(issue) = state.issues.remove(&id) else {
        return no_issue();
    };
    state.reindex(id);
    webhook::emit(state, &Event::Deleted, &issue, call.actor);
    empty(204)
}

fn get_property(state: &State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let key = call.segment(3);
    state.issues[&id].properties.get(key).map_or_else(
        || {
            reply(
                404,
                &render::errors(
                    &[&format!("The property with key '{key}' does not exist.")],
                    &[],
                ),
            )
        },
        |value| reply(200, &json!({"key": key, "value": value})),
    )
}

fn set_property(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let key = call.segment(3).to_owned();
    if !property_key_ok(&key) {
        return bad_request("The property key must have 1 to 255 characters.");
    }
    if call.body.len() > 32_768 {
        return bad_request("The property value must be 32768 bytes or less.");
    }
    let Ok(value) = serde_json::from_slice::<Value>(call.body) else {
        return bad_request("The property value must be valid JSON.");
    };
    let Some(issue) = state.issues.get_mut(&id) else {
        return no_issue();
    };
    match issue.properties.insert(key, value) {
        Some(_) => empty(200),
        None => empty(201),
    }
}

fn get_transitions(state: &State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let list = render::transitions(
        state,
        &state.issues[&id],
        call.expands("transitions.fields"),
        call.query("transitionId"),
    );
    reply(200, &json!({"expand": "transitions", "transitions": list}))
}

/// Moves an issue along one transition. Done gets the resolution. Reopen
/// clears it.
fn finish_transition(
    issue: &mut Issue,
    t: Transition,
    resolution: Option<&'static str>,
    state: &State,
) {
    let now = state.clock.now();
    issue.status = t.to;
    issue.status_changed = now;
    if t.needs_resolution {
        issue.resolution = resolution;
        issue.resolution_date = Some(now);
    } else if workflow::status(t.to).category.key != "done" {
        issue.resolution = None;
        issue.resolution_date = None;
    }
}

/// Applies a transition by a person or the API.
pub(crate) fn transition_issue(
    state: &mut State,
    id: u64,
    t: Transition,
    resolution: Option<&'static str>,
    actor: &str,
) {
    let Some(old) = state.issues.get(&id).cloned() else {
        return;
    };
    let mut new = old.clone();
    finish_transition(&mut new, t, resolution, state);
    commit_change(state, &old, new, actor, "issue_generic");
}

fn do_transition(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let body = match object_body(call.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let wanted = body.get("transition").and_then(|t| match t.get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    });
    let Some(wanted) = wanted else {
        return bad_request("Missing 'transition' identifier");
    };
    let old = state.issues[&id].clone();
    let Some(t) = workflow::transitions_from(old.status).find(|t| t.id == wanted) else {
        return bad_request(&format!(
            "Transition id '{wanted}' is not valid for this issue."
        ));
    };
    let mut edits = Edits::default();
    let given = body.get("fields").and_then(|f| f.get("resolution"));
    let resolution = match (t.needs_resolution, given) {
        (true, None) => {
            edits.error("resolution", "Field 'resolution' is required.");
            None
        }
        (true, Some(value)) => {
            let found = workflow::RESOLUTIONS.iter().find(|(id, name, _)| {
                reference(value, &["id"]) == Some(*id) || reference(value, &["name"]) == Some(*name)
            });
            if found.is_none() {
                edits.error(
                    "resolution",
                    "Could not find valid 'id' or 'name' in resolution object.",
                );
            }
            found.map(|r| r.0)
        }
        (false, Some(_)) => {
            edits.error("resolution", format!("Field 'resolution' {NOT_ON_SCREEN}"));
            None
        }
        (false, None) => None,
    };
    let mut new = old.clone();
    apply_details(
        &mut new,
        &body,
        Screen::Transition,
        &["resolution"],
        &mut edits,
    );
    if !edits.errors.is_empty() {
        return error_reply(&edits);
    }
    finish_transition(&mut new, t, resolution, state);
    commit_change(state, &old, new, call.actor, "issue_generic");
    for comment in edits.comments {
        add_comment(state, id, comment, call.actor, false);
    }
    empty(204)
}

/// Adds a comment and returns it. With `notify`, it queues a
/// `comment_created` delivery.
pub(crate) fn add_comment(
    state: &mut State,
    issue_id: u64,
    body: Value,
    author: &str,
    notify: bool,
) -> Option<Comment> {
    let now = state.clock.now();
    let comment = Comment {
        id: state.next_comment_id,
        author: author.to_owned(),
        body,
        created: now,
        updated: now,
    };
    let issue = state.issues.get_mut(&issue_id)?;
    state.next_comment_id += 1;
    issue.comments.push(comment.clone());
    issue.updated = now;
    let issue = issue.clone();
    state.reindex(issue_id);
    if notify {
        webhook::emit(state, &Event::CommentCreated(&comment), &issue, author);
    }
    Some(comment)
}

fn get_comments(state: &State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let descending = match call.query("orderBy").map(str::trim) {
        None | Some("created" | "+created") => false,
        Some("-created") => true,
        Some(other) => {
            return bad_request(&format!(
                "The orderBy value '{other}' is not valid. Use created, +created or -created."
            ));
        }
    };
    let issue = &state.issues[&id];
    let mut comments: Vec<&Comment> = issue.comments.iter().collect();
    comments.sort_by_key(|c| (c.created, c.id));
    if descending {
        comments.reverse();
    }
    let start = call.query_usize("startAt", 0);
    let max = call.query_usize("maxResults", 100).min(5_000);
    let total = comments.len();
    let page: Vec<&Comment> = comments.into_iter().skip(start).take(max).collect();
    reply(
        200,
        &render::comment_page(state, issue, &page, start, max, total),
    )
}

fn post_comment(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let Some(id) = state.find_issue(call.segment(1)) else {
        return no_issue();
    };
    let body = match object_body(call.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    match body.get("body") {
        Some(adf) if render::is_adf(adf) => {
            match add_comment(state, id, adf.clone(), call.actor, true) {
                Some(comment) => reply(201, &render::comment(state, &state.issues[&id], &comment)),
                None => no_issue(),
            }
        }
        _ => reply(
            400,
            &render::errors(&[], &[("comment", "Comment body can not be empty!")]),
        ),
    }
}

/// The search parameters, from a POST body or a GET query.
#[derive(Debug, Default)]
struct SearchParams {
    jql: String,
    token: Option<String>,
    max_results: Option<usize>,
    fields: Vec<String>,
    properties: Vec<String>,
    expand: Vec<String>,
    reconcile: Vec<u64>,
}

/// Reads a list of strings from a JSON array or a comma-separated text.
fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

impl SearchParams {
    fn read(call: &Call<'_>, post: bool) -> Result<Self, HttpResponse> {
        if !post {
            return Ok(Self {
                jql: call.query("jql").unwrap_or_default().to_owned(),
                token: call.query("nextPageToken").map(str::to_owned),
                max_results: call.query("maxResults").and_then(|m| m.parse().ok()),
                fields: wire::csv(&call.query, "fields"),
                properties: wire::csv(&call.query, "properties"),
                expand: wire::csv(&call.query, "expand"),
                reconcile: wire::csv(&call.query, "reconcileIssues")
                    .iter()
                    .filter_map(|id| id.parse().ok())
                    .collect(),
            });
        }
        let body = object_body(call.body)?;
        Ok(Self {
            jql: body
                .get("jql")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            token: body
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_owned),
            max_results: body
                .get("maxResults")
                .and_then(Value::as_u64)
                .and_then(|m| usize::try_from(m).ok()),
            fields: string_list(body.get("fields")),
            properties: string_list(body.get("properties")),
            expand: string_list(body.get("expand")),
            reconcile: body
                .get("reconcileIssues")
                .and_then(Value::as_array)
                .map(|ids| ids.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default(),
        })
    }
}

/// Parses a query and checks its project values.
fn parse_jql(state: &State, text: &str) -> Result<jql::Query, HttpResponse> {
    let query = jql::parse(text).map_err(|e| bad_request(&e.message()))?;
    query
        .check_projects(&state.projects)
        .map_err(|e| bad_request(&e.message()))?;
    Ok(query)
}

/// Returns the ids of the issues that match, in order. Search reads the
/// index, except for the `reconcile` ids, which read the current issue.
fn matching(state: &State, query: &jql::Query, reconcile: &[u64]) -> Vec<u64> {
    let ctx = Ctx {
        projects: &state.projects,
        me: State::caller(),
    };
    let mut seen: Vec<&Issue> = state
        .index
        .values()
        .filter(|i| !reconcile.contains(&i.id))
        .chain(reconcile.iter().filter_map(|id| state.issues.get(id)))
        .filter(|i| state.issues.contains_key(&i.id) && query.matches(i, &ctx))
        .collect();
    query.sort(&mut seen, &ctx);
    seen.iter().map(|i| i.id).collect()
}

fn search(state: &mut State, call: &Call<'_>) -> HttpResponse {
    let params = match SearchParams::read(call, call.method == "POST") {
        Ok(p) => p,
        Err(r) => return r,
    };
    let query = match parse_jql(state, &params.jql) {
        Ok(q) => q,
        Err(r) => return r,
    };
    let jql_key = params.jql.trim().to_owned();
    let offset = match &params.token {
        None => 0,
        Some(token) => match state.page_tokens.get(token) {
            Some(t) if t.jql == jql_key => t.offset,
            _ => return bad_request("The provided next page token is invalid or expired."),
        },
    };
    let filter = FieldFilter::parse(&params.fields, FieldFilter::default());
    let cap = if filter.ids_only() { 5_000 } else { 100 };
    let max = params.max_results.unwrap_or(50).clamp(1, cap);
    let ids = matching(state, &query, &params.reconcile);
    let parts = BeanParts {
        fields: filter,
        properties: Some(params.properties).filter(|p| !p.is_empty()),
        transitions: params.expand.iter().any(|e| e == "transitions"),
    };
    let issues: Vec<Value> = ids
        .iter()
        .skip(offset)
        .take(max)
        .filter_map(|id| state.issues.get(id))
        .map(|i| render::issue_bean(state, i, &parts))
        .collect();
    let is_last = !call.repeat_token && offset + max >= ids.len();
    let mut token = if call.repeat_token {
        state.last_token.clone()
    } else {
        None
    };
    if !is_last && token.is_none() {
        let next = format!("{}-page-{}", state.site, state.next_token);
        state.next_token += 1;
        state.page_tokens.insert(
            next.clone(),
            PageToken {
                jql: jql_key,
                offset: offset + max,
            },
        );
        state.last_token = Some(next.clone());
        token = Some(next);
    }
    let mut out = json!({"issues": issues, "isLast": is_last});
    if let Some(token) = token {
        out["nextPageToken"] = Value::from(token);
    }
    reply(200, &out)
}

fn count(state: &State, call: &Call<'_>) -> HttpResponse {
    let body = match object_body(call.body) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let text = body.get("jql").and_then(Value::as_str).unwrap_or_default();
    match parse_jql(state, text) {
        Ok(query) => reply(200, &json!({"count": matching(state, &query, &[]).len()})),
        Err(r) => r,
    }
}

/// Adds and removes labels as a person does in the Jira UI.
pub(crate) fn edit_labels(state: &mut State, id: u64, add: &[&str], remove: &[&str], actor: &str) {
    let Some(old) = state.issues.get(&id).cloned() else {
        return;
    };
    let mut new = old.clone();
    for label in add {
        new.labels.insert((*label).to_owned());
    }
    for label in remove {
        new.labels.remove(*label);
    }
    commit_change(state, &old, new, actor, "issue_updated");
}
