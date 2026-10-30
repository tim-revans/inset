//! Builds the programs the tests run.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use inset_host::{Mode, Options, Program, Request, Response, Running, Stopped};

/// Builds a workspace package to a WebAssembly component, and loads it.
pub fn load(package: &str) -> Program {
    let path = build(package);
    Program::load(&path).unwrap_or_else(|e| panic!("{package} loads: {e}"))
}

/// Compiles the Inset program in `dir`, relative to the repository, and loads it.
pub fn compile(dir: &str) -> Program {
    let path = compiled(dir);
    Program::load(&path).unwrap_or_else(|e| panic!("{dir} loads: {e}"))
}

/// Compiles the Inset program in `dir`, relative to the repository, to a
/// WebAssembly component.
pub fn compiled(dir: &str) -> PathBuf {
    // Tests run in parallel, and two building one program would write its
    // project at the same time.
    static BUILDING: Mutex<()> = Mutex::new(());
    let _building = BUILDING.lock().unwrap_or_else(|e| e.into_inner());
    let program = inset_check::Program::load(&repo().join(dir))
        .unwrap_or_else(|e| panic!("{dir} loads: {e}"));
    let name = dir.rsplit('/').next().unwrap_or(dir);
    let out = target().join("inset").join(name);
    match inset_codegen::build(&program, &out, Some(&target())) {
        Ok(built) => {
            // A warning in generated Rust is a compiler bug, as an error is.
            assert_eq!(built.warnings, "", "{dir}: rustc warned");
            built.component
        }
        Err(inset_codegen::BuildError::Rejected(reports)) => {
            let rendered: String = reports.iter().map(|r| program.render(r, false)).collect();
            panic!("{dir} does not compile:\n{rendered}")
        }
        Err(e) => panic!("{dir} does not build: {e}"),
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A target directory of the tests' own, because the one `cargo test` is
/// using is locked while it runs.
fn target() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| repo().join("target"), PathBuf::from)
        .join("wasm-tests")
}

pub fn build(package: &str) -> PathBuf {
    let repo = repo();
    let target = target();
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
