//! Reads the in headers of an exchange.
//!
//! Two kinds of in headers exist:
//!
//! - The override headers `JiraIssueKey`, `JiraStatus` and `JiraJql`. They
//!   replace an option only with `allowHeaderOverrides=true`. A header from
//!   an outside caller must not pick the issue, the status or the query.
//! - The harvest-camel headers `HarvestIdempotencyKey` and
//!   `HarvestActivityAttempt` of a marked write (plan D7).
//!
//! The write rule also reads the camel redelivery headers
//! `CamelRedelivered` and `CamelRedeliveryCounter`. A camel redelivery
//! sends the exchange to the step again, and the earlier try can have sent
//! the write.
//!
//! An error text names the header and the rule. It never holds the header
//! value.

use autumn_jira::ids::IssueRef;
use autumn_jira::policy::Previous;
use autumn_jira::{IdempotencyKey, Jql, StatusTarget};
use camel_api::{CamelError, HEADER_REDELIVERED, HEADER_REDELIVERY_COUNTER, Message, Value};

use crate::options::{EndpointOptions, parse_status};
use crate::{HEADER_ISSUE_KEY, HEADER_JQL, HEADER_STATUS};

/// The harvest-camel header with the activity idempotency key. It is the
/// same for each attempt of one activity.
pub(crate) const HEADER_HARVEST_KEY: &str = "HarvestIdempotencyKey";

/// The harvest-camel header with the 1-based activity attempt.
pub(crate) const HEADER_HARVEST_ATTEMPT: &str = "HarvestActivityAttempt";

/// The key and the previous attempt of a marked write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marked {
    /// The idempotency key of the write.
    pub(crate) key: IdempotencyKey,
    /// What the previous attempt did, as far as the route can know it.
    pub(crate) previous: Previous,
}

/// Returns a header value. The match is exact first and then with no
/// regard to case. A null value counts as no value.
fn header<'a>(message: &'a Message, name: &str) -> Option<&'a Value> {
    message
        .header(name)
        .or_else(|| message.header_ic(name))
        .filter(|v| !v.is_null())
}

/// A `ValidationError` for a bad header.
fn bad_header(name: &str, rule: &str) -> CamelError {
    CamelError::ValidationError(format!("the {name} header {rule}"))
}

/// Reads the harvest-camel headers of a marked write.
///
/// `HarvestIdempotencyKey` is the only source of the key (W12). An attempt
/// above 1 gives `Previous::Unknown`: the camel route cannot see the
/// failure of the previous attempt, so it must assume that the write can
/// have run. A camel redelivery also gives `Previous::Unknown`, whatever
/// the attempt header says: the earlier try of this step can have sent the
/// write. Then the producer does a lookup only.
///
/// No key header gives `None`: the producer sends the write at most once.
/// So an unmarked write on a camel redelivery fails, and the producer
/// sends nothing.
///
/// # Errors
///
/// `ValidationError` for a bad header, for no key header with
/// `requireIdempotencyKey=true`, and for no key header on a camel
/// redelivery.
pub(crate) fn marked(
    message: &Message,
    options: &EndpointOptions,
) -> Result<Option<Marked>, CamelError> {
    let op = options.operation;
    let key = match header(message, HEADER_HARVEST_KEY) {
        None if options.require_idempotency_key => {
            return Err(CamelError::ValidationError(format!(
                "{op} needs the {HEADER_HARVEST_KEY} header: the route sets \
                 requireIdempotencyKey=true"
            )));
        }
        None if redelivered(message) => {
            return Err(CamelError::ValidationError(format!(
                "{op} has no {HEADER_HARVEST_KEY} header, and this is a camel redelivery: \
                 the earlier try can have sent the write, so the component does not send \
                 it again; set the {HEADER_HARVEST_KEY} header to get a marker lookup"
            )));
        }
        None => return Ok(None),
        // The harvest key is not always in the strict key form, so a key
        // that does not parse is hashed. The hash is the same on each
        // attempt.
        Some(Value::String(text)) if !text.trim().is_empty() => {
            IdempotencyKey::parse(text).unwrap_or_else(|_| IdempotencyKey::derive(&[text.as_str()]))
        }
        Some(_) => return Err(bad_header(HEADER_HARVEST_KEY, "must be a non-empty string")),
    };
    let attempt = match header(message, HEADER_HARVEST_ATTEMPT) {
        None => Some(1),
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(text)) => text.trim().parse::<u64>().ok(),
        Some(_) => None,
    }
    .filter(|n| *n >= 1)
    .ok_or_else(|| bad_header(HEADER_HARVEST_ATTEMPT, "must be a whole number from 1"))?;
    let previous = if attempt > 1 || redelivered(message) {
        Previous::Unknown
    } else {
        Previous::First
    };
    Ok(Some(Marked { key, previous }))
}

/// Returns `true` when the message is a camel redelivery.
///
/// `CamelRedelivered` that is not `false`, or `CamelRedeliveryCounter`
/// that is not 0, gives `true`. A value of an other type also gives
/// `true`: the rule fails closed.
///
/// A camel redelivery header can also come from an earlier step that camel
/// sent again. The step cannot see the difference, so it also does a
/// lookup only, or fails for an unmarked write.
fn redelivered(message: &Message) -> bool {
    let flag = match header(message, HEADER_REDELIVERED) {
        None | Some(Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.trim().eq_ignore_ascii_case("false"),
        Some(_) => true,
    };
    let counter = match header(message, HEADER_REDELIVERY_COUNTER) {
        None => false,
        Some(Value::Number(n)) => n.as_u64() != Some(0),
        Some(Value::String(text)) => text.trim() != "0",
        Some(_) => true,
    };
    flag || counter
}

/// Reads the issue: the `JiraIssueKey` header or the `issueKey` option.
///
/// # Errors
///
/// `ValidationError` for a bad or a not allowed header, or for no issue.
pub(crate) fn issue(message: &Message, options: &EndpointOptions) -> Result<IssueRef, CamelError> {
    let parse = |value: &Value| {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        IssueRef::parse(&text).map_err(|_| {
            bad_header(
                HEADER_ISSUE_KEY,
                "must be an issue key or a numeric issue id",
            )
        })
    };
    overridden(
        message,
        options,
        Override::new(HEADER_ISSUE_KEY, "issueKey"),
        options.issue_key.as_ref(),
        parse,
    )
}

/// Reads the target status: the `JiraStatus` header or the `status`
/// option.
///
/// # Errors
///
/// `ValidationError` for a bad or a not allowed header, or for no status.
pub(crate) fn status(
    message: &Message,
    options: &EndpointOptions,
) -> Result<StatusTarget, CamelError> {
    let parse = |value: &Value| match value {
        Value::String(text) => parse_status(text).map_err(|_| {
            bad_header(
                HEADER_STATUS,
                "must be a status name, id:<n> or category:<key>",
            )
        }),
        _ => Err(bad_header(HEADER_STATUS, "must be a string")),
    };
    overridden(
        message,
        options,
        Override::new(HEADER_STATUS, "status"),
        options.status.as_ref(),
        parse,
    )
}

/// Reads the query of `search`: the `project` option, or the `JiraJql`
/// header or the `jql` option.
///
/// A `JiraJql` header next to the `project` option fails, also with
/// `allowHeaderOverrides`: the JQL is raw, so the component does not add a
/// project clause to it, and the header would drop the project filter.
///
/// # Errors
///
/// `ValidationError` for a bad or a not allowed header, or for no query.
pub(crate) fn search_jql(message: &Message, options: &EndpointOptions) -> Result<Jql, CamelError> {
    if let Some(project) = &options.project {
        if header(message, HEADER_JQL).is_none() {
            return Ok(Jql::project(project));
        }
        if options.allow_header_overrides {
            return Err(bad_header(
                HEADER_JQL,
                "cannot replace the project option: the JQL is raw, so the component \
                 does not add a project clause to it",
            ));
        }
        return Err(not_allowed(HEADER_JQL, "jql"));
    }
    let parse = |value: &Value| match value {
        Value::String(text) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(bad_header(HEADER_JQL, "must be a non-empty JQL string")),
    };
    let jql = overridden(
        message,
        options,
        Override::new(HEADER_JQL, "jql"),
        options.jql.as_ref(),
        parse,
    )?;
    Ok(Jql::raw(jql))
}

/// An in header that can replace an option.
#[derive(Debug, Clone, Copy)]
struct Override {
    /// The header name, for example `JiraIssueKey`.
    header: &'static str,
    /// The option name, for example `issueKey`.
    option: &'static str,
}

impl Override {
    /// Pairs a header with its option.
    const fn new(header: &'static str, option: &'static str) -> Self {
        Self { header, option }
    }
}

/// The error of an override header without `allowHeaderOverrides` (R18).
fn not_allowed(header: &str, option: &str) -> CamelError {
    CamelError::ValidationError(format!(
        "the {header} header can replace the {option} option only with \
         allowHeaderOverrides=true; the endpoint does not set it, so the \
         header must not be there or must equal the option"
    ))
}

/// Returns the value of an option that a header can replace.
///
/// With `allowHeaderOverrides`, the header wins over the option. Without
/// it, a header that differs from the option fails the exchange, and does
/// not stay silent (R18). A header equal to the option changes nothing, so
/// it passes: it is often the out header of an earlier `jira:` step on the
/// same issue.
fn overridden<T: Clone + PartialEq>(
    message: &Message,
    options: &EndpointOptions,
    rule: Override,
    option: Option<&T>,
    parse: impl Fn(&Value) -> Result<T, CamelError>,
) -> Result<T, CamelError> {
    match header(message, rule.header) {
        Some(value) if options.allow_header_overrides => parse(value),
        Some(value) => match (parse(value).ok(), option) {
            (Some(parsed), Some(option)) if parsed == *option => Ok(parsed),
            _ => Err(not_allowed(rule.header, rule.option)),
        },
        None => option.cloned().ok_or_else(|| {
            CamelError::ValidationError(format!(
                "{} needs the {} header: the endpoint has no {} option",
                options.operation, rule.header, rule.option
            ))
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(uri: &str) -> EndpointOptions {
        EndpointOptions::parse(uri, &["default"]).unwrap()
    }

    fn message(headers: &[(&str, Value)]) -> Message {
        let mut message = Message::default();
        for (name, value) in headers {
            message.set_header(*name, value.clone());
        }
        message
    }

    #[test]
    fn marked_reads_the_key_and_the_attempt() {
        let o = options("jira:createIssue?project=OPS&issueType=Task");
        assert_eq!(marked(&message(&[]), &o).unwrap(), None);
        // A null header counts as no header.
        assert_eq!(
            marked(&message(&[(HEADER_HARVEST_KEY, json!(null))]), &o).unwrap(),
            None
        );
        let first = marked(&message(&[(HEADER_HARVEST_KEY, json!("k-1"))]), &o)
            .unwrap()
            .unwrap();
        assert_eq!(first.previous, Previous::First);
        // The case of the name does not matter.
        let again = marked(
            &message(&[
                ("harvestidempotencykey", json!("k-1")),
                ("harvestactivityattempt", json!(4)),
            ]),
            &o,
        )
        .unwrap()
        .unwrap();
        assert_eq!(again.key, first.key, "the key is stable across attempts");
        assert_eq!(again.previous, Previous::Unknown);
        let one = marked(
            &message(&[
                (HEADER_HARVEST_KEY, json!("k-1")),
                (HEADER_HARVEST_ATTEMPT, json!("1")),
            ]),
            &o,
        )
        .unwrap()
        .unwrap();
        assert_eq!(one.previous, Previous::First);
        for bad in [json!(0), json!(1.5), json!("two"), json!([2])] {
            let m = message(&[
                (HEADER_HARVEST_KEY, json!("k-1")),
                (HEADER_HARVEST_ATTEMPT, bad.clone()),
            ]);
            let err = marked(&m, &o).unwrap_err();
            assert!(
                err.to_string().contains(HEADER_HARVEST_ATTEMPT),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn a_camel_redelivery_looks_up_only() {
        let o = options("jira:createIssue?project=OPS&issueType=Task");
        let key = (HEADER_HARVEST_KEY, json!("k-1"));
        let first = (HEADER_HARVEST_ATTEMPT, json!(1));
        for redelivery in [
            (HEADER_REDELIVERED, json!(true)),
            (HEADER_REDELIVERED, json!("TRUE")),
            (HEADER_REDELIVERED, json!(1)),
            ("camelredelivered", json!(true)),
            (HEADER_REDELIVERY_COUNTER, json!(1)),
            (HEADER_REDELIVERY_COUNTER, json!("2")),
            (HEADER_REDELIVERY_COUNTER, json!(-1)),
            (HEADER_REDELIVERY_COUNTER, json!([1])),
        ] {
            let m = message(&[key.clone(), first.clone(), redelivery.clone()]);
            let marked = marked(&m, &o).unwrap().unwrap();
            assert_eq!(marked.previous, Previous::Unknown, "{redelivery:?}");
            // With no key, the write fails and names the key header.
            let err = super::marked(&message(&[redelivery.clone()]), &o).unwrap_err();
            assert!(err.to_string().contains(HEADER_HARVEST_KEY), "{err}");
        }
        for none in [
            (HEADER_REDELIVERED, json!(false)),
            (HEADER_REDELIVERED, json!(" false ")),
            (HEADER_REDELIVERED, json!(null)),
            (HEADER_REDELIVERY_COUNTER, json!(0)),
            (HEADER_REDELIVERY_COUNTER, json!("0")),
        ] {
            let m = message(&[key.clone(), first.clone(), none.clone()]);
            assert_eq!(marked(&m, &o).unwrap().unwrap().previous, Previous::First);
            assert_eq!(marked(&message(&[none.clone()]), &o).unwrap(), None);
        }
    }

    #[test]
    fn a_header_equal_to_the_option_passes() {
        let o = options("jira:getIssue?issueKey=10001");
        let m = message(&[(HEADER_ISSUE_KEY, json!(10001))]);
        assert_eq!(issue(&m, &o).unwrap().to_string(), "10001");
        let m = message(&[(HEADER_ISSUE_KEY, json!("10002"))]);
        assert!(
            issue(&m, &o)
                .unwrap_err()
                .to_string()
                .contains("allowHeaderOverrides")
        );
        // A header that does not parse is also not allowed: no value leaks.
        let m = message(&[(HEADER_ISSUE_KEY, json!("../secret"))]);
        let err = issue(&m, &o).unwrap_err().to_string();
        assert!(
            err.contains("allowHeaderOverrides") && !err.contains("secret"),
            "{err}"
        );
        let o = options("jira:transitionToStatus?issueKey=OPS-1&status=category:done");
        let m = message(&[(HEADER_STATUS, json!("category:done"))]);
        assert_eq!(status(&m, &o).unwrap(), StatusTarget::done());
        let o = options("jira:search?jql=labels%20%3D%20a");
        let m = message(&[(HEADER_JQL, json!("labels = a"))]);
        assert_eq!(search_jql(&m, &o).unwrap().as_str(), "labels = a");
    }
}
