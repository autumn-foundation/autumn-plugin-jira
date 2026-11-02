# ADR 0008: Runnable examples and the separate ecosystem workspace

- Status: Accepted
- Date: 2026-11-02

## Context

Users need code they can run and copy for each way to use the crates.
Example code that no job runs can rot. `harvest-camel` needs
`autumn-harvest-plugin` with `connectors`, which turns on the autumn-web
default features and the C builds `pq-src` and `libsqlite3-sys`. In the
main workspace these go into every build. san-mateo ADR 0006 solved
the same problem with a separate workspace.

`run()` cannot stop from code and ends the process when the boot fails. A
test of the real boot must run in a child process.

## Decision

- Four examples in the main workspace, each a cargo example with
  `test = true`: `embedded`, `autumn_app`, `harvest_workflow`,
  `camel_route`. CI runs each with `cargo run`.
- `examples/ecosystem` is its own cargo workspace with its own lock and CI
  job. It runs the alert-to-resolution flow with every crate of the
  ecosystem. Each tool has one job: camel at the edges, harvest for the
  durable flow, the Jira activities for every write, san-mateo for
  credentials, autumn for the webhook door and one route.
- Each example checks exact values. A wrong value gives an error that starts
  with `check failed:`. Failure tests change one fake reply or fault.
- `FakeJira` is the provider. The examples need no port for it and no
  dev-only proxy flags.
- The autumn tests use `TestApp`. A script boots the `autumn_app` binary in
  the dev and prod profiles. One ecosystem test boots the three-plugin app
  with `run()` in a child process and posts one signed delivery over real
  HTTP.
- Harvest examples run a real worker on Postgres, with a queue and ids per
  run. A test that cannot find its database skips locally, but fails in CI.
- A guard test (`examples_wired`) checks the shape of each example by text,
  and that the root lock stays free of the C-build crates.

## Consequences

- Good: CI proves on each PR that every integration path works.
- Good: the main workspace stays free of `pq-src` and `libsqlite3-sys`.
- Bad: a second lock to keep current. The CI lock check prints the
  `cargo update` command.
- Bad: the ecosystem example is not part of the main coverage number.
