# CLAUDE.md

autumn-plugin-jira: a Jira Cloud client, an autumn-web 0.9 plugin, and
adapters for autumn-harvest 0.8 and rust-camel 0.57. San Mateo gives the
auth. `docs/plan.md` is the design. Follow it. A change to a public
signature changes `docs/plan.md` in the same commit.

Style for all docs and comments: ASD-STE100. Short sentences. Active voice.
Present tense. No contractions. Do not delete the reason for a rule to make
a comment short.

## Layout

| Path | Contents |
|---|---|
| `crates/autumn-jira/src/policy.rs` | Verified core: retry decision, waits, pagers, webhook freshness and status. Pure. |
| `verus/policy.rs` | Verus spec and proofs for `policy.rs`. Make the same change in both files. |
| `crates/autumn-jira` | Ids, models, ADF, JQL, errors, `JiraBackend`, `DirectBackend`, `SanMateoBackend`, `JiraClient`, webhook types, `EventSink`. |
| `crates/autumn-jira-fake` | `FakeJira`: an in-memory Jira Cloud. It does not depend on `autumn-jira`. |
| `crates/autumn-plugin-jira` | The autumn-web plugin: `[jira]` config, `JiraService`, health, webhook route. |
| `crates/autumn-jira-harvest` | Activities, `await_issue_status`, `HarvestSignalBridge`. |
| `crates/autumn-jira-camel` | The `jira:` producer component. |
| `examples/ecosystem` | The full loop. A separate workspace (ADR 0008). |
| `docs/` | `plan.md`, `examples-plan.md`, `ac-evidence.md`, `adr/`, `planning/`. |

## Commands

```bash
git config core.hooksPath .githooks   # pre-commit: fmt, clippy, test
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
JIRA_TEST_DATABASE_URL=postgres://jira:jira@localhost/jira_test cargo test --workspace --all-features --locked
cargo llvm-cov --workspace --all-features --json --summary-only --output-path cov.json
python3 scripts/coverage-gate.py cov.json 85
verus verus/policy.rs
# The separate workspace: the root commands do not see it.
cargo fmt --manifest-path examples/ecosystem/Cargo.toml --check
cargo clippy --locked --manifest-path examples/ecosystem/Cargo.toml --all-targets -- -D warnings
JIRA_TEST_DATABASE_URL=postgres://jira:jira@localhost/jira_test cargo test --locked --manifest-path examples/ecosystem/Cargo.toml
DATABASE_URL=postgres://jira:jira@localhost/jira_test cargo run --locked --manifest-path examples/ecosystem/Cargo.toml
cargo +1.89.0 check --locked --manifest-path examples/ecosystem/Cargo.toml --all-targets
```

## Rules

- Write the test first. See it fail. Then write the code.
- A change to `policy.rs` needs the same change in `verus/policy.rs`. The
  test `verus_bodies_match` compares the two. Do not accept a clippy
  auto-fix in `policy.rs`. Add an `allow` with a reason.
- Never send a non-idempotent Jira write a second time after a 5xx or a
  network error. A marker lookup or a state check is the only safe path.
  Never forward an `Idempotency-Key` header to Jira: Jira ignores it.
- Only `JiraSite::base_url` makes a base URL. Never follow a redirect.
- Webhooks are at least once. Do not claim exactly once.
- After verification, the webhook route answers 200 or 503. Never 4xx.
- No `ctx.now()` in harvest workflow code. Use `ctx.system_now()`.
- Do not log bodies, tokens or secrets. Use `san_mateo::Secret`.
- No `unwrap`, `expect`, `todo!` or `TODO` in `src/` outside tests.
- No `unsafe`. Each `allow` has a `reason`.
- No test calls a real Jira. Use `FakeJira` or `MemoryTransport`.
- Postgres tests: one pool for each test, unique ids and queues. Tests do
  not skip when `CI` is set.
- Do mutation tests on a `git clone` in the scratchpad. Never in the repo.
- A new example needs `test = true`, a CI step, a README row, a
  `RUN_TIMEOUT`, a `check` function and a failure test. The test
  `examples_wired` checks these items. It reads code, not comments, and
  the `run:` scripts of CI, not the step names.
- After a change to a crate in this workspace, update
  `examples/ecosystem/Cargo.lock`. The command is the one that the CI
  lock check gives:
  `cargo update --manifest-path examples/ecosystem/Cargo.toml -p autumn-jira -p autumn-plugin-jira -p autumn-jira-harvest -p autumn-jira-camel -p autumn-jira-fake`.
