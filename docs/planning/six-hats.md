# Six thinking hats record

- Technique: de Bono's six thinking hats. Six reviewers read the draft
  design. Each wore one hat. The lead then resolved every point.
- Date: 2026-11-02
- Style: ASD-STE100.
- Decisions: **Accept** (as proposed), **Adapt** (the idea, in another
  form), **Reject** (with a reason), **Later** (after v1; plan section 20).
- "Plan" refers to sections of [`../plan.md`](../plan.md). "ADR" refers to
  [`../adr/`](../adr/).

## Summary of the changes from the draft

1. Webhook route mounted at the full path with `merge`, because
   `SignedWebhook` looks up the endpoint by the request path and `nest`
   strips the prefix (W1).
2. The durable wait uses `ctx.system_now()` and a signal-or-poll race. The
   draft used `ctx.now()`, which never moves in production (W2, B1, Y2).
3. One retry owner per host. The client never sends an unsafe write twice in
   a call. Durable hosts decide with the verified `host_may_send`. A resend
   after a negative lookup is an explicit opt-in (W3, B2, B3, G2).
4. Backend errors split into `NotSent`, `MaybeSent` and `Auth`, with a table
   over every source variant (W7, B11).
5. A star crate graph with the event seam in the core and an independent
   fake crate (G7, G11).
6. Signal routes in an issue property with an HMAC tag, plus one constant
   label (G10, B10, R5).
7. Ergonomics: strict id `Deserialize`, `impl Into<IssueRef>`, status
   categories first, `From<JiraError> for HandlerError`, a `testing`
   feature, mode from the config table (R1 to R19).
8. The ecosystem example shows OAuth refresh, a lost reply, a worker
   restart and a poll fallback; camel stays at the edges (Y3, G16, R9).
9. Execution plan with waves, spikes, file ownership and checkpoints (L).

---

## White hat: facts and gaps

Key points: six blockers where the draft did not work as written, wrong or
missing API facts, and the facts that the review checked as true.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| W1 | `SignedWebhook` fails in a nested router (500 for every delivery). | Accept | Mount with `merge` at the full path. Spike S1 is the first test. Plan 8.4, ADR 0004. |
| W2 | `ctx.now()` is the start time in production. | Accept | `system_now()` deadline and a race; test with the advancing clock off. Plan 9.3, ADR 0006. |
| W3 | The ecosystem retry story contradicts the client lookup. | Accept | One lookup owner; the exact fault script and lag in plan 11.3; sequence in plan 7.2. |
| W4 | Through camel the error type is `camel.<Variant>`; `Retry-After` is lost. | Adapt | All Jira writes in the ecosystem use library activities, so codes stay `jira.<code>`. The camel path documents the lost hint. ADR 0003. |
| W5 | One `SanMateo` must be shared. | Accept | Plan 11.2: one instance, `server_url` ends with the SMP prefix. |
| W6 | The fake cannot serve the chat API; naming clash. | Accept | `FakeJira::with_fallback`, our own chat provider YAML, names `team-chat`/`team`. The ecosystem chat call uses an `Idempotency-Key` through `SanMateoProxy-Idempotency-Key`. Plan 11.2. |
| W7 | The san-mateo error map is incomplete. | Accept | Full table in plan 4.4, with a `_` arm. |
| W8 | `backoff_ms` cannot body-match. | Accept | Copied loop with proof; differential proptest. Plan 6. |
| W9 | Verus CI builds from source. | Accept | Copy the san-mateo job. Plan 14. |
| W10 | Undefined types in the sketches. | Accept | Every type is defined in plan 4, 8, 9, 10. `SanMateoSettings` dropped. `Op` has `Display`; thiserror uses `.messages.len()`. |
| W11 | The conformance factory must be async. | Accept | `run_client_suite<F, Fut>`; the suite is test code, not public API. Plan 13. |
| W12 | Two camel idempotency header names. | Accept | `HarvestIdempotencyKey` is the only source. Plan 10. |
| W13 | Jira retries autumn's 409. | Accept | Kept and documented with the in-flight reason. ADR 0004. |
| W14 | An over-cap body gives 400, not 413. | Accept | Test "over cap is 400"; boot warning; 26 MiB recommended. AC28, AC29. |
| W15 | Boot check misses fields; production rules. | Accept | Timestamp header, prefix, `None` header default; the prod profile sets a 32+ byte secret and the memory opt-in. Plan 8.4. |
| W16 | Use `received_at()` as now. | Accept | Plan 8.4. |
| W17 | `excludeBody` empties the payload. | Accept | Docs and README say `excludeBody: false`; the poll recovers. |
| W18 | Activities need explicit `start_to_close`; nested retries. | Accept | Per-activity values and a budget test. Plan 7.2, 9.2. |
| W19 | `send_signal_idempotent` errors on terminal runs; durable dedup. | Accept | Error classes in plan 9.4; ADR 0006 notes the durable key. |
| W20 | `full_migrations_sql` breaks the guard. | Accept | `migrate::apply` with `migrate::embedded()` under an advisory lock. |
| W21 | The comment lookup needs one call with `orderBy=-created`. | Accept | Plan 7.1; assumption A3. |
| W22 | "429 means no write" is not sourced. | Adapt | Plain unsafe writes do not retry 429; marked and checked writes do. Assumption A1 in ADR 0003. |
| W23 | `lead` needs `expand=lead`. | Accept | The ecosystem escape hatch sends `expand=lead`; the fake honours it. |
| W24 | Model field types from the spec. | Accept | Plan 4.5 model notes and fake fixtures. |
| W25 | Label rules unverified; base32 case. | Accept | Lower-case base32; one validation fn; assumption A2. |
| W26 | Comment and delete payloads may lack labels. | Accept | The bridge routes status changes only and reads routes with a GET. |
| W27 | The fake needs the OAuth token host. | Accept | Plan 5.1. |
| W28 | Name the san-mateo calls for provider and metadata. | Accept | `get_integration`, `list_connections`, `set_metadata`. Plan 4.4. |
| W29 | No randomness source for jitter. | Accept | `san_mateo::clock::Entropy`; `SeqEntropy` in tests. |
| W30 | `print_stdout` hits examples; MSRV not local. | Accept | Per-example `allow` with a reason; CI-only MSRV recorded (R4). |
| W31 | The OAuth-token guard conflicts with Basic direct mode. | Accept | The guard covers OAuth only. ADR 0002. |
| WG | Facts checked as true (presets, verify order, constant time, replay release, harvest and camel APIs, Jira statuses). | Accept | Used as `[V]` facts in the plan. |

## Black hat: risks

Key points: determinism, nested retries, unsafe negative lookups, security
gaps in the webhook and routing, and a scope that may not finish.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| B1 | `ctx.now()` deadline; replay risk of clock-derived timers. | Accept | `system_now()` is recorded in history; a guard bans `ctx.now(` in the harvest crate. |
| B2 | Retries nested three deep; a timeout drops a POST in flight. | Accept | `ClientSettings::durable()`; budget test; rewritten scenario. |
| B3 | A negative eventually consistent lookup does not make a re-POST safe. | Accept | No resend in the client; durable hosts do lookup only; resend is an opt-in; a failed lookup is never "not found"; lag in tokio time. ADR 0003. |
| B4 | The bridge gives 503 for runs that cannot take a signal. | Accept | Error classes; status-change events only. Plan 9.4. |
| B5 | No reconcile; a lost event stalls the workflow. | Accept | The poll timer and a last read before the timeout branch. Plan 9.3. |
| B6 | Timeouts do not agree. | Accept | `MAX_WAIT` check (29 days) under the 30-day execution timeout. Review round 1: that check did not enforce the execution timeout of a run. The wait deadline is now at most `LAST_READ_MARGIN` before `ctx.deadline()` (plan V83). |
| B7 | A missing timestamp turns off the stale check; the window is too wide. | Accept | Missing gives ignored; default 2 h; a future-skew bound in the verified `freshness`. autumn already fails a memory store in prod. |
| B8 | A project bound around raw JQL can be escaped. | Adapt | No wrapping at all: camel `project` and `jql` are exclusive; raw JQL is the app's text. `jql_balanced` is not needed. |
| B9 | Self-event filter by actor is wrong in both modes. | Accept | Explicit list with a service account; `has_comment_marker`; `self_filter` in health. No boot warning (noise for apps that do not write). |
| B10 | Routing labels show ids and can be forged. | Accept | Property routes with an HMAC tag; constant-time check before any DB call. ADR 0006. |
| B11 | Backend errors lose "not sent" against "maybe sent". | Accept | `NotSent`, `MaybeSent`, `Auth`; `_` gives `MaybeSent`. A 2xx create with a bad body gives `Decode`; see the note below. |
| B12 | The REST `Issue` model loses reduced payloads. | Accept | Lenient `WebhookIssue`; a GET for routes. |
| B13 | Transitions that need fields; missing list operations. | Accept | `TransitionFields`; `projects()`, `issue_types()`; the fake needs a resolution for Done. |
| B14 | Tests check the fake against our own assumptions. | Accept | Spec shape check and an opt-in live run. ADR 0007. |
| B15 | A duplicate gets 409 and Jira retries it. | Reject | A 200 for a duplicate that is still in flight loses the event when the first copy fails. Kept and documented. ADR 0004. |
| B16 | The handler timeout cancels work in the middle. | Accept | Each sink runs in a spawned task that finishes after a 503. |
| B17 | The real app composition is never booted. | Accept | Ecosystem boot test with three plugins over real HTTP (spike S3). |
| B18 | Two TLS crypto providers. | Accept | Spike S3 records them; examples install one. D14 says only `pq-src` and `libsqlite3-sys` stay out. |
| B19 | Slow and flaky CI under coverage. | Accept | Advisory lock, nonce ids, short backoffs, a time factor. Plan 13. |
| B20 | One coverage number hides a weak crate. | Accept | Per-crate gate on `src/` lines. Plan 14. |
| B21 | The `autumn` feature on by default. | Accept | The feature is gone: the star graph removes it. |
| B22 | Scope too large; Verus without a local check. | Adapt | Waves with checkpoints and a cut list; spec-as-test proptests; the reduced Verus set. Plan 17, 19. |
| B23 | Standards and guards conflict. | Accept | Newtypes in harvest and camel types; per-example allow; a guard that allows the negation; the Verus file read at run time; `secret_env` under `testing` and `doc(hidden)`; `DeleteOutcome::GoneAfterRetry` only after a maybe-sent attempt. |
| B24 | Ecosystem wiring gaps. | Accept | Fallback transport, `Catalog::builtin().merge(..)` (it adds, it does not replace), 26 MiB cap. |

Note on B11 (2xx with a bad body on create): the issue exists, but the
client cannot read its key. For a marked create the client maps this case
to `MaybeSent`, so `Confirm` runs and finds the issue. For a plain create it
gives `Decode`.

## Red hat: feelings and developer experience

Key points: the safety story feels excellent, adoption feels heavy, and the
ecosystem example feels like a test, not a story.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| R1 | Webhook setup is boilerplate. | Accept | `webhook_endpoint()`, `webhook_endpoint_toml()`; `[jira.webhook]` optional; `endpoint_name`. Plan 8. |
| R2 | App-side tests need a harness. | Accept | `testing` feature: `plugin_for`, `app_config`, `deliver`. Plan 8.5. |
| R3 | Handler errors do not compose with `?`. | Accept | `Transient` and `Permanent`; `From<JiraError>`; `JiraResultExt::http()`. |
| R4 | Two dispatch APIs with different ownership. | Accept | Owned `Arc<Delivery>` for both `sink` and `on_event`; default `name()`. Typed sugar beyond `status_change()`: Later (G8). |
| R5 | Marker and routing labels litter Jira. | Adapt | Prefix `autumn-idem-` (configurable); routes moved to a property; one constant label; "What we write" README table guarded by `WRITES`. Label removal: Later. |
| R6 | A workflow can wait only for issues it created. | Accept | `jira_watch_issue`; the first read in the wait; `StatusTarget` shared by core and harvest. |
| R7 | `Path<IssueKey>` accepts bad input. | Accept | Strict `Deserialize`; lenient private wire types. AC1. |
| R8 | Common calls are too wordy. | Accept | `impl Into<IssueRef>`, `get_issue` with default fields, `StatusTarget::done()`, `IdempotencyKey::derive`. |
| R9 | The ecosystem teaches three ways to call Jira. | Accept | One job per tool; `san_mateo_proxy` is a named escape hatch; negatives as tests; a numbered timeline. Plan 11. |
| R10 | Running the ecosystem takes too many steps. | Adapt | `compose.yaml` and `scripts/run-ecosystem.sh`. The connection setup is in code. A real-Jira mode: Later. |
| R11 | Plugin order is a trap. | Adapt | The order rule stays (state initializers run in order); the error names the fix and detects "registered after". |
| R12 | Builder combinations fail silently. | Accept | `PluginError::Conflict` at startup. |
| R13 | `ignore_actor_account_ids` is a chore; loop by default. | Reject | Learning "self" from `myself` filters a real person when the token is personal (B9). The content marker and a service account are the fix. |
| R14 | `mode = "direct"` is jargon. | Accept | Mode from the table (`[jira.api_token]` or `[jira.san_mateo]`); `JiraSite::Subdomain` and `JiraSite::CloudId`. |
| R15 | Multi-tenant feels bolted on. | Accept | `connection_id` optional; `for_connection` returns `JiraError` and caches clients. |
| R16 | The lifecycle workflow is a demo shipped as product. | Accept | It moved to the `harvest_workflow` example as a recipe; texts are input fields. |
| R17 | The `autumn` feature on by default. | Accept | Gone (star graph). |
| R18 | Ignored override headers are silent. | Accept | A loud `ValidationError` that names `allowHeaderOverrides`. |
| R19 | One dependency for the common case. | Accept | Re-exports in the plugin crate root. |
| R20 | The README must open with the afternoon. | Accept | README order in AC40. |
| R21 | `Op` shows internal steps. | Accept | `#[non_exhaustive]`; `Op::Discovery`. |
| R22 | Keep the delights. | Accept | All kept. |

## Yellow hat: value and reuse

Key points: the safe writes and the fake are the main value; reuse
san-mateo, harvest and autumn parts instead of new code.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| Y1 | Reuse the verified san-mateo retry core. | Adapt | Copy `backoff_ms` with its proof and test it against `san_mateo::policy::backoff_ms`. Our `decide` stays our own: it has five classes and `Confirm`, which the san-mateo `decide` cannot express, and a call into another crate cannot be body-matched. |
| Y2 | Signal-or-poll wait. | Accept | Plan 9.3. |
| Y3 | Show OAuth in the ecosystem. | Accept | Expired token, refresh, discovery and metadata. Plan 11. |
| Y4 | Resolve clients from a connection id. | Accept | `ClientRef::Connection` in harvest; `for_connection` in the plugin. Camel keeps named clients in v1. |
| Y5 | A Jira sync function for records. | Later | Not needed for v1 goals; the poll covers workflows. |
| Y6 | Issue-keyed workflows with `signal_with_start`. | Later | ADR 0006. |
| Y7 | One error table for three hosts. | Accept | `JiraError::class()`. Plan 4.6. The harvest crate does not depend on `san-mateo-harvest` for `is_retryable_status`: the class table covers it, and one more dependency edge gives nothing. |
| Y8 | Metrics, actuator caching, secret rotation. | Adapt | Health keeps a TTL memo (the actuator runs one check at a time) and `max_attempts = 1`; README documents `previous_secret_envs`. Metrics: Later. |
| Y9 | Opt-in live run of the suite. | Accept | `tests/live.rs`; one case per assumption. |
| Y10 | MCP tools in the ecosystem. | Later | Not needed for v1 goals. |
| Y11 | One `IdempotencyKey` across layers. | Accept | `derive`, harvest key, camel header. `from_header` for app routes: Later. |
| Y12 | harvest `#[webhook]` recipe. | Later | |
| Y13 | `DurableRoute` ingress. | Later | |
| Y14 | Action function. | Later | |
| Y15 | `FakeJira` as a product. | Accept | `snapshot()`, `rate_limit()`, `FakeDelivery` helpers, README section. |
| Y16 | `MemoryTransport` as the router for other hosts. | Accept | `with_fallback`. |
| Y17 | Copy san-mateo examples and CI as templates. | Accept | Plan 11.4, 14 name the source files. |
| Y18 | One attempt per layer. | Accept | `for_durable_host()`. |
| Y19 | Small typed sugar. | Accept | `From<JiraError>`, `status_change()`, `StatusTarget::done()`. |
| Y20 | Trim to fund the points. | Adapt | Cut list in plan 19; camel ops reduced to six. |

## Green hat: alternatives

Key points: stronger idempotency mechanisms, a state-machine theorem, a
star graph with the seam in the core, a realistic independent fake, and an
ecosystem of checked moments.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| G1 | Key-window probe with `bulkfetch`. | Later | Depends on three unconfirmed Jira facts; listed in ADR 0003. |
| G2 | "Outcome unknown" as a value that the durable host resolves. | Adapt | `OutcomeUnknown { marker }` becomes `jira.outcome_pending` with a settle delay; the next attempt follows `host_may_send`. |
| G3 | Comment marker as a comment property. | Later | It needs one GET per candidate comment. v1 keeps the text marker. |
| G4 | Idempotency journal. | Later | |
| G5 | One theorem "no resend after maybe sent"; classify in the core. | Adapt | Expressed as `decide` clause (c) plus `host_may_send`, with `classify_status` verified. A full state machine adds code without a new guarantee. |
| G6 | Loop-free verified set; complete drift check; mutations. | Accept | Plan 6, 17.4. |
| G7 | Event seam in the core; star graph. | Accept | ADR 0001. |
| G8 | Extractor-style typed handlers. | Later | Medium cost (arity macro). |
| G9 | Durable inbox mode. | Later | ADR 0004. |
| G10 | Route by property and one constant label. | Accept | ADR 0006, with an HMAC tag. |
| G11 | Separate fake crate. | Accept | ADR 0007. |
| G12 | Sans-IO fake; no outbound server. | Adapt | Sans-IO `handle` and `HttpTransport`. `deliver_to` over HTTP is not needed: the boot test posts with `reqwest`. |
| G13 | Time and consistency in virtual time. | Accept | `index_lag(Duration)`, `TimeoutThenCommit`. |
| G14 | Fake against the spec and a live run. | Adapt | Shape check against extracted names and types; live run opt-in. Shape snapshots from live: Later. |
| G15 | Chaos proptest. | Accept | AC16. |
| G16 | Eight checked moments. | Adapt | Nine timeline lines in plan 11.3. The replay of the Postgres history is dropped from the example (the harvest tests run `replay_check` on every case). |
| G17 | Second tenant proves isolation. | Accept | EX7 test. |
| G18 | Infer the mode from the config shape. | Accept | Plan 8.1. |
| G19 | Status targets by category first. | Accept | Every example uses categories. |
| G20 | "What the plugin writes" as a tested doc. | Accept | `WRITES` and a guard. |

## Blue hat: process and control

Key points: freeze a design v2 with a decision log; spikes first; waves and
exclusive file ownership; checkpoints with gates; a definition of done; an
evidence flow; review angles; a cut list.

| # | Point | Decision | Reason or plan reference |
|---|---|---|---|
| L1 | Freeze design v2 with a decision log (F1 to F10). | Accept | This file and plan 2. F1 to F10 all accepted as proposed, except F4 (adapted: the resend opt-in exists, and the camel path has the same rule) and F7 (adapted: property routes instead of labels, from G10). |
| L2 | Five spikes first. | Accept | Plan 17.1 (S1 to S5). |
| L3 | Prepare the toolchain. | Accept | `rust-toolchain.toml` 1.99.0 with `rustfmt`, `clippy`, `llvm-tools` (the `/home/user` override is 1.97 without them); `cargo-llvm-cov` install in wave 0. |
| L4 | Waves and a short critical path. | Accept | Plan 17.1; the fake no longer waits for the core. |
| L5 | Exclusive files; one writer for shared files. | Accept | Plan 17 table. |
| L6 | The fake as an independent oracle; cut the cross-track trait link. | Accept | ADR 0007; the seam is in the core (G7). |
| L7 | Plan for the machine. | Accept | One shared target dir, per-crate test runs, at most three agents. |
| L8 | SPEC-PROOF-RED-GREEN-REFACTOR as commits; spec-as-test. | Accept | Plan 18. |
| L9 | Six checkpoints with gate commands. | Accept | Plan 17.1. |
| L10 | One definition of done. | Accept | Plan 17.2. |
| L11 | AC and EX split; owners; mutation list. | Accept | Plan 16, 17.4. |
| L12 | Grow CI, guards and docs in each wave. | Accept | ADRs written now; guards land with their feature. |
| L13 | Review by angle with a fix loop. | Accept | Plan 17.4. |
| L14 | Tiered cut list. | Adapt | Tier 0 items are already done in the design (lifecycle workflow is a recipe; one dispatch shape). Plan 19 keeps tiers 1 to 3. |
| L15 | One brief template. | Accept | Plan 17.3. |
