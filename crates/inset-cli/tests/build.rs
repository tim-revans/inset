//! `inset build` compiles a program to a WebAssembly component, refuses one
//! with errors, and `inset run` serves what it built, keeping its tables in
//! SQLite files where it was run.

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
    let restart = "http: HttpServer,\n\n    failure {\n        /// Try again.\n        Panic => restart,\n    }\n";
    std::fs::write(
        dir.join("src/main.inset"),
        source.replace("http: HttpServer,\n", restart),
    )
    .unwrap();
    let out = inset(&["build"], &dir).output().expect("run inset");
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).contains("`restart` cannot be compiled yet"),
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

/// A copy of a test program's source, from `dir` in the repository.
fn program_copy(dir: &str, name: &str) -> PathBuf {
    let to = std::env::temp_dir().join(format!("inset-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    std::fs::create_dir_all(to.join("src")).unwrap();
    for entry in std::fs::read_dir(repo().join(dir).join("src")).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, to.join("src").join(path.file_name().unwrap())).unwrap();
    }
    to
}

/// Sends one request to a program `inset run` serves on `port`, waiting
/// for it to start listening, and returns the status line and body.
fn http(server: &mut std::process::Child, port: u16, request: &str, body: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            let mut reply = String::new();
            let (method, path) = request.split_once(' ').unwrap();
            stream
                .write_all(
                    format!(
                        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
                         Content-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .and_then(|()| stream.read_to_string(&mut reply))
                .unwrap();
            let status = reply.lines().next().unwrap_or_default().to_string();
            let body = reply.split_once("\r\n\r\n").map_or("", |(_, b)| b);
            return format!("{status} {body}");
        }
        let exited = server.try_wait().unwrap();
        assert!(
            exited.is_none() && Instant::now() < deadline,
            "inset run did not serve"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .unwrap()
        .port()
}

#[test]
fn a_run_keeps_its_table_in_a_file_the_next_run_reads() {
    let dir = program_copy("crates/inset-host/tests/programs/stock-room", "stored");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let serve = || {
        let port = free_port();
        let server = inset(&["run"], &dir)
            .args(["--set", &format!("port={port}")])
            .current_dir(&data)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("run inset");
        (server, port)
    };
    let stop = |mut server: std::process::Child| {
        let _ = server.kill();
        let mut stderr = String::new();
        server
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        let _ = server.wait();
        stderr
    };

    let (mut server, port) = serve();
    let mut send = |request, body| http(&mut server, port, request, body);
    assert_eq!(
        send("PUT /items/bolt-m6", "0.10"),
        "HTTP/1.1 200 OK stocked"
    );
    assert_eq!(send("PUT /items/nut-m6", "0.05"), "HTTP/1.1 200 OK stocked");
    assert_eq!(
        send("POST /items/bolt-m6/receipts", "3"),
        "HTTP/1.1 200 OK 3"
    );
    assert_eq!(
        send("POST /items/nut-m6/receipts", "4294967295"),
        "HTTP/1.1 200 OK 4294967295"
    );
    // Taking one from bolt-m6 is written, then adding it to nut-m6
    // overflows, so the request fails and the write is rolled back.
    assert_eq!(
        send("POST /items/bolt-m6/transfers/nut-m6", "1"),
        "HTTP/1.1 500 Internal Server Error "
    );
    let stderr = stop(server);
    assert!(
        stderr.contains("StockRoom/Room rolled back after Overflow"),
        "{stderr}"
    );
    assert!(data.join("stock-room.db").is_file());

    let (mut server, port) = serve();
    let listed = http(&mut server, port, "GET /items", "");
    stop(server);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        listed,
        "HTTP/1.1 200 OK bolt-m6 3 0.30\nnut-m6 4294967295 214748364.75\n"
    );
}
