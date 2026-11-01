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
//! each row, each time afresh, and reports each row's result. Its steps run
//! in order, and the first that does not hold fails it. A fact that does not hold reports both values, written as
//! Rust's `{:?}` writes them, as `assert_eq!` does. The report is the same
//! every time for the same program, so it holds no timings.
//!
//! **Properties.** A requirement with `for all` is run with proptest, with
//! a fixed number of cases and a seed made from its sentence, so its report
//! is the same every time. Each case's values are written as Rust writes
//! them and put in place of the placeholders, and the steps are read and
//! planned again with them, by the checker's [`Cases`], before they run. A
//! failing case is shrunk to the smallest proptest finds that still fails,
//! and that case is run once more, for the step and values to report.
//!
//! **Coverage.** The program reports which elements of its diagram each
//! case reached, and each element is credited to the requirement whose
//! case reached it, so a requirement with an examples table is credited
//! with what any of its rows reached, and a property with what any of its
//! cases reached. A requirement that fails is credited
//! too: its sentence still says the code is wanted. An element marked
//! `#[defensive]` that a requirement reached fails the run, because the
//! reason given for it is then wrong.

use std::collections::BTreeMap;
use std::path::Path;

use inset_check::status::STATUSES;
use inset_check::{
    Cases, Plan, Planned, PlannedBody, PlannedFact, PlannedRequirement, PlannedStep, Program,
};
use inset_host::{Mode, Options, Request, Response, Running};
use inset_syntax::Span;
use inset_value::{Check, CheckError, Decimal, Op, Shape, Types, Value, json, key};
use serde::Serialize;

/// What `inset test --json` prints.
#[derive(Debug, Serialize)]
pub struct TestReport {
    pub app: String,
    pub passed: usize,
    pub failed: usize,
    /// In order of file and then of position.
    pub requirements: Vec<Outcome>,
    /// Code marked `#[defensive]` that a requirement reached, in the
    /// order of the diagram.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defensive: Vec<Reached>,
    /// Which requirements reached each element of the diagram, by ID.
    #[serde(skip)]
    pub coverage: inset_diagram::model::Coverage,
    /// Each element coverage counts, with where it is, when the program has
    /// a diagram.
    #[serde(skip)]
    pub elements: Vec<(inset_diagram::coverage::Element, String)>,
}

/// An element marked `#[defensive]`, which a requirement reached.
#[derive(Debug, Serialize)]
pub struct Reached {
    /// The element's diagram ID.
    pub id: String,
    /// Where it is, as `src/main.inset:12`.
    pub at: String,
    /// The reason the `#[defensive]` gives.
    pub reason: String,
    /// The sentences of the requirements that reached it.
    pub requirements: Vec<String>,
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
    /// For a property, how many generated cases ran and held, not counting
    /// those its `where` condition left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cases: Option<u32>,
    /// For a property that failed, the smallest input found that breaks
    /// it, with a value for each of its inputs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputValue>,
}

/// One input of a property's failing case.
#[derive(Debug, Serialize)]
pub struct InputValue {
    pub name: String,
    /// As it is read in the steps: as Rust writes the value.
    pub value: String,
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
    inset_check::plan_with(program, |plan, cases| {
        let plan =
            plan.map_err(|_| "the program has errors, so its requirements cannot run".to_string())?;
        run_plan(program, component, &plan, cases)
    })
}

fn run_plan(
    program: &Program,
    component: &Path,
    plan: &Plan,
    cases: &Cases<'_, '_>,
) -> Result<TestReport, String> {
    let host = inset_host::Program::load(component).map_err(|e| e.to_string())?;
    let working_dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let options = Options {
        settings: Vec::new(),
        mode: Mode::Fake,
        working_dir,
    };
    let mut requirements = Vec::new();
    let mut reached: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for requirement in &plan.requirements {
        let file = &program.files[requirement.file];
        let line = |span: Span| file.text[..span.start].matches('\n').count() + 1;
        // Each case starts the program afresh.
        let mut run_case = |steps: &[PlannedStep]| -> Result<Option<Failure>, String> {
            let mut running = host.start(&options).map_err(|e| e.to_string())?;
            let failed = run_steps(plan, steps, &mut running);
            for id in running.reached() {
                let by = reached.entry(id.clone()).or_default();
                if !by.contains(&requirement.sentence) {
                    by.push(requirement.sentence.clone());
                }
            }
            Ok(failed.map(|failed| {
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
        let mut property = None;
        let (failure, headings, rows) = match &requirement.examples {
            None if requirement.property.is_some() => {
                let outcome = run_property(plan, cases, requirement, &mut run_case)?;
                let failure = outcome.failure;
                property = Some((outcome.cases, outcome.inputs));
                (failure, Vec::new(), Vec::new())
            }
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
            cases: property.as_ref().map(|(cases, _)| *cases),
            inputs: property.map(|(_, inputs)| inputs).unwrap_or_default(),
        });
    }
    let failed = requirements.iter().filter(|r| r.result == "fail").count();
    let coverage = inset_diagram::model::Coverage { reached };
    // A program whose diagram cannot be drawn yet has no elements to check.
    let elements: Vec<_> = match inset_diagram::emit_with_sources(program) {
        Ok(emitted) => inset_diagram::coverage::elements(&emitted.diagram)
            .into_iter()
            .map(|element| {
                let at = emitted
                    .sources
                    .get(&element.id)
                    .map_or_else(String::new, |source| {
                        let file = &program.files[source.file];
                        let line = file.text[..source.span.start].matches('\n').count() + 1;
                        format!("{}:{line}", file.path)
                    });
                (element, at)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let defensive = elements
        .iter()
        .filter_map(|(element, at)| {
            let reason = element.defensive.clone()?;
            let requirements = coverage.reached.get(&element.id)?.clone();
            Some(Reached {
                id: element.id.clone(),
                at: at.clone(),
                reason,
                requirements,
            })
        })
        .collect();
    Ok(TestReport {
        app: host.manifest().app.clone(),
        passed: requirements.len() - failed,
        failed,
        requirements,
        defensive,
        coverage,
        elements,
    })
}

/// How many cases a property generates that its `where` condition keeps.
/// Fixed, with a seed of its own for each property, so that the report is
/// the same every time.
pub const PROPERTY_CASES: u32 = 100;

/// Runs one case's steps against a freshly started program, and returns
/// the step that did not hold, if one did.
type RunCase<'r> = dyn FnMut(&[PlannedStep]) -> Result<Option<Failure>, String> + 'r;

/// What running a property found.
struct PropertyOutcome {
    cases: u32,
    inputs: Vec<InputValue>,
    failure: Option<Failure>,
}

/// Runs a property: generates values for its inputs, keeps those its
/// `where` condition holds for, and runs its steps with each, until
/// [`PROPERTY_CASES`] have held or one fails. Proptest then shrinks a
/// failing case to the smallest it can find that still fails, and that
/// case is run again for the report.
fn run_property(
    plan: &Plan,
    cases: &Cases<'_, '_>,
    requirement: &PlannedRequirement,
    run_case: &mut RunCase<'_>,
) -> Result<PropertyOutcome, String> {
    use proptest::strategy::BoxedStrategy;
    use proptest::test_runner::{
        Config, RngAlgorithm, RngSeed, TestCaseError, TestError, TestRng, TestRunner,
    };
    use std::cell::{Cell, RefCell};

    let property = requirement
        .property
        .as_ref()
        .expect("only a property is run as one");
    let config = Config {
        cases: PROPERTY_CASES,
        max_local_rejects: PROPERTY_CASES * 100,
        max_global_rejects: PROPERTY_CASES * 100,
        failure_persistence: None,
        fork: false,
        timeout: 0,
        max_shrink_time: 0,
        max_shrink_iters: 1024,
        verbose: 0,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(0),
        ..Config::default()
    };
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &seed(&requirement.sentence));
    let mut runner = TestRunner::new_with_rng(config, rng);
    let strategy: Vec<BoxedStrategy<Value>> = property
        .inputs
        .iter()
        .map(|input| generate(&input.shape))
        .collect();
    let run_case = RefCell::new(run_case);
    let error = RefCell::new(None);
    // The cases that held before the first that failed: shrinking runs
    // more, which do not count.
    let held = Cell::new(0);
    let failed = Cell::new(false);
    // A case's steps, or why its values do not make steps that check.
    let steps = |values: &[Value]| -> Result<Vec<PlannedStep>, String> {
        let texts: Vec<String> = values.iter().map(value_text).collect();
        cases
            .property(requirement, &texts)
            .map_err(|d| format!("the steps do not check with these values: {}", d.message))
    };
    let result = runner.run(&strategy, |values| {
        if let Some(condition) = &property.condition {
            match condition.eval_inputs(&values, &plan.types) {
                Ok(Value::Bool(true)) => {}
                Ok(_) => return Err(TestCaseError::reject("the `where` condition is false")),
                Err(CheckError::Failed(e) | CheckError::Missing { grant: e, .. }) => {
                    return Err(TestCaseError::fail(format!(
                        "but the `where` condition could not be worked out: {e}"
                    )));
                }
            }
        }
        let steps = steps(&values).map_err(|e| {
            failed.set(true);
            TestCaseError::fail(e)
        })?;
        match (run_case.borrow_mut())(&steps) {
            Ok(None) => {
                if !failed.get() {
                    held.set(held.get() + 1);
                }
                Ok(())
            }
            Ok(Some(failure)) => {
                failed.set(true);
                Err(TestCaseError::fail(failure.message))
            }
            Err(e) => {
                error.replace(Some(e));
                Err(TestCaseError::fail("the program could not start"))
            }
        }
    });
    if let Some(e) = error.into_inner() {
        return Err(e);
    }
    let run_case = run_case.into_inner();
    let names = || property.inputs.iter().map(|input| input.name.clone());
    let file = &cases.program().files[requirement.file].text;
    // A failure of the property as a whole, rather than of one of its steps.
    let at_for_all = |message: String| Failure {
        line: file[..property.span.start].matches('\n').count() + 1,
        step: step_text(file, property.span),
        message,
        expected: None,
        actual: None,
    };
    Ok(match result {
        Ok(()) => PropertyOutcome {
            cases: PROPERTY_CASES,
            inputs: Vec::new(),
            failure: None,
        },
        Err(TestError::Fail(reason, values)) => {
            let inputs = names()
                .zip(&values)
                .map(|(name, value)| InputValue {
                    name,
                    value: value_text(value),
                })
                .collect();
            // Run the smallest failing case again, for the step that failed.
            let failure = match steps(&values) {
                Ok(steps) => run_case(&steps)?,
                Err(message) => Some(at_for_all(message)),
            };
            let failure = failure.unwrap_or_else(|| at_for_all(reason.message().to_string()));
            PropertyOutcome {
                cases: held.get(),
                inputs,
                failure: Some(failure),
            }
        }
        Err(TestError::Abort(reason)) => PropertyOutcome {
            cases: held.get(),
            inputs: Vec::new(),
            failure: Some(at_for_all(format!(
                "but only {} generated cases met the `where` condition before too many were \
                 left out: {}",
                held.get(),
                reason.message()
            ))),
        },
    })
}

/// A property's seed, from its sentence, so each property generates values
/// of its own, and the same ones every time.
fn seed(sentence: &str) -> [u8; 32] {
    // FNV-1a, spread over the seed's bytes.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut seed = [0; 32];
    for (i, chunk) in seed.chunks_mut(8).enumerate() {
        for byte in sentence.bytes().chain([i as u8]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        chunk.copy_from_slice(&hash.to_le_bytes());
    }
    seed
}

/// The values a property generates for an input of shape `shape`. Small
/// values come up more often than the type's range alone would give them,
/// since that is where the edge cases are, and every value shrinks
/// towards zero.
fn generate(shape: &Shape) -> proptest::strategy::BoxedStrategy<Value> {
    use proptest::prelude::*;
    match shape {
        Shape::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        Shape::Int { signed, bits } => {
            let (min, max): (i128, i128) = if *signed {
                (-(1 << (bits - 1)), (1 << (bits - 1)) - 1)
            } else {
                (0, (1 << bits) - 1)
            };
            prop_oneof![
                3 => min.max(-100)..=max.min(100),
                1 => min..=max,
            ]
            .prop_map(Value::Int)
            .boxed()
        }
        Shape::Decimal => {
            let mantissa = prop_oneof![
                3 => -10_000i64..=10_000,
                1 => any::<i64>(),
            ];
            // A scale up to 28 digits, as `rust_decimal` allows, mostly small.
            let scale = prop_oneof![4 => 0u32..=4, 1 => 0u32..=28];
            (mantissa, scale)
                .prop_map(|(m, s)| Value::Decimal(Decimal::new(m, s)))
                .boxed()
        }
        _ => unreachable!("the checker generates only integers, `bool` and `Decimal`"),
    }
}

/// A generated value as Rust writes it, which is how it reads in a step:
/// `-12`, `true` or `0.10`.
fn value_text(value: &Value) -> String {
    match value {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Decimal(d) => d.to_string(),
        other => other.to_string(),
    }
}

/// How much of the program the requirements reached, with each element
/// none reached, as `inset test --coverage` prints it.
pub fn render_coverage(report: &TestReport) -> String {
    let mut reached = 0;
    let mut excused = 0;
    let mut missed = Vec::new();
    for (element, at) in &report.elements {
        if report.coverage.reached.contains_key(&element.id) {
            reached += 1;
        } else if element.defensive.is_some() || element.inside_defensive {
            excused += 1;
        } else {
            missed.push((&element.id, at));
        }
    }
    let total = report.elements.len();
    let mut out = format!(
        "\ncoverage: {reached} of {total} reached, {excused} excused, {} not reached\n",
        missed.len()
    );
    for (id, at) in missed {
        out.push_str(&format!("  not reached  {id}  {at}\n"));
    }
    out
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
                let body = match body {
                    PlannedBody::Text(text) => text.clone().into_bytes(),
                    PlannedBody::Json { value, shape } => {
                        match inset_value::serde::encode(value, shape, &plan.types) {
                            Ok(json) => json.to_string().into_bytes(),
                            Err(e) => {
                                return fail(format!("but the body cannot be sent as JSON: {e}"));
                            }
                        }
                    }
                };
                let request = Request {
                    method: method.clone(),
                    path: path.clone(),
                    body,
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
                let mut failed = match body {
                    _ if response.status != *status => Some(Failed::new(
                        step.span,
                        format!("but the reply was {}", describe(response)),
                    )),
                    PlannedBody::Text(text) if response.body != text.as_bytes() => {
                        Some(Failed::new(
                            step.span,
                            format!("but the reply was {}", describe(response)),
                        ))
                    }
                    PlannedBody::Text(_) => None,
                    PlannedBody::Json { value, shape } => {
                        json_reply(response, value, shape, &plan.types).map(|mut failed| {
                            failed.span = step.span;
                            failed
                        })
                    }
                };
                if let Some(failed) = &mut failed {
                    for report in &running.reports()[reports_before..] {
                        failed.message.push_str(&format!("\nafter {report}"));
                    }
                }
                if failed.is_some() {
                    return failed;
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

/// Compares a JSON reply with the value a step expects, as the type's
/// `PartialEq` would, once the reply is read back as the route's type.
fn json_reply(
    response: &Response,
    expected: &Value,
    shape: &Shape,
    types: &Types,
) -> Option<Failed> {
    let failed = |message: String| Some(Failed::new(Span::new(0, 0), message));
    let json: serde_json::Value = match serde_json::from_slice(&response.body) {
        Ok(json) => json,
        Err(e) => {
            return failed(format!(
                "but the reply was {}, which is not JSON: {e}",
                describe(response)
            ));
        }
    };
    let actual = match inset_value::serde::decode(&json, shape, types) {
        Ok(actual) => actual,
        Err(e) => {
            return failed(format!(
                "but the reply was {}, which does not fit its type: {e}",
                describe(response)
            ));
        }
    };
    if actual == *expected {
        return None;
    }
    Some(Failed {
        span: Span::new(0, 0),
        message: format!(
            "but the reply is another value\n  expected: {expected}\n  actual:   {actual}"
        ),
        values: Some((expected.to_string(), actual.to_string())),
    })
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
    let mut out = render_requirements(report);
    for reached in &report.defensive {
        out.push_str(&format!(
            "FAIL  defensive code was reached: {}\n      {}\n      #[defensive({:?})]\n",
            reached.id, reached.at, reached.reason
        ));
        let by: Vec<String> = reached
            .requirements
            .iter()
            .map(|sentence| format!("{sentence:?}"))
            .collect();
        out.push_str(&format!("      but {} reached it\n", by.join(" and ")));
    }
    out.push_str(&summary(report));
    out
}

fn render_requirements(report: &TestReport) -> String {
    let mut out = String::new();
    for outcome in &report.requirements {
        let result = if outcome.result == "fail" {
            "FAIL"
        } else {
            "pass"
        };
        out.push_str(&format!("{result}  {}\n", outcome.sentence));
        if let Some(cases) = outcome.cases {
            if outcome.failure.is_none() {
                out.push_str(&format!("      held in {}\n", count(cases, "case")));
            } else if !outcome.inputs.is_empty() {
                let inputs: Vec<String> = outcome
                    .inputs
                    .iter()
                    .map(|i| format!("{} = {}", i.name, i.value))
                    .collect();
                out.push_str(&format!(
                    "      fails for {}, the smallest failing input found\n",
                    inputs.join(", ")
                ));
            }
        }
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
    out
}

/// The last lines: how many requirements passed and failed, and how much
/// defensive code was reached.
fn summary(report: &TestReport) -> String {
    let mut out = String::new();
    let total = report.requirements.len();
    let noun = if total == 1 {
        "requirement"
    } else {
        "requirements"
    };
    if total > 0 || !report.defensive.is_empty() {
        out.push('\n');
    }
    out.push_str(&match (report.passed, report.failed) {
        (0, 0) => format!("{} has no requirements\n", report.app),
        (_, 0) => format!("{total} {noun} passed\n"),
        (0, _) => format!("{total} {noun} failed\n"),
        (passed, failed) => format!("{total} {noun}: {passed} passed, {failed} failed\n"),
    });
    match report.defensive.len() {
        0 => {}
        1 => out.push_str("1 element marked defensive was reached\n"),
        n => out.push_str(&format!("{n} elements marked defensive were reached\n")),
    }
    out
}

/// `n` and a noun, plural unless `n` is 1.
fn count(n: u32, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
