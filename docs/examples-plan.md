# Plan: runnable examples

- Status: Accepted (as built for EX1 to EX4; EX5 to EX8 in progress, wave 4).
- Date: 2026-11-02
- Style: ASD-STE100. Short sentences. Active voice. Present tense.
- Inputs: [plan.md](plan.md) sections 11, 12 and 16.2, ADR 0008, the
  san-mateo examples plan (`/home/user/san-mateo/docs/examples-plan.md`),
  and the wave reports of WP3 to WP6.

## 1. Problem

Users need code that they can run and copy for each way to use the Jira
crates. There are five ways: the client alone, the autumn-web plugin, the
harvest activities, the camel component, and all of them together. The README has short
snippets. They compile as doc tests, but a doc test does not show a full
flow. The task: four runnable examples in the main workspace, one example
of the whole ecosystem in its own workspace, and CI runs each one.

## 2. Research

| Source | Finding | Use |
|---|---|---|
| san-mateo examples | A cargo example with `test = true`. `main` reads `DATABASE_URL`. It checks its result with exact values and fails when it is wrong. A run timeout. CI does `cargo run --example`. A guard test checks the wiring by text. | The house style for all five examples. |
| harvest-camel `examples/order_flow.rs` | The runner and connector wiring of a camel route under harvest. | The ecosystem setup (plan 11.4). |
| `FakeJira` (plan 5) | A stateful in-memory Jira Cloud. It is an `HttpTransport`. It has faults, index lag in tokio time, OAuth, signed deliveries and a fallback host. | The Jira site of every example. No port, no dev-only proxy flags. |
| autumn-web 0.9 `run()` | No stop from code. `process::exit(1)` on a failed boot. A global tracing subscriber. | The test uses `TestApp`. A script boots the real binary and stops it with SIGTERM. |
| autumn-web 0.9 webhooks | `SignedWebhook` looks up the endpoint by the full path. A missing webhook secret gives an error that does not name the env var. | The plugin mounts with `merge`. The `autumn_app` failure test uses a missing `JIRA_API_TOKEN`, which the plugin names. |
| autumn-harvest 0.8 | Postgres for async activities. `migrate::apply` under `pg_advisory_lock` is safe in parallel. An activity with no queue goes to `"default"`. Strict replay fails a history that ends with signals that no code read. | A real worker on a queue for each run. Activities go to `ctx.queue_name()`. The wait drains its signals (ADR 0006). |
| harvest-camel deps | `autumn-harvest-plugin` with `connectors` turns on the autumn-web default features and the C builds of libpq and SQLite. | The ecosystem example is its own cargo workspace (ADR 0008). |

## 3. Brainstorming

Ideas for each example. The chosen ideas are in bold.

- embedded:
  - **each client area in one run (projects, issue types, CRUD, search
    pages, transition with a resolution, comment, delete)**
  - **an idempotent create after a 503 after the commit**
  - **a 400 on a bad field gives `Validation`**
  - a live mode against a real site (left out: no test calls a real Jira)
- autumn_app:
  - **one `.plugin(..)` call**
  - **a route with the `JiraService` extractor and `.http()?`**
  - **a log sink on the webhook route**
  - **config in `autumn.toml` with env var names only**
  - **a boot of the real binary in the dev and prod profiles**
  - **a signed POST with an openssl HMAC**
  - **SIGTERM stop**
  - **a real path when `JIRA_SITE` is set**
- harvest_workflow:
  - **the recipe `jira_ticket_lifecycle`**
  - **a lost create that the next attempt finds by its marker**
  - **a webhook delivery into `HarvestSignalBridge`**
  - **`jira_watch_issue` on an issue that a person made, done by poll
    only**
  - **two runs at the same time on one database**
- camel_route:
  - **`jira:createIssue` then `jira:transitionToStatus`**
  - **the `HarvestIdempotencyKey` header and a second attempt that sends
    no second write**
  - **the out headers**
  - **an unknown option fails at route add**
  - a consumer route (left out: the component is producer only)
- ecosystem:
  - **the full loop from an alert to a chat message (plan 11.3)**
  - **an expired OAuth token and a cloud id discovery**
  - **a worker restart during the wait**
  - **two tenants on two fake sites**
  - **the real harvest-camel `is_retryable` against the pinned copy**
  - **a boot of three plugins over real HTTP**

## 4. Reverse brainstorming (how to make the examples fail)

| How to fail | Counter-measure |
|---|---|
| An example compiles, but no job runs it. | CI runs each example as a user does (`cargo run`). The guard `examples_wired` checks the CI step. |
| An example passes when the feature is broken. | Each example checks exact values: outcomes, headers, request counts, Jira state. Each has a failure test. |
| A `check` passes on any value. | Each example has `a_wrong_value_fails_the_check`: a changed value gives an error that starts with `check failed:`. |
| An example hangs CI. | `RUN_TIMEOUT` around the scenario. `timeout-minutes` on the jobs. Each stop runs on all paths. |
| A database test skips in CI and stays green. | With no URL, a test skips, but it first asserts that `CI` is not set. |
| Two runs on one database take the tasks of each other. | A unique queue and workflow ids from a nonce for each run. A CI step runs two copies of `harvest_workflow` at the same time. |
| An example teaches an unsafe setting. | No `allow_private_hosts`, no `allow_plain_http`, no `full_migrations_sql` (guard). Config holds env var names only. |
| An example leaks a secret. | The smoke script uses random secrets and scans the app log for them. The summaries hold ids and texts only. |
| The ecosystem dependencies enter the main workspace. | A separate workspace. The guard checks the root `Cargo.lock`. |
| The ecosystem lock goes stale when a crate here changes. | The first CI step checks the lock and prints the `cargo update` command. |
| A shell step hides a failure. | `set -euo pipefail`. Wait for each process and check each exit code. `shellcheck` in the `lint` job. No `\|\|` after a checked command (guard). |

## 5. Six thinking hats

- **White (facts):** the five library crates pass 345 workspace tests.
  `FakeJira` covers each endpoint that the client calls. CI has `lint`,
  `test` (with coverage), `examples`, `msrv`, `verus`, `deps` and
  `example-ecosystem`.
- **Red (feelings):** users want to copy one file and see it work. They do
  not want to set up a Jira site or an OAuth app.
- **Black (risks):** `run()` cannot run in a test. The harvest examples
  need Postgres. The ecosystem job is slow on a cold cache. Strict replay
  in harvest 0.8 fails on signals that no code read.
- **Yellow (benefits):** the examples are also end-to-end tests of the
  adapters. CI proves on each PR that the paths work together.
- **Green (ideas):** a guard test for the wiring. A failure test for each
  example. A weekly run with new dependencies. A parity test between the
  camel error map and harvest-camel.
- **Blue (process):** plan, then for each example: RED (the test, see it
  fail), GREEN (the scenario), REFACTOR (fmt, clippy, STE). Then CI, the
  README rows, review and the evidence.

## 6. Design

| Example | EX | Target | Postgres | CI |
|---|---|---|---|---|
| `embedded` | EX1 | `crates/autumn-jira/examples/embedded.rs` | No | `examples` job: `cargo run --locked -p autumn-jira --example embedded` |
| `autumn_app` | EX2 | `crates/autumn-plugin-jira/examples/autumn_app/` (`main.rs`, `autumn.toml`, `autumn-prod.toml`) | No | `examples` job: `bash scripts/smoke-autumn-example.sh` |
| `harvest_workflow` | EX3 | `crates/autumn-jira-harvest/examples/harvest_workflow.rs` | Yes | `examples` job: `cargo run`, then two runs at the same time |
| `camel_route` | EX4 | `crates/autumn-jira-camel/examples/camel_route.rs` | No | `examples` job: `cargo run --locked -p autumn-jira-camel --example camel_route` |
| `ecosystem` | EX5 to EX7 | `examples/ecosystem/` (own workspace) | Yes | `example-ecosystem` job: lock, fmt, clippy, test, run, MSRV 1.89, one version each |

Rules for each example (EX8 checks them by text):

- One file, or one folder. A `//!` header with the flow, the run command
  and what to change for production.
- `[[example]] test = true`. The `#[cfg(test)]` tests run the same `run`.
- `run` returns a summary. A pure `check` compares exact values. Its error
  starts with `check failed:`. `main` prints the summary and exits non-zero
  when `check` fails.
- `const RUN_TIMEOUT` wraps the scenario. The examples on Postgres scale it
  with `JIRA_EXAMPLE_TIME_FACTOR` (coverage sets 3).
- A failure test with `expect_err(`.
- A CI step with no `||`, and a README row.
- No `unwrap` or `expect` outside tests. No `unsafe`. No dev-only proxy
  flag.

## 7. The examples

### 7.1 `embedded` (EX1)

- **Goal:** show the client alone, with no framework.
- **What it shows:** `JiraClient::api_token` on `FakeJira`. It lists
  projects and issue types. It creates, reads and edits an issue, and reads
  search pages. It moves the issue to Done with a resolution and adds a
  comment. It does an idempotent create after a 503 after the commit (one
  POST, outcome `Found`). Then it deletes an issue.
- **How it runs in CI:** the `examples` job runs `cargo run --locked -p
  autumn-jira --example embedded`. The run prints the summary and `check:
  ok`. `RUN_TIMEOUT` is 30 s.
- **Check and failure tests:** ex: `embedded::the_example_runs_and_checks`
  runs `run` and `check`. `embedded::a_bad_field_is_a_validation_error`
  sends a bad field and wants `JiraError::Validation`.
  `embedded::a_wrong_value_fails_the_check` changes one value and wants
  `check failed:`.

### 7.2 `autumn_app` (EX2)

- **Goal:** show the autumn-web plugin in a real app.
- **What it shows:** one `.plugin(JiraPlugin::..)` call. The route `GET
  /issues/{key}` uses the `JiraService` extractor and `.http()?`. A log
  sink on `POST /jira/webhook`. `autumn.toml` holds env var names only. The
  app uses `FakeJira` unless `JIRA_SITE` is set. With `JIRA_SITE`, it uses
  a `ReqwestTransport` and the plugin reads `JIRA_EMAIL` and
  `JIRA_API_TOKEN` (plan V34). The example needs the `testing` feature
  (`required-features`).
- **How it runs in CI:** the `examples` job runs `bash
  scripts/smoke-autumn-example.sh`. The script builds the binary and boots
  it in the dev and prod profiles. It checks `GET /issues/OPS-1` (200), an
  unknown issue (404) and a bad key (400). It sends a signed POST with an
  openssl HMAC (200), the same POST again (409) and a bad signature (401).
  It finds `jira` in `/actuator/health`. It stops the app with SIGTERM (exit code 0). It finds
  no secret in the log. The real path with no `JIRA_API_TOKEN` must exit
  non-zero and name the variable.
- **Check and failure tests:** ex:
  `autumn_app::the_app_serves_issues_and_webhooks` runs the same routes and
  the `[jira]` section of the same `autumn.toml` in `TestApp`.
  `autumn_app::a_wrong_value_fails_the_check` and
  `autumn_app::the_demo_fake_has_the_issue`. `plugin/t/boot.rs` has
  `missing_secret_exits_and_names_the_variable` and
  `the_app_boots_and_takes_a_signed_post` over a child process
  (`boot_child_server`).

### 7.3 `harvest_workflow` (EX3)

- **Goal:** show the Jira activities in a durable harvest workflow.
- **What it shows:** a real harvest worker on Postgres. Run 1 is the recipe
  workflow `jira_ticket_lifecycle`. First, `jira_create_issue` with a route
  gets a 503 after the commit. Attempt 1 gives `jira.outcome_pending`.
  Attempt 2 finds the issue by its marker, so there is one POST. Then
  `jira_transition_to_status` moves the issue to In Progress.
  `await_issue_status` waits for Done. A person resolves the issue, and the
  `FakeJira` delivery goes into `HarvestSignalBridge::deliver`. Last,
  `jira_add_comment` adds a comment. Run 2 is `jira_watch_lifecycle`: `jira_watch_issue` on
  an issue that a person made, and a wait that ends by poll only. All texts
  are input fields (R16). The workflows live in the example, not in `src`.
- **How it runs in CI:** the `examples` job runs `cargo run --locked -p
  autumn-jira-harvest --example harvest_workflow` once, then two copies at
  the same time on one database. Each run has its own queue and workflow
  ids. `RUN_TIMEOUT` is 120 s, times the time factor.
- **Check and failure tests:** ex:
  `harvest_workflow::lifecycle_resolves_by_webhook`,
  `harvest_workflow::watch_resolves_by_poll`,
  `harvest_workflow::a_400_fails_with_one_post` (a 400 on create gives one
  POST and a workflow error with no retry), and
  `harvest_workflow::a_wrong_value_fails_the_check`. The tests read
  `JIRA_TEST_DATABASE_URL`, and `main` reads `DATABASE_URL`.

### 7.4 `camel_route` (EX4)

- **Goal:** show the `jira:` producer in a camel route under harvest.
- **What it shows:** `direct:in` to `jira:createIssue?project=OPS&
  issueType=Task`, `.set_body(Body::Empty)`, then
  `jira:transitionToStatus?status=category:indeterminate&
  allowHeaderOverrides=true`. The second step takes the issue from the
  `JiraIssueKey` out header of the first. The exchange has the
  `HarvestIdempotencyKey` header, as harvest-camel sends it. Attempt 1
  creates and moves the issue. Attempt 2 (a harvest retry, same key) finds
  the issue by its marker. It sends no second write: one create POST, one
  transition, outcome `moved` then `already_there`. The component has a
  durable client (`ClientSettings::for_durable_host()`).
- **How it runs in CI:** the `examples` job runs `cargo run --locked -p
  autumn-jira-camel --example camel_route`. `RUN_TIMEOUT` is 30 s.
- **Check and failure tests:** ex: `camel_route::the_route_creates_and_moves`
  checks the out headers and the Jira state with exact values.
  `camel_route::unknown_option_fails_at_route_add` wants `InvalidUri` that
  lists the option names. `camel_route::a_wrong_value_fails_the_check`.

### 7.5 `ecosystem` (EX5, EX6, EX7)

- **Goal:** show the whole ecosystem in one run: an alert becomes a ticket,
  a person resolves it, and a chat message goes out (plan 11).
- **What it shows:** camel at the edges (the `harvest:` bridge in, the
  `san-mateo:` chat out, one `jira:getIssue` enricher). harvest runs the
  durable flow `incident_flow`. The `autumn-jira-harvest` activities do
  each Jira write. san-mateo-harvest gives one escape hatch,
  `san_mateo_proxy`. san-mateo keeps the Atlassian OAuth credentials (an
  expired token, a refresh, a cloud id in the metadata) and the chat API
  key. autumn is the webhook door (`JiraPlugin` in san_mateo mode, sink
  `HarvestSignalBridge`) and one route `GET /incidents/{key}`. The run
  prints the nine timeline lines of plan 11.3 and `check: ok`.
- **How it runs in CI:** the `example-ecosystem` job, with Postgres 16
  and the toolchains 1.89.0 and the pinned one. Its steps: the lock check
  with the fix command, the one-version check, fmt, clippy `-D warnings`,
  `cargo test --locked`, `cargo run --locked`, and `cargo +1.89.0 check
  --locked --all-targets`.
  `timeout-minutes: 45`. Locally: `bash scripts/run-ecosystem.sh` starts
  Postgres with `compose.yaml` when needed.
- **Check and failure tests:** `#[cfg(test)]` modules in
  `examples/ecosystem/src/tests.rs`. EX5: `happy_path_timeline_is_exact`.
  EX6: `lost_create_fails_closed`, `bad_webhook_secret_resolves_by_poll`,
  `no_webhook_no_poll_times_out`, `a_wrong_value_fails_the_check`. EX7:
  `webhook_negatives`, `two_tenants_stay_apart`,
  `is_retryable_copy_matches_harvest_camel`, `boot_three_plugins_real_http`.
  Status: done (WP7).

### 7.6 The wiring guard (EX8)

- **Goal:** an example cannot lose its tests, its CI step or its README row
  without a red build.
- **What it checks:** `core/t/examples_wired.rs` reads the files as text.
  `each_example_is_wired` checks `test = true`, `RUN_TIMEOUT`,
  `#[cfg(test)]`, `fn check(`, `expect_err(`, a CI step with no `||`, and a
  README row. `no_example_turns_on_the_dev_only_proxy_flags` checks for
  `full_migrations_sql`, `allow_private_hosts` and `allow_plain_http`.
  `each_example_file_is_in_the_list` finds an example file that is not in
  the list. `the_ecosystem_workspace_stays_apart` checks the separate
  workspace.
- **How it runs in CI:** in the `test` job, with the other tests.
- Status: done (WP8a).

## 8. Acceptance criteria

Plan section 16.2 holds EX1 to EX8 with their tests.
[ac-evidence.md](ac-evidence.md) gives the status and the evidence of each.

## 9. Out of scope

- A real Jira site in any example or test. `crates/autumn-jira/tests/live.rs`
  has ignored live cases, and they never run in CI.
- A real-Jira mode for the ecosystem example (plan section 20).
- A consumer `jira:` endpoint. The component is producer only.
