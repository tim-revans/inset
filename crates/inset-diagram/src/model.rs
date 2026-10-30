//! The diagram data model. Its doc comments become the descriptions in the
//! JSON Schema, so they are written for whoever reads the JSON.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An element's ID, derived from where the element is in the source. See
/// `docs/language/diagrams.md` for the rules.
pub type Id = String;

/// Everything the source says about a program's structure. A view draws from
/// this and chooses what to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Diagram {
    /// The version of this format, which is 1.
    pub version: u32,
    /// The app: the program's outer boundary.
    pub app: App,
    /// Algorithms declared at the top level of a module, rather than in a component.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub algorithms: Vec<Function>,
    /// The edges between elements: capability use, messages and emits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<Edge>,
    /// Which requirements reached each element, when coverage was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

/// The `app` block: its settings, its grants and the components it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct App {
    /// The app's name, which is also the first segment of every other ID.
    pub id: Id,
    /// The app's name, as declared.
    pub name: String,
    /// The app's doc comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// The `setting` statements, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Setting>,
    /// The `grant` statements, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<Grant>,
    /// The browser front end, drawn as one opaque box. Draft: no example has one yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_end: Option<FrontEnd>,
    /// The components the app starts, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
}

/// A `setting`: a grant value that whoever runs the program may choose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// `<app>/setting:<name>`.
    pub id: Id,
    pub name: String,
    /// The setting's type, as written in the source.
    #[serde(rename = "type")]
    pub ty: String,
    /// The default value, as written in the source, such as `8080` or `"catalogue.db"`.
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A `grant`: a capability the host gives the program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    /// `<app>/grant:<name>`.
    pub id: Id,
    pub name: String,
    /// The capability type, as written in the source, such as `Table<String, Item>`.
    #[serde(rename = "type")]
    pub ty: String,
    /// The fields that configure the capability, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<Config>,
    /// What is outside the program at the other end of this capability.
    pub outside: Outside,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// One field of a grant, and where its value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum Config {
    /// A value fixed in the source.
    Fixed {
        name: String,
        /// The literal, as written in the source, such as `"stock.db"`.
        value: String,
    },
    /// The value of a setting.
    Setting {
        name: String,
        /// The setting's ID.
        setting: Id,
    },
}

/// The far end of a capability: who or what the program reaches through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Outside {
    pub kind: OutsideKind,
    /// What to call it, such as `HTTP clients` or `stock.db`.
    pub label: String,
    /// When set, `label` is this setting's default, and whoever runs the program may choose another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setting: Option<Id>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutsideKind {
    /// Others call in, as HTTP clients do. Drawn on the left of the app.
    Clients,
    /// The program calls out, as to a database. Drawn on the right of the app.
    Resource,
}

/// The browser front end. Draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrontEnd {
    /// `<app>/front_end`.
    pub id: Id,
    /// The routes the front end calls, by handler ID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<Id>,
}

/// A running component: one `start` statement and the component it starts.
/// A component type started twice appears twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Component {
    /// The parent's ID, then `/` and the name the `start` binds, or the
    /// component type's name when the `start` binds none.
    pub id: Id,
    /// The name the `start` statement binds, as in `start inventory: Inventory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The component type's name.
    #[serde(rename = "type")]
    pub ty: String,
    /// The component type's Rust path, such as `crate::warehouse::Inventory`.
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Set for an `extern component`, whose code the diagram does not show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<External>,
    /// The handoffs of the `start` statement, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub start: Vec<Handoff>,
    /// The component's fields, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    /// The arms of the `failure` block, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure: Vec<FailureArm>,
    /// Route handlers, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// Message handlers, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Function>,
    /// Algorithms declared in the component, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub algorithms: Vec<Function>,
    /// The components this one starts, in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
}

/// Where an external component's code comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct External {
    /// The language it is written in. Only `rust` exists so far.
    pub language: String,
    /// The file that implements it, relative to the file that declares it.
    pub path: String,
    /// The crates it may use, as `name@version`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crates: Vec<String>,
}

/// One handoff of a `start` statement: how a field of the started component gets its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "pass", rename_all = "snake_case", deny_unknown_fields)]
pub enum Handoff {
    /// `field: move place`. The value or capability moves into the child.
    Move { field: String, from: Id },
    /// `field: &child`. A sibling started earlier is lent to the child.
    Lend { field: String, from: Id },
    /// `field: place`, where the place's type is `Copy`.
    Copy { field: String, from: Id },
    /// `field: expression`, a new value.
    New {
        field: String,
        /// The expression, as written in the source.
        text: String,
    },
    /// `field: send target.message`. The child's outlet is connected to a message.
    Connect {
        field: String,
        /// The message handler's ID.
        to: Id,
    },
}

/// A field of a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// `<component>/field:<name>`.
    pub id: Id,
    pub name: String,
    /// The field's type, as written in the source.
    #[serde(rename = "type")]
    pub ty: String,
    pub kind: FieldKind,
    /// Set when the field is marked `#[handed_down]`: it is only passed on to a child.
    #[serde(default, skip_serializing_if = "is_false")]
    pub handed_down: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// A capability, such as `HttpServer` or `Table<K, V>`.
    Capability,
    /// An `Outlet<T>`, drawn as a port on the component's box.
    Outlet,
    /// A `&Handle<C>` to a sibling lent at start.
    Handle,
    /// The component's own state.
    State,
}

/// One arm of a `failure` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FailureArm {
    /// `<component>/failure:<condition>`, where the condition is `panic`,
    /// `overflow`, `any`, or `capability:`, `message:` or `child:` followed by the name.
    pub id: Id,
    pub condition: Condition,
    pub response: Response,
    /// The arm's doc comment, which says why.
    pub reason: String,
    /// The reason given by `#[defensive("...")]`, when the arm is marked defensive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defensive: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    /// `Panic`.
    Panic,
    /// `Overflow`.
    Overflow,
    /// `Capability(field)`, by the field's ID.
    Capability { field: Id },
    /// `Message(name)`, by the child's or lent sibling's component ID.
    Message { component: Id },
    /// `Child(name)`, by the child's component ID.
    Child { component: Id },
    /// `_`.
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Rollback,
    Restart,
}

/// A route handler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub method: Method,
    /// The path template, as written in the attribute.
    pub path: String,
    /// The ID of the `HttpServer` field the requests arrive through.
    pub via: Id,
    /// The parameter the request body binds to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The function that handles the route.
    pub handler: Function,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

/// A function: a route handler, a message handler or an algorithm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Function {
    /// The owner's ID, then `/fn:` and the name. A top-level algorithm's owner
    /// is the app, followed by `/mod:<path>` unless it is in the root module.
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// How the function takes `self`, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Receiver>,
    /// Set for an `async fn`.
    #[serde(rename = "async", default, skip_serializing_if = "is_false")]
    pub is_async: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<Param>,
    /// The return type, as written in the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// The body as a flowchart. Absent for a message of an external component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<Step>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Receiver {
    /// `&self`.
    Ref,
    /// `&mut self`.
    Mut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// One step of a flowchart. A step's ID is its block's ID, then `/` and its
/// index in the block. A function's body is the block with the function's ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// A statement that runs and carries on.
    Action {
        id: Id,
        /// The statement, as written in the source, without its `;`.
        text: String,
        /// The edges and functions the statement uses, by ID.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
        /// The error path of a `?` in the statement.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<Exit>,
    },
    /// An `if`, a `match` or a `let ... else`: one branch is taken.
    Decision {
        id: Id,
        /// What decides, as written in the source.
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
        /// The branches, in source order. Their IDs end in `/b0`, `/b1` and so on.
        branches: Vec<Branch>,
    },
    /// A `for` loop, or a `fork for`.
    Loop {
        id: Id,
        /// The loop's head, as written in the source.
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
        /// Set for `fork for`, whose bodies run concurrently and are joined.
        #[serde(default, skip_serializing_if = "is_false")]
        concurrent: bool,
        /// Running the body. Its ID ends in `/body`.
        body: Branch,
        /// Not running the body at all. Its ID ends in `/skip`, and it has no steps.
        skip: Branch,
    },
    /// A `fork { a, b, ... }`. Draft.
    Fork {
        id: Id,
        /// One branch per expression. Their IDs end in `/b0`, `/b1` and so on.
        branches: Vec<Branch>,
    },
    /// The function's value: a `return`, or its final expression.
    Return {
        id: Id,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<Id>,
    },
}

/// A way through a decision, loop or fork. Coverage counts branches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    pub id: Id,
    /// What selects the branch, such as `yes`, a `match` pattern or `else`.
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The reason given by `#[defensive("...")]`, when the branch is marked defensive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defensive: Option<String>,
}

/// The error path of a `?`, which returns from the function. Coverage counts it as a branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exit {
    /// The step's ID, then `/err`.
    pub id: Id,
    /// What the function returns, such as `None`.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defensive: Option<String>,
}

/// An edge between two elements. Its ID is `<from>-><to>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edge {
    /// A component uses a capability it holds.
    Capability {
        id: Id,
        /// The capability field's ID.
        from: Id,
        /// The grant's ID.
        to: Id,
        /// The capability's methods the component calls, in name order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        operations: Vec<String>,
    },
    /// A function sends a message to another component.
    Message {
        id: Id,
        /// The sending function's ID.
        from: Id,
        /// The message handler's ID.
        to: Id,
        /// Set for `send`, which does not wait for a reply.
        #[serde(default, skip_serializing_if = "is_false")]
        send: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<Arg>,
    },
    /// A function emits a value through an outlet of its own component.
    Emit {
        id: Id,
        /// The emitting function's ID.
        from: Id,
        /// The outlet field's ID.
        to: Id,
        value: Arg,
    },
}

/// A value that crosses a component boundary, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Arg {
    pub pass: Pass,
    /// The place or expression, as written in the source, without `move` or `&`.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pass {
    /// `move x`.
    Move,
    /// `&x`.
    Lend,
    /// `x`, where `x` is `Copy`.
    Copy,
    /// A new value, built in place.
    New,
}

/// Which requirements reached which elements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// For each element ID reached, the sentences of the requirements that reached it.
    pub reached: BTreeMap<Id, Vec<String>>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Edge {
    pub fn id(&self) -> &str {
        match self {
            Edge::Capability { id, .. } | Edge::Message { id, .. } | Edge::Emit { id, .. } => id,
        }
    }

    pub fn from(&self) -> &str {
        match self {
            Edge::Capability { from, .. }
            | Edge::Message { from, .. }
            | Edge::Emit { from, .. } => from,
        }
    }

    pub fn to(&self) -> &str {
        match self {
            Edge::Capability { to, .. } | Edge::Message { to, .. } | Edge::Emit { to, .. } => to,
        }
    }
}

impl Handoff {
    pub fn field(&self) -> &str {
        match self {
            Handoff::Move { field, .. }
            | Handoff::Lend { field, .. }
            | Handoff::Copy { field, .. }
            | Handoff::New { field, .. }
            | Handoff::Connect { field, .. } => field,
        }
    }
}

impl Step {
    pub fn id(&self) -> &str {
        match self {
            Step::Action { id, .. }
            | Step::Decision { id, .. }
            | Step::Loop { id, .. }
            | Step::Fork { id, .. }
            | Step::Return { id, .. } => id,
        }
    }
}

impl Component {
    /// The ID segment this component adds to its parent's.
    pub fn segment(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.ty)
    }
}

impl Condition {
    /// The condition as written in a `failure` block, given the name of what it refers to.
    pub fn source(&self, name: &str) -> String {
        match self {
            Condition::Panic => "Panic".into(),
            Condition::Overflow => "Overflow".into(),
            Condition::Capability { .. } => format!("Capability({name})"),
            Condition::Message { .. } => format!("Message({name})"),
            Condition::Child { .. } => format!("Child({name})"),
            Condition::Any => "_".into(),
        }
    }

    /// The ID of what the condition refers to, if it refers to anything.
    pub fn target(&self) -> Option<&str> {
        match self {
            Condition::Capability { field } => Some(field),
            Condition::Message { component } | Condition::Child { component } => Some(component),
            Condition::Panic | Condition::Overflow | Condition::Any => None,
        }
    }
}

impl Function {
    /// The parameters and return type, as in `(&self, sku: &str) -> Option<Item>`.
    pub fn signature(&self) -> String {
        let mut params: Vec<String> = Vec::new();
        match self.receiver {
            Some(Receiver::Ref) => params.push("&self".into()),
            Some(Receiver::Mut) => params.push("&mut self".into()),
            None => {}
        }
        params.extend(self.params.iter().map(|p| format!("{}: {}", p.name, p.ty)));
        let returns = self
            .returns
            .as_ref()
            .map_or(String::new(), |ty| format!(" -> {ty}"));
        format!("({}){returns}", params.join(", "))
    }
}
