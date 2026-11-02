# ADR 0003: Retries and safe writes

- Status: Accepted. Amended for design v3 (as built): the activity
  budgets, the lookup-failure rule and the delete rule across attempts.
  Amended after review round 1: the transition resend rule, the camel
  redelivery rules, the camel settle wait and the san-mateo budgets
  (plan V54 to V95).
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
  It reads pages newest first, up to `max_pages`. At the cap it gives
  `PageLimit`, so it fails closed. A match is a comment by the own account
  with a line that is the full marker. A person can type the marker, but
  a person is not the author of the client comments (plan V62).
- Checked transition: before each attempt, the client reads the status. At
  the target it returns `AlreadyThere`. The status alone cannot show that a
  "maybe sent" attempt did not run. So after a 5xx or a "maybe sent"
  attempt, the client sends again only when two rules hold. The issue is
  still at the status of the first attempt. The transition is not global
  and not looped, so Jira rejects it after a commit. Else the call gives
  `OutcomeUnknown`. A 400 or a 409 after a "maybe sent" attempt makes the
  client read the status once more. At the target, it returns `Moved`.
- Why this transition rule (review round 1, plan V63): the status check
  alone sent a transition again in two bad cases. An automation can move
  the issue on after the first attempt commits. A global or looped
  transition is valid from each status, so a second send runs it again.
  The rule reads the `isGlobal` and `looped` flags of the transition list.
  It needs no changelog read, and it never sends two times in these
  cases. The cost: a lost reply on a global transition ends with
  `OutcomeUnknown`, and a person or a later step must check.
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
- Delete across attempts (design v3, plan V70): a 404 on
  `jira_delete_issue` on any attempt after the first gives
  `gone_after_retry`, not a failure. The `sent` flag tells only the last
  send of an attempt. An earlier send of that attempt can have deleted the
  issue.
- A failed pre-lookup sent nothing (plan V69). So its failure has
  `details.sent = false`, and the next attempt can send.
- Activity budgets (design v3, amended in plan V64): each `start_to_close`
  is larger than the durable worst case of its client calls. The model
  takes 60 s for each transport call (`MAX_TRANSPORT_TIMEOUT`). A
  san-mateo send can make four transport calls
  (`SAN_MATEO_CALLS_PER_SEND`): a refresh, the send, a refresh after a
  401, and the send again. Through san-mateo, a durable read or idempotent
  write takes up to 490 s. A marked write takes up to 1 470 s, and a
  checked write up to 2 450 s. A direct client needs less: 130 s, 390 s
  and 650 s.
  So `jira_get_issue`, `jira_search_page`, `jira_edit_issue` and
  `jira_delete_issue` get 9m. `jira_create_issue`, `jira_add_comment`,
  `jira_transition_to_status` and `jira_watch_issue` (four idempotent
  calls, 1 960 s) get 42m. The first budgets (3m, 5m, 12m) held for a
  direct client only. A `start_to_close` timeout gives `Unknown` on the
  next attempt, so a short budget makes more lookups only, not a
  duplicate. The test `start_to_close_covers_the_client_budget` adds up
  the calls with the real constants, for direct and san-mateo sends. The
  plugin caps `[jira.http] timeout_secs` at 60, so the model holds.
- Unknown sends (plan V54, V59): a transport `Invalid` error is "not
  sent" only for the texts of a check before the send. A send error after
  Jira has the request can have run, so it is "maybe sent". On a marked
  write, a 2xx with an unexpected status also goes to the lookups.
- A retry after `outcome_pending` is a lookup only, never the write again
  (plan V60). Any retry layer must follow this rule, not only harvest.
- Camel redelivery (plan V92): a camel redelivery of a marked write does a
  lookup only, also on harvest attempt 1. A camel redelivery of a write
  with no key header fails and sends nothing. So a camel `retry(..)` never
  sends a create or a comment two times. A redelivery header of an earlier
  step also makes a later write look up only. That fails closed.
- Camel settle wait (plan V90): harvest-camel gives a camel attempt no
  `Retry-After`. So a lookup-only `createIssue` that finds nothing waits
  `settleMs` (default 10 s) and looks up one more time. The default fits
  the 30 s timeout of a `direct:` call. A comment lookup does not wait.
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
  camel, because harvest-camel maps camel errors without it. The
  `settleMs` wait of the producer makes up for it on `createIssue`.
- Bad: a 42m `start_to_close` lets a stuck write hold a worker slot longer
  than design v2 did. A deadline inside `SanMateoBackend` from the
  activity deadline is a better fix for later.
- Bad: a camel redelivery that follows a failed earlier step makes each
  later write in that exchange look up only. Such a write can fail with
  `outcome_unknown`, but it never makes a duplicate.
- Considered and deferred: a key-window probe with `bulkfetch` above a
  high-water mark gives a strong negative without the search index. It
  depends on three facts that no source confirms: keys only go up, keys are
  never reused, and `bulkfetch` reads current data. A live test must prove
  them first.
