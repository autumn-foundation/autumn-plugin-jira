# ADR 0003: Retries and safe writes

- Status: Accepted. Amended for design v3 (as built): the activity
  budgets, the lookup-failure rule and the delete rule across attempts.
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
  the target it returns `AlreadyThere`. The status alone cannot show that a
  "maybe sent" attempt did not run. So after a 5xx or a "maybe sent"
  attempt, the client sends again only when two rules hold. The issue is
  still at the status of the first attempt. The transition is not global
  and not looped, so Jira rejects it after a commit. Else the call gives
  `OutcomeUnknown`. A 400 or a 409 after a "maybe sent" attempt makes the
  client read the status once more. At the target, it returns `Moved`.
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
- Lookup failure across attempts (design v3): after a "maybe sent" or
  unknown previous attempt, the activity does a lookup only. When that
  lookup fails with a retryable error, the failure stays
  `jira.outcome_pending` with `details.sent = true` and the settle delay.
  So the next attempt also looks up only. Without this rule, a lookup that
  gets a 429 gives `jira.rate_limited` with `sent = false`, and the attempt
  after it sends the write again. A non-retryable lookup error keeps its
  own failure.
- One "sent" rule: `JiraError::may_have_run` gives `details.sent`. It is
  `false` only when the request cannot have run in Jira.
  `error::NOT_SENT_NETWORK_CODES` lists the `Network` codes that did not
  leave the process, and the san-mateo map test pins them.
- Delete across attempts (design v3): a 404 on `jira_delete_issue` after a
  "maybe sent" or unknown previous attempt gives `gone_after_retry`, not a
  failure. This is the client delete rule, applied across harvest
  attempts.
- Activity budgets (design v3): each `start_to_close` is larger than the
  durable worst case of its client calls at 60 s for each request. A
  durable read or idempotent write takes up to 130 s. A marked write takes
  up to 390 s: a pre-lookup, the write and one lookup. A checked write takes
  up to 650 s. Each of its two attempts reads the status and the
  transitions, and then sends.
  So `jira_get_issue` and `jira_search_page` get 3m, `jira_edit_issue` and
  `jira_delete_issue` get 5m, and `jira_create_issue`, `jira_add_comment`,
  `jira_transition_to_status` and `jira_watch_issue` (four idempotent
  calls, 520 s) get 12m. Design v2 gave 5m to each write, which is too
  short for a marked or a checked write. A `start_to_close` timeout gives
  `Unknown` on the next attempt, so a short budget makes more lookups
  only, not a duplicate. The test `start_to_close_covers_the_client_budget`
  adds up the calls with the real constants.
- The client never forwards an `Idempotency-Key` header to Jira.

## Consequences

- Good: by default no duplicate issue, comment or transition. The rule is a
  named, proved theorem.
- Good: in the normal case (index lag of seconds) a lost reply resolves to
  `found` on the next harvest attempt.
- Bad: by default a lost create whose issue stays hidden longer than the
  settle delay fails closed with `outcome_unknown`. A person must check.
- Bad: with the opt-in, index lag longer than the settle delay can give a
  duplicate. The docs say "this can make a duplicate".
- Bad: a site without the Labels field on the create screen cannot use
  idempotent create. The error says how to fix it. No silent fallback.
- Bad: the Jira `Retry-After` hint is lost when the error goes through
  camel, because harvest-camel maps camel errors without it.
- Bad: a 12m `start_to_close` lets a stuck write hold a worker slot longer
  than design v2 did.
- Considered and deferred: a key-window probe with `bulkfetch` above a
  high-water mark gives a strong negative without the search index. It
  depends on three facts that no source confirms: keys only go up, keys are
  never reused, and `bulkfetch` reads current data. A live test must prove
  them first.
