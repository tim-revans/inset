//! Repository task runner, invoked as `cargo xtask <task>`.
//!
//! Plain Rust with no dependencies, so it works for any contributor or agent
//! without extra tooling. The VS Code extension's grammar tests and packaging
//! need Node.js, so those steps shell out to `npm` in `editors/vscode`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <task>

tasks:
  ci        format check, clippy, tests, editor grammar tests
  editor    package the VS Code extension as a .vsix";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("ci") => ci(),
        Some("editor") => editor(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(step) => {
            eprintln!("xtask: `{step}` failed");
            ExitCode::FAILURE
        }
    }
}

/// The single gate every change must pass.
fn ci() -> Result<(), String> {
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace"])?;
    npm_install()?;
    npm(&["test"])?;
    Ok(())
}

/// Packages the VS Code extension for local install. Publishing waits for the alpha.
fn editor() -> Result<(), String> {
    npm_install()?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir().join("target"));
    let out_dir = target.join("vscode");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {} ({e})", out_dir.display()))?;
    let vsix = out_dir.join("inset.vsix");
    let out = vsix.to_string_lossy();
    npm(&["run", "package", "--", "--out", &out])?;
    eprintln!("xtask: install with `code --install-extension {out}`");
    Ok(())
}

fn cargo(args: &[&str]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    run(
        Command::new(cargo).args(args),
        format!("cargo {}", args.join(" ")),
    )
}

fn npm(args: &[&str]) -> Result<(), String> {
    run(
        Command::new("npm").args(args).current_dir(extension_dir()),
        format!("npm {}", args.join(" ")),
    )
}

/// Installs the extension's locked dependencies, unless they are already installed.
fn npm_install() -> Result<(), String> {
    let dir = extension_dir();
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let lock = modified(&dir.join("package-lock.json"));
    let installed = modified(&dir.join("node_modules").join(".package-lock.json"));
    match (lock, installed) {
        (Some(lock), Some(installed)) if installed >= lock => Ok(()),
        _ => npm(&["ci", "--no-audit", "--no-fund"]),
    }
}

fn run(command: &mut Command, display: String) -> Result<(), String> {
    eprintln!("xtask: {display}");
    let status = command.status().map_err(|e| format!("{display} ({e})"))?;
    if status.success() {
        Ok(())
    } else {
        Err(display)
    }
}

fn repo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is inside the repository")
        .to_path_buf()
}

fn extension_dir() -> PathBuf {
    repo_dir().join("editors").join("vscode")
}
