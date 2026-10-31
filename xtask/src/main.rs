//! Repository task runner, invoked as `cargo xtask <task>`.
//!
//! Plain Rust with no dependencies, so it works for any contributor or agent
//! without extra tooling. The VS Code extension's tests and packaging need
//! Node.js, so those steps shell out to `npm` in `editors/vscode`. The
//! TypeScript compiler that checks the Stock tracker's front end against its
//! API contract is installed there too, as the repository's one Node.js project. Its
//! integration tests start VS Code, which needs a display: under `xvfb-run`
//! when that is installed, as in CI, and on the desktop otherwise.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const USAGE: &str = "usage: cargo xtask <task>

tasks:
  ci                format check, clippy, tests, editor grammar and integration tests
  editor            package the VS Code extension as a .vsix
  editor-test       run the VS Code extension's integration tests alone
  contract          check the Stock tracker's front end against its TypeScript contract
  e2e hello         build Hello HTTP from its source, serve it and request a greeting
  e2e stock-tracker build the Stock tracker, serve it, stock an item as JSON, and ship it";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);
    let result = match task.as_deref() {
        Some("ci") => ci(),
        Some("editor") => editor(),
        Some("editor-test") => npm_install().and_then(|()| editor_integration()),
        Some("contract") => npm_install().and_then(|()| contract()),
        Some("e2e") => match std::env::args().nth(2).as_deref() {
            Some("hello") => hello(),
            Some("stock-tracker") => stock_tracker(),
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
    contract()?;
    editor_integration()
}

/// Checks the Stock tracker's front end with `tsc` against the TypeScript
/// types `inset contract` writes, and checks that renaming a field the front
/// end uses, in a copy of the Inset source, makes the same check fail.
fn contract() -> Result<(), String> {
    cargo(&["build", "--quiet", "-p", "inset-cli"])?;
    let example = repo_dir().join("examples/stock-tracker");
    typecheck(&example)?;
    let copy = target_dir().join("contract-rename");
    let _ = std::fs::remove_dir_all(&copy);
    for sub in ["src", "web"] {
        copy_dir(&example.join(sub), &copy.join(sub))?;
    }
    for file in ["src/main.inset", "src/warehouse.inset"] {
        let path = copy.join(file);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("read {file} ({e})"))?;
        std::fs::write(&path, text.replace("on_hand", "in_stock"))
            .map_err(|e| format!("write {file} ({e})"))?;
    }
    eprintln!("xtask: with `on_hand` renamed `in_stock`, `tsc` must fail");
    if typecheck(&copy).is_ok() {
        return Err("tsc passed the front end after a field it uses was renamed".into());
    }
    std::fs::remove_dir_all(&copy).map_err(|e| format!("remove {} ({e})", copy.display()))
}

/// Writes the TypeScript contract of the program in `dir` where its front
/// end imports it from, and checks the front end's JavaScript with `tsc`.
fn typecheck(dir: &Path) -> Result<(), String> {
    let types = dir.join("target/inset/stock-tracker.d.ts");
    run(
        Command::new(target_dir().join("debug/inset"))
            .arg("contract")
            .arg(dir)
            .arg("--typescript")
            .arg("-o")
            .arg(&types),
        format!("inset contract {} --typescript", dir.display()),
    )?;
    let tsc = extension_dir().join("node_modules/.bin/tsc");
    run(
        Command::new(tsc)
            .args([
                "--noEmit",
                "--allowJs",
                "--checkJs",
                "--strict",
                "--target",
                "es2022",
                "--module",
                "es2022",
                "--moduleResolution",
                "bundler",
                "--lib",
                "es2022,dom,dom.iterable",
            ])
            .arg(dir.join("web/app.js")),
        format!("tsc {}", dir.join("web/app.js").display()),
    )
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("create {} ({e})", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("read {} ({e})", from.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let target = to.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("copy {} ({e})", path.display()))?;
        }
    }
    Ok(())
}

/// Opens the VS Code extension's panels on Hello HTTP in a real VS Code,
/// with the `inset` built from this checkout.
fn editor_integration() -> Result<(), String> {
    cargo(&["build", "-p", "inset-cli"])?;
    let inset = target_dir().join("debug/inset");
    let xvfb = Command::new("xvfb-run").arg("--help").output().is_ok();
    let mut command = if xvfb {
        let mut command = Command::new("xvfb-run");
        command.args(["-a", "npm"]);
        command
    } else {
        Command::new("npm")
    };
    command
        .args(["run", "test:integration"])
        .current_dir(extension_dir())
        .env("INSET_COMMAND", &inset);
    let display = if xvfb { "xvfb-run -a npm" } else { "npm" };
    run(&mut command, format!("{display} run test:integration"))
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

/// Builds the Stock tracker from its Inset source, stocks one item in a
/// fresh `stock.db`, serves it with `inset run`, and checks the replies to
/// requests that receive, ship and reorder over a real socket. A second run
/// finds what the first one stored.
fn stock_tracker() -> Result<(), String> {
    cargo(&["build", "-p", "inset-cli"])?;
    let inset = target_dir().join("debug/inset");
    let example = repo_dir().join("examples/stock-tracker");
    run(
        Command::new(&inset).arg("build").arg(&example),
        "inset build examples/stock-tracker".into(),
    )?;
    // `stock.db` is fixed in the source, so it lands where `inset run` runs.
    let dir = target_dir().join("e2e-stock-tracker");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {} ({e})", dir.display()))?;
    let dir_text = dir.to_string_lossy();
    let page = std::fs::read_to_string(example.join("web/index.html"))
        .map_err(|e| format!("read the front end's page ({e})"))?;
    let item = r#"{"onHand": 12, "reorderLevel": 10, "targetLevel": 50}"#;
    let levels = r#"[{"sku":"bolt-m6","onHand":10,"reorderLevel":10,"low":true}]"#;
    let exchanges: [&[(&str, &str, &str, &str)]; 2] = [
        &[
            ("GET", "/items/bolt-m6", "", "404 Not Found|"),
            ("PUT", "/items/bolt-m6", item, "200 OK|12"),
            ("GET", "/items/bolt-m6", "", "200 OK|12"),
            ("POST", "/items/bolt-m6/shipments", "2", "200 OK|10"),
            ("GET", "/reorders", "", "200 OK|bolt-m6 40\\n"),
            ("GET", "/items?low=true", "", &format!("200 OK|{levels}")),
            ("POST", "/items/bolt-m6/receipts", "5", "200 OK|15"),
            ("POST", "/items/bolt-m6/shipments", "100", "409 Conflict|"),
            ("PUT", "/items/nut-m6", "{}", "400 Bad Request|"),
            ("GET", "/items/nut-m6", "", "404 Not Found|"),
            ("GET", "/", "", &format!("200 OK|{page}")),
        ],
        // Reorders are kept in memory, and the stock in `stock.db`.
        &[
            ("GET", "/items/bolt-m6", "", "200 OK|15"),
            ("GET", "/reorders", "", "200 OK|"),
        ],
    ];
    for exchanges in exchanges {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("find a free port ({e})"))?
            .port();
        eprintln!("xtask: inset run examples/stock-tracker --set port={port}, in {dir_text}");
        let mut server = Command::new(&inset)
            .arg("run")
            .arg(&example)
            .args(["--set", &format!("port={port}")])
            .current_dir(&dir)
            .spawn()
            .map_err(|e| format!("start inset run ({e})"))?;
        let outcome = exchanges
            .iter()
            .try_for_each(|(method, path, body, expected)| {
                let reply = request(port, method, path, body)?;
                let (status, text) = expected.split_once('|').unwrap_or((expected, ""));
                let text = text.replace("\\n", "\n");
                eprintln!(
                    "xtask: {method} {path} {body:?} replied {:?}",
                    reply.lines().next()
                );
                if reply.starts_with(&format!("HTTP/1.1 {status}\r\n"))
                    && reply.ends_with(&format!("\r\n\r\n{text}"))
                {
                    Ok(())
                } else {
                    Err(format!(
                        "e2e stock-tracker: {method} {path} expected {expected:?}, got:\n{reply}"
                    ))
                }
            });
        let _ = server.kill();
        let _ = server.wait();
        outcome?;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("remove {} ({e})", dir.display()))
}

/// Sends a GET request once the server is listening, and returns the raw reply.
fn get(port: u16, path: &str) -> Result<String, String> {
    request(port, "GET", path, "")
}

/// Sends a request once the server is listening, and returns the raw reply.
fn request(port: u16, method: &str, path: &str, body: &str) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("connect to port {port} ({e})")),
        }
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut reply = String::new();
    stream
        .write_all(request.as_bytes())
        .and_then(|()| stream.read_to_string(&mut reply))
        .map_err(|e| format!("{method} {path} ({e})"))?;
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
