# ADR 0007: An independent, sans-IO fake Jira

- Status: Accepted
- Date: 2026-11-02

## Context

No test may call a real Jira. A scripted transport cannot model state:
create, then search, then transition, then a webhook. An outbound fake
server on a port cannot work with fixed hosts: `ReqwestTransport` builds its
own client and has no resolver override, so the only way is plain HTTP and
private hosts, which breaks the SSRF design. A fake that shares models with
the client agrees with the client on every wrong assumption.

## Decision

- `autumn-jira-fake` holds `FakeJira`, a stateful Jira Cloud. Its core is
  `handle(&HttpRequest) -> HttpResponse` with no I/O. It implements
  `san_mateo::HttpTransport`, so it sits under `DirectBackend` and under
  `SanMateo`.
- It serves the site host, the API gateway with a cloud id, the OAuth token
  host (refresh with rotation) and accessible-resources. Other hosts go to an
  optional fallback transport.
- It models projects, issue types, a workflow graph (Done needs a
  resolution), labels, properties, comments, a JQL subset, token pages,
  faults (including "the write happens, the reply is lost"), index lag in
  tokio time and signed webhook deliveries.
- It has its own HMAC and JQL code. A test checks its signature against the
  Atlassian test vector.
- A test checks each reply against the key names and JSON types of the spec
  schemas. A script extracts those names and types from the spec. A weekly
  job fails on drift.
- An opt-in live test runs the same client suite against a real site. It
  never runs in CI.

## Consequences

- Good: every layer is testable with no network, no port and no Docker.
- Good: two readings of the spec meet a third source: the spec file.
- Bad: the fake is a large crate that needs its own tests and coverage.
- Bad: facts that the spec does not state stay assumptions until the live
  test runs.
