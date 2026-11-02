# ADR 0003: Retries and safe writes

- Status: Accepted
- Date: 2026-11-02

## Context

Jira has no idempotency key. `POST /issue`, `POST /comment` and
`POST /transitions` do not dedupe. A retry after a 5xx reply or a network
error can make a second issue. The user standard says: never retry a
non-idempotent Jira write after a 5xx or a network error, unless a
mechanism makes it safe.

Jira search is eventually consistent. A search that finds nothing does not
prove that a lost create made no issue. A request that timed out on our
side can also commit later.

harvest retries activities. harvest-camel retries routes. Nested retry
layers multiply the sends and break the activity timeout.

## Decision

- `policy::decide` classes: `Read`, `IdempotentWrite`, `CheckedWrite`,
  `MarkedWrite`, `UnsafeWrite`. Theorem `no_resend_after_maybe_sent`: after a
  5xx or a "maybe sent" error, an unsafe or marked write never gets
  `Retry`. An unsafe write gives `Unknown`. A marked write gives `Confirm`.
- An unsafe write is sent at most once in a client call.
- Marked create: label `autumn-idem-<26 lower base32>` and issue property
  `autumn-jira` with the full key, in the same POST. Before the POST, a
  positive lookup. After a lost reply, `Confirm` runs positive lookups only.
  A hit counts only when the property holds the full key. A failed lookup
  never counts as "not found". No hit gives `OutcomeUnknown` with the
  marker.
- Marked comment: a last paragraph `autumn-ref: <26 lower base32>`. The
  lookup reads the newest comments, which are not from the search index.
- Checked transition: before each attempt, the client reads the status. At
  the target it returns `AlreadyThere`.
- 429: reads, idempotent, checked and marked writes retry, with
  `Retry-After` as the floor. A plain unsafe write does not retry a 429.
  Assumption A1: a 429 means Jira did not run the write. Atlassian says to
  retry only idempotent requests; the marker makes the request idempotent by
  mechanism.
- Durable hosts use `ClientSettings::durable()`, so harvest is the one retry
  layer. `OutcomeUnknown` with a marker becomes the retryable failure
  `jira.outcome_pending` with a settle delay (30 s). On the next attempt,
  `policy::host_may_send` decides: after a "maybe sent" or unknown previous
  attempt, the activity does a lookup only. No hit gives the non-retryable
  `jira.outcome_unknown`. The camel component uses the same rule with the
  `HarvestActivityAttempt` header.
- An app can opt in with a clear name: `resend_after_negative_lookup`
  (harvest) or `resendAfterNegativeLookup=true` (camel). Then a negative
  lookup after the settle delay leads to one more create.
- The client never forwards an `Idempotency-Key` header to Jira.

## Consequences

- Good: by default no duplicate issue, comment or transition. The rule is a
  named, proved theorem.
- Good: in the normal case (index lag of seconds) a lost reply resolves to
  `found` on the next harvest attempt.
- Bad: by default a lost create whose issue stays hidden longer than the
  settle delay fails closed with `outcome_unknown`. A person must check.
- Bad: with the opt-in, index lag longer than the settle delay can give a
  duplicate. The docs say "best effort; no exactly once".
- Bad: a site without the Labels field on the create screen cannot use
  idempotent create. The error says how to fix it. No silent fallback.
- Bad: the Jira `Retry-After` hint is lost when the error goes through
  camel, because harvest-camel maps camel errors without it.
- Considered and deferred: a key-window probe with `bulkfetch` above a
  high-water mark gives a strong negative without the search index. It
  depends on three facts that no source confirms: keys only go up, keys are
  never reused, and `bulkfetch` reads current data. A live test must prove
  them first.
