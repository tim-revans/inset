//! Values and checks in requirement facts, typed against the program.
//!
//! A value takes the type the program expects in its place (syntax.md,
//! "Values"): `0.10` is a `Decimal` in a `Decimal` field, and `40` a `u32`
//! in a `u32` one. So a value is typed against the type it must have, and
//! turned into an [`inset_value::Value`] that the test runner can store in
//! a fake table or compare with what one holds. Names in a value resolve as
//! they would in `src/main.inset`.
//!
//! A check is a Rust condition on what the fake tables hold. It is typed
//! here too, and turned into an [`inset_value::Check`] for the runner to
//! evaluate. The subset of Rust a check may use is small: entries such as
//! `products["bolt-m6"]`, their fields, comparisons, `&&`, `||`, `!`,
//! arithmetic, `.len()` and `.is_empty()`, and values.

use inset_syntax::ast::{BinaryOp, Expr, ExprKind, LitKind, UnaryOp};
use inset_syntax::{Diagnostic, Ident, Span};
use inset_value::{Check, DataKind, DataShape, Decimal, Op, Shape, Types, Value};

use crate::Checker;
use crate::names::{Def, Ns};
use crate::types::Ty;
use crate::wiring::{int_fits, int_range};

/// The shape of a type, as the stored form and the test runner see it.
pub(crate) fn shape(ty: &Ty) -> Shape {
    match ty {
        Ty::Bool => Shape::Bool,
        Ty::Char => Shape::Char,
        Ty::String | Ty::Str => Shape::String,
        Ty::Int(name) => {
            let name = name.unwrap_or("i32");
            let bits = match name.trim_start_matches(['i', 'u']) {
                "size" => 64,
                n => n.parse().unwrap_or(64),
            };
            Shape::Int {
                signed: name.starts_with('i'),
                bits,
            }
        }
        Ty::Float(_) => Shape::Float,
        Ty::Decimal => Shape::Decimal,
        Ty::Tuple(elems) if elems.is_empty() => Shape::Unit,
        Ty::Tuple(elems) => Shape::Tuple(elems.iter().map(shape).collect()),
        Ty::Ref { inner, .. } | Ty::Slice(inner) => shape(inner),
        Ty::Std(name, args) => {
            let arg = |i: usize| Box::new(args.get(i).map_or(Shape::Unit, shape));
            match *name {
                "Option" => Shape::Option(arg(0)),
                "Result" => Shape::Result(arg(0), arg(1)),
                "Vec" | "VecDeque" => Shape::List(arg(0)),
                "HashSet" | "BTreeSet" => Shape::Set(arg(0)),
                "HashMap" | "BTreeMap" => Shape::Map(arg(0), arg(1)),
                // `Box`, `Rc` and the cells are stored as what they hold.
                _ => *arg(0),
            }
        }
        Ty::Data { id, .. } => Shape::Data(*id),
        _ => Shape::Unit,
    }
}

/// The shapes of the program's structs and enums, by their index.
pub(crate) fn types(cx: &Checker) -> Types {
    Types(
        cx.datas
            .iter()
            .map(|data| DataShape {
                name: data.name.name.clone(),
                json: data.json.clone(),
                kind: if data.is_enum {
                    DataKind::Enum(
                        data.variants
                            .iter()
                            .map(|v| (v.name.name.clone(), v.fields.iter().map(shape).collect()))
                            .collect(),
                    )
                } else {
                    DataKind::Struct(
                        data.fields
                            .iter()
                            .map(|f| (f.name.name.clone(), shape(&f.ty)))
                            .collect(),
                    )
                },
            })
            .collect(),
    )
}

/// Why a type cannot be a table's key, if it cannot.
pub(crate) fn not_a_key(ty: &Ty) -> Option<String> {
    match ty {
        Ty::Unknown | Ty::Bool | Ty::Char | Ty::String | Ty::Int(_) => None,
        Ty::Tuple(elems) if !elems.is_empty() => elems.iter().find_map(not_a_key),
        other => Some(other.to_string()),
    }
}

/// Types a requirement's values and checks, collecting what it finds
/// wrong, and the names it resolves for the language server.
pub(crate) struct Values<'c, 'a> {
    pub cx: &'c Checker<'a>,
    pub diagnostics: Vec<Diagnostic>,
    /// Each name resolved to the program's declaration of it.
    pub refs: Vec<(Span, Span)>,
    /// A property's inputs, by name, when typing its `where` condition,
    /// which reads them instead of a capability.
    pub inputs: Vec<(Ident, Ty)>,
}

impl Values<'_, '_> {
    fn error(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// Resolves a path as `src/main.inset` would. Names in a requirements
    /// file are not in the program's source, so what the resolver records
    /// for the language server is taken here, and kept with the file.
    fn resolve(&mut self, idents: &[Ident], ns: Ns) -> Result<Option<Def>, Diagnostic> {
        let before = self.cx.names.refs.borrow().len();
        let resolved = self.cx.names.resolve_in(0, idents, ns);
        let added: Vec<(Span, Span)> = self.cx.names.refs.borrow_mut().drain(before..).collect();
        self.refs.extend(added);
        resolved
    }

    /// Types `e` as a value of type `ty`. `role` names the place, as in
    /// "the key of `items`".
    pub(crate) fn value(&mut self, e: &Expr, ty: &Ty, role: &str) -> Option<Value> {
        let mismatch = |this: &mut Self| {
            this.error(
                Diagnostic::new(
                    format!("{role} is {}, and this is {}", a(ty), found(e)),
                    e.span,
                    format!("expected {}", a(ty)),
                )
                .with_help(help(ty)),
            );
            None
        };
        if let ExprKind::Paren(inner) = &e.kind {
            return self.value(inner, ty, role);
        }
        match ty {
            Ty::Unknown => None,
            Ty::Bool => match literal(e) {
                Some((LitKind::Bool(b), false)) => Some(Value::Bool(*b)),
                _ => mismatch(self),
            },
            Ty::Char => match literal(e) {
                Some((LitKind::Char(c), false)) => Some(Value::Char(*c)),
                _ => mismatch(self),
            },
            Ty::String => match literal(e) {
                Some((LitKind::Str(s), false)) => Some(Value::String(s.clone())),
                _ => mismatch(self),
            },
            Ty::Int(name) => match literal(e) {
                Some((LitKind::Int(text), negative)) => {
                    let name = name.unwrap_or("i32");
                    if let Err(diagnostic) = int_fits(text, negative, name, e.span) {
                        self.error(diagnostic);
                        return None;
                    }
                    let n = int_value(text)?;
                    let (min, max) = int_range(name);
                    let n = if negative { -n } else { n };
                    (min..=max).contains(&n).then_some(Value::Int(n))
                }
                _ => mismatch(self),
            },
            Ty::Float(_) => match literal(e) {
                Some((LitKind::Int(text) | LitKind::Float(text), negative)) => {
                    let x: f64 = text
                        .replace('_', "")
                        .trim_end_matches("f64")
                        .trim_end_matches("f32")
                        .parse()
                        .ok()?;
                    Some(Value::Float(if negative { -x } else { x }))
                }
                _ => mismatch(self),
            },
            Ty::Decimal => {
                let (text, negative, span) = match &e.kind {
                    ExprKind::Macro { name, args } if matches!(name.segments.as_slice(), [only] if only.ident.name == "dec") =>
                    {
                        let number = match args.as_slice() {
                            [arg] => match literal(arg) {
                                Some((LitKind::Int(t) | LitKind::Float(t), negative)) => {
                                    Some((t, negative, arg.span))
                                }
                                _ => None,
                            },
                            _ => None,
                        };
                        let Some(number) = number else {
                            self.error(
                                Diagnostic::new(
                                    "`dec!` takes one number",
                                    e.span,
                                    "not one number",
                                )
                                .with_help("write the number in it, such as `dec!(0.10)`"),
                            );
                            return None;
                        };
                        number
                    }
                    _ => match literal(e) {
                        Some((LitKind::Int(t) | LitKind::Float(t), negative)) => {
                            (t, negative, e.span)
                        }
                        _ => return mismatch(self),
                    },
                };
                match decimal(text) {
                    Ok(d) => Some(Value::Decimal(if negative { -d } else { d })),
                    Err(diagnostic) => {
                        self.error(diagnostic(span));
                        None
                    }
                }
            }
            Ty::Tuple(elems) => match &e.kind {
                ExprKind::Tuple(items) if items.len() == elems.len() => {
                    if elems.is_empty() {
                        return Some(Value::Unit);
                    }
                    let values: Vec<Option<Value>> = items
                        .iter()
                        .zip(elems)
                        .enumerate()
                        .map(|(i, (item, ty))| {
                            self.value(item, ty, &format!("member {i} of {role}"))
                        })
                        .collect();
                    Some(Value::Tuple(values.into_iter().collect::<Option<_>>()?))
                }
                _ => mismatch(self),
            },
            Ty::Std(name, args) => self.std_value(e, name, args, role, ty),
            Ty::Data { id, .. } => self.data_value(e, *id, role, ty),
            _ => mismatch(self),
        }
    }

    fn std_value(
        &mut self,
        e: &Expr,
        name: &str,
        args: &[Ty],
        role: &str,
        ty: &Ty,
    ) -> Option<Value> {
        fn callee(e: &Expr) -> Option<(String, &[Expr])> {
            match &e.kind {
                ExprKind::Call { callee, args } => match &callee.kind {
                    ExprKind::Path(p) if p.segments.len() == 1 => {
                        Some((p.segments[0].ident.name.clone(), args.as_slice()))
                    }
                    _ => None,
                },
                ExprKind::Path(p) if p.segments.len() == 1 => {
                    Some((p.segments[0].ident.name.clone(), &[]))
                }
                _ => None,
            }
        }
        let inner_role = |what: &str| format!("{what} in {role}");
        match (name, callee(e)) {
            ("Option", Some((n, []))) if n == "None" => Some(Value::Option(None)),
            ("Option", Some((n, [inner]))) if n == "Some" => {
                let inner = self.value(inner, &args[0], &inner_role("what `Some` holds"))?;
                Some(Value::Option(Some(Box::new(inner))))
            }
            ("Result", Some((n, [inner]))) if n == "Ok" || n == "Err" => {
                let ok = n == "Ok";
                let ty = &args[if ok { 0 } else { 1 }];
                let inner =
                    Box::new(self.value(inner, ty, &inner_role(&format!("what `{n}` holds")))?);
                Some(Value::Result(if ok { Ok(inner) } else { Err(inner) }))
            }
            ("Box", _) => self.value(e, &args[0], role),
            ("Vec" | "VecDeque", _) => {
                let items = match &e.kind {
                    ExprKind::Array(items) => items,
                    ExprKind::Macro { name, args } if matches!(name.segments.as_slice(), [only] if only.ident.name == "vec") => {
                        args
                    }
                    _ => {
                        self.error(
                            Diagnostic::new(
                                format!("{role} is {}, and this is {}", a(ty), found(e)),
                                e.span,
                                format!("expected {}", a(ty)),
                            )
                            .with_help("write a list as `vec![a, b]`"),
                        );
                        return None;
                    }
                };
                let values: Vec<Option<Value>> = items
                    .iter()
                    .map(|item| self.value(item, &args[0], &inner_role("an element")))
                    .collect();
                Some(Value::List(values.into_iter().collect::<Option<_>>()?))
            }
            ("Option" | "Result", _) => {
                self.error(
                    Diagnostic::new(
                        format!("{role} is {}, and this is {}", a(ty), found(e)),
                        e.span,
                        format!("expected {}", a(ty)),
                    )
                    .with_help(help(ty)),
                );
                None
            }
            _ => {
                self.error(Diagnostic::new(
                    format!("a `{ty}` in a requirement is not supported yet"),
                    e.span,
                    "the checker does not handle this yet",
                ));
                None
            }
        }
    }

    fn data_value(&mut self, e: &Expr, id: usize, role: &str, ty: &Ty) -> Option<Value> {
        let data = &self.cx.datas[id];
        let mismatch = |this: &mut Self| {
            this.error(
                Diagnostic::new(
                    format!("{role} is {}, and this is {}", a(ty), found(e)),
                    e.span,
                    format!("expected {}", a(ty)),
                )
                .with_help(help(ty)),
            );
            None
        };
        if !data.is_enum {
            let ExprKind::Struct { path, fields, rest } = &e.kind else {
                return mismatch(self);
            };
            let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
            match self.resolve(&idents, Ns::Type) {
                Ok(Some(Def::Data(found))) if found == id => {}
                Err(diagnostic) => {
                    self.error(diagnostic);
                    return None;
                }
                _ => return mismatch(self),
            }
            if let Some(rest) = rest {
                self.error(Diagnostic::new(
                    "a value in a requirement gives every field",
                    rest.span,
                    "a requirement has no value to take the rest from",
                ));
                return None;
            }
            let mut values = Vec::new();
            let mut ok = true;
            for declared in &data.fields {
                let given: Vec<_> = fields
                    .iter()
                    .filter(|f| f.name.name == declared.name.name)
                    .collect();
                match given.as_slice() {
                    [] => {
                        self.error(Diagnostic::new(
                            format!(
                                "`{}` has no field `{}` here",
                                data.name.name, declared.name.name
                            ),
                            path.span,
                            format!("give `{}` a value", declared.name.name),
                        ));
                        ok = false;
                    }
                    [field, repeated @ ..] => {
                        if let Some(repeated) = repeated.first() {
                            self.error(Diagnostic::new(
                                format!("the field `{}` is given twice", declared.name.name),
                                repeated.name.span,
                                "this repeats an earlier field",
                            ));
                            ok = false;
                        }
                        self.refs.push((field.name.span, declared.name.span));
                        let Some(value) = &field.value else {
                            self.error(
                                Diagnostic::new(
                                    format!("the field `{}` has no value", field.name.name),
                                    field.name.span,
                                    "a requirement has no variables to take it from",
                                )
                                .with_help(format!("write `{}: value`", field.name.name)),
                            );
                            ok = false;
                            continue;
                        };
                        let role = format!("the field `{}`", declared.name.name);
                        match self.value(value, &declared.ty, &role) {
                            Some(v) => values.push((declared.name.name.clone(), v)),
                            None => ok = false,
                        }
                    }
                }
            }
            for field in fields {
                if data.field(&field.name.name).is_none() {
                    self.error(
                        Diagnostic::new(
                            format!("`{}` has no field `{}`", data.name.name, field.name.name),
                            field.name.span,
                            "not a field of this struct",
                        )
                        .with_help(format!(
                            "the fields of `{}` are {}",
                            data.name.name,
                            crate::names(data.fields.iter().map(|f| f.name.name.as_str()))
                        )),
                    );
                    ok = false;
                }
            }
            return ok.then(|| Value::Struct {
                name: data.name.name.clone(),
                fields: values,
            });
        }
        let (path, args) = match &e.kind {
            ExprKind::Path(path) => (path, &[][..]),
            ExprKind::Call { callee, args } => match &callee.kind {
                ExprKind::Path(path) => (path, args.as_slice()),
                _ => return mismatch(self),
            },
            _ => return mismatch(self),
        };
        let idents: Vec<Ident> = path.segments.iter().map(|s| s.ident.clone()).collect();
        let variant = match self.resolve(&idents, Ns::Value) {
            Ok(Some(Def::Variant(found, variant))) if found == id => variant,
            Err(diagnostic) => {
                self.error(diagnostic);
                return None;
            }
            _ => return mismatch(self),
        };
        let declared = &data.variants[variant];
        if args.len() != declared.fields.len() {
            self.error(Diagnostic::new(
                format!(
                    "`{}::{}` holds {}, but {} {} given",
                    data.name.name,
                    declared.name.name,
                    declared.fields.len(),
                    args.len(),
                    if args.len() == 1 { "is" } else { "are" }
                ),
                e.span,
                format!("expected {}", declared.fields.len()),
            ));
            return None;
        }
        let values: Vec<Option<Value>> = args
            .iter()
            .zip(&declared.fields)
            .enumerate()
            .map(|(i, (arg, ty))| {
                self.value(arg, ty, &format!("field {i} of `{}`", declared.name.name))
            })
            .collect();
        Some(Value::Variant {
            name: declared.name.name.clone(),
            fields: values.into_iter().collect::<Option<_>>()?,
        })
    }

    /// The key and value types of the table granted as `grant`.
    pub(crate) fn table(&self, grant: &str) -> Option<(&Ty, &Ty)> {
        let app = self.cx.app?;
        let index = app.grants.iter().position(|g| g.name.name == grant)?;
        match self.cx.grant_tys.get(index)? {
            Ty::Table(k, v) => Some((k, v)),
            _ => None,
        }
    }

    /// Types a check, which must be a `bool`.
    pub(crate) fn check(&mut self, e: &Expr) -> Option<Check> {
        if !self.inputs.is_empty() {
            if !self.reads(e) {
                self.error(
                    Diagnostic::new(
                        "this condition reads no input",
                        e.span,
                        "a `where` condition limits the inputs `for all` generates",
                    )
                    .with_help("compare an input with something, such as `net >= dec!(0)`"),
                );
                return None;
            }
        } else if !reads_a_table(e) {
            self.error(
                Diagnostic::new(
                    "this check reads no capability",
                    e.span,
                    "a check is a condition on what a capability holds",
                )
                .with_help("read an entry by its key, such as `items[\"bolt-m6\"].on_hand > 0`"),
            );
            return None;
        }
        let (check, ty) = self.typed(e)?;
        if !matches!(ty, Ty::Bool | Ty::Unknown) {
            self.error(
                Diagnostic::new(
                    format!("a check is a `bool`, and this is {}", a(&ty)),
                    e.span,
                    "this is not a condition",
                )
                .with_help("compare it with something, such as `== 0`"),
            );
            return None;
        }
        Some(check)
    }

    /// Types a part of a check that reads a table.
    fn typed(&mut self, e: &Expr) -> Option<(Check, Ty)> {
        match &e.kind {
            ExprKind::Paren(inner) => self.typed(inner),
            ExprKind::Path(path) if !self.inputs.is_empty() => {
                let input = match path.segments.as_slice() {
                    [only] => self
                        .inputs
                        .iter()
                        .position(|(name, _)| name.name == only.ident.name),
                    _ => None,
                };
                let Some(index) = input else {
                    self.unsupported(e.span);
                    return None;
                };
                Some((Check::Input(index), self.inputs[index].1.clone()))
            }
            ExprKind::Index { .. } if !self.inputs.is_empty() => {
                self.error(
                    Diagnostic::new(
                        "a `where` condition cannot read a capability",
                        e.span,
                        "this reads a table",
                    )
                    .with_help("a `where` condition limits the inputs, before the steps run"),
                );
                None
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } if args.is_empty() && method.ident.name == "scale" => {
                let (base, ty) = self.typed(receiver)?;
                if !matches!(ty, Ty::Decimal | Ty::Unknown) {
                    self.error(Diagnostic::new(
                        format!("{} has no `scale`", a(&ty)),
                        method.ident.span,
                        "only a `Decimal` has a scale",
                    ));
                    return None;
                }
                Some((Check::Scale(Box::new(base)), Ty::Int(Some("u32"))))
            }
            ExprKind::Index { base, index } => {
                let grant = match &base.kind {
                    ExprKind::Path(p) if p.segments.len() == 1 => p.segments[0].ident.clone(),
                    _ => {
                        self.unsupported(e.span);
                        return None;
                    }
                };
                let (key_ty, value_ty) = self.entry_types(&grant)?;
                let (key_ty, value_ty) = (key_ty.clone(), value_ty.clone());
                let role = format!("the key of `{}`", grant.name);
                let key_value = self.value(index, &key_ty, &role)?;
                let key = inset_value::key::encode(&key_value, &shape(&key_ty)).ok()?;
                Some((
                    Check::Entry {
                        grant: grant.name.clone(),
                        key,
                        key_value,
                        shape: shape(&value_ty),
                    },
                    value_ty,
                ))
            }
            ExprKind::Field { base, name } => {
                let (base, ty) = self.typed(base)?;
                if let Ok(index) = name.name.parse::<usize>() {
                    return match ty {
                        Ty::Tuple(elems) if index < elems.len() => Some((
                            Check::TupleField(Box::new(base), index),
                            elems[index].clone(),
                        )),
                        _ => {
                            self.error(Diagnostic::new(
                                format!("{} has no field `{index}`", a(&ty)),
                                name.span,
                                "not a field",
                            ));
                            None
                        }
                    };
                }
                let field = match &ty {
                    Ty::Data { id, .. } => self.cx.datas[*id].field(&name.name),
                    _ => None,
                };
                let Some(field) = field else {
                    self.error(Diagnostic::new(
                        format!("{} has no field `{}`", a(&ty), name.name),
                        name.span,
                        "not a field",
                    ));
                    return None;
                };
                self.refs.push((name.span, field.name.span));
                Some((
                    Check::Field(Box::new(base), name.name.clone()),
                    field.ty.clone(),
                ))
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } if args.is_empty() && matches!(method.ident.name.as_str(), "len" | "is_empty") => {
                let (base, ty) = self.typed(receiver)?;
                let has_len = matches!(
                    &ty,
                    Ty::String
                        | Ty::Std(
                            "Vec" | "VecDeque" | "HashSet" | "BTreeSet" | "HashMap" | "BTreeMap",
                            _
                        )
                );
                if !has_len {
                    self.error(Diagnostic::new(
                        format!("{} has no `{}`", a(&ty), method.ident.name),
                        method.ident.span,
                        "only text and collections have a length",
                    ));
                    return None;
                }
                Some(if method.ident.name == "len" {
                    (Check::Len(Box::new(base)), Ty::Int(Some("usize")))
                } else {
                    (Check::IsEmpty(Box::new(base)), Ty::Bool)
                })
            }
            ExprKind::Unary { op, operand } => {
                let (inner, ty) = self.typed(operand)?;
                match (op, &ty) {
                    (UnaryOp::Not, Ty::Bool) => Some((Check::Not(Box::new(inner)), ty)),
                    (UnaryOp::Neg, Ty::Int(_) | Ty::Float(_) | Ty::Decimal) => {
                        Some((Check::Neg(Box::new(inner)), ty))
                    }
                    _ => {
                        self.error(Diagnostic::new(
                            format!("this operator does not apply to {}", a(&ty)),
                            e.span,
                            "not a number or a bool",
                        ));
                        None
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(e, *op, lhs, rhs),
            _ => {
                self.unsupported(e.span);
                None
            }
        }
    }

    fn binary(&mut self, e: &Expr, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> Option<(Check, Ty)> {
        let op = match op {
            BinaryOp::Eq => Op::Eq,
            BinaryOp::Ne => Op::Ne,
            BinaryOp::Lt => Op::Lt,
            BinaryOp::Le => Op::Le,
            BinaryOp::Gt => Op::Gt,
            BinaryOp::Ge => Op::Ge,
            BinaryOp::And => Op::And,
            BinaryOp::Or => Op::Or,
            BinaryOp::Add => Op::Add,
            BinaryOp::Sub => Op::Sub,
            BinaryOp::Mul => Op::Mul,
            BinaryOp::Div => Op::Div,
            BinaryOp::Rem => Op::Rem,
            _ => {
                self.unsupported(e.span);
                return None;
            }
        };
        if matches!(op, Op::And | Op::Or) {
            let lhs = self.check(lhs);
            let rhs = self.check(rhs);
            return Some((Check::Binary(op, Box::new(lhs?), Box::new(rhs?)), Ty::Bool));
        }
        // The side that reads a table gives the other side its type.
        let (l, r, ty) = if self.reads(lhs) {
            let (l, ty) = self.typed(lhs)?;
            let r = self.operand(rhs, &ty)?;
            (l, r, ty)
        } else {
            let (r, ty) = self.typed(rhs)?;
            let l = self.operand(lhs, &ty)?;
            (l, r, ty)
        };
        let result = match op {
            Op::Eq | Op::Ne => {
                if let Some(diagnostic) = self.not_comparable(&ty, e.span) {
                    self.error(diagnostic);
                    return None;
                }
                Ty::Bool
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let ordered = matches!(
                    ty,
                    Ty::Int(_)
                        | Ty::Float(_)
                        | Ty::Decimal
                        | Ty::String
                        | Ty::Char
                        | Ty::Bool
                        | Ty::Unknown
                );
                if !ordered {
                    self.error(Diagnostic::new(
                        format!("a check cannot order {} yet", a(&ty)),
                        e.span,
                        "only numbers, text, `char` and `bool` are ordered in a check",
                    ));
                    return None;
                }
                Ty::Bool
            }
            _ => {
                let arithmetic =
                    matches!(ty, Ty::Int(_) | Ty::Float(_) | Ty::Decimal | Ty::Unknown)
                        || (op == Op::Add && ty == Ty::String);
                if !arithmetic {
                    self.error(Diagnostic::new(
                        format!("`{}` does not apply to {}", inset_value::symbol(op), a(&ty)),
                        e.span,
                        "not a number",
                    ));
                    return None;
                }
                ty
            }
        };
        Some((Check::Binary(op, Box::new(l), Box::new(r)), result))
    }

    /// One side of an operator: what it reads, or a value of the other side's type.
    fn operand(&mut self, e: &Expr, ty: &Ty) -> Option<Check> {
        if self.reads(e) {
            let (check, found) = self.typed(e)?;
            if !crate::types::fits(ty, &found) {
                self.error(Diagnostic::new(
                    format!("the two sides are {} and {}", a(ty), a(&found)),
                    e.span,
                    format!("expected {}", a(ty)),
                ));
                return None;
            }
            return Some(check);
        }
        self.value(e, ty, "the other side").map(Check::Value)
    }

    /// The key and value types of the table a fact or check names,
    /// reporting a name that is not a table grant.
    pub(crate) fn entry_types(&mut self, grant: &Ident) -> Option<(&Ty, &Ty)> {
        let Some((key, _)) = self.table(&grant.name) else {
            let diagnostic = self.not_a_table(grant, "holds entries");
            self.error(diagnostic?);
            return None;
        };
        // A key type a table cannot have is reported at the grant.
        if not_a_key(key).is_some() {
            return None;
        }
        self.table(&grant.name)
    }

    /// Whether a grant is an `HttpClient`.
    pub(crate) fn is_client(&self, grant: &str) -> bool {
        let Some(app) = self.cx.app else {
            return false;
        };
        let index = app.grants.iter().position(|g| g.name.name == grant);
        index.and_then(|i| self.cx.grant_tys.get(i)) == Some(&Ty::HttpClient)
    }

    /// Reports a name that is not an `HttpClient` grant.
    pub(crate) fn not_a_client(&self, grant: &Ident) -> Option<Diagnostic> {
        let app = self.cx.app?;
        let index = app.grants.iter().position(|g| g.name.name == grant.name);
        Some(match index {
            None => self.not_a_table(grant, "holds entries")?,
            Some(index) => {
                let ty = self.cx.grant_tys.get(index).cloned().unwrap_or(Ty::Unknown);
                Diagnostic::new(
                    format!(
                        "`{}` is {}, and only an `HttpClient` replies",
                        grant.name,
                        a(&ty)
                    ),
                    grant.span,
                    "not a client",
                )
                .with_help("`replies` sets up how a fake `HttpClient` answers a call")
            }
        })
    }

    /// Reports a name that is not a table grant: an unknown grant, or one
    /// of another capability. `None` when there is no app to name.
    pub(crate) fn not_a_table(&self, grant: &Ident, what: &str) -> Option<Diagnostic> {
        let app = self.cx.app?;
        let index = app.grants.iter().position(|g| g.name.name == grant.name);
        Some(match index {
            None => {
                let grants: Vec<&str> = app.grants.iter().map(|g| g.name.name.as_str()).collect();
                Diagnostic::new(
                    format!("`{}` has no capability `{}`", app.name.name, grant.name),
                    grant.span,
                    "not a grant of the app",
                )
                .with_help(if grants.is_empty() {
                    format!("`{}` grants nothing", app.name.name)
                } else {
                    format!(
                        "the grants of `{}` are {}",
                        app.name.name,
                        crate::names(grants)
                    )
                })
            }
            Some(index) => {
                let ty = self.cx.grant_tys.get(index).cloned().unwrap_or(Ty::Unknown);
                Diagnostic::new(
                    format!("`{}` is {}, and only a `Table` {what}", grant.name, a(&ty)),
                    grant.span,
                    "not a table",
                )
                .with_help("an `HttpServer`'s requests and replies are the `when` and `then` steps")
            }
        })
    }

    /// Why values of a type cannot be compared in a step: a data type
    /// inside it that does not derive `PartialEq` and `Debug`, which a
    /// failed test needs to show both values.
    pub(crate) fn not_comparable(&self, ty: &Ty, span: Span) -> Option<Diagnostic> {
        match ty {
            Ty::Data { id, name, .. } => {
                let data = &self.cx.datas[*id];
                let missing: Vec<&str> = ["PartialEq", "Debug"]
                    .into_iter()
                    .filter(|t| !data.derives(t))
                    .collect();
                if missing.is_empty() {
                    return None;
                }
                Some(
                    Diagnostic::new(
                        format!(
                            "`{name}` is compared here, and does not derive {}",
                            crate::names(missing.iter().copied())
                        ),
                        span,
                        format!("compared as {}", a(ty)),
                    )
                    .with_help(format!(
                        "a type compared in a requirement derives `PartialEq` and `Debug`, so a \
                         failed test can show both values: add {} to the `#[derive]` of `{name}`",
                        crate::names(missing)
                    )),
                )
            }
            Ty::Tuple(elems) | Ty::Std(_, elems) => {
                elems.iter().find_map(|t| self.not_comparable(t, span))
            }
            _ => None,
        }
    }

    /// Whether `e` reads what a check is about: a table, or, in a `where`
    /// condition, an input.
    fn reads(&self, e: &Expr) -> bool {
        if self.inputs.is_empty() {
            return reads_a_table(e);
        }
        match &e.kind {
            ExprKind::Path(path) => matches!(path.segments.as_slice(), [only]
                if self.inputs.iter().any(|(name, _)| name.name == only.ident.name)),
            ExprKind::Index { .. } => true,
            ExprKind::Paren(inner) | ExprKind::Field { base: inner, .. } => self.reads(inner),
            ExprKind::MethodCall { receiver, .. } => self.reads(receiver),
            ExprKind::Unary { operand, .. } => self.reads(operand),
            ExprKind::Binary { lhs, rhs, .. } => self.reads(lhs) || self.reads(rhs),
            _ => false,
        }
    }

    fn unsupported(&mut self, span: Span) {
        if !self.inputs.is_empty() {
            self.error(
                Diagnostic::new(
                    "this is not supported in a `where` condition yet",
                    span,
                    "the checker does not handle this yet",
                )
                .with_help(
                    "a `where` condition compares the inputs, `.len()`, `.is_empty()` and a \
                     `Decimal`'s `.scale()` with values, using `==`, `<` and the like, `&&`, `||` \
                     and `!`",
                ),
            );
            return;
        }
        self.error(
            Diagnostic::new(
                "this is not supported in a check yet",
                span,
                "the checker does not handle this yet",
            )
            .with_help(
                "a check reads entries such as `items[\"bolt-m6\"]` and their fields, and \
                 compares them with `==`, `<` and the like, `&&`, `||` and `!`",
            ),
        );
    }
}

/// Whether an expression reads a table, through an entry such as `items["x"]`.
fn reads_a_table(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Index { .. } => true,
        ExprKind::Paren(inner) | ExprKind::Field { base: inner, .. } => reads_a_table(inner),
        ExprKind::MethodCall { receiver, .. } => reads_a_table(receiver),
        ExprKind::Unary { operand, .. } => reads_a_table(operand),
        ExprKind::Binary { lhs, rhs, .. } => reads_a_table(lhs) || reads_a_table(rhs),
        _ => false,
    }
}

/// A literal, and whether a `-` is before it.
fn literal(e: &Expr) -> Option<(&LitKind, bool)> {
    match &e.kind {
        ExprKind::Lit(lit) => Some((&lit.kind, false)),
        ExprKind::Paren(inner) => literal(inner),
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Lit(lit) if matches!(lit.kind, LitKind::Int(_) | LitKind::Float(_)) => {
                Some((&lit.kind, true))
            }
            _ => None,
        },
        _ => None,
    }
}

/// An integer literal's value, without its sign or suffix.
fn int_value(text: &str) -> Option<i128> {
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    let digits = crate::types::INTS
        .iter()
        .find_map(|s| digits.strip_suffix(s))
        .unwrap_or(&digits);
    let (radix, digits) = match digits.get(..2) {
        Some("0x") => (16, &digits[2..]),
        Some("0o") => (8, &digits[2..]),
        Some("0b") => (2, &digits[2..]),
        _ => (10, digits),
    };
    u128::from_str_radix(digits, radix)
        .ok()
        .and_then(|n| i128::try_from(n).ok())
}

/// A `Decimal` from a literal's text, keeping its scale: `0.10` has two
/// places. The error is a diagnostic for the literal's span.
pub(crate) fn decimal(text: &str) -> Result<Decimal, impl Fn(Span) -> Diagnostic> {
    let digits: String = text.chars().filter(|c| *c != '_').collect();
    let plain =
        digits.chars().all(|c| c.is_ascii_digit() || c == '.') && digits.matches('.').count() <= 1;
    let parsed = if plain {
        Decimal::from_str_exact(&digits).ok()
    } else {
        None
    };
    parsed.ok_or(move |span| {
        let (message, label) = if plain {
            (
                format!("`{text}` does not fit in a `Decimal`"),
                "a `Decimal` holds at most 28 digits",
            )
        } else {
            (
                format!("`{text}` is not a `Decimal`"),
                "a `Decimal` is written with digits and a point",
            )
        };
        Diagnostic::new(message, span, label)
            .with_help("write it as digits with a point, such as `0.10`")
    })
}

/// `a u32`, `an Item`: the type with its article, in backticks. The
/// article follows how the name is said: "an i32" and "an f64", but "a u32".
pub(crate) fn a(ty: &Ty) -> String {
    let text = ty.to_string();
    let vowel = text.starts_with(['a', 'e', 'i', 'o', 'A', 'E', 'I', 'O'])
        || (text.starts_with('f') && text[1..].starts_with(|c: char| c.is_ascii_digit()))
        || text.starts_with("Http");
    format!("{} `{text}`", if vowel { "an" } else { "a" })
}

/// What a value is, as written: "an integer", "text", "a `Place`".
fn found(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Lit(lit) => match lit.kind {
            LitKind::Int(_) => "an integer".into(),
            LitKind::Float(_) => "a number with a point".into(),
            LitKind::Str(_) => "text".into(),
            LitKind::Bool(_) => "a `bool`".into(),
            LitKind::Char(_) => "a `char`".into(),
        },
        ExprKind::Paren(inner) => found(inner),
        ExprKind::Unary { operand, .. } => found(operand),
        ExprKind::Struct { path, .. } => format!("a `{}`", path_text(path)),
        ExprKind::Path(path) => format!("`{}`", path_text(path)),
        ExprKind::Call { callee, .. } => match &callee.kind {
            ExprKind::Path(path) => format!("`{}(...)`", path_text(path)),
            _ => "a call".into(),
        },
        ExprKind::Tuple(items) if items.is_empty() => "`()`".into(),
        ExprKind::Tuple(_) => "a tuple".into(),
        ExprKind::Array(_) => "a list".into(),
        ExprKind::Macro { name, .. } => format!("`{}!(...)`", path_text(name)),
        _ => "an expression".into(),
    }
}

fn path_text(path: &inset_syntax::ast::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
}

/// How to write a value of a type, for a mismatch's help.
fn help(ty: &Ty) -> String {
    match ty {
        Ty::String => "write text in quotes, such as \"bolt-m6\"".into(),
        Ty::Int(_) => "write an integer, such as `40`".into(),
        Ty::Float(_) => "write a number, such as `1.5`".into(),
        Ty::Decimal => "write a number with a point, such as `0.10`".into(),
        Ty::Bool => "write `true` or `false`".into(),
        Ty::Char => "write a character in single quotes, such as 'a'".into(),
        Ty::Data { name, .. } => format!("write a value of `{name}` as Rust would"),
        Ty::Std("Option", _) => "write `Some(value)` or `None`".into(),
        Ty::Std("Result", _) => "write `Ok(value)` or `Err(error)`".into(),
        _ => format!("write a value of `{ty}` as Rust would"),
    }
}
