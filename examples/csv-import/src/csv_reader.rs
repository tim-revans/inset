//! The `CsvReader` external component, written in Rust with the `csv` crate.
//!
//! `import.inset` declares its interface. The compiler checks that this file
//! provides it, and the diagram shows it as an opaque box.

use crate::Row;

pub struct CsvReader;

impl CsvReader {
    pub fn read(&self, text: &String) -> Vec<Row> {
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
