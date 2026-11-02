//! `SanMateoBackend`: each attempt goes through `SanMateo::proxy` with
//! `retries = 0`.
//!
//! San Mateo holds the credentials. `jira-basic` sends Basic auth to
//! `https://{subdomain}.atlassian.net`. `atlassian` sends a Bearer token to
//! `https://api.atlassian.com`, so the backend puts `/ex/jira/{cloudId}`
//! before each path. With `retries = 0`, san-mateo sends once, plus one
//! resend after a 401 refresh.

use std::fmt;

use async_trait::async_trait;
use san_mateo::store::ConnectionFilter;
use san_mateo::{ConnectionKey, ProxyRequest, SanMateo};
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

use super::{
    ApiRequest, ApiResponse, BackendError, BackendInfo, BackendMode, JiraBackend, backend_error,
    resolve_cloud_id,
};
use crate::error::{JiraError, Op, Result};
use crate::ids::CloudId;
use crate::model::AccessibleResource;

/// The san-mateo provider of a Jira API token.
pub(crate) const JIRA_BASIC: &str = "jira-basic";
/// The san-mateo provider of Atlassian OAuth.
pub(crate) const ATLASSIAN: &str = "atlassian";
/// The connection metadata key that holds the cloud id.
pub(crate) const CLOUD_ID_KEY: &str = "cloudId";
/// The discovery path on `https://api.atlassian.com`.
const RESOURCES_PATH: &str = "/oauth/token/accessible-resources";

/// The san-mateo connection that a [`SanMateoBackend`] uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanMateoTarget {
    /// The san-mateo environment id, for example `dev`.
    pub environment: String,
    /// The integration key and the connection id.
    pub connection: ConnectionKey,
    /// `atlassian` only. Else the backend reads metadata key `cloudId`, then discovers.
    pub cloud_id: Option<CloudId>,
    /// Picks one site from `accessible-resources`, for example `https://acme.atlassian.net`.
    pub site_url: Option<String>,
}

impl SanMateoTarget {
    /// Makes a target with no cloud id and no site URL.
    #[must_use]
    pub fn new(environment: impl Into<String>, connection: ConnectionKey) -> Self {
        Self {
            environment: environment.into(),
            connection,
            cloud_id: None,
            site_url: None,
        }
    }

    /// Sets the cloud id (`atlassian` only).
    #[must_use]
    pub fn cloud_id(mut self, cloud_id: CloudId) -> Self {
        self.cloud_id = Some(cloud_id);
        self
    }

    /// Sets the site URL that picks one entry of `accessible-resources`.
    #[must_use]
    pub fn site_url(mut self, url: impl Into<String>) -> Self {
        self.site_url = Some(url.into());
        self
    }
}

/// Sends each attempt through a san-mateo connection.
pub struct SanMateoBackend {
    san: SanMateo,
    target: SanMateoTarget,
    prefix: OnceCell<String>,
}

impl fmt::Debug for SanMateoBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SanMateoBackend")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

/// Maps a san-mateo error to a backend error (plan 4.4).
pub(crate) fn map_error(error: &san_mateo::Error) -> BackendError {
    use san_mateo::Error as E;
    match error {
        E::Upstream(_) => BackendError::MaybeSent { code: "upstream" },
        E::UpstreamTooLarge => BackendError::MaybeSent {
            code: "upstream_too_large",
        },
        E::Store(_) | E::RefreshCooldown => BackendError::NotSent {
            transient: true,
            code: error.code(),
        },
        E::Provider { status, .. } => BackendError::NotSent {
            transient: status.is_none_or(|s| s >= 500 || matches!(s, 408 | 425 | 429)),
            code: "provider",
        },
        E::RefreshExhausted | E::Unauthorized(_) => BackendError::Auth { code: error.code() },
        E::UnknownIntegration(_)
        | E::UnknownConnection(_)
        | E::UnknownProvider(_)
        | E::Template(_)
        | E::ForbiddenHost(_)
        | E::Invalid(_)
        | E::MissingField(_)
        | E::UnsupportedAuthMode(_)
        | E::IntegrationNotAllowed(_)
        | E::Crypto
        | E::Internal(_)
        | E::InvalidConnectSession
        | E::InvalidOAuthState
        | E::Conflict(_)
        | E::UnknownFunction(_)
        | E::Function(_) => BackendError::NotSent {
            transient: false,
            code: error.code(),
        },
        // A future variant: the safe side for a write (B11).
        _ => BackendError::MaybeSent { code: "unknown" },
    }
}

/// Turns a client error of the discovery into a backend error. The Jira
/// call did not leave this process.
const fn not_sent(error: &JiraError) -> BackendError {
    BackendError::NotSent {
        transient: error.is_retryable(),
        code: error.code(),
    }
}

/// Returns the path prefix of a provider.
pub(crate) fn prefix_for(
    provider: &str,
    cloud_id: Option<&CloudId>,
) -> std::result::Result<Option<String>, BackendError> {
    match provider {
        JIRA_BASIC => Ok(Some(String::new())),
        ATLASSIAN => Ok(cloud_id.map(|id| format!("/ex/jira/{id}"))),
        _ => Err(BackendError::NotSent {
            transient: false,
            code: "not_a_jira_provider",
        }),
    }
}

impl SanMateoBackend {
    /// Makes the backend. It makes no call until the first request.
    #[must_use]
    pub fn new(san: SanMateo, target: SanMateoTarget) -> Self {
        Self {
            san,
            target,
            prefix: OnceCell::new(),
        }
    }

    /// Returns the target.
    #[must_use]
    pub const fn target(&self) -> &SanMateoTarget {
        &self.target
    }

    /// Reads `accessible-resources`, picks the site, stores `cloudId` in
    /// the connection metadata (merge), and returns it.
    ///
    /// # Errors
    ///
    /// `JiraError::CloudId` when no site matches, or the provider is not
    /// `atlassian`. The errors of the call.
    pub async fn discover_cloud_id(&self) -> Result<CloudId> {
        let provider = self
            .provider()
            .await
            .map_err(|e| backend_error(Op::Discovery, &e))?;
        if provider != ATLASSIAN {
            return Err(JiraError::CloudId(
                "only the atlassian provider has a cloud id to discover".to_owned(),
            ));
        }
        self.discover().await
    }

    /// Returns the provider name of the integration.
    async fn provider(&self) -> std::result::Result<String, BackendError> {
        self.san
            .get_integration(
                &self.target.environment,
                &self.target.connection.provider_config_key,
            )
            .await
            .map(|i| i.provider)
            .map_err(|e| map_error(&e))
    }

    /// Returns the path prefix: from the provider, the target, the
    /// metadata, or a discovery, in this order.
    async fn resolve_prefix(&self) -> std::result::Result<String, BackendError> {
        let provider = self.provider().await?;
        if let Some(prefix) = prefix_for(&provider, self.target.cloud_id.as_ref())? {
            return Ok(prefix);
        }
        let id = match self.metadata_cloud_id().await? {
            Some(id) => id,
            None => self.discover().await.map_err(|e| not_sent(&e))?,
        };
        Ok(format!("/ex/jira/{id}"))
    }

    /// Reads the cloud id from the connection metadata. A bad value counts
    /// as none.
    async fn metadata_cloud_id(&self) -> std::result::Result<Option<CloudId>, BackendError> {
        let filter = ConnectionFilter {
            provider_config_key: Some(self.target.connection.provider_config_key.clone()),
            connection_id: Some(self.target.connection.connection_id.clone()),
            ..ConnectionFilter::default()
        };
        let list = self
            .san
            .list_connections(&self.target.environment, &filter)
            .await
            .map_err(|e| map_error(&e))?;
        Ok(list.iter().find_map(|c| {
            c.metadata
                .get(CLOUD_ID_KEY)
                .and_then(Value::as_str)
                .and_then(|t| CloudId::parse(t).ok())
        }))
    }

    /// Reads `accessible-resources`, picks the site, and stores the id.
    async fn discover(&self) -> Result<CloudId> {
        let env = &self.target.environment;
        let key = &self.target.connection;
        let mut request = ProxyRequest::get(RESOURCES_PATH);
        request.headers = vec![("accept".to_owned(), "application/json".to_owned())];
        request.retries = 0;
        let reply = self
            .san
            .proxy(env, key, request)
            .await
            .map_err(|e| backend_error(Op::Discovery, &map_error(&e)))?;
        let reply = ApiResponse {
            status: reply.status,
            headers: reply.headers,
            body: reply.body,
        };
        if !(200..=299).contains(&reply.status) {
            return Err(crate::client::status_error(Op::Discovery, &reply));
        }
        let resources: Vec<AccessibleResource> = crate::model::decode(Op::Discovery, &reply.body)?;
        let id = resolve_cloud_id(&resources, self.target.site_url.as_deref())?;
        let metadata = Map::from_iter([(CLOUD_ID_KEY.to_owned(), json!(id.as_str()))]);
        self.san
            .set_metadata(env, key, metadata, true)
            .await
            .map_err(|e| backend_error(Op::Discovery, &map_error(&e)))?;
        tracing::info!(site = %self.info().site, "jira: stored the discovered cloud id");
        Ok(id)
    }
}

#[async_trait]
impl JiraBackend for SanMateoBackend {
    async fn send(&self, request: ApiRequest) -> std::result::Result<ApiResponse, BackendError> {
        let prefix = self
            .prefix
            .get_or_try_init(|| self.resolve_prefix())
            .await?;
        let mut proxy = ProxyRequest::new(
            request.method.as_str(),
            format!("{prefix}{}", request.path_and_query()),
        );
        proxy.headers = vec![("accept".to_owned(), "application/json".to_owned())];
        if let Some(body) = request.body {
            proxy
                .headers
                .push(("content-type".to_owned(), "application/json".to_owned()));
            proxy.body = body;
        }
        // One send for each attempt. The client owns the retries.
        proxy.retries = 0;
        proxy.retry_on = Vec::new();
        let reply = self
            .san
            .proxy(&self.target.environment, &self.target.connection, proxy)
            .await
            .map_err(|e| map_error(&e))?;
        Ok(ApiResponse {
            status: reply.status,
            headers: reply.headers,
            body: reply.body,
        })
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            mode: BackendMode::SanMateo,
            site: format!(
                "san-mateo:{}/{}",
                self.target.connection.provider_config_key, self.target.connection.connection_id
            ),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use san_mateo::Error;

    fn not_sent(transient: bool, code: &'static str) -> BackendError {
        BackendError::NotSent { transient, code }
    }

    #[test]
    fn a_send_error_of_the_transport_is_maybe_sent() {
        // san-mateo wraps a transport `Invalid` in this text. A reqwest
        // send error ("error sending request") can come after Jira has the
        // whole request.
        let wrap = |m: &str| Error::Invalid(format!("the upstream request is not valid: {m}"));
        for sent in [
            "error sending request",
            "request or response body error",
            "",
        ] {
            assert_eq!(
                map_error(&wrap(sent)),
                BackendError::MaybeSent { code: "send_error" },
                "{sent}"
            );
        }
        // The checks before the send stay "not sent".
        for local in [
            "bad HTTP method",
            "bad URL",
            "bad header name",
            "bad header value",
            "builder error",
        ] {
            assert_eq!(
                map_error(&wrap(local)),
                not_sent(false, "invalid_body"),
                "{local}"
            );
        }
        // Another `Invalid` of san-mateo is a check before the send.
        assert_eq!(
            map_error(&Error::Invalid("the path is not valid".to_owned())),
            not_sent(false, "invalid_body")
        );
    }

    #[test]
    fn error_map_covers_every_variant() {
        let s = || "x".to_owned();
        let provider = |status: Option<u16>| Error::Provider {
            code: "token_exchange_failed",
            status,
            message: s(),
        };
        let cases = [
            (
                Error::Upstream(s()),
                BackendError::MaybeSent { code: "upstream" },
            ),
            (
                Error::UpstreamTooLarge,
                BackendError::MaybeSent {
                    code: "upstream_too_large",
                },
            ),
            (Error::Store(s()), not_sent(true, "store_error")),
            (
                Error::RefreshCooldown,
                not_sent(true, "connection_refresh_backoff"),
            ),
            (provider(None), not_sent(true, "provider")),
            (provider(Some(500)), not_sent(true, "provider")),
            (provider(Some(429)), not_sent(true, "provider")),
            (provider(Some(408)), not_sent(true, "provider")),
            (provider(Some(425)), not_sent(true, "provider")),
            (provider(Some(400)), not_sent(false, "provider")),
            (provider(Some(401)), not_sent(false, "provider")),
            (
                Error::RefreshExhausted,
                BackendError::Auth {
                    code: Error::RefreshExhausted.code(),
                },
            ),
            (
                Error::Unauthorized(s()),
                BackendError::Auth {
                    code: Error::Unauthorized(s()).code(),
                },
            ),
        ];
        for (error, expected) in &cases {
            assert_eq!(map_error(error), *expected, "{error:?}");
        }
        // A transient `NotSent` becomes `JiraError::Network` after the last
        // attempt. Each of its codes must be in `NOT_SENT_NETWORK_CODES`,
        // or a durable host counts the call as "maybe sent".
        for (error, _) in &cases {
            if let BackendError::NotSent {
                transient: true,
                code,
            } = map_error(error)
            {
                assert!(
                    crate::error::NOT_SENT_NETWORK_CODES.contains(&code),
                    "{code} is not pinned"
                );
                let network = JiraError::Network {
                    op: Op::GetIssue,
                    code,
                };
                assert!(!network.may_have_run(), "{code}");
            }
        }
        let config = [
            Error::UnknownIntegration(s()),
            Error::UnknownConnection(s()),
            Error::UnknownProvider(s()),
            Error::Template(s()),
            Error::ForbiddenHost(s()),
            Error::Invalid(s()),
            Error::MissingField(s()),
            Error::UnsupportedAuthMode(s()),
            Error::IntegrationNotAllowed(s()),
            Error::Crypto,
            Error::Internal(s()),
            Error::InvalidConnectSession,
            Error::InvalidOAuthState,
            Error::Conflict(s()),
            Error::UnknownFunction(s()),
            Error::Function(s()),
        ];
        for error in config {
            assert_eq!(
                map_error(&error),
                not_sent(false, error.code()),
                "{error:?}"
            );
        }
    }

    #[test]
    fn prefix_by_provider() {
        let id = CloudId::parse("11111111-2222-4333-8444-555555555555").unwrap();
        assert_eq!(prefix_for(JIRA_BASIC, None).unwrap(), Some(String::new()));
        assert_eq!(
            prefix_for(JIRA_BASIC, Some(&id)).unwrap(),
            Some(String::new())
        );
        assert_eq!(
            prefix_for(ATLASSIAN, Some(&id)).unwrap(),
            Some("/ex/jira/11111111-2222-4333-8444-555555555555".to_owned())
        );
        // `atlassian` with no cloud id yet: the caller discovers it.
        assert_eq!(prefix_for(ATLASSIAN, None).unwrap(), None);
        assert_eq!(
            prefix_for("github", None).unwrap_err(),
            not_sent(false, "not_a_jira_provider")
        );
    }
}
