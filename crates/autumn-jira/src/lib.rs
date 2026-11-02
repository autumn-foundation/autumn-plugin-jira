//! `autumn_jira`: a typed, safe Jira Cloud REST v3 client.
//!
//! See the crate README and `docs/plan.md`.

pub mod adf;
pub mod error;
pub mod ids;
pub mod jql;
pub mod policy;
pub mod site;
pub mod time;

pub use adf::Adf;
pub use error::{JiraError, Result};
pub use ids::{AccountId, IdempotencyKey, IssueId, IssueKey, IssueRef, ProjectKey};
pub use jql::Jql;
pub use san_mateo;
pub use site::{BasicAuth, JiraSite};
