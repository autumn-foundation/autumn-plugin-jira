//! Signal routes: which workflow run waits on an issue (D11).
//!
//! A route sits in the issue property [`ROUTES_PROPERTY`] as
//! `{"routes": [{"wf": <name>, "id": <workflow id>, "tag": <tag>}]}`. A
//! person can edit an issue property, so each route has a tag. Only a
//! holder of the [`RouteKey`] can make a tag, so a person cannot point an
//! issue at another run. The check of a tag runs in constant time.
//!
//! The tag binds the run and the issue (v2, R2F-fixcheck-a-3). It is the
//! first 32 lower hex chars of this MAC:
//!
//! ```text
//! HMAC-SHA256(key, "v2\n" + name + "\n" + id + "\n" + kind + "\n" + value)
//! ```
//!
//! A [`RouteBinding`] gives the kind and the value:
//!
//! - `label`: the marker label of a create. The client knows it before the
//!   POST, and it is unique for each create key. So the create body holds
//!   the route, and no second write is necessary.
//! - `issue`: the issue id, for `jira_watch_issue`. The watch reads the
//!   issue first, so it knows the id.
//!
//! The bridge checks a tag against the id and the labels of the issue that
//! it reads on its own site. A copy of an entry on another issue does not
//! verify there, so it gives `BadTag` and no signal.
//!
//! Limit: a person who can edit the labels and the properties of an issue
//! can copy an entry together with the marker label. The bridge
//! cannot tell that copy from the issue of the create. Give the permission
//! to edit issues only to people that you trust.
//!
//! A v1 tag binds the run only: `HMAC-SHA256(key, "v1\n" + name + "\n" + id)`.
//! A copy of a v1 entry verifies on each issue. So the bridge refuses
//! v1 tags, unless the app opts in with
//! `HarvestSignalBridge::accept_unbound_v1_tags` for the upgrade.

use std::fmt;

use autumn_jira::policy::is_lower_hex;
use autumn_jira::{IssueId, JiraError};
use hmac::{Hmac, Mac};
use san_mateo::Secret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;

/// The issue property that holds the routes.
pub const ROUTES_PROPERTY: &str = "autumn-jira-routes";

/// The label of an issue with routes. The bridge reads the property only
/// for an issue with this label.
pub const TRACKED_LABEL: &str = "autumn-tracked";

/// The smallest route key, in bytes.
pub const MIN_ROUTE_KEY_BYTES: usize = 32;

/// The length of a tag, in hex chars (16 bytes).
pub const TAG_LEN: usize = 32;

/// The longest workflow name or workflow id of a route, in chars.
pub const MAX_ROUTE_PART: usize = 200;

/// The version prefix of the tag input.
const TAG_VERSION: &str = "v2";

/// The version prefix of a v1 tag input, which binds no issue.
const TAG_V1: &str = "v1";

type HmacSha256 = Hmac<Sha256>;

/// The key for route tags (D11). From an env var at startup. Debug hides it.
#[derive(Clone)]
pub struct RouteKey(Secret);

impl RouteKey {
    /// Makes a key.
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when the secret has fewer than
    /// [`MIN_ROUTE_KEY_BYTES`] bytes. The error does not hold the secret.
    pub fn new(secret: Secret) -> Result<Self, JiraError> {
        if secret.expose().len() < MIN_ROUTE_KEY_BYTES {
            return Err(JiraError::InvalidInput {
                what: "route key",
                reason: format!("must have {MIN_ROUTE_KEY_BYTES} bytes or more"),
            });
        }
        Ok(Self(secret))
    }

    /// Returns the MAC of `route` and `binding`, before the output step.
    fn mac(&self, route: &SignalRoute, binding: &RouteBinding) -> Option<HmacSha256> {
        let mut mac = self.run_mac(TAG_VERSION, route)?;
        mac.update(b"\n");
        mac.update(binding.kind().as_bytes());
        mac.update(b"\n");
        mac.update(binding.value().as_bytes());
        Some(mac)
    }

    /// Returns the MAC of `version`, the name and the id of `route`.
    fn run_mac(&self, version: &str, route: &SignalRoute) -> Option<HmacSha256> {
        // HMAC accepts a key of any length, so this never gives `None`.
        let mut mac = HmacSha256::new_from_slice(self.0.expose().as_bytes()).ok()?;
        mac.update(version.as_bytes());
        mac.update(b"\n");
        mac.update(route.workflow_name.as_bytes());
        mac.update(b"\n");
        mac.update(route.workflow_id.as_bytes());
        Some(mac)
    }
}

impl fmt::Debug for RouteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RouteKey(<hidden>)")
    }
}

/// The issue that a v2 tag binds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RouteBinding {
    /// The marker label of the create of the issue, for example
    /// `autumn-idem-01J...`. The tag input has the kind `label`.
    MarkerLabel(String),
    /// The issue id. The tag input has the kind `issue`.
    Issue(IssueId),
}

impl RouteBinding {
    /// The kind in the tag input.
    const fn kind(&self) -> &'static str {
        match self {
            Self::MarkerLabel(_) => "label",
            Self::Issue(_) => "issue",
        }
    }

    /// The value in the tag input.
    fn value(&self) -> &str {
        match self {
            Self::MarkerLabel(label) => label,
            Self::Issue(id) => id.as_str(),
        }
    }
}

/// One workflow run that waits on an issue: the workflow name and the
/// business workflow id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "RawRoute")]
pub struct SignalRoute {
    /// The workflow name, for example `incident_flow`.
    pub workflow_name: String,
    /// The business workflow id, for example `incident:A-1001`.
    pub workflow_id: String,
}

/// The unchecked serde form of a [`SignalRoute`].
#[derive(Deserialize)]
struct RawRoute {
    workflow_name: String,
    workflow_id: String,
}

impl TryFrom<RawRoute> for SignalRoute {
    type Error = JiraError;

    fn try_from(raw: RawRoute) -> Result<Self, Self::Error> {
        Self::new(&raw.workflow_name, &raw.workflow_id)
    }
}

/// Returns `true` for 1 to [`MAX_ROUTE_PART`] chars of `[A-Za-z0-9._:-]`.
fn is_route_part(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_ROUTE_PART
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

/// Decodes `TAG_LEN` lower hex chars to bytes. Other text gives `None`.
fn decode_tag(tag: &str) -> Option<[u8; TAG_LEN / 2]> {
    let bytes = tag.as_bytes();
    if bytes.len() != TAG_LEN || !bytes.iter().all(|b| is_lower_hex(*b)) {
        return None;
    }
    let mut out = [0_u8; TAG_LEN / 2];
    let (pairs, _) = bytes.as_chunks::<2>();
    for (slot, [high, low]) in out.iter_mut().zip(pairs) {
        *slot = (hex_value(*high) << 4) | hex_value(*low);
    }
    Some(out)
}

/// The value of one lower hex char. The caller checks the char first.
const fn hex_value(b: u8) -> u8 {
    if b.is_ascii_digit() {
        b - b'0'
    } else {
        b - b'a' + 10
    }
}

/// Encodes bytes as lower hex.
fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

impl SignalRoute {
    /// Each part: 1..=200 chars of [A-Za-z0-9._:-] (no space, R5).
    ///
    /// # Errors
    ///
    /// `JiraError::InvalidInput` when a part breaks the rule. The error does
    /// not hold the value.
    pub fn new(workflow_name: &str, workflow_id: &str) -> Result<Self, JiraError> {
        let rule = format!("must be 1 to {MAX_ROUTE_PART} chars of [A-Za-z0-9._:-]");
        if !is_route_part(workflow_name) {
            return Err(JiraError::InvalidInput {
                what: "route workflow name",
                reason: rule,
            });
        }
        if !is_route_part(workflow_id) {
            return Err(JiraError::InvalidInput {
                what: "route workflow id",
                reason: rule,
            });
        }
        Ok(Self {
            workflow_name: workflow_name.to_owned(),
            workflow_id: workflow_id.to_owned(),
        })
    }

    /// The v2 tag: HMAC-SHA256(key, "v2\n" + name + "\n" + id + "\n" +
    /// kind + "\n" + value), first 32 lower hex chars.
    #[must_use]
    pub fn tag(&self, key: &RouteKey, binding: &RouteBinding) -> String {
        key.mac(self, binding).map_or_else(String::new, |mac| {
            let full = mac.finalize().into_bytes();
            encode_hex(&full[..TAG_LEN / 2])
        })
    }

    /// Returns `true` when `tag` is the v2 tag of this route and `binding`.
    /// The check runs in constant time (`Mac::verify_truncated_left`). A tag
    /// that is not 32 lower hex chars fails before the MAC.
    #[must_use]
    pub fn verify(&self, key: &RouteKey, binding: &RouteBinding, tag: &str) -> bool {
        let (Some(expected), Some(mac)) = (decode_tag(tag), key.mac(self, binding)) else {
            return false;
        };
        mac.verify_truncated_left(&expected).is_ok()
    }

    /// Returns `true` when `tag` is the v1 tag of this route. A v1 tag binds
    /// no issue. Only the opt-in of the bridge for the upgrade reads it. The
    /// check runs in constant time.
    pub(crate) fn verify_v1(&self, key: &RouteKey, tag: &str) -> bool {
        let (Some(expected), Some(mac)) = (decode_tag(tag), key.run_mac(TAG_V1, self)) else {
            return false;
        };
        mac.verify_truncated_left(&expected).is_ok()
    }

    /// Returns the property entry of this route for the issue of `binding`:
    /// `{"wf", "id", "tag"}`.
    #[must_use]
    pub fn property_entry(&self, key: &RouteKey, binding: &RouteBinding) -> Value {
        json!({"wf": self.workflow_name, "id": self.workflow_id, "tag": self.tag(key, binding)})
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn key() -> RouteKey {
        RouteKey::new(Secret::new("k".repeat(32))).unwrap()
    }

    fn label() -> RouteBinding {
        RouteBinding::MarkerLabel("autumn-idem-x".into())
    }

    fn issue() -> RouteBinding {
        RouteBinding::Issue(IssueId::parse("10001").unwrap())
    }

    #[test]
    fn route_key_needs_32_bytes_and_hides_itself() {
        let err = RouteKey::new(Secret::new("short-secret-value")).unwrap_err();
        assert_eq!(err.code(), "invalid_input");
        assert!(!err.to_string().contains("short-secret-value"));
        assert!(RouteKey::new(Secret::new("x".repeat(31))).is_err());
        let key = RouteKey::new(Secret::new("super-secret-route-key-0123456789")).unwrap();
        let debug = format!("{key:?}");
        assert_eq!(debug, "RouteKey(<hidden>)");
        assert!(!debug.contains("super-secret"));
    }

    #[test]
    fn route_parts_follow_the_rule() {
        assert!(SignalRoute::new("incident_flow", "incident:A-1001.v2").is_ok());
        let long = "a".repeat(MAX_ROUTE_PART);
        assert!(SignalRoute::new(&long, &long).is_ok());
        let too_long = "a".repeat(MAX_ROUTE_PART + 1);
        for (name, id) in [
            ("", "x"),
            ("x", ""),
            ("a b", "x"),
            ("x", "a b"),
            ("x/y", "x"),
            ("x", "é"),
            (too_long.as_str(), "x"),
            ("x", too_long.as_str()),
        ] {
            let err = SignalRoute::new(name, id).unwrap_err();
            assert_eq!(err.code(), "invalid_input", "{name:?} {id:?}");
        }
    }

    #[test]
    fn serde_checks_the_rule() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let value = serde_json::to_value(&route).unwrap();
        assert_eq!(
            value,
            json!({"workflow_name": "incident_flow", "workflow_id": "A-1"})
        );
        assert_eq!(serde_json::from_value::<SignalRoute>(value).unwrap(), route);
        let bad = json!({"workflow_name": "a b", "workflow_id": "A-1"});
        assert!(serde_json::from_value::<SignalRoute>(bad).is_err());
    }

    #[test]
    fn tag_is_hmac_of_the_versioned_input() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let tag = route.tag(&key(), &label());
        // The first 32 hex chars of HMAC-SHA256 with the key "k" x 32, from:
        // printf 'v2\nincident_flow\nA-1\nlabel\nautumn-idem-x' |
        //   openssl dgst -sha256 -hmac kkk...k
        assert_eq!(tag, "ec8ee53b27c6aca90205d8d9b2054d23");
        // printf 'v2\nincident_flow\nA-1\nissue\n10001' | openssl ...
        assert_eq!(
            route.tag(&key(), &issue()),
            "132366b5d450b6a5dd68acca2b0585a2"
        );
        assert_eq!(tag.len(), TAG_LEN);
        assert!(tag.bytes().all(is_lower_hex));
        assert!(route.verify(&key(), &label(), &tag));
        // Another key, name, id or binding gives another tag.
        let other_key = RouteKey::new(Secret::new("j".repeat(32))).unwrap();
        assert_ne!(route.tag(&other_key, &label()), tag);
        assert_ne!(
            SignalRoute::new("incident_flow", "A-2")
                .unwrap()
                .tag(&key(), &label()),
            tag
        );
        let other_label = RouteBinding::MarkerLabel("autumn-idem-y".into());
        assert_ne!(route.tag(&key(), &other_label), tag);
        assert!(!route.verify(&key(), &other_label, &tag));
        assert!(!route.verify(&key(), &issue(), &tag));
        // The kind keeps a label apart from an issue id with the same text.
        let as_label = RouteBinding::MarkerLabel("10001".into());
        assert_ne!(route.tag(&key(), &as_label), route.tag(&key(), &issue()));
        // The separator keeps "a" + "b\nc" apart from "a\nb" + "c".
        let one = SignalRoute::new("ab", "c").unwrap();
        let two = SignalRoute::new("a", "bc").unwrap();
        assert_ne!(one.tag(&key(), &label()), two.tag(&key(), &label()));
    }

    /// A v1 tag binds no issue. It verifies only with `verify_v1`, and a v2
    /// check never takes it.
    #[test]
    fn v1_tags_verify_only_as_v1() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        // printf 'v1\nincident_flow\nA-1' | openssl dgst -sha256 -hmac kkk...k
        let v1 = "df60e928addb470f158ced8d9f527e82";
        assert!(route.verify_v1(&key(), v1));
        assert!(!route.verify(&key(), &label(), v1));
        assert!(!route.verify(&key(), &issue(), v1));
        let v2 = route.tag(&key(), &issue());
        assert!(!route.verify_v1(&key(), &v2));
        assert!(!route.verify_v1(&key(), "zz"));
    }

    #[test]
    fn verify_rejects_bad_tags() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let tag = route.tag(&key(), &issue());
        let mut last_changed = tag.clone();
        let last = last_changed.pop().unwrap();
        last_changed.push(if last == '0' { '1' } else { '0' });
        for bad in [
            String::new(),
            last_changed,
            tag.to_uppercase(),
            format!("{tag}0"),
            tag[..TAG_LEN - 1].to_owned(),
            "zz".repeat(16),
        ] {
            assert!(!route.verify(&key(), &issue(), &bad), "{bad}");
        }
        let other = SignalRoute::new("incident_flow", "A-2").unwrap();
        assert!(!other.verify(&key(), &issue(), &tag));
    }

    #[test]
    fn property_entry_has_the_tag() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        assert_eq!(
            route.property_entry(&key(), &issue()),
            json!({"wf": "incident_flow", "id": "A-1", "tag": route.tag(&key(), &issue())})
        );
        assert_eq!(ROUTES_PROPERTY, "autumn-jira-routes");
        assert_eq!(TRACKED_LABEL, "autumn-tracked");
    }

    #[test]
    fn hex_helpers_round_trip() {
        let bytes: Vec<u8> = (0..16).map(|i| i * 17).collect();
        let text = encode_hex(&bytes);
        assert_eq!(decode_tag(&text).unwrap().to_vec(), bytes);
    }
}
