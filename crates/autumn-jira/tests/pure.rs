//! Pure tests over the public API: ids, markers and the root re-exports.

use autumn_jira::{IdempotencyKey, IssueId, IssueKey, IssueRef, ProjectKey};

#[test]
fn idempotency_key_derive_is_stable() {
    // The value is pinned: a change breaks the markers of earlier writes.
    let key = IdempotencyKey::derive(&["alert", "42"]);
    assert_eq!(
        key.as_str(),
        "d1-0ec232fcb24bdab3ca15ae85ffa63ab58e7b3ac0d1b1c02c37ab3e3529381dcb"
    );
    assert_eq!(IdempotencyKey::derive(&["alert", "42"]), key);
    // A part boundary changes the key.
    assert_ne!(
        IdempotencyKey::derive(&["ab", "c"]),
        IdempotencyKey::derive(&["a", "bc"])
    );
    assert_ne!(IdempotencyKey::derive(&[]), IdempotencyKey::derive(&[""]));
    // A derived key is a valid key, and its markers are stable.
    assert_eq!(IdempotencyKey::parse(key.as_str()).unwrap(), key);
    assert_eq!(
        key.marker_label("autumn-idem-"),
        IdempotencyKey::derive(&["alert", "42"]).marker_label("autumn-idem-")
    );
}

#[test]
fn root_re_exports_work() {
    let key = IssueKey::parse("OPS-12").unwrap();
    assert_eq!(key.project(), ProjectKey::parse("OPS").unwrap());
    let r: IssueRef = (&key).into();
    assert_eq!(r.path_segment(), "OPS-12");
    let id: IssueRef = IssueId::parse("10001").unwrap().into();
    assert_eq!(id.to_string(), "10001");
    let _ = autumn_jira::san_mateo::Secret::new("x");
}
