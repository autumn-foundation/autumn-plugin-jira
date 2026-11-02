//! The client conformance suite (W11). Each case runs on each backend
//! setup: API token, san-mateo `jira-basic`, san-mateo `atlassian`.

use std::future::Future;

use autumn_jira::client::{
    ClientSettings, CommentOutcome, CreateOutcome, DeleteOutcome, TransitionOutcome,
};
use autumn_jira::error::Op;
use autumn_jira::ids::{FieldId, IssueTypeId, StatusId, TransitionId};
use autumn_jira::jql::{JqlField, Order};
use autumn_jira::request::{
    EditOptions, FieldSet, IssueEdit, IssueTypeRef, SearchRequest, TransitionFields,
};
use autumn_jira::{
    Adf, IdempotencyKey, IssueKey, JiraClient, JiraError, Jql, NewIssue, ProjectKey, StatusTarget,
};
use autumn_jira_fake::{ANN, BOT, FakeJira, FakeOp, FakeProject, Fault};
use serde_json::json;

/// Runs every suite case. `make` turns a fresh fake into a client. A case
/// can set up the fake after `make`: clones of a fake share their state.
pub async fn run_client_suite<F, Fut>(make: F)
where
    F: Fn(FakeJira) -> Fut,
    Fut: Future<Output = JiraClient>,
{
    macro_rules! case {
        ($($name:ident),* $(,)?) => {$(
            let fake = FakeJira::new();
            let client = make(fake.clone()).await;
            $name(&fake, &client).await;
        )*};
    }
    case!(
        myself_and_server_info,
        projects_and_one_project,
        create_get_edit_delete,
        notify_users_only_when_set,
        delete_gone_after_retry,
        properties_round_trip,
        search_pages_to_the_end,
        search_stalls_on_a_repeated_token,
        search_page_limit,
        comments_offset_pages,
        projects_list_all_pages,
        issue_types_of_a_project,
        approximate_count,
        transition_by_category,
        already_there_sends_no_post,
        no_transition_lists_names,
        done_needs_resolution_fields,
        add_and_list_comments,
        all_comments_is_bounded,
        idempotent_writes_round_trip,
    );
}

fn ops() -> ProjectKey {
    ProjectKey::parse("OPS").unwrap()
}

fn task(summary: &str) -> NewIssue {
    NewIssue::new(ops(), IssueTypeRef::Name("Task".to_owned()), summary)
}

fn with(client: &JiraClient, change: impl FnOnce(&mut ClientSettings)) -> JiraClient {
    let mut settings = client.settings().clone();
    change(&mut settings);
    client.clone().with_settings(settings)
}

async fn new_issue(client: &JiraClient, summary: &str) -> IssueKey {
    client.create_issue(&task(summary)).await.unwrap().key
}

async fn myself_and_server_info(_fake: &FakeJira, client: &JiraClient) {
    let me = client.myself().await.unwrap();
    assert_eq!(me.account_id.as_str(), BOT);
    assert_eq!(client.account_id().await.unwrap().as_str(), BOT);
    let info = client.server_info().await.unwrap();
    assert_eq!(info.base_url.as_deref(), Some("https://acme.atlassian.net"));
    assert!(!format!("{client:?}").contains("fake-api-token"));
}

async fn projects_and_one_project(_fake: &FakeJira, client: &JiraClient) {
    let project = client.get_project(&ops()).await.unwrap();
    assert_eq!(
        (project.key.as_str(), project.name.as_str()),
        ("OPS", "Operations")
    );
    let missing = client
        .get_project(&ProjectKey::parse("NOPE").unwrap())
        .await
        .unwrap_err();
    assert!(matches!(
        missing,
        JiraError::NotFoundOrForbidden { op: Op::GetProject }
    ));
    let page = client.projects(0, 10).await.unwrap();
    assert_eq!(page.values.len(), 1);
    assert!(page.is_last);
}

async fn create_get_edit_delete(fake: &FakeJira, client: &JiraClient) {
    let custom = FieldId::parse("customfield_10010").unwrap();
    let issue = task("Disk full")
        .description(Adf::text("On db-1\nAt 03:00"))
        .labels(["alert", "db"])
        .field(custom.clone(), json!({"value": "High"}))
        .property("app-data", json!({"alert": 42}));
    let created = client.create_issue(&issue).await.unwrap();
    assert_eq!(created.key.as_str(), "OPS-1");
    // AC6: the create body sets the user property.
    assert_eq!(
        client
            .get_issue_property(&created.key, "app-data")
            .await
            .unwrap(),
        Some(json!({"alert": 42}))
    );

    let got = client.get_issue(&created.key).await.unwrap();
    assert_eq!((&got.id, &got.key), (&created.id, &created.key));
    assert_eq!(got.fields.summary.as_deref(), Some("Disk full"));
    assert_eq!(got.fields.labels, ["alert", "db"]);
    assert_eq!(
        got.fields.description.as_ref().unwrap().to_plain_text(),
        "On db-1\nAt 03:00"
    );
    assert_eq!(got.status().unwrap().name, "To Do");
    assert_eq!(got.fields.issue_type.as_ref().unwrap().name, "Task");
    // The default field set does not ask for the custom field.
    assert_eq!(got.field(&custom), None);
    let explicit = client
        .get_issue_with(
            &created.id,
            &FieldSet::new([FieldId::parse("summary").unwrap()]).with(custom.clone()),
        )
        .await
        .unwrap();
    assert_eq!(explicit.fields.summary.as_deref(), Some("Disk full"));
    assert_eq!(explicit.fields.status, None);
    assert_eq!(explicit.field(&custom), Some(&json!({"value": "High"})));
    let last = fake.requests().pop().unwrap();
    assert!(
        last.query.contains("fields=summary%2Ccustomfield_10010"),
        "{}",
        last.query
    );

    let edit = IssueEdit::new()
        .set_summary("Disk full on db-1")
        .add_label("triaged")
        .remove_label("db");
    client
        .edit_issue(&created.key, &edit, EditOptions::default())
        .await
        .unwrap();
    let edited = client.get_issue(&created.key).await.unwrap();
    assert_eq!(edited.fields.summary.as_deref(), Some("Disk full on db-1"));
    assert_eq!(edited.fields.labels, ["alert", "triaged"]);
    // An edit with a bad field is a typed validation error.
    let bad = IssueEdit::new().set_field(FieldId::parse("nonsense").unwrap(), json!(1));
    let err = client
        .edit_issue(&created.key, &bad, EditOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, JiraError::Validation { status: 400, fields, .. } if fields == &["nonsense"])
    );

    assert_eq!(
        client.delete_issue(&created.key, false).await.unwrap(),
        DeleteOutcome::Deleted
    );
    let last = fake.requests().pop().unwrap();
    assert_eq!(
        (last.method.as_str(), last.query.as_str()),
        ("DELETE", "deleteSubtasks=false")
    );
    // A 404 on the first attempt is not "gone after retry".
    let again = client.delete_issue(&created.key, true).await.unwrap_err();
    assert!(matches!(
        again,
        JiraError::NotFoundOrForbidden {
            op: Op::DeleteIssue
        }
    ));
    assert!(fake.issue("OPS-1").is_none());
}

async fn notify_users_only_when_set(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "n").await;
    let edit = IssueEdit::new().set_summary("n2");
    client
        .edit_issue(&key, &edit, EditOptions::default())
        .await
        .unwrap();
    assert_eq!(fake.requests().pop().unwrap().query, "");
    client
        .edit_issue(&key, &edit, EditOptions::notify_users(false))
        .await
        .unwrap();
    assert_eq!(fake.requests().pop().unwrap().query, "notifyUsers=false");
}

async fn delete_gone_after_retry(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "d").await;
    // The delete happens and the reply is lost. The retry gets 404.
    fake.fault(FakeOp::DeleteIssue, Fault::StatusAfterCommit(503));
    assert_eq!(
        client.delete_issue(&key, false).await.unwrap(),
        DeleteOutcome::GoneAfterRetry
    );
    assert_eq!(fake.count(FakeOp::DeleteIssue), 2);
}

async fn properties_round_trip(_fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "p").await;
    assert_eq!(
        client.get_issue_property(&key, "app.route").await.unwrap(),
        None
    );
    let value = json!({"workflow": "w1", "tags": [1, 2]});
    client
        .set_issue_property(&key, "app.route", &value)
        .await
        .unwrap();
    // A second PUT replaces the value (200 in place of 201).
    client
        .set_issue_property(&key, "app.route", &value)
        .await
        .unwrap();
    assert_eq!(
        client.get_issue_property(&key, "app.route").await.unwrap(),
        Some(value)
    );
    let empty = client
        .set_issue_property(&key, "", &json!(1))
        .await
        .unwrap_err();
    assert!(matches!(empty, JiraError::InvalidInput { .. }));
}

async fn search_pages_to_the_end(fake: &FakeJira, client: &JiraClient) {
    for n in 0..5 {
        new_issue(client, &format!("s{n}")).await;
    }
    let request = SearchRequest::new(Jql::project(&ops()).order_by(JqlField::Key, Order::Asc))
        .fields(FieldSet::new([FieldId::parse("summary").unwrap()]))
        .max_results(2);
    let mut pager = client.search(request.clone());
    let mut keys = Vec::new();
    while let Some(page) = pager.next_page().await.unwrap() {
        keys.extend(page.into_iter().map(|i| i.key.to_string()));
    }
    assert_eq!(keys, ["OPS-1", "OPS-2", "OPS-3", "OPS-4", "OPS-5"]);
    assert_eq!(pager.pages(), 3);
    assert_eq!(pager.next_page().await.unwrap(), None);
    // Each search is a POST to /search/jql.
    assert!(
        fake.requests()
            .iter()
            .filter(|r| r.path.ends_with("/search/jql"))
            .all(|r| r.method == "POST")
    );
    let some = client.search_all(request.clone(), 3).await.unwrap();
    assert_eq!(some.len(), 3);
    assert_eq!(client.search_all(request, 100).await.unwrap().len(), 5);
}

async fn search_stalls_on_a_repeated_token(fake: &FakeJira, client: &JiraClient) {
    for n in 0..5 {
        new_issue(client, &format!("r{n}")).await;
    }
    let mut pager = client.search(SearchRequest::new(Jql::project(&ops())).max_results(2));
    assert_eq!(pager.next_page().await.unwrap().unwrap().len(), 2);
    fake.fault(FakeOp::Search, Fault::RepeatToken);
    let err = pager.next_page().await.unwrap_err();
    assert_eq!(
        err,
        JiraError::PaginationStalled {
            op: Op::Search,
            pages: 2
        }
    );
    assert_eq!(pager.next_page().await.unwrap(), None);
}

async fn search_page_limit(_fake: &FakeJira, client: &JiraClient) {
    for n in 0..5 {
        new_issue(client, &format!("l{n}")).await;
    }
    let small = with(client, |s| s.max_pages = 2);
    let mut pager = small.search(SearchRequest::new(Jql::project(&ops())).max_results(2));
    assert!(pager.next_page().await.unwrap().is_some());
    let err = pager.next_page().await.unwrap_err();
    assert_eq!(
        err,
        JiraError::PageLimit {
            op: Op::Search,
            max: 2
        }
    );
    assert_eq!(pager.pages(), 2);
}

async fn comments_offset_pages(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "c").await;
    for n in 0..3 {
        fake.human_comment(key.as_str(), &format!("note {n}"), ANN)
            .unwrap();
    }
    let page = client.comments(&key, 1, 1).await.unwrap();
    assert_eq!((page.start_at, page.total, page.comments.len()), (1, 3, 1));
    let text = page.comments[0].body.as_ref().unwrap().to_plain_text();
    assert_eq!(text, "note 1");
    let all = client.all_comments(&key).await.unwrap();
    let texts: Vec<String> = all
        .iter()
        .map(|c| c.body.as_ref().unwrap().to_plain_text())
        .collect();
    assert_eq!(texts, ["note 0", "note 1", "note 2"]);
    assert_eq!(all[0].author.as_ref().unwrap().account_id.as_str(), ANN);
}

async fn projects_list_all_pages(fake: &FakeJira, client: &JiraClient) {
    let mut fake = fake.clone();
    for n in 0..54 {
        let key = format!("P{n:02}");
        fake = fake.with_project(
            FakeProject::new(&key, &format!("{}", 20_000 + n), "Extra").issue_type("1", "Task"),
        );
    }
    let all = client.all_projects().await.unwrap();
    assert_eq!(all.len(), 55);
    assert_eq!(fake.count(FakeOp::ProjectSearch), 2);
    let one_page = with(client, |s| s.max_pages = 1);
    let err = one_page.all_projects().await.unwrap_err();
    assert_eq!(
        err,
        JiraError::PageLimit {
            op: Op::ListProjects,
            max: 1
        }
    );
}

async fn issue_types_of_a_project(fake: &FakeJira, client: &JiraClient) {
    issue_types_list_all_pages(fake, client).await;
    let types = client.issue_types(&ops()).await.unwrap();
    let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Task", "Incident", "Bug"]);
    assert_eq!(types[0].id, IssueTypeId::parse("10001").unwrap());
    let by_id = NewIssue::new(ops(), IssueTypeRef::Id(types[2].id.clone()), "bug");
    let created = client.create_issue(&by_id).await.unwrap();
    let issue = client.get_issue(&created.key).await.unwrap();
    assert_eq!(issue.fields.issue_type.unwrap().name, "Bug");
}

/// AC7: the issue types (createmeta) of a project come from all pages.
async fn issue_types_list_all_pages(fake: &FakeJira, client: &JiraClient) {
    let mut big = FakeProject::new("BIG", "30000", "Big");
    for n in 0..53 {
        big = big.issue_type(&format!("{}", 40_000 + n), &format!("Type {n:02}"));
    }
    let fake = fake.clone().with_project(big);
    let key = ProjectKey::parse("BIG").unwrap();
    let before = fake.count(FakeOp::CreateMeta);
    let types = client.issue_types(&key).await.unwrap();
    assert_eq!(types.len(), 53);
    assert_eq!(types[0].name, "Type 00");
    assert_eq!(types[52].name, "Type 52");
    assert_eq!(types[52].id, IssueTypeId::parse("40052").unwrap());
    assert_eq!(
        fake.count(FakeOp::CreateMeta) - before,
        2,
        "two pages of 50"
    );
    let one_page = with(client, |s| s.max_pages = 1);
    let err = one_page.issue_types(&key).await.unwrap_err();
    assert_eq!(
        err,
        JiraError::PageLimit {
            op: Op::ListIssueTypes,
            max: 1
        }
    );
}

async fn approximate_count(_fake: &FakeJira, client: &JiraClient) {
    for n in 0..3 {
        new_issue(client, &format!("a{n}")).await;
    }
    assert_eq!(
        client
            .approximate_count(&Jql::project(&ops()))
            .await
            .unwrap(),
        3
    );
    let none = Jql::project(&ops()).and(Jql::field(JqlField::Labels).eq("none").unwrap());
    assert_eq!(client.approximate_count(&none).await.unwrap(), 0);
}

async fn transition_by_category(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "t").await;
    let list = client.transitions(&key).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Start work");
    let outcome = client
        .transition_to_status(&key, &StatusTarget::in_progress())
        .await
        .unwrap();
    let TransitionOutcome::Moved { via, to } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(via, TransitionId::parse("21").unwrap());
    assert_eq!(to.name, "In Progress");
    // By status name (any case) and by status id.
    let resolve = TransitionFields::default().resolution("Done");
    let done = client
        .transition_to_status_with(&key, &StatusTarget::Name("done".to_owned()), &resolve)
        .await
        .unwrap();
    assert!(matches!(done, TransitionOutcome::Moved { .. }));
    let back = client
        .transition_to_status(&key, &StatusTarget::Id(StatusId::parse("10000").unwrap()))
        .await
        .unwrap();
    assert!(matches!(back, TransitionOutcome::Moved { to, .. } if to.name == "To Do"));
    assert_eq!(fake.count(FakeOp::DoTransition), 3);
    // A plain transition by id.
    client
        .transition(
            &key,
            &TransitionId::parse("21").unwrap(),
            &TransitionFields::default(),
        )
        .await
        .unwrap();
    assert_eq!(
        client.get_issue(&key).await.unwrap().status().unwrap().name,
        "In Progress"
    );
}

async fn already_there_sends_no_post(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "a").await;
    let outcome = client
        .transition_to_status(&key, &StatusTarget::todo())
        .await
        .unwrap();
    assert!(matches!(outcome, TransitionOutcome::AlreadyThere(s) if s.name == "To Do"));
    assert_eq!(fake.count(FakeOp::DoTransition), 0);
    assert_eq!(fake.count(FakeOp::GetTransitions), 0);
}

async fn no_transition_lists_names(_fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "x").await;
    // To Do has only "Start work"; Done is two steps away.
    let err = client
        .transition_to_status(&key, &StatusTarget::done())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        JiraError::TransitionNotAvailable {
            target: StatusTarget::done().to_string(),
            available: vec!["Start work".to_owned()],
        }
    );
    let unknown = client
        .transition_to_status(&key, &StatusTarget::Name("Nowhere".to_owned()))
        .await
        .unwrap_err();
    assert!(matches!(unknown, JiraError::TransitionNotAvailable { .. }));
}

async fn done_needs_resolution_fields(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "r").await;
    client
        .transition_to_status(&key, &StatusTarget::in_progress())
        .await
        .unwrap();
    let missing = client
        .transition_to_status(&key, &StatusTarget::done())
        .await
        .unwrap_err();
    assert!(
        matches!(
            missing,
            JiraError::Validation {
                op: Op::DoTransition,
                ..
            }
        ),
        "{missing:?}"
    );
    let fields = TransitionFields::default().resolution("Done");
    client
        .transition_to_status_with(&key, &StatusTarget::done(), &fields)
        .await
        .unwrap();
    let snapshot = fake.snapshot();
    assert_eq!(snapshot["issues"][0]["status"], "Done");
    assert_eq!(snapshot["issues"][0]["resolution"], "Done");
}

async fn add_and_list_comments(_fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "c").await;
    let body = Adf::builder()
        .paragraph("First line")
        .bullet_list(&["a", "b"])
        .build()
        .unwrap();
    let comment = client.add_comment(&key, &body).await.unwrap();
    assert!(comment.body.as_ref().unwrap().contains_text("First line"));
    assert_eq!(comment.author.as_ref().unwrap().account_id.as_str(), BOT);
    let page = client.comments(&key, 0, 50).await.unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.comments[0].id, comment.id);
}

async fn all_comments_is_bounded(fake: &FakeJira, client: &JiraClient) {
    let key = new_issue(client, "many").await;
    for n in 0..101 {
        fake.human_comment(key.as_str(), &format!("c{n}"), ANN)
            .unwrap();
    }
    assert_eq!(client.all_comments(&key).await.unwrap().len(), 101);
    let one_page = with(client, |s| s.max_pages = 1);
    let err = one_page.all_comments(&key).await.unwrap_err();
    assert_eq!(
        err,
        JiraError::PageLimit {
            op: Op::ListComments,
            max: 1
        }
    );
}

async fn idempotent_writes_round_trip(fake: &FakeJira, client: &JiraClient) {
    let key = IdempotencyKey::derive(&["suite", "1"]);
    let first = client
        .create_issue_idempotent(&task("idem"), &key)
        .await
        .unwrap();
    let CreateOutcome::Created(created) = first else {
        panic!("{first:?}");
    };
    let again = client
        .create_issue_idempotent(&task("idem"), &key)
        .await
        .unwrap();
    assert!(matches!(again, CreateOutcome::Found(r) if r.key == created.key));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    let found = client.find_by_idempotency_key(&ops(), &key).await.unwrap();
    assert_eq!(found.unwrap().id, created.id);

    let body = Adf::text("Resolved by the bot");
    let c1 = client
        .add_comment_idempotent(&created.key, &body, &key)
        .await
        .unwrap();
    assert!(matches!(c1, CommentOutcome::Created(_)));
    let c2 = client
        .add_comment_idempotent(&created.key, &body, &key)
        .await
        .unwrap();
    assert!(matches!(c2, CommentOutcome::Found(_)));
    assert_eq!(fake.count(FakeOp::AddComment), 1);
}
