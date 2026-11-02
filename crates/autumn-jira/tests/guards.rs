//! Guard tests (plan section 16). Each guard reads files of the repository
//! as text and checks one rule of `CLAUDE.md` or `docs/plan.md`.
//!
//! The guards use `std` only. Each guard is a pure function over text, so a
//! self-test can feed it a bad sample and see it fail. A guard that cannot
//! fail does not guard (the `the_drift_check_sees_changes` pattern).
//!
//! Most guards read code, not comments: a small lexer marks comments and
//! string literals. Items under `#[cfg(test)]` are test code. The rules for
//! `src/` do not apply to them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "guard tests: a failure is a test failure"
)]

use std::path::{Path, PathBuf};

use autumn_jira::writes::{WRITES, WriteKind};

mod check {
    //! The checks. Each returns one line for each violation.

    use std::collections::BTreeMap;
    use std::path::Path;

    pub fn base_url_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn legacy_search_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn instrument_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn mac_eq_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn fake_dependency_violations(_manifest: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn ctx_now_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn exactly_once_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn unwrap_todo_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn allow_reason_violations(_path: &str, _text: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn ci_violations(_ci: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn c_build_violations(_lock: &str) -> Vec<String> {
        Vec::new()
    }
    pub fn lint_tables(_manifest: &str) -> BTreeMap<String, BTreeMap<String, String>> {
        BTreeMap::new()
    }
    pub fn writes_rows(_readme: &str) -> Option<Vec<(String, String, String)>> {
        None
    }
    pub fn evidence_violations(_md: &str, _root: &Path) -> Vec<String> {
        Vec::new()
    }
    pub fn adr_violations(_names: &[String], _referenced: &[u32]) -> Vec<String> {
        Vec::new()
    }
    pub fn adr_references(_text: &str) -> Vec<u32> {
        Vec::new()
    }
    pub fn code_only(_text: &str) -> String {
        String::new()
    }
}

/// The five library crates of the workspace.
const CRATES: [&str; 5] = [
    "autumn-jira",
    "autumn-jira-fake",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
];

/// The crates that call Jira. The fake is the server, so it is not one.
const CLIENT_CRATES: [&str; 4] = [
    "autumn-jira",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The path relative to the repository root, with `/`.
fn rel(path: &Path) -> String {
    let root = root().canonicalize().unwrap();
    let path = path.canonicalize().unwrap();
    path.strip_prefix(&root)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The files under `dir` with one of the extensions, in a stable order.
/// Build and VCS folders are skipped.
fn files_under(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | ".git" | "node_modules") {
                out.extend(files_under(&path, exts));
            }
        } else if path
            .extension()
            .is_some_and(|e| exts.iter().any(|x| e == *x))
        {
            out.push(path);
        }
    }
    out
}

fn src_files(krates: &[&str]) -> Vec<PathBuf> {
    krates
        .iter()
        .flat_map(|k| files_under(&root().join("crates").join(k).join("src"), &["rs"]))
        .collect()
}

/// Runs `check` on each file and fails with all violations.
fn assert_files(name: &str, files: &[PathBuf], check: fn(&str, &str) -> Vec<String>) {
    assert!(!files.is_empty(), "{name}: no files to check");
    let found: Vec<String> = files
        .iter()
        .flat_map(|f| check(&rel(f), &std::fs::read_to_string(f).unwrap()))
        .collect();
    assert!(found.is_empty(), "{name}:\n{}", found.join("\n"));
}

// --- The lexer -----------------------------------------------------------

#[test]
fn the_lexer_drops_comments_and_test_items() {
    let text = "let a = \"// not a comment\"; // a comment\n\
                let b = r#\"/* raw */\"#; /* block /* nested */ */ let c = 'x';\n\
                fn f<'a>(x: &'a str) {}\n\
                #[cfg(test)]\nmod tests { fn t() { let s = \"}\"; x.unwrap(); } }\n\
                fn g() {}\n";
    let code = check::code_only(text);
    assert!(code.contains("\"// not a comment\""), "{code}");
    assert!(!code.contains("a comment\n"), "{code}");
    assert!(code.contains("r#\"/* raw */\"#"), "{code}");
    assert!(!code.contains("nested"), "{code}");
    assert!(code.contains("let c = 'x';"), "{code}");
    assert!(code.contains("fn f<'a>(x: &'a str)"), "{code}");
    assert!(!code.contains("unwrap"), "{code}");
    assert!(code.contains("fn g() {}"), "{code}");
    // The line count stays the same, so a violation names the right line.
    assert_eq!(code.lines().count(), text.lines().count());
}

// --- AC2: one base URL builder -------------------------------------------

#[test]
fn one_base_url_builder() {
    assert_files(
        "one_base_url_builder",
        &src_files(&CLIENT_CRATES),
        check::base_url_violations,
    );
}

#[test]
fn one_base_url_builder_sees_a_second_builder() {
    let bad = "fn url(site: &str) -> String { format!(\"https://{site}.atlassian.net/rest\") }";
    assert_eq!(
        check::base_url_violations("crates/autumn-plugin-jira/src/x.rs", bad).len(),
        1
    );
    let gateway = "const G: &str = \"https://api.atlassian.com/ex/jira/\";";
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/client.rs", gateway).len(),
        1
    );
    // The builder itself, the san-mateo path prefix and comments pass.
    assert!(check::base_url_violations("crates/autumn-jira/src/site.rs", bad).is_empty());
    let prefix = "fn p(id: &str) -> String { format!(\"/ex/jira/{id}\") }";
    assert!(
        check::base_url_violations("crates/autumn-jira/src/backend/san_mateo.rs", prefix)
            .is_empty()
    );
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/client.rs", prefix).len(),
        1
    );
    let comment = "/// Sends to `https://acme.atlassian.net`.\nfn f() {}";
    assert!(check::base_url_violations("crates/autumn-jira/src/x.rs", comment).is_empty());
}

// --- AC7: no legacy search path ------------------------------------------

#[test]
fn no_legacy_search_path() {
    assert_files(
        "no_legacy_search_path",
        &src_files(&CLIENT_CRATES),
        check::legacy_search_violations,
    );
}

#[test]
fn no_legacy_search_path_sees_the_old_path() {
    let p = "crates/autumn-jira/src/client.rs";
    for bad in [
        "let p = format!(\"{API}/search\");",
        "let p = \"/rest/api/3/search?jql=x\";",
        "let p = format!(\"{API}/search/\");",
        "let p = \"/search\";",
    ] {
        assert_eq!(check::legacy_search_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "let p = format!(\"{API}/search/jql\");",
        "let p = format!(\"{API}/search/approximate-count\");",
        "let p = format!(\"{API}/project/search\");",
        "/// The old `GET /rest/api/3/search` is gone.\nfn f() {}",
    ] {
        assert!(check::legacy_search_violations(p, good).is_empty(), "{good}");
    }
}

// --- AC21: no #[instrument] without skip_all -----------------------------

#[test]
fn instrument_skips_all() {
    assert_files(
        "instrument_skips_all",
        &src_files(&CRATES),
        check::instrument_violations,
    );
}

#[test]
fn instrument_skips_all_sees_a_skip_list() {
    let p = "crates/autumn-jira/src/x.rs";
    let bad = "#[tracing::instrument(skip(self))]\nasync fn f(&self, body: &str) {}";
    assert_eq!(check::instrument_violations(p, bad).len(), 1);
    let bare = "#[instrument]\nfn f(token: &str) {}";
    assert_eq!(check::instrument_violations(p, bare).len(), 1);
    let good = "#[instrument(\n    skip_all,\n    fields(op = %op)\n)]\nfn f(op: u8) {}";
    assert!(check::instrument_violations(p, good).is_empty());
    let comment = "// #[instrument] is not used here.\nfn f() {}";
    assert!(check::instrument_violations(p, comment).is_empty());
}

// --- AC19: no == on MAC bytes --------------------------------------------

#[test]
fn no_eq_on_mac_bytes() {
    assert_files(
        "no_eq_on_mac_bytes",
        &src_files(&CRATES),
        check::mac_eq_violations,
    );
}

#[test]
fn no_eq_on_mac_bytes_sees_an_eq() {
    let p = "crates/autumn-jira/src/webhook.rs";
    let bad = "use hmac::Mac;\n\
               fn v(mac: HmacSha256, expected: &[u8]) -> bool {\n\
               \x20   mac.finalize().into_bytes().as_slice() == expected\n}";
    assert_eq!(check::mac_eq_violations(p, bad).len(), 1);
    let tag = "use hmac::Mac;\nfn v(tag: &str, want: &str) -> bool { tag != want }";
    assert_eq!(check::mac_eq_violations(p, tag).len(), 1);
    let good = "use hmac::Mac;\n\
                // Never write `mac == expected`.\n\
                fn v(mac: HmacSha256, expected: &[u8]) -> bool {\n\
                \x20   bytes.len() != TAG_LEN || mac.verify_slice(expected).is_ok()\n}";
    assert!(check::mac_eq_violations(p, good).is_empty());
}

// --- AC22: the fake does not depend on the client ------------------------

#[test]
fn fake_does_not_depend_on_the_client() {
    let found = check::fake_dependency_violations(&read("crates/autumn-jira-fake/Cargo.toml"));
    assert!(found.is_empty(), "{}", found.join("\n"));
    // The lock agrees: the fake package lists no `autumn-jira` dependency.
    let lock = read("Cargo.lock");
    let block = lock
        .split("[[package]]")
        .find(|b| b.lines().any(|l| l.trim() == "name = \"autumn-jira-fake\""))
        .expect("the lock has autumn-jira-fake");
    assert!(
        !block.lines().any(|l| l.trim().trim_end_matches(',') == "\"autumn-jira\""),
        "Cargo.lock: autumn-jira-fake depends on autumn-jira"
    );
}

#[test]
fn fake_does_not_depend_on_the_client_sees_a_dependency() {
    let good = "[package]\nname = \"autumn-jira-fake\"\n\n[dependencies]\n\
                san-mateo.workspace = true\n\n[dev-dependencies]\n\
                autumn-jira-fake = { path = \".\" }\n";
    assert!(check::fake_dependency_violations(good).is_empty());
    for bad in [
        "[dependencies]\nautumn-jira.workspace = true\n",
        "[dev-dependencies]\nautumn-jira = { path = \"../autumn-jira\" }\n",
        "[dependencies]\nclient = { package = \"autumn-jira\", version = \"0.1\" }\n",
        "[dependencies.autumn-jira]\nworkspace = true\n",
        "[target.'cfg(unix)'.dependencies]\n\"autumn-jira\" = \"0.1\"\n",
    ] {
        assert_eq!(check::fake_dependency_violations(bad).len(), 1, "{bad}");
    }
}

// --- AC33: no ctx.now() in harvest code ----------------------------------

#[test]
fn no_ctx_now_in_harvest() {
    let mut files = files_under(&root().join("crates/autumn-jira-harvest"), &["rs"]);
    files.extend(files_under(&root().join("examples/ecosystem/src"), &["rs"]));
    assert_files("no_ctx_now_in_harvest", &files, check::ctx_now_violations);
}

#[test]
fn no_ctx_now_in_harvest_sees_ctx_now() {
    let p = "crates/autumn-jira-harvest/src/wait.rs";
    let bad = "let deadline = ctx\n    .now()\n    + timeout;";
    assert_eq!(check::ctx_now_violations(p, bad).len(), 1);
    let typed = "let t = WorkflowContext::now(ctx);";
    assert_eq!(check::ctx_now_violations(p, typed).len(), 1);
    let good = "// Never call ctx.now() here.\nlet t = ctx.system_now();\nlet s = \"ctx.now()\";";
    assert!(check::ctx_now_violations(p, good).is_empty());
}

// --- AC30: no exactly-once claim -----------------------------------------

#[test]
fn no_exactly_once_claim() {
    let mut files = files_under(&root(), &["md", "rs"]);
    files.retain(|f| !rel(f).starts_with("target/"));
    assert_files("no_exactly_once_claim", &files, check::exactly_once_violations);
}

#[test]
fn no_exactly_once_claim_sees_a_claim() {
    // The phrase is built at run time, so this file does not hold it.
    let phrase = ["exactly", "once"].join(" ");
    let p = "README.md";
    for bad in [
        format!("The route delivers each event {phrase}."),
        format!("It gives {}-processing.", phrase.replace(' ', "-")),
        format!("/// Each sink runs\n/// {phrase}.\nfn f() {{}}"),
        format!("Exactly once for each delivery. Not a typo, {phrase} again."),
    ] {
        assert!(
            !check::exactly_once_violations(p, &bad).is_empty(),
            "{bad}"
        );
    }
    for good in [
        format!("Webhooks are at least once. Do not claim {phrase}."),
        format!("/// Delivery is not\n/// {phrase}: a sink must be idempotent."),
        format!("No \"{phrase}\" here."),
        format!("The docs say \"best effort; no {phrase}\"."),
        format!("- \"{}\". Webhooks are at least once.", capitalize(&phrase)),
    ] {
        assert!(
            check::exactly_once_violations(p, &good).is_empty(),
            "{good}"
        );
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

// --- AC38: no unwrap, expect, todo!, TODO in src/ ------------------------

#[test]
fn no_unwrap_or_todo_in_src() {
    assert_files(
        "no_unwrap_or_todo_in_src",
        &src_files(&CRATES),
        check::unwrap_todo_violations,
    );
}

#[test]
fn no_unwrap_or_todo_in_src_sees_each_kind() {
    let p = "crates/autumn-jira/src/x.rs";
    for bad in [
        "fn f(x: Option<u8>) -> u8 { x.unwrap() }",
        "fn f(x: Option<u8>) -> u8 {\n    x\n        .expect(\"a value\")\n}",
        "fn f() { todo!() }",
        "fn f() { unimplemented!(\"later\") }",
        "// TODO: make this fast.\nfn f() {}",
        "/* FIXME */ fn f() {}",
    ] {
        assert_eq!(check::unwrap_todo_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "fn f(x: Option<u8>) -> u8 { x.unwrap_or(1) }",
        "fn f(x: Result<u8, u8>) { let _ = x.expect_err; }",
        "/// ```\n/// let x = Some(1).unwrap();\n/// ```\nfn f() {}",
        "let s = \"do not unwrap() here, TODO\";",
        "pub(crate) const TODO: u8 = 1;",
        "#[cfg(test)]\nmod tests {\n    // TODO in a test is a test matter.\n    fn t() { Some(1).unwrap(); }\n}",
    ] {
        assert!(check::unwrap_todo_violations(p, good).is_empty(), "{good}");
    }
}

// --- AC38: each allow has a reason ---------------------------------------

#[test]
fn every_allow_has_a_reason() {
    let mut files = files_under(&root().join("crates"), &["rs"]);
    files.extend(files_under(&root().join("examples"), &["rs"]));
    assert_files("every_allow_has_a_reason", &files, check::allow_reason_violations);
}

#[test]
fn every_allow_has_a_reason_sees_a_bare_allow() {
    let p = "crates/autumn-jira/src/x.rs";
    for bad in [
        "#[allow(dead_code)] // reason: a comment is not a reason\nfn f() {}",
        "#![allow(\n    clippy::unwrap_used,\n    clippy::expect_used\n)]",
        "#[expect(clippy::cast_possible_truncation)]\nfn f() {}",
        "#[cfg_attr(test, allow(dead_code))]\nfn f() {}",
    ] {
        assert_eq!(check::allow_reason_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "#[allow(\n    clippy::too_many_lines,\n    reason = \"one table\"\n)]\nfn f() {}",
        "#![allow(clippy::print_stdout, reason = \"the example prints its summary\")]",
        "// #[allow(dead_code)] in a comment is not an attribute.\nfn f() {}",
        "let s = \"#[allow(dead_code)]\";",
    ] {
        assert!(check::allow_reason_violations(p, good).is_empty(), "{good}");
    }
}

// --- AC38: no continue-on-error, no || after a checked command -----------

#[test]
fn no_continue_on_error() {
    let found = check::ci_violations(&read(".github/workflows/ci.yml"));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn no_continue_on_error_sees_hidden_failures() {
    for bad in [
        "    steps:\n      - run: cargo test\n        continue-on-error: true\n",
        "    steps:\n      - run: cargo test || true\n",
        "    steps:\n      - name: x\n        run: |\n          cargo build\n          cargo test || echo failed\n",
        "    steps:\n      - name: x\n        run: >-\n          cargo llvm-cov\n          --json || :\n",
    ] {
        assert_eq!(check::ci_violations(bad).len(), 1, "{bad}");
    }
    let good = "    steps:\n\
                \x20     - name: x\n\
                \x20       if: github.event_name == 'schedule' || github.event_name == 'push'\n\
                \x20       run: |\n\
                \x20         # A comment with || in it.\n\
                \x20         ra=0; wait \"$a\" || ra=$?\n\
                \x20         test \"$ra\" -eq 0\n\
                \x20     - run: cargo test\n";
    let found = check::ci_violations(good);
    assert!(found.is_empty(), "{found:?}");
}

// --- AC39: the root lock is free of C builds -----------------------------

#[test]
fn root_lock_is_free_of_c_builds() {
    let found = check::c_build_violations(&read("Cargo.lock"));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn root_lock_is_free_of_c_builds_sees_one() {
    let lock = "[[package]]\nname = \"libsqlite3-sys\"\nversion = \"0.30.1\"\n\n\
                [[package]]\nname = \"harvest-camel\"\nversion = \"0.2.0\"\n\n\
                [[package]]\nname = \"camel-core\"\nversion = \"0.57.0\"\n";
    assert_eq!(check::c_build_violations(lock).len(), 2);
    assert!(check::c_build_violations("[[package]]\nname = \"pq-sys\"\n").is_empty());
}

// --- AC39: the ecosystem lints equal the root lints ----------------------

#[test]
fn ecosystem_lints_equal_the_root() {
    let path = "examples/ecosystem/Cargo.toml";
    assert!(
        root().join(path).exists(),
        "{path} is missing: the ecosystem example copies the root lints"
    );
    let root_lints = check::lint_tables(&read("Cargo.toml"));
    assert!(
        root_lints.contains_key("lints.rust") && root_lints.contains_key("lints.clippy"),
        "the root has no [workspace.lints.rust] and [workspace.lints.clippy]"
    );
    assert_eq!(
        check::lint_tables(&read(path)),
        root_lints,
        "{path}: copy [workspace.lints] of the root Cargo.toml"
    );
}

#[test]
fn the_lint_compare_sees_a_change() {
    let root = "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n\
                [workspace.lints.clippy]\n# A comment.\npedantic = { level = \"warn\", priority = -1 }\n\
                unwrap_used = \"deny\"\n\n[profile.dev]\ndebug = 1\n";
    let same = "[package]\nname = \"ecosystem\"\n\n[lints.clippy]\nunwrap_used = \"deny\"\n\
                pedantic = {level = \"warn\",   priority = -1}\n\n[lints.rust]\n\
                unsafe_code = \"forbid\" # forbid\n";
    let tables = check::lint_tables(root);
    assert_eq!(tables.len(), 2, "{tables:?}");
    assert_eq!(check::lint_tables(same), tables);
    let missing = same.replace("unwrap_used = \"deny\"\n", "");
    assert_ne!(check::lint_tables(&missing), tables);
    let weaker = same.replace("\"forbid\"", "\"deny\"");
    assert_ne!(check::lint_tables(&weaker), tables);
}

// --- AC40: the README writes table equals WRITES -------------------------

/// The README text of a [`WriteKind`]. A new kind breaks this match.
const fn kind_text(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Label => "label",
        WriteKind::IssueProperty => "issue property",
        WriteKind::CommentText => "comment text",
    }
}

fn expected_writes() -> Vec<(String, String, String)> {
    WRITES
        .iter()
        .map(|w| {
            (
                w.what.to_owned(),
                kind_text(w.kind).to_owned(),
                w.when.to_owned(),
            )
        })
        .collect()
}

#[test]
fn readme_writes_table_equals_writes() {
    let rows = check::writes_rows(&read("README.md"))
        .expect("README.md has no section \"What the plugin writes to your Jira\"");
    assert_eq!(rows, expected_writes(), "README.md: one row for each WRITES entry");
}

#[test]
fn the_writes_compare_sees_a_change() {
    let table: String = expected_writes()
        .iter()
        .map(|(what, kind, when)| format!("| `{what}` | {kind} | `{when}` | why |\n"))
        .collect();
    let readme = format!(
        "# x\n\n## What the plugin writes to your Jira\n\nText.\n\n\
         | What | Kind | When | Why |\n|---|---|---|---|\n{table}\n## Next\n\n| a | b | c |\n"
    );
    assert_eq!(check::writes_rows(&readme), Some(expected_writes()));
    let fewer = readme.replacen("| `autumn-tracked`", "| `autumn-other`", 1);
    assert_ne!(check::writes_rows(&fewer), Some(expected_writes()));
    assert_eq!(check::writes_rows("# x\n\n## Other\n"), None);
}

// --- AC40: the evidence names real tests ---------------------------------

#[test]
fn evidence_names_real_tests() {
    let path = "docs/ac-evidence.md";
    assert!(root().join(path).exists(), "{path} is missing");
    let found = check::evidence_violations(&read(path), &root());
    assert!(found.is_empty(), "{path}:\n{}", found.join("\n"));
}

#[test]
fn the_evidence_check_sees_a_missing_test() {
    let md = "| AC | Tests | Status |\n|---|---|---|\n\
              | AC1 | `core/t/guards.rs::{one_base_url_builder, no_such_guard}`; u: `site::{base_urls}` | Done |\n\
              | AC2 | u: `ids::{strict_rules_table, no_such_unit_test}`; CI job `verus` | Done |\n\
              | AC3 | suite cases `{create_get_edit_delete, no_such_case}` | Done |\n\
              | AC4 | ex: `embedded::{the_example_runs_and_checks, no_such_example_test}` | Done |\n\
              | AC5 | `core/t/no_such_file.rs::x`; `idempotency_key_derive_is_stable`; `no_such_fn` | Done |\n";
    let found = check::evidence_violations(md, &root());
    let missing: Vec<&str> = [
        "no_such_guard",
        "no_such_unit_test",
        "no_such_case",
        "no_such_example_test",
        "no_such_file.rs",
        "no_such_fn",
    ]
    .into_iter()
    .filter(|name| !found.iter().any(|f| f.contains(name)))
    .collect();
    assert!(missing.is_empty(), "not seen: {missing:?}; found: {found:?}");
    assert_eq!(found.len(), 6, "{found:?}");
}

// --- AC40: the ADR files exist -------------------------------------------

fn adr_names() -> Vec<String> {
    files_under(&root().join("docs/adr"), &["md"])
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn adr_files_exist() {
    let mut referenced = Vec::new();
    for file in files_under(&root(), &["md", "rs"]) {
        if !rel(&file).starts_with("target/") {
            referenced.extend(check::adr_references(&std::fs::read_to_string(file).unwrap()));
        }
    }
    let found = check::adr_violations(&adr_names(), &referenced);
    assert!(found.is_empty(), "{}", found.join("\n"));
    for name in adr_names() {
        let text = read(&format!("docs/adr/{name}"));
        assert!(text.starts_with("# "), "docs/adr/{name}: the first line is a title");
    }
}

#[test]
fn adr_files_exist_sees_a_gap() {
    let names: Vec<String> = (1..=8)
        .filter(|n| *n != 5)
        .map(|n| format!("{n:04}-x.md"))
        .collect();
    assert_eq!(check::adr_violations(&names, &[]).len(), 1);
    let mut all: Vec<String> = (1..=8).map(|n| format!("{n:04}-x.md")).collect();
    assert!(check::adr_violations(&all, &[3, 8]).is_empty());
    assert_eq!(check::adr_violations(&all, &[9]).len(), 1);
    all.push("0002-y.md".to_owned());
    assert_eq!(check::adr_violations(&all, &[]).len(), 1);
    assert_eq!(
        check::adr_references("See ADR 0003 and ADR 0010. adr/0004-x.md"),
        [3, 10, 4]
    );
}
