# ADR 0004: Inbound webhooks: verification, dedup and status codes

- Status: Accepted. Amended after review round 1 (plan V71 to V80, V88).
  The amendments are the config bounds, the handler slots, the body-cap
  memory note, the ignored actors and the one-site rule.
- Date: 2026-11-02

## Context

Jira admin and REST webhooks with a secret send
`X-Hub-Signature: sha256=<hex>` (HMAC-SHA256 of the raw body) and
`X-Atlassian-Webhook-Identifier` (the same value on each retry). Jira
retries up to 5 times on 408, 409, 425, 429, 5xx and connection errors. It
does not retry other 4xx. Payloads can reach 25 MB. Delivery is at least
once, with no order.

autumn-web 0.9 has `SignedWebhook`: a constant-time HMAC check, a replay
store on the delivery id, and replay key release on a 5xx. It finds the
endpoint by `parts.uri.path()`. An axum `nest` strips the prefix from that
path, so a nested plugin route gets 500 for every delivery. The default body
cap is 1 MiB, and an over-cap body gets 400. A duplicate id gets 409.

## Decision

- The plugin mounts `POST {prefix}/webhook` at the full path with `merge`,
  declared `Public` with `declare_plugin_routes`.
- The app declares the endpoint in `[[security.webhooks.endpoints]]`
  (provider `generic`, the two Jira headers, no timestamp header). The
  plugin checks the entry at boot and fails with the TOML to paste. The
  helper `webhook_endpoint` builds a correct entry. A body cap below 8 MiB
  logs a warning; the recommended cap is 26 MiB.
- Config bounds (plan V74): the boot check needs `max_age_secs` +
  `max_future_skew_secs` < `replay_window_secs`. Else a captured copy can
  be fresh after autumn forgets its delivery id, and the route accepts it
  again. `max_future_skew_secs` is at most 300, the verified bound
  `policy::MAX_FUTURE_SKEW_MS`. A `max_age_secs` below
  `JIRA_RETRY_HORIZON_SECS` (5400) logs a warning only. Jira sends a
  delivery again after a 503 with the first timestamp, for up to about
  90 minutes. A smaller age makes that copy stale, and the route drops it
  with 200. The app owns that choice, so the start does not fail.
- After verification the plugin answers 200 for accepted and ignored
  deliveries, and 503 for a handler fault or a timeout (`webhook_status`,
  verified). It never answers a 4xx, because Jira does not retry it.
- The plugin answers 200 and ignores a delivery for each of these reasons.
  Each reason has a counter in the health details.
  - It has no timestamp.
  - It is stale: older than `max_age_secs` (default 2 h, with the skew
    inside the replay window).
  - It is too far in the future (300 s).
  - Its actor is in `ignore_actor_account_ids`, and no sink of
    `sink_for_all_actors` exists. Such a sink gets the delivery, and the
    other sinks do not (plan V71). A sink that waits for a state, for
    example `HarvestSignalBridge`, needs it.
  - Its event is unknown.
  - Its body does not parse.
- The plugin keeps autumn's 409 for a duplicate. A 200 is wrong when
  the first copy is still in flight and then fails: Jira does not send it
  again. Jira sends a delivery that got 409 again, up to 5 times. Each copy
  gets 409. The first copy carries the event.
- Each sink runs in a spawned task under `handler_timeout_ms`. On a timeout
  the route answers 503 and the task continues.
- Handler slots (plan V73): a task that times out holds one of
  `max_handler_tasks` slots (default 64) until it ends. The plugin stops
  it at `abandon_after_ms` (default 300 000). With no free slot, the route
  answers 503 at once and starts no task. Jira then sends the delivery
  again. So a sink that hangs cannot make tasks without a limit.
- When autumn stops the request at its request timeout, a drop guard
  counts the delivery as `timed_out` (plan V79). The start warns when the
  handlers can take longer than that timeout.
- One site for each endpoint (plan V88): a delivery does not name its Jira
  site, and two sites can have the same issue id. So one webhook endpoint
  and one secret serve one site. Give each site its own endpoint, secret
  and bridge.
- Dedup layers: the autumn replay store, then `send_signal_idempotent` per
  workflow run, then idempotent sinks. The docs say "at least once".
- v1 accepts HMAC webhooks only. A dynamic webhook with a Bearer JWT gets
  400 from autumn.

## Consequences

- Good: verification and replay protection come from autumn, in constant
  time.
- Good: a handler fault that gives 503 does not lose the event. The 503
  frees the replay keys, and Jira sends the delivery again.
- Bad: a process crash during a handler can lose a delivery. autumn stores
  the replay keys before the handlers run. A crash (an OOM kill, a
  SIGKILL) sends no reply and does not free the keys. With the Redis
  replay store, the keys stay for `replay_window_secs`. Each Jira retry
  then gets 409, and no sink gets the event. The memory store loses its
  keys with the process, so a retry to a new process gets through.
- Mitigation: the sinks dedup on their own (`send_signal_idempotent`). A
  wait with a poll ends within one poll period (ADR 0006). An upstream
  autumn change can close the gap. The key gets a short life while the
  handler runs. It gets the full window only after a reply that is not a
  5xx.
- Bad: the body cap is memory that an unsigned request can use (plan
  V80). autumn reads the body before it checks the HMAC. So with the
  26 MiB cap, each request at the public path without credentials can make
  the app hold 26 MiB until the 401. Use proxy limits on body size, rate
  and concurrency, or a smaller cap.
- Bad: a duplicate shows as a failed delivery in the Jira webhook log.
- Bad: a memory replay store is per process and empty after a restart. In
  production autumn requires an explicit opt-in for it, or Redis.
- Later: a durable inbox mode that starts a harvest run per delivery id.
