//! The `CsvReader` external component, written in Rust with the `csv` crate.
//!
//! This file is the root of a crate of its own, built as a WebAssembly
//! component of its own. The compiler generates the `interface` crate from the
//! declaration in `main.inset`: the `CsvReader` trait, the `Row` type, and the
//! `export!` macro that makes `Reader` the component's implementation.

use interface::{CsvReader, Row};

struct Reader;

impl CsvReader for Reader {
    fn new() -> Self {
        Reader
    }

    fn read(&self, text: &str) -> Vec<Row> {
        let mut reader = csv::ReaderBuilder::new()
            .flexible(true)
            .from_reader(text.as_bytes());
        let mut rows = Vec::new();
        for record in reader.records() {
            // The text is already valid UTF-8, it is read from memory, and
            // `flexible` accepts rows of any length, so no read can fail.
            let record = record.expect("in-memory CSV text is always readable");
            let line = record.position().map_or(0, |position| position.line());
            let fields = record.iter().map(String::from).collect();
            rows.push(Row { line, fields });
        }
        rows
    }
}

interface::export!(Reader);
