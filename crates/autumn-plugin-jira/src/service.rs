//! `JiraService`: the started client, for handlers and jobs.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use autumn_jira::backend::{
    ApiRequest, ApiResponse, BackendError, BackendInfo, BackendMode, JiraBackend, SanMateoTarget,
};
use autumn_jira::client::ClientSettings;
use autumn_jira::ids::CloudId;
use autumn_jira::san_mateo::{ConnectionKey, SanMateo};
use autumn_jira::{JiraClient, JiraError};
use autumn_web::reexports::axum::extract::FromRequestParts;
use autumn_web::reexports::http::StatusCode;
use autumn_web::reexports::http::request::Parts;
use autumn_web::{AppState, AutumnError, AutumnResult};

/// The message of a 503 before the plugin starts.
pub(crate) const NOT_STARTED: &str = "Jira has not started";
/// The message of an error whose text stays in the log.
const HIDDEN: &str = "the Jira call failed; see the server log";
/// The longest connection id.
const MAX_CONNECTION_ID: usize = 200;

/// The started Jira client. The plugin puts it in the app state.
///
/// Get it in a handler as an extractor, or with
/// [`JiraService::from_state`] or [`JiraService::require`]. It derefs to
/// [`JiraClient`]. Use [`JiraResultExt::http`] to answer a Jira error with
/// its HTTP status:
///
/// ```rust,no_run
/// use autumn_plugin_jira::{IssueKey, JiraResultExt, JiraService};
/// use autumn_web::{AutumnResult, Path};
///
/// async fn summary(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<String> {
///     let issue = jira.get_issue(key).await.http()?;
///     Ok(issue.fields.summary.unwrap_or_default())
/// }
/// ```
#[derive(Clone)]
pub struct JiraService {
    client: JiraClient,
    tenants: Option<Arc<Tenants>>,
}

impl std::fmt::Debug for JiraService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraService")
            .field("client", &self.client)
            .field("san_mateo", &self.tenants.is_some())
            .finish()
    }
}

impl JiraService {
    /// Wraps a client. `tenants` is set in `san_mateo` mode.
    pub(crate) const fn new(client: JiraClient, tenants: Option<Arc<Tenants>>) -> Self {
        Self { client, tenants }
    }

    /// The service, if the plugin started.
    #[must_use]
    pub fn from_state(state: &AppState) -> Option<Self> {
        state.extension::<Self>().map(|s| (*s).clone())
    }

    /// The service.
    ///
    /// # Errors
    /// Returns a 503 [`AutumnError`] with the text "Jira has not started"
    /// when the plugin did not start.
    pub fn require(state: &AppState) -> AutumnResult<Self> {
        Self::from_state(state).ok_or_else(|| AutumnError::service_unavailable_msg(NOT_STARTED))
    }

    /// The client.
    #[must_use]
    pub const fn client(&self) -> &JiraClient {
        &self.client
    }

    /// San Mateo mode: a client for another connection of the same
    /// integration. Use it for one Jira site for each tenant.
    ///
    /// The service keeps one client for each connection id. The clients
    /// share the settings of `[jira.http]`.
    ///
    /// `cloud_id` and `site_url` of `[jira.san_mateo]` apply only to the
    /// client of `connection_id`. The client of another connection gets
    /// its site from the `cloudId` in the metadata of that connection, else
    /// from a discovery with its own token. So the token of one tenant
    /// never goes to the site of another tenant.
    ///
    /// # Errors
    /// [`JiraError::InvalidInput`] for an id that is empty, longer than
    /// 200 chars, or has a space or a control char.
    /// [`JiraError::Config`] when the plugin is not in `san_mateo` mode.
    pub fn for_connection(&self, connection_id: &str) -> Result<JiraClient, JiraError> {
        self.tenants
            .as_ref()
            .ok_or(JiraError::Config {
                code: "for_connection_needs_san_mateo",
            })?
            .client(connection_id)
    }
}

impl Deref for JiraService {
    type Target = JiraClient;

    fn deref(&self) -> &JiraClient {
        &self.client
    }
}

impl FromRequestParts<AppState> for JiraService {
    type Rejection = AutumnError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let _ = parts;
        std::future::ready(Self::require(state))
    }
}

/// The clients of one San Mateo integration, one for each connection id.
pub(crate) struct Tenants {
    san: SanMateo,
    environment: String,
    integration: String,
    /// The `connection_id` of `[jira.san_mateo]`. Only its client gets
    /// `cloud_id` and `site_url`.
    default_connection: Option<String>,
    cloud_id: Option<CloudId>,
    site_url: Option<String>,
    settings: ClientSettings,
    clients: Mutex<HashMap<String, JiraClient>>,
}

impl Tenants {
    /// Makes the cache. It makes no client and no call.
    pub(crate) fn new(
        san: SanMateo,
        target: &crate::config::SanMateoTarget,
        settings: ClientSettings,
    ) -> Result<Self, JiraError> {
        let text = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        let cloud_id = text(&target.cloud_id)
            .map(|id| CloudId::parse(&id))
            .transpose()?;
        Ok(Self {
            san,
            environment: target.environment.clone(),
            integration: target.integration.clone(),
            default_connection: text(&target.connection_id),
            cloud_id,
            site_url: text(&target.site_url),
            settings,
            clients: Mutex::default(),
        })
    }

    /// The client of `connection_id`. It makes the client on the first call.
    pub(crate) fn client(&self, connection_id: &str) -> Result<JiraClient, JiraError> {
        if connection_id.is_empty()
            || connection_id.len() > MAX_CONNECTION_ID
            || !connection_id.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(JiraError::InvalidInput {
                what: "connection id",
                reason: "must have 1 to 200 printable ASCII chars with no space".to_owned(),
            });
        }
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        let client = clients
            .entry(connection_id.to_owned())
            .or_insert_with(|| {
                let mut target = SanMateoTarget::new(
                    self.environment.clone(),
                    ConnectionKey::new(self.integration.clone(), connection_id),
                );
                // The configured site is the site of the default tenant.
                // Another tenant finds its own site.
                if self.default_connection.as_deref() == Some(connection_id) {
                    target.cloud_id.clone_from(&self.cloud_id);
                    target.site_url.clone_from(&self.site_url);
                }
                JiraClient::san_mateo(self.san.clone(), target).with_settings(self.settings.clone())
            })
            .clone();
        drop(clients);
        Ok(client)
    }

    /// The client when `[jira.san_mateo]` names no connection: each call
    /// fails with `JiraError::Config`. Use `for_connection(id)`.
    pub(crate) fn no_connection(&self) -> JiraClient {
        JiraClient::new(Arc::new(NoConnection {
            info: BackendInfo {
                mode: BackendMode::SanMateo,
                site: format!("san-mateo:{}/(no connection_id)", self.integration),
            },
        }))
        .with_settings(self.settings.clone())
    }
}

/// A backend that sends nothing: no connection id is set.
struct NoConnection {
    info: BackendInfo,
}

#[async_trait]
impl JiraBackend for NoConnection {
    async fn send(&self, request: ApiRequest) -> Result<ApiResponse, BackendError> {
        let _ = request;
        Err(BackendError::NotSent {
            transient: false,
            code: "no_connection_id",
        })
    }

    fn info(&self) -> BackendInfo {
        self.info.clone()
    }
}

/// Converts a Jira error to an [`AutumnError`].
///
/// The status comes from `err.class().http_status`. A `Decode` or a
/// `Config` error gives a general text: the details stay in the log, as
/// they tell the caller about the server, not about the request.
#[must_use]
pub fn autumn_error(err: &JiraError) -> AutumnError {
    let class = err.class();
    let status = StatusCode::from_u16(class.http_status).unwrap_or(StatusCode::BAD_GATEWAY);
    match err {
        JiraError::Decode { .. } | JiraError::Config { .. } => {
            tracing::error!(code = class.code, "jira call failed");
            AutumnError::internal_server_error_msg(HIDDEN).with_status(status)
        }
        other => AutumnError::from(other.clone()).with_status(status),
    }
}

/// `.http()` on a Jira result: `jira.get_issue(key).await.http()?`.
pub trait JiraResultExt<T> {
    /// Maps the error with [`autumn_error`].
    ///
    /// # Errors
    /// The mapped error.
    fn http(self) -> AutumnResult<T>;
}

impl<T> JiraResultExt<T> for Result<T, JiraError> {
    fn http(self) -> AutumnResult<T> {
        self.map_err(|e| autumn_error(&e))
    }
}
