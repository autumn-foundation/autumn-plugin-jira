# Reverse brainstorm record

- Question: "How can we make this Jira plugin fail, be insecure, flaky,
  unusable, or a maintenance nightmare?"
- Method: list each way to fail. Invert it into a requirement, a guard and a
  test.
- Date: 2026-11-02
- Style: ASD-STE100.
- Severity: **S1** data corruption, duplicate writes, a security hole or a
  secret leak. **S2** an outage, silent loss, flaky CI or a broken feature.
  **S3** usability, speed, maintenance, docs drift. **S4** polish.
- Where each requirement lands: [`../plan.md`](../plan.md) section 16 (AC)
  and the ADRs. Some rows changed after the six hats; the "Final" column
  says how.

## 1. The ten strongest outputs

1. Jira has no idempotency key. A forwarded `Idempotency-Key` header does
   nothing at Jira, so a retry of `POST /issue` can make a duplicate (F01,
   F02).
2. The autumn replay store answers 409 to a duplicate. Jira retries 409. A
   4xx after verification keeps the replay keys, so a retry is rejected:
   silent loss (F07).
3. The autumn body cap default is 1 MiB. Jira sends up to 25 MB. An over-cap
   body gets 400, which Jira does not retry (F08).
4. A bot comment makes a `comment_created` event. A handler that comments
   again loops and hits the per-issue write limit (F09).
5. OAuth dynamic webhooks use a JWT, not HMAC, and expire after 30 days
   (F10, F27).
6. A Jira call at startup, or a `Readiness` indicator, makes a Jira outage an
   app outage (F20, F21).
7. Two copies of a shared crate break trait impls or the contract check
   (F29).
8. A repeated `nextPageToken` loops for ever (F15).
9. Jira timestamps use `+0000`, which RFC 3339 parsers reject (F19).
10. Examples that skip when `DATABASE_URL` is unset pass CI with no run
    (F35).

## 2. Failure modes and their inversions

| Id | Sev | Way to fail | Requirement and guard | Final |
|---|---|---|---|---|
| F01 | S1 | Retry `POST /issue` after a 5xx or a network error. | An unsafe write is sent once; a 5xx or a network error gives `OutcomeUnknown`. Verified `decide`. | AC15, ADR 0003 |
| F02 | S1 | A harvest or camel retry with a forwarded `Idempotency-Key`. | Never forward it to Jira. Text guard. Durable hosts follow `host_may_send`. | AC32, AC37 |
| F03 | S1 | No safe retry, so the app writes its own loop. | Marker label plus property, positive lookups, an opt-in resend. Best effort, documented. | AC16, ADR 0003 |
| F04 | S1 | Accept a POST with no or a bad signature. | `SignedWebhook` before parse; Atlassian test vector. | AC19, AC29 |
| F05 | S1 | Compare the signature with `==`. | Constant time only. Text guard. | AC19 |
| F06 | S1 | Replay a captured signed body. | Replay store on the delivery id; a stale and a future check on the JSON `timestamp`; a missing timestamp is ignored. | AC29 |
| F07 | S1 | A 4xx after verification loses the event. | 200 or 503 only (verified `webhook_status`); 503 frees the keys. | AC29, ADR 0004 |
| F08 | S1 | An over-cap body is lost. | The helper sets 26 MiB; a boot warning below 8 MiB; the poll fallback recovers the state. (Over-cap is 400, not 413.) | AC28, AC33 |
| F09 | S1 | A bot feedback loop. | `ignore_actor_account_ids` with a service account; the comment marker check `has_comment_marker`; `self_filter` in health. | AC29 |
| F10 | S1 | JWT webhooks rejected, or a bypass flag. | HMAC only in v1; no bypass. | ADR 0004 |
| F11 | S1 | A secret in `Debug`, logs or error text. | `Secret`; env var names; `skip_all`; a capture test. | AC21, AC25 |
| F12 | S1 | A token in `autumn.toml`. | `deny_unknown_fields`; a `token` key names `token_env`. | AC25 |
| F13 | S1 | SSRF by site, redirect or path. | Fixed hosts, no redirects, validated and encoded path ids. | AC2 |
| F14 | S1 | JQL injection. | `jql_quote` with proptests; `Jql::raw` is explicit; camel `project` and `jql` are exclusive. | AC10, AC36 |
| F15 | S2 | Infinite pagination. | Verified `token_page_step`; `PaginationStalled`, `PageLimit`. | AC7, AC18 |
| F16 | S2 | The removed legacy search (410). | `/search/jql` only. Text guard. | AC7 |
| F17 | S2 | Search returns only `id`. | Explicit `fields` on every search. | AC7 |
| F18 | S2 | A short page read as the last page. | Two pagers: token and offset. | AC7 |
| F19 | S2 | `+0000` timestamps fail to parse. | `JiraTimestamp` accepts `+0000`, RFC 3339 and epoch ms. | AC12 |
| F20 | S2 | A Jira call at startup. | No network at startup. | AC26 |
| F21 | S2 | The indicator in `Readiness`; probes spend the rate limit. | `HealthOnly`, cached. | AC27 |
| F22 | S2 | CSRF blocks the webhook. | The endpoint entry is auto-exempt; the boot check prints the TOML. | AC28 |
| F23 | S2 | A memory replay store in production. | autumn fails it unless opted in; docs show Redis. | ADR 0004 |
| F24 | S2 | Tests hit the real Jira. | `FakeJira` everywhere; one opt-in live test. | ADR 0007 |
| F25 | S2 | A non-deterministic workflow. | `system_now`, `BTreeMap`, activities for I/O; `replay_check`. | AC33 |
| F26 | S2 | One activity reads 50k issues. | One page per activity; small `IssueSummary`. | AC32 |
| F27 | S2 | Dynamic webhooks expire. | Out of scope; ADR note. | ADR 0004 |
| F28 | S2 | The plugin refreshes OAuth tokens itself. | All OAuth in san-mateo. The guard covers OAuth only; Basic in API token mode is an app choice. | ADR 0002 |
| F29 | S2 | Two copies of a shared crate. | One-version CI check; contract self-test. | AC24, AC39 |
| F30 | S2 | Missing OAuth scopes. | `Unauthorized` names the scopes. | AC13 |
| F31 | S2 | Wrong cloudId (Confluence or another site). | `resolve_cloud_id` rules. | AC4 |
| F32 | S2 | A 401 loop trips the CAPTCHA. | No 401 retry; `CaptchaRequired`. | AC3 |
| F33 | S2 | Success checked as `== 200`. | `Op::expected`; a 204 is not parsed. | AC6 |
| F34 | S2 | 429 ignored or `Beta-Retry-After` used. | `Retry-After` floor; `Beta-*` ignored. | AC14 |
| F35 | S2 | Examples skip in CI. | Skip only when `CI` is unset. | EX3, EX5 |
| F36 | S2 | Tests steal each other's tasks. | A queue and ids per test. | section 13 |
| F37 | S2 | Wall-clock sleeps in tests. | `start_paused`; tokio-time index lag. | AC14 |
| F38 | S3 | A plain string description gives 400. | `Adf::text`. | AC11 |
| F39 | S3 | Strict models break on new Jira fields. | Lenient models; `Other` events. | AC19 |
| F40 | S3 | `changelog.id` as a number or a string. | Accept both. | AC19 |
| F41 | S3 | 404 read as "does not exist". | `NotFoundOrForbidden`. | AC13 |
| F42 | S3 | A strict key regex on replies. | Strict input, lenient output. | AC1 |
| F43 | S3 | Transition names are not status names. | Match by target status; `TransitionNotAvailable { available }`. | AC8 |
| F44 | S3 | `notifyUsers` spam. | Sent only when set. | AC6 |
| F45 | S3 | Out-of-order events. | The wait skips older timestamps. | AC33 |
| F46 | S3 | A project delete sends no event. | Docs; the poll is the reconcile path. | README |
| F47 | S3 | Docs drift. | README doctests; guards. | AC40 |
| F48 | S3 | Verus spec and code drift. | `verus_bodies_match`, drift completeness, CI job. | AC18 |
| F49 | S3 | C builds in the main workspace. | The separate ecosystem workspace. | ADR 0008 |
| F50 | S3 | Coverage under 85%. | A per-crate gate. | AC38 |
| F51 | S3 | Crate-wide clippy `allow`. | Each `allow` has a reason. Guard. | AC38 |
| F52 | S3 | MSRV break. | CI job on 1.89. | AC38 |
| F53 | S3 | A query string (PII) in an error. | No query in `Display`. | AC13 |
| F54 | S3 | Route prefix collision; unclear attribution. | `name()` is the crate name; prefix is a builder method. | AC24 |
| F55 | S3 | No typed route: `run()` fails. | Each example has a typed route. | EX2 |
| F56 | S4 | Ids mixed up. | Newtypes. | AC1 |
| F57 | S4 | A new field breaks users. | `#[non_exhaustive]`, builders. | section 4 |
| F58 | S4 | No dependency audit. | Later (`cargo deny`). | section 20 |
| F59 | S4 | Mutation tests edit the repo. | Only on a scratch clone. | CLAUDE.md |
| F60 | S4 | Long, ambiguous prose. | ASD-STE100. | all docs |

## 3. Verified core candidates (final set in plan section 6)

| Candidate | Final |
|---|---|
| `retry_plan(op)` | Became `decide` with five classes and the theorem `no_resend_after_maybe_sent`. |
| `backoff_ms` | Copied from san-mateo with its proof; a differential proptest. |
| `page_step` | `token_page_step` with a termination lemma. |
| `escape_jql_literal` | `jql_quote`, proptest only (a loop). |
| `is_stale` | `freshness`, with a future-skew bound. |
| (new) | `classify_status`, `retry_wait_ms`, `apply_jitter`, `host_may_send`, `offset_next`, `webhook_status`, `is_lower_hex`. |
