//! The backend seam: one attempt of one Jira call.
//!
//! A [`JiraBackend`] sends one attempt. It never retries and never follows
//! a redirect. The client does all retries with the verified policy
//! ([`crate::policy`]). [`DirectBackend`] sends with an API token (Basic
//! auth). [`SanMateoBackend`] sends through a san-mateo connection.

mod direct;
mod san_mateo;

use async_trait::async_trait;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};

use crate::error::{JiraError, Op, Result};
use crate::ids::CloudId;
use crate::model::AccessibleResource;

pub use self::san_mateo::{CLOUD_ID_RECHECK_INTERVAL, SanMateoBackend, SanMateoTarget};
pub use direct::DirectBackend;

/// The characters that stay as they are in a path segment or a query
/// value: the RFC 3986 unreserved set.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The `TransportError::Invalid` texts of `san_mateo::ReqwestTransport`
/// for a check before the send.
const INVALID_BEFORE_SEND: [&str; 4] = [
    "bad HTTP method",
    "bad URL",
    "bad header name",
    "bad header value",
];
/// The start of the reqwest text for a request that reqwest cannot build.
const BUILDER_ERROR: &str = "builder error";

/// Returns `true` when a `TransportError::Invalid` text says that the
/// request did not leave this process.
///
/// `ReqwestTransport` also gives `Invalid` for a reqwest send error
/// ("error sending request"), for example a reset after Jira has the whole
/// request. Such a write can have run. So only the known texts of a check
/// before the send give `true`. Every other text gives `false`: that is the
/// safe side for a write (B11).
pub(crate) fn invalid_before_send(text: &str) -> bool {
    INVALID_BEFORE_SEND.contains(&text) || text.starts_with(BUILDER_ERROR)
}

/// Percent-encodes one path segment. A `/` in the text cannot start a new
/// segment.
pub(crate) fn encode_segment(text: &str) -> String {
    utf8_percent_encode(text, UNRESERVED).to_string()
}

/// Encodes a query: `name=value` pairs joined by `&`.
pub(crate) fn encode_query(query: &[(String, String)]) -> String {
    query
        .iter()
        .map(|(name, value)| format!("{}={}", encode_segment(name), encode_segment(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// An HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
}

impl Method {
    /// Returns the upper-case name, for example `GET`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// One Jira call, before the host and the auth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiRequest {
    /// The operation.
    pub op: Op,
    /// The method.
    pub method: Method,
    /// `/rest/api/3/...`. The client percent-encodes each id segment.
    pub path: String,
    /// The query pairs, not encoded.
    pub query: Vec<(String, String)>,
    /// JSON bytes. `None` for no body.
    pub body: Option<Vec<u8>>,
}

impl ApiRequest {
    /// Makes a request with no query and no body.
    #[must_use]
    pub fn new(op: Op, method: Method, path: impl Into<String>) -> Self {
        Self {
            op,
            method,
            path: path.into(),
            query: Vec::new(),
            body: None,
        }
    }

    /// Adds one query pair.
    #[must_use]
    pub fn query(mut self, name: &str, value: impl Into<String>) -> Self {
        self.query.push((name.to_owned(), value.into()));
        self
    }

    /// Sets a JSON body.
    #[must_use]
    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.body = Some(value.to_string().into_bytes());
        self
    }

    /// Returns the path and the encoded query, for example
    /// `/rest/api/3/issue/OPS-1?fields=summary`.
    #[must_use]
    pub fn path_and_query(&self) -> String {
        if self.query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{}", self.path, encode_query(&self.query))
        }
    }
}

/// One HTTP reply. A non-2xx status is not an error at this layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    /// The status.
    pub status: u16,
    /// The headers.
    pub headers: Vec<(String, String)>,
    /// The body bytes.
    pub body: Vec<u8>,
}

impl ApiResponse {
    /// Returns the first value of a header. The name match ignores case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// One attempt failed with no HTTP reply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// The request did not leave this process. `transient` says if a retry can help.
    #[error("not sent ({code})")]
    NotSent {
        /// `true` when a later attempt can work.
        transient: bool,
        /// A short code.
        code: &'static str,
    },
    /// The request can have reached Jira. The outcome of a write is unknown.
    #[error("maybe sent ({code})")]
    MaybeSent {
        /// A short code.
        code: &'static str,
    },
    /// The credentials cannot be used (refresh exhausted). Not sent.
    #[error("auth ({code})")]
    Auth {
        /// A short code.
        code: &'static str,
    },
}

/// How a backend authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    /// An API token with Basic auth.
    ApiToken,
    /// A san-mateo connection.
    SanMateo,
}

/// The mode and the site of a backend, for health details. No secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInfo {
    /// The mode.
    pub mode: BackendMode,
    /// `JiraSite::label`, or `san-mateo:<integration>/<connection>`.
    pub site: String,
}

/// Sends one attempt of one Jira call.
#[async_trait]
pub trait JiraBackend: Send + Sync + 'static {
    /// Sends one attempt. It never retries and never follows a redirect.
    async fn send(&self, request: ApiRequest) -> Result<ApiResponse, BackendError>;

    /// Returns the mode and the site for health details. No secrets.
    fn info(&self) -> BackendInfo;

    /// Reads the site of the backend again, at once. Use it when a tenant
    /// connects to another site.
    ///
    /// It returns the new cloud id that the next attempt uses. `None` means
    /// that the backend has a fixed site, so it reads nothing. The default
    /// gives `None` and makes no call: an API token backend has a fixed
    /// site.
    ///
    /// # Errors
    ///
    /// The errors of the read.
    async fn refresh_site(&self) -> Result<Option<CloudId>> {
        Ok(None)
    }
}

/// Picks the cloud id of a Jira site from `accessible-resources`.
///
/// It keeps the entries with a scope that starts with `read:jira-` or
/// `write:jira-`, so a Confluence-only entry is skipped. With `site_url`,
/// it keeps only the entry with that URL.
///
/// # Errors
///
/// `JiraError::CloudId` for zero matches, or for more than one match with
/// no `site_url`.
pub fn resolve_cloud_id(
    resources: &[AccessibleResource],
    site_url: Option<&str>,
) -> Result<CloudId> {
    let picked = jira_sites(resources, site_url);
    match picked.as_slice() {
        [one] => Ok(one.id.clone()),
        [] if site_url.is_some() => Err(JiraError::CloudId(
            "no Jira site in accessible-resources has the site_url".to_owned(),
        )),
        [] => Err(JiraError::CloudId(
            "accessible-resources has no Jira site".to_owned(),
        )),
        _ => Err(JiraError::CloudId(
            "accessible-resources has more than one Jira site; set site_url".to_owned(),
        )),
    }
}

/// Returns the Jira entries of `accessible-resources`: an entry with a scope
/// that starts with `read:jira-` or `write:jira-`. With `site_url`, only the
/// entry with that URL.
pub(crate) fn jira_sites<'a>(
    resources: &'a [AccessibleResource],
    site_url: Option<&str>,
) -> Vec<&'a AccessibleResource> {
    let jira = resources.iter().filter(|r| {
        r.scopes
            .iter()
            .any(|s| s.starts_with("read:jira-") || s.starts_with("write:jira-"))
    });
    match site_url {
        Some(url) => jira.filter(|r| same_url(&r.url, url)).collect(),
        None => jira.collect(),
    }
}

/// Compares two site URLs. Case and a trailing `/` do not matter.
fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/')
        .eq_ignore_ascii_case(b.trim_end_matches('/'))
}

/// Maps a backend error to the error of a call that stops (plan 6, glue).
pub(crate) const fn backend_error(op: Op, error: &BackendError) -> JiraError {
    match *error {
        BackendError::NotSent {
            transient: false,
            code,
        } => JiraError::Config { code },
        BackendError::NotSent { code, .. } | BackendError::MaybeSent { code } => {
            JiraError::Network { op, code }
        }
        BackendError::Auth { code } => JiraError::Auth { code },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use serde_json::json;

    const JIRA_A: &str = "11111111-2222-4333-8444-555555555555";
    const JIRA_B: &str = "22222222-2222-4333-8444-555555555555";
    const WIKI: &str = "ffffffff-2222-4333-8444-555555555555";

    fn resource(id: &str, url: &str, scopes: &[&str]) -> AccessibleResource {
        serde_json::from_value(json!({"id": id, "name": "x", "url": url, "scopes": scopes}))
            .unwrap()
    }

    #[test]
    fn resolve_cloud_id_picks_jira() {
        let list = [
            resource(
                WIKI,
                "https://acme.atlassian.net",
                &["read:confluence-content.all"],
            ),
            resource(JIRA_A, "https://acme.atlassian.net", &["read:jira-work"]),
        ];
        assert_eq!(resolve_cloud_id(&list, None).unwrap().as_str(), JIRA_A);
        let write_only = [resource(
            JIRA_B,
            "https://b.atlassian.net",
            &["write:jira-work"],
        )];
        assert_eq!(
            resolve_cloud_id(&write_only, None).unwrap().as_str(),
            JIRA_B
        );
        let wiki_only = [resource(
            WIKI,
            "https://acme.atlassian.net",
            &["write:confluence-content"],
        )];
        assert!(matches!(
            resolve_cloud_id(&wiki_only, None),
            Err(JiraError::CloudId(_))
        ));
        assert!(matches!(
            resolve_cloud_id(&[], None),
            Err(JiraError::CloudId(_))
        ));
    }

    #[test]
    fn resolve_cloud_id_needs_a_site_when_many() {
        let list = [
            resource(JIRA_A, "https://acme.atlassian.net", &["read:jira-work"]),
            resource(JIRA_B, "https://beta.atlassian.net", &["read:jira-user"]),
        ];
        let many = resolve_cloud_id(&list, None).unwrap_err();
        assert!(matches!(many, JiraError::CloudId(_)));
        assert!(many.to_string().contains("site_url"));
        let picked = resolve_cloud_id(&list, Some("https://beta.atlassian.net")).unwrap();
        assert_eq!(picked.as_str(), JIRA_B);
        // A trailing slash and case do not matter.
        let slash = resolve_cloud_id(&list, Some("https://ACME.atlassian.net/")).unwrap();
        assert_eq!(slash.as_str(), JIRA_A);
        assert!(resolve_cloud_id(&list, Some("https://other.atlassian.net")).is_err());
    }

    #[test]
    fn requests_and_replies() {
        let request = ApiRequest::new(Op::GetIssue, Method::Get, "/rest/api/3/issue/OPS-1")
            .query("fields", "summary,status")
            .query("x y", "a&b=c/d");
        assert_eq!(
            request.path_and_query(),
            "/rest/api/3/issue/OPS-1?fields=summary%2Cstatus&x%20y=a%26b%3Dc%2Fd"
        );
        assert_eq!(
            ApiRequest::new(Op::Myself, Method::Get, "/rest/api/3/myself").path_and_query(),
            "/rest/api/3/myself"
        );
        let body = ApiRequest::new(Op::Search, Method::Post, "/p").json(&json!({"a": 1}));
        assert_eq!(body.body.as_deref(), Some(br#"{"a":1}"#.as_slice()));
        assert_eq!(encode_segment("a b/c..~"), "a%20b%2Fc..~");
        let reply = ApiResponse {
            status: 200,
            headers: vec![("Retry-After".to_owned(), "5".to_owned())],
            body: Vec::new(),
        };
        assert_eq!(reply.header("retry-after"), Some("5"));
        assert_eq!(reply.header("x"), None);
        let names: Vec<_> = [Method::Get, Method::Post, Method::Put, Method::Delete]
            .into_iter()
            .map(Method::as_str)
            .collect();
        assert_eq!(names, ["GET", "POST", "PUT", "DELETE"]);
    }
}
