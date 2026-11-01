//! The types the checker knows, and how a written type resolves to one.
//!
//! The check is deliberately partial: a type the checker cannot work out is
//! [`Ty::Unknown`], which matches anything, so the checker only reports a
//! mismatch it is sure of.

use std::fmt;

use inset_syntax::ast::{self, TypeKind};
use inset_syntax::{Diagnostic, Ident};

use crate::names::{Def, Names, Ns};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ty {
    /// Not worked out. Never reported, and compatible with every type.
    Unknown,
    /// What `return` and `panic!` evaluate to.
    Never,
    Bool,
    Char,
    /// An integer type, or `None` for an integer literal without a suffix.
    Int(Option<&'static str>),
    /// A float type, or `None` for a float literal without a suffix.
    Float(Option<&'static str>),
    Str,
    String,
    Decimal,
    /// `()` when empty.
    Tuple(Vec<Ty>),
    Ref {
        mutable: bool,
        inner: Box<Ty>,
    },
    Slice(Box<Ty>),
    /// A generic standard type, such as `Option<T>` or `Vec<T>`.
    Std(&'static str, Vec<Ty>),
    HttpServer,
    HttpClient,
    Table(Box<Ty>, Box<Ty>),
    /// `Handle<C>`: the component's index and its name.
    Handle(usize, String),
    Outlet(Box<Ty>),
    /// A struct or enum the program declares: its index, its name, and
    /// whether it derives `Copy` and `Clone`.
    Data {
        id: usize,
        name: String,
        copy: bool,
        clone: bool,
    },
}

pub(crate) const INTS: [&str; 12] = [
    "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
];
pub(crate) const FLOATS: [&str; 2] = ["f32", "f64"];

/// Generic standard types, with how many type arguments each takes.
const STD_GENERICS: [(&str, usize); 16] = [
    ("Option", 1),
    ("Result", 2),
    ("Vec", 1),
    ("VecDeque", 1),
    ("HashMap", 2),
    ("BTreeMap", 2),
    ("HashSet", 1),
    ("BTreeSet", 1),
    ("Box", 1),
    ("Rc", 1),
    ("Arc", 1),
    ("Cell", 1),
    ("RefCell", 1),
    ("Mutex", 1),
    ("RwLock", 1),
    // axum's and actix-web's `Json<T>`, for a JSON body or reply.
    ("Json", 1),
];

/// Types that share state between their owners, which may not cross a
/// component boundary (syntax.md, "Rust features Inset leaves out").
pub(crate) const SHARED: [&str; 6] = ["Rc", "Arc", "Cell", "RefCell", "Mutex", "RwLock"];

impl Ty {
    pub(crate) fn unit() -> Ty {
        Ty::Tuple(Vec::new())
    }

    pub(crate) fn is_unit(&self) -> bool {
        matches!(self, Ty::Tuple(elems) if elems.is_empty())
    }

    pub(crate) fn is_known(&self) -> bool {
        !matches!(self, Ty::Unknown)
    }

    pub(crate) fn is_capability(&self) -> bool {
        matches!(self, Ty::HttpServer | Ty::HttpClient | Ty::Table(..))
    }

    /// A capability, handle or outlet: something only a component may hold.
    pub(crate) fn is_effect(&self) -> bool {
        self.is_capability() || matches!(self, Ty::Handle(..) | Ty::Outlet(_))
    }

    /// Whether the type is `Copy`, or `None` if that is not known.
    pub(crate) fn is_copy(&self) -> Option<bool> {
        match self {
            Ty::Unknown => None,
            Ty::Never | Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Float(_) | Ty::Decimal => Some(true),
            Ty::Ref { mutable, .. } => Some(!mutable),
            Ty::Tuple(elems) => elems
                .iter()
                .try_fold(true, |all, t| Some(all && t.is_copy()?)),
            Ty::Data { copy, .. } => Some(*copy),
            _ => Some(false),
        }
    }

    /// Whether the type is `Clone`, or `None` if that is not known.
    pub(crate) fn is_clone(&self) -> Option<bool> {
        match self {
            Ty::Unknown => None,
            Ty::HttpServer
            | Ty::HttpClient
            | Ty::Table(..)
            | Ty::Handle(..)
            | Ty::Outlet(_)
            | Ty::Str => Some(false),
            Ty::Data { clone, .. } => Some(*clone),
            Ty::Slice(_) => Some(false),
            Ty::Ref { mutable, .. } => Some(!mutable),
            Ty::Tuple(elems) | Ty::Std(_, elems) => elems
                .iter()
                .try_fold(true, |all, t| Some(all && t.is_clone()?)),
            _ => Some(true),
        }
    }

    /// The first capability, handle or outlet held inside this type, below
    /// its top level.
    pub(crate) fn effect_inside(&self) -> Option<&Ty> {
        self.children().find_map(|t| {
            if t.is_effect() {
                Some(t)
            } else {
                t.effect_inside()
            }
        })
    }

    /// The first shared-state type anywhere in this type, by name.
    pub(crate) fn shared_state(&self) -> Option<&'static str> {
        if let Ty::Std(name, _) = self
            && SHARED.contains(name)
        {
            return Some(name);
        }
        self.children().find_map(Ty::shared_state)
    }

    fn children(&self) -> Box<dyn Iterator<Item = &Ty> + '_> {
        match self {
            Ty::Tuple(elems) | Ty::Std(_, elems) => Box::new(elems.iter()),
            Ty::Ref { inner, .. } | Ty::Slice(inner) | Ty::Outlet(inner) => {
                Box::new(std::iter::once(&**inner))
            }
            Ty::Table(k, v) => Box::new([&**k, &**v].into_iter()),
            _ => Box::new(std::iter::empty()),
        }
    }

    /// The type behind any references.
    pub(crate) fn deref(&self) -> &Ty {
        match self {
            Ty::Ref { inner, .. } => inner.deref(),
            _ => self,
        }
    }
}

/// Whether a value of type `found` can go where `expected` is wanted. Unknown
/// types fit anywhere, and Rust's deref coercions from `&String` to `&str`
/// and from `&Vec<T>` to `&[T]` are allowed.
pub(crate) fn fits(expected: &Ty, found: &Ty) -> bool {
    match (expected, found) {
        (Ty::Unknown, _) | (_, Ty::Unknown) | (_, Ty::Never) => true,
        (Ty::Int(_), Ty::Int(None)) | (Ty::Int(None), Ty::Int(_)) => true,
        (Ty::Float(_), Ty::Float(None)) | (Ty::Float(None), Ty::Float(_)) => true,
        (
            Ty::Ref {
                mutable: false,
                inner: e,
            },
            Ty::Ref { inner: f, .. },
        ) => {
            fits(e, f)
                || matches!((&**e, &**f), (Ty::Str, Ty::String))
                || matches!((&**e, &**f), (Ty::Slice(e), Ty::Std("Vec", f)) if fits(e, &f[0]))
        }
        (
            Ty::Ref {
                mutable: true,
                inner: e,
            },
            Ty::Ref {
                mutable: true,
                inner: f,
            },
        ) => fits(e, f),
        (Ty::Tuple(e), Ty::Tuple(f)) | (Ty::Std(_, e), Ty::Std(_, f)) => {
            e.len() == f.len()
                && std_name(expected) == std_name(found)
                && e.iter().zip(f).all(|(e, f)| fits(e, f))
        }
        (Ty::Slice(e), Ty::Slice(f)) | (Ty::Outlet(e), Ty::Outlet(f)) => fits(e, f),
        (Ty::Table(ek, ev), Ty::Table(fk, fv)) => fits(ek, fk) && fits(ev, fv),
        _ => expected == found,
    }
}

fn std_name(ty: &Ty) -> Option<&'static str> {
    match ty {
        Ty::Std(name, _) => Some(name),
        _ => None,
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn list(f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
            for (i, ty) in tys.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{ty}")?;
            }
            Ok(())
        }
        match self {
            Ty::Unknown => f.write_str("_"),
            Ty::Never => f.write_str("!"),
            Ty::Bool => f.write_str("bool"),
            Ty::Char => f.write_str("char"),
            Ty::Int(Some(name)) | Ty::Float(Some(name)) => f.write_str(name),
            Ty::Int(None) => f.write_str("integer"),
            Ty::Float(None) => f.write_str("float"),
            Ty::Str => f.write_str("str"),
            Ty::String => f.write_str("String"),
            Ty::Decimal => f.write_str("Decimal"),
            Ty::Tuple(elems) => {
                f.write_str("(")?;
                list(f, elems)?;
                if elems.len() == 1 {
                    f.write_str(",")?;
                }
                f.write_str(")")
            }
            Ty::Ref { mutable, inner } => {
                write!(f, "&{}{inner}", if *mutable { "mut " } else { "" })
            }
            Ty::Slice(inner) => write!(f, "[{inner}]"),
            Ty::Std(name, args) => {
                write!(f, "{name}<")?;
                list(f, args)?;
                f.write_str(">")
            }
            Ty::HttpServer => f.write_str("HttpServer"),
            Ty::HttpClient => f.write_str("HttpClient"),
            Ty::Table(k, v) => write!(f, "Table<{k}, {v}>"),
            Ty::Handle(_, component) => write!(f, "Handle<{component}>"),
            Ty::Outlet(inner) => write!(f, "Outlet<{inner}>"),
            Ty::Data { name, .. } => f.write_str(name),
        }
    }
}

/// Resolves written types in one module, reporting names that are not types.
pub(crate) struct Resolver<'c, 'a> {
    pub names: &'c Names<'a>,
    pub module: usize,
}

impl Resolver<'_, '_> {
    pub(crate) fn resolve(&self, ty: &ast::Type, diagnostics: &mut Vec<Diagnostic>) -> Ty {
        match &ty.kind {
            TypeKind::Infer => Ty::Unknown,
            TypeKind::Tuple(elems) => {
                Ty::Tuple(elems.iter().map(|t| self.resolve(t, diagnostics)).collect())
            }
            TypeKind::Ref { mutable, inner } => Ty::Ref {
                mutable: *mutable,
                inner: Box::new(self.resolve(inner, diagnostics)),
            },
            TypeKind::Slice(inner) => Ty::Slice(Box::new(self.resolve(inner, diagnostics))),
            TypeKind::Path(path) => self.resolve_path(path, diagnostics),
        }
    }

    fn resolve_path(&self, path: &ast::Path, diagnostics: &mut Vec<Diagnostic>) -> Ty {
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        let last = &path.segments[path.segments.len() - 1];
        let name = match self.names.resolve_in(self.module, &idents, Ns::Type) {
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                return Ty::Unknown;
            }
            Ok(Some(Def::Std(name))) => name,
            Ok(Some(def)) => {
                if !last.generics.is_empty() {
                    diagnostics.push(Diagnostic::new(
                        format!("`{}` takes no type arguments", last.ident.name),
                        path.span,
                        "a program's own types are not generic",
                    ));
                }
                return match def {
                    Def::Data(id) => data_ty(self.names, id),
                    Def::Component(_) | Def::App => {
                        diagnostics.push(
                            Diagnostic::new(
                                format!("`{}` is a component, not a type", last.ident.name),
                                path.span,
                                "a component is started, not held as a value",
                            )
                            .with_help(format!(
                                "a component's handle is a `Handle<{}>`",
                                last.ident.name
                            )),
                        );
                        Ty::Unknown
                    }
                    _ => {
                        diagnostics.push(unknown_type(path));
                        Ty::Unknown
                    }
                };
            }
            Ok(None) if path.segments.len() == 1 => last.ident.name.clone(),
            Ok(None) => {
                diagnostics.push(unknown_type(path));
                return Ty::Unknown;
            }
        };
        let name = name.as_str();
        let args = &last.generics;
        let arity = |expected: usize, diagnostics: &mut Vec<Diagnostic>| {
            if args.len() == expected {
                return true;
            }
            diagnostics.push(Diagnostic::new(
                format!(
                    "`{name}` takes {expected} type argument{}, but {} {} given",
                    if expected == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "is" } else { "are" },
                ),
                path.span,
                format!("expected {expected}"),
            ));
            false
        };
        if let Some(int) = INTS.iter().find(|i| **i == name) {
            return if arity(0, diagnostics) {
                Ty::Int(Some(int))
            } else {
                Ty::Unknown
            };
        }
        if let Some(float) = FLOATS.iter().find(|f| **f == name) {
            return if arity(0, diagnostics) {
                Ty::Float(Some(float))
            } else {
                Ty::Unknown
            };
        }
        let simple = match name {
            "bool" => Some(Ty::Bool),
            "char" => Some(Ty::Char),
            "str" => Some(Ty::Str),
            "String" => Some(Ty::String),
            "Decimal" => Some(Ty::Decimal),
            "HttpServer" => Some(Ty::HttpServer),
            "HttpClient" => Some(Ty::HttpClient),
            "Self" => Some(Ty::Unknown),
            _ => None,
        };
        if let Some(ty) = simple {
            return if arity(0, diagnostics) {
                ty
            } else {
                Ty::Unknown
            };
        }
        if let Some((name, expected)) = STD_GENERICS.iter().find(|(n, _)| *n == name) {
            if !arity(*expected, diagnostics) {
                return Ty::Unknown;
            }
            let args = args.iter().map(|t| self.resolve(t, diagnostics)).collect();
            return Ty::Std(name, args);
        }
        match name {
            "Table" if arity(2, diagnostics) => Ty::Table(
                Box::new(self.resolve(&args[0], diagnostics)),
                Box::new(self.resolve(&args[1], diagnostics)),
            ),
            "Outlet" if arity(1, diagnostics) => {
                Ty::Outlet(Box::new(self.resolve(&args[0], diagnostics)))
            }
            "Handle" if arity(1, diagnostics) => {
                let target = match &args[0].kind {
                    TypeKind::Path(p) => {
                        let idents: Vec<Ident> =
                            p.segments.iter().map(|s| s.ident.clone()).collect();
                        match self.names.resolve_in(self.module, &idents, Ns::Type) {
                            Ok(Some(Def::Component(c))) => Some(c),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match target {
                    Some(c) => Ty::Handle(c, self.names.components[c].ast.name.name.clone()),
                    _ => {
                        diagnostics.push(
                            Diagnostic::new(
                                "`Handle` takes a component",
                                args[0].span,
                                "not a component of this program",
                            )
                            .with_help("write `Handle<C>` with the name of a component, such as `Handle<Reorders>`"),
                        );
                        Ty::Unknown
                    }
                }
            }
            "Table" | "Outlet" | "Handle" => Ty::Unknown,
            _ => {
                diagnostics.push(unknown_type(path));
                Ty::Unknown
            }
        }
    }
}

fn unknown_type(path: &ast::Path) -> Diagnostic {
    let name = path
        .segments
        .iter()
        .map(|s| s.ident.name.as_str())
        .collect::<Vec<_>>()
        .join("::");
    Diagnostic::new(
        format!("cannot find type `{name}`"),
        path.span,
        "not a type this program knows",
    )
    .with_help(
        "declare it as a `struct` or `enum`, or import it with `use`; the checker also knows \
         Rust's primitive and standard types, `Decimal`, the capability types and `Handle` and `Outlet`",
    )
}

/// The type of a struct or enum the program declares.
pub(crate) fn data_ty(names: &Names, id: usize) -> Ty {
    let decl = &names.datas[id];
    let derives = derives(decl.item);
    Ty::Data {
        id,
        name: decl.ast.name().name.clone(),
        copy: derives.iter().any(|d| d.name == "Copy"),
        clone: derives.iter().any(|d| d.name == "Clone"),
    }
}

/// The traits an item's `#[derive]` attributes list.
pub(crate) fn derives(item: &ast::Item) -> Vec<&Ident> {
    item.attrs
        .iter()
        .filter_map(|a| match &a.kind {
            ast::AttributeKind::Derive(names) => Some(names.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}
