//! A program's modules: the tree of files that `mod` declarations make,
//! starting from `src/main.inset`, found where Rust would find them.
//!
//! Each module's file is parsed with its spans moved on by a base offset of
//! its own, so that every span in the program is unique, as in rustc's
//! source map. The checker then compares and looks up spans without asking
//! which file each came from, and [`Modules::report`] turns a span back into
//! a file and an offset in it.

use inset_syntax::ast::{self, ItemKind};
use inset_syntax::{Diagnostic, Ident, Span, is_requirements_file, parse_code_at};

use crate::{MAIN, Program, Report};

/// Every module of a program, the root first.
#[derive(Clone, Debug, Default)]
pub struct Modules {
    pub list: Vec<Module>,
}

/// One module: one file of code.
#[derive(Clone, Debug)]
pub struct Module {
    /// The names from the root, empty for the root itself: `["warehouse"]`.
    pub path: Vec<String>,
    /// The file's index in [`Program::files`].
    pub file: usize,
    /// The file's path, such as `src/warehouse.inset`.
    pub file_path: String,
    /// Where the file's offsets start among the program's.
    pub base: usize,
    /// The file's text.
    pub text: String,
    /// Its syntax tree, with every span moved on by `base`.
    pub ast: ast::Module,
    /// The module whose `mod` declares this one, and the declaration's name.
    pub parent: Option<(usize, Ident)>,
}

impl Module {
    /// The module's path in Rust: `crate`, or `crate::warehouse`.
    pub fn rust_path(&self) -> String {
        let mut path = String::from("crate");
        for name in &self.path {
            path.push_str("::");
            path.push_str(name);
        }
        path
    }

    /// The source text of a span in this module.
    pub fn text_of(&self, span: Span) -> &str {
        &self.text[span.start - self.base..span.end - self.base]
    }
}

impl Modules {
    /// Parses `src/main.inset` and every module it declares, directly or
    /// through other modules. The reports are the parser's, a `mod` with no
    /// file or with two, and a code file under `src/` that no `mod` declares.
    pub fn load(program: &Program) -> (Modules, Vec<Report>) {
        let mut modules = Modules::default();
        let mut reports = Vec::new();
        if let Some(main) = program.files.iter().position(|f| f.path == MAIN) {
            modules.add(program, main, Vec::new(), None, &mut reports);
        }
        for (index, file) in program.files.iter().enumerate() {
            let is_code = file.path.starts_with("src/") && !is_requirements_file(&file.path);
            if is_code && !modules.list.iter().any(|m| m.file == index) {
                reports.push(Report {
                    file: index,
                    diagnostic: unreachable_file(&file.path),
                });
            }
        }
        (modules, reports)
    }

    fn add(
        &mut self,
        program: &Program,
        file: usize,
        path: Vec<String>,
        parent: Option<(usize, Ident)>,
        reports: &mut Vec<Report>,
    ) {
        let base = self
            .list
            .last()
            .map_or(0, |last| last.base + last.text.len() + 1);
        let text = program.files[file].text.clone();
        let parsed = parse_code_at(&text, base);
        reports.extend(parsed.diagnostics.into_iter().map(|d| Report {
            file,
            diagnostic: d.unshifted(base),
        }));
        let index = self.list.len();
        let declared: Vec<Ident> = parsed
            .ast
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Mod(name) => Some(name.clone()),
                _ => None,
            })
            .collect();
        self.list.push(Module {
            path: path.clone(),
            file,
            file_path: program.files[file].path.clone(),
            base,
            text,
            ast: parsed.ast,
            parent,
        });
        let dir = child_dir(&program.files[file].path);
        let mut seen: Vec<&str> = Vec::new();
        for name in &declared {
            if seen.contains(&name.name.as_str()) {
                // Reported as a name defined twice when the names are collected.
                continue;
            }
            seen.push(&name.name);
            let candidates = [
                format!("{dir}/{}.inset", name.name),
                format!("{dir}/{}/mod.inset", name.name),
            ];
            let found: Vec<usize> = candidates
                .iter()
                .filter_map(|c| program.files.iter().position(|f| &f.path == c))
                .collect();
            let diagnostic = match found.as_slice() {
                [child] => {
                    let mut child_path = path.clone();
                    child_path.push(name.name.clone());
                    self.add(
                        program,
                        *child,
                        child_path,
                        Some((index, name.clone())),
                        reports,
                    );
                    continue;
                }
                [] => Diagnostic::new(
                    format!("no file for the module `{}`", name.name),
                    name.span,
                    format!("neither `{}` nor `{}` exists", candidates[0], candidates[1]),
                )
                .with_help(format!(
                    "create `{}` with the module's items, or remove this `mod`",
                    candidates[0]
                )),
                _ => Diagnostic::new(
                    format!("the module `{}` has two files", name.name),
                    name.span,
                    format!("both `{}` and `{}` exist", candidates[0], candidates[1]),
                )
                .with_help("keep one of the two files"),
            };
            reports.push(Report {
                file,
                diagnostic: diagnostic.unshifted(base),
            });
        }
    }

    /// The module a program-wide offset is in.
    pub fn at(&self, offset: usize) -> Option<usize> {
        self.list
            .iter()
            .rposition(|m| m.base <= offset && offset <= m.base + m.text.len())
    }

    /// A diagnostic with a program-wide span, as a report on its file.
    pub fn report(&self, diagnostic: Diagnostic) -> Option<Report> {
        let module = &self.list[self.at(diagnostic.span.start)?];
        Some(Report {
            file: module.file,
            diagnostic: diagnostic.unshifted(module.base),
        })
    }

    /// A program-wide span as a file and the span in it.
    pub fn locate(&self, span: Span) -> Option<(usize, Span)> {
        let module = &self.list[self.at(span.start)?];
        Some((module.file, span.unshifted(module.base)))
    }

    /// The source text of a program-wide span.
    pub fn text(&self, span: Span) -> &str {
        match self.at(span.start) {
            Some(m) => self.list[m].text_of(span),
            None => "",
        }
    }

    /// Whether `module` is `ancestor` or inside it.
    pub fn is_within(&self, module: usize, ancestor: usize) -> bool {
        let mut at = Some(module);
        while let Some(m) = at {
            if m == ancestor {
                return true;
            }
            at = self.list[m].parent.as_ref().map(|(p, _)| *p);
        }
        false
    }

    pub fn parent(&self, module: usize) -> Option<usize> {
        self.list[module].parent.as_ref().map(|(p, _)| *p)
    }
}

/// The directory a module's children are in: beside `main.inset` and
/// `mod.inset`, and in a directory named after any other module's file.
fn child_dir(path: &str) -> String {
    let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
    if file == "main.inset" || file == "mod.inset" {
        dir.to_string()
    } else {
        format!("{dir}/{}", file.trim_end_matches(".inset"))
    }
}

fn unreachable_file(path: &str) -> Diagnostic {
    let relative = path.strip_prefix("src/").unwrap_or(path);
    let (parent, name) = match relative.trim_end_matches(".inset").rsplit_once('/') {
        Some((dir, "mod")) => match dir.rsplit_once('/') {
            Some((parent, name)) => (format!("src/{parent}.inset"), name.to_string()),
            None => (MAIN.to_string(), dir.to_string()),
        },
        Some((dir, name)) => (format!("src/{dir}.inset"), name.to_string()),
        None => (
            MAIN.to_string(),
            relative.trim_end_matches(".inset").to_string(),
        ),
    };
    Diagnostic::new(
        "this file is not a module of the program",
        Span::new(0, 0),
        "no `mod` declares it",
    )
    .with_help(format!(
        "declare it with `mod {name};` in `{parent}`, or move its items into a module that is"
    ))
}
