//! What `inset check` still reports on the examples the compiler does not
//! cover yet. Each error belongs to a construct a later part of the plan
//! adds, so none may be about types, modules, `match` or `for`, which the
//! checker reads. The snapshots pin the rest, so a new error shows up.

use std::path::Path;

use inset_check::{Program, check};

/// The constructs the checker covers, as diagnostics name them.
const COVERED: [&str; 8] = [
    "`struct`",
    "`enum`",
    "`#[derive]`",
    "`#[status]`",
    "`mod`",
    "`use`",
    "`match`",
    "`for`",
];

fn remaining(name: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    let program = Program::load(&dir).expect("load the example");
    let reports = check(&program);
    for report in &reports {
        let message = &report.diagnostic.message;
        assert!(
            !COVERED.iter().any(|c| message.contains(c)),
            "{name}: {message}"
        );
    }
    reports.iter().map(|r| program.render(r, false)).collect()
}

#[test]
fn stock_tracker() {
    insta::assert_snapshot!(remaining("stock-tracker"));
}

#[test]
fn csv_import() {
    insta::assert_snapshot!(remaining("csv-import"));
}

/// The language server checks a program on every keystroke, so no edit
/// may make the checker panic. Each run cuts a few characters out of one of
/// the Stock tracker's files, at every point through it.
#[test]
fn checking_a_broken_program_never_panics() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/stock-tracker");
    let program = Program::load(&dir).expect("load the example");
    for (index, file) in program.files.iter().enumerate() {
        let text = &file.text;
        let boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
        for (n, &at) in boundaries.iter().enumerate().step_by(4) {
            for cut in [1, 4] {
                let end = boundaries.get(n + cut).copied().unwrap_or(text.len());
                let mut broken = program.clone();
                broken.files[index].text = format!("{}{}", &text[..at], &text[end..]);
                let _ = check(&broken);
            }
        }
    }
}
