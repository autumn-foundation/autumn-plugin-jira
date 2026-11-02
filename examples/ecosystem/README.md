# The ecosystem example: alert to ticket to resolution

This example runs one incident from an alert to a chat message. It uses
every crate of the ecosystem: autumn-web, san-mateo,
autumn-plugin-san-mateo, autumn-harvest, autumn-harvest-plugin,
san-mateo-harvest, rust-camel, san-mateo-camel, harvest-camel, and the
four crates of this repository.

It is a separate cargo workspace with its own `Cargo.lock` (ADR 0008).
`harvest-camel` needs `autumn-harvest-plugin` with `connectors`. That turns
on the autumn-web default features and the C builds of libpq and SQLite.
In the main workspace, these builds would go into every build.

## Run it

The example needs Postgres. `FakeJira` is Jira Cloud and the Atlassian
OAuth server, and a `MemoryTransport` is the chat API. No run calls a real
service, and no run opens a port.

```sh
# With your own Postgres:
DATABASE_URL=postgres://jira:jira@localhost/jira_test \
  cargo run --locked --manifest-path examples/ecosystem/Cargo.toml

# Or let the script start Postgres 16 with Docker Compose:
bash scripts/run-ecosystem.sh

# The tests (EX5, EX6, EX7). They skip without a database, but not in CI.
JIRA_TEST_DATABASE_URL=postgres://jira:jira@localhost/jira_test \
  cargo test --locked --manifest-path examples/ecosystem/Cargo.toml
```

The run prints nine lines and `check: ok`. `check` compares each line with
the exact text below. A wrong value gives an error that starts with
`check failed:`, and the exit code is not zero. Each run uses its own
harvest queue and its own workflow ids, so you can run it many times on one
database. `JIRA_EXAMPLE_TIME_FACTOR=3` gives slow machines more time.

```text
[1] camel    alert A-1001 sent twice -> one run incident_flow/A-1001
[2] sanmateo token for acme expired -> refreshed (1 token call); cloudId stored in metadata
[3] escape   san_mateo_proxy GET project OPS expand=lead -> lead acct-lead
[4] jira     create: 503 after commit -> outcome_pending -> attempt 2 found OPS-1 (1 POST)
[5] jira     OPS-1 -> In Progress (moved)
[6] harvest  worker A stopped while waiting; worker B resumed the run
[7] autumn   POST /jira/webhook -> 200; bridge signalled 1 run
[8] harvest  OPS-1 Done (source signal) -> comment (created) -> chat message 201
[9] autumn   GET /incidents/OPS-1 -> OPS-1 Done
```

Each line comes from a fact of the run. The facts are a count of the
fakes, a row of the harvest database, the workflow output, or an HTTP
reply. No line is a
constant that the run prints when a step passes.

## Who does what

Each tool has one job:

| Tool | Job | File |
|---|---|---|
| camel | The edges: alert in (`harvest:` bridge), chat out (`san-mateo:`), one content enricher (`jira:getIssue`). | `src/routes.rs` |
| harvest | The durable flow `incident_flow`. | `src/workflow.rs` |
| autumn-jira-harvest | Every Jira write: create, transition, comment. The wait and the signal bridge. | `src/workflow.rs`, `src/app.rs` |
| san-mateo-harvest | One escape hatch, `san_mateo_proxy`, for a call that the client does not wrap. | `src/workflow.rs` |
| san-mateo | The credentials: Atlassian OAuth (refresh, cloud id) and the chat API key. | `src/transport.rs` |
| autumn | The webhook door and one app route. | `src/app.rs` |

## The sequence

```mermaid
sequenceDiagram
  autonumber
  participant D as Driver (main)
  participant C as camel routes
  participant H as harvest worker (Postgres)
  participant S as san-mateo
  participant J as FakeJira
  participant A as autumn app (JiraPlugin)
  D->>S: SanMateoBackend::discover_cloud_id
  S->>J: token expired: POST auth.atlassian.com/oauth/token (refresh)
  S->>J: GET accessible-resources
  S-->>D: cloudId stored in connection metadata
  D->>C: alert A-1001 to direct:alerts (twice)
  C->>H: harvest: bridge, idHeader AlertId: one run incident_flow/A-1001
  H->>S: san_mateo_proxy GET /ex/jira/{cloudId}/rest/api/3/project/OPS?expand=lead
  S-->>H: lead accountId
  H->>S: jira_create_issue (marker, route, assignee)
  S->>J: lookup (none), POST /issue
  J-->>H: 503 after commit
  H->>H: jira.outcome_pending, retry after settle
  H->>S: attempt 2: lookup only
  J-->>H: found OPS-1 (one POST in total)
  H->>S: jira_transition_to_status(in progress)
  H->>H: await_issue_status(done), worker A stops, worker B resumes
  D->>J: human_transition OPS-1 Done (actor ann)
  J-->>D: signed delivery
  D->>A: POST /jira/webhook
  A->>J: GET issue property autumn-jira-routes (bridge)
  A->>H: send_signal_idempotent(jira.event, key = delivery id)
  H->>S: jira_add_comment (marker)
  H->>C: notify_chat: direct:notify
  C->>S: jira:getIssue (enrich), then san-mateo:team-chat POST /v1/messages
  H-->>D: { issueKey OPS-1, outcome resolved }
  D->>A: GET /incidents/OPS-1
  A-->>D: { key OPS-1, status Done }
  D->>D: check(summary)
```

The driver discovers the cloud id before it sends the alert. The first
activity of the run needs the cloud id for the escape hatch path. The
connector puts it in the workflow input.

## The walkthrough, line by line

### [1] camel: one run for two sends

The broker sends the alert two times, as after a crash. The inbound route
gives each message the stable id in the `AlertId` header. harvest keeps the
id, so the second send attaches to the run of the first.

```rust
// src/routes.rs
let alerts = RouteBuilder::from(ALERTS)
    .route_id("alerts-in")
    .to(format!("harvest:{STREAM}?idHeader={ALERT_ID}"))
    .build()?;
```

The connector maps the alert to the workflow input and starts the run on
the queue of this run (`src/main.rs`, `connector`). The line counts the
runs on that queue in `harvest_workflow_executions`.

### [2] san-mateo: an expired token, one refresh, the cloud id

The driver imports the connection `acme` with an expired access token and a
refresh token. The discovery goes through `SanMateo::proxy`, so san-mateo
refreshes the token first. Then it reads `accessible-resources` and stores
`cloudId` in the connection metadata.

```rust
// src/transport.rs
let id = SanMateoBackend::new(san.clone(), jira_target())
    .discover_cloud_id()
    .await?;
```

The line counts the token calls of the fake (`token_refreshes()`) and reads
the metadata back.

### [3] escape: one call that the client does not wrap

The client has no "project with lead" call. The workflow uses the
san-mateo-harvest activity `san_mateo_proxy`, and san-mateo adds the OAuth
token. The input holds ids and a path only, never a token.

```rust
// src/workflow.rs
let lookup = ProxyInput::new(
    ENV,
    &jira_key(),
    "GET",
    format!("/ex/jira/{}/rest/api/3/project/{PROJECT}", input.cloud_id),
)
.query("expand", "lead");
```

### [4] jira: a lost create reply, and still one issue

The fake commits the first create and then replies 503. The client never
sends a create a second time after a 5xx. The activity looks up the issue by
its marker label. The search index lags 2 s, so the lookup finds nothing.
The attempt fails with the retryable `jira.outcome_pending`.
harvest waits `settle_secs` (3 s). Attempt 2 only looks up, and it finds
`OPS-1`. The history keeps only the last result of an activity, so a
harvest activity interceptor (`AttemptLog` in `src/workflow.rs`) logs each
attempt with its error type. The line reads that log and counts the POSTs
of the fake.

```rust
// src/workflow.rs
create.assignee = Some(lead.clone());
create.route = Some(route);
create.settle_secs = Some(input.settle_secs);
let created: CreateIssueOutput = ctx
    .execute_activity_with_opts(&jira_create_issue_info(), &create, Some(&queue), None, None)
    .await
    .map_err(|e| failed(&e))?;
```

When the lag is longer than the settle delay, attempt 2 finds nothing
either, and the run fails closed with `jira.outcome_unknown`. It does not
create a second issue (`tests::lost_create_fails_closed`).

### [5] jira: a checked transition

`jira_transition_to_status` reads the status before each attempt. A retry
after a lost reply gives `already_there`, not a second transition.

```rust
// src/workflow.rs
TransitionInput::new(issue.clone(), StatusTarget::in_progress()),
```

### [6] harvest: the run survives a worker restart

When the run waits, the driver stops worker A and starts worker B on the
same queue. The wait is durable: its deadline and its reads are in the
history, so worker B continues the same run.

```rust
// src/main.rs
if let Some(a) = d.runner.take() {
    a.stop().await;
}
let b = start_worker(d.state, "B").await?;
```

`notify_chat` reports the worker that ran it, and the test checks that it
is B.

### [7] autumn: the webhook door

A person moves the issue to Done. The fake signs the delivery, and the
driver posts it to the app. autumn checks the HMAC and the delivery id.
The plugin then calls the sink `HarvestSignalBridge`. The bridge reads the
route property of the issue with a strongly consistent GET. It checks the
tag in constant time and the allowlist. Then it reads the status of the
issue on the site of its own client and signals the run. The delivery id
is the signal idempotency key.

```rust
// src/main.rs and src/app.rs
let bridge = HarvestSignalBridge::new(pool.clone(), jira.clone(), key.clone())
    .allow_workflow(INCIDENT_FLOW);
JiraPlugin::with_config(jira_config()?).sink_for_all_actors(bridge)
```

The bot account `acct-bot` makes the API writes of the flow, and
`ignore_actor_account_ids` holds it. The bridge is a sink for all actors,
so a status that the bot sets still signals a waiting run. The bridge
makes no write, so it cannot make a loop. Other sinks of the app do not
get the events of the bot.

Webhooks are at least once. The same delivery again gives 409 and no second
signal. A tampered copy gives 401. A copy from the bot gives 200 and goes
to the bridge only. It has its own delivery id, so the run takes it as a
late signal, or the run is already finished (`tests::webhook_negatives`).

### [8] harvest: the end of the run

The signal ends the wait. The run adds one comment with its marker, then
calls the `notify` route. The route enriches the exchange with the issue
(`jira:getIssue`), and posts the message with san-mateo. It copies the
harvest idempotency key to the chat API, so a retry makes no second
message.

```rust
// src/routes.rs
ex.input.set_header("SanMateoProxy-Idempotency-Key", key);
```

A webhook can come after the wait, while the run adds the comment or
tells the chat. No code reads that signal, and a strict replay fails on a
history that ends with it. So `incident_flow` takes the late signals as
its last step.

```rust
// src/workflow.rs
drain_jira_signals(ctx)?;
```

With a wrong webhook secret, every POST gives 401, and a poll ends the
wait (`source poll`). With no webhook and no poll, the run takes the
timeout branch and adds an escalation comment.

### [9] autumn: the app route

The app route uses `JiraService` in `san_mateo` mode. The plugin reads the
cloud id from the connection metadata, so it makes no discovery call.

```rust
// src/app.rs
#[autumn_web::get("/incidents/{key}")]
pub async fn incident(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<Json<Value>> {
    let issue = jira.get_issue(key).await.http()?;
```

## The tests

| Test | Shows |
|---|---|
| `tests::happy_path_timeline_is_exact` | The nine lines, one POST, one refresh, the cloud id, the restart, one chat message with a key. |
| `tests::lost_create_fails_closed` | A lag longer than the settle delay gives `jira.outcome_unknown` and one POST. |
| `tests::bad_webhook_secret_resolves_by_poll` | Each POST gives 401; a poll ends the wait. |
| `tests::no_webhook_no_poll_times_out` | The timeout branch and its escalation comment. |
| `tests::a_wrong_value_fails_the_check` | A wrong line gives `check failed:`. |
| `tests::webhook_negatives` | 409 for the same delivery, 401 for a tampered one, 200 for the bot; one signal. |
| `tests::two_tenants_stay_apart` | Two connections on two fake sites, one plugin, no mix. |
| `tests::is_retryable_copy_matches_harvest_camel` | The pinned copy in `crates/autumn-jira-camel/tests/retry_rule.rs` equals the real `harvest_camel::is_retryable`. |
| `tests::boot_three_plugins_real_http` | `run()` with `SanMateoPlugin`, `HarvestPlugin` (external mode) and `JiraPlugin` takes one signed POST over real HTTP. |

## Notes for production

- Use `PgStore` for san-mateo (all workers share one store), and read the
  keys from a secret store.
- Use `ReqwestTransport` in place of `FakeJira`, and a broker consumer such
  as `from("kafka:alerts")` in place of `direct:alerts`.
- The graph has two rustls crypto providers (`ring` and `aws-lc-rs`).
  `main` installs `ring` before any client is built.
- Stop in this order: the connector, the harvest worker, then camel.
