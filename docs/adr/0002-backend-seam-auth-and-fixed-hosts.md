# ADR 0002: One backend seam, two auth modes, fixed hosts

- Status: Accepted. Amended after review round 2: the stored cloud id
  check repeats after an interval (plan V101).
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
- A stored cloud id can go stale when a tenant connects to another site.
  After a 401 or a 404 under a stored id, the backend checks the id again
  with `accessible-resources`. It does this at most one time in each
  `CLOUD_ID_RECHECK_INTERVAL` (300 s), so ordinary 404 replies do not each
  make a call. A failed check also counts, so a failure that lasts makes
  at most one call in each interval. When the id is not listed any more,
  the backend stores the new id (plan V55, V101, V119).
- `JiraBackend::refresh_site` reads the site at once, with no wait for the
  interval. Its default makes no call. `SanMateoBackend` runs the discovery
  when the target has no cloud id. The plugin calls it in
  `JiraService::refresh_default` (plan V123).
- `resolve_cloud_id` keeps entries with a Jira scope. It fails on zero
  matches, and on more than one match without a site URL.
- A backend failure is `NotSent { transient }`, `MaybeSent` or `Auth`. Each
  `san_mateo::Error` and `TransportError` variant has a row in a table test.
  A new, unknown variant maps to `MaybeSent`, the safe side for writes. A
  `TransportError::Connect` is `MaybeSent`, because `ReqwestTransport` also
  maps a reset during the reply read to `Connect`.
- Each id in a path is a validated newtype. The client percent-encodes
  each path segment.
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
