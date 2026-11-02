# ADR 0001: Five crates in a star graph, and the v1 scope

- Status: Accepted
- Date: 2026-11-02

## Context

The user wants an autumn plugin for Jira Cloud and an example of the whole
integration ecosystem: autumn-web, san-mateo, autumn-harvest and rust-camel.
Each host has its own release cycle. One crate with features gives one
version for all hosts, so each host bump forces a release of everything. A
crate that depends on another adapter crate couples two hosts. The draft had
the harvest crate depend on the plugin crate for the event handler trait.

The test fake is large. When the fake lives in the client crate and shares
its models, a wrong reading of the Jira spec is in both places, and all
tests pass.

## Decision

- Five crates:
  - `autumn-jira`: the client, the verified policy, the backends, the
    webhook types and the event seam (`EventSink`, `HandlerError`).
  - `autumn-jira-fake`: the fake Jira. It depends on `san-mateo` for the
    transport types only. It does not depend on `autumn-jira`.
  - `autumn-plugin-jira`: the autumn-web 0.9 plugin.
  - `autumn-jira-harvest`: activities, the durable wait and the signal
    bridge.
  - `autumn-jira-camel`: the `jira:` producer.
- Each adapter depends on the core and on its own host only. The event seam
  is in the core, so the harvest bridge implements `EventSink` with no
  dependency on the plugin.
- The core depends on `san-mateo 0.1` and reuses its `HttpTransport`,
  `ReqwestTransport`, `MemoryTransport`, `Secret` and `Entropy`. The
  san-mateo backend is in the core. A sixth adapter crate gives nothing.
- Scope: Jira Cloud REST v3. Issues (create, get, edit, delete, properties),
  search, transitions, comments, projects and issue types. Admin and REST
  webhooks with an HMAC secret.

## Consequences

- Good: an autumn-web bump does not force a harvest or camel release.
- Good: the fake is a second, independent reading of the spec. A guard test
  keeps it apart from the client.
- Good: app developers can use the fake as a dev-dependency.
- Bad: five crates to publish and version.
- Out of scope: Data Center, `/rest/api/2`, Agile, Service Management, Forge
  and Connect, dynamic webhooks (JWT, 30-day expiry), attachments, worklogs,
  links, bulk create, OpenAPI code generation (huge models, poor names).
