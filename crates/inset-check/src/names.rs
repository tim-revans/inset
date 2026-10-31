//! What each module declares and imports, and how a path resolves through
//! the modules, with Rust's rules: `crate::`, `super::` and `self::`, a
//! module's own items and its `use` imports, and `pub` deciding where an
//! item can be named from.
//!
//! As in Rust, types, components, modules and the app share one namespace,
//! and functions another, so a `use` imports a name from both. A `use`
//! path is resolved from the module it is written in. Paths into `std` are
//! taken on trust, apart from the modules Inset leaves out, since the
//! standard library is Rust's to check.

use std::cell::RefCell;
use std::collections::HashMap;

use inset_syntax::ast::{self, ItemKind, UseTree, Visibility};
use inset_syntax::{Diagnostic, Ident, Span};

use crate::modules::Modules;

/// What a name resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Def {
    App,
    Module(usize),
    Component(usize),
    /// A struct or enum, by its index in [`Names::datas`].
    Data(usize),
    /// An enum's variant: the enum and the variant's index.
    Variant(usize, usize),
    /// A function at the top level of a module.
    Algorithm(usize),
    /// Something from the standard library, by its last name.
    Std(String),
}

/// Where an item can be named from: anywhere, or inside one module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Vis {
    Public,
    In(usize),
}

#[derive(Clone, Debug)]
struct Binding {
    def: Def,
    vis: Vis,
    /// The name of the declaration, which a reference leads to.
    target: Span,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ns {
    Type,
    Value,
}

#[derive(Default)]
struct Scope {
    types: HashMap<String, Binding>,
    values: HashMap<String, Binding>,
}

impl Scope {
    fn get(&self, ns: Ns, name: &str) -> Option<&Binding> {
        match ns {
            Ns::Type => self.types.get(name),
            Ns::Value => self.values.get(name),
        }
    }
}

/// An item declared at the top level of a module.
pub(crate) struct Decl<'a, T> {
    pub ast: T,
    pub item: &'a ast::Item,
    pub module: usize,
}

/// Every module's declarations, and how names resolve in each.
pub(crate) struct Names<'a> {
    pub modules: &'a Modules,
    pub app: Option<Decl<'a, &'a ast::App>>,
    pub components: Vec<Decl<'a, &'a ast::Component>>,
    pub datas: Vec<Decl<'a, DataAst<'a>>>,
    pub algorithms: Vec<Decl<'a, &'a ast::Function>>,
    scopes: Vec<Scope>,
    /// Each name resolved, with the name of what it refers to.
    pub refs: RefCell<Vec<(Span, Span)>>,
}

/// A struct or an enum.
#[derive(Clone, Copy)]
pub(crate) enum DataAst<'a> {
    Struct(&'a ast::Struct),
    Enum(&'a ast::Enum),
}

impl<'a> DataAst<'a> {
    pub fn name(&self) -> &'a Ident {
        match self {
            DataAst::Struct(s) => &s.name,
            DataAst::Enum(e) => &e.name,
        }
    }
}

/// A `use` waiting to be resolved: where it is, its path, and the name it binds.
struct Import {
    module: usize,
    vis: Vis,
    path: Vec<Ident>,
    name: Ident,
}

impl<'a> Names<'a> {
    pub(crate) fn build(modules: &'a Modules, diagnostics: &mut Vec<Diagnostic>) -> Names<'a> {
        let mut names = Names {
            modules,
            app: None,
            components: Vec::new(),
            datas: Vec::new(),
            algorithms: Vec::new(),
            scopes: (0..modules.list.len()).map(|_| Scope::default()).collect(),
            refs: RefCell::new(Vec::new()),
        };
        let mut imports = Vec::new();
        for (index, module) in modules.list.iter().enumerate() {
            for item in &module.ast.items {
                names.declare(index, item, &mut imports, diagnostics);
            }
        }
        if names.app.is_none() && !modules.list.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    "the program has no app",
                    Span::new(0, 0),
                    "no `app` in this file",
                )
                .with_help("add `app Name { ... }` with the program's settings, grants and starts"),
            );
        }
        names.resolve_imports(imports, diagnostics);
        names
    }

    fn vis(&self, module: usize, vis: &Visibility) -> Vis {
        match vis {
            Visibility::Pub(_) | Visibility::PubCrate(_) => Vis::Public,
            Visibility::PubSuper(_) => Vis::In(self.modules.parent(module).unwrap_or(module)),
            Visibility::Private => Vis::In(module),
        }
    }

    fn declare(
        &mut self,
        module: usize,
        item: &'a ast::Item,
        imports: &mut Vec<Import>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let vis = self.vis(module, &item.vis);
        let (ns, name, def) = match &item.kind {
            ItemKind::App(app) => {
                if module != 0 {
                    diagnostics.push(
                        Diagnostic::new(
                            "the app belongs in `src/main.inset`",
                            app.name.span,
                            "this app is in another module",
                        )
                        .with_help("move the app to the root module, `src/main.inset`"),
                    );
                    return;
                }
                if let Some(first) = &self.app {
                    diagnostics.push(
                        Diagnostic::new(
                            "a program has one app",
                            app.name.span,
                            "this is a second app",
                        )
                        .with_help(format!(
                            "put its settings, grants and starts in `app {}`",
                            first.ast.name.name
                        )),
                    );
                    return;
                }
                self.app = Some(Decl {
                    ast: app,
                    item,
                    module,
                });
                (Ns::Type, &app.name, Def::App)
            }
            ItemKind::Component(c) => {
                let def = Def::Component(self.components.len());
                if !self.free(module, Ns::Type, &c.name, diagnostics) {
                    return;
                }
                self.components.push(Decl {
                    ast: c,
                    item,
                    module,
                });
                (Ns::Type, &c.name, def)
            }
            ItemKind::Struct(s) => {
                let def = Def::Data(self.datas.len());
                if !self.free(module, Ns::Type, &s.name, diagnostics) {
                    return;
                }
                self.datas.push(Decl {
                    ast: DataAst::Struct(s),
                    item,
                    module,
                });
                (Ns::Type, &s.name, def)
            }
            ItemKind::Enum(e) => {
                let def = Def::Data(self.datas.len());
                if !self.free(module, Ns::Type, &e.name, diagnostics) {
                    return;
                }
                self.datas.push(Decl {
                    ast: DataAst::Enum(e),
                    item,
                    module,
                });
                (Ns::Type, &e.name, def)
            }
            ItemKind::Function(f) => {
                let def = Def::Algorithm(self.algorithms.len());
                if !self.free(module, Ns::Value, &f.name, diagnostics) {
                    return;
                }
                self.algorithms.push(Decl {
                    ast: f,
                    item,
                    module,
                });
                (Ns::Value, &f.name, def)
            }
            ItemKind::Mod(name) => {
                let child = self.modules.list.iter().position(|m| {
                    m.parent
                        .as_ref()
                        .is_some_and(|(p, decl)| *p == module && decl.span == name.span)
                });
                if !self.free(module, Ns::Type, name, diagnostics) {
                    return;
                }
                match child {
                    Some(child) => (Ns::Type, name, Def::Module(child)),
                    // No file was found, which has been reported.
                    None => return,
                }
            }
            ItemKind::Use(tree) => {
                let mut flat = Vec::new();
                flatten(tree, &mut Vec::new(), &mut flat, diagnostics);
                for (path, name) in flat {
                    imports.push(Import {
                        module,
                        vis,
                        path,
                        name,
                    });
                }
                return;
            }
        };
        self.bind(module, ns, name, def, vis);
    }

    /// Whether `name` is not yet taken in the module's namespace, reporting
    /// it if it is.
    fn free(&self, module: usize, ns: Ns, name: &Ident, diagnostics: &mut Vec<Diagnostic>) -> bool {
        if self.scopes[module].get(ns, &name.name).is_none() {
            return true;
        }
        diagnostics.push(
            Diagnostic::new(
                format!("the name `{}` is defined twice", name.name),
                name.span,
                "this repeats an earlier name",
            )
            .with_help("rename one of the two"),
        );
        false
    }

    fn bind(&mut self, module: usize, ns: Ns, name: &Ident, def: Def, vis: Vis) {
        let target = self.target(&def).unwrap_or(name.span);
        let binding = Binding { def, vis, target };
        let scope = &mut self.scopes[module];
        match ns {
            Ns::Type => scope.types.insert(name.name.clone(), binding),
            Ns::Value => scope.values.insert(name.name.clone(), binding),
        };
    }

    /// The name a reference to a definition leads to.
    pub(crate) fn target(&self, def: &Def) -> Option<Span> {
        Some(match def {
            Def::App => self.app.as_ref()?.ast.name.span,
            Def::Module(m) => self.modules.list[*m].parent.as_ref()?.1.span,
            Def::Component(c) => self.components[*c].ast.name.span,
            Def::Data(d) => self.datas[*d].ast.name().span,
            Def::Variant(d, v) => match self.datas[*d].ast {
                DataAst::Enum(e) => e.variants.get(*v)?.name.span,
                DataAst::Struct(_) => return None,
            },
            Def::Algorithm(a) => self.algorithms[*a].ast.name.span,
            Def::Std(_) => return None,
        })
    }

    /// Resolves the `use` imports, in rounds, since one may import through
    /// another.
    fn resolve_imports(&mut self, mut pending: Vec<Import>, diagnostics: &mut Vec<Diagnostic>) {
        loop {
            let before = pending.len();
            let mut waiting = Vec::new();
            for import in pending {
                match self.resolve_import(&import, diagnostics) {
                    Ok(Some(found)) => {
                        for (ns, def) in found {
                            let taken = self.scopes[import.module].get(ns, &import.name.name);
                            if taken.is_some() {
                                diagnostics.push(
                                    Diagnostic::new(
                                        format!("the name `{}` is defined twice", import.name.name),
                                        import.name.span,
                                        "this repeats an earlier name",
                                    )
                                    .with_help(format!(
                                        "import it under another name, with `as`, or rename the other `{}`",
                                        import.name.name
                                    )),
                                );
                                break;
                            }
                            self.bind(import.module, ns, &import.name, def, import.vis);
                        }
                    }
                    Ok(None) => waiting.push(import),
                    Err(diagnostic) => diagnostics.push(diagnostic),
                }
            }
            pending = waiting;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        // What is left names something that is not there.
        for import in pending {
            let first = &import.path[0];
            diagnostics.push(
                Diagnostic::new(
                    format!("cannot find `{}` to import", first.name),
                    first.span,
                    format!(
                        "not an item or module of `{}`",
                        self.modules.list[import.module].rust_path()
                    ),
                )
                .with_help(
                    "a `use` path starts at the module it is written in; start it with `crate::` \
                     to name an item of the root module, or with `super::` for the parent's",
                ),
            );
        }
    }

    /// The definitions an import binds, in each namespace where its last name
    /// is found, or `None` if its path starts with an import not resolved yet.
    /// A private item is still imported, once its error is reported, so the
    /// names that use it do not report it again.
    fn resolve_import(
        &self,
        import: &Import,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<Option<Vec<(Ns, Def)>>, Diagnostic> {
        let (last, prefix) = import.path.split_last().expect("a path has a name");
        let module = import.module;
        if prefix.is_empty() {
            if matches!(last.name.as_str(), "crate" | "super" | "self") {
                return Err(Diagnostic::new(
                    format!("`{}` cannot be imported on its own", last.name),
                    last.span,
                    "this names a module relative to this one",
                )
                .with_help(format!(
                    "import the items you need: `use {}::{{...}};`",
                    last.name
                )));
            }
            // `use name;` in a module only renames what the module already has.
            return Err(Diagnostic::new(
                format!("`use {}` imports nothing", last.name),
                last.span,
                "a `use` path names a module, then the item",
            )
            .with_help(
                "write the path from `crate::`, such as `use crate::warehouse::Warehouse;`",
            ));
        }
        let owner = match self.resolve_in(module, prefix, Ns::Type)? {
            Some(def) => def,
            None if is_std(&prefix[0].name) => {
                if let Some(diagnostic) = std_effect(&import.path) {
                    return Err(diagnostic);
                }
                return Ok(Some(vec![
                    (Ns::Type, Def::Std(last.name.clone())),
                    (Ns::Value, Def::Std(last.name.clone())),
                ]));
            }
            None => return Ok(None),
        };
        let mut found = Vec::new();
        match &owner {
            Def::Module(m) => {
                for ns in [Ns::Type, Ns::Value] {
                    if let Some(binding) = self.scopes[*m].get(ns, &last.name) {
                        if !self.visible(binding.vis, module) {
                            diagnostics.push(self.private(&last.name, binding, *m, last.span));
                        }
                        self.refer(last.span, binding.target);
                        found.push((ns, binding.def.clone()));
                    }
                }
                if found.is_empty() {
                    return Err(self.not_in_module(*m, last));
                }
            }
            Def::Data(d) => match self.variant(*d, &last.name) {
                Some(v) => {
                    let def = Def::Variant(*d, v);
                    if let Some(target) = self.target(&def) {
                        self.refer(last.span, target);
                    }
                    found.push((Ns::Value, def.clone()));
                    found.push((Ns::Type, def));
                }
                None => return Err(self.no_variant(*d, last)),
            },
            Def::Std(_) => {
                found.push((Ns::Type, Def::Std(last.name.clone())));
                found.push((Ns::Value, Def::Std(last.name.clone())));
            }
            _ => return Err(not_a_module(&prefix[prefix.len() - 1])),
        }
        Ok(Some(found))
    }

    pub(crate) fn visible(&self, vis: Vis, from: usize) -> bool {
        match vis {
            Vis::Public => true,
            Vis::In(module) => self.modules.is_within(from, module),
        }
    }

    fn refer(&self, span: Span, target: Span) {
        self.refs.borrow_mut().push((span, target));
    }

    /// The index of an enum's variant.
    pub(crate) fn variant(&self, data: usize, name: &str) -> Option<usize> {
        match self.datas[data].ast {
            DataAst::Enum(e) => e.variants.iter().position(|v| v.name.name == name),
            DataAst::Struct(_) => None,
        }
    }

    /// Resolves a path written in `from`, its last name in `ns`. `Ok(None)`
    /// means its first name is not declared or imported there, which leaves
    /// the caller to try the names Inset provides, such as `String` or `Some`.
    pub(crate) fn resolve_in(
        &self,
        from: usize,
        path: &[Ident],
        ns: Ns,
    ) -> Result<Option<Def>, Diagnostic> {
        let mut def;
        let mut rest = path;
        match path[0].name.as_str() {
            "crate" => {
                def = Def::Module(0);
                rest = &path[1..];
            }
            "self" if path.len() > 1 => {
                def = Def::Module(from);
                rest = &path[1..];
            }
            "super" => {
                let mut module = from;
                while let Some((segment, after)) = rest.split_first()
                    && segment.name == "super"
                {
                    module = self.modules.parent(module).ok_or_else(|| {
                        Diagnostic::new(
                            "`super` names the parent module, and this is the root",
                            segment.span,
                            "`src/main.inset` has no parent",
                        )
                        .with_help("start the path with `crate::` instead")
                    })?;
                    rest = after;
                }
                def = Def::Module(module);
            }
            name => {
                let ns0 = if path.len() == 1 { ns } else { Ns::Type };
                match self.scopes[from].get(ns0, name) {
                    Some(binding) => {
                        self.refer(path[0].span, binding.target);
                        def = binding.def.clone();
                        rest = &path[1..];
                    }
                    None if path.len() > 1 && is_std(name) => {
                        if let Some(diagnostic) = std_effect(path) {
                            return Err(diagnostic);
                        }
                        return Ok(Some(Def::Std(path[path.len() - 1].name.clone())));
                    }
                    None => return Ok(None),
                }
            }
        }
        for (i, segment) in rest.iter().enumerate() {
            let last = i + 1 == rest.len();
            let ns_here = if last { ns } else { Ns::Type };
            def = match &def {
                Def::Module(m) => {
                    let binding = self.scopes[*m]
                        .get(ns_here, &segment.name)
                        .or_else(|| {
                            self.scopes[*m]
                                .get(Ns::Type, &segment.name)
                                .filter(|_| last)
                        })
                        .ok_or_else(|| self.not_in_module(*m, segment))?;
                    if !self.visible(binding.vis, from) {
                        return Err(self.private(&segment.name, binding, *m, segment.span));
                    }
                    self.refer(segment.span, binding.target);
                    binding.def.clone()
                }
                Def::Data(d) if last => {
                    let v = self
                        .variant(*d, &segment.name)
                        .ok_or_else(|| self.no_variant(*d, segment))?;
                    let def = Def::Variant(*d, v);
                    if let Some(target) = self.target(&def) {
                        self.refer(segment.span, target);
                    }
                    def
                }
                Def::Std(_) => return Ok(Some(Def::Std(path[path.len() - 1].name.clone()))),
                _ => {
                    let before = if i == 0 {
                        &path[path.len() - rest.len() - 1]
                    } else {
                        &rest[i - 1]
                    };
                    return Err(not_a_module(before));
                }
            };
        }
        Ok(Some(def))
    }

    fn private(&self, name: &str, binding: &Binding, module: usize, span: Span) -> Diagnostic {
        let owner = self.modules.list[module].rust_path();
        let within = match binding.vis {
            Vis::In(m) => self.modules.list[m].rust_path(),
            Vis::Public => owner.clone(),
        };
        Diagnostic::new(
            format!("`{name}` is private to `{within}`"),
            span,
            format!("`{name}` is declared in `{owner}` without `pub`"),
        )
        .with_help(format!(
            "make it `pub(crate)` where it is declared to use it from anywhere in the program, \
             or use it only inside `{within}`"
        ))
    }

    fn not_in_module(&self, module: usize, name: &Ident) -> Diagnostic {
        Diagnostic::new(
            format!(
                "cannot find `{}` in `{}`",
                name.name,
                self.modules.list[module].rust_path()
            ),
            name.span,
            "not an item of this module",
        )
    }

    fn no_variant(&self, data: usize, name: &Ident) -> Diagnostic {
        let owner = &self.datas[data].ast.name().name;
        let variants = match self.datas[data].ast {
            DataAst::Enum(e) => crate::names(e.variants.iter().map(|v| v.name.name.as_str())),
            DataAst::Struct(_) => String::new(),
        };
        let diagnostic = Diagnostic::new(
            format!("`{owner}` has no variant `{}`", name.name),
            name.span,
            format!("not a variant of `{owner}`"),
        );
        if variants.is_empty() {
            diagnostic
        } else {
            diagnostic.with_help(format!("the variants of `{owner}` are {variants}"))
        }
    }
}

fn not_a_module(name: &Ident) -> Diagnostic {
    Diagnostic::new(
        format!("`{}` is not a module", name.name),
        name.span,
        "a path goes through modules, then names an item",
    )
}

pub(crate) fn is_std(name: &str) -> bool {
    matches!(name, "std" | "core" | "alloc")
}

/// The parts of the standard library that are effects, which Inset leaves
/// out (syntax.md, "Rust features Inset leaves out").
pub(crate) fn std_effect(path: &[Ident]) -> Option<Diagnostic> {
    let first = &path[0];
    let second = path.get(1)?;
    let (why, help) = match second.name.as_str() {
        "fs" => (
            "file access is an effect",
            "use a capability the app grants",
        ),
        "io" => (
            "input and output are effects",
            "use a capability the app grants",
        ),
        "net" => (
            "networking is an effect",
            "use a capability the app grants, such as `HttpServer`",
        ),
        "process" => (
            "running a process is an effect",
            "use a capability the app grants",
        ),
        "env" => (
            "environment variables are an effect",
            "declare a `setting` in the app",
        ),
        "time" => (
            "the clock is an effect",
            "use a capability the app grants, so tests can fake the clock",
        ),
        "thread" => (
            "threads are concurrency the flowchart cannot draw",
            "use `fork`",
        ),
        _ => return None,
    };
    Some(
        Diagnostic::new(
            format!("Inset has no `{}::{}`", first.name, second.name),
            first.span.to(second.span),
            why,
        )
        .with_help(format!(
            "{help}, so the diagram shows it and a test can fake it"
        )),
    )
}

/// Each path a `use` tree imports, with the name it binds.
fn flatten(
    tree: &UseTree,
    prefix: &mut Vec<Ident>,
    out: &mut Vec<(Vec<Ident>, Ident)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match tree {
        UseTree::Path { segment, rest } => {
            prefix.push(segment.clone());
            flatten(rest, prefix, out, diagnostics);
            prefix.pop();
        }
        UseTree::Name { name, alias } if name.name == "self" => match prefix.last() {
            Some(module) => out.push((prefix.clone(), alias.clone().unwrap_or(module.clone()))),
            None => diagnostics.push(Diagnostic::new(
                "`self` cannot be imported on its own",
                name.span,
                "this names the module it is written in",
            )),
        },
        UseTree::Name { name, alias } => {
            let mut path = prefix.clone();
            path.push(name.clone());
            out.push((path, alias.clone().unwrap_or(name.clone())));
        }
        UseTree::Group(trees, _) => {
            for tree in trees {
                flatten(tree, prefix, out, diagnostics);
            }
        }
    }
}
