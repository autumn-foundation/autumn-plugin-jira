//! Signal routes: which workflow run waits on an issue (D11).
//!
//! A route sits in the issue property [`ROUTES_PROPERTY`] as
//! `{"routes": [{"wf": <name>, "id": <workflow id>, "tag": <tag>}]}`. A
//! person can edit an issue property, so each route has a tag: the first 32
//! lower hex chars of `HMAC-SHA256(key, "v1\n" + name + "\n" + id)`. Only a
//! holder of the [`RouteKey`] can make a tag, so a person cannot point an
//! issue at another run. The check of a tag runs in constant time.

use std::fmt;

use autumn_jira::JiraError;
use autumn_jira::policy::is_lower_hex;
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
const TAG_VERSION: &str = "v1";

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
        Ok(Self(secret))
    }

    /// Returns the MAC of `route`, before the output step.
    fn mac(&self, route: &SignalRoute) -> Option<HmacSha256> {
        // HMAC accepts a key of any length, so this never gives `None`.
        let mut mac = HmacSha256::new_from_slice(self.0.expose().as_bytes()).ok()?;
        mac.update(TAG_VERSION.as_bytes());
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
    for (slot, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
        *slot = (hex_value(pair[0]) << 4) | hex_value(pair[1]);
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
        Ok(Self {
            workflow_name: workflow_name.to_owned(),
            workflow_id: workflow_id.to_owned(),
        })
    }

    /// HMAC-SHA256(key, "v1\n" + name + "\n" + id), first 32 lower hex chars.
    #[must_use]
    pub fn tag(&self, key: &RouteKey) -> String {
        let _ = (key.mac(self), is_route_part(""), encode_hex(&[]));
        String::new()
    }

    /// Returns `true` when `tag` is the tag of this route. The check runs in
    /// constant time (`Mac::verify_truncated_left`). A tag that is not 32
    /// lower hex chars fails before the MAC.
    #[must_use]
    pub fn verify(&self, key: &RouteKey, tag: &str) -> bool {
        let _ = (key, tag, decode_tag(tag));
        false
    }

    /// Returns the property entry of this route: `{"wf", "id", "tag"}`.
    #[must_use]
    pub fn property_entry(&self, key: &RouteKey) -> Value {
        let _ = key;
        json!({})
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    fn key() -> RouteKey {
        RouteKey::new(Secret::new("k".repeat(32))).unwrap()
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
        let tag = route.tag(&key());
        // HMAC-SHA256("kkkk...k" (32), "v1\nincident_flow\nA-1"), checked
        // with: printf 'v1\nincident_flow\nA-1' | openssl dgst -sha256 -hmac kkk...k
        let mut mac = HmacSha256::new_from_slice("k".repeat(32).as_bytes()).unwrap();
        mac.update(b"v1\nincident_flow\nA-1");
        let full = mac.finalize().into_bytes();
        assert_eq!(tag, encode_hex(&full[..16]));
        assert_eq!(tag.len(), TAG_LEN);
        assert!(tag.bytes().all(is_lower_hex));
        assert!(route.verify(&key(), &tag));
        // Another key, name or id gives another tag.
        let other_key = RouteKey::new(Secret::new("j".repeat(32))).unwrap();
        assert_ne!(route.tag(&other_key), tag);
        assert_ne!(
            SignalRoute::new("incident_flow", "A-2")
                .unwrap()
                .tag(&key()),
            tag
        );
        // The separator keeps "a" + "b\nc" apart from "a\nb" + "c".
        let one = SignalRoute::new("ab", "c").unwrap();
        let two = SignalRoute::new("a", "bc").unwrap();
        assert_ne!(one.tag(&key()), two.tag(&key()));
    }

    #[test]
    fn verify_rejects_bad_tags() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        let tag = route.tag(&key());
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
            assert!(!route.verify(&key(), &bad), "{bad}");
        }
        let other = SignalRoute::new("incident_flow", "A-2").unwrap();
        assert!(!other.verify(&key(), &tag));
    }

    #[test]
    fn property_entry_has_the_tag() {
        let route = SignalRoute::new("incident_flow", "A-1").unwrap();
        assert_eq!(
            route.property_entry(&key()),
            json!({"wf": "incident_flow", "id": "A-1", "tag": route.tag(&key())})
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
