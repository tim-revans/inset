//! Runs a program's requirements against its built component, in the host
//! with a fake behind every grant, and reports each by its sentence.
//!
//! The steps come from the checker's plan, with every value typed, so this
//! runs them without reading the program's code. `given` facts write the
//! fake tables in the stored form `inset-value` defines, which is what the
//! program's own code writes, and `then` facts and checks read them back.
//!
//! Each requirement starts the program afresh, so no requirement depends on
//! another having run. A requirement with an examples table runs once for
//! each row, each time afresh, and reports each row's result. Its steps run in order, and the first that does not
//! hold fails it. A fact that does not hold reports both values, written as
//! Rust's `{:?}` writes them, as `assert_eq!` does. The report is the same
//! every time for the same program, so it holds no timings.

use std::path::Path;

use inset_check::status::STATUSES;
use inset_check::{Plan, Planned, PlannedFact, PlannedStep, Program};
use inset_host::{Mode, Options, Request, Response, Running};
use inset_syntax::Span;
use inset_value::{Check, CheckError, Op, Shape, Types, Value, json, key};
use serde::Serialize;

/// What `inset test --json` prints.
#[derive(Debug, Serialize)]
pub struct TestReport {
    pub app: String,
    pub passed: usize,
    pub failed: usize,
    /// In order of file and then of position.
    pub requirements: Vec<Outcome>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub sentence: String,
    /// The path from the program's directory, as in `requirements/hello.req.inset`.
    pub file: String,
    /// The line the requirement starts on, counting from 1.
    pub line: usize,
    /// `pass` or `fail`. A requirement with an examples table fails if any
    /// of its rows does.
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    /// The headings of the examples table, if the requirement has one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub headings: Vec<String>,
    /// Each row of the examples table, run as a case of its own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowOutcome>,
}

/// One row of an examples table.
#[derive(Debug, Serialize)]
pub struct RowOutcome {
    /// Each cell as written.
    pub cells: Vec<String>,
    /// The line the row is on, counting from 1.
    pub line: usize,
    /// `pass` or `fail`.
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
}

/// The step that did not hold, and what happened instead.
#[derive(Debug, Serialize)]
pub struct Failure {
    pub line: usize,
    /// The step as written.
    pub step: String,
    pub message: String,
    /// For a fact or check that did not hold, the value the step expected,
    /// as Rust's `{:?}` writes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// The value found instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
}

/// A step that did not hold.
struct Failed {
    span: Span,
    message: String,
    /// What was expected and what was found, when the step compared two values.
    values: Option<(String, String)>,
}

impl Failed {
    fn new(span: Span, message: String) -> Failed {
        Failed {
            span,
            message,
            values: None,
        }
    }
}

/// Runs every requirement of `program` against the component built from it.
pub fn run(program: &Program, component: &Path) -> Result<TestReport, String> {
    let plan = inset_check::plan(program)
        .map_err(|_| "the program has errors, so its requirements cannot run".to_string())?;
    let host = inset_host::Program::load(component).map_err(|e| e.to_string())?;
    let working_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let options = Options {
        settings: Vec::new(),
        mode: Mode::Fake,
        working_dir,
    };
    let mut requirements = Vec::new();
    for requirement in &plan.requirements {
        let file = &program.files[requirement.file];
        let line = |span: Span| file.text[..span.start].matches('\n').count() + 1;
        // Each case starts the program afresh.
        let run_case = |steps: &[PlannedStep]| -> Result<Option<Failure>, String> {
            let mut running = host.start(&options).map_err(|e| e.to_string())?;
            Ok(run_steps(&plan, steps, &mut running).map(|failed| {
                let (expected, actual) = failed.values.unzip();
                Failure {
                    line: line(failed.span),
                    step: step_text(&file.text, failed.span),
                    message: failed.message,
                    expected,
                    actual,
                }
            }))
        };
        let (failure, headings, rows) = match &requirement.examples {
            None => (run_case(&requirement.steps)?, Vec::new(), Vec::new()),
            Some(examples) => {
                let mut rows = Vec::new();
                for row in &examples.rows {
                    let failure = run_case(&row.steps)?;
                    rows.push(RowOutcome {
                        cells: row.cells.clone(),
                        line: line(row.span),
                        result: if failure.is_some() { "fail" } else { "pass" },
                        failure,
                    });
                }
                (None, examples.headings.clone(), rows)
            }
        };
        let failed = failure.is_some() || rows.iter().any(|r| r.failure.is_some());
        requirements.push(Outcome {
            sentence: requirement.sentence.clone(),
            file: file.path.clone(),
            line: line(requirement.span),
            result: if failed { "fail" } else { "pass" },
            failure,
            headings,
            rows,
        });
    }
    let failed = requirements.iter().filter(|r| r.result == "fail").count();
    Ok(TestReport {
        app: host.manifest().app.clone(),
        passed: requirements.len() - failed,
        failed,
        requirements,
    })
}

/// Runs a case's steps, and returns the first that does not hold, with
/// what happened instead.
fn run_steps(plan: &Plan, steps: &[PlannedStep], running: &mut Running<'_>) -> Option<Failed> {
    let mut last: Option<Response> = None;
    let mut reports_before = 0;
    for step in steps {
        let fail = |message: String| Some(Failed::new(step.span, message));
        match &step.kind {
            Planned::Request {
                server,
                method,
                path,
                body,
            } => {
                let request = Request {
                    method: method.clone(),
                    path: path.clone(),
                    body: body.clone(),
                };
                reports_before = running.reports().len();
                match running.request(server, &request) {
                    Ok(response) => last = Some(response),
                    Err(stopped) => {
                        return fail(format!(
                            "the program stopped before it replied: {}",
                            inset_host::describe(&stopped)
                        ));
                    }
                }
            }
            Planned::Reply { status, body } => {
                let response = last.as_ref().expect("the checker puts a request first");
                if response.status != *status || response.body != body.as_bytes() {
                    let mut message = format!("but the reply was {}", describe(response));
                    for report in &running.reports()[reports_before..] {
                        message.push_str(&format!("\nafter {report}"));
                    }
                    return fail(message);
                }
            }
            Planned::Given(fact) => {
                if let Err(e) = given(running, fact) {
                    return fail(format!("but the fake could not be set up: {e}"));
                }
            }
            Planned::Then(fact) => {
                if let Some(mut failed) = then(running, fact, &plan.types) {
                    failed.span = step.span;
                    return Some(failed);
                }
            }
            Planned::Check(check) => {
                if let Some(mut failed) = check_holds(running, check, &plan.types) {
                    failed.span = step.span;
                    return Some(failed);
                }
            }
        }
    }
    None
}

/// Sets up a fake table, as a `given` fact says.
fn given(running: &Running<'_>, fact: &PlannedFact) -> Result<(), String> {
    match fact {
        PlannedFact::Entry {
            grant, key, value, ..
        } => {
            let stored = json::encode(value).to_string();
            table(running, grant).set(key, stored.as_bytes())
        }
        PlannedFact::Empty { grant, .. } => table(running, grant).clear(),
        PlannedFact::Fails { grant } => {
            table(running, grant).fails = true;
            Ok(())
        }
    }
}

fn table<'r>(
    running: &'r Running<'_>,
    grant: &str,
) -> std::sync::MutexGuard<'r, inset_host::Table> {
    running
        .table(grant)
        .expect("the checker names only the app's table grants")
}

/// Checks a `then` fact against what a fake table holds.
fn then(running: &Running<'_>, fact: &PlannedFact, types: &Types) -> Option<Failed> {
    let failed = |message: String| Some(Failed::new(Span::new(0, 0), message));
    match fact {
        PlannedFact::Entry {
            grant,
            key,
            key_value,
            value,
            shape,
        } => {
            let stored = match table(running, grant).get(key) {
                Ok(stored) => stored,
                Err(e) => return failed(format!("but `{grant}` could not be read: {e}")),
            };
            let Some(stored) = stored else {
                return failed(format!("but `{grant}` holds nothing under {key_value}"));
            };
            let actual = match read(&stored, shape, types) {
                Ok(actual) => actual,
                Err(e) => return failed(format!("but {grant}[{key_value}] {e}")),
            };
            if actual == *value {
                return None;
            }
            Some(Failed {
                span: Span::new(0, 0),
                message: format!(
                    "but {grant}[{key_value}] is another value\n  expected: {value}\n  actual:   {actual}"
                ),
                values: Some((value.to_string(), actual.to_string())),
            })
        }
        PlannedFact::Empty {
            grant,
            key_shape,
            shape,
        } => {
            let entries = match table(running, grant).entries() {
                Ok(entries) => entries,
                Err(e) => return failed(format!("but `{grant}` could not be read: {e}")),
            };
            if entries.is_empty() {
                return None;
            }
            let count = entries.len();
            let mut message = format!(
                "but `{grant}` holds {count} {}",
                if count == 1 { "entry" } else { "entries" }
            );
            for (key, value) in entries.iter().take(5) {
                let key = key::decode(key, key_shape).map_or_else(|e| e, |k| k.to_string());
                let value = read(value, shape, types).map_or_else(|e| e, |v| v.to_string());
                message.push_str(&format!("\n  {grant}[{key}] is {value}"));
            }
            if count > 5 {
                message.push_str(&format!("\n  and {} more", count - 5));
            }
            failed(message)
        }
        PlannedFact::Fails { .. } => unreachable!("the checker allows `fails` after `given` only"),
    }
}

/// Reads a stored value, or says why it cannot be read.
fn read(stored: &[u8], shape: &Shape, types: &Types) -> Result<Value, String> {
    let json: serde_json::Value =
        serde_json::from_slice(stored).map_err(|e| format!("is not stored as JSON: {e}"))?;
    json::decode(&json, shape, types).map_err(|e| format!("does not fit its type: {e}"))
}

/// Evaluates a check against what the fakes hold.
fn check_holds(running: &Running<'_>, check: &Check, types: &Types) -> Option<Failed> {
    let failed = |message: String| Some(Failed::new(Span::new(0, 0), message));
    let mut lookup = |grant: &str, key: &[u8]| table(running, grant).get(key).ok().flatten();
    match check.eval(&mut lookup, types) {
        Ok(Value::Bool(true)) => None,
        Ok(_) => {
            // A comparison shows both of its sides, as `assert!` does not.
            if let Check::Binary(op, lhs, rhs) = check
                && !matches!(op, Op::And | Op::Or)
            {
                let left = lhs.eval(&mut lookup, types).ok()?;
                let right = rhs.eval(&mut lookup, types).ok()?;
                return failed(format!(
                    "but it is false\n  left:  {left}\n  right: {right}"
                ));
            }
            failed("but it is false".into())
        }
        Err(CheckError::Missing { grant, key }) => {
            failed(format!("but `{grant}` holds nothing under {key}"))
        }
        Err(CheckError::Failed(e)) => failed(format!("but it could not be worked out: {e}")),
    }
}

/// A reply as a step would state it: `OK with "Hello, Ada!"`.
fn describe(response: &Response) -> String {
    let status = STATUSES
        .iter()
        .find(|(_, code)| *code == response.status)
        .map_or_else(|| response.status.to_string(), |(name, _)| name.to_string());
    if response.body.is_empty() {
        format!("{status} with an empty body")
    } else {
        match std::str::from_utf8(&response.body) {
            Ok(text) => format!("{status} with {text:?}"),
            Err(_) => format!(
                "{status} with {} bytes that are not text",
                response.body.len()
            ),
        }
    }
}

/// A step as written, with the indentation of its step line taken off any
/// lines it continues onto.
fn step_text(source: &str, span: Span) -> String {
    let text = &source[span.start..span.end];
    let mut lines = text.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(line.strip_prefix("    ").unwrap_or(line));
    }
    out
}

/// Where a step failed, the step, and what happened instead, indented.
fn failed(out: &mut String, file: &str, failure: &Failure, indent: &str) {
    let detail = format!(
        "{file}:{}\n{}\n{}",
        failure.line, failure.step, failure.message
    );
    for line in detail.lines() {
        out.push_str(&format!("{indent}{line}\n"));
    }
}

/// An examples table as text, its headings first, with each column as
/// wide as its widest cell.
fn examples_table(headings: &[String], rows: &[RowOutcome]) -> Vec<String> {
    if headings.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&[String]> = std::iter::once(headings)
        .chain(rows.iter().map(|r| r.cells.as_slice()))
        .collect();
    let widths: Vec<usize> = (0..headings.len())
        .map(|i| {
            lines
                .iter()
                .map(|cells| cells.get(i).map_or(0, |c| c.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    lines
        .iter()
        .map(|cells| {
            let cells: Vec<String> = cells
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect()
}

/// The report as `inset test` prints it.
pub fn render(report: &TestReport) -> String {
    let mut out = String::new();
    for outcome in &report.requirements {
        let result = if outcome.result == "fail" {
            "FAIL"
        } else {
            "pass"
        };
        out.push_str(&format!("{result}  {}\n", outcome.sentence));
        if let Some(failure) = &outcome.failure {
            failed(&mut out, &outcome.file, failure, "      ");
        }
        let table = examples_table(&outcome.headings, &outcome.rows);
        if let Some(headings) = table.first() {
            out.push_str(&format!("            {headings}\n"));
        }
        for (row, text) in outcome.rows.iter().zip(table.iter().skip(1)) {
            let result = if row.result == "fail" { "FAIL" } else { "pass" };
            out.push_str(&format!("      {result}  {text}\n"));
            if let Some(failure) = &row.failure {
                failed(&mut out, &outcome.file, failure, "            ");
            }
        }
    }
    let total = report.requirements.len();
    let noun = if total == 1 {
        "requirement"
    } else {
        "requirements"
    };
    if total > 0 {
        out.push('\n');
    }
    out.push_str(&match (report.passed, report.failed) {
        (0, 0) => format!("{} has no requirements\n", report.app),
        (_, 0) => format!("{total} {noun} passed\n"),
        (0, _) => format!("{total} {noun} failed\n"),
        (passed, failed) => format!("{total} {noun}: {passed} passed, {failed} failed\n"),
    });
    out
}
