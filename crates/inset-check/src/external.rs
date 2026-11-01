//! External components: an `extern component`'s `#[rust]` attribute, and
//! its messages, whose values must cross into a WebAssembly component of
//! its own.
//!
//! Each external component is built as a WebAssembly component from a WIT
//! world the compiler writes from its declaration, and a language's
//! bindings generator turns that world into the code its author implements
//! (`docs/language/runtime.md`, "External components"). So a message's
//! parameters and reply must be types WIT can describe, and names that
//! survive the trip into WIT and back out of the bindings unchanged: the
//! Rust author writes `Row { line, fields }` against the generated types,
//! and the host matches the stored form's names against WIT's.
//!
//! WIT names are kebab-case, and `wit-bindgen` turns them back into Rust's
//! cases. A field `on_hand` is `on-hand` in WIT and `on_hand` again in
//! Rust; a type or variant `InvalidRows` is `invalid-rows` and `InvalidRows`
//! again. A name that would come back different, such as `line_2`, whose
//! WIT word `2` cannot start with a digit, is refused here.
//!
//! [`externals`] describes each external component for the compiler, which
//! writes its WIT world and its bindings from the description alone.

use inset_syntax::ast::{self, AttributeKind, FailureConditionKind, FailureResponse, PatternKind};
use inset_syntax::{Diagnostic, Span};

use crate::modules::Modules;
use crate::names::DataAst;
use crate::types::Ty;
use crate::{Checker, FnKind, Program, Report};

/// An external component, as the compiler needs it to write its WIT world,
/// its language's bindings and the program's side of the boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct External {
    /// The component's name, such as `CsvReader`.
    pub name: String,
    pub doc: Option<String>,
    /// Where it is declared: the file's index in the program, and the
    /// span of its name in that file.
    pub file: usize,
    pub span: Span,
    /// The Rust file that implements it, from the program's directory,
    /// such as `src/csv_reader.rs`.
    pub path: String,
    /// The crates the file may use, each as a name and a version requirement.
    pub crates: Vec<(String, String)>,
    pub messages: Vec<ExternalMessage>,
    /// The program's data types the messages carry, directly or inside one
    /// another, in the order the program declares them.
    pub types: Vec<ExternalType>,
}

/// A `#[message]` of an external component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalMessage {
    pub name: String,
    pub doc: Option<String>,
    /// Whether it takes `&mut self`.
    pub mutable: bool,
    pub params: Vec<ExternalParam>,
    /// The reply, or `None` for `()`.
    pub ret: Option<ExternalReply>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalParam {
    pub name: String,
    /// The value's type as WIT carries it: `&str` is a `string`.
    pub ty: WitType,
    /// The type as the signature declares it, in Rust, with the program's
    /// types by their names alone, as the bindings hold them: `&str`.
    pub rust: String,
    /// Whether it is lent, `&T`, so the bindings' owned value is passed on
    /// as a borrow.
    pub lent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalReply {
    pub ty: WitType,
    /// As [`ExternalParam::rust`].
    pub rust: String,
}

/// A type WIT can carry. Names are Inset's; the compiler writes them in
/// WIT's kebab case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WitType {
    Bool,
    /// An integer: whether it is signed, and its width in bits.
    Int {
        signed: bool,
        bits: u8,
    },
    F32,
    F64,
    Char,
    String,
    List(Box<WitType>),
    Option(Box<WitType>),
    /// `Result<T, E>`, with `None` for `()`.
    Result(Option<Box<WitType>>, Option<Box<WitType>>),
    Tuple(Vec<WitType>),
    /// One of the program's data types, by its name.
    Data(String),
}

/// A struct or enum an external component's messages carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalType {
    pub name: String,
    pub doc: Option<String>,
    pub kind: ExternalTypeKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalTypeKind {
    /// A struct: each field's name, doc comment and type.
    Record(Vec<(String, Option<String>, WitType)>),
    /// An enum: each variant's name, doc comment and what it holds, if anything.
    Variant(Vec<(String, Option<String>, Option<WitType>)>),
}

/// An external component's `#[rust]` attribute, resolved, for the check
/// against the program's files.
pub(crate) struct RustSource {
    /// From the program's directory, such as `src/csv_reader.rs`.
    pub path: String,
    pub span: Span,
}

/// Checks the program, and describes its external components if it checks clean.
pub fn externals(program: &Program) -> Result<Vec<External>, Vec<Report>> {
    let (modules, mut reports) = Modules::load(program);
    let mut checker = Checker::new(&modules);
    checker.check();
    crate::finish(
        program,
        &modules,
        &mut checker,
        &mut reports,
        &mut Vec::new(),
    );
    if !reports.is_empty() {
        return Err(reports);
    }
    Ok(describe(&checker))
}

/// Describes each external component of a program that checked clean.
pub(crate) fn describe(cx: &Checker) -> Vec<External> {
    let mut externals = Vec::new();
    for (index, comp) in cx.components.iter().enumerate() {
        if comp.ast.external.is_none() {
            continue;
        }
        let decl = &cx.names.components[index];
        let Some(rust) = rust_attr(decl.item) else {
            continue;
        };
        let module = &cx.names.modules.list[comp.module];
        let mut used = Vec::new();
        let mut messages = Vec::new();
        for sig in &comp.functions {
            let mut params = Vec::new();
            for (param, ty) in sig.ast.params.iter().zip(&sig.params) {
                let PatternKind::Binding { name, .. } = &param.pattern.kind else {
                    continue;
                };
                let lent = matches!(ty, Ty::Ref { .. });
                let Ok(wit) = wit_type(cx, ty, true, &mut used) else {
                    continue;
                };
                params.push(ExternalParam {
                    name: name.name.clone(),
                    ty: wit,
                    rust: rust_text(ty),
                    lent,
                });
            }
            let ret = if sig.ret.is_unit() {
                None
            } else {
                wit_type(cx, &sig.ret, false, &mut used)
                    .ok()
                    .map(|ty| ExternalReply {
                        ty,
                        rust: rust_text(&sig.ret),
                    })
            };
            messages.push(ExternalMessage {
                name: sig.name().to_string(),
                doc: doc(&sig.ast.docs),
                mutable: sig.ast.receiver.as_ref().is_some_and(|r| r.mutable),
                params,
                ret,
            });
        }
        used.sort_unstable();
        used.dedup();
        let types = used.into_iter().map(|id| external_type(cx, id)).collect();
        externals.push(External {
            name: comp.name().to_string(),
            doc: doc(&decl.item.docs),
            file: module.file,
            span: comp.ast.name.span.unshifted(module.base),
            path: resolve_path(&module.file_path, &rust.path.value).unwrap_or_default(),
            crates: rust
                .crates
                .iter()
                .filter_map(|c| {
                    let (name, version) = c.value.split_once('@')?;
                    Some((name.to_string(), version.to_string()))
                })
                .collect(),
            messages,
            types,
        });
    }
    externals
}

fn external_type(cx: &Checker, id: usize) -> ExternalType {
    let data = &cx.datas[id];
    let decl = &cx.names.datas[id];
    let mut ignored = Vec::new();
    let mut wit = |ty: &Ty| wit_type(cx, ty, false, &mut ignored).unwrap_or(WitType::Bool);
    let kind = match decl.ast {
        DataAst::Struct(s) => ExternalTypeKind::Record(
            s.fields
                .iter()
                .zip(&data.fields)
                .map(|(ast, field)| (field.name.name.clone(), doc(&ast.docs), wit(&field.ty)))
                .collect(),
        ),
        DataAst::Enum(e) => ExternalTypeKind::Variant(
            e.variants
                .iter()
                .zip(&data.variants)
                .map(|(ast, variant)| {
                    (
                        variant.name.name.clone(),
                        doc(&ast.docs),
                        variant.fields.first().map(&mut wit),
                    )
                })
                .collect(),
        ),
    };
    ExternalType {
        name: data.name.name.clone(),
        doc: doc(&decl.item.docs),
        kind,
    }
}

fn doc(docs: &[ast::Doc]) -> Option<String> {
    (!docs.is_empty()).then(|| {
        docs.iter()
            .map(|d| d.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn rust_attr(item: &ast::Item) -> Option<&ast::RustAttr> {
    item.attrs.iter().find_map(|a| match &a.kind {
        AttributeKind::Rust(rust) => Some(rust),
        _ => None,
    })
}

/// A path written relative to the file `from`, as a path from the
/// program's directory, or `None` if it leaves the file's directory or is
/// not a relative path.
fn resolve_path(from: &str, path: &str) -> Option<String> {
    let dir = from.rsplit_once('/').map_or("", |(dir, _)| dir);
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return None;
    }
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The type as Rust writes it, with the program's data types by their names
/// alone, as the bindings declare them.
fn rust_text(ty: &Ty) -> String {
    ty.to_string()
}

impl Checker<'_> {
    /// Checks each external component's `#[rust]` attribute, fields and
    /// messages.
    pub(crate) fn check_externals(&mut self) {
        let mut errors = Vec::new();
        let mut sources = Vec::new();
        for (index, comp) in self.components.iter().enumerate() {
            let Some(external) = comp.ast.external else {
                continue;
            };
            let decl = &self.names.components[index];
            let module = &self.names.modules.list[comp.module];
            match rust_attr(decl.item) {
                Some(rust) => {
                    if let Some(source) = check_rust_attr(rust, &module.file_path, &mut errors) {
                        sources.push(source);
                    }
                }
                None => errors.push(
                    Diagnostic::new(
                        format!("`{}` does not say what implements it", comp.name()),
                        external.to(comp.ast.name.span),
                        "an external component's code is written in another language",
                    )
                    .with_help(
                        "name the Rust file that implements it, and the crates the file uses, \
                         above it: `#[rust(path = \"name.rs\", crates = [\"name@version\"])]`",
                    ),
                ),
            }
            if let Some(field) = comp.ast.fields.first() {
                errors.push(
                    Diagnostic::new(
                        "an external component with fields is not supported yet",
                        field.span,
                        "its fields would be the capabilities it holds",
                    )
                    .with_help(
                        "give the capability to an Inset component, which sends the external \
                         component what it needs to compute",
                    ),
                );
            }
            let earlier = self.components[..index]
                .iter()
                .any(|c| c.ast.external.is_some() && c.name() == comp.name());
            if earlier {
                errors.push(
                    Diagnostic::new(
                        format!("two external components are named `{}`", comp.name()),
                        comp.ast.name.span,
                        "each external component is built as a file named after it",
                    )
                    .with_help("rename one of them"),
                );
            }
            if wit_type_name(comp.name()).is_none() {
                errors.push(name_error("type", comp.name(), comp.ast.name.span));
            }
            if is_reserved_crate(comp.name()) {
                errors.push(Diagnostic::new(
                    format!("an external component cannot be named `{}`", comp.name()),
                    comp.ast.name.span,
                    "the generated bindings use this name",
                ));
            }
            let mut used = Vec::new();
            for sig in &comp.functions {
                self.check_external_message(sig, &mut used, &mut errors);
            }
            used.sort_unstable();
            used.dedup();
            check_type_names(self, comp, &used, &mut errors);
        }
        self.rust_sources = sources;
        self.diagnostics.extend(errors);
    }

    fn check_external_message(
        &self,
        sig: &crate::Sig,
        used: &mut Vec<usize>,
        errors: &mut Vec<Diagnostic>,
    ) {
        let function = sig.ast;
        if sig.kind != FnKind::Message {
            let span = function
                .attrs
                .iter()
                .find(|a| matches!(a.kind, AttributeKind::Route(_)))
                .map_or(function.name.span, |a| a.span);
            errors.push(
                Diagnostic::new(
                    "an external component's functions are messages",
                    span,
                    "other components reach it only through `#[message]` functions",
                )
                .with_help("write `#[message]` above the signature"),
            );
        }
        if function.receiver.is_none() {
            errors.push(
                Diagnostic::new(
                    format!("the message `{}` takes no `self`", function.name.name),
                    function.name.span,
                    "a message is handled by the running component",
                )
                .with_help("take `&self`, or `&mut self` to change the component's state"),
            );
        }
        if function.is_async {
            errors.push(
                Diagnostic::new(
                    "an external component's message is not `async`",
                    function.name.span,
                    "it runs to completion in its own WebAssembly component",
                )
                .with_help("remove `async`; the sender still awaits the reply"),
            );
        }
        if function.name.name == "new" {
            errors.push(
                Diagnostic::new(
                    "an external component's message cannot be named `new`",
                    function.name.span,
                    "the generated trait's `new` starts the component",
                )
                .with_help("give the message another name"),
            );
        } else if wit_field_name(&function.name.name).is_none() {
            errors.push(name_error(
                "message",
                &function.name.name,
                function.name.span,
            ));
        }
        for (param, ty) in function.params.iter().zip(&sig.params) {
            match &param.pattern.kind {
                PatternKind::Binding { name, .. } if wit_field_name(&name.name).is_none() => {
                    errors.push(name_error("parameter", &name.name, name.span));
                }
                PatternKind::Binding { .. } => {}
                _ => errors.push(Diagnostic::new(
                    "an external component's parameters are names",
                    param.pattern.span,
                    "the generated trait names each parameter",
                )),
            }
            if let Err((ty, why)) = wit_type(self, ty, true, used) {
                errors.push(carry_error(&ty, why, param.ty.span));
            }
        }
        if let Some(ret) = &function.ret
            && !sig.ret.is_unit()
            && let Err((ty, why)) = wit_type(self, &sig.ret, false, used)
        {
            errors.push(carry_error(&ty, why, ret.span));
        }
    }

    /// Checks the `failure` arms that name an external child: only `restart`
    /// is allowed for one, since its state is code the compiler cannot see.
    pub(crate) fn check_external_failures(&mut self, index: usize) {
        let comp = &self.components[index];
        let Some(failure) = &comp.ast.failure else {
            return;
        };
        let mut errors = Vec::new();
        for arm in &failure.arms {
            let FailureConditionKind::Child(name) = &arm.condition.kind else {
                continue;
            };
            let external = comp
                .start_of(&name.name)
                .and_then(|start| self.started(start))
                .is_some_and(|c| self.components[c].ast.external.is_some());
            if external && arm.response == FailureResponse::Rollback {
                errors.push(
                    Diagnostic::new(
                        format!("`{}` is external, so it cannot be rolled back", name.name),
                        Span::new(arm.condition.span.start, arm.span.end),
                        "its state is code the compiler cannot see",
                    )
                    .with_help(format!(
                        "respond with `Child({}) => restart`, which starts it afresh",
                        name.name
                    )),
                );
            }
        }
        self.diagnostics.extend(errors);
    }

    /// Reports a `#[rust]` path naming no Rust file of the program, when
    /// the program's files are known.
    pub(crate) fn check_rust_files(&mut self, rust: Option<&[String]>) {
        let Some(rust) = rust else {
            return;
        };
        let mut errors = Vec::new();
        for source in &self.rust_sources {
            if !rust.contains(&source.path) {
                errors.push(
                    Diagnostic::new(
                        format!("there is no file `{}`", source.path),
                        source.span,
                        "no Rust file has this path",
                    )
                    .with_help(
                        "the path is relative to the file declaring the component; \
                         write the Rust that implements it there",
                    ),
                );
            }
        }
        self.diagnostics.extend(errors);
    }
}

fn check_rust_attr(
    rust: &ast::RustAttr,
    declared_in: &str,
    errors: &mut Vec<Diagnostic>,
) -> Option<RustSource> {
    let mut seen: Vec<&str> = Vec::new();
    for krate in &rust.crates {
        let Some((name, version)) = krate.value.split_once('@') else {
            errors.push(
                Diagnostic::new(
                    format!("`{}` names no version", krate.value),
                    krate.span,
                    "a crate is written `name@version`",
                )
                .with_help(format!(
                    "write the version the file needs, such as \"{}@1.0\"",
                    krate.value
                )),
            );
            continue;
        };
        if !is_crate_name(name) {
            errors.push(Diagnostic::new(
                format!("`{name}` is not a crate's name"),
                krate.span,
                "a crate's name is letters, digits, `-` and `_`",
            ));
        } else if is_reserved_crate(name) {
            errors.push(Diagnostic::new(
                format!("the crate `{name}` cannot be used"),
                krate.span,
                "the generated bindings use this name",
            ));
        } else if seen.contains(&name) {
            errors.push(Diagnostic::new(
                format!("the crate `{name}` is named twice"),
                krate.span,
                "this repeats an earlier crate",
            ));
        }
        seen.push(name);
        if !is_version(version) {
            errors.push(
                Diagnostic::new(
                    format!("`{version}` is not a version"),
                    krate.span,
                    "a version is numbers separated by `.`, as in Cargo.toml",
                )
                .with_help(format!(
                    "write a version such as \"{name}@1.3\"; `inset.lock` keeps the exact one chosen"
                )),
            );
        }
    }
    let path = &rust.path;
    if !path.value.ends_with(".rs") {
        errors.push(
            Diagnostic::new(
                "`path` names a Rust file",
                path.span,
                "this does not end in `.rs`",
            )
            .with_help("name the `.rs` file that implements the component"),
        );
        return None;
    }
    let Some(resolved) = resolve_path(declared_in, &path.value) else {
        errors.push(
            Diagnostic::new(
                "`path` is relative to the file declaring the component",
                path.span,
                "this leaves the file's directory",
            )
            .with_help("put the Rust file beside the declaring file, or below it"),
        );
        return None;
    };
    Some(RustSource {
        path: resolved,
        span: path.span,
    })
}

fn is_crate_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The names the generated bindings take for themselves.
fn is_reserved_crate(name: &str) -> bool {
    matches!(name, "interface" | "wit-bindgen" | "wit_bindgen")
}

/// A version requirement as Cargo.toml writes the simplest one: `1`, `1.3`
/// or `1.3.0`.
fn is_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() <= 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// The WIT name of a field, parameter or message, such as `on-hand` for
/// `on_hand`, if `wit-bindgen` would give the same name back.
pub fn wit_field_name(name: &str) -> Option<String> {
    let words: Vec<&str> = name.split('_').collect();
    words
        .iter()
        .all(|w| {
            w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && w.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
        .then(|| words.join("-"))
}

/// The WIT name of a type or variant, such as `invalid-rows` for
/// `InvalidRows`, if `wit-bindgen` would give the same name back.
pub fn wit_type_name(name: &str) -> Option<String> {
    let mut words: Vec<String> = Vec::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            words.push(c.to_ascii_lowercase().to_string());
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            words.last_mut()?.push(c);
        } else {
            return None;
        }
    }
    (!words.is_empty()).then(|| words.join("-"))
}

fn name_error(what: &str, name: &str, span: Span) -> Diagnostic {
    let help = if what == "type" || what == "variant" {
        "an external component's types and variants are named in `UpperCamelCase`, \
         with no `_`, such as `CsvRow`"
    } else {
        "an external component's messages, parameters and fields are named in `snake_case`, \
         with each word starting with a letter, such as `line_two`"
    };
    Diagnostic::new(
        format!("the {what} `{name}` has no name in WIT"),
        span,
        "an external component's bindings are generated from WIT, which cannot keep this name",
    )
    .with_help(help)
}

/// Why a type cannot cross into an external component.
enum Refused {
    /// WIT has no such type.
    Never(&'static str),
    /// A type WIT could carry, which Inset does not carry there yet.
    NotYet,
}

fn carry_error(ty: &Ty, why: Refused, span: Span) -> Diagnostic {
    match why {
        Refused::Never(help) => Diagnostic::new(
            format!("`{ty}` cannot cross into an external component"),
            span,
            "the WebAssembly component model has no such type",
        )
        .with_help(help),
        Refused::NotYet => Diagnostic::new(
            format!("`{ty}` in an external component's message is not supported yet"),
            span,
            "the generated bindings cannot give the Rust code this type yet",
        )
        .with_help(
            "carry it as a type the bindings give back exactly: a `String`, a number, \
             `bool`, `char`, a `Vec`, an `Option`, a `Result`, a tuple, or a struct or enum \
             of the program",
        ),
    }
}

/// The WIT type of a message's parameter (`param`) or reply, and the
/// program's data types it uses, added to `used`. A parameter may be lent
/// at its top level: `&str`, `&[T]` or `&T`.
fn wit_type(
    cx: &Checker,
    ty: &Ty,
    param: bool,
    used: &mut Vec<usize>,
) -> Result<WitType, (Ty, Refused)> {
    let ty = match ty {
        Ty::Ref {
            mutable: false,
            inner,
        } if param => match &**inner {
            Ty::Str => return Ok(WitType::String),
            Ty::Slice(elem) => {
                return Ok(WitType::List(Box::new(wit_type(cx, elem, false, used)?)));
            }
            inner => inner,
        },
        ty => ty,
    };
    let mut path = Vec::new();
    wit_inner(cx, ty, used, &mut path)
}

fn wit_inner(
    cx: &Checker,
    ty: &Ty,
    used: &mut Vec<usize>,
    path: &mut Vec<usize>,
) -> Result<WitType, (Ty, Refused)> {
    let refuse = |why| Err((ty.clone(), why));
    let inner = |t: &Ty, used: &mut Vec<usize>, path: &mut Vec<usize>| {
        wit_inner(cx, t, used, path).map(Box::new)
    };
    Ok(match ty {
        Ty::Bool => WitType::Bool,
        Ty::Char => WitType::Char,
        Ty::String => WitType::String,
        Ty::Int(Some(name)) => {
            let signed = name.starts_with('i');
            match name[1..].parse::<u8>() {
                Ok(bits @ (8 | 16 | 32 | 64)) => WitType::Int { signed, bits },
                _ => {
                    return refuse(Refused::Never(
                        "use an integer of 8, 16, 32 or 64 bits, such as `u64`",
                    ));
                }
            }
        }
        Ty::Float(Some("f32")) => WitType::F32,
        Ty::Float(Some(_)) => WitType::F64,
        Ty::Std("Vec", args) => WitType::List(inner(&args[0], used, path)?),
        Ty::Std("Option", args) => WitType::Option(inner(&args[0], used, path)?),
        Ty::Std("Result", args) => {
            let side = |t: &Ty, used: &mut Vec<usize>, path: &mut Vec<usize>| {
                if t.is_unit() {
                    Ok(None)
                } else {
                    wit_inner(cx, t, used, path).map(|w| Some(Box::new(w)))
                }
            };
            WitType::Result(side(&args[0], used, path)?, side(&args[1], used, path)?)
        }
        Ty::Tuple(elems) if !elems.is_empty() => WitType::Tuple(
            elems
                .iter()
                .map(|t| wit_inner(cx, t, used, path))
                .collect::<Result<_, _>>()?,
        ),
        Ty::Data { id, name, .. } => {
            if path.contains(id) {
                return refuse(Refused::Never(
                    "a type cannot contain itself in a message to an external component; \
                     carry the values as a list instead",
                ));
            }
            let data = &cx.datas[*id];
            if data.is_enum && data.variants.is_empty() {
                return refuse(Refused::Never("give the enum a variant"));
            }
            if !data.is_enum && data.fields.is_empty() {
                return refuse(Refused::Never("give the struct a field"));
            }
            if data.variants.iter().any(|v| v.fields.len() > 1) {
                return refuse(Refused::NotYet);
            }
            path.push(*id);
            let fields = data
                .fields
                .iter()
                .map(|f| &f.ty)
                .chain(data.variants.iter().flat_map(|v| &v.fields));
            for field in fields {
                wit_inner(cx, field, used, path)?;
            }
            path.pop();
            used.push(*id);
            WitType::Data(name.clone())
        }
        Ty::Decimal | Ty::Std("VecDeque" | "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet", _) => {
            return refuse(Refused::NotYet);
        }
        Ty::Unknown => WitType::Bool,
        Ty::Tuple(_) => {
            return refuse(Refused::Never(
                "`()` is carried only as a message's reply, by returning nothing",
            ));
        }
        _ => {
            return refuse(Refused::Never(
                "carry a `String`, a number, `bool`, `char`, a `Vec`, an `Option`, a \
                 `Result`, a tuple, or a struct or enum of the program",
            ));
        }
    })
}

/// Checks the names of the data types an external component's messages
/// carry, and of their fields and variants.
fn check_type_names(
    cx: &Checker,
    comp: &crate::Comp,
    used: &[usize],
    errors: &mut Vec<Diagnostic>,
) {
    let resource = wit_type_name(comp.name());
    let mut names: Vec<(String, &str)> = Vec::new();
    for &id in used {
        let data = &cx.datas[id];
        let name = &data.name;
        match wit_type_name(&name.name) {
            None => errors.push(name_error("type", &name.name, name.span)),
            Some(wit) => {
                if let Some((_, other)) = names.iter().find(|(n, _)| *n == wit) {
                    errors.push(Diagnostic::new(
                        format!(
                            "`{}` and `{other}` have one name in the messages of `{}`",
                            name.name,
                            comp.name()
                        ),
                        comp.ast.name.span,
                        "the generated bindings hold every type the messages carry in one place",
                    ));
                } else if Some(&wit) == resource.as_ref() {
                    errors.push(Diagnostic::new(
                        format!("`{}` has the name of the external component", name.name),
                        comp.ast.name.span,
                        "the generated bindings would declare two things of this name",
                    ));
                }
                names.push((wit, &name.name));
            }
        }
        for field in &data.fields {
            if wit_field_name(&field.name.name).is_none() {
                errors.push(name_error("field", &field.name.name, field.name.span));
            }
        }
        for variant in &data.variants {
            if wit_type_name(&variant.name.name).is_none() {
                errors.push(name_error("variant", &variant.name.name, variant.name.span));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_path, wit_field_name, wit_type_name};

    #[test]
    fn names_that_survive_wit() {
        assert_eq!(wit_field_name("on_hand").as_deref(), Some("on-hand"));
        assert_eq!(wit_field_name("line2").as_deref(), Some("line2"));
        assert_eq!(wit_field_name("line_2"), None);
        assert_eq!(wit_field_name("onHand"), None);
        assert_eq!(wit_field_name("_x"), None);
        assert_eq!(
            wit_type_name("InvalidRows").as_deref(),
            Some("invalid-rows")
        );
        assert_eq!(wit_type_name("HTTPError").as_deref(), Some("h-t-t-p-error"));
        assert_eq!(wit_type_name("V2").as_deref(), Some("v2"));
        assert_eq!(wit_type_name("csv"), None);
        assert_eq!(wit_type_name("Csv_Row"), None);
    }

    #[test]
    fn paths_from_the_declaring_file() {
        assert_eq!(
            resolve_path("src/main.inset", "csv_reader.rs").as_deref(),
            Some("src/csv_reader.rs")
        );
        assert_eq!(
            resolve_path("src/a/mod.inset", "./b/c.rs").as_deref(),
            Some("src/a/b/c.rs")
        );
        assert_eq!(resolve_path("src/main.inset", "../x.rs"), None);
        assert_eq!(resolve_path("src/main.inset", "/x.rs"), None);
    }
}
