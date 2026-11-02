//! The Jira site and the API token auth.
//!
//! `JiraSite::base_url` is the only base URL builder in the crate. There is
//! no free base URL, so a config value cannot send a token to another host.

use std::fmt;

use san_mateo::Secret;

use crate::error::{JiraError, Result};
use crate::ids::{CloudId, SiteName};

/// A Jira Cloud site. Only the two fixed host shapes exist.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JiraSite {
    /// `https://{subdomain}.atlassian.net`. Classic API token.
    Subdomain(SiteName),
    /// `https://api.atlassian.com/ex/jira/{cloud_id}`. Scoped API token.
    CloudId(CloudId),
}

impl JiraSite {
    /// Returns the base URL. It is the only base URL builder (guard test).
    #[must_use]
    pub fn base_url(&self) -> String {
        match self {
            Self::Subdomain(name) => format!("https://{name}.atlassian.net"),
            Self::CloudId(id) => format!("https://api.atlassian.com/ex/jira/{id}"),
        }
    }

    /// Returns a label for logs and health: `acme.atlassian.net` or
    /// `cloud:<id>`. It holds no secret.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Subdomain(name) => format!("{name}.atlassian.net"),
            Self::CloudId(id) => format!("cloud:{id}"),
        }
    }
}

/// Basic auth with an Atlassian account email and an API token.
#[derive(Clone)]
pub struct BasicAuth {
    email: String,
    token: Secret,
}

impl BasicAuth {
    /// Makes the auth. It never logs the email or the token.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` for an empty email or an empty token.
    pub fn new(email: impl Into<String>, token: Secret) -> Result<Self> {
        let email = email.into();
        if email.trim().is_empty() {
            return Err(JiraError::invalid("basic auth", "the email is empty"));
        }
        if token.is_empty() {
            return Err(JiraError::invalid("basic auth", "the API token is empty"));
        }
        Ok(Self { email, token })
    }

    /// Returns the `Authorization` header value: `Basic base64(email:token)`.
    #[allow(dead_code, reason = "the DirectBackend of WP3 uses it")]
    pub(crate) fn authorization(&self) -> Secret {
        let pair = format!("{}:{}", self.email, self.token.expose());
        Secret::new(format!(
            "Basic {}",
            data_encoding::BASE64.encode(pair.as_bytes())
        ))
    }
}

impl fmt::Debug for BasicAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BasicAuth")
            .field("email", &"<redacted>")
            .field("token", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn base_urls() {
        let sub = JiraSite::Subdomain(SiteName::parse("acme").unwrap());
        assert_eq!(sub.base_url(), "https://acme.atlassian.net");
        assert_eq!(sub.label(), "acme.atlassian.net");
        let id = "1324a887-45db-1bf4-1e99-ef0ff456d421";
        let cloud = JiraSite::CloudId(CloudId::parse(id).unwrap());
        assert_eq!(
            cloud.base_url(),
            "https://api.atlassian.com/ex/jira/1324a887-45db-1bf4-1e99-ef0ff456d421"
        );
        assert_eq!(cloud.label(), format!("cloud:{id}"));
    }

    #[test]
    fn site_name_rejects_hosts() {
        for bad in [
            "acme.evil.com",
            "evil.com/",
            "acme@evil",
            "acme:443",
            "https://acme",
            "ACME",
            "",
            "-acme",
            "acme/../x",
            "acme#",
            "acme?x",
        ] {
            assert!(SiteName::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn basic_auth_rules() {
        assert!(BasicAuth::new("", Secret::new("t")).is_err());
        assert!(BasicAuth::new("a@b.c", Secret::new("")).is_err());
        let auth = BasicAuth::new("fred", Secret::new("fred")).unwrap();
        assert_eq!(auth.authorization().expose(), "Basic ZnJlZDpmcmVk");
    }

    #[test]
    fn debug_hides_the_email_and_the_token() {
        let auth = BasicAuth::new("mia@example.com", Secret::new("tok-123")).unwrap();
        let text = format!("{auth:?}");
        assert_eq!(
            text,
            "BasicAuth { email: \"<redacted>\", token: \"<redacted>\" }"
        );
        assert!(!format!("{:?}", auth.authorization()).contains("tok"));
    }
}
