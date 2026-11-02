//! One client call: attempts, waits and typed errors.
//!
//! The loop sends one attempt, classifies it, and asks
//! `policy::decide` for the next step. This file is the non-verified glue
//! of plan section 6: the map from an HTTP status or a `BackendError` to an
//! `Outcome`, and from a `Decision` to a client action.

use std::time::Duration;

use serde::de::DeserializeOwned;

use super::JiraClient;
use crate::backend::{ApiRequest, ApiResponse, BackendError, backend_error};
use crate::error::{JiraError, Op, Result, SCOPE_HINT, UnknownCause};
use crate::model::decode;
use crate::model::wire::ErrorCollection;
use crate::policy::{self, Decision, OpClass, Outcome};

/// The header of a CAPTCHA challenge.
const SERAPH_HEADER: &str = "X-Seraph-LoginReason";
/// The value of a CAPTCHA challenge.
const SERAPH_DENIED: &str = "AUTHENTICATION_DENIED";

/// The result of one attempt.
pub(crate) type Sent = std::result::Result<ApiResponse, BackendError>;

/// What the client does after one attempt.
pub(crate) enum Next {
    /// The reply has an expected 2xx status.
    Done(ApiResponse),
    /// Wait, then send again.
    Wait(Duration),
    /// A marked write can have run. Look up the marker.
    Confirm(UnknownCause),
    /// Stop with this error.
    Fail(JiraError),
}

/// How a call ended, when it did not fail.
pub(crate) enum Ended {
    /// The reply has an expected 2xx status.
    Reply(ApiResponse),
    /// A marked write can have run. Look up the marker.
    Confirm(UnknownCause),
}

/// A failed call.
pub(crate) struct Failure {
    /// The error.
    pub(crate) error: JiraError,
    /// `true` when an earlier attempt was a 5xx or "maybe sent".
    pub(crate) after_maybe_sent: bool,
}

/// Classifies one attempt for the policy.
pub(crate) const fn classify(sent: &Sent) -> Outcome {
    match sent {
        Ok(reply) => policy::classify_status(reply.status),
        Err(BackendError::NotSent {
            transient: true, ..
        }) => Outcome::NotSent,
        Err(BackendError::MaybeSent { .. }) => Outcome::MaybeSent,
        // Not transient, or bad credentials: the policy stops.
        Err(_) => Outcome::ClientError,
    }
}

/// Returns the `Retry-After` wait: integer seconds only. `Beta-Retry-After`
/// and dates are ignored (F34).
pub(crate) fn retry_after(reply: &ApiResponse) -> Option<Duration> {
    reply
        .header("Retry-After")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Maps a non-2xx reply to a typed error. The error holds no body text,
/// except the Jira `errorMessages` of a validation error, which are not in
/// `Display`.
pub(crate) fn status_error(op: Op, reply: &ApiResponse) -> JiraError {
    let status = reply.status;
    match status {
        300..=399 => JiraError::Redirect { op, status },
        401 | 403 => {
            let denied = reply
                .header(SERAPH_HEADER)
                .is_some_and(|v| v.trim().eq_ignore_ascii_case(SERAPH_DENIED));
            if denied {
                JiraError::CaptchaRequired { op }
            } else {
                JiraError::Unauthorized {
                    op,
                    status,
                    hint: SCOPE_HINT,
                }
            }
        }
        400 | 422 => {
            let errors = ErrorCollection::parse(&reply.body);
            JiraError::Validation {
                op,
                status,
                messages: errors.messages(),
                fields: errors.fields(),
            }
        }
        404 => JiraError::NotFoundOrForbidden { op },
        409 => JiraError::Conflict { op },
        410 => JiraError::Gone { op },
        413 => JiraError::PayloadTooLarge { op },
        429 => JiraError::RateLimited {
            op,
            retry_after: retry_after(reply),
            reason: reply.header("RateLimit-Reason").map(str::to_owned),
        },
        408 | 425 | 500.. => JiraError::Upstream { op, status },
        _ => JiraError::UnexpectedStatus { op, status },
    }
}

/// Returns the error of a call that stops after this attempt.
fn stop_error(op: Op, sent: &Sent) -> JiraError {
    match sent {
        Ok(reply) => status_error(op, reply),
        Err(e) => backend_error(op, e),
    }
}

/// Returns why the outcome of a write is unknown.
const fn unknown_cause(sent: &Sent) -> UnknownCause {
    match sent {
        Ok(reply) => UnknownCause::ServerError(reply.status),
        Err(BackendError::MaybeSent { code }) if matches!(code.as_bytes(), b"timeout") => {
            UnknownCause::Timeout
        }
        Err(_) => UnknownCause::Network,
    }
}

impl JiraClient {
    /// Returns a jitter factor in permille, from the entropy.
    fn permille(&self) -> u32 {
        let mut buf = [0_u8; 4];
        self.entropy.fill(&mut buf);
        let span = policy::JITTER_MAX_PERMILLE - policy::JITTER_MIN_PERMILLE + 1;
        policy::JITTER_MIN_PERMILLE + u32::from_le_bytes(buf) % span
    }

    /// Returns the wait before the next attempt, or `None` when a
    /// `Retry-After` asks for more than `max_backoff_ms`.
    fn wait(&self, attempt: u32, retry_after: Option<Duration>) -> Option<Duration> {
        let after_ms =
            retry_after.map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        let cap = self.settings.max_backoff_ms;
        let wait = policy::retry_wait_ms(attempt, self.settings.base_backoff_ms, cap, after_ms)?;
        let wait = if self.settings.jitter {
            policy::apply_jitter(wait, self.permille(), after_ms.unwrap_or(0), cap)
        } else {
            wait
        };
        Some(Duration::from_millis(wait))
    }

    /// Decides the next step after attempt number `attempt` (1-based).
    pub(crate) fn judge(&self, class: OpClass, op: Op, attempt: u32, sent: Sent) -> Next {
        let outcome = classify(&sent);
        let decision = policy::decide(class, outcome, attempt, self.settings.max_attempts);
        tracing::debug!(
            op = op.name(),
            attempt,
            status = sent.as_ref().ok().map(|r| r.status),
            ?outcome,
            ?decision,
            "jira attempt"
        );
        match decision {
            Decision::Done => match sent {
                Ok(reply) if op.expected().contains(&reply.status) => Next::Done(reply),
                Ok(reply) => Next::Fail(JiraError::UnexpectedStatus {
                    op,
                    status: reply.status,
                }),
                Err(e) => Next::Fail(backend_error(op, &e)),
            },
            Decision::Retry => {
                let after = sent.as_ref().ok().and_then(retry_after);
                self.wait(attempt, after)
                    .map_or_else(|| Next::Fail(stop_error(op, &sent)), Next::Wait)
            }
            Decision::Confirm => Next::Confirm(unknown_cause(&sent)),
            Decision::Unknown => {
                tracing::warn!(op = op.name(), "jira: the outcome of a write is unknown; not sent again");
                Next::Fail(JiraError::OutcomeUnknown {
                    op,
                    cause: unknown_cause(&sent),
                    marker: None,
                })
            }
            Decision::Stop => Next::Fail(stop_error(op, &sent)),
        }
    }

    /// Runs one call: attempts, waits, and the typed end.
    pub(crate) async fn run(
        &self,
        class: OpClass,
        request: &ApiRequest,
    ) -> std::result::Result<Ended, Failure> {
        let mut after_maybe_sent = false;
        let mut attempt = 1_u32;
        loop {
            let sent = self.backend.send(request.clone()).await;
            let maybe = matches!(classify(&sent), Outcome::ServerError | Outcome::MaybeSent);
            match self.judge(class, request.op, attempt, sent) {
                Next::Done(reply) => return Ok(Ended::Reply(reply)),
                Next::Confirm(cause) => return Ok(Ended::Confirm(cause)),
                Next::Fail(error) => {
                    return Err(Failure {
                        error,
                        after_maybe_sent,
                    });
                }
                Next::Wait(wait) => {
                    after_maybe_sent |= maybe;
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
            }
        }
    }

    /// Runs one call and returns the reply. A `Confirm` (marked writes
    /// only) gives `OutcomeUnknown` with no marker.
    pub(crate) async fn send_ok(&self, class: OpClass, request: &ApiRequest) -> Result<ApiResponse> {
        match self.run(class, request).await {
            Ok(Ended::Reply(reply)) => Ok(reply),
            Ok(Ended::Confirm(cause)) => Err(JiraError::OutcomeUnknown {
                op: request.op,
                cause,
                marker: None,
            }),
            Err(failure) => Err(failure.error),
        }
    }

    /// Runs one read and decodes the reply.
    pub(crate) async fn read<T: DeserializeOwned>(&self, request: &ApiRequest) -> Result<T> {
        let reply = self.send_ok(OpClass::Read, request).await?;
        decode(request.op, &reply.body)
    }
}
