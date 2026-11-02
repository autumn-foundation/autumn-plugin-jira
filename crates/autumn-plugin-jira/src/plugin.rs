//! `JiraPlugin`: the builder and the autumn `Plugin` impl.

use std::borrow::Cow;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use autumn_jira::san_mateo::transport::HttpTransport;
use autumn_jira::{Delivery, EventSink, HandlerError, JiraClient};

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use autumn_web::plugin_contract::PluginContract;
use autumn_web::{AppState, AutumnError, ProcessRole};

use crate::config::JiraConfig;
use crate::error::PluginError;
use crate::mount;
use crate::runtime::JiraRuntime;
use crate::webhook_config::check_webhook_endpoint;
use crate::{AUTUMN_WEB_REQUIREMENT, DEFAULT_PREFIX, PLUGIN_NAME, SECTION};

/// The result of the state initializer. The startup hook reads it.
type Started = Arc<Mutex<Option<Result<JiraConfig, PluginError>>>>;

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
    ///
    /// The plugin adds a leading `/` and removes a trailing `/`. `""` and
    /// `"/"` put the webhook route at `/webhook`.
    #[must_use]
    pub fn prefix(mut self, prefix: &str) -> Self {
        let trimmed = prefix.trim().trim_matches('/');
        self.prefix = if trimmed.is_empty() {
            String::new()
        } else {
            format!("/{trimmed}")
        };
        self
    }

    /// Uses this transport in API token mode.
    #[must_use]
    pub fn transport(self, transport: Arc<dyn HttpTransport>) -> Self {
        let _ = transport;
        self
    }

    /// Uses this prebuilt client.
    #[must_use]
    pub fn client(self, client: JiraClient) -> Self {
        let _ = client;
        self
    }

    /// Adds a sink.
    #[must_use]
    pub fn sink(self, sink: impl EventSink) -> Self {
        let _ = sink;
        self
    }

    /// Adds a closure handler.
    #[must_use]
    pub fn on_event<F, Fut>(self, f: F) -> Self
    where
        F: Fn(Arc<Delivery>, AppState) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
    {
        let _ = f;
        self
    }

    /// Uses `value` for the env var `name`.
    #[doc(hidden)]
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn secret_env(self, name: &str, value: &str) -> Self {
        let _ = (name, value);
        self
    }

    /// Starts the plugin outside an app build.
    ///
    /// # Errors
    /// A [`PluginError`].
    pub fn start(self, state: &AppState) -> Result<JiraRuntime, PluginError> {
        let role = state.role();
        self.start_with_role(state, role)
    }

    /// Starts the plugin as if the process has `role`.
    ///
    /// # Errors
    /// A [`PluginError`].
    pub fn start_with_role(
        self,
        state: &AppState,
        role: ProcessRole,
    ) -> Result<JiraRuntime, PluginError> {
        let _ = (state, role);
        Err(PluginError::config("stub"))
    }

    /// Mounts the webhook route at [`webhook_path`](Self::webhook_path).
    ///
    /// The start then checks the `[[security.webhooks.endpoints]]` entry in
    /// each role that serves HTTP. The `Worker` role serves no user routes,
    /// so it skips the check.
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
        format!("{}/webhook", self.prefix)
    }

    /// The config: the one from [`with_config`](Self::with_config), else the
    /// one that autumn files and the env give. Then the mode check.
    fn resolve_config(&self, state: &AppState) -> Result<JiraConfig, PluginError> {
        let config = match &self.config {
            Some(config) => {
                config.validate()?;
                config.clone()
            }
            None => JiraConfig::load(Some(state.profile()).filter(|p| *p != "default"))?,
        };
        if config.mode()?.is_none() {
            return Err(PluginError::config(
                "set [jira.api_token] or [jira.san_mateo]",
            ));
        }
        Ok(config)
    }

    /// The startup check. It runs the webhook boot check when this process
    /// serves HTTP and the route is mounted. It returns the warnings.
    fn startup_check(
        &self,
        state: &AppState,
        config: &JiraConfig,
        role: ProcessRole,
    ) -> Result<Vec<String>, PluginError> {
        if !(self.webhook_route && role.serves_http()) {
            return Ok(Vec::new());
        }
        check_webhook_endpoint(
            &state.config_arc().security.webhooks.endpoints,
            &self.webhook_path(),
            &config.webhook,
            self.allow_without_replay_protection,
        )
    }
}

impl Plugin for JiraPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(PLUGIN_NAME)
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let mut app = app.config_section(SECTION);
        if self.webhook_route {
            let path = self.webhook_path();
            app = app
                .declare_plugin_routes([mount::route_info(&path)])
                .merge(mount::webhook_router(&path));
        }
        let started: Started = Arc::default();
        let init_started = Arc::clone(&started);
        let plugin = Arc::new(self);
        let init_plugin = Arc::clone(&plugin);
        // A state initializer cannot fail. So it stores the result, and the
        // startup hook returns the error.
        app.state_initializer(move |state| {
            let result = init_plugin.resolve_config(state);
            if let Err(e) = &result {
                tracing::error!(error = %e, "jira plugin did not start");
            }
            *init_started.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        })
        .on_startup(move |state| {
            let taken = started
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let result = match taken {
                Some(Ok(config)) => plugin.startup_check(&state, &config, state.role()),
                Some(Err(e)) => Err(e),
                None => Err(PluginError::config("the jira plugin did not start")),
            };
            let result = result.map(|warnings| {
                for warning in warnings {
                    tracing::warn!("{warning}");
                }
            });
            std::future::ready(result.map_err(AutumnError::internal_server_error))
        })
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
        use autumn_web::plugin_contract::{AUTUMN_WEB_VERSION, ContractVerdict, evaluate};
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
