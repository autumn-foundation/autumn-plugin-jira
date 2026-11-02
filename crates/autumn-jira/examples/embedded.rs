//! Example `embedded`: the API token client on `FakeJira`.
//!
//! The run lists projects and issue types, creates, reads and edits an
//! issue, reads search pages, moves the issue to Done with a resolution,
//! adds a comment, does an idempotent create after a lost reply (503 after
//! the commit), and deletes an issue. `check` compares each value with the
//! expected value. No network: the fake is the transport.
//!
//! ```text
//! cargo run --locked -p autumn-jira --example embedded
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the example prints its summary"
)]

use std::error::Error;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use autumn_jira::client::{ClientSettings, CreateOutcome, DeleteOutcome};
use autumn_jira::ids::FieldId;
use autumn_jira::jql::{JqlField, Order};
use autumn_jira::request::{
    EditOptions, FieldSet, IssueEdit, IssueTypeRef, SearchRequest, TransitionFields,
};
use autumn_jira::{
    Adf, BasicAuth, IdempotencyKey, IssueKey, JiraClient, JiraSite, Jql, NewIssue, ProjectKey,
    StatusTarget,
};
use autumn_jira_fake::{FakeJira, FakeOp, Fault};

/// The longest time that one run can take.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// What the run saw.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    /// The project keys.
    projects: Vec<String>,
    /// The issue type names of OPS.
    issue_types: Vec<String>,
    /// The key of the first issue.
    created: String,
    /// The summary after the edit.
    title: String,
    /// The labels after the edit.
    labels: Vec<String>,
    /// The pages of the search.
    search_pages: u32,
    /// The keys of the search, in order.
    search_keys: Vec<String>,
    /// The status after the transitions.
    status: String,
    /// The resolution after the transitions.
    resolution: String,
    /// The comments of the first issue.
    comments: usize,
    /// The outcome of the idempotent create after the lost reply.
    idempotent: String,
    /// The create POSTs of the whole run.
    create_posts: usize,
    /// The outcome of the delete.
    deleted: String,
}

/// Runs the flow on a fake.
async fn run(fake: &FakeJira) -> Result<Summary, Box<dyn Error>> {
    let client = client_for(fake)?;
    let ops = ProjectKey::parse("OPS")?;
    let task =
        |summary: &str| NewIssue::new(ops.clone(), IssueTypeRef::Name("Task".to_owned()), summary);

    let projects = client.all_projects().await?;
    let issue_types = client.issue_types(&ops).await?;

    let first = task("Disk full on db-1")
        .description(Adf::text(
            "The disk of db-1 is at 95%.\nThe alert fired at 03:00.",
        ))
        .labels(["alert"]);
    let created = client.create_issue(&first).await?;
    let edit = IssueEdit::new()
        .set_summary("Disk full on db-1 (95%)")
        .add_label("triaged");
    client
        .edit_issue(&created.key, &edit, EditOptions::notify_users(false))
        .await?;
    let edited = client.get_issue(&created.key).await?;

    client.create_issue(&task("Second issue")).await?;
    client.create_issue(&task("Third issue")).await?;
    let request = SearchRequest::new(Jql::project(&ops).order_by(JqlField::Key, Order::Asc))
        .fields(FieldSet::new([FieldId::parse("summary")?]))
        .max_results(2);
    let mut pager = client.search(request);
    let mut search_keys = Vec::new();
    while let Some(page) = pager.next_page().await? {
        search_keys.extend(page.into_iter().map(|i| i.key.to_string()));
    }

    client
        .transition_to_status(&created.key, &StatusTarget::in_progress())
        .await?;
    let resolve = TransitionFields::default().resolution("Done");
    client
        .transition_to_status_with(&created.key, &StatusTarget::done(), &resolve)
        .await?;
    let resolution = FieldId::parse("resolution")?;
    let done = client
        .get_issue_with(&created.key, &FieldSet::default().with(resolution.clone()))
        .await?;

    client
        .add_comment(&created.key, &Adf::text("Freed 20 GB. Closing."))
        .await?;
    let comments = client.all_comments(&created.key).await?;

    // The write happens, and the reply is lost. The marker lookup finds it.
    fake.fault(FakeOp::CreateIssue, Fault::StatusAfterCommit(503));
    let key = IdempotencyKey::derive(&["alert", "db-1", "disk"]);
    let idempotent = match client
        .create_issue_idempotent(&task("Disk alert"), &key)
        .await?
    {
        CreateOutcome::Created(c) => format!("created {}", c.key),
        CreateOutcome::Found(f) => format!("found {}", f.key),
    };
    let fourth = IssueKey::parse("OPS-4")?;
    let deleted = match client.delete_issue(&fourth, false).await? {
        DeleteOutcome::Deleted => "deleted",
        DeleteOutcome::GoneAfterRetry => "gone after retry",
    };

    Ok(Summary {
        projects: projects.iter().map(|p| p.key.to_string()).collect(),
        issue_types: issue_types.iter().map(|t| t.name.clone()).collect(),
        created: created.key.to_string(),
        title: edited.fields.summary.clone().unwrap_or_default(),
        labels: edited.fields.labels.clone(),
        search_pages: pager.pages(),
        search_keys,
        status: done.status().map(|s| s.name).unwrap_or_default(),
        resolution: done
            .field(&resolution)
            .and_then(|r| r.get("name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        comments: comments.len(),
        idempotent,
        create_posts: fake.count(FakeOp::CreateIssue),
        deleted: deleted.to_owned(),
    })
}

/// Makes the client of the example: the API token client with short waits.
fn client_for(fake: &FakeJira) -> Result<JiraClient, Box<dyn Error>> {
    let site = JiraSite::Subdomain(autumn_jira::ids::SiteName::parse(
        autumn_jira_fake::DEFAULT_SITE,
    )?);
    let auth = BasicAuth::new(
        autumn_jira_fake::DEFAULT_EMAIL,
        autumn_jira::san_mateo::Secret::new(autumn_jira_fake::DEFAULT_API_TOKEN),
    )?;
    let mut settings = ClientSettings::default();
    settings.base_backoff_ms = 50;
    settings.lookup_delay_ms = 100;
    Ok(JiraClient::api_token(site, auth, Arc::new(fake.clone())).with_settings(settings))
}

/// The expected summary.
fn expected() -> Summary {
    let s = |v: &str| v.to_owned();
    Summary {
        projects: vec![s("OPS")],
        issue_types: vec![s("Task"), s("Incident"), s("Bug")],
        created: s("OPS-1"),
        title: s("Disk full on db-1 (95%)"),
        labels: vec![s("alert"), s("triaged")],
        search_pages: 2,
        search_keys: vec![s("OPS-1"), s("OPS-2"), s("OPS-3")],
        status: s("Done"),
        resolution: s("Done"),
        comments: 1,
        idempotent: s("found OPS-4"),
        create_posts: 4,
        deleted: s("deleted"),
    }
}

/// Compares a summary with the expected values.
fn check(summary: &Summary) -> Result<(), String> {
    let want = expected();
    if summary == &want {
        Ok(())
    } else {
        Err(format!("check failed: got {summary:?}, want {want:?}"))
    }
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("no runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let fake = FakeJira::new();
    let result = runtime.block_on(async { tokio::time::timeout(RUN_TIMEOUT, run(&fake)).await });
    let summary = match result {
        Ok(Ok(summary)) => summary,
        Ok(Err(e)) => {
            eprintln!("run failed: {e}");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            eprintln!("run failed: no result in {RUN_TIMEOUT:?}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = check(&summary) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    println!("projects:       {}", summary.projects.join(", "));
    println!("issue types:    {}", summary.issue_types.join(", "));
    println!("created:        {}", summary.created);
    println!("title:          {}", summary.title);
    println!("labels:         {}", summary.labels.join(", "));
    println!(
        "search:         {} page(s): {}",
        summary.search_pages,
        summary.search_keys.join(", ")
    );
    println!(
        "status:         {} ({})",
        summary.status, summary.resolution
    );
    println!("comments:       {}", summary.comments);
    println!("idempotent:     {} after a lost reply", summary.idempotent);
    println!("create POSTs:   {}", summary.create_posts);
    println!("delete:         {}", summary.deleted);
    println!("check: ok");
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use autumn_jira::JiraError;

    #[tokio::test]
    async fn the_example_runs_and_checks() {
        let summary = tokio::time::timeout(RUN_TIMEOUT, run(&FakeJira::new()))
            .await
            .unwrap()
            .unwrap();
        check(&summary).unwrap();
    }

    #[tokio::test]
    async fn a_bad_field_is_a_validation_error() {
        let fake = FakeJira::new();
        let client = client_for(&fake).unwrap();
        let issue = NewIssue::new(
            ProjectKey::parse("OPS").unwrap(),
            IssueTypeRef::Name("Task".to_owned()),
            "x",
        )
        .field(FieldId::parse("nonsense").unwrap(), serde_json::json!(1));
        let err = client
            .create_issue(&issue)
            .await
            .expect_err("a bad field fails");
        assert!(
            matches!(&err, JiraError::Validation { status: 400, fields, .. } if fields == &["nonsense"]),
            "{err:?}"
        );
        assert_eq!(fake.issues().len(), 0);
    }

    #[test]
    fn a_wrong_value_fails_the_check() {
        let mut summary = expected();
        summary.status = "In Progress".to_owned();
        let err = check(&summary).expect_err("a wrong value fails");
        assert!(err.starts_with("check failed:"), "{err}");
        assert!(check(&expected()).is_ok());
    }
}
