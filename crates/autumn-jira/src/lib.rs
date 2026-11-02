//! `autumn_jira`: a typed, safe Jira Cloud REST v3 client.
//!
//! The crate has strict ids, models, Atlassian Document Format (ADF) and a
//! JQL builder. It has typed errors with one class table. It has the webhook
//! types and the signature check. [`policy`] holds the verified retry core.
//!
//! The safe path is the default. A value always goes into JQL as one
//! literal, and an id that goes into a URL path matches a strict rule.
//! Webhooks are at least once: a sink must be idempotent.
//!
//! ```
//! use autumn_jira::jql::JqlField;
//! use autumn_jira::{Adf, IdempotencyKey, Jql, ProjectKey};
//!
//! # fn main() -> autumn_jira::Result<()> {
//! let project = ProjectKey::parse("OPS")?;
//! let jql = Jql::project(&project).and(Jql::field(JqlField::Labels).eq("x\" OR 1=1")?);
//! assert_eq!(jql.as_str(), r#"project = "OPS" AND labels = "x\" OR 1=1""#);
//!
//! let body = Adf::text("Disk full\nOn host db-1");
//! assert_eq!(body.to_plain_text(), "Disk full\nOn host db-1");
//!
//! let key = IdempotencyKey::derive(&["alert", "42"]);
//! assert!(key.marker_label("autumn-idem-").starts_with("autumn-idem-"));
//! # Ok(())
//! # }
//! ```
//!
//! See the crate README and `docs/plan.md`.

pub mod adf;
pub mod backend;
pub mod client;
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
pub use client::{ClientSettings, JiraClient};
pub use error::{JiraError, Result};
pub use ids::{AccountId, IdempotencyKey, IssueId, IssueKey, IssueRef, ProjectKey};
pub use jql::Jql;
pub use request::{NewIssue, StatusTarget};
pub use san_mateo;
pub use sink::{EventSink, HandlerError};
pub use site::{BasicAuth, JiraSite};
pub use webhook::{Delivery, JiraEvent};
