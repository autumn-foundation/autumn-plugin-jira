# Brainstorm record

- Technique: divergent brainstorming. Quantity first, judgment last.
- Date: 2026-11-02
- Inputs: the API notes on autumn-web 0.9, san-mateo 0.1, autumn-harvest
  0.8, rust-camel 0.57 and Jira Cloud REST v3; the Jira v3 OpenAPI spec; the
  san-mateo house style.
- Style: ASD-STE100.
- Marks: **MVP** = in the first release. **NICE** = after the MVP. **NO** =
  rejected, with a reason. The final scope is in [`../plan.md`](../plan.md);
  some marks changed after the six hats ([`six-hats.md`](six-hats.md)).

## 1. Idea pools

The team wrote 60 product ideas, 10 layout options, 24 Jira capabilities, 10
auth modes, 22 autumn integration points, 20 harvest ideas, 10 camel ideas
and 12 example scenarios. The clusters below keep each idea once.

| Pool | Ideas (short) |
|---|---|
| Product | typed client; one-call plugin; extractor; signed webhook route; typed events; event handlers; dedup on the delivery id; health indicator; `[jira]` config with env var names; connection resolver; multi-site; multi-tenant; cloudId discovery and metadata cache; ADF builder and reader; markdown to ADF; JQL builder and bounded check; search stream; `reconcileIssues`; approximate count; idempotent create; transition by status; "ensure status"; comments; projects and issue types; users; field metadata; edit with update ops; delete; attachments; worklogs; links; watchers; issue properties; dynamic and admin webhook registration; rate-limit awareness; per-issue write throttle; typed errors; `+0000` dates; lenient models; fake Jira; conformance suite; widgets; metrics; CLI command; sync job and san-mateo sync function; action functions; outbound app events; remote links; JSM; Data Center; Forge and Connect; webhook replay tool; secret rotation; OpenAPI code generation; typed JQL DSL; live page updates. |
| Layout | one crate with features; a san-mateo style workspace; harvest and camel as plugin features; core and plugin only; a transport trait with two impls; reuse `san_mateo::HttpTransport`; naming schemes; `verus/policy.rs` mirror; `xtask`; fixtures written from the spec. |
| Auth | classic API token; scoped API token (gateway base); san-mateo `jira-basic`; san-mateo `atlassian` OAuth; san-mateo REST proxy over HTTP; Data Center PAT; OAuth client credentials; Forge and Connect JWT; one `Auth` enum; a `JiraTarget` with a fixed host list. |
| autumn | contract and name; config section and loader; state initializer cell pattern; extractor with 503 before start; san-mateo read at startup; `HealthOnly` indicator; `SignedWebhook` route; own HMAC with `exempt_paths`; boot check of the endpoint entry; typed dispatch; job-queue dispatch; broadcast channel; harvest signal dispatch; `declare_plugin_routes` with `Public`; conformance; process roles; metrics source; widgets; test seams; child-process boot test; smoke script; replay store notes. |
| harvest | idempotent create with a property marker; with a label marker; non-retryable create without a marker; get, search, comment, transition activities; transition by state check; idempotent comment; `incident_to_ticket` workflow; wait-for-transition workflow; signal routing by workflow id or by issue property; sync loop; webhook refresh sweep; saga; child workflows; debounce; per-issue concurrency; rate limit; error map; reuse `san_mateo_proxy` only; replay and simulator tests; harvest `#[webhook]` macro. |
| camel | producer `jira:<op>`; JSON body and header overrides; typed reply; harvest-camel error map; forward the idempotency key; consumer from webhooks; polling consumer; `san-mateo:` only; validation at route add; `uri_options()` metadata. |
| Examples | alert to ticket to resolution; the same with OAuth; nightly sync; support form; deploy gate; alert storm with debounce; separate plugin and ecosystem examples; a harvest example without camel; a 503 failure path; duplicate delivery; fake as a server or as a transport; a real-Jira mode. |

## 2. Clusters and decisions

### A. Core client

- MVP: a core crate over `san_mateo::HttpTransport`; newtypes; issue CRUD;
  transitions by target status; comments; JQL search with token pages;
  projects and issue types; myself and serverInfo; ADF both ways; JQL
  quoting; typed errors; `+0000` dates; lenient models; a verified
  `policy.rs`.
- NICE: field metadata, attachments, worklogs, links, bulk create, a
  per-issue write throttle, markdown to ADF.
- NO: Agile, Data Center, Forge and Connect, JSM (other bases and auth);
  OpenAPI code generation (huge models, poor names); the legacy
  `/rest/api/3/search` (removed, 410).

### B. Auth and connection

- MVP: classic API token; san-mateo `jira-basic` and `atlassian`; a fixed
  host rule; cloudId discovery and the metadata cache.
- MVP after the hats: the scoped API token base (`JiraSite::CloudId`), as
  the same fixed-host rule covers it.
- NICE: the san-mateo REST proxy over HTTP, client credentials.

### C. autumn plugin

- MVP: contract, config, state cell, extractor, `HealthOnly` indicator,
  `SignedWebhook` route with a boot check, typed dispatch, san-mateo mode,
  conformance, process roles, test seams, boot test.
- NICE: job-queue dispatch, broadcast channel, metrics, widgets.
- NO: a CLI subcommand (needs an autumn repo change); live `ws` updates.

### D. harvest

- MVP: typed activities, the state-check transition, the error map, replay
  and simulator tests, an idempotent create.
- Changed after the hats: the marker is a label plus a property; the
  routing is an issue property with an HMAC tag (not a workflow id
  `jira-issue:<key>`, because the key is not known at the start); the
  lifecycle workflow is an example recipe; the idempotent comment is MVP.
- NO: reuse of `san_mateo_proxy` only (it loses typed ops and the safe
  create). It stays as an escape hatch in the ecosystem example.

### E. camel

- MVP: a producer-only `jira:`, options checked at route add, the
  harvest-camel error map, the idempotency key from `HarvestIdempotencyKey`.
- NICE: an in-process consumer, a polling consumer.
- NO: `san-mateo:` only (it shows nothing Jira-specific).

### F. Examples

- MVP: `embedded`, `autumn_app`, `harvest_workflow`, `camel_route` in the
  main workspace; `examples/ecosystem` in its own workspace with the full
  loop and failure tests; the fake as a transport (no port, no
  `allow_private_hosts`).
- Changed after the hats: the ecosystem uses the `atlassian` OAuth
  connection, a worker restart, a poll fallback and a three-plugin boot test.
- NICE: the sync, deploy gate and alert storm scenarios; a real-Jira mode.
- NO: the htmx support form (widgets are not MVP).

### G. Project hygiene

- MVP: plan, evidence file, examples plan, ADRs, CLAUDE.md, README with
  Mermaid; CI with fmt, clippy pedantic and nursery, Postgres tests,
  coverage of 85% or more, Verus, MSRV, `examples_wired`, the separate
  workspace job, a one-version check; lints that deny `unwrap` and `unsafe`.
- NICE: `cargo mutants` on a scratch clone (became a review step).

## 3. Top ideas

1. One core client over `san_mateo::HttpTransport`, two auth paths, one fake.
2. A verified `policy.rs`: the write retry rule, backoff with a
   `Retry-After` floor, pagination termination, the webhook status rule.
3. An idempotent create with a marker and a positive lookup; no second POST
   after a 5xx or a network error without a mechanism.
4. A transition by target status with "already there" as a result.
5. The webhook on autumn `SignedWebhook`, with a boot check and typed events,
   at least once.
6. `JiraService`, a `HealthOnly` indicator, `[jira]` config with env var
   names, and the conformance pass.
7. Typed harvest activities and a durable wait for a status.
8. A producer-only `jira:` with the harvest-camel error map.
9. The ecosystem example in its own workspace, with failure tests.
10. Fixed hosts, no redirects, no token or body in logs.
