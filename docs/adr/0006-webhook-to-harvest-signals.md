# ADR 0006: From Jira webhooks to harvest signals

- Status: Accepted. Amended for design v3 (as built): the end-of-wait
  drain. Amended after review round 1: the drain function, the read times,
  the wait bounds, the bridge status read and its limits (plan V83 to
  V89).
- Date: 2026-11-02

## Context

A workflow creates or watches an issue and then waits for a status. The
webhook payload has the issue fields (labels included), but no issue
properties. Comment payloads carry a reduced issue. The issue key is not
known when the workflow starts. A routing label with the workflow id shows
the id to every Jira user, fills the global label list, and any user can
forge it.

`WorkflowContext::now()` returns the start time of the run, always. A
deadline from `now()` never comes closer in production. `system_now()`
records the wall clock in history.

## Decision

- Routes live in the issue property `autumn-jira-routes`:
  `{"routes": [{"wf": name, "id": workflow id, "tag": hmac}]}`. The tag is
  HMAC-SHA256 with an app `RouteKey` (32 bytes or more) over
  `"v1\n" + name + "\n" + id`, cut to 32 hex chars.
- One constant label `autumn-tracked` marks a routed issue. A Jira admin can
  filter webhooks at the source with `labels = autumn-tracked`.
- `jira_create_issue` writes the label and the property in the create body.
  `jira_watch_issue` adds them to an issue that a person made.
- `HarvestSignalBridge` is an `EventSink`. It acts on `jira:issue_created`
  and on `jira:issue_updated` with a status change. It reads the property
  with a GET and checks each tag in constant time. It keeps the allowlisted
  workflow names and resolves the run by workflow id. Then it calls
  `send_signal_idempotent` with the delivery id as the key. The key is kept
  for the life of the run.
- Status read (plan V88): for a route that passes both checks, the bridge
  reads the issue status on the site of its own client. The signal holds
  that status and key, not the payload values. A delivery does not name
  its site, and two sites can have the same issue id. So a delivery of
  site B cannot give a wait on site A a false status. One bridge serves
  one site, with its own endpoint and secret.
- Register the bridge with `JiraPlugin::sink_for_all_actors` when the app
  moves issues with an account in `ignore_actor_account_ids` (plan V71).
  Else a status change by that account signals nothing, and only the poll
  ends the wait.
- Errors: no run, a finished run, `Config`, `Cancelled`, `NotFound` give 200
  (counted). `ShardUnavailable`, database and pool errors give 503.
- `await_issue_status` reads the status once, then races the signal against
  a poll timer under one deadline from `system_now()`. A poll runs
  `jira_get_issue`. The workflow checks the issue id and the timestamp of
  each signal. Webhooks make the wait fast. The poll makes it correct when a
  delivery is lost.
- Drain (design v3): before it returns, `await_issue_status` takes each
  `jira.event` signal that is already in the history
  (`ctx.drain_signals_raw`). Strict replay in harvest 0.8 (the sweep,
  `replay_check` and `WorkflowReplayer`) gives `EarlyCompletion` for a
  completed history that ends with signals that no code read. A minimal
  `race(signal, timer)` workflow that ignores signals failed on 18 of 64
  seeds without the drain, and passed on 64 of 64 with it. Live mode did
  not fail. A drain on the `TimedOut` path only does not help: a `Reached`
  result can also leave signals in the history.
- `poll_every` must be 1 s or more (`MIN_POLL_EVERY`). A zero poll gives a
  hot loop of reads.
- Late signals (plan V84): a webhook can come after the wait, while the
  workflow runs other activities. The history then ends with a signal
  that no code read, and a strict replay fails. So a workflow that works
  after the wait calls `drain_jira_signals(ctx)?` as its last step before
  it returns.
- Read times (plan V86): each read of the wait moves the newest time by
  `issue.updated`, so a signal older than a read is stale.
- Bounds (plan V83, V85, V87): a wait has at most `MAX_POLL_ROUNDS` (3000)
  poll rounds. It takes at most `MAX_SIGNAL_ROUNDS` (1000) signals that do
  not end it. Its deadline is at most `LAST_READ_MARGIN` (46 min) before
  the execution deadline of the run. The waits of one run run one at a
  time. A retryable failure of a poll read is a missed poll.

## Consequences

- Good: no workflow id is visible on boards, and a forged route makes no
  database call.
- Good: a lost webhook (cap, `excludeBody`, Jira gave up) delays the wait by
  one poll period at most.
- Bad: two GETs per tracked delivery: the routes property, then the
  status of each routed delivery. A forged route makes no extra call.
- Bad: the route tag binds the run, not the issue (plan V89). A copied
  route entry on another issue can make signals for the run. The wait
  checks the issue id of each signal, the bridge reads the status from
  its site, and `MAX_SIGNAL_ROUNDS` bounds the rounds. A tag with the
  issue id needs a second PUT after the create. That is an open decision.
- Bad: `jira_watch_issue` does a read-modify-write of the property. Two
  watchers at the same moment can lose one route.
- Bad: the bridge resolves runs on one shard. A multi-shard engine needs a
  fan-out lookup.
- Bad: the drain takes each `jira.event` signal that came before the end of
  the wait. A workflow that reads `jira.event` itself after the wait does
  not get them. A later wait reads the issue first, and its read time
  makes an older signal stale, so it does not need them.
- Bad: the bridge returns 503 at the first transient route error. The
  routes before it can already have their signal. Jira sends the delivery
  again, and those routes then give `Duplicate`, so the result is correct.
- Upstream: the `EarlyCompletion` behaviour of strict replay is a harvest
  0.8 issue. When harvest fixes it, the drain can go.
- Later: issue-keyed workflows started with `signal_with_start`.
