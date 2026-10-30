//! Repository task runner, invoked as `cargo xtask <task>`.
//!
//! Plain Rust with no dependencies, so it works for any contributor or agent
//! without extra tooling. The VS Code extension's grammar tests and packaging
//! need Node.js, so those steps shell out to `npm` in `editors/vscode`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: cargo xtask <task>

tasks:
  ci                format check, clippy, tests, editor grammar tests
  editor            package the VS Code extension as a .vsix
  e2e hello         build Hello HTTP from its source, serve it and request a greeting";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("ci") => ci(),
        Some("editor") => editor(),
        Some("e2e") => match std::env::args().nth(2).as_deref() {
            Some("hello") => hello(),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
        },
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
    // The runtime's reports to the host compile only for WebAssembly.
    cargo(&[
        "clippy",
        "--target",
        "wasm32-wasip2",
        "-p",
        "inset-runtime",
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
    let out_dir = target_dir().join("vscode");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {} ({e})", out_dir.display()))?;
    let vsix = out_dir.join("inset.vsix");
    let out = vsix.to_string_lossy();
    npm(&["run", "package", "--", "--out", &out])?;
    eprintln!("xtask: install with `code --install-extension {out}`");
    Ok(())
}

/// Builds Hello HTTP from its Inset source with `inset build`, serves it
/// with `inset run` on a free port, and checks the greeting that comes back
/// over a real socket.
fn hello() -> Result<(), String> {
    cargo(&["build", "-p", "inset-cli"])?;
    let inset = target_dir().join("debug/inset");
    let example = repo_dir().join("examples/hello-http");
    run(
        Command::new(&inset).arg("build").arg(&example),
        "inset build examples/hello-http".into(),
    )?;
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map_err(|e| format!("find a free port ({e})"))?
        .port();
    eprintln!("xtask: inset run examples/hello-http --set port={port}");
    let mut server = Command::new(&inset)
        .arg("run")
        .arg(&example)
        .args(["--set", &format!("port={port}")])
        .spawn()
        .map_err(|e| format!("start inset run ({e})"))?;
    let reply = get(port, "/hello/Ada");
    let _ = server.kill();
    let _ = server.wait();
    let reply = reply?;
    eprintln!("xtask: GET /hello/Ada replied:\n{reply}");
    let ok = reply.starts_with("HTTP/1.1 200 OK\r\n") && reply.ends_with("\r\n\r\nHello, Ada!");
    if ok {
        Ok(())
    } else {
        Err("e2e hello: unexpected reply".into())
    }
}

/// Sends a GET request once the server is listening, and returns the raw reply.
fn get(port: u16, path: &str) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("connect to port {port} ({e})")),
        }
    };
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let mut reply = String::new();
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.read_to_string(&mut reply))
        .map_err(|e| format!("GET {path} ({e})"))?;
    Ok(reply)
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_dir().join("target"))
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
