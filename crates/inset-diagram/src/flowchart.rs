//! The flowchart view of one function.
//!
//! A flowchart is laid out from the structure of its steps, as nested blocks:
//! a sequence stacks its steps, a decision puts its branches side by side and
//! joins the ones that carry on, and a loop draws its body below its head with
//! the way back on the left and the way round on the right. Each block has an
//! entry at the top and, unless every path through it returns, an exit at the
//! bottom, both at its centre line.

use std::collections::BTreeMap;

use crate::coverage::{self, Status};
use crate::model::*;
use crate::svg::{Canvas, LH, Seg, chars, width, wrap};
use crate::view::{Hide, View};

/// Gap between consecutive steps, which holds the arrow between them.
const STEP_GAP: f64 = 22.0;
/// Space between a decision and its branches, which holds the branch labels.
const BRANCH_GAP: f64 = 34.0;
const COLUMN_GAP: f64 = 28.0;
const TEXT_WIDTH: usize = 48;
const MARGIN: f64 = 16.0;

enum Item {
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        rx: f64,
        class: &'static str,
        id: Option<String>,
        /// The element whose coverage colours the outline, if not `id`.
        of: Option<String>,
    },
    Polygon {
        points: Vec<(f64, f64)>,
        class: &'static str,
        id: Option<String>,
    },
    Text {
        x: f64,
        y: f64,
        segs: Vec<Seg>,
        /// The element whose coverage colours the text, such as the branch
        /// a label names.
        of: Option<String>,
    },
    Path {
        points: Vec<(f64, f64)>,
        arrow: bool,
        id: Option<String>,
    },
}

impl Item {
    fn shift(&mut self, dx: f64, dy: f64) {
        match self {
            Item::Rect { x, y, .. } | Item::Text { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Item::Polygon { points, .. } | Item::Path { points, .. } => {
                for (x, y) in points {
                    *x += dx;
                    *y += dy;
                }
            }
        }
    }
}

/// A laid-out piece of flowchart, with its origin at its top left.
struct Block {
    w: f64,
    h: f64,
    /// The x of the entry at the top and the exit at the bottom.
    cx: f64,
    /// Whether a path through the block reaches its bottom, rather than returning.
    falls: bool,
    items: Vec<Item>,
}

impl Block {
    fn empty() -> Self {
        Block {
            w: 20.0,
            h: 0.0,
            cx: 10.0,
            falls: true,
            items: Vec::new(),
        }
    }

    fn put(&mut self, other: Block, dx: f64, dy: f64) {
        for mut item in other.items {
            item.shift(dx, dy);
            self.items.push(item);
        }
    }
}

fn path(points: Vec<(f64, f64)>, arrow: bool, id: Option<&str>) -> Item {
    Item::Path {
        points,
        arrow,
        id: id.map(String::from),
    }
}

fn label(x: f64, y: f64, text: &str, class: &'static str) -> Item {
    Item::Text {
        x,
        y,
        segs: vec![Seg::new(text, class)],
        of: None,
    }
}

/// The label of a branch or an exit, which coverage colours as the branch.
fn branch_text(x: f64, y: f64, text: &str, id: &str) -> Item {
    Item::Text {
        x,
        y,
        segs: vec![Seg::new(text, "dim")],
        of: Some(id.to_string()),
    }
}

fn text_lines(text: &str) -> Vec<String> {
    let lines = wrap(text, TEXT_WIDTH);
    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn lines_w(lines: &[String]) -> f64 {
    lines.iter().map(|line| chars(line)).fold(0.0, f64::max)
}

fn text_items(items: &mut Vec<Item>, x: f64, y: f64, lines: &[String], class: &'static str) {
    for (i, line) in lines.iter().enumerate() {
        items.push(label(x, y + i as f64 * LH + 12.0, line, class));
    }
}

/// A box or pill holding text.
fn node(text: &str, class: &'static str, rx: f64, id: Option<&str>) -> Block {
    let lines = text_lines(text);
    let w = lines_w(&lines) + 20.0;
    let h = lines.len() as f64 * LH + 8.0;
    let mut items = vec![Item::Rect {
        x: 0.0,
        y: 0.0,
        w,
        h,
        rx,
        class,
        id: id.map(String::from),
        of: None,
    }];
    text_items(&mut items, 10.0, 4.0, &lines, "");
    Block {
        w,
        h,
        cx: w / 2.0,
        falls: true,
        items,
    }
}

fn pill(text: &str, id: Option<&str>) -> Block {
    let mut block = node(text, "term", 12.0, id);
    block.falls = false;
    block
}

/// A six-sided box, for a decision or a loop head.
fn hexagon(text: &str, id: &str) -> (Vec<Item>, f64, f64) {
    let lines = text_lines(text);
    let w = lines_w(&lines) + 40.0;
    let h = lines.len() as f64 * LH + 12.0;
    let mut items = vec![Item::Polygon {
        points: vec![
            (0.0, h / 2.0),
            (14.0, 0.0),
            (w - 14.0, 0.0),
            (w, h / 2.0),
            (w - 14.0, h),
            (14.0, h),
        ],
        class: "decide",
        id: Some(id.to_string()),
    }];
    text_items(&mut items, 20.0, 6.0, &lines, "");
    (items, w, h)
}

fn branch_label(branch: &Branch) -> String {
    match &branch.defensive {
        Some(reason) => format!("{} (defensive: {reason})", branch.label),
        None => branch.label.clone(),
    }
}

fn sequence(blocks: Vec<Block>) -> Block {
    if blocks.is_empty() {
        return Block::empty();
    }
    let cx = blocks.iter().map(|b| b.cx).fold(0.0, f64::max);
    let right = blocks.iter().map(|b| b.w - b.cx).fold(0.0, f64::max);
    let mut out = Block {
        w: cx + right,
        h: 0.0,
        cx,
        falls: true,
        items: Vec::new(),
    };
    let mut y = 0.0;
    let mut first = true;
    for block in blocks {
        if !first {
            if out.falls {
                out.items
                    .push(path(vec![(cx, y), (cx, y + STEP_GAP)], true, None));
            }
            y += STEP_GAP;
        }
        first = false;
        let (h, falls) = (block.h, block.falls);
        let dx = cx - block.cx;
        out.put(block, dx, y);
        y += h;
        out.falls = falls;
    }
    out.h = y;
    out
}

fn steps(steps: &[Step]) -> Block {
    sequence(steps.iter().map(step).collect())
}

fn step(step: &Step) -> Block {
    match step {
        Step::Action { id, text, exit, .. } => {
            let mut block = node(text, "node", 2.0, Some(id));
            if let Some(exit) = exit {
                let y = block.h / 2.0;
                let out = pill(&format!("return {}", exit.text), Some(&exit.id));
                let x = block.w + 36.0;
                let dy = y - out.h / 2.0;
                block
                    .items
                    .push(path(vec![(block.w, y), (x, y)], true, Some(&exit.id)));
                block
                    .items
                    .push(branch_text(block.w + 6.0, y - 4.0, "?", &exit.id));
                let out_w = out.w;
                block.put(out, x, dy);
                block.w = x + out_w;
            }
            block
        }
        Step::Return { id, text, .. } => pill(text, Some(id)),
        Step::Decision {
            id, text, branches, ..
        } => decision(id, text, branches),
        Step::Loop {
            id,
            text,
            concurrent: false,
            body,
            skip,
            ..
        } => repeat(id, text, body, skip),
        Step::Loop {
            id,
            text,
            concurrent: true,
            body,
            skip,
            ..
        } => fork_for(id, text, body, skip),
        Step::Fork { id, branches } => fork(id, branches),
    }
}

/// Lays branches out side by side, returning each one's block, its x offset
/// and the total width.
fn columns(branches: &[Branch]) -> (Vec<(Block, f64, String)>, f64) {
    let mut out = Vec::new();
    let mut x = 0.0;
    for branch in branches {
        let block = if branch.steps.is_empty() {
            Block::empty()
        } else {
            steps(&branch.steps)
        };
        let text = branch_label(branch);
        let col_w = block.w.max(block.cx + 7.0 + chars(&text));
        out.push((block, x, text));
        x += col_w + COLUMN_GAP;
    }
    (out, (x - COLUMN_GAP).max(0.0))
}

fn decision(id: &str, text: &str, branches: &[Branch]) -> Block {
    let (hex, hex_w, hex_h) = hexagon(text, id);
    let (cols, cols_w) = columns(branches);
    let w = hex_w.max(cols_w);
    let cx = w / 2.0;
    let mut out = Block {
        w,
        h: 0.0,
        cx,
        falls: false,
        items: Vec::new(),
    };
    let hex_x = cx - hex_w / 2.0;
    for mut item in hex {
        item.shift(hex_x, 0.0);
        out.items.push(item);
    }
    let cols_x = cx - cols_w / 2.0;
    let top = hex_h + BRANCH_GAP;
    let n = cols.len() as f64;
    let mut merge = Vec::new();
    let mut bottom: f64 = top;
    for (i, ((block, x, text), branch)) in cols.into_iter().zip(branches).enumerate() {
        let start_x = hex_x + 14.0 + (hex_w - 28.0) * (i as f64 + 1.0) / (n + 1.0);
        let col_cx = cols_x + x + block.cx;
        out.items.push(path(
            vec![
                (start_x, hex_h),
                (start_x, hex_h + 10.0),
                (col_cx, hex_h + 10.0),
                (col_cx, top),
            ],
            !block.items.is_empty(),
            Some(&branch.id),
        ));
        out.items
            .push(branch_text(col_cx + 7.0, top - 6.0, &text, &branch.id));
        bottom = bottom.max(top + block.h);
        if block.falls {
            merge.push((col_cx, top + block.h, branch.id.clone()));
        }
        out.put(block, cols_x + x, top);
    }
    if merge.is_empty() {
        out.h = bottom;
    } else {
        let merge_y = bottom + 12.0;
        for (x, y, _) in merge {
            out.items
                .push(path(vec![(x, y), (x, merge_y), (cx, merge_y)], false, None));
        }
        out.h = merge_y;
        out.falls = true;
    }
    out
}

fn repeat(id: &str, text: &str, body: &Branch, skip: &Branch) -> Block {
    let (hex, hex_w, hex_h) = hexagon(text, id);
    let inner = steps(&body.steps);
    let left = (hex_w / 2.0).max(inner.cx);
    let right = (hex_w / 2.0).max(inner.w - inner.cx);
    let cx = MARGIN + left;
    let right_x = cx + right + 10.0;
    let skip_text = branch_label(skip);
    let mut out = Block {
        w: right_x + 6.0 + chars(&skip_text),
        h: 0.0,
        cx,
        falls: true,
        items: Vec::new(),
    };
    let hex_x = cx - hex_w / 2.0;
    for mut item in hex {
        item.shift(hex_x, 0.0);
        out.items.push(item);
    }
    let top = hex_h + BRANCH_GAP;
    out.items
        .push(path(vec![(cx, hex_h), (cx, top)], true, Some(&body.id)));
    out.items.push(branch_text(
        cx + 4.0,
        top - 8.0,
        &branch_label(body),
        &body.id,
    ));
    let inner_h = inner.h;
    let inner_falls = inner.falls;
    let dx = cx - inner.cx;
    out.put(inner, dx, top);
    let below = top + inner_h;
    if inner_falls {
        out.items.push(path(
            vec![
                (cx, below),
                (cx, below + 10.0),
                (MARGIN / 2.0, below + 10.0),
                (MARGIN / 2.0, hex_h / 2.0),
                (hex_x, hex_h / 2.0),
            ],
            true,
            None,
        ));
    }
    let exit_y = below + 24.0;
    out.items.push(path(
        vec![
            (hex_x + hex_w, hex_h / 2.0),
            (right_x, hex_h / 2.0),
            (right_x, exit_y),
            (cx, exit_y),
        ],
        false,
        Some(&skip.id),
    ));
    out.items.push(branch_text(
        right_x + 6.0,
        hex_h / 2.0 + 16.0,
        &skip_text,
        &skip.id,
    ));
    out.h = exit_y;
    out
}

fn bar(x: f64, y: f64, w: f64, id: Option<&str>) -> Item {
    Item::Rect {
        x,
        y,
        w,
        h: 5.0,
        rx: 1.0,
        class: "bar",
        id: id.map(String::from),
        of: None,
    }
}

fn fork_for(id: &str, text: &str, body: &Branch, skip: &Branch) -> Block {
    let lines = text_lines(&format!("fork: {text}"));
    let text_w = lines_w(&lines);
    let inner = steps(&body.steps);
    let bar_w = (inner.w + 40.0).max(text_w);
    let join_w = chars("join") + 8.0;
    let bar_x = join_w;
    let cx = bar_x + bar_w / 2.0;
    let right_x = bar_x + bar_w + 12.0;
    let skip_text = branch_label(skip);
    let mut out = Block {
        w: right_x + 6.0 + chars(&skip_text),
        h: 0.0,
        cx,
        falls: true,
        items: Vec::new(),
    };
    text_items(&mut out.items, cx - text_w / 2.0, 0.0, &lines, "kw");
    let bar_y = lines.len() as f64 * LH + 6.0;
    out.items.push(bar(bar_x, bar_y, bar_w, Some(id)));
    let top = bar_y + 5.0 + BRANCH_GAP - 8.0;
    out.items.push(path(
        vec![(cx, bar_y + 5.0), (cx, top)],
        true,
        Some(&body.id),
    ));
    out.items.push(branch_text(
        cx + 4.0,
        top - 8.0,
        &branch_label(body),
        &body.id,
    ));
    let (inner_h, inner_falls) = (inner.h, inner.falls);
    let dx = cx - inner.cx;
    out.put(inner, dx, top);
    let join_y = top + inner_h + STEP_GAP;
    if inner_falls {
        out.items
            .push(path(vec![(cx, top + inner_h), (cx, join_y)], true, None));
    }
    out.items.push(bar(bar_x, join_y, bar_w, None));
    out.items.push(label(0.0, join_y + 6.0, "join", "kw"));
    out.items.push(path(
        vec![
            (bar_x + bar_w, bar_y + 2.5),
            (right_x, bar_y + 2.5),
            (right_x, join_y + 2.5),
            (bar_x + bar_w, join_y + 2.5),
        ],
        true,
        Some(&skip.id),
    ));
    out.items.push(branch_text(
        right_x + 6.0,
        bar_y + 18.0,
        &skip_text,
        &skip.id,
    ));
    out.h = join_y + 5.0;
    out
}

fn fork(id: &str, branches: &[Branch]) -> Block {
    let (cols, cols_w) = columns(branches);
    let bar_w = cols_w + 20.0;
    let cx = bar_w / 2.0;
    let mut out = Block {
        w: bar_w + chars("join") + 8.0,
        h: 0.0,
        cx,
        falls: true,
        items: vec![bar(0.0, 0.0, bar_w, Some(id))],
    };
    let top = 5.0 + BRANCH_GAP;
    let bottom = cols
        .iter()
        .map(|(block, _, _)| top + block.h)
        .fold(top, f64::max);
    let join_y = bottom + STEP_GAP;
    for ((block, x, text), branch) in cols.into_iter().zip(branches) {
        let col_cx = 10.0 + x + block.cx;
        out.items.push(path(
            vec![(col_cx, 5.0), (col_cx, top)],
            true,
            Some(&branch.id),
        ));
        out.items
            .push(branch_text(col_cx + 4.0, top - 8.0, &text, &branch.id));
        if block.falls {
            out.items.push(path(
                vec![(col_cx, top + block.h), (col_cx, join_y)],
                true,
                None,
            ));
        }
        out.put(block, 10.0 + x, top);
    }
    out.items.push(bar(0.0, join_y, bar_w, None));
    out.items
        .push(label(bar_w + 8.0, join_y + 6.0, "join", "kw"));
    out.h = join_y + 5.0;
    out
}

pub(crate) fn render(
    function: &Function,
    owner: Option<&str>,
    view: &View,
    statuses: Option<&BTreeMap<Id, Status>>,
) -> String {
    let mut canvas = Canvas::new(view);
    let title = match owner {
        Some(owner) => format!("{owner}::{}", function.name),
        None => function.name.clone(),
    };
    let mut y = 20.0;
    canvas.open(&function.id, None);
    canvas.text(MARGIN, y, &[Seg::new(title, "title")]);
    canvas.close();
    let mut text_w = chars(&function.name) + 40.0;
    if let Some(doc) = &function.doc {
        for line in wrap(doc, 72) {
            y += LH;
            text_w = text_w.max(chars(&line));
            canvas.text(MARGIN, y, &[Seg::new(line, "doc").opt(Hide::Docs)]);
        }
    }
    y += LH;

    let start = {
        let segs = vec![
            Seg::new(function.name.clone(), ""),
            Seg::new(function.signature(), "dim").opt(Hide::Signatures),
        ];
        let w = width(&segs) + 24.0;
        Block {
            w,
            h: 24.0,
            cx: w / 2.0,
            falls: true,
            items: vec![
                Item::Rect {
                    x: 0.0,
                    y: 0.0,
                    w,
                    h: 24.0,
                    rx: 12.0,
                    class: "term",
                    id: None,
                    of: Some(function.id.clone()),
                },
                Item::Text {
                    x: 12.0,
                    y: 16.0,
                    segs,
                    of: None,
                },
            ],
        }
    };
    let mut blocks = vec![start];
    let body = function.body.as_deref().unwrap_or_default();
    blocks.push(steps(body));
    let mut chart = sequence(blocks);
    if chart.falls {
        let end = pill("end", None);
        chart = sequence(vec![chart, end]);
    }

    // An element's coverage, when the view shows it.
    let status = |id: Option<&String>| {
        let id = id?;
        statuses?.get(id).copied()
    };
    for mut item in chart.items {
        item.shift(MARGIN, y);
        match item {
            Item::Rect {
                x,
                y,
                w,
                h,
                rx,
                class,
                id,
                of,
            } => {
                let class = match status(of.as_ref().or(id.as_ref())) {
                    Some(status) => format!("{class} {}", status.class()),
                    None => class.to_string(),
                };
                canvas.rect((x, y, w, h), rx, &class, id.as_deref());
            }
            Item::Polygon { points, class, id } => canvas.polygon(&points, class, id.as_deref()),
            Item::Text { x, y, mut segs, of } => {
                if let Some(status) = status(of.as_ref()) {
                    for seg in &mut segs {
                        seg.class = status.class();
                    }
                }
                canvas.text(x, y, &segs);
            }
            Item::Path { points, arrow, id } => {
                let class = match status(id.as_ref()) {
                    Some(status) => format!("edge {}", status.class()),
                    None => "edge".to_string(),
                };
                canvas.path(&points, &class, arrow.then_some("arrow"), id.as_deref());
            }
        }
    }
    let mut w = (MARGIN + chart.w + MARGIN).max(MARGIN + text_w + MARGIN);
    let mut h = y + chart.h + MARGIN;
    if statuses.is_some() {
        h += coverage::legend(&mut canvas, MARGIN, h - 8.0);
        w = w.max(MARGIN + coverage::legend_w() + MARGIN);
    }
    canvas.finish(w, h)
}
