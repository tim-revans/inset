//! Which components a request can reach, which it reserves while it runs.
//!
//! A request to a route reserves the component that serves it, and every
//! component the route's handler can reach: through a message it sends or
//! awaits, through a value it emits to the message an outlet is connected
//! to, and so on from each handler reached. The diagram already holds every
//! such edge, so the set is read off it. It is an upper bound: a branch that
//! does not run still reserves what it would have reached.
//!
//! Requests whose sets do not overlap run side by side, and the others wait
//! in turn, so every run equals the requests run one at a time, in the
//! order they finish (`docs/language/runtime.md`, "Concurrent requests").
//! Taking the whole set when the request arrives, rather than as it reaches
//! each component, means no two requests can each wait for the other.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Component, Diagram, Edge, Handoff};

/// For each route handler, by its ID, the IDs of the components a request
/// to it reserves, in ID order. Routes are listed in the order the
/// diagram draws them.
pub fn reservations(diagram: &Diagram) -> Vec<(String, Vec<String>)> {
    // Where each function's edges lead: the handlers it sends or awaits,
    // and the outlets it emits through.
    let mut leads: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in &diagram.edges {
        match edge {
            Edge::Message { from, to, .. } | Edge::Emit { from, to, .. } => {
                leads.entry(from).or_default().push(to);
            }
            Edge::Capability { .. } => {}
        }
    }
    // Where each outlet is connected, `field: send target.message`, and
    // every route handler, in the order the diagram draws them.
    let mut connected: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut routes = Vec::new();
    collect(&diagram.app.components, &mut connected, &mut routes);
    routes
        .into_iter()
        .map(|handler| {
            let mut reached = BTreeSet::new();
            let mut seen = BTreeSet::new();
            let mut stack = vec![handler.to_string()];
            while let Some(id) = stack.pop() {
                if !seen.insert(id.clone()) {
                    continue;
                }
                match id.split_once("/field:") {
                    // An outlet: what it is connected to.
                    Some(_) => {
                        for to in connected.get(&id).into_iter().flatten() {
                            stack.push(to.to_string());
                        }
                    }
                    None => {
                        if let Some((component, _)) = id.rsplit_once("/fn:") {
                            reached.insert(component.to_string());
                        }
                        for to in leads.get(id.as_str()).into_iter().flatten() {
                            stack.push(to.to_string());
                        }
                    }
                }
            }
            (handler.to_string(), reached.into_iter().collect())
        })
        .collect()
}

fn collect<'d>(
    components: &'d [Component],
    connected: &mut BTreeMap<String, Vec<&'d str>>,
    routes: &mut Vec<&'d str>,
) {
    for component in components {
        for handoff in &component.start {
            if let Handoff::Connect { field, to } = handoff {
                connected
                    .entry(format!("{}/field:{field}", component.id))
                    .or_default()
                    .push(to);
            }
        }
        routes.extend(component.routes.iter().map(|r| r.handler.id.as_str()));
        collect(&component.components, connected, routes);
    }
}
