//! The runnable examples stay wired (EX8). For each example, this test
//! checks the Cargo target, a self-check and a failure test in the source,
//! the CI commands and the README row. See ADR 0008.
//!
//! The test reads files of the repository. It copies the san-mateo
//! `examples_wired` pattern. The checks are pure functions over text, and
//! each has a self-test with a bad sample:
//!
//! - The source checks read code only. A comment or a string that names
//!   `RUN_TIMEOUT`, `fn check(` or `expect_err(` does not count.
//! - The CI checks read the `run:` scripts of the steps only. A step `name:`
//!   or a step with an `if:` does not count as a run.
//! - The README command of an example has each env var that it needs.
//! - A README snippet that says "From the <name> example" holds only lines
//!   of that example.
//! - `CLAUDE.md` and the README give the same commands as CI for the
//!   separate workspace.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "a guard test: a failure is a test failure"
)]

use std::path::{Path, PathBuf};

struct Example {
    name: &'static str,
    /// The Cargo manifest that declares the target.
    manifest: &'static str,
    /// The target header: `[[example]]` or `[[bin]]`.
    target: &'static str,
    /// The source files, or folders of source files.
    sources: &'static [&'static str],
    /// Commands that the CI workflow must run.
    ci: &'static [&'static str],
    /// The command that the README must show.
    readme: &'static str,
    /// The env vars that the README command line must set, because the
    /// example does not start without them.
    readme_env: &'static [&'static str],
    /// A server runs until it gets a signal, so its `main` has no time
    /// bound. Its `RUN_TIMEOUT` can be in the tests only. The CI script
    /// that boots it gives the time bound.
    server: bool,
}

const EXAMPLES: &[Example] = &[
    Example {
        name: "embedded",
        manifest: "crates/autumn-jira/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira/examples/embedded.rs"],
        ci: &["cargo run --locked -p autumn-jira --example embedded"],
        readme: "cargo run --locked -p autumn-jira --example embedded",
        readme_env: &[],
        server: false,
    },
    Example {
        name: "autumn_app",
        manifest: "crates/autumn-plugin-jira/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-plugin-jira/examples/autumn_app"],
        // The smoke script builds the example and boots it in dev and prod.
        ci: &["bash scripts/smoke-autumn-example.sh"],
        readme: "cargo run --locked -p autumn-plugin-jira --features testing --example autumn_app",
        // autumn reads `autumn.toml` from `AUTUMN_MANIFEST_DIR`. Without
        // it, the app has no webhook endpoint and the boot fails.
        readme_env: &[
            "AUTUMN_MANIFEST_DIR=crates/autumn-plugin-jira/examples/autumn_app",
            "JIRA_WEBHOOK_SECRET=",
        ],
        server: true,
    },
    Example {
        name: "harvest_workflow",
        manifest: "crates/autumn-jira-harvest/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira-harvest/examples/harvest_workflow.rs"],
        ci: &["cargo run --locked -p autumn-jira-harvest --example harvest_workflow"],
        readme: "cargo run --locked -p autumn-jira-harvest --example harvest_workflow",
        readme_env: &["DATABASE_URL="],
        server: false,
    },
    Example {
        name: "camel_route",
        manifest: "crates/autumn-jira-camel/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira-camel/examples/camel_route.rs"],
        ci: &["cargo run --locked -p autumn-jira-camel --example camel_route"],
        readme: "cargo run --locked -p autumn-jira-camel --example camel_route",
        readme_env: &[],
        server: false,
    },
    Example {
        name: "ecosystem",
        manifest: "examples/ecosystem/Cargo.toml",
        target: "[[bin]]",
        sources: &["examples/ecosystem/src"],
        // The root jobs do not see this workspace, so CI must run its tests.
        ci: &[
            "cargo test --locked --manifest-path examples/ecosystem/Cargo.toml",
            "cargo run --locked --manifest-path examples/ecosystem/Cargo.toml",
        ],
        readme: "cargo run --locked --manifest-path examples/ecosystem/Cargo.toml",
        readme_env: &["DATABASE_URL="],
        server: false,
    },
];

/// Files that users copy. They must not turn on the dev-only proxy flags.
const COPIED_FILES: &[&str] = &[
    "crates/autumn-plugin-jira/examples/autumn_app/autumn.toml",
    "crates/autumn-plugin-jira/examples/autumn_app/autumn-prod.toml",
    "scripts/smoke-autumn-example.sh",
];

/// The crates of this repository. The ecosystem example uses each by path.
const CRATES: [&str; 5] = [
    "autumn-jira",
    "autumn-jira-fake",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
];

/// The CI job of the separate workspace. `CLAUDE.md` and the README must
/// give its commands.
const ECOSYSTEM_JOB: &str = "example-ecosystem";

/// The documents that give the contributor commands.
const COMMAND_DOCS: [&str; 2] = ["CLAUDE.md", "README.md"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The files of a folder, with no build folder.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                out.extend(files_under(&path));
            }
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// The text of all source files of an example.
fn source_text(ex: &Example) -> String {
    let mut text = String::new();
    for source in ex.sources {
        let path = root().join(source);
        let files = if path.is_dir() {
            files_under(&path)
        } else {
            vec![path]
        };
        for file in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        {
            text.push_str(&std::fs::read_to_string(file).unwrap());
            text.push('\n');
        }
    }
    text
}

/// The lines of the target section `name` in a manifest, up to the next
/// section header.
fn target_section(manifest: &str, header: &str, name: &str) -> Option<String> {
    let lines: Vec<&str> = manifest.lines().collect();
    let name_line = format!("name = \"{name}\"");
    lines.iter().enumerate().find_map(|(i, line)| {
        if line.trim() != header || lines.get(i + 1).map(|l| l.trim()) != Some(&name_line) {
            return None;
        }
        let end = lines[i + 1..]
            .iter()
            .position(|l| l.starts_with('['))
            .map_or(lines.len(), |p| i + 1 + p);
        Some(lines[i..end].join("\n"))
    })
}

mod wired {
    //! The checks. Each returns one line for each problem.

    // --- Rust source -----------------------------------------------------

    const fn is_ident(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    /// The code of a Rust source: comments and the contents of string and
    /// char literals become spaces. New lines stay. It knows line and
    /// block comments, escapes, raw strings, and char literals as against
    /// lifetimes.
    pub fn code_of(text: &str) -> String {
        scrub(text, false)
    }

    /// The source with the comments as spaces. With `keep_strings`, the
    /// string and char literals stay. Each char keeps its place, so the
    /// output lines up char for char with [`code_of`].
    fn scrub(text: &str, keep_strings: bool) -> String {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
        let mut out: Vec<char> = chars.clone();
        let blank = |out: &mut Vec<char>, from: usize, to: usize| {
            for c in &mut out[from..to.min(n)] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        };
        let mut i = 0;
        while i < n {
            let c = chars[i];
            let prev = if i == 0 { ' ' } else { chars[i - 1] };
            let end = if c == '/' && at(i + 1) == '/' {
                let end = (i..n).find(|&j| chars[j] == '\n').unwrap_or(n);
                blank(&mut out, i, end);
                end
            } else if c == '/' && at(i + 1) == '*' {
                let mut depth = 0;
                let mut j = i;
                while j < n {
                    if chars[j] == '/' && at(j + 1) == '*' {
                        depth += 1;
                        j += 2;
                    } else if chars[j] == '*' && at(j + 1) == '/' {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                blank(&mut out, i, j);
                j.min(n)
            } else if c == 'r' && !is_ident(prev) && matches!(at(i + 1), '"' | '#') {
                let hashes = (i + 1..n).take_while(|&j| chars[j] == '#').count();
                let open = i + 1 + hashes;
                if at(open) == '"' {
                    let mut j = open + 1;
                    while j < n && !(chars[j] == '"' && (1..=hashes).all(|k| at(j + k) == '#')) {
                        j += 1;
                    }
                    if !keep_strings {
                        blank(&mut out, open + 1, j);
                    }
                    (j + 1 + hashes).min(n)
                } else {
                    i + 1
                }
            } else if c == '"' {
                let mut j = i + 1;
                while j < n && chars[j] != '"' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                if !keep_strings {
                    blank(&mut out, i + 1, j);
                }
                (j + 1).min(n)
            } else if c == '\'' && at(i + 1) == '\\' {
                let j = (i + 3..n).find(|&j| chars[j] == '\'').unwrap_or(n);
                if !keep_strings {
                    blank(&mut out, i + 1, j);
                }
                (j + 1).min(n)
            } else if c == '\'' && at(i + 2) == '\'' {
                if !keep_strings {
                    blank(&mut out, i + 1, i + 2);
                }
                i + 3
            } else {
                i + 1
            };
            i = end;
        }
        out.into_iter().collect()
    }

    /// The code with each item under `#[cfg(test)]` removed: from the
    /// attribute to the `;` or the matching `}` of the item. The input is
    /// the output of [`code_of`].
    pub fn without_test_items(code: &str) -> String {
        let chars: Vec<char> = code.chars().collect();
        keep_outside(&chars, &test_item_ranges(&chars))
    }

    /// The chars of `chars` outside the ranges.
    fn keep_outside(chars: &[char], ranges: &[std::ops::Range<usize>]) -> String {
        chars
            .iter()
            .enumerate()
            .filter(|(i, _)| !ranges.iter().any(|r| r.contains(i)))
            .map(|(_, c)| *c)
            .collect()
    }

    /// The char ranges of the items under `#[cfg(test)]`: from the
    /// attribute to the `;` or the matching `}` of the item. The input is
    /// the output of [`code_of`], so no brace in a string or a comment
    /// counts.
    fn test_item_ranges(chars: &[char]) -> Vec<std::ops::Range<usize>> {
        const ATTR: [char; 12] = ['#', '[', 'c', 'f', 'g', '(', 't', 'e', 's', 't', ')', ']'];
        let n = chars.len();
        let mut out = Vec::new();
        let mut i = 0;
        while i + ATTR.len() <= n {
            if chars[i..i + ATTR.len()] != ATTR {
                i += 1;
                continue;
            }
            let start = i;
            let mut j = i + ATTR.len();
            let mut depth = 0_i32;
            let mut end = n;
            while j < n {
                match chars[j] {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    ';' if depth == 0 => {
                        end = j + 1;
                        break;
                    }
                    '{' if depth == 0 => {
                        let mut braces = 0_i32;
                        for (k, d) in chars.iter().enumerate().skip(j) {
                            match d {
                                '{' => braces += 1,
                                '}' => braces -= 1,
                                _ => {}
                            }
                            if braces == 0 {
                                end = k + 1;
                                break;
                            }
                        }
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
            out.push(start..end);
            i = end;
        }
        out
    }

    /// The code lines of an example for a snippet check: no comment and
    /// no `#[cfg(test)]` item, but with the strings (R2E-tests-guards-9).
    fn snippet_lines(text: &str) -> Vec<String> {
        let code: Vec<char> = code_of(text).chars().collect();
        let kept: Vec<char> = scrub(text, true).chars().collect();
        keep_outside(&kept, &test_item_ranges(&code))
            .lines()
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty())
            .collect()
    }

    /// The source rules of one example: a time bound, test code, a
    /// self-check and a failure test, in code and not in comments.
    pub fn source_violations(name: &str, source: &str, server: bool) -> Vec<String> {
        let code = code_of(source);
        let main = without_test_items(&code);
        let mut out = Vec::new();
        for must in ["#[cfg(test)]", "fn check(", "expect_err("] {
            if !code.contains(must) {
                out.push(format!("{name}: the source needs `{must}`"));
            }
        }
        let timeout = if server { &code } else { &main };
        if !timeout.contains("const RUN_TIMEOUT") {
            let place = if server { "" } else { " outside the tests" };
            out.push(format!(
                "{name}: the source needs `const RUN_TIMEOUT`{place}"
            ));
        }
        if source.contains("full_migrations_sql") {
            out.push(format!("{name}: use `migrate::apply`"));
        }
        out
    }

    // --- The CI workflow -------------------------------------------------

    /// One step of a workflow job.
    pub struct Step {
        /// The job of the step.
        pub job: String,
        /// The `if:` value, when the step has one.
        pub cond: Option<String>,
        /// The lines of the `run:` script.
        pub run: Vec<String>,
    }

    fn indent(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }

    /// The steps of a workflow. A `$MANIFEST` takes the value of the
    /// `MANIFEST:` env entry, so a command reads as a user types it.
    /// Comment lines do not count.
    pub fn steps(ci: &str) -> Vec<Step> {
        let manifest = ci
            .lines()
            .find_map(|l| l.trim().strip_prefix("MANIFEST:"))
            .map(|v| v.trim().trim_matches('"').to_owned());
        let lines: Vec<String> = ci
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .map(|l| {
                manifest.as_ref().map_or_else(
                    || l.to_owned(),
                    |m| l.replace("\"$MANIFEST\"", m).replace("$MANIFEST", m),
                )
            })
            .collect();
        let mut out = Vec::new();
        let mut job = String::new();
        let mut in_jobs = false;
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if indent(line) == 0 && !t.is_empty() {
                in_jobs = t == "jobs:";
            } else if in_jobs && indent(line) == 2 && t.ends_with(':') {
                t.trim_end_matches(':').clone_into(&mut job);
            }
            let Some(first) = t.strip_prefix("- ") else {
                continue;
            };
            let dash = indent(line);
            let keys_at = dash + 2;
            let end = (i + 1..lines.len())
                .find(|&j| !lines[j].trim().is_empty() && indent(&lines[j]) <= dash)
                .unwrap_or(lines.len());
            let mut step = Step {
                job: job.clone(),
                cond: None,
                run: Vec::new(),
            };
            let mut j = i;
            while j < end {
                let text = if j == i { first } else { lines[j].trim() };
                let is_key = j == i || indent(&lines[j]) == keys_at;
                j += 1;
                if !is_key {
                    continue;
                }
                if let Some(value) = text.strip_prefix("if:") {
                    step.cond = Some(value.trim().to_owned());
                } else if let Some(value) = text.strip_prefix("run:") {
                    let value = value.trim();
                    if value.starts_with('|') || value.starts_with('>') {
                        while j < end && (lines[j].trim().is_empty() || indent(&lines[j]) > keys_at)
                        {
                            step.run.push(lines[j].trim().to_owned());
                            j += 1;
                        }
                    } else {
                        step.run.push(value.to_owned());
                    }
                }
            }
            out.push(step);
        }
        out
    }

    /// The CI rules of one example: each command is in the `run:` script
    /// of a step with no `if:`, and no `||` hides its failure.
    pub fn ci_violations(name: &str, commands: &[&str], steps: &[Step]) -> Vec<String> {
        let mut out = Vec::new();
        for command in commands {
            let lines: Vec<&String> = steps
                .iter()
                .filter(|s| s.cond.is_none())
                .flat_map(|s| s.run.iter())
                .filter(|l| l.contains(command))
                .collect();
            if lines.is_empty() {
                out.push(format!("{name}: CI does not run `{command}`"));
            }
            if lines.iter().any(|l| l.contains("||")) {
                out.push(format!("{name}: CI hides a failure of `{command}`"));
            }
        }
        out
    }

    // --- The docs --------------------------------------------------------

    /// The README rules of one example: a table row, and a line with the
    /// command and each env var that it needs.
    pub fn readme_violations(name: &str, command: &str, env: &[&str], readme: &str) -> Vec<String> {
        let mut out = Vec::new();
        // A shell line that ends in `\` goes on in the next line.
        let joined = readme.replace("\\\n", " ");
        let lines: Vec<&str> = joined.lines().filter(|l| l.contains(command)).collect();
        if lines.is_empty() {
            out.push(format!("{name}: the README does not show `{command}`"));
        }
        if !readme.contains(&format!("| [`{name}`](")) {
            out.push(format!("{name}: the README Examples table has no row"));
        }
        for var in env {
            if !lines.is_empty() && !lines.iter().all(|l| l.contains(var)) {
                out.push(format!(
                    "{name}: a README line with `{command}` does not set `{var}`"
                ));
            }
        }
        out
    }

    /// The fenced blocks of a Markdown text: (info string, lines).
    fn fenced_blocks(md: &str) -> Vec<(String, Vec<String>)> {
        let mut out = Vec::new();
        let mut current: Option<(String, Vec<String>)> = None;
        for line in md.lines() {
            let t = line.trim();
            match (&mut current, t.strip_prefix("```")) {
                (None, Some(info)) => current = Some((info.trim().to_owned(), Vec::new())),
                (Some(_), Some(_)) => out.extend(current.take()),
                (Some((_, body)), None) => body.push(line.to_owned()),
                (None, None) => {}
            }
        }
        out
    }

    /// A README snippet that says "From the <name> example" holds only
    /// code lines of that example, in the same order. Comments and test
    /// items of the example do not count. `source` gives the text of an
    /// example by name.
    pub fn snippet_violations(
        readme: &str,
        source: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<String> {
        let mut out = Vec::new();
        for (_, body) in fenced_blocks(readme) {
            let Some(first) = body.first() else {
                continue;
            };
            let Some(name) = first
                .trim()
                .strip_prefix("// From the ")
                .and_then(|rest| rest.split_whitespace().next())
            else {
                continue;
            };
            let name = name.trim_matches('`');
            let Some(text) = source(name) else {
                out.push(format!(
                    "README: a snippet names the unknown example `{name}`"
                ));
                continue;
            };
            // The code lines of the snippet come in the same order in the
            // code of the example. A comment in the snippet is not checked.
            let lines = snippet_lines(&text);
            let mut from = 0;
            for line in snippet_lines(&body[1..].join("\n")) {
                match lines[from..].iter().position(|l| *l == line) {
                    Some(k) => from += k + 1,
                    None => out.push(format!(
                        "README: the `{name}` snippet has a line that the example code does not have in this order: {line}"
                    )),
                }
            }
        }
        out
    }

    /// The command that the CI lock check tells the user to run, after
    /// `Run: ` in its error line.
    pub fn lock_hint(steps: &[Step]) -> Option<String> {
        steps.iter().flat_map(|s| s.run.iter()).find_map(|l| {
            let at = l.find("Cargo.lock is out of date. Run: ")?;
            let rest = &l[at + "Cargo.lock is out of date. Run: ".len()..];
            Some(rest.trim_end_matches('"').trim().to_owned())
        })
    }

    /// The contributor commands of a document: the lock hint, and each
    /// one-line `cargo` command of the job `job` that runs on each event.
    pub fn command_doc_violations(
        doc_name: &str,
        doc: &str,
        job: &str,
        steps: &[Step],
    ) -> Vec<String> {
        let mut out = Vec::new();
        let one_line = doc.replace("\\\n", " ");
        let one_line: String = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut commands: Vec<String> = steps
            .iter()
            .filter(|s| s.job == job && s.cond.is_none() && s.run.len() == 1)
            .map(|s| s.run[0].clone())
            .filter(|c| c.starts_with("cargo "))
            .collect();
        match lock_hint(steps) {
            Some(hint) => commands.push(hint),
            None => out.push("ci.yml: the lock check gives no command after `Run: `".to_owned()),
        }
        if commands.is_empty() {
            out.push(format!("ci.yml: the job `{job}` has no cargo command"));
        }
        for command in commands {
            if !one_line.contains(&command) {
                out.push(format!(
                    "{doc_name}: does not give the CI command `{command}`"
                ));
            }
        }
        out
    }
}

/// Self-tests: each check sees its bad sample.
mod self_tests {
    use super::wired;

    const NONE: [String; 0] = [];

    #[test]
    fn a_comment_or_a_string_is_not_code() {
        let bad = "// const RUN_TIMEOUT: u8 = 1;\n\
                   /// see fn check(\n\
                   /* expect_err( */\n\
                   const S: &str = \"#[cfg(test)]\";\n\
                   fn main() {}\n";
        let found = wired::source_violations("x", bad, false);
        assert_eq!(found.len(), 4, "{found:?}");
        let good = "const RUN_TIMEOUT: u8 = 1;\n\
                    fn check() {}\n\
                    #[cfg(test)]\n\
                    mod tests {\n    fn t() { super::check().expect_err(\"x\"); }\n}\n";
        assert_eq!(wired::source_violations("x", good, false), NONE);
    }

    #[test]
    fn a_time_bound_in_the_tests_is_for_a_server_only() {
        let tests_only = "fn check() {}\n\
                          #[cfg(test)]\n\
                          mod tests {\n    const RUN_TIMEOUT: u8 = 1;\n    fn t() { let s = \"}\"; x.expect_err(s); }\n}\n";
        let found = wired::source_violations("x", tests_only, false);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("outside the tests"), "{found:?}");
        assert_eq!(wired::source_violations("x", tests_only, true), NONE);
    }

    #[test]
    fn a_step_name_or_an_if_is_not_a_run() {
        let ci = "jobs:\n  examples:\n    steps:\n\
                  \x20     - name: \"cargo run --example a\"\n\
                  \x20       run: echo skipped\n\
                  \x20     - name: b\n\
                  \x20       if: false\n\
                  \x20       run: cargo run --example b\n\
                  \x20     - name: c\n\
                  \x20       run: |\n\
                  \x20         cargo build\n\
                  \x20         cargo run --example c || true\n\
                  \x20     - run: cargo run --example d\n";
        let steps = wired::steps(ci);
        for (command, problem) in [
            ("cargo run --example a", "does not run"),
            ("cargo run --example b", "does not run"),
            ("cargo run --example c", "hides a failure"),
        ] {
            let found = wired::ci_violations("x", &[command], &steps);
            assert_eq!(found.len(), 1, "{command}: {found:?}");
            assert!(found[0].contains(problem), "{command}: {found:?}");
        }
        assert_eq!(
            wired::ci_violations("x", &["cargo run --example d"], &steps),
            NONE
        );
        assert!(steps.iter().all(|s| s.job == "examples"));
    }

    #[test]
    fn a_readme_command_needs_its_env_vars() {
        let command = "cargo run --example app";
        let bad = "| [`app`](x) | ... | `cargo run --example app` |\n";
        let found = wired::readme_violations("app", command, &["APP_DIR="], bad);
        assert_eq!(found.len(), 1, "{found:?}");
        let good = "| [`app`](x) | ... | `APP_DIR=x cargo run --example app` |\n";
        assert_eq!(
            wired::readme_violations("app", command, &["APP_DIR="], good),
            NONE
        );
        let split = "| [`app`](x) |\n```sh\nAPP_DIR=x \\\n  cargo run --example app\n```\n";
        assert_eq!(
            wired::readme_violations("app", command, &["APP_DIR="], split),
            NONE
        );
        let found = wired::readme_violations("app", command, &[], "no row\n");
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_snippet_must_copy_its_example() {
        let example = "fn start() {\n    let route = build(\"jira:get?issueKey=OPS-1\")\n        .run()?;\n}\n";
        let source = |name: &str| (name == "demo").then(|| example.to_owned());
        let good = "```rust,ignore\n// From the demo example, which CI runs.\nlet route = build(\"jira:get?issueKey=OPS-1\")\n    .run()?;\n```\n";
        assert_eq!(wired::snippet_violations(good, &source), NONE);
        let bad = "```rust,ignore\n// From the demo example, which CI runs.\nlet route = build(\"jira:get?issueKey=OPS-7\")\n    .run()?;\n```\n\
                   ```rust,ignore\n// From the other example.\nx();\n```\n";
        let found = wired::snippet_violations(bad, &source);
        assert_eq!(found.len(), 2, "{found:?}");
        let free = "```rust,ignore\nlet any = 1;\n```\n";
        assert_eq!(wired::snippet_violations(free, &source), NONE);
        // R2E-tests-guards-9: the order counts, and test code and comments
        // are not the example.
        let two = "fn start() {\n    let a = one();\n    // let c = gone();\n    let b = two();\n}\n\
                   #[cfg(test)]\nmod tests {\n    fn t() { let d = test_only(); }\n}\n";
        let source = |name: &str| (name == "demo").then(|| two.to_owned());
        let snippet =
            |lines: &str| format!("```rust,ignore\n// From the demo example.\n{lines}```\n");
        let in_order = snippet("let a = one();\n// A note.\nlet b = two();\n");
        assert_eq!(wired::snippet_violations(&in_order, &source), NONE);
        for bad in [
            snippet("let b = two();\nlet a = one();\n"),
            snippet("let a = one();\nlet d = test_only();\n"),
            snippet("let a = one();\nlet c = gone();\n"),
        ] {
            let found = wired::snippet_violations(&bad, &source);
            assert_eq!(found.len(), 1, "{bad}: {found:?}");
        }
    }

    #[test]
    fn a_doc_must_give_the_ci_commands() {
        let ci = "jobs:\n  eco:\n    env:\n      MANIFEST: m/Cargo.toml\n    steps:\n\
                  \x20     - name: weekly\n\
                  \x20       if: github.event_name == 'schedule'\n\
                  \x20       run: cargo update --manifest-path \"$MANIFEST\"\n\
                  \x20     - name: lock\n\
                  \x20       run: |\n\
                  \x20         echo \"::error::m/Cargo.lock is out of date. Run: cargo update --manifest-path $MANIFEST -p a -p b\"\n\
                  \x20     - run: cargo fmt --manifest-path \"$MANIFEST\" --check\n";
        let steps = wired::steps(ci);
        let good = "cargo fmt --manifest-path m/Cargo.toml --check\n\
                    cargo update --manifest-path m/Cargo.toml \\\n  -p a -p b\n";
        let found = wired::command_doc_violations("DOC", good, "eco", &steps);
        assert!(found.is_empty(), "{found:?}");
        let bad = "cargo update --manifest-path m/Cargo.toml -p a\n";
        let found = wired::command_doc_violations("DOC", bad, "eco", &steps);
        assert_eq!(found.len(), 2, "{found:?}");
    }
}

#[test]
fn each_example_is_wired() {
    let ci = read(".github/workflows/ci.yml");
    let steps = wired::steps(&ci);
    let readme = read("README.md");
    let mut problems = Vec::new();
    for ex in EXAMPLES {
        let manifest = read(ex.manifest);
        let Some(section) = target_section(&manifest, ex.target, ex.name) else {
            problems.push(format!(
                "{}: no {} target in {}",
                ex.name, ex.target, ex.manifest
            ));
            continue;
        };
        let has = |line: &str| section.lines().any(|l| l.trim() == line);
        if ex.target == "[[example]]" && !has("test = true") {
            problems.push(format!("{}: the target needs `test = true`", ex.name));
        }
        if has("test = false") {
            problems.push(format!("{}: the target must keep its tests", ex.name));
        }
        problems.extend(wired::source_violations(
            ex.name,
            &source_text(ex),
            ex.server,
        ));
        problems.extend(wired::ci_violations(ex.name, ex.ci, &steps));
        problems.extend(wired::readme_violations(
            ex.name,
            ex.readme,
            ex.readme_env,
            &readme,
        ));
    }
    if ci
        .lines()
        .any(|l| !l.trim_start().starts_with('#') && l.contains("continue-on-error"))
    {
        problems.push("a CI step must not hide a failure".to_owned());
    }
    // The smoke script runs the same example as the README command.
    let smoke = read("scripts/smoke-autumn-example.sh");
    if !smoke.contains("-p autumn-plugin-jira --features testing --example autumn_app") {
        problems.push("scripts/smoke-autumn-example.sh does not build autumn_app".to_owned());
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn readme_snippets_copy_their_examples() {
    let source = |name: &str| EXAMPLES.iter().find(|ex| ex.name == name).map(source_text);
    let found = wired::snippet_violations(&read("README.md"), &source);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn docs_give_the_ci_commands_of_the_separate_workspace() {
    let steps = wired::steps(&read(".github/workflows/ci.yml"));
    let mut found = Vec::new();
    for doc in COMMAND_DOCS {
        found.extend(wired::command_doc_violations(
            doc,
            &read(doc),
            ECOSYSTEM_JOB,
            &steps,
        ));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn no_example_turns_on_the_dev_only_proxy_flags() {
    let mut files: Vec<PathBuf> = EXAMPLES
        .iter()
        .flat_map(|ex| ex.sources.iter())
        .flat_map(|s| {
            let path = root().join(s);
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect();
    files.extend(COPIED_FILES.iter().map(|f| root().join(f)));
    // Each file of the ecosystem workspace that a user copies: code,
    // config and scripts. The Markdown can name a flag to warn about it.
    let eco = root().join("examples/ecosystem");
    if eco.exists() {
        files.extend(files_under(&eco).into_iter().filter(|f| {
            f.extension()
                .is_some_and(|e| ["rs", "toml", "yaml", "yml", "sh"].iter().any(|x| e == *x))
        }));
    }
    for path in files {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .to_ascii_lowercase();
        for flag in ["allow_private_hosts", "allow_plain_http"] {
            assert!(!text.contains(flag), "{} has {flag}", path.display());
        }
    }
}

#[test]
fn each_example_file_is_in_the_list() {
    let listed = |name: &str| EXAMPLES.iter().any(|ex| ex.name == name);
    for krate in std::fs::read_dir(root().join("crates")).unwrap() {
        let dir = krate.unwrap().path().join("examples");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            // A cargo example is a `.rs` file or a folder with `main.rs`.
            let is_example =
                path.extension().is_some_and(|e| e == "rs") || path.join("main.rs").exists();
            if is_example && !name.starts_with('.') {
                assert!(listed(&name), "the example {name} is not in EXAMPLES");
            }
        }
    }
    // An example with its own workspace is a folder of `examples/`.
    for entry in std::fs::read_dir(root().join("examples")).unwrap() {
        let dir = entry.unwrap().path();
        if dir.join("Cargo.toml").exists() {
            let relative = format!(
                "examples/{}/Cargo.toml",
                dir.file_name().unwrap().to_string_lossy()
            );
            assert!(
                EXAMPLES.iter().any(|ex| ex.manifest == relative),
                "the workspace {relative} is not in EXAMPLES"
            );
        }
    }
}

#[test]
fn the_ecosystem_workspace_stays_apart() {
    let root_manifest = read("Cargo.toml");
    assert!(
        root_manifest.contains("exclude = [\"examples/ecosystem\"]"),
        "the root workspace must exclude examples/ecosystem"
    );
    let members = root_manifest
        .split("members = [")
        .nth(1)
        .and_then(|m| m.split(']').next())
        .expect("the root workspace lists its members");
    assert!(
        !members.contains("examples"),
        "an example is a member of the root workspace"
    );
    // The example has its own workspace and lock. Its builds stay out of
    // the root lock (ADR 0008).
    let manifest = read("examples/ecosystem/Cargo.toml");
    for line in ["[workspace]", "publish = false"] {
        assert!(
            manifest.lines().any(|l| l.trim() == line),
            "examples/ecosystem/Cargo.toml needs `{line}`"
        );
    }
    assert!(
        root().join("examples/ecosystem/Cargo.lock").is_file(),
        "examples/ecosystem needs its own Cargo.lock"
    );
    let lock = read("Cargo.lock");
    for name in ["ecosystem", "harvest-camel", "autumn-harvest-plugin"] {
        assert!(
            !lock.contains(&format!("name = \"{name}\"")),
            "the root Cargo.lock has {name}"
        );
    }
    // The example builds the crates of this checkout, not a release.
    for krate in CRATES {
        let entry = format!("path = \"../../crates/{krate}\"");
        assert!(
            manifest
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{krate} =")) && l.contains(&entry)),
            "examples/ecosystem/Cargo.toml must use {krate} by path"
        );
    }
}
