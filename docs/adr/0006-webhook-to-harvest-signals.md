# ADR 0006: From Jira webhooks to harvest signals

- Status: Accepted
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
  with a GET, checks each tag in constant time, keeps allowlisted workflow
  names, resolves the run by workflow id, and calls
  `send_signal_idempotent` with the delivery id as the key. The key is kept
  for the life of the run.
- Errors: no run, a finished run, `Config`, `Cancelled`, `NotFound` give 200
  (counted). `ShardUnavailable`, database and pool errors give 503.
- `await_issue_status` reads the status once, then races the signal against
  a poll timer under one deadline from `system_now()`. A poll runs
  `jira_get_issue`. The workflow checks the issue id and the timestamp of
  each signal. Webhooks make the wait fast. The poll makes it correct when a
  delivery is lost.

## Consequences

- Good: no workflow id is visible on boards, and a forged route makes no
  database call.
- Good: a lost webhook (cap, `excludeBody`, Jira gave up) delays the wait by
  one poll period at most.
- Bad: one GET per tracked delivery.
- Bad: `jira_watch_issue` does a read-modify-write of the property. Two
  watchers at the same moment can lose one route.
- Bad: the bridge resolves runs on one shard. A multi-shard engine needs a
  fan-out lookup.
- Later: issue-keyed workflows started with `signal_with_start`.
