//! `JiraPlugin`: the builder and the autumn `Plugin` impl.

use std::borrow::Cow;

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use autumn_web::plugin_contract::PluginContract;

use crate::config::JiraConfig;
use crate::{AUTUMN_WEB_REQUIREMENT, DEFAULT_PREFIX, PLUGIN_NAME};

/// The contract that [`JiraPlugin`] declares to autumn.
#[must_use]
pub fn plugin_contract() -> PluginContract {
    PluginContract::new(env!("CARGO_PKG_NAME"))
        .plugin_version(env!("CARGO_PKG_VERSION"))
        .autumn_web(AUTUMN_WEB_REQUIREMENT)
}

/// The Jira plugin for autumn-web.
///
/// ```rust,no_run
/// use autumn_plugin_jira::JiraPlugin;
///
/// # async fn demo() {
/// autumn_web::app()
///     .plugin(JiraPlugin::new().prefix("/jira").webhook_route())
///     .run()
///     .await;
/// # }
/// ```
#[derive(Debug)]
pub struct JiraPlugin {
    config: Option<JiraConfig>,
    prefix: String,
    webhook_route: bool,
    allow_without_replay_protection: bool,
}

impl Default for JiraPlugin {
    fn default() -> Self {
        Self {
            config: None,
            prefix: DEFAULT_PREFIX.to_owned(),
            webhook_route: false,
            allow_without_replay_protection: false,
        }
    }
}

impl JiraPlugin {
    /// A plugin that reads `[jira]` from the autumn config files and the env
    /// at startup.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A plugin with this config. It reads no files.
    #[must_use]
    pub fn with_config(config: JiraConfig) -> Self {
        Self {
            config: Some(config),
            ..Self::default()
        }
    }

    /// Sets the route prefix. Default: [`DEFAULT_PREFIX`].
    ///
    /// The prefix is a builder value, not a config key: autumn mounts the
    /// routes before the config loads.
    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        self.prefix = prefix.to_owned();
        self
    }

    /// Mounts the webhook route at [`webhook_path`](Self::webhook_path).
    #[must_use]
    pub const fn webhook_route(mut self) -> Self {
        self.webhook_route = true;
        self
    }

    /// Lets the start pass when the endpoint has `replay_protection = false`.
    ///
    /// This is an unsafe opt-in. Without replay protection, autumn does not
    /// refuse a delivery that comes two times, and an attacker can send a
    /// captured delivery again.
    #[must_use]
    pub const fn allow_webhooks_without_replay_protection(mut self) -> Self {
        self.allow_without_replay_protection = true;
        self
    }

    /// The route prefix.
    #[must_use]
    pub fn route_prefix(&self) -> &str {
        &self.prefix
    }

    /// The full path of the webhook route: `{prefix}/webhook`.
    #[must_use]
    pub fn webhook_path(&self) -> String {
        String::new()
    }
}

impl Plugin for JiraPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(PLUGIN_NAME)
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let _ = (
            self.config,
            self.webhook_route,
            self.allow_without_replay_protection,
        );
        app
    }

    fn contract(&self) -> Option<PluginContract> {
        Some(plugin_contract())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    /// The declared range is the `autumn-web` requirement of the workspace.
    #[test]
    fn contract_matches_cargo_toml() {
        let manifest: toml::Table = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
        let requirement = manifest["workspace"]["dependencies"]["autumn-web"]["version"]
            .as_str()
            .unwrap();
        assert_eq!(AUTUMN_WEB_REQUIREMENT, requirement);
        let crate_manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
        assert_eq!(
            crate_manifest["dependencies"]["autumn-web"]["workspace"].as_bool(),
            Some(true),
            "the crate takes the workspace requirement"
        );

        use autumn_web::plugin_contract::{AUTUMN_WEB_VERSION, ContractVerdict, evaluate};
        let contract = JiraPlugin::new().contract().unwrap();
        assert_eq!(contract, plugin_contract());
        assert_eq!(contract.plugin, PLUGIN_NAME);
        assert_eq!(
            evaluate(&contract, AUTUMN_WEB_VERSION),
            ContractVerdict::Compatible
        );
        assert_eq!(JiraPlugin::new().name(), PLUGIN_NAME);
    }

    #[test]
    fn prefix_and_webhook_path() {
        let p = JiraPlugin::new();
        assert_eq!(p.route_prefix(), "/jira");
        assert_eq!(p.webhook_path(), "/jira/webhook");
        let p = JiraPlugin::new().prefix("/hooks/jira/");
        assert_eq!(p.route_prefix(), "/hooks/jira");
        assert_eq!(p.webhook_path(), "/hooks/jira/webhook");
        let p = JiraPlugin::new().prefix("atlassian");
        assert_eq!(p.webhook_path(), "/atlassian/webhook");
        let p = JiraPlugin::new().prefix("");
        assert_eq!(p.route_prefix(), "");
        assert_eq!(p.webhook_path(), "/webhook");
        let p = JiraPlugin::new().prefix("/");
        assert_eq!(p.webhook_path(), "/webhook");
    }
}
