//! The component view: the app as the outer box, components nested inside
//! it, capability edges to what is outside, and message arrows between
//! components.
//!
//! The layout is deliberately simple. Each box lists its own elements as rows
//! of text, and its children are stacked below them. An arrow between two
//! components runs in a lane at the left of the smallest box holding both, so
//! it never needs routing around other boxes. A component that is not
//! expanded hides its children, and an arrow that ends inside it ends at its
//! box instead. An arrow with both ends inside it is not drawn.

use std::collections::BTreeMap;

use crate::model::*;
use crate::svg::{Canvas, LH, Seg, chars, width, wrap};
use crate::validate::{Index, last_name};
use crate::view::{Hide, View};

const PAD: f64 = 10.0;
const TOP: f64 = 6.0;
const BOTTOM: f64 = 10.0;
const GAP: f64 = 18.0;
const LANE: f64 = 10.0;
const DOC_WIDTH: usize = 64;

struct Row {
    segs: Vec<Seg>,
    /// The element the row stands for, so arrows can end at it.
    anchor: Option<String>,
    doc: Option<String>,
    /// An outlet, drawn as a port on the box's left edge.
    port: bool,
    /// For a route handler, the `HttpServer` field the route arrives through.
    via: Option<String>,
}

impl Row {
    fn plain(segs: Vec<Seg>) -> Self {
        Row {
            segs,
            anchor: None,
            doc: None,
            port: false,
            via: None,
        }
    }

    fn anchored(segs: Vec<Seg>, anchor: &str, doc: Option<&str>) -> Self {
        Row {
            segs,
            anchor: Some(anchor.to_string()),
            doc: doc.map(String::from),
            port: false,
            via: None,
        }
    }
}

struct Node<'a> {
    id: &'a str,
    external: bool,
    children: Vec<usize>,
    parent: Option<usize>,
    rows: Vec<Row>,
    lanes: usize,
    label_w: f64,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Node<'_> {
    fn lanes_w(&self) -> f64 {
        if self.lanes == 0 {
            0.0
        } else {
            self.lanes as f64 * LANE + 4.0
        }
    }

    fn rows_x(&self) -> f64 {
        self.x + PAD + self.lanes_w()
    }

    fn row_y(&self, row: usize) -> f64 {
        self.y + TOP + row as f64 * LH + LH / 2.0
    }

    fn child_x(&self) -> f64 {
        self.rows_x() + self.label_w
    }
}

#[derive(Clone, Copy)]
struct End {
    node: usize,
    row: usize,
}

struct Arrow {
    id: String,
    from: End,
    to: End,
    dashed: bool,
    container: usize,
    lane: usize,
    label: Vec<Seg>,
}

pub(crate) fn render(diagram: &Diagram, view: &View) -> String {
    let index = Index::new(diagram);
    let app = &diagram.app;
    let mut nodes = vec![Node {
        id: &app.id,
        external: false,
        children: Vec::new(),
        parent: None,
        rows: app_rows(app, &diagram.algorithms),
        lanes: 0,
        label_w: 0.0,
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    }];
    let mut visible = BTreeMap::new();
    visible.insert(app.id.as_str(), 0);
    for component in &app.components {
        add(
            &mut nodes,
            &mut visible,
            0,
            &app.id,
            component,
            &index,
            view,
        );
    }

    let mut connects = Vec::new();
    collect_connects(&app.components, &mut connects);
    let arrows = arrows(diagram, &index, &connects, &mut nodes, &visible);

    measure(&mut nodes, 0);

    let field_label = |field: &str| {
        let ty = index.fields.get(field).map_or("", |f| f.ty.as_str());
        vec![
            Seg::new(last_name(field), "").opt(Hide::CapabilityNames),
            Seg::new(format!(": {ty}"), "dim").opt(Hide::CapabilityTypes),
        ]
    };
    let outside_label = |grant: &Grant| {
        let mut segs = vec![Seg::new(grant.outside.label.clone(), "")];
        if let Some(setting) = &grant.outside.setting {
            segs.push(Seg::new(
                format!(" or setting {}", last_name(setting)),
                "dim",
            ));
        }
        segs
    };
    let grants: BTreeMap<&str, &Grant> = app.grants.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut clients = Vec::new();
    let mut resources = Vec::new();
    for edge in &diagram.edges {
        if let Edge::Capability {
            id,
            from,
            to,
            operations,
        } = edge
        {
            let Some(grant) = grants.get(to.as_str()) else {
                continue;
            };
            let entry = (id.as_str(), from.as_str(), *grant, operations);
            match grant.outside.kind {
                OutsideKind::Clients => clients.push(entry),
                OutsideKind::Resource => resources.push(entry),
            }
        }
    }
    let label_w = |list: &[(&str, &str, &Grant, &Vec<String>)]| {
        list.iter()
            .map(|(_, field, _, ops)| width(&field_label(field)).max(chars(&ops.join(", "))))
            .fold(0.0, f64::max)
    };
    let node_w = |list: &[(&str, &str, &Grant, &Vec<String>)]| {
        list.iter()
            .map(|(_, _, grant, _)| width(&outside_label(grant)) + 20.0)
            .fold(0.0, f64::max)
    };

    let left = if clients.is_empty() {
        16.0
    } else {
        10.0 + node_w(&clients) + 16.0 + label_w(&clients) + 16.0
    };
    place(&mut nodes, 0, left, 16.0);
    let app_right = nodes[0].x + nodes[0].w;
    let resource_x = app_right + 16.0 + label_w(&resources) + 16.0;
    let total_w = if resources.is_empty() {
        app_right + 16.0
    } else {
        resource_x + node_w(&resources) + 10.0
    };
    let total_h = nodes[0].y + nodes[0].h + 16.0;

    let mut canvas = Canvas::new(view);
    for node in &nodes {
        draw_box(&mut canvas, node);
    }
    for arrow in &arrows {
        draw_arrow(&mut canvas, &nodes, arrow);
    }

    let find = |field: &str| {
        let owner = index.owners.get(field).copied().unwrap_or(app.id.as_str());
        let node = visible_node(&visible, owner);
        (node, nodes[node].id == owner)
    };
    let client_w = node_w(&clients);
    for (id, field, grant, ops) in &clients {
        let (holder, own) = find(field);
        let holder = &nodes[holder];
        let row = if own {
            holder
                .rows
                .iter()
                .position(|row| row.via.as_deref() == Some(field))
                .unwrap_or(0)
        } else {
            0
        };
        let y = holder.row_y(row);
        let x = 10.0 + client_w;
        canvas.path(&[(x, y), (holder.x, y)], "cap", Some("arrow-cap"), Some(id));
        canvas.text(x + 10.0, y - 5.0, &field_label(field));
        canvas.text(
            x + 10.0,
            y + 14.0,
            &[Seg::new(ops.join(", "), "dim").opt(Hide::CapabilityOperations)],
        );
        outside(
            &mut canvas,
            10.0,
            y,
            client_w,
            &outside_label(grant),
            &grant.id,
        );
    }
    let mut per_holder: BTreeMap<usize, usize> = BTreeMap::new();
    let resource_w = node_w(&resources);
    for (id, field, grant, ops) in &resources {
        let (holder, _) = find(field);
        let k = per_holder.entry(holder).or_insert(0);
        let holder = &nodes[holder];
        let y = holder.y + 14.0 + *k as f64 * LH;
        *k += 1;
        canvas.path(
            &[(holder.x + holder.w, y), (resource_x, y)],
            "cap",
            Some("arrow-cap"),
            Some(id),
        );
        canvas.text(app_right + 10.0, y - 5.0, &field_label(field));
        canvas.text(
            app_right + 10.0,
            y + 14.0,
            &[Seg::new(ops.join(", "), "dim").opt(Hide::CapabilityOperations)],
        );
        outside(
            &mut canvas,
            resource_x,
            y,
            resource_w,
            &outside_label(grant),
            &grant.id,
        );
    }

    canvas.finish(total_w, total_h)
}

fn outside(canvas: &mut Canvas, x: f64, y: f64, w: f64, label: &[Seg], grant: &str) {
    canvas.rect(
        (x, y - 11.0, w, 22.0),
        11.0,
        "outside",
        Some(&format!("{grant}/outside")),
    );
    canvas.text(x + 10.0, y + 4.0, label);
}

fn add<'a>(
    nodes: &mut Vec<Node<'a>>,
    visible: &mut BTreeMap<&'a str, usize>,
    parent: usize,
    parent_id: &str,
    component: &'a Component,
    index: &Index,
    view: &View,
) {
    let expanded = view.expands(&component.id);
    let me = nodes.len();
    nodes.push(Node {
        id: &component.id,
        external: component.external.is_some(),
        children: Vec::new(),
        parent: Some(parent),
        rows: component_rows(component, parent_id, index, expanded),
        lanes: 0,
        label_w: 0.0,
        x: 0.0,
        y: 0.0,
        w: 0.0,
        h: 0.0,
    });
    nodes[parent].children.push(me);
    visible.insert(&component.id, me);
    if expanded {
        for child in &component.components {
            add(nodes, visible, me, &component.id, child, index, view);
        }
    }
}

fn doc_rows(rows: &mut Vec<Row>, doc: Option<&str>) {
    if let Some(doc) = doc {
        for line in wrap(doc, DOC_WIDTH) {
            rows.push(Row::plain(vec![Seg::new(line, "doc").opt(Hide::Docs)]));
        }
    }
}

fn heading(rows: &mut Vec<Row>, text: &str, opts: &[Hide]) {
    rows.push(Row::plain(vec![Seg::new(text, "sec").opts(opts)]));
}

fn app_rows(app: &App, algorithms: &[Function]) -> Vec<Row> {
    let mut rows = vec![Row::anchored(
        vec![Seg::new("app ", "kw"), Seg::new(app.name.clone(), "title")],
        &app.id,
        None,
    )];
    doc_rows(&mut rows, app.doc.as_deref());
    for setting in &app.settings {
        rows.push(Row::anchored(
            vec![
                Seg::new("setting ", "kw"),
                Seg::new(setting.name.clone(), ""),
                Seg::new(format!(": {}", setting.ty), "dim").opt(Hide::SettingTypes),
                Seg::new(format!(" = {}", setting.default), ""),
            ],
            &setting.id,
            setting.doc.as_deref(),
        ));
    }
    for grant in &app.grants {
        let config: Vec<String> = grant
            .config
            .iter()
            .map(|config| match config {
                Config::Fixed { name, value } => format!("{name}: {value}"),
                Config::Setting { name, setting } => {
                    format!("{name}: setting {}", last_name(setting))
                }
            })
            .collect();
        let config = if config.is_empty() {
            String::new()
        } else {
            format!(" {{ {} }}", config.join(", "))
        };
        rows.push(Row::anchored(
            vec![
                Seg::new("grant ", "kw"),
                Seg::new(grant.name.clone(), ""),
                Seg::new(format!(": {}", grant.ty), "dim").opt(Hide::GrantTypes),
                Seg::new(config, "").opt(Hide::GrantValues),
            ],
            &grant.id,
            grant.doc.as_deref(),
        ));
    }
    if !algorithms.is_empty() {
        heading(&mut rows, "algorithms", &[]);
    }
    for function in algorithms {
        rows.push(function_row(function, Vec::new()));
    }
    rows
}

fn function_row(function: &Function, prefix: Vec<Seg>) -> Row {
    let mut segs = prefix;
    segs.push(Seg::new(function.name.clone(), ""));
    segs.push(Seg::new(function.signature(), "dim").opt(Hide::Signatures));
    Row::anchored(segs, &function.id, function.doc.as_deref())
}

/// How a handoff reads in a `start` statement.
fn handoff_text(handoff: &Handoff, parent_id: &str, index: &Index) -> String {
    match handoff {
        Handoff::Move { field, from } => format!("{field}: move {}", last_name(from)),
        Handoff::Lend { field, from } => format!("{field}: &{}", last_name(from)),
        Handoff::Copy { field, from } => format!("{field}: {}", last_name(from)),
        Handoff::New { field, text } => format!("{field}: {text}"),
        Handoff::Connect { field, to } => {
            let owner = index.owners.get(to.as_str()).copied().unwrap_or("");
            let target = if owner == parent_id {
                "self"
            } else {
                last_name(owner)
            };
            format!("{field}: send {target}.{}", last_name(to))
        }
    }
}

fn component_rows(
    component: &Component,
    parent_id: &str,
    index: &Index,
    expanded: bool,
) -> Vec<Row> {
    let mut header = Vec::new();
    if let Some(name) = &component.name {
        header.push(Seg::new(format!("{name}: "), "title"));
    }
    header.push(Seg::new(component.ty.clone(), "title"));
    let mut rows = vec![Row::anchored(header, &component.id, None)];
    if let Some(external) = &component.external {
        let mut text = format!("external {}: {}", external.language, external.path);
        if !external.crates.is_empty() {
            text.push_str(&format!(", crates {}", external.crates.join(", ")));
        }
        rows.push(Row::plain(vec![Seg::new(text, "dim")]));
        if !component
            .fields
            .iter()
            .any(|field| field.kind == FieldKind::Capability)
        {
            rows.push(Row::plain(vec![Seg::new("holds no capabilities", "dim")]));
        }
    }
    doc_rows(&mut rows, component.doc.as_deref());
    if !component.start.is_empty() {
        let handoffs: Vec<String> = component
            .start
            .iter()
            .map(|handoff| handoff_text(handoff, parent_id, index))
            .collect();
        rows.push(Row::plain(vec![
            Seg::new(format!("start {{ {} }}", handoffs.join(", ")), "dim").opt(Hide::Handoffs),
        ]));
    }

    let listed: Vec<&Field> = component
        .fields
        .iter()
        .filter(|field| field.kind != FieldKind::Capability || field.handed_down)
        .collect();
    if !listed.is_empty() {
        heading(&mut rows, "fields", &[]);
    }
    for field in listed {
        let types = if field.kind == FieldKind::Outlet {
            Hide::OutletTypes
        } else {
            Hide::FieldTypes
        };
        let mut segs = vec![
            Seg::new(field.name.clone(), ""),
            Seg::new(format!(": {}", field.ty), "dim").opt(types),
        ];
        if field.handed_down {
            segs.push(Seg::new(", handed down", "dim"));
        }
        let mut row = Row::anchored(segs, &field.id, field.doc.as_deref());
        row.port = field.kind == FieldKind::Outlet;
        rows.push(row);
    }

    if !component.routes.is_empty() {
        heading(&mut rows, "routes", &[]);
    }
    for route in &component.routes {
        let method = format!("{:?}", route.method).to_uppercase();
        let mut prefix = vec![
            Seg::new(format!("{method} "), "kw"),
            Seg::new(format!("{}  ", route.path), ""),
        ];
        if let Some(body) = &route.body {
            prefix.push(Seg::new(format!("body {body}  "), "dim"));
        }
        let mut row = function_row(&route.handler, prefix);
        row.via = Some(route.via.clone());
        rows.push(row);
    }
    if !component.messages.is_empty() {
        heading(&mut rows, "messages", &[]);
    }
    for function in &component.messages {
        rows.push(function_row(function, Vec::new()));
    }
    if !component.algorithms.is_empty() {
        heading(&mut rows, "algorithms", &[]);
    }
    for function in &component.algorithms {
        rows.push(function_row(function, Vec::new()));
    }
    if !component.failure.is_empty() {
        heading(&mut rows, "failure", &[Hide::Failures]);
    }
    for arm in &component.failure {
        let target = arm.condition.target().map_or("", last_name);
        let response = match arm.response {
            Response::Rollback => "rollback",
            Response::Restart => "restart",
        };
        rows.push(Row::anchored(
            vec![
                Seg::new(
                    format!("{} => {response}", arm.condition.source(target)),
                    "warn",
                )
                .opt(Hide::Failures),
            ],
            &arm.id,
            None,
        ));
        for line in wrap(&arm.reason, DOC_WIDTH) {
            rows.push(Row::plain(vec![
                Seg::new(format!("  {line}"), "doc").opts(&[Hide::Failures, Hide::FailureReasons]),
            ]));
        }
    }
    if !expanded && !component.components.is_empty() {
        let children: Vec<String> = component
            .components
            .iter()
            .map(|child| match &child.name {
                Some(name) => format!("{name}: {}", child.ty),
                None => child.ty.clone(),
            })
            .collect();
        rows.push(Row::plain(vec![Seg::new(
            format!("[+] {}", children.join(", ")),
            "dim",
        )]));
    }
    rows
}

fn collect_connects(components: &[Component], out: &mut Vec<(String, String, String)>) {
    for component in components {
        for handoff in &component.start {
            if let Handoff::Connect { field, to } = handoff {
                let from = format!("{}/field:{field}", component.id);
                out.push((format!("{from}->{to}"), from, to.clone()));
            }
        }
        collect_connects(&component.components, out);
    }
}

fn visible_node(visible: &BTreeMap<&str, usize>, id: &str) -> usize {
    let mut id = id;
    loop {
        if let Some(&node) = visible.get(id) {
            return node;
        }
        match id.rsplit_once('/') {
            Some((parent, _)) => id = parent,
            None => return 0,
        }
    }
}

fn ancestors(nodes: &[Node], node: usize) -> Vec<usize> {
    let mut chain = vec![node];
    let mut node = node;
    while let Some(parent) = nodes[node].parent {
        chain.push(parent);
        node = parent;
    }
    chain
}

fn arg_text(arg: &Arg) -> String {
    match arg.pass {
        Pass::Move => format!("move {}", arg.text),
        Pass::Lend => format!("&{}", arg.text),
        Pass::Copy | Pass::New => arg.text.clone(),
    }
}

fn arrows(
    diagram: &Diagram,
    index: &Index,
    connects: &[(String, String, String)],
    nodes: &mut [Node],
    visible: &BTreeMap<&str, usize>,
) -> Vec<Arrow> {
    let mut candidates = Vec::new();
    for edge in &diagram.edges {
        match edge {
            Edge::Message {
                id,
                from,
                to,
                send,
                args,
            } => {
                let args: Vec<String> = args.iter().map(arg_text).collect();
                let mut label = Vec::new();
                if *send {
                    label.push(Seg::new("send ", "kw"));
                }
                label.push(Seg::new(last_name(to), ""));
                label.push(
                    Seg::new(format!("({})", args.join(", ")), "dim").opt(Hide::MessageParams),
                );
                candidates.push((id.clone(), from.clone(), to.clone(), *send, label));
            }
            Edge::Emit {
                id,
                from,
                to,
                value,
            } => {
                let label = vec![
                    Seg::new("send ", "kw"),
                    Seg::new(format!("{}.emit", last_name(to)), ""),
                    Seg::new(format!("({})", arg_text(value)), "dim").opt(Hide::MessageParams),
                ];
                candidates.push((id.clone(), from.clone(), to.clone(), true, label));
            }
            Edge::Capability { .. } => {}
        }
    }
    for (id, from, to) in connects {
        let owner = index.owners.get(to.as_str()).copied().unwrap_or("");
        let label = vec![
            Seg::new("send ", "kw"),
            Seg::new(format!("{}.{}", last_name(owner), last_name(to)), ""),
        ];
        candidates.push((id.clone(), from.clone(), to.clone(), true, label));
    }

    let endpoint = |element: &str, nodes: &[Node]| -> Option<(End, bool)> {
        let owner = index.owners.get(element).copied()?;
        let node = visible_node(visible, owner);
        if nodes[node].id == owner {
            let row = nodes[node]
                .rows
                .iter()
                .position(|row| row.anchor.as_deref() == Some(element))?;
            Some((End { node, row }, false))
        } else {
            Some((End { node, row: 0 }, true))
        }
    };

    let mut arrows = Vec::new();
    for (id, from, to, dashed, mut label) in candidates {
        let (Some((a, a_hidden)), Some((b, b_hidden))) =
            (endpoint(&from, nodes), endpoint(&to, nodes))
        else {
            continue;
        };
        if a.node == b.node && (a_hidden || b_hidden) {
            continue;
        }
        let a_chain = ancestors(nodes, a.node);
        let container = ancestors(nodes, b.node)
            .into_iter()
            .find(|node| a_chain.contains(node))
            .unwrap_or(0);
        let lane = nodes[container].lanes;
        nodes[container].lanes += 1;
        if b.node == container {
            let row = &mut nodes[b.node].rows[b.row];
            row.segs.push(Seg::new("  <- ", "dim"));
            row.segs.append(&mut label);
        } else {
            let w = width(&label) + 24.0;
            if w > nodes[container].label_w {
                nodes[container].label_w = w;
            }
        }
        arrows.push(Arrow {
            id,
            from: a,
            to: b,
            dashed,
            container,
            lane,
            label,
        });
    }
    arrows
}

fn measure(nodes: &mut [Node], node: usize) {
    let children = nodes[node].children.clone();
    let mut child_w: f64 = 0.0;
    let mut child_h = 0.0;
    for &child in &children {
        measure(nodes, child);
        child_w = child_w.max(nodes[child].w);
        child_h += nodes[child].h + GAP;
    }
    for &child in &children {
        nodes[child].w = child_w;
    }
    let me = &mut nodes[node];
    if !children.is_empty() {
        me.label_w = me.label_w.max(20.0);
    }
    let rows_w = me
        .rows
        .iter()
        .map(|row| width(&row.segs))
        .fold(0.0, f64::max);
    let inner = if children.is_empty() {
        rows_w
    } else {
        rows_w.max(me.label_w + child_w)
    };
    me.w = PAD + me.lanes_w() + inner + PAD * 1.6;
    me.h = TOP + me.rows.len() as f64 * LH + child_h + BOTTOM;
}

fn place(nodes: &mut [Node], node: usize, x: f64, y: f64) {
    nodes[node].x = x;
    nodes[node].y = y;
    let child_x = nodes[node].child_x();
    let mut child_y = y + TOP + nodes[node].rows.len() as f64 * LH + GAP / 2.0;
    for child in nodes[node].children.clone() {
        place(nodes, child, child_x, child_y);
        child_y += nodes[child].h + GAP;
    }
}

fn draw_box(canvas: &mut Canvas, node: &Node) {
    let class = if node.parent.is_none() {
        "app"
    } else if node.external {
        "ext"
    } else {
        "comp"
    };
    let rx = if node.parent.is_none() { 8.0 } else { 4.0 };
    canvas.rect((node.x, node.y, node.w, node.h), rx, class, Some(node.id));
    if node.external {
        canvas.rect(
            (node.x + 3.0, node.y + 3.0, node.w - 6.0, node.h - 6.0),
            2.0,
            class,
            None,
        );
    }
    for (i, row) in node.rows.iter().enumerate() {
        let y = node.row_y(i);
        match &row.anchor {
            Some(anchor) => {
                canvas.open(anchor, row.doc.as_deref());
                canvas.text(node.rows_x(), y + 4.0, &row.segs);
                if row.port {
                    canvas.circle(node.x, y, 4.0, "port", None);
                }
                canvas.close();
            }
            None => canvas.text(node.rows_x(), y + 4.0, &row.segs),
        }
    }
}

fn draw_arrow(canvas: &mut Canvas, nodes: &[Node], arrow: &Arrow) {
    let container = &nodes[arrow.container];
    let lane_x = container.x + PAD + arrow.lane as f64 * LANE + LANE / 2.0;
    let point = |end: End| {
        let y = nodes[end.node].row_y(end.row);
        if end.node == arrow.container {
            (container.rows_x() - 3.0, y)
        } else {
            (nodes[end.node].x, y)
        }
    };
    let (start, end) = (point(arrow.from), point(arrow.to));
    let class = if arrow.dashed { "edge send" } else { "edge" };
    canvas.path(
        &[start, (lane_x, start.1), (lane_x, end.1), end],
        class,
        Some("arrow"),
        Some(&arrow.id),
    );
    if arrow.to.node != arrow.container {
        let x = container.child_x() - 8.0 - width(&arrow.label);
        canvas.text(x, end.1 - 4.0, &arrow.label);
    }
}
