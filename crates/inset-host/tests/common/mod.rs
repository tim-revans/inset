//! Builds the programs the tests run.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use inset_host::{Mode, Options, Program, Request, Response, Running, Stopped};

/// Builds a workspace package to a WebAssembly component, and loads it.
///
/// The build uses a target directory of its own, because the one `cargo
/// test` is using is locked while it runs.
pub fn load(package: &str) -> Program {
    let path = build(package);
    Program::load(&path).unwrap_or_else(|e| panic!("{package} loads: {e}"))
}

pub fn build(package: &str) -> PathBuf {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo.join("target"), PathBuf::from)
        .join("wasm-tests");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--quiet",
            "--release",
            "--target",
            "wasm32-wasip2",
            "-p",
            package,
        ])
        .arg("--target-dir")
        .arg(&target)
        .current_dir(&repo)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "{package} builds");
    target
        .join("wasm32-wasip2/release")
        .join(format!("{}.wasm", package.replace('-', "_")))
}

pub fn fake() -> Options {
    Options {
        settings: Vec::new(),
        mode: Mode::Fake,
        working_dir: PathBuf::from("/srv/app"),
    }
}

pub fn request(running: &mut Running<'_>, method: &str, path: &str) -> Result<Response, Stopped> {
    let request = Request {
        method: method.into(),
        path: path.into(),
        body: Vec::new(),
    };
    running.request("http", &request)
}

/// The status and text of a reply, from a program that is still running.
pub fn text(running: &mut Running<'_>, method: &str, path: &str) -> (u16, String) {
    let response = request(running, method, path).expect("the program is running");
    (
        response.status,
        String::from_utf8(response.body).expect("UTF-8"),
    )
}
