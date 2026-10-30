//! Runs a program's requirements against its built component, in the host
//! with a fake behind every grant, and reports each by its sentence.
//!
//! Each requirement starts the program afresh, so no requirement depends on
//! another having run. Its steps run in order, and the first that does not
//! hold fails it. The report is the same every time for the same program,
//! so it holds no timings.

use std::path::Path;

use inset_check::Program;
use inset_check::status::STATUSES;
use inset_host::{Mode, Options, Request, Response};
use inset_syntax::ast::{Requirement, StepKind};
use inset_syntax::{Span, is_requirements_file, parse_requirements};
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
}

/// Runs every requirement of `program` against the component built from it.
pub fn run(program: &Program, component: &Path) -> Result<TestReport, String> {
    let host = inset_host::Program::load(component).map_err(|e| e.to_string())?;
    let servers: Vec<&str> = host
        .manifest()
        .grants
        .iter()
        .filter(|g| g.capability == "HttpServer")
        .map(|g| g.name.as_str())
        .collect();
    let working_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let options = Options {
        settings: Vec::new(),
        mode: Mode::Fake,
        working_dir,
    };
    let mut requirements = Vec::new();
    for file in &program.files {
        if !file.path.starts_with("requirements/") || !is_requirements_file(&file.path) {
            continue;
        }
        let line = |span: Span| file.text[..span.start].matches('\n').count() + 1;
        for requirement in parse_requirements(&file.text).ast.requirements {
            let mut running = host.start(&options).map_err(|e| e.to_string())?;
            let failure =
                steps(&requirement, &mut running, &servers).map(|(span, message)| Failure {
                    line: line(span),
                    step: step_text(&file.text, span),
                    message,
                });
            requirements.push(Outcome {
                sentence: requirement.sentence.value.clone(),
                file: file.path.clone(),
                line: line(requirement.span),
                result: if failure.is_some() { "fail" } else { "pass" },
                failure,
            });
        }
    }
    let failed = requirements.iter().filter(|r| r.failure.is_some()).count();
    Ok(TestReport {
        app: host.manifest().app.clone(),
        passed: requirements.len() - failed,
        failed,
        requirements,
    })
}

/// Runs a requirement's steps, and returns the first that does not hold,
/// with what happened instead.
fn steps(
    requirement: &Requirement,
    running: &mut inset_host::Running<'_>,
    servers: &[&str],
) -> Option<(Span, String)> {
    let mut last: Option<Response> = None;
    let mut reports_before = 0;
    for step in &requirement.steps {
        match &step.kind {
            StepKind::Request {
                method,
                path,
                via,
                body,
                ..
            } => {
                // The checker made sure a request names its server when
                // there is more than one.
                let server = via.as_ref().map_or(servers[0], |v| v.name.as_str());
                let request = Request {
                    method: method.as_str().into(),
                    path: path.value.clone(),
                    body: body
                        .as_ref()
                        .map_or_else(Vec::new, |b| b.value.clone().into_bytes()),
                };
                reports_before = running.reports().len();
                match running.request(server, &request) {
                    Ok(response) => last = Some(response),
                    Err(stopped) => {
                        let message = format!(
                            "the program stopped before it replied: {}",
                            inset_host::describe(&stopped)
                        );
                        return Some((step.span, message));
                    }
                }
            }
            StepKind::Reply { status, body, .. } => {
                let response = last.as_ref().expect("the checker puts a request first");
                let code = inset_check::status::code(&status.name)
                    .expect("the checker accepts only known statuses");
                let expected = body.as_ref().map_or("", |b| b.value.as_str());
                if response.status != code || response.body != expected.as_bytes() {
                    let mut message = format!("but the reply was {}", describe(response));
                    for report in &running.reports()[reports_before..] {
                        message.push_str(&format!("\nafter {report}"));
                    }
                    return Some((step.span, message));
                }
            }
        }
    }
    None
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

/// The report as `inset test` prints it.
pub fn render(report: &TestReport) -> String {
    let mut out = String::new();
    for outcome in &report.requirements {
        match &outcome.failure {
            None => out.push_str(&format!("pass  {}\n", outcome.sentence)),
            Some(failure) => {
                out.push_str(&format!("FAIL  {}\n", outcome.sentence));
                let detail = format!(
                    "{}:{}\n{}\n{}",
                    outcome.file, failure.line, failure.step, failure.message
                );
                for line in detail.lines() {
                    out.push_str(&format!("      {line}\n"));
                }
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
