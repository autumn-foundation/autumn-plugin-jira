//! Finds the files that the doc tests and the unit tests read from outside
//! `src/`.
//!
//! In the workspace, the README and the workspace `Cargo.toml` are two
//! levels up. In the published package, cargo copies the README next to
//! `Cargo.toml`, and the workspace `Cargo.toml` does not exist. A fixed
//! `include_str!` path then points outside the package, and
//! `cargo test --doc` of the package does not compile. So this script gives
//! the path that exists, and a cfg for each file that it finds.

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(jira_readme)");
    println!("cargo::rustc-check-cfg=cfg(jira_workspace)");
    println!("cargo::rerun-if-changed=build.rs");
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let dir = Path::new(&dir);
    // The package has the README next to `Cargo.toml`. The workspace has it
    // at its root.
    let readme = [dir.join("README.md"), dir.join("../../README.md")]
        .into_iter()
        .find(|p| p.is_file());
    if let Some(readme) = readme {
        println!("cargo::rerun-if-changed={}", readme.display());
        println!(
            "cargo::rustc-env=AUTUMN_PLUGIN_JIRA_README={}",
            readme.display()
        );
        println!("cargo::rustc-cfg=jira_readme");
    }
    // A published package has `Cargo.toml.orig`. Only the workspace has the
    // workspace manifest.
    let workspace = dir.join("../../Cargo.toml");
    if !dir.join("Cargo.toml.orig").exists() && workspace.is_file() {
        println!("cargo::rerun-if-changed={}", workspace.display());
        println!(
            "cargo::rustc-env=AUTUMN_PLUGIN_JIRA_WORKSPACE_MANIFEST={}",
            workspace.display()
        );
        println!("cargo::rustc-cfg=jira_workspace");
    }
}
