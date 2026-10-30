//! `inset build` compiles a program to a WebAssembly component, refuses one
//! with errors, and `inset run` serves what it built.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `inset` building in a target directory apart from the one `cargo test`
/// holds locked, shared with the other tests that build programs.
fn inset(args: &[&str], dir: &Path) -> Command {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo().join("target"), PathBuf::from)
        .join("wasm-tests");
    let mut command = Command::new(env!("CARGO_BIN_EXE_inset"));
    command.args(args).arg(dir).env("CARGO_TARGET_DIR", target);
    command
}

/// A copy of Hello HTTP's source, so building it writes nothing into the example.
fn hello_copy(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("inset-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::copy(
        repo().join("examples/hello-http/src/main.inset"),
        dir.join("src/main.inset"),
    )
    .unwrap();
    dir
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn builds_hello_http() {
    let dir = hello_copy("build");
    let out = inset(&["build"], &dir).output().expect("run inset");
    assert!(out.status.success(), "{}", stderr(&out));
    let component = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    assert_eq!(component, dir.join("target/inset/hello-http.wasm"));
    assert!(component.is_file());
    assert!(dir.join("target/inset/src/lib.rs").is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn refuses_a_program_with_errors() {
    let dir = hello_copy("refuse");
    let source = std::fs::read_to_string(dir.join("src/main.inset")).unwrap();
    std::fs::write(
        dir.join("src/main.inset"),
        source.replace("fn hello", "async fn hello"),
    )
    .unwrap();
    let out = inset(&["build"], &dir).output().expect("run inset");
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).contains("an `async fn` cannot be compiled yet"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn runs_hello_http() {
    let dir = hello_copy("run");
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap()
        .port();
    let mut server = inset(&["run"], &dir)
        .args(["--set", &format!("port={port}")])
        .spawn()
        .expect("run inset");
    let deadline = Instant::now() + Duration::from_secs(300);
    let reply = loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let mut reply = String::new();
            stream
                .write_all(
                    b"GET /hello/Ada HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                )
                .and_then(|()| stream.read_to_string(&mut reply))
                .unwrap();
            break reply;
        }
        let exited = server.try_wait().unwrap();
        assert!(
            exited.is_none() && Instant::now() < deadline,
            "inset run did not serve"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = server.kill();
    let _ = server.wait();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    assert!(reply.ends_with("\r\n\r\nHello, Ada!"), "{reply}");
}

#[test]
fn run_rejects_a_malformed_setting() {
    let out = inset(&["run"], &repo().join("examples/hello-http"))
        .args(["--set", "port"])
        .output()
        .expect("run inset");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("is not `--set name=value`"),
        "{}",
        stderr(&out)
    );
}
