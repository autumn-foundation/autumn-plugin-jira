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
/// The most clients that the `for_connection` cache keeps.
pub(crate) const MAX_CACHED_CLIENTS: usize = 1024;

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
    /// The service keeps a client for each connection id, at most 1024. A
    /// new id then drops the client that was used least recently. The
    /// clients share the settings of `[jira.http]`. Give only connection
    /// ids that your app has checked: an id makes no call, but each new id
    /// can drop a cached client.
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

    /// San Mateo mode: drops the cached client of `connection_id`. The next
    /// [`for_connection`](Self::for_connection) makes a new client, which
    /// reads the site of the connection again. Use it when a tenant
    /// connects to another site. The default client does not change.
    ///
    /// In API token mode, or for an id that is not cached, it does nothing.
    pub fn forget(&self, connection_id: &str) {
        if let Some(tenants) = &self.tenants {
            tenants.forget(connection_id);
        }
    }

    /// Reads the site of the default client again, at once. The default
    /// client is the client of [`client`](Self::client) and of `Deref`.
    ///
    /// Use it when the tenant of the default connection connects to another
    /// site. The next call goes to the new site. It does not wait for
    /// [`CLOUD_ID_RECHECK_INTERVAL`](autumn_jira::backend::CLOUD_ID_RECHECK_INTERVAL).
    /// For a `for_connection` client, use [`forget`](Self::forget).
    ///
    /// It returns the new cloud id. It returns `None` and makes no call when
    /// the site is fixed: API token mode, a `jira-basic` connection, a
    /// `cloud_id` in `[jira.san_mateo]`, or no `connection_id`.
    ///
    /// # Errors
    /// The errors of [`JiraClient::refresh_site`], for example
    /// `JiraError::CloudId` when no site matches `site_url`.
    pub async fn refresh_default(&self) -> Result<Option<CloudId>, JiraError> {
        self.client.refresh_site().await
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
    clients: Mutex<Clients>,
}

/// The cached clients, with the time of the last use of each one.
#[derive(Default)]
struct Clients {
    /// The client and the tick of its last use, by connection id.
    map: HashMap<String, (JiraClient, u64)>,
    /// Goes up by one at each use.
    tick: u64,
}

impl Clients {
    fn len(&self) -> usize {
        self.map.len()
    }

    /// Drops the client with the oldest use.
    fn drop_oldest(&mut self) {
        let oldest = self
            .map
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest {
            self.map.remove(&id);
        }
    }
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
        clients.tick += 1;
        let tick = clients.tick;
        if let Some((client, used)) = clients.map.get_mut(connection_id) {
            *used = tick;
            return Ok(client.clone());
        }
        if clients.len() >= MAX_CACHED_CLIENTS {
            clients.drop_oldest();
        }
        let mut target = SanMateoTarget::new(
            self.environment.clone(),
            ConnectionKey::new(self.integration.clone(), connection_id),
        );
        // The configured site is the site of the default tenant. Another
        // tenant finds its own site.
        if self.default_connection.as_deref() == Some(connection_id) {
            target.cloud_id.clone_from(&self.cloud_id);
            target.site_url.clone_from(&self.site_url);
        }
        let client =
            JiraClient::san_mateo(self.san.clone(), target).with_settings(self.settings.clone());
        clients
            .map
            .insert(connection_id.to_owned(), (client.clone(), tick));
        drop(clients);
        Ok(client)
    }

    /// Drops the cached client of `connection_id`.
    fn forget(&self, connection_id: &str) {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map
            .remove(connection_id);
    }

    /// The number of cached clients.
    #[cfg(test)]
    fn cached(&self) -> usize {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// `true` when the cache holds a client for `connection_id`.
    #[cfg(test)]
    fn is_cached(&self, connection_id: &str) -> bool {
        self.clients
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map
            .contains_key(connection_id)
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
/// `Config` error gives a general text. The details stay in the log. They
/// tell the caller about the server, not about the request.
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use autumn_jira::san_mateo::{EnvironmentConfig, KeyRing, MemoryTransport};

    use super::*;

    fn tenants() -> Tenants {
        let san = SanMateo::builder()
            .environment(EnvironmentConfig::new("dev", "sm-secret-key-0123456789"))
            .key_ring(KeyRing::new("k1", [7; 32]).unwrap())
            .transport(Arc::new(MemoryTransport::new()))
            .build()
            .unwrap();
        Tenants::new(
            san,
            &crate::config::SanMateoTarget::default(),
            ClientSettings::default(),
        )
        .unwrap()
    }

    /// R10-plugin-ecosystem-4, R7-api-docs-9: ids from requests cannot
    /// grow the cache without a limit.
    #[test]
    fn the_cache_has_a_bound_and_drops_the_least_recently_used() {
        let t = tenants();
        for i in 0..(MAX_CACHED_CLIENTS * 3) {
            t.client(&format!("t{i}")).unwrap();
        }
        assert_eq!(t.cached(), MAX_CACHED_CLIENTS);

        let t = tenants();
        for i in 0..MAX_CACHED_CLIENTS {
            t.client(&format!("t{i}")).unwrap();
        }
        // A use keeps `t0`. The new id drops `t1`, the oldest use.
        t.client("t0").unwrap();
        t.client("new").unwrap();
        assert!(t.is_cached("t0") && t.is_cached("new"));
        assert!(!t.is_cached("t1"));
        assert_eq!(t.cached(), MAX_CACHED_CLIENTS);
    }

    /// `forget` drops one client, so the next call reads the site again.
    #[test]
    fn forget_drops_one_client() {
        let service = JiraService::new(tenants().no_connection(), Some(Arc::new(tenants())));
        service.for_connection("acme").unwrap();
        service.for_connection("globex").unwrap();
        let t = service.tenants.as_ref().unwrap();
        assert!(t.is_cached("acme"));
        service.forget("acme");
        assert!(!t.is_cached("acme") && t.is_cached("globex"));
        // Not cached, or not in san_mateo mode: nothing happens.
        service.forget("acme");
        JiraService::new(tenants().no_connection(), None).forget("acme");
    }
}
