# ADR 0005: The verified policy core

- Status: Accepted. Amended after review round 1: `scale_permille` is
  body-matched, the three lists are exact, and a test bans proof escapes
  (plan V67). Amended after review round 2: the escape scan ignores white
  space, and the body extractor removes comments first (plan V103).
- Date: 2026-11-02

## Context

The user standard asks for three items, as san-mateo has them. The items
are a Verus spec of the critical pure invariants, a small verified core in
`src`, and a body-match test. Verus is not installable on the development machine. It can
run in CI. A loop with proof code does not body-match.

## Decision

- `crates/autumn-jira/src/policy.rs` holds pure functions only. They are
  in three groups.
- Proved and body-matched (`BODY_MATCHED`): `classify_status`, `decide`,
  `retry_wait_ms`, `scale_permille`, `apply_jitter`, `host_may_send`,
  `token_page_step`, `offset_next`, `freshness`, `webhook_status` and
  `is_lower_hex`. `verus/policy.rs` holds the same bodies with `ensures`
  clauses and proofs. The body compare drops each `proof { ... }` block of
  the Verus body, so `scale_permille` matches too (plan V67).
- Proved, with no body match (`PROOF_PROPTESTED`): `backoff_ms`. Its Verus
  loop has proof steps in its branches, so the bodies cannot match. It is
  copied from san-mateo with its proof. The proptest
  `spec_as_test_backoff` checks it against a closed form. A differential
  proptest checks that it equals `san_mateo::policy::backoff_ms`.
- Proptest-only, not in the Verus file (`PROPTEST_ONLY`):
  `signature_shape_ok` and `jql_quote`.
- Headline theorems: `no_resend_after_maybe_sent` (in `decide`),
  `host_may_send`, `token_pager_terminates`, and the 200-or-503 rule.
- `verus_bodies_match` compares bodies and constants. Each function before
  `#[cfg(test)]`, of any visibility, is in exactly one of the three lists.
  `every_const_fn_is_proved_or_listed` and `each_fn_is_in_exactly_one_list`
  check this. A proved function needs a `pub fn` line in the spec, and a
  comment does not count. `the_drift_check_sees_changes` proves that the
  check works.
- `the_spec_has_no_proof_escape` fails when `verus/policy.rs` holds
  `assume(`, `admit(`, `assume_new(`, `external_body`,
  `verifier::external`, `#[verifier(`, `assume_specification` or
  `exec_allows_no_decreases_clause` outside comments. Verus still says
  "verified" with one of them, so the rule needs a test. Verus tokens can
  have white space between them. So the scan removes the comments and all
  white space first, and `assume (false)` counts. An escape word directly
  after a name character, as in `my_assume(`, is a name, not an escape.
- The body extractor of `verus_bodies_match` removes the comments of the
  whole file before it finds a function. So a `proof {` or a brace in a
  comment cannot hide code from the compare (plan V103). The guard `adr_0005_names_each_policy_fn`
  fails when this list does not name a public function of `policy.rs`.
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
