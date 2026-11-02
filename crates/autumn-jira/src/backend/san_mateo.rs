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
use san_mateo::{ConnectionKey, SanMateo};
use tokio::sync::OnceCell;

use super::{ApiRequest, ApiResponse, BackendError, BackendInfo, BackendMode, JiraBackend};
use crate::error::Result;
use crate::ids::CloudId;

/// The san-mateo provider of a Jira API token.
pub(crate) const JIRA_BASIC: &str = "jira-basic";
/// The san-mateo provider of Atlassian OAuth.
pub(crate) const ATLASSIAN: &str = "atlassian";
/// The connection metadata key that holds the cloud id.
pub(crate) const CLOUD_ID_KEY: &str = "cloudId";

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
    let _ = error;
    BackendError::MaybeSent { code: "unknown" }
}

/// Returns the path prefix of a provider.
pub(crate) fn prefix_for(
    provider: &str,
    cloud_id: Option<&CloudId>,
) -> std::result::Result<Option<String>, BackendError> {
    let _ = (provider, cloud_id);
    Ok(None)
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
        let _ = (&self.san, &self.prefix);
        Err(crate::error::JiraError::CloudId("not written yet".to_owned()))
    }
}

#[async_trait]
impl JiraBackend for SanMateoBackend {
    async fn send(&self, request: ApiRequest) -> std::result::Result<ApiResponse, BackendError> {
        let _ = request;
        Err(BackendError::MaybeSent { code: "unknown" })
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
    fn error_map_covers_every_variant() {
        let s = || "x".to_owned();
        let provider = |status: Option<u16>| Error::Provider {
            code: "token_exchange_failed",
            status,
            message: s(),
        };
        let cases = [
            (Error::Upstream(s()), BackendError::MaybeSent { code: "upstream" }),
            (
                Error::UpstreamTooLarge,
                BackendError::MaybeSent {
                    code: "upstream_too_large",
                },
            ),
            (Error::Store(s()), not_sent(true, "store_error")),
            (Error::RefreshCooldown, not_sent(true, "connection_refresh_backoff")),
            (provider(None), not_sent(true, "provider")),
            (provider(Some(500)), not_sent(true, "provider")),
            (provider(Some(429)), not_sent(true, "provider")),
            (provider(Some(408)), not_sent(true, "provider")),
            (provider(Some(425)), not_sent(true, "provider")),
            (provider(Some(400)), not_sent(false, "provider")),
            (provider(Some(401)), not_sent(false, "provider")),
            (Error::RefreshExhausted, BackendError::Auth { code: Error::RefreshExhausted.code() }),
            (
                Error::Unauthorized(s()),
                BackendError::Auth {
                    code: Error::Unauthorized(s()).code(),
                },
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(map_error(&error), expected, "{error:?}");
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
            assert_eq!(map_error(&error), not_sent(false, error.code()), "{error:?}");
        }
    }

    #[test]
    fn prefix_by_provider() {
        let id = CloudId::parse("11111111-2222-4333-8444-555555555555").unwrap();
        assert_eq!(prefix_for(JIRA_BASIC, None).unwrap(), Some(String::new()));
        assert_eq!(prefix_for(JIRA_BASIC, Some(&id)).unwrap(), Some(String::new()));
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
