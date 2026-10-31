//! Stores an entry in a table a built program is granted, as a `given`
//! fact would, so an end-to-end run can start with data the program has no
//! route to add. `cargo xtask e2e stock-tracker` uses it.
//!
//! ```sh
//! cargo run -p inset-host --example seed -- <component.wasm> <dir> <grant> <key> <value>
//! ```
//!
//! The program's tables are opened in `dir`, as `inset run` there would
//! open them. The key is a `String` key's text, which is its stored form,
//! and the value is the value's stored form, as JSON. A value that does not
//! fit the table's type is refused when the program next starts.

use std::path::PathBuf;
use std::process::ExitCode;

use inset_host::{Mode, Options, Program};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [component, dir, grant, key, value] = args.as_slice() else {
        eprintln!("usage: seed <component.wasm> <dir> <grant> <key> <value>");
        return ExitCode::FAILURE;
    };
    match seed(component, dir, grant, key, value) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("seed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn seed(component: &str, dir: &str, grant: &str, key: &str, value: &str) -> Result<(), String> {
    let program = Program::load(&PathBuf::from(component)).map_err(|e| e.to_string())?;
    let options = Options {
        settings: Vec::new(),
        mode: Mode::Real,
        working_dir: PathBuf::from(dir),
    };
    let running = program.start(&options).map_err(|e| e.to_string())?;
    let mut table = running
        .table(grant)
        .ok_or_else(|| format!("the program has no table grant `{grant}`"))?;
    table.set(key.as_bytes(), value.as_bytes())
}
