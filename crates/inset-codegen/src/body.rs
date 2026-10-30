//! Function bodies, types and patterns. Inside a function, Inset is Rust, so
//! these are emitted as they were written. The syntax tree keeps every
//! parenthesis, so the tokens nest exactly as the source does.

use inset_syntax::ast::{
    BinaryOp, Block, Closure, Doc, Expr, ExprKind, Lit, LitKind, Path, PathSegment, Pattern,
    PatternKind, Stmt, Type, TypeKind, UnaryOp,
};
use proc_macro2::{Literal, TokenStream};
use quote::quote;

use crate::Emitter;

/// The types Inset's standard library adds, which the runtime provides.
const RUNTIME_TYPES: [&str; 4] = ["HttpServer", "Table", "Handle", "Outlet"];

/// Words Rust reserves that Inset does not, which are written as raw identifiers.
const RESERVED: [&str; 14] = [
    "abstract", "become", "box", "do", "final", "gen", "macro", "override", "priv", "try",
    "typeof", "unsized", "virtual", "yield",
];

pub(crate) fn ident(name: &str) -> proc_macro2::Ident {
    let span = proc_macro2::Span::call_site();
    if RESERVED.contains(&name) {
        proc_macro2::Ident::new_raw(name, span)
    } else {
        proc_macro2::Ident::new(name, span)
    }
}

/// Doc comments, as `#[doc]` attributes, which print as `///` again.
pub(crate) fn docs(docs: &[Doc]) -> TokenStream {
    let lines = docs.iter().map(|d| {
        let text = if d.text.is_empty() {
            String::new()
        } else {
            format!(" {}", d.text)
        };
        quote!(#[doc = #text])
    });
    quote!(#(#lines)*)
}

pub(crate) fn ty(t: &Type) -> TokenStream {
    match &t.kind {
        TypeKind::Path(p) => {
            let path = path(p, false);
            match p.segments.as_slice() {
                [only] if RUNTIME_TYPES.contains(&only.ident.name.as_str()) => {
                    quote!(::inset_runtime::#path)
                }
                _ => path,
            }
        }
        TypeKind::Ref { mutable, inner } => {
            let inner = ty(inner);
            if *mutable {
                quote!(&mut #inner)
            } else {
                quote!(&#inner)
            }
        }
        TypeKind::Tuple(elems) => {
            let elems: Vec<_> = elems.iter().map(ty).collect();
            tuple(&elems)
        }
        TypeKind::Slice(inner) => {
            let inner = ty(inner);
            quote!([#inner])
        }
        TypeKind::Infer => quote!(_),
    }
}

/// A path in a type, or, with `expr`, in an expression, where generic
/// arguments take `::<`.
fn path(p: &Path, expr: bool) -> TokenStream {
    let segments = p.segments.iter().map(|s| segment(s, expr));
    quote!(#(#segments)::*)
}

fn segment(s: &PathSegment, expr: bool) -> TokenStream {
    let name = ident(&s.ident.name);
    if s.generics.is_empty() {
        return quote!(#name);
    }
    let generics = s.generics.iter().map(ty);
    if expr {
        quote!(#name::<#(#generics),*>)
    } else {
        quote!(#name<#(#generics),*>)
    }
}

/// `(a, b)`, and `(a,)` for one element.
fn tuple(elems: &[TokenStream]) -> TokenStream {
    if let [one] = elems {
        quote!((#one,))
    } else {
        quote!((#(#elems),*))
    }
}

pub(crate) fn pattern(p: &Pattern) -> TokenStream {
    match &p.kind {
        PatternKind::Wild => quote!(_),
        PatternKind::Binding { mutable, name } => {
            let name = ident(&name.name);
            if *mutable {
                quote!(mut #name)
            } else {
                quote!(#name)
            }
        }
        PatternKind::Tuple(elems) => {
            let elems: Vec<_> = elems.iter().map(pattern).collect();
            tuple(&elems)
        }
        PatternKind::TupleStruct { path: p, elems } => {
            let p = path(p, true);
            let elems = elems.iter().map(pattern);
            quote!(#p(#(#elems),*))
        }
        PatternKind::Path(p) => path(p, true),
        PatternKind::Lit(l) => lit(l),
    }
}

fn lit(l: &Lit) -> TokenStream {
    match &l.kind {
        LitKind::Bool(true) => quote!(true),
        LitKind::Bool(false) => quote!(false),
        LitKind::Int(text) | LitKind::Float(text) => {
            let literal: Literal = text.parse().expect("the lexer accepted the number");
            quote!(#literal)
        }
        LitKind::Str(value) => {
            let literal = Literal::string(value);
            quote!(#literal)
        }
        LitKind::Char(c) => {
            let literal = Literal::character(*c);
            quote!(#literal)
        }
    }
}

impl Emitter {
    pub(crate) fn block(&mut self, block: &Block) -> TokenStream {
        let stmts: Vec<_> = block.stmts.iter().map(|s| self.stmt(s)).collect();
        quote!({ #(#stmts)* })
    }

    fn stmt(&mut self, stmt: &Stmt) -> TokenStream {
        match stmt {
            Stmt::Let {
                pattern: p,
                ty: t,
                init,
                else_block,
                ..
            } => {
                let p = pattern(p);
                let t = t.as_ref().map(|t| {
                    let t = ty(t);
                    quote!(: #t)
                });
                let init = init.as_ref().map(|e| {
                    let e = self.expr(e);
                    quote!(= #e)
                });
                let else_block = else_block.as_ref().map(|b| {
                    let b = self.block(b);
                    quote!(else #b)
                });
                quote!(let #p #t #init #else_block;)
            }
            Stmt::Expr { expr, semi } => {
                let expr = self.expr(expr);
                if *semi { quote!(#expr;) } else { expr }
            }
        }
    }

    fn exprs(&mut self, exprs: &[Expr]) -> Vec<TokenStream> {
        exprs.iter().map(|e| self.expr(e)).collect()
    }

    pub(crate) fn expr(&mut self, e: &Expr) -> TokenStream {
        match &e.kind {
            ExprKind::Lit(l) => lit(l),
            ExprKind::Path(p) => path(p, true),
            ExprKind::Macro { name, args } => {
                let args = self.exprs(args);
                let is_vec = matches!(name.segments.as_slice(), [only] if only.ident.name == "vec");
                let name = path(name, true);
                if is_vec {
                    quote!(#name![#(#args),*])
                } else {
                    quote!(#name!(#(#args),*))
                }
            }
            ExprKind::Call { callee, args } => {
                let callee = self.expr(callee);
                let args = self.exprs(args);
                quote!(#callee(#(#args),*))
            }
            ExprKind::MethodCall {
                receiver,
                method,
                args,
            } => {
                let receiver = self.expr(receiver);
                let method = segment(method, true);
                let args = self.exprs(args);
                quote!(#receiver.#method(#(#args),*))
            }
            ExprKind::Field { base, name } => {
                let base = self.expr(base);
                match name.name.parse::<usize>() {
                    Ok(index) => {
                        let index = Literal::usize_unsuffixed(index);
                        quote!(#base.#index)
                    }
                    Err(_) => {
                        let name = ident(&name.name);
                        quote!(#base.#name)
                    }
                }
            }
            ExprKind::Index { base, index } => {
                let base = self.expr(base);
                let index = self.expr(index);
                quote!(#base[#index])
            }
            ExprKind::Await(base) => {
                self.unsupported("`.await`", e.span, "awaited here");
                let base = self.expr(base);
                quote!(#base.await)
            }
            ExprKind::Try(base) => {
                let base = self.expr(base);
                quote!(#base?)
            }
            ExprKind::Unary { op, operand } => {
                let operand = self.expr(operand);
                match op {
                    UnaryOp::Neg => quote!(-#operand),
                    UnaryOp::Not => quote!(!#operand),
                    UnaryOp::Deref => quote!(*#operand),
                }
            }
            ExprKind::Ref { mutable, operand } => {
                let operand = self.expr(operand);
                if *mutable {
                    quote!(&mut #operand)
                } else {
                    quote!(&#operand)
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let lhs = self.expr(lhs);
                let rhs = self.expr(rhs);
                let op = binary(*op);
                quote!(#lhs #op #rhs)
            }
            ExprKind::Assign { op, lhs, rhs } => {
                let lhs = self.expr(lhs);
                let rhs = self.expr(rhs);
                let op = op.map_or_else(|| quote!(=), compound);
                quote!(#lhs #op #rhs)
            }
            ExprKind::Cast { expr, ty: t } => {
                let expr = self.expr(expr);
                let t = ty(t);
                quote!(#expr as #t)
            }
            ExprKind::Struct {
                path: p,
                fields,
                rest,
            } => {
                let p = path(p, true);
                let fields: Vec<_> = fields
                    .iter()
                    .map(|f| {
                        let name = ident(&f.name.name);
                        match &f.value {
                            Some(value) => {
                                let value = self.expr(value);
                                quote!(#name: #value)
                            }
                            None => quote!(#name),
                        }
                    })
                    .collect();
                match rest {
                    Some(rest) => {
                        let rest = self.expr(rest);
                        quote!(#p { #(#fields,)* ..#rest })
                    }
                    None => quote!(#p { #(#fields),* }),
                }
            }
            ExprKind::Tuple(elems) => {
                let elems = self.exprs(elems);
                tuple(&elems)
            }
            ExprKind::Paren(inner) => {
                let inner = self.expr(inner);
                quote!((#inner))
            }
            ExprKind::Array(elems) => {
                let elems = self.exprs(elems);
                quote!([#(#elems),*])
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::If {
                cond,
                then_block,
                else_branch,
            } => {
                let cond = self.expr(cond);
                let then_block = self.block(then_block);
                let else_branch = else_branch.as_ref().map(|e| {
                    let e = self.expr(e);
                    quote!(else #e)
                });
                quote!(if #cond #then_block #else_branch)
            }
            ExprKind::Return(value) => {
                let value = value.as_ref().map(|v| self.expr(v));
                quote!(return #value)
            }
            ExprKind::Send(message) => {
                self.unsupported("`send`", e.span, "sent here");
                self.expr(message)
            }
            ExprKind::Move(_) => {
                self.unsupported("`move` across a boundary", e.span, "moved here");
                quote!(())
            }
            ExprKind::Closure(closure) => self.closure(closure),
        }
    }

    fn closure(&mut self, closure: &Closure) -> TokenStream {
        let is_move = closure.is_move.then(|| quote!(move));
        let params = closure.params.iter().map(|p| {
            let pat = pattern(&p.pattern);
            match &p.ty {
                Some(t) => {
                    let t = ty(t);
                    quote!(#pat: #t)
                }
                None => pat,
            }
        });
        let ret = closure.ret.as_ref().map(|t| {
            let t = ty(t);
            quote!(-> #t)
        });
        let body = self.expr(&closure.body);
        quote!(#is_move |#(#params),*| #ret #body)
    }
}

fn binary(op: BinaryOp) -> TokenStream {
    match op {
        BinaryOp::Add => quote!(+),
        BinaryOp::Sub => quote!(-),
        BinaryOp::Mul => quote!(*),
        BinaryOp::Div => quote!(/),
        BinaryOp::Rem => quote!(%),
        BinaryOp::And => quote!(&&),
        BinaryOp::Or => quote!(||),
        BinaryOp::BitAnd => quote!(&),
        BinaryOp::BitOr => quote!(|),
        BinaryOp::BitXor => quote!(^),
        BinaryOp::Shl => quote!(<<),
        BinaryOp::Shr => quote!(>>),
        BinaryOp::Eq => quote!(==),
        BinaryOp::Ne => quote!(!=),
        BinaryOp::Lt => quote!(<),
        BinaryOp::Le => quote!(<=),
        BinaryOp::Gt => quote!(>),
        BinaryOp::Ge => quote!(>=),
    }
}

/// `op=`. The parser makes compound assignments only of the operators that have one.
fn compound(op: BinaryOp) -> TokenStream {
    match op {
        BinaryOp::Add => quote!(+=),
        BinaryOp::Sub => quote!(-=),
        BinaryOp::Mul => quote!(*=),
        BinaryOp::Div => quote!(/=),
        BinaryOp::Rem => quote!(%=),
        BinaryOp::BitAnd => quote!(&=),
        BinaryOp::BitOr => quote!(|=),
        BinaryOp::BitXor => quote!(^=),
        BinaryOp::Shl => quote!(<<=),
        BinaryOp::Shr => quote!(>>=),
        _ => unreachable!("no compound assignment for {op:?}"),
    }
}
