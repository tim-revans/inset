//! What coverage counts in a diagram, and how each element stands once the
//! requirements have run.
//!
//! The elements are those `syntax.md` counts: entering each function with a
//! body, each branch of a decision or loop, each error path of a `?`, and
//! each arm of a `failure` block. The emitted code marks the same elements
//! by the same IDs, and a test in `inset-codegen` keeps the two in step.
//!
//! An element is covered when some requirement reached it. One marked
//! `#[defensive]` that nothing reached is excused, as is everything inside
//! a branch that is excused, because nothing can reach it without reaching
//! the branch. A defensive element that a requirement did reach is
//! contradicted: the reason given for it is wrong, and the test report
//! says so. Anything else is uncovered.

use std::collections::BTreeMap;

use crate::model::{Component, Coverage, Diagram, Function, Id, Step};
use crate::svg::{Canvas, LH, Seg, chars};

/// How an element stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// A requirement reached it.
    Covered,
    /// No requirement reached it, and nothing excuses it.
    Uncovered,
    /// No requirement reached it, and `#[defensive]` says none can.
    Excused,
    /// It is marked `#[defensive]`, but a requirement reached it.
    Contradicted,
}

/// An element coverage counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub id: Id,
    /// The reason of the `#[defensive]` on the element itself.
    pub defensive: Option<String>,
    /// Whether a branch around it is marked `#[defensive]`.
    pub inside_defensive: bool,
}

/// Every element of the diagram that coverage counts, in the order the
/// diagram lists them.
pub fn elements(diagram: &Diagram) -> Vec<Element> {
    let mut out = Vec::new();
    for component in &diagram.app.components {
        component_elements(component, &mut out);
    }
    for function in &diagram.algorithms {
        function_elements(function, &mut out);
    }
    out
}

fn component_elements(component: &Component, out: &mut Vec<Element>) {
    for arm in &component.failure {
        out.push(Element {
            id: arm.id.clone(),
            defensive: arm.defensive.clone(),
            inside_defensive: false,
        });
    }
    let handlers = component.routes.iter().map(|r| &r.handler);
    for function in handlers
        .chain(&component.messages)
        .chain(&component.algorithms)
    {
        function_elements(function, out);
    }
    for child in &component.components {
        component_elements(child, out);
    }
}

fn function_elements(function: &Function, out: &mut Vec<Element>) {
    let Some(body) = &function.body else {
        return;
    };
    out.push(Element {
        id: function.id.clone(),
        defensive: None,
        inside_defensive: false,
    });
    step_elements(body, false, out);
}

fn step_elements(steps: &[Step], inside: bool, out: &mut Vec<Element>) {
    for step in steps {
        let branches = match step {
            Step::Action { exit, .. } => {
                if let Some(exit) = exit {
                    out.push(Element {
                        id: exit.id.clone(),
                        defensive: exit.defensive.clone(),
                        inside_defensive: inside,
                    });
                }
                continue;
            }
            Step::Return { .. } => continue,
            Step::Decision { branches, .. } | Step::Fork { branches, .. } => {
                branches.iter().collect::<Vec<_>>()
            }
            Step::Loop { body, skip, .. } => vec![body, skip],
        };
        for branch in branches {
            out.push(Element {
                id: branch.id.clone(),
                defensive: branch.defensive.clone(),
                inside_defensive: inside,
            });
            step_elements(&branch.steps, inside || branch.defensive.is_some(), out);
        }
    }
}

/// How each element stands, by ID, given which requirements reached what.
pub fn statuses(diagram: &Diagram, coverage: &Coverage) -> BTreeMap<Id, Status> {
    elements(diagram)
        .into_iter()
        .map(|element| {
            let reached = coverage.reached.contains_key(&element.id);
            let status = match (reached, element.defensive.is_some()) {
                (true, true) => Status::Contradicted,
                (true, false) => Status::Covered,
                (false, true) => Status::Excused,
                (false, false) if element.inside_defensive => Status::Excused,
                (false, false) => Status::Uncovered,
            };
            (element.id, status)
        })
        .collect()
}

/// Counts of each status among the elements under `prefix`: the elements
/// of a component and its children, or of a function.
pub(crate) fn tally(statuses: &BTreeMap<Id, Status>, prefix: &str) -> Tally {
    let mut tally = Tally::default();
    let under = format!("{prefix}/");
    for (id, status) in statuses {
        if id == prefix || id.starts_with(&under) {
            match status {
                Status::Covered => tally.covered += 1,
                Status::Uncovered => tally.uncovered += 1,
                Status::Excused => tally.excused += 1,
                Status::Contradicted => tally.contradicted += 1,
            }
        }
    }
    tally
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tally {
    pub covered: usize,
    pub uncovered: usize,
    pub excused: usize,
    pub contradicted: usize,
}

impl Tally {
    /// The status that stands out most: a wrong excuse, then anything
    /// uncovered, then anything excused, and otherwise covered.
    pub(crate) fn worst(self) -> Option<Status> {
        if self.contradicted > 0 {
            Some(Status::Contradicted)
        } else if self.uncovered > 0 {
            Some(Status::Uncovered)
        } else if self.excused > 0 {
            Some(Status::Excused)
        } else if self.covered > 0 {
            Some(Status::Covered)
        } else {
            None
        }
    }
}

impl Status {
    /// The class that colours an element with this status.
    pub(crate) fn class(self) -> &'static str {
        match self {
            Status::Covered => "cov-covered",
            Status::Uncovered => "cov-uncovered",
            Status::Excused => "cov-excused",
            Status::Contradicted => "cov-contradicted",
        }
    }

    /// The mark beside a row, which says what the colour says.
    pub(crate) fn mark(self) -> &'static str {
        match self {
            Status::Covered => "✓",
            Status::Uncovered => "✗",
            Status::Excused => "~",
            Status::Contradicted => "!",
        }
    }

    /// Whether the element needs the reader's attention.
    pub(crate) fn stands_out(self) -> bool {
        matches!(self, Status::Uncovered | Status::Contradicted)
    }
}

/// The key to the marks, written under a picture that shows coverage.
/// Returns the height it takes.
pub(crate) fn legend(canvas: &mut Canvas, x: f64, y: f64) -> f64 {
    let mut segs = Vec::new();
    for (status, text) in [
        (Status::Covered, " reached  "),
        (Status::Uncovered, " not reached  "),
        (Status::Excused, " excused by #[defensive]  "),
        (Status::Contradicted, " defensive, yet reached"),
    ] {
        segs.push(Seg::new(status.mark(), status.class()));
        segs.push(Seg::new(text, "dim"));
    }
    canvas.text(x, y + 12.0, &segs);
    LH + 4.0
}

/// The width of the legend.
pub(crate) fn legend_w() -> f64 {
    chars("✓ reached  ✗ not reached  ~ excused by #[defensive]  ! defensive, yet reached")
}

/// Where `inset test --coverage` writes the coverage of the program in
/// `dir`, which `inset diagram --coverage` and an editor read.
pub fn file(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("target").join("inset").join("coverage.json")
}

/// Reads the coverage `inset test --coverage` last wrote for the program in
/// `dir`, and gives it to the program's diagram. Coverage that names an
/// element the diagram does not have is from another version of the
/// program, and is refused.
pub fn load(diagram: &mut Diagram, dir: &std::path::Path) -> Result<(), String> {
    let path = file(dir);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "there is no coverage in {}: run `inset test --coverage` first",
            path.display()
        )
    })?;
    let coverage: Coverage = serde_json::from_str(&text)
        .map_err(|e| format!("cannot read the coverage in {}: {e}", path.display()))?;
    let known: std::collections::BTreeSet<Id> =
        elements(diagram).into_iter().map(|e| e.id).collect();
    if let Some(id) = coverage.reached.keys().find(|id| !known.contains(*id)) {
        return Err(format!(
            "the coverage in {} is from another version of the program, which had `{id}`: \
             run `inset test --coverage` again",
            path.display()
        ));
    }
    diagram.coverage = Some(coverage);
    Ok(())
}
