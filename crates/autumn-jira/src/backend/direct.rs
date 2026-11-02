//! `DirectBackend`: Basic auth with an API token over an `HttpTransport`.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use san_mateo::transport::{HttpRequest, HttpTransport, TransportError};

use super::{
    ApiRequest, ApiResponse, BackendError, BackendInfo, BackendMode, JiraBackend,
    invalid_before_send,
};
use crate::site::{BasicAuth, JiraSite};

/// The `user-agent` header value.
const USER_AGENT: &str = concat!("autumn-jira/", env!("CARGO_PKG_VERSION"));

/// Sends each attempt to a fixed Jira site with Basic auth.
///
/// The URL is `site.base_url()` plus the path and the encoded query. The
/// backend sends the `Authorization` header pre-emptively. Use
/// `san_mateo::ReqwestTransport` in production: it follows no redirect and
/// caps the body size.
#[derive(Clone)]
pub struct DirectBackend {
    site: JiraSite,
    auth: BasicAuth,
    transport: Arc<dyn HttpTransport>,
}

impl DirectBackend {
    /// Makes the backend.
    #[must_use]
    pub fn new(site: JiraSite, auth: BasicAuth, transport: Arc<dyn HttpTransport>) -> Self {
        Self {
            site,
            auth,
            transport,
        }
    }

    /// Returns the site.
    #[must_use]
    pub const fn site(&self) -> &JiraSite {
        &self.site
    }
}

impl fmt::Debug for DirectBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirectBackend")
            .field("site", &self.site.label())
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// Maps a transport error to a backend error (plan 4.4).
pub(crate) fn map_transport_error(error: &TransportError) -> BackendError {
    match error {
        // `ReqwestTransport` maps a reset during the reply read to `Connect`
        // (G5). The request can have reached Jira.
        TransportError::Connect(_) => BackendError::MaybeSent { code: "connect" },
        TransportError::Timeout => BackendError::MaybeSent { code: "timeout" },
        // Jira replied, so a write ran.
        TransportError::BodyTooLarge => BackendError::MaybeSent {
            code: "body_too_large",
        },
        TransportError::Invalid(text) if invalid_before_send(text) => BackendError::NotSent {
            transient: false,
            code: "invalid_request",
        },
        // A reqwest send error: the request can have reached Jira.
        TransportError::Invalid(_) => BackendError::MaybeSent { code: "send_error" },
    }
}

#[async_trait]
impl JiraBackend for DirectBackend {
    async fn send(&self, request: ApiRequest) -> Result<ApiResponse, BackendError> {
        let url = format!("{}{}", self.site.base_url(), request.path_and_query());
        let mut http = HttpRequest::new(request.method.as_str(), url)
            .header("authorization", self.auth.authorization().expose())
            .header("accept", "application/json")
            .header("user-agent", USER_AGENT);
        if let Some(body) = request.body {
            http = http.header("content-type", "application/json").body(body);
        }
        let reply = self
            .transport
            .send(http)
            .await
            .map_err(|e| map_transport_error(&e))?;
        Ok(ApiResponse {
            status: reply.status,
            headers: reply.headers,
            body: reply.body,
        })
    }

    fn info(&self) -> BackendInfo {
        BackendInfo {
            mode: BackendMode::ApiToken,
            site: self.site.label(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use crate::backend::Method;
    use crate::error::Op;
    use crate::ids::SiteName;
    use san_mateo::transport::HttpResponse;
    use san_mateo::{MemoryTransport, Secret};

    const TOKEN: &str = "tok-very-secret";
    const EMAIL: &str = "bot@acme.example";

    fn backend(transport: &MemoryTransport) -> DirectBackend {
        DirectBackend::new(
            JiraSite::Subdomain(SiteName::parse("acme").unwrap()),
            BasicAuth::new(EMAIL, Secret::new(TOKEN)).unwrap(),
            Arc::new(transport.clone()),
        )
    }

    #[test]
    fn transport_error_map() {
        let cases = [
            (
                TransportError::Connect("reset".to_owned()),
                BackendError::MaybeSent { code: "connect" },
            ),
            (
                TransportError::Timeout,
                BackendError::MaybeSent { code: "timeout" },
            ),
            (
                TransportError::BodyTooLarge,
                BackendError::MaybeSent {
                    code: "body_too_large",
                },
            ),
            (
                TransportError::Invalid("bad URL".to_owned()),
                BackendError::NotSent {
                    transient: false,
                    code: "invalid_request",
                },
            ),
            // An unknown text is the safe side for a write.
            (
                TransportError::Invalid("bad".to_owned()),
                BackendError::MaybeSent { code: "send_error" },
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(map_transport_error(&error), expected, "{error:?}");
        }
    }

    #[test]
    fn only_a_check_before_the_send_is_not_sent() {
        // `ReqwestTransport` gives these texts before the request leaves.
        for local in [
            "bad HTTP method",
            "bad URL",
            "bad header name",
            "bad header value",
            "builder error",
        ] {
            assert_eq!(
                map_transport_error(&TransportError::Invalid(local.to_owned())),
                BackendError::NotSent {
                    transient: false,
                    code: "invalid_request",
                },
                "{local}"
            );
        }
        // A reqwest send error can come after Jira has the request.
        for sent in ["error sending request", "request or response body error"] {
            assert_eq!(
                map_transport_error(&TransportError::Invalid(sent.to_owned())),
                BackendError::MaybeSent { code: "send_error" },
                "{sent}"
            );
        }
    }

    #[test]
    fn debug_hides_the_token() {
        let text = format!("{:?}", backend(&MemoryTransport::new()));
        assert!(text.contains("acme.atlassian.net"), "{text}");
        assert!(!text.contains(TOKEN) && !text.contains(EMAIL), "{text}");
    }

    #[tokio::test]
    async fn one_attempt_with_basic_auth() {
        let transport = MemoryTransport::new();
        let url = "https://acme.atlassian.net/rest/api/3/issue/OPS-1";
        transport.reply("GET", url, HttpResponse::new(503, "busy"));
        transport.fail(
            "POST",
            "https://acme.atlassian.net/rest/api/3/issue",
            TransportError::Connect("reset".to_owned()),
        );
        let b = backend(&transport);
        let get = ApiRequest::new(Op::GetIssue, Method::Get, "/rest/api/3/issue/OPS-1")
            .query("fields", "summary,status");
        let reply = b.send(get).await.unwrap();
        assert_eq!(
            (reply.status, reply.body.as_slice()),
            (503, b"busy".as_slice())
        );
        let post = ApiRequest::new(Op::CreateIssue, Method::Post, "/rest/api/3/issue")
            .json(&serde_json::json!({"fields": {}}));
        assert_eq!(
            b.send(post).await.unwrap_err(),
            BackendError::MaybeSent { code: "connect" }
        );
        let sent = transport.requests();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].url, format!("{url}?fields=summary%2Cstatus"));
        let basic = format!(
            "Basic {}",
            data_encoding::BASE64.encode(format!("{EMAIL}:{TOKEN}").as_bytes())
        );
        assert_eq!(sent[0].header_value("authorization"), Some(basic.as_str()));
        assert_eq!(sent[0].header_value("accept"), Some("application/json"));
        assert_eq!(sent[0].header_value("content-type"), None);
        assert!(
            sent[0]
                .header_value("user-agent")
                .unwrap()
                .starts_with("autumn-jira/")
        );
        assert_eq!(
            sent[1].header_value("content-type"),
            Some("application/json")
        );
        assert_eq!(sent[1].body, br#"{"fields":{}}"#);
        assert_eq!(
            b.info(),
            BackendInfo {
                mode: BackendMode::ApiToken,
                site: "acme.atlassian.net".to_owned()
            }
        );
    }
}
