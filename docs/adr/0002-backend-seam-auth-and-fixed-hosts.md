# ADR 0002: One backend seam, two auth modes, fixed hosts

- Status: Accepted
- Date: 2026-11-02

## Context

The client must work with a direct API token and with a san-mateo
connection (`jira-basic` or the `atlassian` OAuth provider). San Mateo adds
the auth header, refreshes OAuth tokens on a 401 and checks hosts. It also
has its own retry rules. Two retry layers make the retry count hard to
bound and the write-safety proof weak.

The `atlassian` provider uses the base `https://api.atlassian.com`. Jira
calls then need the prefix `/ex/jira/{cloudId}`. The cloud id comes from
`/oauth/token/accessible-resources`, which can list Confluence sites and
more than one Jira site.

A free-form base URL or a followed redirect is an SSRF path.

## Decision

- One trait `JiraBackend` sends one attempt. It never retries and never
  follows a redirect.
- `DirectBackend` sends Basic auth over `HttpTransport` to
  `JiraSite::Subdomain` (`https://{subdomain}.atlassian.net`) or
  `JiraSite::CloudId` (`https://api.atlassian.com/ex/jira/{cloudId}`).
  `JiraSite::base_url` is the only base URL builder (guard test).
- `SanMateoBackend` calls `SanMateo::proxy` with `retries = 0`. San Mateo
  then sends once, plus one resend after a 401 refresh. The provider comes
  from `get_integration`. The cloud id comes from config, then from the
  connection metadata, then from discovery. Discovery stores the id in the
  metadata with a merge.
- `resolve_cloud_id` keeps entries with a Jira scope. It fails on zero
  matches, and on more than one match without a site URL.
- A backend failure is `NotSent { transient }`, `MaybeSent` or `Auth`. Each
  `san_mateo::Error` and `TransportError` variant has a row in a table test.
  A new, unknown variant maps to `MaybeSent`, the safe side for writes. A
  `TransportError::Connect` is `MaybeSent`, because `ReqwestTransport` also
  maps a reset during the reply read to `Connect`.
- Ids in paths are validated newtypes and are percent-encoded.
- The plugin never holds OAuth tokens. The Basic header in API token mode is
  an explicit app choice.

## Consequences

- Good: one retry policy for both modes, in one verified place.
- Good: no redirects and fixed hosts in both modes.
- Good: san-mateo still refreshes tokens inside one attempt.
- Bad: when a 401 refresh fails, san-mateo returns the 401 reply. The client
  shows `Unauthorized`, not a refresh error.
- Bad: san-mateo maps a connect error and a timeout to the same `Upstream`
  error, so the client cannot tell them apart.
