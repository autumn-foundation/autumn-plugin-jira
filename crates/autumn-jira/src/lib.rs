//! `autumn_jira`: a typed, safe Jira Cloud REST v3 client.
//!
//! See the crate README and `docs/plan.md`.

pub mod adf;
pub mod error;
pub mod ids;
pub mod jql;
pub mod model;
pub mod policy;
pub mod request;
pub mod sink;
pub mod site;
pub mod time;
pub mod webhook;
pub mod writes;

pub use adf::Adf;
pub use error::{JiraError, Result};
pub use ids::{AccountId, IdempotencyKey, IssueId, IssueKey, IssueRef, ProjectKey};
pub use jql::Jql;
pub use request::{NewIssue, StatusTarget};
pub use san_mateo;
pub use sink::{EventSink, HandlerError};
pub use site::{BasicAuth, JiraSite};
pub use webhook::{Delivery, JiraEvent};
