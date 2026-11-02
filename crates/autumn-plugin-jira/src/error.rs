//! The plugin error type.

use autumn_jira::JiraError;

/// An error from the plugin. A message never holds a secret value.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// The `[jira]` config or the plugin setup is not valid.
    #[error("jira config: {0}")]
    Config(String),
    /// An env var that the config names is not set or is empty. The text
    /// names the variable, never a value.
    #[error("set the environment variable {env}")]
    MissingSecret {
        /// The name of the env var.
        env: String,
    },
    /// `[jira.san_mateo]` is set, and no `SanMateoPlugin` is registered.
    #[error("san_mateo mode needs SanMateoPlugin; register it before JiraPlugin")]
    SanMateoMissing,
    /// `SanMateoPlugin` is registered after `JiraPlugin`, so its service
    /// does not exist when the Jira plugin starts.
    #[error("SanMateoPlugin is registered after JiraPlugin; register it before")]
    SanMateoAfterJira,
    /// The `[[security.webhooks.endpoints]]` entry does not fit Jira.
    ///
    /// `fix` is a TOML block that fits. Its first line is a comment that
    /// names the problem.
    #[error("the webhook endpoint does not fit Jira; use:\n{fix}")]
    WebhookEndpoint {
        /// The TOML block that fits, with a comment that names the problem.
        fix: String,
    },
    /// Two settings of the plugin cannot be used together.
    #[error("{0} and {1} cannot be used together")]
    Conflict(&'static str, &'static str),
    /// A Jira value is not valid, for example the site name.
    #[error(transparent)]
    Jira(#[from] JiraError),
}

impl PluginError {
    /// Makes a config error.
    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}
