//! The plugin error type.

/// An error from the plugin. A message never holds a secret value.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// The `[jira]` config or the plugin setup is not valid.
    #[error("jira config: {0}")]
    Config(String),
    /// The `[[security.webhooks.endpoints]]` entry does not fit Jira.
    ///
    /// `fix` is a TOML block that fits. Its first line is a comment that
    /// names the problem.
    #[error("the webhook endpoint does not fit Jira; use:\n{fix}")]
    WebhookEndpoint {
        /// The TOML block that fits, with a comment that names the problem.
        fix: String,
    },
}

impl PluginError {
    /// Makes a config error.
    pub(crate) fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
}
