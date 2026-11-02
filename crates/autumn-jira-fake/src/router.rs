//! The request router: hosts, auth, op counts, faults and delayed writes.

use std::sync::Arc;

use san_mateo::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};
use tokio::time::Instant;

use crate::api::{self, Call, Route};
use crate::oauth::{self, Bearer};
use crate::render;
use crate::state::{PendingCommit, Planned, State};
use crate::types::{FakeOp, FakeRequest, Fault};
use crate::wire::{self, Url};

/// The OAuth token host.
const AUTH_HOST: &str = "auth.atlassian.com";
/// The API gateway host.
const API_HOST: &str = "api.atlassian.com";
/// The token path on the token host.
const TOKEN_PATH: &str = "/oauth/token";
/// The accessible-resources path on the gateway host.
const RESOURCES_PATH: &str = "/oauth/token/accessible-resources";
/// The REST API prefix.
const REST_PREFIX: &str = "/rest/api/3/";

/// The result of one request.
pub(crate) enum Served {
    /// The fake answers.
    Done(Result<HttpResponse, TransportError>),
    /// Another transport answers.
    Delegate(Arc<dyn HttpTransport>),
}

/// What a request is for, on this site.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A REST path. `gateway` is true on `api.atlassian.com/ex/jira/..`.
    Rest {
        path: String,
        gateway: bool,
    },
    Resources,
    Token,
}

/// Whether this site owns a request.
#[derive(Debug)]
enum Decision {
    Mine(Target),
    /// Not for this site. The value is the answer when no fallback exists.
    Foreign(Result<HttpResponse, TransportError>),
}

/// Returns the REST path of a site or gateway URL, without the cloud id.
fn split_gateway<'a>(path: &'a str, cloud_id: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/ex/jira/")?;
    let (id, tail) = rest.split_once('/').unwrap_or((rest, ""));
    (id == cloud_id).then_some(tail)
}

impl State {
    /// Serves one request. With `may_delegate`, a request for another site
    /// goes to the fallback transport, when there is one.
    pub(crate) fn serve(&mut self, request: &HttpRequest, may_delegate: bool) -> Served {
        let Some(url) = wire::parse_url(&request.url) else {
            return Served::Done(Err(TransportError::Invalid(
                "the fake needs an https URL".to_owned(),
            )));
        };
        self.tick();
        let record = FakeRequest {
            method: request.method.clone(),
            host: url.host.to_owned(),
            path: url.path.to_owned(),
            query: url.query.to_owned(),
        };
        match self.decide(request, url) {
            Decision::Foreign(default) => {
                if may_delegate && let Some(other) = self.fallback.clone() {
                    return Served::Delegate(other);
                }
                self.requests.push(record);
                match (url.host, url.path) {
                    (AUTH_HOST, TOKEN_PATH) => self.count(FakeOp::Token),
                    (API_HOST, RESOURCES_PATH) => self.count(FakeOp::AccessibleResources),
                    _ => {}
                }
                Served::Done(default)
            }
            Decision::Mine(target) => {
                self.requests.push(record);
                Served::Done(self.answer(request, url, target))
            }
        }
    }

    /// Decides if this site owns a request.
    fn decide(&self, request: &HttpRequest, url: Url<'_>) -> Decision {
        if url.host == self.site_host() {
            return Decision::Mine(Target::Rest {
                path: url.path.to_owned(),
                gateway: false,
            });
        }
        match url.host {
            API_HOST if url.path == RESOURCES_PATH => {
                if oauth::bearer(self, request) == Bearer::Unknown {
                    Decision::Foreign(Ok(oauth::gateway_unauthorized()))
                } else {
                    Decision::Mine(Target::Resources)
                }
            }
            API_HOST => split_gateway(url.path, &self.cloud_id).map_or_else(
                || Decision::Foreign(Ok(api::not_found())),
                |tail| {
                    Decision::Mine(Target::Rest {
                        path: format!("/{tail}"),
                        gateway: true,
                    })
                },
            ),
            AUTH_HOST if url.path == TOKEN_PATH => {
                if request.method != "POST" || oauth::owns_client(self, request) {
                    Decision::Mine(Target::Token)
                } else {
                    Decision::Foreign(Ok(oauth::unknown_client()))
                }
            }
            AUTH_HOST => Decision::Mine(Target::Rest {
                path: url.path.to_owned(),
                gateway: false,
            }),
            other => Decision::Foreign(Err(TransportError::Connect(format!(
                "the fake has no host {other}"
            )))),
        }
    }

    /// Answers a request that this site owns.
    fn answer(
        &mut self,
        request: &HttpRequest,
        url: Url<'_>,
        target: Target,
    ) -> Result<HttpResponse, TransportError> {
        match target {
            Target::Token => {
                self.count(FakeOp::Token);
                if request.method != "POST" {
                    return Ok(api::method_not_allowed());
                }
                self.with_fault(FakeOp::Token, request, |state, _| {
                    oauth::token(state, request)
                })
            }
            Target::Resources => {
                self.count(FakeOp::AccessibleResources);
                if request.method != "GET" {
                    return Ok(api::method_not_allowed());
                }
                if oauth::bearer(self, request) != Bearer::Valid {
                    return Ok(oauth::gateway_unauthorized());
                }
                self.with_fault(FakeOp::AccessibleResources, request, |state, _| {
                    oauth::resources(state)
                })
            }
            Target::Rest { path, gateway } => {
                let Some(segments) = rest_segments(&path) else {
                    return Ok(api::not_found());
                };
                let route = api::classify(&request.method, &segments);
                let op = match route {
                    Route::NotFound => return Ok(api::not_found()),
                    Route::MethodNotAllowed => return Ok(api::method_not_allowed()),
                    Route::Gone => None,
                    Route::Op(op) => {
                        self.count(op);
                        Some(op)
                    }
                };
                if !self.authorized(request, gateway) {
                    return Ok(unauthorized());
                }
                let Some(op) = op else {
                    return Ok(api::gone());
                };
                let query = wire::parse_pairs(url.query);
                self.with_fault(op, request, |state, repeat_token| {
                    let call = Call {
                        method: &request.method,
                        segments,
                        query,
                        body: &request.body,
                        actor: Self::caller(),
                        repeat_token,
                    };
                    api::serve(state, op, &call)
                })
            }
        }
    }

    /// Checks the auth of a REST call. The site host takes Basic auth. The
    /// gateway takes Basic auth (a scoped API token) or a live Bearer token.
    fn authorized(&self, request: &HttpRequest, gateway: bool) -> bool {
        let Some(header) = request.header_value("authorization") else {
            return false;
        };
        if let Some((user, password)) = wire::basic_credentials(header) {
            return user == self.basic.0 && password == self.basic.1;
        }
        gateway && oauth::bearer(self, request) == Bearer::Valid
    }

    /// Runs one op under its next planned fault, if any.
    fn with_fault(
        &mut self,
        op: FakeOp,
        request: &HttpRequest,
        run: impl FnOnce(&mut Self, bool) -> HttpResponse,
    ) -> Result<HttpResponse, TransportError> {
        let Some(planned) = self.take_planned(op) else {
            return Ok(run(self, false));
        };
        let fault = match planned {
            Planned::RateLimit(secs) => return Ok(self.rate_limited(secs)),
            Planned::Fault(fault) => fault,
        };
        let reset = || TransportError::Connect("connection reset by the fake".to_owned());
        match fault {
            Fault::Status(status) => Ok(status_reply(status)),
            Fault::StatusAfterCommit(status) => {
                run(self, false);
                Ok(status_reply(status))
            }
            Fault::Network => Err(reset()),
            Fault::NetworkAfterCommit => {
                run(self, false);
                Err(reset())
            }
            Fault::Timeout => Err(TransportError::Timeout),
            Fault::TimeoutThenCommit(delay) => {
                self.pending_commits.push(PendingCommit {
                    due: Instant::now() + delay,
                    op,
                    request: request.clone(),
                });
                Err(TransportError::Timeout)
            }
            Fault::Redirect => {
                Ok(HttpResponse::new(302, Vec::new()).header("location", "https://evil.example/"))
            }
            Fault::RepeatToken => Ok(run(self, true)),
            Fault::BadJson => {
                let reply = run(self, false);
                if (200..300).contains(&reply.status) {
                    let status = if reply.status == 201 { 201 } else { 200 };
                    Ok(HttpResponse::new(status, b"<html>not json".to_vec())
                        .header("content-type", "application/json"))
                } else {
                    Ok(reply)
                }
            }
        }
    }

    /// Returns a 429 reply with the Jira rate limit headers.
    fn rate_limited(&self, retry_after_secs: u64) -> HttpResponse {
        let reset = self.clock.now()
            + chrono::TimeDelta::seconds(
                i64::try_from(retry_after_secs).unwrap_or(i64::MAX / 1000),
            );
        HttpResponse::json(429, &render::errors(&["Rate limit exceeded."], &[]))
            .header("retry-after", retry_after_secs.to_string())
            .header("ratelimit-reason", "jira-burst-based")
            .header("x-ratelimit-limit", "100")
            .header("x-ratelimit-remaining", "0")
            .header(
                "x-ratelimit-reset",
                reset.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            )
    }

    /// Applies the delayed writes and the index changes that are due.
    pub(crate) fn tick(&mut self) {
        for commit in self.take_due_commits() {
            self.commit_later(&commit);
        }
        self.tick_index();
    }

    /// Runs a delayed write. Its reply is lost, as the client saw a timeout.
    fn commit_later(&mut self, commit: &PendingCommit) {
        if commit.op == FakeOp::Token {
            oauth::token(self, &commit.request);
            return;
        }
        let Some(url) = wire::parse_url(&commit.request.url) else {
            return;
        };
        let path = if url.host == API_HOST {
            split_gateway(url.path, &self.cloud_id).map(|tail| format!("/{tail}"))
        } else {
            Some(url.path.to_owned())
        };
        let Some(segments) = path.as_deref().and_then(rest_segments) else {
            return;
        };
        let call = Call {
            method: &commit.request.method,
            segments,
            query: wire::parse_pairs(url.query),
            body: &commit.request.body,
            actor: Self::caller(),
            repeat_token: false,
        };
        api::serve(self, commit.op, &call);
    }
}

/// Returns the decoded segments of a `/rest/api/3/...` path.
fn rest_segments(path: &str) -> Option<Vec<String>> {
    let rest = path.strip_prefix(REST_PREFIX)?;
    Some(
        rest.split('/')
            .filter(|s| !s.is_empty())
            .map(|s| wire::percent_decode(s, false))
            .collect(),
    )
}

/// The reply to a REST call with no valid auth.
fn unauthorized() -> HttpResponse {
    HttpResponse::json(
        401,
        &render::errors(
            &["Client must be authenticated to access this resource."],
            &[],
        ),
    )
}

/// The reply of `Fault::Status`.
fn status_reply(status: u16) -> HttpResponse {
    HttpResponse::json(
        status,
        &render::errors(&[&format!("The fake replies with HTTP {status}.")], &[]),
    )
}
