//! The OAuth hosts: the token endpoint with refresh token rotation, and
//! accessible-resources.

use san_mateo::transport::{HttpRequest, HttpResponse};
use serde_json::{Value, json};

use crate::state::State;
use crate::wire;

/// The lifetime of a new access token, in seconds.
const EXPIRES_IN: u64 = 3600;

/// The scopes of a token.
const SCOPES: &str = "read:jira-work write:jira-work read:jira-user offline_access";

/// The parameters of a token request.
#[derive(Debug, Default)]
pub(crate) struct TokenRequest {
    pub(crate) grant_type: String,
    pub(crate) client_id: String,
    pub(crate) client_secret: String,
    pub(crate) refresh_token: String,
}

impl TokenRequest {
    /// Reads a JSON or form body. Basic auth can hold the client id and
    /// secret.
    pub(crate) fn read(request: &HttpRequest) -> Self {
        let pairs: Vec<(String, String)> = match serde_json::from_slice::<Value>(&request.body) {
            Ok(Value::Object(map)) => map
                .into_iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_owned())))
                .collect(),
            _ => wire::parse_pairs(&String::from_utf8_lossy(&request.body)),
        };
        let get = |name: &str| wire::first(&pairs, name).unwrap_or_default().to_owned();
        let mut out = Self {
            grant_type: get("grant_type"),
            client_id: get("client_id"),
            client_secret: get("client_secret"),
            refresh_token: get("refresh_token"),
        };
        if let Some((id, secret)) = request
            .header_value("authorization")
            .and_then(wire::basic_credentials)
        {
            out.client_id = wire::percent_decode(&id, true);
            out.client_secret = wire::percent_decode(&secret, true);
        }
        out
    }
}

/// Returns an OAuth error reply.
pub(crate) fn oauth_error(status: u16, error: &str, description: &str) -> HttpResponse {
    HttpResponse::json(
        status,
        &json!({"error": error, "error_description": description}),
    )
}

/// The reply to an unknown client.
pub(crate) fn unknown_client() -> HttpResponse {
    oauth_error(401, "invalid_client", "Unauthorized")
}

/// Returns `true` when the token request names the client of this site.
pub(crate) fn owns_client(state: &State, request: &HttpRequest) -> bool {
    let wanted = TokenRequest::read(request);
    state
        .oauth
        .as_ref()
        .is_some_and(|o| !wanted.client_id.is_empty() && o.client_id == wanted.client_id)
}

/// Serves `POST https://auth.atlassian.com/oauth/token`.
pub(crate) fn token(state: &mut State, request: &HttpRequest) -> HttpResponse {
    let wanted = TokenRequest::read(request);
    let site = state.site.clone();
    let Some(oauth) = state.oauth.as_mut() else {
        return unknown_client();
    };
    if oauth.client_id != wanted.client_id || oauth.client_secret != wanted.client_secret {
        return unknown_client();
    }
    if wanted.grant_type != "refresh_token" {
        return oauth_error(
            400,
            "unsupported_grant_type",
            "The fake supports grant_type refresh_token only.",
        );
    }
    if wanted.refresh_token.is_empty() || wanted.refresh_token != oauth.refresh {
        return oauth_error(403, "invalid_grant", "Unknown or invalid refresh token.");
    }
    oauth.issued += 1;
    let access = format!("{site}-at-{}", oauth.issued);
    let refresh = format!("{site}-rt-{}", oauth.issued);
    oauth.access.insert(access.clone());
    oauth.refresh.clone_from(&refresh);
    state.refreshes += 1;
    HttpResponse::json(
        200,
        &json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_in": EXPIRES_IN,
            "token_type": "Bearer",
            "scope": SCOPES,
        }),
    )
}

/// The token state of a Bearer header on this site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bearer {
    /// No Bearer header.
    Missing,
    /// A live token of this site.
    Valid,
    /// A token of this site that `expire_access_token` ended.
    Expired,
    /// A token that this site never issued.
    Unknown,
}

/// Checks the Bearer header of a request.
pub(crate) fn bearer(state: &State, request: &HttpRequest) -> Bearer {
    let Some(token) = request
        .header_value("authorization")
        .and_then(wire::bearer_token)
    else {
        return Bearer::Missing;
    };
    match &state.oauth {
        Some(o) if o.access.contains(token) => Bearer::Valid,
        Some(o) if o.expired.contains(token) => Bearer::Expired,
        _ => Bearer::Unknown,
    }
}

/// The reply of the API gateway for bad auth.
pub(crate) fn gateway_unauthorized() -> HttpResponse {
    HttpResponse::json(401, &json!({"code": 401, "message": "Unauthorized"}))
}

/// Serves `GET https://api.atlassian.com/oauth/token/accessible-resources`.
/// One Jira entry, and one Confluence-only entry (F31).
pub(crate) fn resources(state: &State) -> HttpResponse {
    let url = state.base();
    let wiki_id = format!("ffffffff{}", state.cloud_id.get(8..).unwrap_or_default());
    let avatar = "https://site-admin-avatar-cdn.prod.public.atl-paas.net/avatars/240/rocket.png";
    HttpResponse::json(
        200,
        &json!([
            {
                "id": state.cloud_id,
                "url": url,
                "name": state.site,
                "scopes": ["read:jira-work", "write:jira-work", "read:jira-user", "manage:jira-webhook"],
                "avatarUrl": avatar,
            },
            {
                "id": wiki_id,
                "url": url,
                "name": state.site,
                "scopes": ["read:confluence-content.all", "write:confluence-content"],
                "avatarUrl": avatar,
            },
        ]),
    )
}
