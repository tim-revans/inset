//! Structs and enums: their fields and variants, `#[derive]`, and the
//! `#[status]` of an error enum's variants.
//!
//! Rust's own errors in the generated code are compiler bugs, so a derive
//! that rustc would refuse, such as `Copy` on a struct holding a `String`,
//! is reported here, where the checker knows the field's type.

use inset_syntax::ast::{AttributeKind, Visibility};
use inset_syntax::{Diagnostic, Ident, Span};

use crate::names::{DataAst, Vis};
use crate::types::{Resolver, Ty, derives};
use crate::{Checker, names, status};

/// The traits `#[derive]` may list (syntax.md, "Types").
pub(crate) const DERIVES: [&str; 11] = [
    "Clone",
    "Copy",
    "Debug",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Hash",
    "Default",
    "Serialize",
    "Deserialize",
];

/// What a trait needs derived beside it, as Rust requires.
const NEEDS: [(&str, &[&str]); 4] = [
    ("Copy", &["Clone"]),
    ("Eq", &["PartialEq"]),
    ("PartialOrd", &["PartialEq"]),
    ("Ord", &["PartialOrd", "Eq"]),
];

/// A struct or enum, with its types resolved.
pub(crate) struct Data {
    pub name: Ident,
    /// The struct's fields, or nothing for an enum.
    pub fields: Vec<DataField>,
    /// The enum's variants, or nothing for a struct.
    pub variants: Vec<DataVariant>,
    pub is_enum: bool,
    /// The traits it derives.
    pub derives: Vec<String>,
}

pub(crate) struct DataField {
    pub name: Ident,
    pub ty: Ty,
    pub vis: Vis,
    /// The declared type's span.
    pub ty_span: Span,
}

pub(crate) struct DataVariant {
    pub name: Ident,
    pub fields: Vec<Ty>,
    /// The `#[status(Name)]` on it.
    pub status: Option<Ident>,
}

impl Data {
    pub(crate) fn derives(&self, name: &str) -> bool {
        self.derives.iter().any(|d| d == name)
    }

    pub(crate) fn field(&self, name: &str) -> Option<&DataField> {
        self.fields.iter().find(|f| f.name.name == name)
    }

    /// The statuses its variants name, when it is an error enum: one with
    /// a `#[status]` on any variant. A variant without one is reported where
    /// the enum is declared.
    pub(crate) fn statuses(&self) -> Option<Vec<String>> {
        let statuses: Vec<String> = self
            .variants
            .iter()
            .filter_map(|v| v.status.as_ref().map(|s| s.name.clone()))
            .collect();
        (!statuses.is_empty()).then_some(statuses)
    }
}

impl Checker<'_> {
    pub(crate) fn collect_datas(&mut self) {
        let mut datas = Vec::new();
        for (index, decl) in self.names.datas.iter().enumerate() {
            let resolver = Resolver {
                names: &self.names,
                module: decl.module,
            };
            let mut errors = Vec::new();
            for attr in &decl.item.attrs {
                if !matches!(attr.kind, AttributeKind::Derive(_)) {
                    errors.push(Diagnostic::new(
                        "a struct or enum takes no attribute but `#[derive]`",
                        attr.span,
                        "remove this attribute",
                    ));
                }
            }
            let derived = derives(decl.item);
            let data = match decl.ast {
                DataAst::Struct(s) => {
                    let mut fields: Vec<DataField> = Vec::new();
                    for field in &s.fields {
                        if fields.iter().any(|f| f.name.name == field.name.name) {
                            errors.push(Diagnostic::new(
                                format!("the field `{}` is declared twice", field.name.name),
                                field.name.span,
                                "this repeats an earlier field",
                            ));
                            continue;
                        }
                        let ty = resolver.resolve(&field.ty, &mut errors);
                        let vis = match &field.vis {
                            Visibility::Pub(_) | Visibility::PubCrate(_) => Vis::Public,
                            Visibility::PubSuper(_) => Vis::In(
                                self.names
                                    .modules
                                    .parent(decl.module)
                                    .unwrap_or(decl.module),
                            ),
                            Visibility::Private => Vis::In(decl.module),
                        };
                        fields.push(DataField {
                            name: field.name.clone(),
                            ty,
                            vis,
                            ty_span: field.ty.span,
                        });
                    }
                    Data {
                        name: s.name.clone(),
                        fields,
                        variants: Vec::new(),
                        is_enum: false,
                        derives: derived.iter().map(|d| d.name.clone()).collect(),
                    }
                }
                DataAst::Enum(e) => {
                    let mut variants: Vec<DataVariant> = Vec::new();
                    for variant in &e.variants {
                        if variants.iter().any(|v| v.name.name == variant.name.name) {
                            errors.push(Diagnostic::new(
                                format!("the variant `{}` is declared twice", variant.name.name),
                                variant.name.span,
                                "this repeats an earlier variant",
                            ));
                            continue;
                        }
                        let mut status = None;
                        for attr in &variant.attrs {
                            match &attr.kind {
                                AttributeKind::Status(name) if status.is_none() => {
                                    status = Some(name.clone());
                                }
                                AttributeKind::Status(_) => errors.push(Diagnostic::new(
                                    "a variant has one `#[status]`",
                                    attr.span,
                                    "this is a second status",
                                )),
                                _ => errors.push(Diagnostic::new(
                                    "a variant takes no attribute but `#[status]`",
                                    attr.span,
                                    "remove this attribute",
                                )),
                            }
                        }
                        let fields = variant
                            .fields
                            .iter()
                            .map(|t| resolver.resolve(t, &mut errors))
                            .collect();
                        variants.push(DataVariant {
                            name: variant.name.clone(),
                            fields,
                            status,
                        });
                    }
                    Data {
                        name: e.name.clone(),
                        fields: Vec::new(),
                        variants,
                        is_enum: true,
                        derives: derived.iter().map(|d| d.name.clone()).collect(),
                    }
                }
            };
            self.check_derives(index, &data, &derived, &mut errors);
            if data.is_enum
                && let DataAst::Enum(e) = decl.ast
            {
                check_statuses(&data, e, &mut errors);
            }
            for field in &data.fields {
                check_held(&field.ty, field.ty_span, &mut errors);
            }
            if let DataAst::Enum(e) = decl.ast {
                for (variant, ast) in data.variants.iter().zip(&e.variants) {
                    for (ty, written) in variant.fields.iter().zip(&ast.fields) {
                        check_held(ty, written.span, &mut errors);
                    }
                }
            }
            self.diagnostics.extend(errors);
            datas.push(data);
        }
        self.datas = datas;
    }

    fn check_derives(
        &self,
        index: usize,
        data: &Data,
        derived: &[&Ident],
        errors: &mut Vec<Diagnostic>,
    ) {
        let kind = if data.is_enum { "enum" } else { "struct" };
        let mut seen: Vec<&str> = Vec::new();
        for name in derived {
            let trait_name = name.name.as_str();
            if !DERIVES.contains(&trait_name) {
                errors.push(
                    Diagnostic::new(
                        format!("Inset cannot derive `{trait_name}`"),
                        name.span,
                        "not a trait `#[derive]` may list",
                    )
                    .with_help(format!("derive any of {}", names(DERIVES.iter().copied()))),
                );
                continue;
            }
            if seen.contains(&trait_name) {
                errors.push(Diagnostic::new(
                    format!("`{trait_name}` is derived twice"),
                    name.span,
                    "this repeats an earlier derive",
                ));
                continue;
            }
            seen.push(trait_name);
            if let Some((_, needs)) = NEEDS.iter().find(|(t, _)| *t == trait_name) {
                let missing: Vec<&str> =
                    needs.iter().copied().filter(|n| !data.derives(n)).collect();
                if !missing.is_empty() {
                    errors.push(
                        Diagnostic::new(
                            format!(
                                "deriving `{trait_name}` needs {} too",
                                names(missing.iter().copied())
                            ),
                            name.span,
                            format!(
                                "Rust derives `{trait_name}` only beside {}",
                                names(needs.iter().copied())
                            ),
                        )
                        .with_help(format!(
                            "add {} to the `#[derive]`",
                            names(missing.iter().copied())
                        )),
                    );
                }
            }
            if trait_name == "Default" && data.is_enum {
                errors.push(Diagnostic::new(
                    "deriving `Default` for an enum is not supported yet",
                    name.span,
                    "the checker does not handle this yet",
                ));
                continue;
            }
            // Each field must have the trait itself.
            let fields: Vec<(&str, &Ty)> = data
                .fields
                .iter()
                .map(|f| (f.name.name.as_str(), &f.ty))
                .chain(
                    data.variants
                        .iter()
                        .flat_map(|v| v.fields.iter().map(move |t| (v.name.name.as_str(), t))),
                )
                .collect();
            for (field, ty) in fields {
                if self.implements(ty, trait_name, index) == Some(false) {
                    let what = if data.is_enum { "variant" } else { "field" };
                    errors.push(
                        Diagnostic::new(
                            format!(
                                "the {kind} `{}` cannot derive `{trait_name}`",
                                data.name.name
                            ),
                            name.span,
                            format!("its {what} `{field}` holds a `{ty}`, which is not `{trait_name}`"),
                        )
                        .with_help(format!(
                            "remove `{trait_name}` from the `#[derive]`, or change the type of `{field}`"
                        )),
                    );
                }
            }
        }
    }

    /// Whether a type has a derivable trait, or `None` if that is not known.
    /// `this` is the data type being declared, which has what it derives.
    pub(crate) fn implements(&self, ty: &Ty, trait_name: &str, this: usize) -> Option<bool> {
        let all = |tys: &[Ty]| {
            tys.iter().try_fold(true, |all, t| {
                Some(all && self.implements(t, trait_name, this)?)
            })
        };
        match ty {
            Ty::Unknown | Ty::Never => None,
            Ty::Data { id, .. } if *id == this => Some(true),
            Ty::Data { id, .. } => {
                let decl = &self.names.datas[*id];
                Some(derives(decl.item).iter().any(|d| d.name == trait_name))
            }
            _ if trait_name == "Serialize" || trait_name == "Deserialize" => None,
            Ty::HttpServer | Ty::Table(..) | Ty::Handle(..) | Ty::Outlet(_) => Some(false),
            Ty::Ref { mutable, inner } => match trait_name {
                "Copy" | "Clone" => Some(!mutable),
                "Default" => None,
                _ => self.implements(inner, trait_name, this),
            },
            Ty::Float(_) if matches!(trait_name, "Eq" | "Ord" | "Hash") => Some(false),
            Ty::Tuple(elems) => all(elems),
            Ty::Std(name, args) => match trait_name {
                "Copy" => Some(false),
                "Hash" | "Ord" | "PartialOrd" if matches!(*name, "HashMap" | "HashSet") => {
                    Some(false)
                }
                _ => all(args),
            },
            Ty::Slice(inner) => match trait_name {
                "Copy" | "Clone" | "Default" => Some(false),
                _ => self.implements(inner, trait_name, this),
            },
            Ty::String | Ty::Str if trait_name == "Copy" => Some(false),
            Ty::Str if trait_name == "Clone" || trait_name == "Default" => Some(false),
            _ => Some(true),
        }
    }
}

/// A struct's fields and an enum's variants hold data, never a capability,
/// handle or outlet.
fn check_held(ty: &Ty, span: Span, errors: &mut Vec<Diagnostic>) {
    let held = if ty.is_effect() {
        Some(ty)
    } else {
        ty.effect_inside()
    };
    if let Some(held) = held {
        errors.push(
            Diagnostic::new(
                format!("a struct or enum cannot hold a `{held}`"),
                span,
                "data types hold data, which can be copied and compared",
            )
            .with_help("capabilities, handles and outlets are held only by a component's fields"),
        );
    }
}

/// An error enum's `#[status]` attributes: on every variant or on none, each
/// naming a status, and each variant replying with nothing or a `String`.
fn check_statuses(data: &Data, ast: &inset_syntax::ast::Enum, errors: &mut Vec<Diagnostic>) {
    if data.variants.iter().all(|v| v.status.is_none()) {
        return;
    }
    for (variant, written) in data.variants.iter().zip(&ast.variants) {
        let Some(status) = &variant.status else {
            errors.push(
                Diagnostic::new(
                    format!("the variant `{}` has no `#[status]`", variant.name.name),
                    variant.name.span,
                    format!("other variants of `{}` have one", data.name.name),
                )
                .with_help(format!(
                    "an error enum names the status each variant replies with; write one above `{}`, such as `#[status(BadRequest)]`",
                    variant.name.name
                )),
            );
            continue;
        };
        if status::code(&status.name).is_none() {
            let help = match status::by_other_case(&status.name) {
                Some(known) => format!("the status is written `{known}`"),
                None => "name a status by its standard name, such as `NotFound` or `Conflict`"
                    .to_string(),
            };
            errors.push(
                Diagnostic::new(
                    format!("unknown status `{}`", status.name),
                    status.span,
                    "not an HTTP status",
                )
                .with_help(help),
            );
        }
        match variant.fields.as_slice() {
            [] | [Ty::String | Ty::Unknown] => {}
            _ => {
                let span = written
                    .fields
                    .first()
                    .map_or(written.name.span, |f| f.span)
                    .to(written.fields.last().map_or(written.name.span, |f| f.span));
                let found: Vec<String> = variant.fields.iter().map(Ty::to_string).collect();
                errors.push(
                    Diagnostic::new(
                        format!(
                            "the variant `{}` replies with a `{}`",
                            variant.name.name,
                            found.join(", ")
                        ),
                        span,
                        "an error variant's reply is its body, which is text",
                    )
                    .with_help(
                        "hold nothing, for an empty body, or one `String`, which is the body",
                    ),
                );
            }
        }
    }
}
