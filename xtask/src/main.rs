//! Repository task runner, invoked as `cargo xtask <task>`.
//!
//! Plain Rust with no dependencies, so it works for any contributor or agent
//! without extra tooling.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("ci") => ci(),
        _ => {
            eprintln!("usage: cargo xtask <task>\n\ntasks:\n  ci    format check, clippy, tests");
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
    Ok(())
}

fn cargo(args: &[&str]) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let display = format!("cargo {}", args.join(" "));
    eprintln!("xtask: {display}");
    let status = Command::new(cargo)
        .args(args)
        .status()
        .map_err(|e| format!("{display} ({e})"))?;
    if status.success() {
        Ok(())
    } else {
        Err(display)
    }
}
