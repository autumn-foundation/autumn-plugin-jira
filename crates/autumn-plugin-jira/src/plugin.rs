//! `JiraPlugin`: the builder and the autumn `Plugin` impl.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use autumn_jira::client::ClientSettings;
use autumn_jira::ids::{CloudId, SiteName};
use autumn_jira::san_mateo::transport::{HttpTransport, ReqwestTransport, TransportConfig};
use autumn_jira::{BasicAuth, Delivery, EventSink, HandlerError, JiraClient, JiraSite};
use autumn_plugin_san_mateo::SanMateoService;
use autumn_web::actuator::HealthIndicator;
use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use autumn_web::plugin_contract::PluginContract;
use autumn_web::{AppState, AutumnError, ProcessRole};
use tokio::sync::Semaphore;

use crate::config::{JiraConfig, JiraHttpConfig, JiraMode, JiraWebhookConfig};
use crate::error::PluginError;
use crate::health::JiraHealth;
use crate::runtime::{Cell, Counters, Handler, HandlerFuture, JiraRuntime, Runtime};
use crate::secrets::Secrets;
use crate::service::{JiraService, Tenants};
use crate::webhook;
use crate::webhook_config::check_webhook_endpoint;
use crate::{AUTUMN_WEB_REQUIREMENT, DEFAULT_PREFIX, INDICATOR_NAME, PLUGIN_NAME, SECTION};

/// The name of the San Mateo plugin.
const SAN_MATEO_PLUGIN: &str = autumn_plugin_san_mateo::PLUGIN_NAME;

/// The error of the state initializer. The startup hook returns it.
type Failure = Arc<Mutex<Option<PluginError>>>;

/// The contract that [`JiraPlugin`] declares to autumn.
#[must_use]
pub fn plugin_contract() -> PluginContract {
    PluginContract::new(env!("CARGO_PKG_NAME"))
        .plugin_version(env!("CARGO_PKG_VERSION"))
        .autumn_web(AUTUMN_WEB_REQUIREMENT)
}

/// The Jira plugin for autumn-web.
///
/// The plugin reads `[jira]` at startup, builds the client and puts a
/// [`JiraService`] in the app state. It makes no network call at startup.
/// With a sink, an `on_event` closure or
/// [`webhook_route`](Self::webhook_route), it mounts the webhook route.
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
///
/// Webhooks are at least once. Each sink can get the same delivery again,
/// also when another sink fails. Make each sink idempotent.
pub struct JiraPlugin {
    config: Option<JiraConfig>,
    prefix: String,
    webhook_route: bool,
    allow_without_replay_protection: bool,
    transport: Option<Arc<dyn HttpTransport>>,
    client: Option<JiraClient>,
    handlers: Vec<Handler>,
    secrets: BTreeMap<String, String>,
}

impl std::fmt::Debug for JiraPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Env var names only. The values are secrets.
        f.debug_struct("JiraPlugin")
            .field("prefix", &self.prefix)
            .field("config", &self.config)
            .field("webhook_route", &self.webhook_route)
            .field(
                "allow_without_replay_protection",
                &self.allow_without_replay_protection,
            )
            .field("transport", &self.transport.is_some())
            .field("client", &self.client)
            .field(
                "handlers",
                &self.handlers.iter().map(Handler::name).collect::<Vec<_>>(),
            )
            .field("secret_envs", &self.secrets.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Default for JiraPlugin {
    fn default() -> Self {
        Self {
            config: None,
            prefix: DEFAULT_PREFIX.to_owned(),
            webhook_route: false,
            allow_without_replay_protection: false,
            transport: None,
            client: None,
            handlers: Vec::new(),
            secrets: BTreeMap::new(),
        }
    }
}

impl JiraPlugin {
    /// A plugin that reads `[jira]` at startup with [`JiraConfig::load`]:
    /// `autumn.toml`, the profile file, `.env` and the `AUTUMN_JIRA__*` env
    /// vars. A later source wins.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A plugin with this config, as given. It reads no file and no
    /// `AUTUMN_JIRA__*` env var. To use the files and the env, use
    /// [`new`](Self::new), or [`JiraConfig::load`] and then change the
    /// values that you set in code.
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

    /// Uses this transport in API token mode. Default: a `ReqwestTransport`
    /// with the `[jira.http] timeout_secs`. Tests give `FakeJira` here.
    ///
    /// In `san_mateo` mode, San Mateo owns the transport. The start then
    /// fails with [`PluginError::Conflict`].
    #[must_use]
    pub fn transport(mut self, transport: Arc<dyn HttpTransport>) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Uses this prebuilt client. The plugin then reads no secret.
    ///
    /// The client keeps its own settings: set them with
    /// `JiraClient::with_settings`. Do not set `[jira.api_token]`,
    /// `[jira.san_mateo]` or a `[jira.http]` that is not the default with
    /// it: the start fails with [`PluginError::Conflict`].
    #[must_use]
    pub fn client(mut self, client: JiraClient) -> Self {
        self.client = Some(client);
        self
    }

    /// Adds a sink, and mounts the webhook route.
    ///
    /// The sinks and the `on_event` closures run in the order that you add
    /// them. Deliveries are at least once: make the sink idempotent.
    #[must_use]
    pub fn sink(mut self, sink: impl EventSink) -> Self {
        self.handlers.push(Handler::Sink(Arc::new(sink)));
        self
    }

    /// Adds a closure that gets each delivery and the app state, and mounts
    /// the webhook route. It runs in order with the sinks.
    #[must_use]
    pub fn on_event<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Arc<Delivery>, AppState) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerError>> + Send + 'static,
    {
        let boxed = move |delivery, state| -> HandlerFuture { Box::pin(f(delivery, state)) };
        self.handlers.push(Handler::OnEvent(Arc::new(boxed)));
        self
    }

    /// Mounts the webhook route at [`webhook_path`](Self::webhook_path),
    /// also with no handler. The route then logs each delivery id.
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

    /// Uses `value` for the env var `name`. The plugin then does not read
    /// `name` from the environment. For tests only.
    #[doc(hidden)]
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn secret_env(mut self, name: &str, value: &str) -> Self {
        self.secrets.insert(name.to_owned(), value.to_owned());
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

    /// Starts the plugin outside an app build, in the role of `state`. Use
    /// it in tests. The app build does the same at startup.
    ///
    /// # Errors
    /// The errors of the start: config, secrets, conflicts, San Mateo and
    /// the webhook boot check.
    pub fn start(self, state: &AppState) -> Result<JiraRuntime, PluginError> {
        let role = state.role();
        self.start_with_role(state, role)
    }

    /// Starts the plugin as if the process has `role`. A role that serves no
    /// HTTP skips the webhook boot check.
    ///
    /// # Errors
    /// Same as [`Self::start`].
    pub fn start_with_role(
        self,
        state: &AppState,
        role: ProcessRole,
    ) -> Result<JiraRuntime, PluginError> {
        let cell: Cell = Arc::default();
        let health = Arc::new(JiraHealth::new(Arc::clone(&cell)));
        let runtime = self.start_inner(state, false)?;
        for warning in self.startup_check(state, &runtime.webhook, role)? {
            tracing::warn!("{warning}");
        }
        let _ = cell.set(Arc::clone(&runtime));
        Ok(JiraRuntime::new(runtime, health))
    }

    /// `true` when the plugin mounts the webhook route.
    const fn mounts_route(&self) -> bool {
        self.webhook_route || !self.handlers.is_empty()
    }

    /// The config: the one from [`with_config`](Self::with_config), else the
    /// one that autumn files and the env give.
    fn resolve_config(&self, state: &AppState) -> Result<JiraConfig, PluginError> {
        match &self.config {
            Some(config) => {
                config.validate()?;
                Ok(config.clone())
            }
            None => JiraConfig::load(Some(state.profile()).filter(|p| *p != "default")),
        }
    }

    /// Builds the client and the runtime, and puts the service in the app
    /// state. It makes no network call. `san_mateo_before` tells if the San
    /// Mateo plugin was registered before this plugin.
    fn start_inner(
        &self,
        state: &AppState,
        san_mateo_before: bool,
    ) -> Result<Arc<Runtime>, PluginError> {
        let config = self.resolve_config(state)?;
        let settings = client_settings(&config.http);
        let mut probe = true;
        let (client, tenants, mode) = match (&self.client, config.mode()?) {
            (Some(_), Some(mode)) => {
                return Err(PluginError::Conflict("client(..)", table(mode)));
            }
            (Some(_), None) if self.transport.is_some() => {
                return Err(PluginError::Conflict("transport(..)", "client(..)"));
            }
            // A prebuilt client keeps its own settings.
            (Some(_), None) if config.http != JiraHttpConfig::default() => {
                return Err(PluginError::Conflict("client(..)", "[jira.http]"));
            }
            (Some(client), None) => (client.clone(), None, mode_name(client)),
            (None, None) => {
                return Err(PluginError::config(
                    "set [jira.api_token] or [jira.san_mateo]",
                ));
            }
            (None, Some(JiraMode::ApiToken)) => (
                self.api_token_client(&config, settings)?,
                None,
                JiraMode::ApiToken.as_str(),
            ),
            (None, Some(JiraMode::SanMateo)) => {
                if self.transport.is_some() {
                    return Err(PluginError::Conflict("transport(..)", "[jira.san_mateo]"));
                }
                let tenants = self.tenants(state, &config, settings, san_mateo_before)?;
                let default = config
                    .san_mateo
                    .as_ref()
                    .and_then(|t| t.connection_id.as_deref())
                    .filter(|id| !id.trim().is_empty());
                // No default connection: the health check has nothing to probe.
                probe = default.is_some();
                let client = match default {
                    Some(id) => tenants.client(id)?,
                    None => tenants.no_connection(),
                };
                (client, Some(Arc::new(tenants)), JiraMode::SanMateo.as_str())
            }
        };
        let mut health_settings = client.settings().clone();
        health_settings.max_attempts = 1;
        let health_client = probe.then(|| client.clone().with_settings(health_settings));
        let service = JiraService::new(client, tenants);
        state.insert_extension(service.clone());
        let max_body_bytes = self
            .mounts_route()
            .then(|| {
                state
                    .config_arc()
                    .security
                    .webhooks
                    .endpoints
                    .iter()
                    .find(|e| e.name == config.webhook.endpoint_name)
                    .map(|e| e.max_body_bytes)
            })
            .flatten();
        tracing::info!(
            mode,
            site = %service.info().site,
            handlers = self.handlers.len(),
            "jira started"
        );
        let slots = Arc::new(Semaphore::new(
            usize::try_from(config.webhook.max_handler_tasks).unwrap_or(Semaphore::MAX_PERMITS),
        ));
        Ok(Arc::new(Runtime {
            service,
            health_client,
            slots,
            webhook: config.webhook,
            health: config.health,
            handlers: self.handlers.clone(),
            counters: Counters::default(),
            mode,
            max_body_bytes,
        }))
    }

    /// The API token client: the site, the two secrets and the transport.
    fn api_token_client(
        &self,
        config: &JiraConfig,
        settings: ClientSettings,
    ) -> Result<JiraClient, PluginError> {
        let Some(api) = &config.api_token else {
            return Err(PluginError::config("set [jira.api_token]"));
        };
        let text = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let site = match (text(&api.site), text(&api.cloud_id)) {
            (Some(site), _) => JiraSite::Subdomain(SiteName::parse(&site)?),
            (None, Some(id)) => JiraSite::CloudId(CloudId::parse(&id)?),
            (None, None) => {
                return Err(PluginError::config(
                    "set site or cloud_id in [jira.api_token]",
                ));
            }
        };
        let secrets = Secrets::new(self.secrets.clone());
        let email = secrets.require(&api.email_env)?;
        let token = secrets.require(&api.token_env)?;
        let auth = BasicAuth::new(email.expose(), token)?;
        let transport = if let Some(transport) = &self.transport {
            Arc::clone(transport)
        } else {
            let transport = ReqwestTransport::new(TransportConfig {
                timeout: Duration::from_secs(config.http.timeout_secs),
                ..TransportConfig::default()
            })
            .map_err(|e| PluginError::config(format!("the HTTP transport: {e}")))?;
            Arc::new(transport) as Arc<dyn HttpTransport>
        };
        Ok(JiraClient::api_token(site, auth, transport).with_settings(settings))
    }

    /// The San Mateo clients. San Mateo must have started before.
    fn tenants(
        &self,
        state: &AppState,
        config: &JiraConfig,
        settings: ClientSettings,
        san_mateo_before: bool,
    ) -> Result<Tenants, PluginError> {
        let _ = self;
        let Some(san) = SanMateoService::from_state(state) else {
            // autumn registers the health indicators before it runs the
            // state initializers. So the San Mateo indicator is there also
            // when its plugin starts after this one.
            let registered_later = state
                .health_indicator_registry()
                .contains(autumn_plugin_san_mateo::INDICATOR_NAME);
            return Err(if san_mateo_before {
                PluginError::config("SanMateoPlugin did not start; see its error")
            } else if registered_later {
                PluginError::SanMateoAfterJira
            } else {
                PluginError::SanMateoMissing
            });
        };
        let Some(target) = &config.san_mateo else {
            return Err(PluginError::config("set [jira.san_mateo]"));
        };
        Ok(Tenants::new(san.san().clone(), target, settings)?)
    }

    /// The startup check. It runs the webhook boot check when this process
    /// serves HTTP and the route is mounted. It returns the warnings, also
    /// the warning when the handlers can run past the autumn request
    /// timeout.
    fn startup_check(
        &self,
        state: &AppState,
        settings: &JiraWebhookConfig,
        role: ProcessRole,
    ) -> Result<Vec<String>, PluginError> {
        if !(self.mounts_route() && role.serves_http()) {
            return Ok(Vec::new());
        }
        let config = state.config_arc();
        let mut warnings = check_webhook_endpoint(
            &config.security.webhooks.endpoints,
            &self.webhook_path(),
            settings,
            self.allow_without_replay_protection,
        )?;
        warnings.extend(handler_budget_warning(
            self.handlers.len(),
            settings.handler_timeout_ms,
            config.server.timeouts.request_timeout_ms,
        ));
        Ok(warnings)
    }
}

/// The name of the table of `mode`, for a conflict message.
const fn table(mode: JiraMode) -> &'static str {
    match mode {
        JiraMode::ApiToken => "[jira.api_token]",
        JiraMode::SanMateo => "[jira.san_mateo]",
    }
}

/// The mode name of a prebuilt client.
fn mode_name(client: &JiraClient) -> &'static str {
    match client.info().mode {
        autumn_jira::backend::BackendMode::ApiToken => JiraMode::ApiToken.as_str(),
        autumn_jira::backend::BackendMode::SanMateo => JiraMode::SanMateo.as_str(),
    }
}

/// A warning when the handlers can run past the autumn request timeout.
fn handler_budget_warning(
    handlers: usize,
    handler_timeout_ms: u64,
    request_timeout_ms: Option<u64>,
) -> Option<String> {
    let request_ms = request_timeout_ms.filter(|ms| *ms > 0)?;
    let count = u64::try_from(handlers).unwrap_or(u64::MAX);
    let total = count.saturating_mul(handler_timeout_ms);
    (handlers > 0 && total >= request_ms).then(|| {
        format!(
            "the {handlers} jira webhook handlers can take {total} ms ({handlers} x [jira.webhook] handler_timeout_ms = {handler_timeout_ms}), and server.timeouts.request_timeout_ms is {request_ms}; autumn then stops the request with 503, the later handlers do not run, and Jira sends the delivery again; set handler_timeout_ms below {}",
            request_ms / count
        )
    })
}

/// The client settings of `[jira.http]`.
fn client_settings(http: &JiraHttpConfig) -> ClientSettings {
    let mut settings = ClientSettings::default();
    settings.max_attempts = http.max_attempts;
    settings.base_backoff_ms = http.base_backoff_ms;
    settings.max_backoff_ms = http.max_backoff_ms;
    settings.max_pages = http.max_pages;
    settings.lookup_attempts = http.lookup_attempts;
    settings.lookup_delay_ms = http.lookup_delay_ms;
    settings
}

impl Plugin for JiraPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed(PLUGIN_NAME)
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let san_mateo_before = app.has_plugin(SAN_MATEO_PLUGIN);
        let cell: Cell = Arc::default();
        let health = Arc::new(JiraHealth::new(Arc::clone(&cell)));
        let mut app = app.config_section(SECTION).health_indicator(
            INDICATOR_NAME,
            Arc::clone(&health) as Arc<dyn HealthIndicator>,
        );
        if self.mounts_route() {
            let path = self.webhook_path();
            app = app
                .declare_plugin_routes([webhook::route_info(&path)])
                .merge(webhook::router(&path, Arc::clone(&cell)));
        }
        let failure: Failure = Arc::default();
        let init_failure = Arc::clone(&failure);
        let init_cell = Arc::clone(&cell);
        let plugin = Arc::new(self);
        let init_plugin = Arc::clone(&plugin);
        // A state initializer cannot fail. So it stores the error, and the
        // startup hook returns it. It runs before the job workers, so a
        // `#[job]` can use `JiraService`.
        app.state_initializer(move |state| {
            match init_plugin.start_inner(state, san_mateo_before) {
                Ok(runtime) => {
                    let _ = init_cell.set(runtime);
                }
                Err(e) => {
                    // Also log: some autumn modes run no startup hook.
                    tracing::error!(error = %e, "jira plugin did not start");
                    health.mark_failed();
                    *init_failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(e);
                }
            }
        })
        .on_startup(move |state| {
            let failed = failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let result = match (failed, cell.get()) {
                (Some(e), _) => Err(e),
                (None, Some(runtime)) => {
                    plugin.startup_check(&state, &runtime.webhook, state.role())
                }
                (None, None) => Err(PluginError::config("the jira plugin did not start")),
            };
            let result = result.map(|warnings| {
                for warning in warnings {
                    tracing::warn!("{warning}");
                }
            });
            // The message holds the `Display` text, so the boot error and a
            // `TestApp` panic both show the fix, not the `Debug` form.
            std::future::ready(
                result.map_err(|e| AutumnError::internal_server_error_msg(e.to_string())),
            )
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

    /// R10-plugin-ecosystem-7: the start warns when the handlers can run
    /// past `server.timeouts.request_timeout_ms`.
    #[test]
    fn handler_budget_warns_at_the_request_timeout() {
        let w = handler_budget_warning(4, 8000, Some(30_000)).unwrap();
        assert!(
            w.contains("handler_timeout_ms") && w.contains("request_timeout_ms"),
            "{w}"
        );
        assert!(w.contains("32000") && w.contains("30000"), "{w}");
        assert!(handler_budget_warning(3, 10_000, Some(30_000)).is_some());
        assert!(handler_budget_warning(3, 8000, Some(30_000)).is_none());
        assert!(handler_budget_warning(4, 8000, None).is_none());
        assert!(handler_budget_warning(4, 8000, Some(0)).is_none());
        assert!(handler_budget_warning(0, 8000, Some(1)).is_none());
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

    #[test]
    fn client_settings_follow_the_http_table() {
        let http = JiraHttpConfig {
            max_attempts: 3,
            base_backoff_ms: 7,
            max_backoff_ms: 70,
            max_pages: 9,
            lookup_attempts: 2,
            lookup_delay_ms: 5,
            ..JiraHttpConfig::default()
        };
        let s = client_settings(&http);
        assert_eq!(
            (
                s.max_attempts,
                s.base_backoff_ms,
                s.max_backoff_ms,
                s.max_pages,
                s.lookup_attempts,
                s.lookup_delay_ms
            ),
            (3, 7, 70, 9, 2, 5)
        );
        assert_eq!(table(JiraMode::SanMateo), "[jira.san_mateo]");
    }
}
