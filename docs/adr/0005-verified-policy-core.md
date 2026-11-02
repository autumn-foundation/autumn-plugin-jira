# ADR 0005: The verified policy core

- Status: Accepted
- Date: 2026-11-02

## Context

The user standard asks for a Verus spec of the critical pure invariants, a
small verified core mirrored in `src`, and a body-match test, as san-mateo
does. Verus is not installable on the development machine. It can run in CI.
A loop with proof code does not body-match.

## Decision

- `crates/autumn-jira/src/policy.rs` holds pure functions only:
  `classify_status`, `decide`, `retry_wait_ms`, `backoff_ms`,
  `apply_jitter`, `host_may_send`, `token_page_step`, `offset_next`,
  `freshness`, `webhook_status`, `is_lower_hex`, plus the proptest-only
  `signature_shape_ok` and `jql_quote`.
- `verus/policy.rs` holds the same functions with `ensures` clauses and
  proofs. Headline theorems: `no_resend_after_maybe_sent` (in `decide`),
  `host_may_send`, `token_pager_terminates`, and the 200-or-503 rule.
- `backoff_ms` is copied from san-mateo with its proof. A differential
  proptest checks it equals `san_mateo::policy::backoff_ms`.
- `verus_bodies_match` compares bodies and constants.
  `every_const_fn_is_proved_or_listed` fails when a function is in neither
  the Verus file nor the proptest-only list. `the_drift_check_sees_changes`
  proves that the check works.
- Each `ensures` clause also exists as a proptest, so the local loop checks
  the same rules that Verus proves.
- The CI job copies the san-mateo `verus` job: a pinned revision built from
  source, with a cache.
- Hand mutations on a scratch clone show that a named test fails for each
  critical rule.

## Consequences

- Good: the write-safety rule is one named theorem.
- Good: a new policy function cannot skip the proof by omission.
- Bad: proofs fail only in CI. The team pushes early and in batches.
- Bad: the glue around the core (status mapping inputs, the client loop) is
  tested, not proved. A chaos proptest covers it.
