//! The syntax tree of both dialects. Every node records its span, so later
//! stages can point diagnostics and diagram elements at the source.

use crate::{Ident, Span};

// ---- Code dialect --------------------------------------------------------

/// One `.inset` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

/// A doc comment line, `/// text`.
#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub docs: Vec<Doc>,
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    App(App),
    Component(Component),
    /// An algorithm at the top level of a module.
    Function(Function),
    Struct(Struct),
    Enum(Enum),
    /// `mod name;`
    Mod(Ident),
    /// `use path;`
    Use(UseTree),
}

/// `struct Name { fields }`
#[derive(Clone, Debug, PartialEq)]
pub struct Struct {
    pub name: Ident,
    pub fields: Vec<StructField>,
}

/// `name: Type` in a struct, with its visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct StructField {
    pub docs: Vec<Doc>,
    /// Only `#[serde(...)]` is allowed here, which the checker sees to.
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
}

/// `enum Name { variants }`
#[derive(Clone, Debug, PartialEq)]
pub struct Enum {
    pub name: Ident,
    pub variants: Vec<Variant>,
}

/// `Name` or `Name(Type, ...)` in an enum.
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub docs: Vec<Doc>,
    pub attrs: Vec<Attribute>,
    pub name: Ident,
    /// The types a tuple variant holds, empty for a unit variant.
    pub fields: Vec<Type>,
    pub span: Span,
}

/// The tree of a `use` declaration: `a::b::{c, d as e}`.
#[derive(Clone, Debug, PartialEq)]
pub enum UseTree {
    /// `segment::rest`, where the segment may be `crate`, `super` or `self`.
    Path { segment: Ident, rest: Box<UseTree> },
    /// A name imported, optionally under another: `name as alias`.
    Name { name: Ident, alias: Option<Ident> },
    /// `{a, b}`
    Group(Vec<UseTree>, Span),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Visibility {
    Private,
    Pub(Span),
    PubCrate(Span),
    PubSuper(Span),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub kind: AttributeKind,
    pub span: Span,
}

/// The attributes Inset adds. Where each may go is checked later; the
/// parser reads any of them anywhere an attribute can be written.
#[derive(Clone, Debug, PartialEq)]
pub enum AttributeKind {
    Route(Route),
    Message,
    HandedDown,
    /// `#[derive(Trait, ...)]`
    Derive(Vec<Ident>),
    /// `#[status(Name)]` on an error enum's variant.
    Status(Ident),
    /// `#[serde(name = "value", ...)]` on a data type, field or variant.
    Serde(Vec<SerdeArg>),
    /// `#[defensive("reason")]`, which goes on a statement, a `match` arm
    /// or a `failure` arm rather than an item.
    Defensive(StrLit),
}

/// `#[defensive("reason")]`: code no requirement can reach by design, which
/// coverage shows as excused rather than uncovered.
#[derive(Clone, Debug, PartialEq)]
pub struct Defensive {
    pub reason: StrLit,
    pub span: Span,
}

/// `name = "value"`, or a bare `name`, in `#[serde(...)]`.
#[derive(Clone, Debug, PartialEq)]
pub struct SerdeArg {
    pub name: Ident,
    pub value: Option<StrLit>,
    pub span: Span,
}

/// `#[get("/hello/{name}", via = http)]`
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub method: HttpMethod,
    pub path: StrLit,
    pub via: Ident,
    pub body: Option<Ident>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    /// The method from its attribute name, such as `get`.
    pub fn from_attribute(name: &str) -> Option<HttpMethod> {
        HttpMethod::ALL
            .into_iter()
            .find(|m| m.attribute_name() == name)
    }

    /// The method as written in a requirement step, such as `GET`.
    pub fn from_request(name: &str) -> Option<HttpMethod> {
        HttpMethod::ALL.into_iter().find(|m| m.as_str() == name)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }

    pub fn attribute_name(self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
        }
    }

    const ALL: [HttpMethod; 5] = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Patch,
        HttpMethod::Delete,
    ];
}

/// A string literal: its value, with escapes resolved, and where it was
/// written, quotes included.
#[derive(Clone, Debug, PartialEq)]
pub struct StrLit {
    pub value: String,
    pub span: Span,
}

/// `app Name { settings, grants and starts }`
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub name: Ident,
    pub settings: Vec<Setting>,
    pub grants: Vec<Grant>,
    pub starts: Vec<Start>,
    /// `front end ...;`, in the order written. The checker allows one.
    pub front_ends: Vec<FrontEnd>,
}

/// `front end via server calls ["METHOD /path", ...];`: the browser front
/// end in `web/`, the `HttpServer` grant that serves it, and the routes it calls.
#[derive(Clone, Debug, PartialEq)]
pub struct FrontEnd {
    pub docs: Vec<Doc>,
    pub via: Ident,
    /// Each a route's method and its path, as its attribute writes it.
    pub calls: Vec<StrLit>,
    pub span: Span,
}

/// `setting name: Type = default;`
#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    pub docs: Vec<Doc>,
    pub name: Ident,
    pub ty: Type,
    pub default: Expr,
    pub span: Span,
}

/// `grant name: Capability { fields };`
#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub docs: Vec<Doc>,
    pub name: Ident,
    pub ty: Type,
    pub fields: Vec<FieldInit>,
    pub span: Span,
}

/// `start name: Component { handoffs };`, the name optional.
#[derive(Clone, Debug, PartialEq)]
pub struct Start {
    pub docs: Vec<Doc>,
    pub binding: Option<Ident>,
    pub component: Ident,
    pub handoffs: Vec<Handoff>,
    pub span: Span,
}

/// One field of a started component, and how its value crosses the boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct Handoff {
    pub field: Ident,
    pub value: HandoffValue,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HandoffValue {
    /// `move place`
    Move(Place),
    /// `&child`
    Lend(Ident),
    /// `send target.message`, connecting an outlet.
    Connect { target: Ident, message: Ident },
    /// Any other expression, which makes a new value or copies one.
    Expr(Expr),
}

/// A variable or field: `name` or `name.field.field`.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub parts: Vec<Ident>,
    pub span: Span,
}

/// `name: value` in a grant or a struct literal, or the shorthand `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` for the shorthand, where the value is the variable `name`.
    pub value: Option<Expr>,
    pub span: Span,
}

/// `component Name { fields, starts, failure block, functions }`
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub starts: Vec<Start>,
    pub failure: Option<Failure>,
    pub functions: Vec<Function>,
}

/// `failure { arms }`: how a component responds to failures.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub arms: Vec<FailureArm>,
    pub span: Span,
}

/// `/// reason` then `Condition => response,`
#[derive(Clone, Debug, PartialEq)]
pub struct FailureArm {
    /// The reason for the response, which the diagram shows.
    pub docs: Vec<Doc>,
    pub condition: FailureCondition,
    pub response: FailureResponse,
    pub defensive: Option<Defensive>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FailureCondition {
    pub kind: FailureConditionKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FailureConditionKind {
    Panic,
    Overflow,
    /// `Capability(field)`
    Capability(Ident),
    /// `Message(child)`
    Message(Ident),
    /// `Child(child)`
    Child(Ident),
    /// `_`, any other failure.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureResponse {
    Rollback,
    Restart,
}

/// `name: Type` in a component.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub docs: Vec<Doc>,
    pub attrs: Vec<Attribute>,
    pub name: Ident,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub docs: Vec<Doc>,
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub is_async: bool,
    pub name: Ident,
    pub receiver: Option<Receiver>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Block,
    pub span: Span,
}

/// `&self` or `&mut self`.
#[derive(Clone, Debug, PartialEq)]
pub struct Receiver {
    pub mutable: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub pattern: Pattern,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    /// `String`, `Vec<u8>`, `crate::Item`
    Path(Path),
    /// `&T` or `&mut T`
    Ref { mutable: bool, inner: Box<Type> },
    /// `(A, B)`, and `()` when empty.
    Tuple(Vec<Type>),
    /// `[T]`, behind a reference.
    Slice(Box<Type>),
    /// `_`, left for the compiler to work out.
    Infer,
}

/// `a::b::C`, each segment with any generic arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub segments: Vec<PathSegment>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathSegment {
    /// A name, or `self`, `Self`, `super` or `crate`.
    pub ident: Ident,
    pub generics: Vec<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `let pattern: Type = init else { ... };`
    Let {
        pattern: Pattern,
        ty: Option<Type>,
        init: Option<Expr>,
        else_block: Option<Block>,
        defensive: Option<Defensive>,
        span: Span,
    },
    /// An expression, with or without its `;`. A block's last statement
    /// without `;` is its value.
    Expr {
        expr: Expr,
        semi: bool,
        defensive: Option<Defensive>,
    },
}

impl Stmt {
    /// The `#[defensive]` written above the statement, if any.
    pub fn defensive(&self) -> Option<&Defensive> {
        match self {
            Stmt::Let { defensive, .. } | Stmt::Expr { defensive, .. } => defensive.as_ref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    /// `_`
    Wild,
    /// `name` or `mut name`
    Binding {
        mutable: bool,
        name: Ident,
    },
    /// `(a, b)`
    Tuple(Vec<Pattern>),
    /// `Some(x)`, `Json(product)`
    TupleStruct {
        path: Path,
        elems: Vec<Pattern>,
    },
    /// A path with no fields, such as `None` or `ShipError::UnknownItem`.
    Path(Path),
    /// `Item { on_hand, reorder_level: level, .. }`
    Struct {
        path: Path,
        fields: Vec<FieldPattern>,
        /// Whether it ends in `..`, ignoring the other fields.
        rest: bool,
    },
    /// `a | b`
    Or(Vec<Pattern>),
    Lit(Lit),
}

/// `name: pattern`, or the shorthand `name`, in a struct pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldPattern {
    pub name: Ident,
    /// `None` for the shorthand, which binds the field to `name`.
    pub pattern: Option<Pattern>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Lit(Lit),
    Path(Path),
    /// `name!(args)`. Arguments are read as expressions, which suits the
    /// standard macros Inset keeps.
    Macro {
        name: Path,
        args: Vec<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    MethodCall {
        receiver: Box<Expr>,
        method: PathSegment,
        args: Vec<Expr>,
    },
    /// `base.name`, or `base.0` on a tuple.
    Field {
        base: Box<Expr>,
        name: Ident,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `base.await`
    Await(Box<Expr>),
    /// `base?`
    Try(Box<Expr>),
    Unary {
        op: UnaryOp,
        operand: Box<Expr>,
    },
    /// `&x` or `&mut x`
    Ref {
        mutable: bool,
        operand: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `lhs = rhs`, or `lhs op= rhs` when `op` is given.
    Assign {
        op: Option<BinaryOp>,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    /// `Path { fields, ..rest }`
    Struct {
        path: Path,
        fields: Vec<FieldInit>,
        rest: Option<Box<Expr>>,
    },
    /// `(a, b)`, and `()` when empty.
    Tuple(Vec<Expr>),
    /// `(a)`, kept so the source text can be shown as written.
    Paren(Box<Expr>),
    Array(Vec<Expr>),
    Block(Block),
    If {
        cond: Box<Expr>,
        then_block: Block,
        else_branch: Option<Box<Expr>>,
    },
    Return(Option<Box<Expr>>),
    /// `send message`, which does not wait for the receiver.
    Send(Box<Expr>),
    /// `move place`, a value moved across a component boundary in a
    /// message's arguments.
    Move(Place),
    /// `|params| body`, or `move |params| body`.
    Closure(Closure),
    /// `match scrutinee { arms }`
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// `for pattern in iter { body }`
    For {
        pattern: Pattern,
        iter: Box<Expr>,
        body: Block,
    },
    /// `while cond { body }`, where `cond` may be a `let`.
    While {
        cond: Box<Expr>,
        body: Block,
    },
    /// `let pattern = expr` as the condition of an `if` or `while`.
    Let {
        pattern: Pattern,
        expr: Box<Expr>,
    },
    /// `start..end` or `start..=end`, either end optional.
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
}

/// `pattern if guard => body` in a `match`.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub defensive: Option<Defensive>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Closure {
    pub is_move: bool,
    pub params: Vec<ClosureParam>,
    pub ret: Option<Type>,
    pub body: Box<Expr>,
}

/// A closure's parameter, whose type may be left out.
#[derive(Clone, Debug, PartialEq)]
pub struct ClosureParam {
    pub pattern: Pattern,
    pub ty: Option<Type>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lit {
    pub kind: LitKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LitKind {
    Bool(bool),
    /// As written, with any suffix.
    Int(String),
    /// As written, with any suffix.
    Float(String),
    Str(String),
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    Deref,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

// ---- Requirements dialect ------------------------------------------------

/// One `.req.inset` file.
#[derive(Clone, Debug, PartialEq)]
pub struct RequirementsFile {
    pub requirements: Vec<Requirement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Requirement {
    /// The requirement as a sentence, which test reports show.
    pub sentence: StrLit,
    /// The steps, when there is no `examples` table. With one, the steps
    /// have placeholders, and each row has them read with its cells.
    pub steps: Vec<Step>,
    pub examples: Option<Examples>,
    pub span: Span,
}

impl Requirement {
    /// Each case the requirement tests: its steps, with the examples row
    /// they were read with, if any.
    pub fn cases(&self) -> Vec<(Option<&ExampleRow>, &[Step])> {
        match &self.examples {
            None => vec![(None, &self.steps)],
            Some(examples) => examples
                .rows
                .iter()
                .map(|row| (Some(row), row.steps.as_slice()))
                .collect(),
        }
    }
}

/// An `examples` table: each heading names a placeholder, and each row is
/// one case of the requirement.
#[derive(Clone, Debug, PartialEq)]
pub struct Examples {
    /// The `examples` keyword.
    pub span: Span,
    pub headings: Vec<Text>,
    pub rows: Vec<ExampleRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExampleRow {
    /// Each cell's text, trimmed, in the order of the headings.
    pub cells: Vec<Text>,
    /// The row, from its first `|` to its last.
    pub span: Span,
    /// The requirement's steps, read with this row's cells in place of the
    /// placeholders. A span in them that came from a cell is the span of
    /// its placeholder in the step.
    pub steps: Vec<Step>,
    /// Where the placeholders are in the steps.
    pub placeholders: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The word the step was written with. An `and` step has the kind of the
    /// step before it.
    pub keyword: StepKeyword,
    pub kind: StepKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKeyword {
    Given,
    When,
    Then,
    And,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StepKind {
    /// `when App receives METHOD /path [via server] [with body]`
    Request {
        app: Ident,
        method: HttpMethod,
        path: Text,
        /// The `HttpServer` grant the request arrives through.
        via: Option<Ident>,
        body: Option<Body>,
    },
    /// `then App replies Status [with body]`
    Reply {
        app: Ident,
        status: Ident,
        body: Option<Body>,
    },
    /// `given fact`, which sets up a fake capability, or `then fact`, which
    /// checks what it holds.
    Fact { given: bool, fact: Fact },
    /// `then condition`: a Rust condition on what fake capabilities hold.
    Check(Expr),
}

/// What a fake capability holds, named by its grant.
#[derive(Clone, Debug, PartialEq)]
pub struct Fact {
    pub grant: Ident,
    pub kind: FactKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FactKind {
    /// `name[key] is value`
    Entry { key: Box<Expr>, value: Box<Expr> },
    /// `name is empty`
    Empty,
    /// `name fails`
    Fails,
}

/// What follows `with` in a step.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// A string or a text block, sent or compared exactly as written.
    Text(Text),
    /// A value, sent or compared as JSON of the route's `Json<T>` type.
    Value(Box<Expr>),
}

impl Body {
    pub fn span(&self) -> Span {
        match self {
            Body::Text(text) => text.span,
            Body::Value(expr) => expr.span,
        }
    }
}

/// Text in a step: a path, a string literal's value, or a text block's lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub value: String,
    pub span: Span,
}
