# autumn-plugin-jira

Jira Cloud for the autumn ecosystem. You get a typed, safe Jira client, an
autumn-web plugin with a signed webhook route, durable harvest activities,
a camel `jira:` component, and an in-memory Jira for your tests.

In one afternoon, you can:

- read and write Jira issues from an autumn route with one `.plugin(..)`
  call,
- take Jira webhooks with a checked signature, and answer them correctly,
- open a ticket from a durable workflow, and wait for a person to close it,
- test all of it with no Jira site and no network.

The safe path is the default. A create, a comment or a transition is never
sent a second time after a lost reply. The client finds the first write by
its marker. Webhooks are at least once, so each handler must be idempotent.

## Crates

```mermaid
flowchart LR
  app[your autumn app] --> plugin[autumn-plugin-jira]
  flow[your harvest workflows] --> harvest[autumn-jira-harvest]
  routes[your camel routes] --> camel[autumn-jira-camel]
  plugin --> core[autumn-jira]
  harvest --> core
  camel --> core
  core --> sm[san-mateo HttpTransport]
  fake[autumn-jira-fake] -. is an .-> sm
```

| Crate | What it gives |
|---|---|
| [`autumn-jira`](crates/autumn-jira) | The client: strict ids, issues, transitions, comments, JQL search, ADF, typed errors, webhook types, the verified retry core. API token auth or San Mateo auth. |
| [`autumn-plugin-jira`](crates/autumn-plugin-jira) | The autumn-web 0.9 plugin: the `[jira]` config, the `JiraService` extractor, the `jira` health indicator, the webhook route. |
| [`autumn-jira-harvest`](crates/autumn-jira-harvest) | autumn-harvest 0.8 activities, the durable wait `await_issue_status`, and the webhook-to-signal bridge. |
| [`autumn-jira-camel`](crates/autumn-jira-camel) | The rust-camel 0.57 `jira:` producer component. |
| [`autumn-jira-fake`](crates/autumn-jira-fake) | `FakeJira`: a stateful in-memory Jira Cloud with faults, a workflow and signed webhooks. It does not depend on `autumn-jira`. |

Each crate depends on `autumn-jira` only. The plugin, harvest and camel
crates do not depend on each other. Take the crate that fits your app.

## Quick start: Jira in an autumn app

Add the plugin:

```toml
[dependencies]
autumn-plugin-jira = "0.1"
autumn-web = "0.9"
```

Put the config in `autumn.toml`. The `[jira]` section holds the names of
env vars, never the secrets. The `[[security.webhooks.endpoints]]` entry
tells autumn how to check a Jira delivery.

```toml
[jira.api_token]
site = "acme"                 # https://acme.atlassian.net
email_env = "JIRA_EMAIL"
token_env = "JIRA_API_TOKEN"

[[security.webhooks.endpoints]]
name = "jira"
path = "/jira/webhook"
provider = "generic"
secret_env = "JIRA_WEBHOOK_SECRET"
signature_header = "X-Hub-Signature"
signature_prefix = "sha256="
delivery_id_header = "X-Atlassian-Webhook-Identifier"
replay_protection = true
replay_window_secs = 86400
max_body_bytes = 27262976
```

Add one route and one event handler:

```rust,no_run
use std::sync::Arc;

use autumn_plugin_jira::{Delivery, IssueKey, JiraPlugin, JiraResultExt, JiraService};
use autumn_web::{AutumnResult, Path};

#[autumn_web::get("/issues/{key}")]
async fn summary(jira: JiraService, Path(key): Path<IssueKey>) -> AutumnResult<String> {
    // A bad key gives 400. A Jira 404 gives 404.
    let issue = jira.get_issue(key).await.http()?;
    Ok(issue.fields.summary.unwrap_or_default())
}

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .routes(autumn_web::routes![summary])
        .plugin(JiraPlugin::new().on_event(|d: Arc<Delivery>, _state| async move {
            tracing::info!(event = %d.event, "jira event");
            Ok(())
        }))
        .run()
        .await;
}
```

Run it:

```sh
export JIRA_EMAIL=you@example.com
export JIRA_API_TOKEN=...            # an Atlassian API token
export JIRA_WEBHOOK_SECRET="$(openssl rand -hex 32)"
cargo run
curl http://127.0.0.1:3000/issues/OPS-1
```

In Jira, add a webhook to `https://<your host>/jira/webhook` with the same
secret. Keep `excludeBody: false`: an empty body has no event to read.

The start makes no network call. A missing env var stops the start, and
the error names the variable. When the endpoint entry does not fit Jira,
the start fails and prints the block that fits. The code gives the same
block:

```rust
use autumn_plugin_jira::webhook_endpoint_toml;

let block = webhook_endpoint_toml("/jira/webhook", "JIRA_WEBHOOK_SECRET");
assert!(block.contains("signature_header = \"X-Hub-Signature\""));
assert!(block.contains("delivery_id_header = \"X-Atlassian-Webhook-Identifier\""));
assert!(block.contains("max_body_bytes = 27262976"));
```

### The webhook route

autumn checks the signature and the delivery id. After that check, the
route answers 200 or 503 only. An ignored delivery gets 200 and a counter.
A failed sink gets 503, so Jira sends the delivery again.

```mermaid
sequenceDiagram
  participant J as Jira Cloud
  participant A as autumn SignedWebhook
  participant R as JiraPlugin route
  participant S as your sinks
  J->>A: POST /jira/webhook
  A-->>J: 400 no signature, 401 bad signature, 409 a seen delivery id
  A->>R: the verified body
  R-->>J: 200 ignored: stale, future, self actor, unknown event, bad JSON
  R->>S: Arc<Delivery>, one sink after the other, each with a timeout
  S-->>R: Ok, Permanent, Transient, or a timeout
  R-->>J: 200 for Ok and Permanent, 503 for Transient and a timeout
  Note over J,R: After a 503, Jira sends the delivery again.
```

- The sinks run in order, each in its own task, under `handler_timeout_ms`.
  After a `Transient` error or a timeout, the later sinks do not run. A
  timed-out task runs to its end.
- Each sink can get the same delivery again, also when another sink
  fails. Make each sink idempotent.
- `ignore_actor_account_ids` drops the events of your own service account,
  so the app does not answer its own writes.
- In the prod profile, autumn needs a webhook secret of 32 bytes or more.
  It refuses the memory replay store unless
  `allow_memory_in_production = true` (one replica only). With more
  replicas, use the Redis replay store.

### Test your handler

The `testing` feature gives the plugin on `FakeJira`, an app config with
the endpoint, and a signed delivery in a few lines:

```rust
use autumn_jira_fake::FakeJira;
use autumn_plugin_jira::jira::request::IssueTypeRef;
use autumn_plugin_jira::jira::{NewIssue, ProjectKey};
use autumn_plugin_jira::testing::{DeliverExt, app_config, plugin_for};
use autumn_plugin_jira::JiraService;
use autumn_web::test::TestApp;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fake = FakeJira::new();
    let app = TestApp::new()
        .config(app_config())
        .plugin(plugin_for(&fake).on_event(|_delivery, _state| async { Ok(()) }))
        .build();
    let jira = JiraService::from_state(app.state()).ok_or("no JiraService")?;
    let task = IssueTypeRef::Name("Task".to_owned());
    let issue = jira
        .create_issue(&NewIssue::new(ProjectKey::parse("OPS")?, task, "Disk full"))
        .await?;
    fake.human_comment(issue.key.as_str(), "On it", "ann")?;
    for delivery in fake.take_deliveries() {
        app.deliver(&delivery).await.assert_status(200);
    }
    Ok(())
}
```

## Two auth modes

The table that exists in `[jira]` sets the mode. Set `[jira.api_token]` or
`[jira.san_mateo]`. Both, or none, fail the start.

**API token.** Basic auth with an email and an Atlassian API token. The
plugin reads both from the env vars that `email_env` and `token_env`
name. A key such as `token` or `password` in the config fails the start,
and the error names `token_env`. For a scoped token, set `cloud_id` in
place of `site`.

**San Mateo.** San Mateo keeps the credentials, refreshes OAuth tokens and
stores the Jira cloud id. Use the provider `jira-basic` (API token) or
`atlassian` (OAuth 2.0, path `/ex/jira/{cloudId}`).

```toml
[jira.san_mateo]
environment = "prod"
integration = "jira"
connection_id = "acme"        # optional: for_connection(id) serves other tenants
```

Register `SanMateoPlugin` before `JiraPlugin`. It puts `SanMateo` into the
app state. In the other order, the start fails with the fix.

```rust,no_run
use autumn_plugin_jira::JiraPlugin;
use autumn_plugin_san_mateo::SanMateoPlugin;

#[autumn_web::main]
async fn main() {
    autumn_web::app()
        .plugin(SanMateoPlugin::new())
        .plugin(JiraPlugin::new().webhook_route())
        .run()
        .await;
}
```

`JiraService::for_connection(id)` gives a cached client for each tenant
connection.

Other settings, with their defaults:

| Table | Keys |
|---|---|
| `[jira.http]` | `max_attempts = 5`, `base_backoff_ms = 2000`, `max_backoff_ms = 30000`, `max_pages = 1000`, `lookup_attempts = 3`, `lookup_delay_ms = 2000`, `timeout_secs = 30` |
| `[jira.webhook]` | `endpoint_name = "jira"`, `max_age_secs = 7200`, `max_future_skew_secs = 300`, `handler_timeout_ms = 8000`, `ignore_actor_account_ids = []` |
| `[jira.health]` | `cache_secs = 60`, `timeout_ms = 5000` |

autumn reads `autumn.toml`, then `autumn-<profile>.toml`, then `.env` and
the `AUTUMN_JIRA__*` env vars, for example `AUTUMN_JIRA__HTTP__MAX_PAGES=200`.
A later source wins.

The `jira` health indicator is `HealthOnly`, so `/ready` does not wait for
Jira. It calls `GET /myself` with one attempt, at most once in
`cache_secs`. Its details hold no email, account id or token.

## The client on its own

`autumn-jira` works with no plugin. Give it a transport: `ReqwestTransport`
for a real site, or `FakeJira` in a test.

```rust
use std::sync::Arc;

use autumn_jira::client::CreateOutcome;
use autumn_jira::ids::SiteName;
use autumn_jira::request::IssueTypeRef;
use autumn_jira::san_mateo::Secret;
use autumn_jira::{BasicAuth, IdempotencyKey, JiraClient, JiraSite, NewIssue, ProjectKey};
use autumn_jira_fake::{DEFAULT_API_TOKEN, DEFAULT_EMAIL, FakeJira, FakeOp};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fake = FakeJira::new();
    let client = JiraClient::api_token(
        JiraSite::Subdomain(SiteName::parse("acme")?),
        BasicAuth::new(DEFAULT_EMAIL, Secret::new(DEFAULT_API_TOKEN))?,
        Arc::new(fake.clone()),
    );
    let task = IssueTypeRef::Name("Task".to_owned());
    let issue = NewIssue::new(ProjectKey::parse("OPS")?, task, "Disk full on db-1");
    // The same key gives the same issue, also after a lost reply.
    let key = IdempotencyKey::derive(&["alert", "A-1001"]);
    let first = client.create_issue_idempotent(&issue, &key).await?;
    let again = client.create_issue_idempotent(&issue, &key).await?;
    assert!(matches!(first, CreateOutcome::Created(_)));
    assert!(matches!(again, CreateOutcome::Found(_)));
    assert_eq!(fake.count(FakeOp::CreateIssue), 1);
    Ok(())
}
```

- `JiraSite::base_url` makes the only base URL:
  `https://{site}.atlassian.net` or
  `https://api.atlassian.com/ex/jira/{cloudId}`. Ids are strict, and each
  path segment is percent-encoded.
- Reads and idempotent writes (edit, property, delete) retry 429, 408,
  425, 5xx and network errors up to `max_attempts`. The wait has a
  `Retry-After` floor, a cap and jitter. A `Retry-After` above
  `max_backoff_ms` gives `RateLimited`.
- `Jql` quotes each value. `x" OR project = SECRET` stays one literal.
  Only `Jql::raw` takes JQL text as it is.
- Searches use `POST /search/jql` with explicit fields. The pagers stop at
  `max_pages`. A repeated page token gives `PaginationStalled`.
- `JiraError::class()` gives one table of code, retry class and HTTP
  status for all hosts. `Display` shows no body, no query string and no
  secret.

## Durable workflows with harvest

`autumn_jira_harvest::activities()` gives eight activities for
`HarvestBuilder::activities`. Put `JiraClients` and a `RouteKey` into the
worker state. Each activity input holds ids and options, never a secret.

```rust,ignore
// From the harvest_workflow example, which CI runs.
let (registry, _dags, _schedules, config) = HarvestBuilder::new()
    .workflows(vec![jira_ticket_lifecycle_info()])
    .activities(autumn_jira_harvest::activities())
    .state(JiraClients::new(client))
    .state(RouteKey::new(Secret::new(route_key_from_env))?)
    .worker(config)
    .try_build()?
    .into_worker_parts();
```

| Activity | Does |
|---|---|
| `jira_create_issue` | Creates at most once for the activity idempotency key. With a `route`, the same POST adds the label `autumn-tracked` and the route property. |
| `jira_get_issue` | Reads one issue. |
| `jira_search_page` | Reads one search page at a token. |
| `jira_edit_issue` | Edits an issue (idempotent). |
| `jira_transition_to_status` | Moves to a status with a state check. A retry gives `already_there`. |
| `jira_add_comment` | Adds a comment at most once for the key. |
| `jira_delete_issue` | Deletes. A 404 after a "maybe sent" attempt gives `gone_after_retry`. |
| `jira_watch_issue` | Adds a route to an issue that a person made. A second call sends no PUT. |

`await_issue_status` waits until an issue reaches a status. Webhooks make
the wait faster. The wait does not need them: a poll ends it too.

```rust,ignore
#[workflow]
async fn wait_for_done(ctx: &WorkflowContext, issue: IssueId) -> Result<StatusWait, WorkflowFailure> {
    let options = WaitOptions::new(Duration::from_secs(24 * 3600));
    await_issue_status(ctx, &issue, &StatusTarget::done(), &options).await
}
```

- The deadline comes from `ctx.system_now()`. The history records it, so
  a replay is the same.
- A signal of another issue, or an older signal, is skipped. It does not
  move the deadline. The longest wait is `MAX_WAIT` (29 days).

`HarvestSignalBridge` is an `EventSink` for the webhook route. It turns a
status change on a tracked issue into the `jira.event` signal of the run
that waits. The route property carries an HMAC tag, so a person who edits
the property cannot signal another run. The delivery id is the signal
idempotency key: a repeated delivery gives one signal. A database error
gives 503, and Jira sends the delivery again.

```rust,ignore
let bridge = HarvestSignalBridge::new(pool, client, route_key).allow_workflow("jira_ticket_lifecycle");
let plugin = JiraPlugin::new().sink(bridge);
```

`failure_for(&JiraError)` gives the harvest error type `jira.<code>`, the
retry class of the client and `retry_after`. After an attempt that can
have sent a create or a comment, the next attempt only looks for the
marker. Found gives `outcome = "found"`. Not found gives a non-retryable
`jira.outcome_unknown`.

## Camel routes

`autumn-jira-camel` gives the producer `jira:<operation>?<options>`.

```rust,ignore
// From the camel_route example, which CI runs.
camel.register_component(JiraComponent::new(client.durable()));
let route = RouteBuilder::from("direct:in")
    .to("jira:createIssue?project=OPS&issueType=Task")
    .set_body(Body::Empty)
    .to("jira:transitionToStatus?issueKey=OPS-1&status=category:indeterminate")
    .build()?;
```

| Operation | Body in | Reply |
|---|---|---|
| `createIssue?project=OPS&issueType=Task` | `{summary, description?, labels?, fields?}` | `{id, key, outcome}` |
| `getIssue?issueKey=OPS-1` | ignored | the issue |
| `editIssue?issueKey=OPS-1` | `{setSummary?, addLabels?, removeLabels?, fields?}` | `{issue}` |
| `transitionToStatus?issueKey=OPS-1&status=category:indeterminate` | empty or `{fields}` | `{issue, outcome, status}` |
| `addComment?issueKey=OPS-1` | `{text}` or a string | `{issue, id, outcome}` |
| `search?project=OPS&maxPages=5` | ignored | `{issues, nextPageToken}` |

- The component checks every option when you add the route. An unknown
  option fails at once and lists all names.
- Out headers: `JiraIssueKey`, `JiraIssueId`, `JiraOutcome` and
  `JiraNextPageToken`.
- The headers `JiraIssueKey`, `JiraStatus` and `JiraJql` replace the
  options only with `allowHeaderOverrides=true`. Without the flag, such a
  header fails the exchange loudly.
- Under harvest-camel, the `HarvestIdempotencyKey` header makes
  `createIssue` and `addComment` idempotent. An attempt above 1 does a
  lookup only. For a route that a harvest activity calls, use
  `client.durable()`, so that harvest is the one layer that retries.

## The ecosystem example

`examples/ecosystem` runs the full loop: an alert comes in through camel,
a harvest workflow opens a ticket through San Mateo OAuth, a person
resolves it in Jira, the webhook reaches autumn, the bridge signals the
run, and camel posts a chat message.

```mermaid
flowchart LR
  alert[alert] --> camel[camel route] --> run[harvest incident_flow]
  run -- jira_create_issue --> sm[San Mateo OAuth] --> jira[FakeJira]
  person[a person resolves OPS-1] --> jira
  jira -- signed webhook --> app[autumn app JiraPlugin]
  app -- HarvestSignalBridge --> run
  run -- notify --> chat[camel san-mateo: chat]
```

It is a separate workspace, because harvest-camel brings C builds that the
main workspace must not have (ADR 0008). Run it with Postgres:

```sh
docker compose -f examples/ecosystem/compose.yaml up -d
DATABASE_URL=postgres://jira:jira@localhost/jira_test \
  cargo run --locked --manifest-path examples/ecosystem/Cargo.toml
```

See [`examples/ecosystem/README.md`](examples/ecosystem/README.md) for the
walkthrough.

## What the plugin writes to your Jira

Apart from the fields that you send, the client and its adapters write
only these items. They make retries safe and route webhooks. Do not remove
them from a live issue.

| What | Kind | When | Why |
|---|---|---|---|
| `autumn-idem-<26 chars>` | Label | `create_issue_idempotent` | A lookup finds the issue after a lost reply. |
| `autumn-jira` | Issue property | `create_issue_idempotent` | It holds the full idempotency key. A label-only match without it is skipped. |
| `autumn-ref: <26 chars>` | Comment text | `add_comment_idempotent` | A lookup finds the comment after a lost reply. |
| `autumn-tracked` | Label | `harvest route or watch` | The bridge reads only the events of tracked issues. |
| `autumn-jira-routes` | Issue property | `harvest route or watch` | The workflow runs that wait on the issue, each with an HMAC tag. |

A test compares this table with `autumn_jira::writes::WRITES`.

## Safety rules

**Safe writes.** Jira ignores an `Idempotency-Key` header, so the client
never sends one. A create or a comment carries a marker. After a 5xx or a
network error that can have reached Jira, the client never sends the write
a second time. It looks for the marker. A transition reads the status
first.

```mermaid
flowchart TD
  call[create_issue_idempotent] --> pre{lookup by marker}
  pre -- found --> found[Found: no POST]
  pre -- nothing --> post[one POST with the marker label and property]
  post -- 201 --> created[Created]
  post -- 5xx, reset or timeout --> look{lookups by marker}
  look -- found --> found2[Found: one POST in total]
  look -- nothing --> unknown[OutcomeUnknown with the marker: no second POST]
```

A plain `create_issue`, `add_comment` or `transition` has no marker. After
a lost reply, it gives `OutcomeUnknown { marker: None }`. After a 429, it
gives `RateLimited`. The opt-in `resend_after_negative_lookup` sends a
write again after a lookup finds nothing. The search index can lag, so
this can make a duplicate (ADR 0003).

**At-least-once webhooks.** Jira can send a delivery more than once.
autumn answers 409 to a repeated delivery id inside the replay window, but
a sink must still be idempotent. The bridge uses the delivery id as the signal
idempotency key.

**Fixed hosts, no redirects.** Only `JiraSite::base_url` makes a base URL.
A 3xx gives `Redirect`, and the client never calls the `Location`.

**Secrets by env var name.** The config holds the names of env vars. A
secret value is a `san_mateo::Secret`. No log line, error text or health
detail holds a token, an `Authorization` value or a body.

## Examples

| Example | Crate | Shows | Needs | Run |
|---|---|---|---|---|
| [`embedded`](crates/autumn-jira/examples/embedded.rs) | `autumn-jira` | The API token client on `FakeJira`: projects, create, edit, search pages, transition with resolution, comment, idempotent create after a lost reply, delete. | nothing | `cargo run --locked -p autumn-jira --example embedded` |
| [`autumn_app`](crates/autumn-plugin-jira/examples/autumn_app/main.rs) | `autumn-plugin-jira` | An autumn app with `GET /issues/{key}` and a log sink. `scripts/smoke-autumn-example.sh` boots it in dev and prod and posts a signed webhook. | nothing | `cargo run --locked -p autumn-plugin-jira --features testing --example autumn_app` |
| [`harvest_workflow`](crates/autumn-jira-harvest/examples/harvest_workflow.rs) | `autumn-jira-harvest` | A Postgres worker: the lifecycle recipe resolves by webhook, and a watched issue resolves by poll. | Postgres | `cargo run --locked -p autumn-jira-harvest --example harvest_workflow` |
| [`camel_route`](crates/autumn-jira-camel/examples/camel_route.rs) | `autumn-jira-camel` | `direct:in` to `jira:createIssue` to `jira:transitionToStatus`, with the out headers. | nothing | `cargo run --locked -p autumn-jira-camel --example camel_route` |
| [`ecosystem`](examples/ecosystem) | all | Alert to ticket to resolution with San Mateo, harvest, camel and autumn. A separate workspace. | Postgres | `cargo run --locked --manifest-path examples/ecosystem/Cargo.toml` |

Each example checks its own result and exits non-zero when a value is
wrong. CI runs each one. Each has a failure test.

## Development

```sh
git config core.hooksPath .githooks   # pre-commit: fmt, clippy, test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
JIRA_TEST_DATABASE_URL=postgres://jira:jira@localhost/jira_test \
  cargo test --workspace --all-features --locked
cargo llvm-cov --workspace --all-features --json --summary-only --output-path cov.json
python3 scripts/coverage-gate.py cov.json 85
verus verus/policy.rs
# The separate workspace: the root commands do not see it.
cargo test --locked --manifest-path examples/ecosystem/Cargo.toml
```

The README snippets compile as doc tests of `autumn-plugin-jira`, with the
`testing` feature. The harvest and camel snippets are marked `ignore`:
the plugin crate does not depend on those crates. They are copies of the
`harvest_workflow` and `camel_route` examples, which CI runs.

No test calls a real Jira. The tests use `FakeJira` or a
`MemoryTransport`. The Postgres tests need `JIRA_TEST_DATABASE_URL`, and
they do not skip in CI.

## Docs

- [`docs/plan.md`](docs/plan.md): the design and the acceptance criteria.
- [`docs/ac-evidence.md`](docs/ac-evidence.md): each criterion with its tests.
- [`docs/adr/`](docs/adr): the decisions, ADR 0001 to ADR 0008.
- [`CLAUDE.md`](CLAUDE.md): the rules for contributors and agents.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
