//! The runnable examples stay wired (EX8). For each example, this test
//! checks the Cargo target, a self-check and a failure test in the source,
//! the CI commands and the README row. See ADR 0008.
//!
//! The test reads files of the repository. It uses text search, so keep the
//! fixed formats below. It copies the san-mateo `examples_wired` pattern.

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
}

const EXAMPLES: &[Example] = &[
    Example {
        name: "embedded",
        manifest: "crates/autumn-jira/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira/examples/embedded.rs"],
        ci: &["cargo run --locked -p autumn-jira --example embedded"],
        readme: "cargo run --locked -p autumn-jira --example embedded",
    },
    Example {
        name: "autumn_app",
        manifest: "crates/autumn-plugin-jira/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-plugin-jira/examples/autumn_app"],
        // The smoke script builds the example and boots it in dev and prod.
        ci: &["bash scripts/smoke-autumn-example.sh"],
        readme: "cargo run --locked -p autumn-plugin-jira --features testing --example autumn_app",
    },
    Example {
        name: "harvest_workflow",
        manifest: "crates/autumn-jira-harvest/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira-harvest/examples/harvest_workflow.rs"],
        ci: &["cargo run --locked -p autumn-jira-harvest --example harvest_workflow"],
        readme: "cargo run --locked -p autumn-jira-harvest --example harvest_workflow",
    },
    Example {
        name: "camel_route",
        manifest: "crates/autumn-jira-camel/Cargo.toml",
        target: "[[example]]",
        sources: &["crates/autumn-jira-camel/examples/camel_route.rs"],
        ci: &["cargo run --locked -p autumn-jira-camel --example camel_route"],
        readme: "cargo run --locked -p autumn-jira-camel --example camel_route",
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

/// The workflow lines with no comment lines. A `$MANIFEST` takes the value
/// of the `MANIFEST:` env entry, so a command reads as a user types it.
fn ci_lines() -> Vec<String> {
    let ci = read(".github/workflows/ci.yml");
    let manifest = ci
        .lines()
        .find_map(|l| l.trim().strip_prefix("MANIFEST:"))
        .map(|v| v.trim().trim_matches('"').to_owned());
    ci.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| {
            manifest.as_ref().map_or_else(
                || l.to_owned(),
                |m| l.replace("\"$MANIFEST\"", m).replace("$MANIFEST", m),
            )
        })
        .collect()
}

#[test]
fn each_example_is_wired() {
    let ci = ci_lines();
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
        let source = source_text(ex);
        for must in [
            "const RUN_TIMEOUT",
            "#[cfg(test)]",
            "fn check(",
            "expect_err(",
        ] {
            if !source.contains(must) {
                problems.push(format!("{}: the source needs `{must}`", ex.name));
            }
        }
        if source.contains("full_migrations_sql") {
            problems.push(format!("{}: use `migrate::apply`", ex.name));
        }
        for command in ex.ci {
            let lines: Vec<&String> = ci.iter().filter(|l| l.contains(command)).collect();
            if lines.is_empty() {
                problems.push(format!("{}: CI does not run `{command}`", ex.name));
            }
            if lines.iter().any(|l| l.contains("||")) {
                problems.push(format!("{}: CI hides a failure of `{command}`", ex.name));
            }
        }
        if !readme.contains(ex.readme) {
            problems.push(format!(
                "{}: the README does not show `{}`",
                ex.name, ex.readme
            ));
        }
        if !readme.contains(&format!("| [`{}`](", ex.name)) {
            problems.push(format!("{}: the README Examples table has no row", ex.name));
        }
    }
    if ci.iter().any(|l| l.contains("continue-on-error")) {
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
