//! Every example checks clean, and the checker never panics on a broken one.

use std::path::Path;

use inset_check::{Program, check};

/// What `inset check` reports on an example.
fn remaining(name: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    let program = Program::load(&dir).expect("load the example");
    check(&program)
        .iter()
        .map(|r| program.render(r, false))
        .collect()
}

#[test]
fn hello_http() {
    assert_eq!(remaining("hello-http"), "");
}

#[test]
fn stock_tracker() {
    assert_eq!(remaining("stock-tracker"), "");
}

#[test]
fn csv_import() {
    assert_eq!(remaining("csv-import"), "");
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
