//! The pure parts of the webhook-to-signal bridge (plan 9.4).
//!
//! The bridge itself (`HarvestSignalBridge`) needs the Jira client and the
//! harvest database. It comes in WP5 part H1. The two functions here decide
//! which deliveries need routing and which routes of an issue property are
//! valid. They do no I/O.

use std::collections::BTreeSet;

use autumn_jira::JiraEvent;
use serde_json::Value;

use crate::route::{RouteKey, SignalRoute};

/// The result of one route of one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteOutcome {
    /// The route passed the checks, and the run got the signal.
    Signalled,
    /// The run already has a signal with this delivery id.
    Duplicate,
    /// No run has this workflow id.
    NoRun,
    /// The run is finished.
    Finished,
    /// The workflow name is not in the allowlist.
    NotAllowed,
    /// The tag does not verify. The bridge makes no database call.
    BadTag,
}

/// Pure: does this delivery need routing? jira:issue_created, or jira:issue_updated with a status change (B4, W26).
#[must_use]
pub fn wants(event: &JiraEvent) -> bool {
    let _ = event;
    false
}

/// Pure: the routes in a property value, kept only when allowed and the tag verifies.
///
/// Each item is a route and the result of its checks:
///
/// - [`RouteOutcome::BadTag`]: the tag does not verify. This check comes
///   first and runs in constant time.
/// - [`RouteOutcome::NotAllowed`]: the tag verifies, but the workflow name
///   is not in `allowed`. An empty `allowed` allows no name.
/// - [`RouteOutcome::Signalled`]: both checks pass. The bridge then sends the
///   signal and sets the final outcome of the route.
///
/// An entry with no `wf`, `id` or `tag` string, or with a name or an id that
/// breaks the [`SignalRoute::new`] rule, is not a route and is left out. A
/// value with no `routes` array gives no routes.
#[must_use]
pub fn routes_from_property(
    value: &Value,
    key: &RouteKey,
    allowed: &BTreeSet<String>,
) -> Vec<(SignalRoute, RouteOutcome)> {
    let _ = (value, key, allowed, SignalRoute::new("", ""));
    Vec::new()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use autumn_jira::webhook::parse_event;
    use san_mateo::Secret;
    use serde_json::json;

    fn event(value: &Value) -> JiraEvent {
        parse_event(&serde_json::to_vec(value).unwrap())
            .unwrap()
            .event
    }

    fn issue(event_name: &str, items: &Value) -> Value {
        json!({
            "timestamp": 1_606_480_436_302_i64,
            "webhookEvent": event_name,
            "issue": {"id": "99291", "key": "OPS-7", "fields": {"labels": ["autumn-tracked"]}},
            "changelog": {"id": "10124", "items": items}
        })
    }

    fn key() -> RouteKey {
        RouteKey::new(Secret::new("k".repeat(32))).unwrap()
    }

    #[test]
    fn wants_only_status_changes() {
        let status = json!([{"field": "status", "from": "1", "fromString": "To Do", "to": "3", "toString": "In Progress"}]);
        let summary = json!([{"field": "summary", "fromString": "a", "toString": "b"}]);
        assert!(wants(&event(&issue("jira:issue_created", &json!([])))));
        assert!(wants(&event(&issue("jira:issue_updated", &status))));
        assert!(!wants(&event(&issue("jira:issue_updated", &summary))));
        assert!(!wants(&event(&issue("jira:issue_updated", &json!([])))));
        assert!(!wants(&event(&issue("jira:issue_deleted", &status))));
        let comment = json!({
            "webhookEvent": "comment_created",
            "comment": {"id": "10100", "body": {"type": "doc", "version": 1, "content": []}},
            "issue": {"id": "99291", "key": "OPS-7", "fields": {}}
        });
        assert!(!wants(&event(&comment)));
        assert!(!wants(&event(&json!({"webhookEvent": "sprint_started"}))));
    }

    #[test]
    fn routes_need_tag_and_allowlist() {
        let key = key();
        let good = SignalRoute::new("incident_flow", "A-1").unwrap();
        let other = SignalRoute::new("other_flow", "B-2").unwrap();
        let forged = SignalRoute::new("incident_flow", "A-9").unwrap();
        let allowed: BTreeSet<String> = ["incident_flow".to_owned()].into();
        let value = json!({"routes": [
            good.property_entry(&key),
            other.property_entry(&key),
            {"wf": "incident_flow", "id": "A-9", "tag": good.tag(&key)},
            {"wf": "incident_flow", "id": "A-1"},
            {"wf": "a b", "id": "A-1", "tag": good.tag(&key)},
            {"wf": 7, "id": "A-1", "tag": good.tag(&key)},
            "not an object"
        ]});
        assert_eq!(
            routes_from_property(&value, &key, &allowed),
            vec![
                (good.clone(), RouteOutcome::Signalled),
                (other.clone(), RouteOutcome::NotAllowed),
                (forged, RouteOutcome::BadTag),
            ]
        );
        // An empty allowlist allows no name. A bad tag is still a bad tag.
        let none = BTreeSet::new();
        let outcomes: Vec<RouteOutcome> = routes_from_property(&value, &key, &none)
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect();
        assert_eq!(
            outcomes,
            vec![
                RouteOutcome::NotAllowed,
                RouteOutcome::NotAllowed,
                RouteOutcome::BadTag
            ]
        );
        // Another key makes every tag bad.
        let other_key = RouteKey::new(Secret::new("j".repeat(32))).unwrap();
        assert!(
            routes_from_property(&value, &other_key, &allowed)
                .iter()
                .all(|(_, outcome)| *outcome == RouteOutcome::BadTag)
        );
        for empty in [json!(null), json!({}), json!({"routes": {}}), json!([])] {
            assert!(routes_from_property(&empty, &key, &allowed).is_empty());
        }
    }

    /// The tag check uses the constant-time `verify_truncated_left` of the
    /// `hmac` crate. No code compares tag text or MAC bytes with `==`.
    #[test]
    fn tag_is_constant_time() {
        let source = include_str!("route.rs");
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("mac.verify_truncated_left(&expected)"));
        for line in code.lines() {
            let text = line.split("//").next().unwrap();
            let has_eq = text.contains("==") || text.contains("!=");
            let names_a_tag = text.contains("tag") || text.contains("expected");
            assert!(
                !(has_eq && names_a_tag && !text.contains(".len()")),
                "a plain compare on a tag: {line}"
            );
        }
        let bridge = include_str!("bridge.rs");
        let code = bridge.split("#[cfg(test)]").next().unwrap();
        assert!(code.contains("route.verify(key, tag)"));
        assert!(!code.contains("tag =="));
        assert!(!code.contains("== tag"));
        // The behaviour: a tag that differs in one char fails.
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let tag = route.tag(&key());
        let mut flipped = tag.clone().into_bytes();
        flipped[0] = if flipped[0] == b'a' { b'b' } else { b'a' };
        assert!(!route.verify(&key(), std::str::from_utf8(&flipped).unwrap()));
        assert!(route.verify(&key(), &tag));
    }
}
