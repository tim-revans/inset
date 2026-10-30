//! Checks that diagram data is consistent: every ID follows the rules, is
//! unique, and every reference names an element of the right kind. The JSON
//! Schema checks shape only, so a hand-written or generated file needs both.

use std::collections::BTreeMap;

use crate::model::*;

/// What kind of element an ID names, for checking references.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    App,
    Setting,
    Grant,
    FrontEnd,
    Component,
    Field(FieldKind),
    Failure,
    Function { message: bool },
    Step,
    Branch,
    Exit,
    Edge,
}

/// Every element by ID, with its kind and, for elements inside a component, the component's ID.
pub(crate) struct Index<'a> {
    pub kinds: BTreeMap<&'a str, Kind>,
    pub owners: BTreeMap<&'a str, &'a str>,
    pub components: BTreeMap<&'a str, &'a Component>,
    pub functions: BTreeMap<&'a str, &'a Function>,
    pub fields: BTreeMap<&'a str, &'a Field>,
    pub errors: Vec<String>,
}

impl<'a> Index<'a> {
    pub fn new(diagram: &'a Diagram) -> Self {
        let mut index = Index {
            kinds: BTreeMap::new(),
            owners: BTreeMap::new(),
            components: BTreeMap::new(),
            functions: BTreeMap::new(),
            fields: BTreeMap::new(),
            errors: Vec::new(),
        };
        let app = &diagram.app;
        index.add(&app.id, &app.name, Kind::App);
        for setting in &app.settings {
            index.add(
                &setting.id,
                &format!("{}/setting:{}", app.id, setting.name),
                Kind::Setting,
            );
        }
        for grant in &app.grants {
            index.add(
                &grant.id,
                &format!("{}/grant:{}", app.id, grant.name),
                Kind::Grant,
            );
        }
        if let Some(front_end) = &app.front_end {
            index.add(
                &front_end.id,
                &format!("{}/front_end", app.id),
                Kind::FrontEnd,
            );
        }
        for component in &app.components {
            index.component(&app.id, component);
        }
        for function in &diagram.algorithms {
            let id = &function.id;
            let rest = id.strip_prefix(&format!("{}/", app.id));
            let suffix = format!("fn:{}", function.name);
            let ok = rest.is_some_and(|rest| {
                rest == suffix
                    || rest
                        .strip_prefix("mod:")
                        .and_then(|rest| rest.strip_suffix(&format!("/{suffix}")))
                        .is_some_and(|module| !module.is_empty() && !module.contains('/'))
            });
            if !ok {
                index.errors.push(format!(
                    "`{id}` should be `{}/{suffix}` or `{}/mod:<path>/{suffix}`",
                    app.id, app.id
                ));
            }
            index.function(id, function, false);
        }
        for edge in &diagram.edges {
            let expected = format!("{}->{}", edge.from(), edge.to());
            let id = edge.id();
            let numbered = id
                .strip_prefix(&expected)
                .and_then(|rest| rest.strip_prefix('#'))
                .is_some_and(|n| n.parse::<u32>().is_ok_and(|n| n >= 2));
            if numbered {
                index.insert(id, Kind::Edge);
            } else {
                index.add(id, &expected, Kind::Edge);
            }
        }
        index
    }

    fn insert(&mut self, id: &'a str, kind: Kind) {
        if self.kinds.insert(id, kind).is_some() {
            self.errors.push(format!("`{id}` names two elements"));
        }
    }

    fn add(&mut self, id: &'a str, expected: &str, kind: Kind) {
        if id != expected {
            self.errors.push(format!("`{id}` should be `{expected}`"));
        }
        self.insert(id, kind);
    }

    fn component(&mut self, parent: &str, component: &'a Component) {
        let id = &component.id;
        self.add(
            id,
            &format!("{parent}/{}", component.segment()),
            Kind::Component,
        );
        self.components.insert(id, component);
        for field in &component.fields {
            self.add(
                &field.id,
                &format!("{id}/field:{}", field.name),
                Kind::Field(field.kind),
            );
            self.owners.insert(&field.id, id);
            self.fields.insert(&field.id, field);
        }
        for arm in &component.failure {
            let condition = match &arm.condition {
                Condition::Panic => "panic".to_string(),
                Condition::Overflow => "overflow".to_string(),
                Condition::Any => "any".to_string(),
                Condition::Capability { field } => format!("capability:{}", last_name(field)),
                Condition::Message { component } => format!("message:{}", last_name(component)),
                Condition::Child { component } => format!("child:{}", last_name(component)),
            };
            self.add(&arm.id, &format!("{id}/failure:{condition}"), Kind::Failure);
            self.owners.insert(&arm.id, id);
        }
        for route in &component.routes {
            self.member_function(id, &route.handler, false);
        }
        for function in &component.messages {
            self.member_function(id, function, true);
        }
        for function in &component.algorithms {
            self.member_function(id, function, false);
        }
        for child in &component.components {
            self.component(id, child);
        }
    }

    fn member_function(&mut self, owner: &'a str, function: &'a Function, message: bool) {
        let expected = format!("{owner}/fn:{}", function.name);
        if function.id != expected {
            self.errors
                .push(format!("`{}` should be `{expected}`", function.id));
        }
        self.owners.insert(&function.id, owner);
        self.function(&function.id, function, message);
    }

    fn function(&mut self, id: &'a str, function: &'a Function, message: bool) {
        self.insert(id, Kind::Function { message });
        self.functions.insert(id, function);
        if let Some(steps) = &function.body {
            self.steps(id, steps);
        }
    }

    fn steps(&mut self, block: &str, steps: &'a [Step]) {
        for (i, step) in steps.iter().enumerate() {
            let id = step.id();
            self.add(id, &format!("{block}/{i}"), Kind::Step);
            match step {
                Step::Action {
                    exit: Some(exit), ..
                } => {
                    self.add(&exit.id, &format!("{id}/err"), Kind::Exit);
                }
                Step::Action { exit: None, .. } | Step::Return { .. } => {}
                Step::Decision { branches, .. } | Step::Fork { branches, .. } => {
                    for (b, branch) in branches.iter().enumerate() {
                        self.branch(&format!("{id}/b{b}"), branch);
                    }
                }
                Step::Loop { body, skip, .. } => {
                    self.branch(&format!("{id}/body"), body);
                    self.branch(&format!("{id}/skip"), skip);
                    if !skip.steps.is_empty() {
                        self.errors.push(format!("`{}` has steps", skip.id));
                    }
                }
            }
        }
    }

    fn branch(&mut self, expected: &str, branch: &'a Branch) {
        self.add(&branch.id, expected, Kind::Branch);
        self.steps(&branch.id, &branch.steps);
    }
}

/// The name an ID ends with, after its last `/` and any `kind:` prefix.
pub(crate) fn last_name(id: &str) -> &str {
    let segment = id.rsplit('/').next().unwrap_or(id);
    segment.rsplit(':').next().unwrap_or(segment)
}

/// Every problem found in the diagram data. Empty when it is consistent.
pub fn validate(diagram: &Diagram) -> Vec<String> {
    let index = Index::new(diagram);
    let mut errors = index.errors.clone();
    let mut missing = Vec::new();
    let mut expect = |id: &str, what: &str, ok: &dyn Fn(Kind) -> bool| match index.kinds.get(id) {
        Some(&kind) if ok(kind) => {}
        Some(_) => errors.push(format!("`{id}` is not {what}")),
        None => errors.push(format!("`{id}` names nothing, but should be {what}")),
    };
    let capability = |k| k == Kind::Field(FieldKind::Capability);
    let message = |k| k == Kind::Function { message: true };
    let function = |k| matches!(k, Kind::Function { .. });

    for grant in &diagram.app.grants {
        for config in &grant.config {
            if let Config::Setting { setting, .. } = config {
                expect(setting, "a setting", &|k| k == Kind::Setting);
            }
        }
        if let Some(setting) = &grant.outside.setting {
            expect(setting, "a setting", &|k| k == Kind::Setting);
        }
    }
    if let Some(front_end) = &diagram.app.front_end {
        for call in &front_end.calls {
            expect(call, "a route handler", &function);
        }
    }
    for component in index.components.values() {
        for handoff in &component.start {
            let field = format!("{}/field:{}", component.id, handoff.field());
            if !index.fields.contains_key(field.as_str()) {
                missing.push(format!(
                    "`{}` has no field `{}`",
                    component.id,
                    handoff.field()
                ));
            }
            match handoff {
                Handoff::Move { from, .. } | Handoff::Copy { from, .. } => {
                    expect(from, "a grant or field", &|k| {
                        matches!(k, Kind::Grant | Kind::Field(_) | Kind::Setting)
                    });
                }
                Handoff::Lend { from, .. } => {
                    expect(from, "a component", &|k| k == Kind::Component)
                }
                Handoff::New { .. } => {}
                Handoff::Connect { to, .. } => expect(to, "a message handler", &message),
            }
        }
        for arm in &component.failure {
            if let Some(target) = arm.condition.target() {
                match arm.condition {
                    Condition::Capability { .. } => {
                        expect(target, "a capability field", &capability)
                    }
                    _ => expect(target, "a component", &|k| k == Kind::Component),
                }
            }
        }
        for route in &component.routes {
            expect(&route.via, "a capability field", &capability);
        }
    }
    for function in index.functions.values() {
        if let Some(steps) = &function.body {
            check_links(steps, &mut expect);
        }
    }
    for edge in &diagram.edges {
        match edge {
            Edge::Capability { from, to, .. } => {
                expect(from, "a capability field", &capability);
                expect(to, "a grant", &|k| k == Kind::Grant);
            }
            Edge::Message { from, to, .. } => {
                expect(from, "a function", &function);
                expect(to, "a message handler", &message);
            }
            Edge::Emit { from, to, .. } => {
                expect(from, "a function", &function);
                expect(to, "an outlet", &|k| k == Kind::Field(FieldKind::Outlet));
            }
        }
    }
    if let Some(coverage) = &diagram.coverage {
        for id in coverage.reached.keys() {
            expect(id, "an element", &|_| true);
        }
    }
    errors.extend(missing);
    errors
}

fn check_links(steps: &[Step], expect: &mut impl FnMut(&str, &str, &dyn Fn(Kind) -> bool)) {
    let linkable = |k| matches!(k, Kind::Edge | Kind::Function { .. });
    for step in steps {
        match step {
            Step::Action { links, .. } | Step::Return { links, .. } => {
                for link in links {
                    expect(link, "an edge or function", &linkable);
                }
            }
            Step::Decision {
                links, branches, ..
            } => {
                for link in links {
                    expect(link, "an edge or function", &linkable);
                }
                for branch in branches {
                    check_links(&branch.steps, expect);
                }
            }
            Step::Loop {
                links, body, skip, ..
            } => {
                for link in links {
                    expect(link, "an edge or function", &linkable);
                }
                check_links(&body.steps, expect);
                check_links(&skip.steps, expect);
            }
            Step::Fork { branches, .. } => {
                for branch in branches {
                    check_links(&branch.steps, expect);
                }
            }
        }
    }
}
