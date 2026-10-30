# Inset syntax

The living reference for Inset's syntax. It is a draft: the parser does not exist yet, and the syntax changes as each example program is written.

Each construct is marked with its status:

- **Used**: an example program uses it.
- **Draft**: planned, but no example uses it yet, so expect it to change.

How the generated diagrams look is in [`diagrams.md`](diagrams.md). The example programs are in [`examples/`](../../examples/): Hello HTTP, Stock tracker and CSV import.

## Principles

An AI writes Inset source, and a human reviews the diagrams and requirement tests generated from it. The syntax follows from both.

- **Lean on what models already know.** Code is shaped like Rust, which the compiler emits anyway and which language models have seen a great deal of. Requirement tests are shaped like Given/When/Then, which reads as English.
- **Familiar syntax keeps its familiar meaning.** Anything that looks like Rust means what it means in Rust, with no exceptions. Models that write Inset will bring their Rust knowledge with them, and should be right to. A construct that looked like Rust but behaved differently would turn that knowledge into bugs. Inset is a layer over Rust that relies on Rust for safety and abstracts away the implementation, so it never redefines a Rust keyword, attribute or rule. Where it needs something Rust lacks, it adds syntax that is not valid Rust.
- **New concepts look new.** Rust has no components, capabilities, apps or requirements, so these get their own keywords, which a model cannot mistake for Rust.
- **Data flow is written down.** Every value or capability that crosses a component boundary says whether it is moved, lent or copied, so the diagram can label each arrow truthfully.
- **Capabilities have one origin and one holder.** Capabilities enter the program only through the app's `grant` statements, reach components only by being moved down, and are held by one component at a time, so every external access appears as exactly one diagram edge.
- **Requirements see only the outside.** A requirement tests what the program does through its boundary: HTTP requests and replies, and what its granted capabilities hold. How the program is built is the diagram's job.
- **Structure is wired like hardware.** Hardware description languages are designed to be drawn, and Inset takes its structure from them. Components nest like modules. A component sends messages to the children it started, and announces events through outlets, like a module's output ports, whose connections its parent chooses. Every arrow is known from the source, and no two components can end up waiting on each other.

## Two dialects

| Dialect | Files | Shaped like | Written for |
| --- | --- | --- | --- |
| Code | `*.inset` | Rust: braces, semicolons, Rust's types and expressions | The AI that writes it |
| Requirements | `*.req.inset` | Given/When/Then: one step per line, indented under the requirement | The software engineer who reviews it |

A program is a directory of UTF-8 files of both kinds: code in `src/` and requirements in `requirements/` (see [Requirements and coverage](#requirements-and-coverage)). The code is organised in modules, exactly as in Rust (see [Modules](#modules)). The file name decides the dialect, so the two block styles never mix in one file. The Stock tracker splits its code across two files.

*Why two dialects:* each one is matched to its reader. Braces suit code, where a model writes them reliably and whitespace mistakes cannot change the meaning. Prose suits requirements, which a person must be able to read without knowing the code.

## Code dialect

### Lexical structure

The code dialect uses Rust's lexical rules: identifiers, integer and string literals, `//` line comments, `/* */` block comments, and `///` doc comments on the item below.

- Doc comments become the descriptions shown on diagram elements. They may go on items, fields, functions and enum variants.
- String literals do not interpolate, as in Rust. Build text with `format!`.
- Inset adds these keywords to Rust's: `app`, `component`, `setting`, `grant`, `start`, `send`, `fork`, and `failure` with its responses `rollback` and `restart`. It also gives Rust's `extern` a new use, in `extern component`.
- Inset adds these attributes: the route attributes `#[get]`, `#[post]`, `#[put]`, `#[patch]` and `#[delete]`, and `#[message]`, `#[handed_down]`, `#[status]`, `#[rust]` and `#[defensive]`. Attributes that change what an item does are familiar from Rust frameworks, so these keep Rust's meaning of an attribute.
- Inset adds two standard types alongside the capability types: `Handle<C>`, for a running component, and `Outlet<T>`, for announcing events (see [Outlets](#outlets)).

### What Inset keeps from Rust

Inside a function body, Inset is Rust, and it keeps what models use every day: control flow (`if`, `match`, `for`, `while`, `loop`, `let ... else`, `?`), closures and iterator adaptors, slices and `&str`, the standard collections (`Vec`, `HashMap`, `BTreeMap`, `HashSet`, `BTreeSet`), `impl` blocks on data types, generic functions and data types, lifetimes in algorithms and data types, and `#[derive]`. Data types may implement standard traits such as `Display`, `From`, `FromStr`, `Default` and `Ord`. The pure standard macros `vec!`, `format!`, `matches!`, `assert!` and `write!` are kept, along with `dec!` from `rust_decimal`.

Some of these carry a restriction, so that every effect stays visible:

- A closure cannot `await`, `send`, or call a capability. Every message arrow and capability edge is then a statement of its own, which the flowchart can show. A flowchart draws an iterator chain such as `map` as one "for each" step.
- A method on a data type is an algorithm. Data types cannot hold capabilities or component handles, so such methods are pure.
- Components and messages are not generic and have no lifetimes, so every diagram arrow has a concrete type.

### Rust features Inset leaves out

Inset leaves out only what would hide an effect, a data flow, or which code runs. The compiler rejects each with an error that names the Inset alternative. Stage 6 pins the wording.

| Rust feature | Inset alternative | Why Inset leaves it out |
| --- | --- | --- |
| `unsafe` | None | Inset relies on Rust for safety |
| `static`, global state | A component's fields | State shared outside a component is a data flow the diagram cannot draw |
| `std` I/O, networking, threads, processes, environment variables, the clock and random numbers | Capabilities, `fork` | Every effect goes through a capability, so it appears on the diagram and a test can fake it. The clock and random numbers are effects too, and faking them keeps requirement tests repeatable |
| `println!`, `eprintln!` and other printing | Capabilities | Output is an effect |
| `macro_rules!` and procedural macros | Functions | Code generated by a macro has no source the diagram can point to |
| Declaring traits, and `dyn Trait` | Enums, concrete types | Dynamic dispatch hides which code runs, so the flowchart could not say what a call does |
| `Rc`, `Arc`, `Cell`, `RefCell`, `Mutex`, `RwLock` and atomics in a value that crosses a boundary | Messages | Rust's thread-safety rules would allow an `Arc<Mutex<_>>` handed to two components, which is shared state the diagram cannot see. Inside one component they are allowed |

*Why so little is left out:* models write Inset with their Rust knowledge. Every feature Inset removes is one a model will reach for and be told off for, so a feature stays unless it would make the diagram untrue.

### App

**Status:** Used.

An app is the root of a program. It names the settings it can be run with and the capabilities the host must grant, and it starts the top-level components. A program has exactly one app.

```rust
app PriceImport {
    /// The port to serve on.
    setting port: u16 = 8080;
    /// The catalogue database, which other tools also read.
    setting catalogue: String = "catalogue.db";
    grant http: HttpServer { port };
    grant products: Table<String, Product> { file: catalogue, name: "products" };
    start Importer { http: move http, products: move products };
}
```

- `setting name: Type = default;` declares a value that whoever runs the program may choose, such as `inset run --set port=9090`. Without one, the default is used, and requirement tests always use the default.
- `grant name: Capability { fields };` asks the host for a capability and binds it to `name`. The fields configure it, such as the port to listen on. Each field is a literal, fixed in the source, or the name of a setting.
- `start Component { handoffs };` starts a component and hands it its fields (see [Starting components](#starting-components)).
- `grant` is the only way to create a capability. `setting` and `grant` may appear only in an app.
- Settings may be `bool`, an integer type, or `String`. Passing a setting to a component in `start` is Draft.
- A relative file path in a grant is resolved by the host. *(Draft: stage 4 settles against what.)*

*Why settings are declared:* some configuration is part of what the reviewer approves, and some belongs to whoever runs the program. The Stock tracker's database is its own, so its file is fixed in the grant. CSV import writes a catalogue that other tools read, so where it lives is a setting. Inset does not choose between the two. The source states which values can change at run time, and the diagram lists them with their defaults, so the reviewer can see exactly what the person running the program controls. A setting can only change a value in a grant. It cannot add a grant or change a capability's type.

*Why an app separate from components:* the security-relevant facts of a program, which is everything it can touch outside itself, sit in one short block that the diagram draws as the outer boundary. All three examples keep the app to a few lines of grants and starts. An app has no functions, so it cannot grow logic that hides among the grants.

### Component

**Status:** Used.

A component is an actor. It runs concurrently with other components, holds its own state, and talks to the outside only through messages and the capabilities it was handed.

```rust
/// Keeps the reorders waiting to be placed, at most one per item.
component Reorders {
    orders: Vec<Reorder>,

    /// Records a reorder, replacing any earlier one for the same item.
    #[message]
    fn raise(&mut self, order: Reorder) { ... }

    /// Every reorder waiting to be placed, oldest first.
    #[message]
    fn pending(&self) -> Vec<Reorder> {
        self.orders.clone()
    }
}
```

A component's body has four parts, in this order:

1. **Fields**, written like a Rust struct's fields. They hold the capabilities it was handed, its outlets, the handles of siblings lent to it, and its state. Whoever starts the component supplies every field.
2. **`start` statements**, which start its children (see [Starting components](#starting-components)).
3. **A `failure` block**, if it has one, which says how it responds to failures (see [Failures](#failures)).
4. **Functions.** Each one is exactly one of:
   - a **route handler**: a function with a route attribute (see [Routes](#routes));
   - a **message handler**: a function with `#[message]`, which other components send messages to (see [Messages](#messages-between-components));
   - an **algorithm**: any other function, which takes no `self` (see [Algorithms](#algorithms)).

- Handlers take `&self` or `&mut self` to read or change the component's fields, as Rust methods do. A handler that touches no field takes neither, like Hello HTTP's `hello`.
- A function without a route attribute or `#[message]` that takes `self` is an error. Code shared between handlers goes in an algorithm.
- `pub` on a component, and on its functions, means what it means in Rust: visible outside the module (see [Modules](#modules)).

*Why `#[message]` and not `pub`:* in Rust, `pub` only changes where a name is visible. Using it to mark a message would give a familiar keyword a new meaning, and a model that made a function `pub` for visibility would silently create a message. An attribute marks a handler the same way the route attributes do.
- A handler that waits for anything, such as a message reply or a capability call, is an `async fn`, as in Rust.
- A component handles one message or request at a time, in the order they arrive.

### Starting components

**Status:** Used.

A `start` statement starts a component and supplies each of its fields. It is the same in an app and in a component.

```rust
pub component Warehouse {
    http: HttpServer,
    #[handed_down]
    items: Table<String, Item>,

    start reorders: Reorders { orders: Vec::new() };
    start inventory: Inventory { items: move items, low_stock: send reorders.raise };
    ...
}
```

- `start name: Component { handoffs };` starts a child and binds it to `name`. Its functions reach the child as `self.name`, a `Handle<Component>` for sending it messages.
- `start Component { handoffs };` starts a child that nothing sends messages to, such as one that only serves routes.
- Each handoff supplies one field of the child, and says how its value crosses the boundary (see [Ownership and borrowing](#ownership-and-borrowing)).
- In a component, a handoff can name the component's own fields. A field that is moved into a child is marked `#[handed_down]`. It is supplied when the component starts, passed straight to the child, and is not kept, so the component's functions cannot use it. `Warehouse` holds `items` only to hand it to `Inventory`. The compiler requires the attribute on every field moved into a child, and only on those.

  *Why `#[handed_down]`:* in Rust, every declared field of a struct exists for as long as the struct does. A field that is moved away at start would break that rule silently, so the attribute says, where the field is declared, that it is only passed through.
- A handoff connects a child's outlet to a message with `send`: `low_stock: send reorders.raise` (see [Outlets](#outlets)).
- *(Draft: no example needs it yet.)* A handoff can lend a child started earlier in the same body to a later one: `reorders: &reorders`. The later child declares the field as a borrowed handle, `reorders: &Handle<Reorders>`, and can send it messages and await their replies. Like all borrows across a boundary, it is written without a lifetime: the compiler knows the parent outlives both children. The parent still owns the lent child.
- A child can only be lent to siblings started after it, never to itself or to its parent.
- A component handle cannot be moved, copied, stored in data or sent in a message. The only handles a component holds are its own children and the siblings lent to it at `start`. A component that only needs to tell another something is given an outlet instead.
- A component's children start before it handles anything, and stop after it stops.
- A component started by a parent is nested inside the parent on the diagram. The app's children are the top-level components.

*Why the parent wires its children:* who can message whom is then fixed in the source, so the diagram can draw every arrow. Hardware description languages such as Verilog and VHDL wire submodules the same way, and the WebAssembly component model, which Inset compiles to, composes component instances in the same order. Actor languages such as Erlang and Akka instead let a handle travel in any message. Their diagrams cannot be drawn from the code, and two calls waiting on each other can deadlock.

*Why lending only goes to later siblings:* a component that waits for a reply then waits only on its own children or on components started before it, which can never wait on it in turn. So the one-at-a-time rule cannot deadlock. The parent still owns the lent child and outlives the sibling it was lent to, so `&` keeps its Rust meaning.

*Why `start name: Component { ... }`:* it reads like `grant name: Capability { ... }`, the other statement that brings something into being, and like a Rust struct literal for the handoffs. `start` marks it as new: a child is a running actor, not a value.

### Capabilities

**Status:** Used.

A capability is an owned handle to something outside the program, such as a network listener or a stored table. Capabilities cannot be copied, cloned or lent. A component can use only the capabilities in its fields.

| Capability | Granted with | Gives the holder |
| --- | --- | --- |
| `HttpServer` | `HttpServer { port: u16 }` | Incoming HTTP requests on the port, answered by route handlers |
| `Table<K, V>` | `Table<K, V> { file: String, name: String }` | A stored map from keys of type `K` to values of type `V`, kept in the table `name` of the database file `file` |

A program cannot declare its own capability types. A capability is something the host grants and enforces, so only the host can provide one. The standard library declares those above.

**Calling a capability.** A capability's methods are async Rust methods, called with the usual Rust rules: `self.items.get(sku).await`. Their arguments are not component boundaries, so they need no boundary notation. The diagram shows one edge per capability, labelled with the operations used.

| `Table<K, V>` method | Does |
| --- | --- |
| `get(&self, key: &Q) -> Option<V>` | Reads the value stored under `key`. As with `HashMap::get`, `key` may be any borrowed form of `K`, such as a `&str` for a `String` key |
| `insert(&mut self, key: K, value: V)` | Stores `value` under `key`, replacing any value already there |
| `remove(&mut self, key: &K) -> Option<V>` | Removes and returns the value under `key` *(Draft)* |
| `entries(&self) -> Vec<(K, V)>` | Every key and value, in key order *(Draft)* |

`K` and `V` must be data types: they cannot hold capabilities or component handles.

*Why capabilities are never lent:* a lent capability would give two components the same database, and the diagram would show two edges to it with no way to say which writes what. With one holder, any component that needs the data asks the holder by message, so the holder is the single place to review all access to that system. In the Stock tracker, only `Inventory` touches `items`.

### Routes

**Status:** Used.

A route attribute turns a component function into an HTTP route handler. The attribute style follows Rust web frameworks such as actix-web and Rocket.

```rust
#[post("/items/{sku}/shipments", via = http, body = quantity)]
async fn ship(&self, sku: String, quantity: u32) -> Result<String, ShipError> {
    ...
}
```

- The attributes are `get`, `post`, `put`, `patch` and `delete`, one for each HTTP method.
- `via = field` names the `HttpServer` capability the requests arrive through. It is always required.
- A path segment in `{` `}` is a path parameter. It binds to the function parameter of the same name, and takes that parameter's type.
- `body = name` binds the request body to the function parameter `name`. The body is read as UTF-8 text and converted to the parameter's type as a path segment is, so a `u32` body is the digits of the number.
- The compiler rejects a path parameter or `body` with no matching function parameter, and a function parameter that nothing supplies.
- Each path segment is URL-decoded before it is converted to the parameter's type. A request whose path or parameter types match no route gets `404 Not Found` without reaching a handler. A body that cannot be converted gets `400 Bad Request`.
- The function's value is the reply, as with any Rust function: its final expression, or a `return`.

The return type decides the response:

| Return type | Response |
| --- | --- |
| `String` | `200 OK` with the string as a `text/plain; charset=utf-8` body |
| `Option<String>` | `Some` as for `String`. `None` replies `404 Not Found` with an empty body |
| `Result<String, E>` | `Ok` as for `String`. `Err` replies with the status named on the error's variant |

The error type `E` is an enum whose every variant has a `#[status(Name)]` attribute, using the status names of the requirements dialect. A unit variant replies with an empty body. A variant holding one `String` replies with that string as the body.

```rust
enum ShipError {
    #[status(NotFound)]
    UnknownItem,
    #[status(Conflict)]
    NotEnoughStock,
}
```

**JSON, query parameters and headers.** *(Draft: no example uses them yet. Stage 9 confirms them against the typed API contract.)* They take the forms of Rust web frameworks.

```rust
#[put("/products/{sku}?{dry_run}", via = http, body = product, header("If-Match") = version)]
async fn update(
    &self,
    sku: String,
    dry_run: Option<bool>,
    version: String,
    product: Json<Product>,
) -> Result<Json<Product>, UpdateError> {
    let Json(product) = product;
    ...
}
```

- `Json<T>` is axum's and actix-web's type of the same name, a tuple struct around the value. A body parameter of type `Json<T>` reads the request body as JSON. A request whose body is not valid JSON for `T` gets `400 Bad Request` without reaching the handler.
- A reply of type `Json<T>` is `200 OK` with `T` as an `application/json` body. `Option<Json<T>>` and `Result<Json<T>, E>` reply as `Option<String>` and `Result<String, E>` do.
- `T` must derive serde's `Serialize` to be replied, and `Deserialize` to be read, as it would in Rust. Serde's attributes, such as `#[serde(rename_all = "camelCase")]`, mean what they mean in Rust, and the API contract shows their effect.
- A query parameter is written after `?` in the path template, in `{` `}` like a path parameter, and binds to the function parameter of the same name. Several are separated by `&`: `"/items?{limit}&{after}"`. A parameter of type `Option<T>` may be left out of the request. Any other must be present, or the request gets `400 Bad Request`.
- `header("Name") = param` binds a request header to a parameter, converted as a path segment is. As for query parameters, an `Option` header may be missing and any other must be present.
- Headers in the reply, other than `Content-Type`, are not yet supported.

*Why query parameters go in the path template:* every input of a route is then visible in one string, which is what the diagram labels the route with and what the API contract describes. Rocket writes them the same way, with `<` `>` where Inset uses `{` `}` for its path parameters.

*Why `via` is required:* the capability is the source of the diagram arrow, so it is always written down. It also lets a component hold two servers, such as a public one and an admin one.

*Why statuses are attributes on the error type:* the same enum is often both a message's error and a route's, as `ShipError` is. The `?` operator then carries an error from a child straight to the HTTP reply, and the status sits next to the variant that means it.

### Messages between components

**Status:** Used.

A component's `#[message]` functions are its messages. Another component sends one by calling the method of the same name on a `Handle` to it.

| Form | Meaning |
| --- | --- |
| `self.inventory.ship(&sku, quantity).await` | Send a message, wait for the reply, and evaluate to it |
| `send self.reorders.raise(order);` | Send a message and carry on without waiting. The function must return nothing |

- Each call becomes a message arrow on the diagram, from the sender to the receiver. Arrows for `send` are drawn differently from those that wait.
- A message is always awaited or sent with `send`, whether or not the receiving function is `async`.
- `Handle<C>` has one async method for each message of `C`, with the same parameters and reply, taking `&self`. So sending a message needs only `&self`, and the receiver's `&mut self` applies in its own handler, on its own task. This is the actor-handle pattern from Rust's async ecosystem, so a call on a handle means what it would in Rust.
- A message function's visibility follows Rust: the handle's method can be called wherever the function is visible.
- A component can send messages to the children it started and to the siblings lent to it (see [Starting components](#starting-components)). It holds no handle to its parent or to a later sibling. To tell them something, it emits through an outlet (see [Outlets](#outlets)).
- An awaited message is in its receiver's mailbox before its sender carries on. A message sent with `send` is delivered when the sending handler completes, before its reply, so that a handler that fails can take it back (see [Failures](#failures)). So when one message leads to another, even through other components, the first arrives first. The Stock tracker relies on this: `Inventory` emits a reorder through its outlet in its `ship` handler, so a `pending()` that `Warehouse` sends after the reply finds the reorder already raised.

*Why calls look like async method calls:* a message that waits for its reply behaves like an awaited async call, so the familiar syntax keeps its familiar meaning. `send` has no equivalent in Rust, so it gets a keyword.

### Outlets

**Status:** Used.

An outlet lets a component announce something without knowing who receives it. The component declares the outlet as a field and emits values through it. Whoever starts the component connects the outlet to a message.

```rust
component Inventory {
    items: Table<String, Item>,
    /// A reorder for each item that shipping leaves at or below its reorder level.
    low_stock: Outlet<Reorder>,
    ...
            send self.low_stock.emit(Reorder { sku: sku.to_string(), quantity: order });
    ...
}
```

```rust
start inventory: Inventory { items: move items, low_stock: send reorders.raise };
```

- `Outlet<T>` has one method, `emit(&self, value: T)`, which is always used with `send`. An outlet has nothing to await.
- The handoff `send name.message` connects the outlet to a message of the child or sibling `name`, and `send self.message` to a message of the component doing the starting. The message must take one parameter, of type `T`, and return nothing.
- The target can be any sibling, started before or after, or the parent itself.
- Every outlet is connected, to exactly one message.
- *(Draft)* A component can connect a child's outlet to an outlet of its own of the same type, `send self.changes`, passing the value on up, as a module's port in a hardware description language connects to a port of the module around it.
- An emitted value is moved, and is delivered as any `send` is: when the emitting handler completes. A rollback drops it.
- An outlet, like a handle, cannot be moved, copied, stored in data or sent in a message.
- The diagram draws an outlet as a port on the edge of its component's box, labelled with its name, and a dashed arrow from the port to the message it is connected to.

*Why the parent connects outlets:* the component names only what it emits, never who receives it, so it depends on nothing around it and works unchanged under another parent. Every connection is written in the parent's `start` statements, so one place holds all the arrows between a parent's children, and the diagram is drawn from it. Hardware description languages work the same way: a module declares its output ports, and the module that instantiates it decides what they drive. An outlet also gives no more than is needed. `Inventory` only ever raises reorders, so it gets an outlet rather than a handle to `Reorders`, which would also let it await `pending()`.

*Why an outlet can point anywhere:* it is only ever used with `send`, which never waits, so a connection back up the structure or to a later sibling cannot make two components wait on each other.

*Why `send reorders.raise`:* naming a method without calling it is not valid Rust, so the connection cannot be mistaken for Rust, and `send` says it never waits. The closure `|order| send reorders.raise(order)` would read more like Rust, but closures cannot contain `send`.

### Failures

**Status:** Used.

Errors a component expects are values, as in Rust: a handler returns a `Result`, and its error type says what went wrong, as `ShipError` does. A **failure** is everything else: a panic, an integer overflow, a capability call that fails, or a message that fails. By default a failure stops the whole program. A component may respond to particular failures instead, and each response it gives is shown on the diagram with its reason.

```rust
component Inventory {
    items: Table<String, Item>,
    low_stock: Outlet<Reorder>,

    failure {
        /// A database error affects only the request it happened in. The next request
        /// may succeed, so the program keeps running.
        Capability(items) => rollback,
    }
    ...
}
```

The conditions a component can respond to:

| Condition | Raised when |
| --- | --- |
| `Panic` | A handler panics: `unwrap`, `expect`, an index out of bounds, `assert!` or `panic!` |
| `Overflow` | Integer arithmetic overflows. Arithmetic is always checked, in tests and in use alike. Code that expects overflow says so with Rust's `checked_add`, `saturating_sub` and similar, which return values instead |
| `Capability(field)` | A call to the capability in `field` fails, such as a database error |
| `Message(name)` | An awaited message to the child or lent sibling `name` fails |
| `Child(name)` | The child `name` fails with a failure it does not respond to |
| `_` | Any other failure |

The responses:

| Response | Effect |
| --- | --- |
| `rollback` | The handler is undone: the component's fields go back to how they were before it, its capability writes are rolled back, and messages it sent with `send` are not delivered. Its caller's await fails with `Message`, or an HTTP request gets `500 Internal Server Error` with an empty body. The component carries on |
| `restart` | As `rollback`, and the component is then reset to how it started, with its children restarted. After `Child(name)`, it is the child that restarts |

- Each arm must have a doc comment giving the reason. The diagram shows it.
- Every rollback is reported by the host, even when the program carries on, so a person running the tool can see it happened. A handler that was delivered a message by `send` has no caller to fail, so the report is all that shows it.
- A failure the component does not respond to passes to its parent as `Child(name)`. A failure the app reaches stops the program, and the host reports where it happened.
- A failure is never turned into a success. A response can only contain a failure: fail one request instead of the whole program, or start a component again.
- A capability whose effects cannot be undone allows only `restart` for its own failures. *(Draft: no such capability exists yet.)*
- An external component cannot be rolled back, because its state is Rust the compiler cannot see, so only `restart` is allowed for it.

*Why the default is to stop everything:* a failure is a bug or a broken environment, and a program that carries on after one hides it. Restarting by default would hide it from a person testing the tool by hand. So nothing is hidden unless the source says why, and the reviewer judges the reason on the diagram. This is also why there is no response that replies with a value: an expected error belongs in the handler's `Result`, where requirements can test it.

*Why the parent decides about its children:* a child cannot know whether the rest of the program can go on without it. Erlang's supervisors and Akka's supervision strategies put the decision in the parent for the same reason.

### Ownership and borrowing

**Status:** Used.

Inset uses Rust's ownership rules: every value has one owner, and a value can be lent out by borrowing. Inside a component, the notation is Rust's and moves are implicit. At a boundary, the notation is required.

| Notation | Meaning | Inside a component | Across a boundary |
| --- | --- | --- | --- |
| `move x` | Ownership passes to the receiver, and `x` can no longer be used | Implicit, as in Rust | Required for a value that is not `Copy` |
| `&x` | The receiver may read `x` until it replies, or use the lent child `x` while it runs | As in Rust | Allowed in a message that is awaited, and for lending a child in `start` *(Draft)* |
| `x`, where `x` is `Copy` | The receiver gets a copy, and the sender keeps `x` | As in Rust | Allowed |
| `&mut x` | The receiver may change `x` | As in Rust | Never allowed |

A boundary is any place a value leaves a component: a handoff in `start`, and an argument of a message to another component.

- A place, meaning a variable or a field, that crosses a boundary is written with `move` or `&`, or bare if its type is `Copy`. The compiler rejects a bare place of any other type.
- Any other expression makes a new value, which simply moves. `Vec::new()`, `"Hello"` and `Reorder { sku, quantity: order }` need no notation.
- A reply always moves its value, so it needs no notation.
- `&x` in a message is allowed only when the sender awaits the reply, because the sender then outlives the borrow. `send` does not wait, so it can only move or copy. `&child` in `start` is allowed because the parent outlives its children.
- A value lent in a message reaches the receiver as a copy, and its handler gets `&T` to the copy. The type must therefore be `Clone`. The compiler rejects lending any other type, and its error says to derive `Clone`. For large values the runtime may share one immutable copy instead, which cannot be told apart.

*Why a lent value is copied:* the receiver runs on its own task, and Rust does not let a plain reference cross tasks. Running the receiver inside the sender's borrow would be unsound, because a sender cancelled while it waits would leave the receiver holding a dangling reference, and it would need `unsafe` to build. A copy behind `&T` behaves exactly as a Rust borrow does for the receiver: it can read the value, but cannot change it, and cannot keep the reference, because Inset has no lifetimes to store it with. So `&` keeps its Rust meaning, with no `unsafe`.
- Capabilities are never `Copy` and never lent, so handing one down is always a `move`.

*Why `move` is written at boundaries when Rust leaves it implicit:* the boundary rule is what the diagrams depend on, so that is where a reader must see it. `move` in this position is not Rust syntax, which marks it as an Inset boundary rather than a familiar construct with a new meaning.

*Why `Copy` values cross bare:* a copy shares nothing, so it cannot hide a data flow. Writing `move quantity` for a number the sender goes on using would be untrue, and Rust itself copies such values silently. The diagram labels the arrow as a copy. Whether a type is `Copy` is decided as in Rust, by `#[derive(Clone, Copy)]` on its own types.

### Modules

**Status:** Used.

Inset's modules are Rust's, with `.inset` in place of `.rs`.

```rust
// src/main.inset
mod warehouse;

use warehouse::Warehouse;
```

- The root module is `src/main.inset`, which holds the app.
- `mod name;` declares a module, found where Rust would find it: `src/name.inset`, or `src/name/mod.inset`.
- `use` brings names into scope, with Rust's paths: `crate::`, `super::` and `self::`.
- `pub`, `pub(crate)` and `pub(super)` mean what they mean in Rust. An item without `pub` is visible in its own module and the modules inside it, so the Stock tracker's `warehouse` module can `use crate::{Item, Reorder, ShipError}`.
- Modules organise the source. They do not appear on the diagrams, which show components.

*Why Rust's modules exactly:* every model that writes Rust writes `mod`, `use` and `pub` without thinking. Any other module system would be one more thing to get wrong, and a look-alike with different rules would be worse.

### Types

**Status:** Used.

Inset uses Rust's types: `bool`, the integer types, `String`, tuples, `Option`, `Result`, the standard collections, `struct` and `enum`. It adds `Decimal`, an exact decimal number for money and quantities, written with the `dec!` macro as in the `rust_decimal` crate: `dec!(1.20)`. Its methods are those of `rust_decimal`, such as `scale` and `round_dp`.

```rust
/// A stocked item, stored under its SKU.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    /// How many the warehouse holds.
    on_hand: u32,
    ...
}
```

- `struct` and `enum` are declared at the top level, as in Rust.
- `#[derive(...)]` may list `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` and `Default`, and serde's `Serialize` and `Deserialize` for JSON bodies *(Draft)*. Other standard traits are implemented by hand (see [What Inset keeps from Rust](#what-inset-keeps-from-rust)).
- A type compared in a requirement step must derive `PartialEq` and `Debug`, so that a failed test can show both values.
- The compiler decides how a stored type is written to a `Table`. No derive is needed for it, because the stored form is never seen outside the program. A JSON body is seen outside, so its form is stated with serde, as in Rust.

### Algorithms

**Status:** Used.

An algorithm is a function that takes no `self`, written as in Rust. Its body becomes a flowchart.

```rust
/// Adds VAT at 20%, rounded to the nearest penny.
fn add_vat(net: Decimal) -> Decimal {
    (net * dec!(1.20)).round_dp(2)
}
```

- An algorithm at the top level of a module is a Rust function, as in CSV import. An algorithm inside a component is a Rust associated function: the component calls it as `Self::name(...)`, as `Inventory` calls `Self::reorder_quantity`.
- Where an algorithm can be called from follows Rust's visibility rules, with `pub` to call it from outside its module.
- An algorithm is pure. With no `self`, it cannot reach a capability or a child, and it cannot be `async`. It can call other algorithms and use `fork`.

*Why algorithms are pure:* the flowchart then shows the whole of what the algorithm does, with no effect hidden inside it. An algorithm that needs stored data is given it by the handler that calls it, so the capability edge stays on the component.

### Structured concurrency

**Status:** `fork for` is Used. `fork { ... }` is Draft.

`fork` runs work concurrently and waits for all of it to finish before going on. The fork and its join appear on the flowchart.

```rust
let results = fork for row in rows {
    parse_row(row)
};
```

- `fork for pattern in collection { body }` runs the body once for each element, concurrently, and evaluates to a `Vec` of the body's values in the order of the collection.
- `fork { a, b, ... }` runs each expression concurrently and evaluates to a tuple of their values. *(Draft: first needed by a handler that awaits two children at once.)*
- The branches follow Rust's rules for scoped threads: they may share borrowed data, but none may change data the others can see.
- `fork` can be used in algorithms and in handlers. In a handler, its branches may await messages.

*Why `fork` is a keyword:* Rust's structured concurrency, such as `std::thread::scope` and `join!`, uses closures and macros, which Inset leaves out. Concurrency is also what a reviewer most needs to see on a flowchart, so it gets syntax that looks new. `fork for` reads like the `for` loop it parallelises.

### External components

**Status:** Used for Rust. External components written in other languages, as ready-built WebAssembly components, are Draft.

An external component is written in another language. Inset declares its messages and capabilities, and the diagram shows it as an opaque box with its interface.

```rust
/// Reads CSV text with the `csv` crate. Holds no capabilities, so it can only compute.
#[rust(path = "csv_reader.rs", crates = ["csv@1.3"])]
extern component CsvReader {
    /// Reads every row after the header line, whatever number of fields it has.
    #[message]
    fn read(&self, text: &str) -> Vec<Row>;
}
```

- `extern component` declares a component with no Inset body. As in a Rust `extern` block, its functions are signatures ending in `;`.
- Its fields are the capabilities it holds, as in any component. `CsvReader` has none. *(Draft: an external component with capabilities.)*
- `#[rust(path = ..., crates = [...])]` names the Rust file that implements it, relative to the file declaring it, and the crates that file may use, as `name@version`. The versions chosen are locked in `inset.lock` beside `src/`, in Cargo's lock-file format, so a build always uses the same code.
- An external component's message parameters and replies must be types the WebAssembly component model can carry: the program's own `struct` and `enum` types, and the standard types, with a map carried as a list of pairs. Messages already allow no generics or lifetimes.

**The Rust side.** The Rust file is the root of a crate of its own. The compiler generates a crate named `interface` from the declaration, holding:

- a trait with the component's name, with one method per declared message, exactly as declared, and `fn new(...) -> Self`, which takes the component's fields in order;
- the program's types that the messages use, such as `Row`;
- an `export!` macro, which names the type that implements the trait.

```rust
use interface::{CsvReader, Row};

struct Reader;

impl CsvReader for Reader {
    fn new() -> Self {
        Reader
    }

    fn read(&self, text: &str) -> Vec<Row> { ... }
}

interface::export!(Reader);
```

- A file that does not match the declaration fails to build with rustc's own error about the trait, such as a missing method or a wrong signature.
- `new` is called when the component starts, and again when it restarts.
- The trait exists only in the generated Rust. Inset code still cannot declare traits.

*Why a generated trait:* it is how WebAssembly's own Rust bindings, from `wit-bindgen`, ask for an implementation, so a model has seen it many times, and a mismatch is reported in terms a model already knows how to fix.

**Enforcement.** Each external component is built as a WebAssembly component of its own. It imports only the capabilities in its fields, so the host gives it nothing else: a crate that tries to open a file gets an error from the host, not the file. Values crossing into and out of it are copied, as message values are.

*Why a component of its own:* Rust compiled into the same WebAssembly component as the rest of the program could call anything the program was granted, whatever its declaration says, and the diagram would then be wrong. A separate component lets the host enforce the declaration. This is what the WebAssembly component model is for, and why Inset compiles to it.
- An external component is started and sent messages like any component. Like any component, requirements reach it only through the app, as CSV import's "A field in quotes may contain commas" does.

*Why `extern`:* in Rust, `extern` declares something whose body is written elsewhere, which is what this is. `component` after it says what kind of thing is declared.

## Requirements dialect

**Status:** Used.

Each requirement is exactly one test of what the program does, seen from outside. The test opens with the requirement as a sentence, then states the behaviour in steps.

```
requirement "Visitors are greeted by the name in the web address"
  when HelloHttp receives GET /hello/Ada
  then HelloHttp replies OK with "Hello, Ada!"
```

**Layout.** A requirement starts at the left margin. Its lines are indented under it by two spaces, one line per step. A step continues onto following lines that are indented further. Tabs are an error. `//` starts a comment.

**Name.** The text after `requirement` is the requirement. It is unique in the program, and it is what test reports show.

**What a requirement can see.** A requirement tests the app as a black box. Names in its values, such as `Item`, resolve as they would in `src/main.inset`. It can send the app HTTP requests, set up what the app's granted capabilities hold, and observe the replies and what the capabilities hold afterwards. It cannot name a component, a message or an algorithm. The test runs the built program in the same WebAssembly host that runs it for real, with a fake behind each `grant`, and every component starts for real.

*Why tests run in the host:* there is then one way to run a requirement, and it tests what ships, including the host's enforcement of each component's capabilities. A faster native mode would add a second answer to "what did this test run?", and a program could pass in one and fail in the other. An AI author spends nothing while it waits for tests, and the host starts a built component in well under a second, so the cost is small.

*Why requirements see only the outside:* review splits into jobs. The diagrams show how the program is built, and the requirements state what it must do. A requirement that named `Inventory` or `reorder_quantity` would write the AI's own design into the statement of intent, which is where a hidden assumption does the most harm, because the reviewer approves it as a requirement. A requirement written against the outside also survives any restructuring of the inside. The cost is that a failure names the behaviour that broke, not the code that broke it. The diagram and the coverage overlay show where to look.

### Steps

The compiler resolves every step against the program, so a step that names an unknown route or capability is a compile error, not a test failure.

| Step | Meaning |
| --- | --- |
| `given <fact>` | Sets up what a fake capability holds before the test |
| `when App receives METHOD /path` | Delivers an HTTP request to the app |
| `when App receives METHOD /path with body` | Delivers an HTTP request with a body |
| `then App replies Status` | The response has the status and an empty body |
| `then App replies Status with body` | The response has the status, and its body equals `body` |
| `then <fact>` | A fake capability holds this now |
| `then <check>` | A Rust condition on what fake capabilities hold is true |
| `and ...` | Repeats the kind of the step before it |

- `App` is the name of the program's app, so the steps read as sentences about the program.
- Steps run in order. A requirement may have several `when` steps, each followed by the `then` steps that check it, as in "A new reorder for an item replaces its earlier one". A `then ... replies` step checks the reply to the most recent `when`.
- A fake capability starts empty, so `given items is empty` states the default. It is written when the empty state is part of the requirement.
- The path is written without quotes, as it would appear in a browser's address bar.
- Statuses are written with their standard names in `UpperCamelCase`, such as `OK`, `NotFound`, `Conflict`, `BadRequest` and `InternalServerError`.
- When the app grants more than one `HttpServer`, the `when` step names the one to use: `when HelloHttp receives GET /hello/Ada via http`. *(Draft.)*

*Why steps use HTTP terms:* requirements are reviewed by software engineers, so steps use the precise terms of the boundary they test. At an HTTP boundary, the method and the status are often the requirement itself, such as a missing item giving `NotFound`. Plain verbs such as "asks for" would hide them behind a vocabulary the reviewer has to learn anyway. The plain-English part of a requirement is its sentence, which is what test reports show.

### Capability facts

A fact states what a fake capability holds. The capability is named by its `grant`. Each capability type defines the facts that can be stated about it, in a fixed form, so no phrase needs glue code.

| Capability | Fact | Meaning |
| --- | --- | --- |
| `Table<K, V>` | `name[key] is value` | The table holds `value` under `key` |
| `Table<K, V>` | `name is empty` | The table holds nothing |
| Any | `name fails` | Every call to the capability fails, after `given` only |
| `HttpServer` | None | Its requests and replies are the `when` and `then` steps |

The same fact sets up state after `given`, and checks it after `then`. Facts after `then` are how a requirement observes what the program did, such as `and items["bolt-m6"] is Item { on_hand: 35, ... }`. Replies and capabilities are everything a program can do that anyone outside it could observe.

A **check** is a Rust condition, for a `then` step whose expected value is not known exactly, as in a property. In a check, `name[key]` is the value the table holds under `key`, and the test fails if it holds none: `and products["bolt-m6"].gross >= <net>`.

*Why facts look like indexing:* `items["bolt-m6"]` is how a Rust programmer reads one entry of a map, and `is` reads as a statement of fact both when setting up and when checking.

### Values

Values in facts are written as Rust literals and constructors: `40`, `"M6 bolt"`, `Item { on_hand: 40, ... }`.

- A value takes the type the program expects in that place. `"M6 bolt"` is a `String` in a `String` field, and `0.10` is a `Decimal` in a `Decimal` field.
- An HTTP body is text. It is written as a string literal, or as a text block.
- *(Draft)* A JSON body is written as a value of the route's type, such as `with Product { sku: "bolt-m6", net: dec!(0.10) }`. The test converts it to JSON with the type's serde form, and a reply is compared as a value after it is read back, so the order of keys and spacing do not matter. A string literal or text block is still sent as written, for when the exact JSON is the requirement.
- *(Draft)* A query parameter is part of the path, as in an address bar: `when StockTracker receives GET /items?limit=10`. A request header is a line of its own after the `when` step: `and header If-Match is "3"`.

**Text blocks.** A body that spans lines is written as a text block, after `with` at the end of the step's line. The block starts and ends with `"""` on lines of their own, indented under the step. Each line between them is one line of the text, ending in a newline, with the block's indentation removed. Text blocks come from Gherkin's doc strings.

```
  when PriceImport receives POST /price-lists with
    """
    sku,name,net
    bolt-m6,M6 bolt,0.10
    """
```

### Examples

A requirement with several cases writes its steps once, with `<name>` placeholders, and lists the cases in an `examples` table after the steps. Each heading names a placeholder, each row is one case, and the report shows each row's result. This is Gherkin's Scenario Outline.

```
requirement "Stock that falls to its reorder level or below is reordered up to its target level"
  given items["bolt-m6"] is Item { on_hand: <on hand>, reorder_level: 10, target_level: <target level> }
  when StockTracker receives POST /items/bolt-m6/shipments with "1"
  then StockTracker replies OK with "<left>"
  when StockTracker receives GET /reorders
  then StockTracker replies OK with <reorders>
  examples
  | on hand | target level | left | reorders       |
  | 12      | 50           | 11   | ""             |
  | 11      | 50           | 10   | "bolt-m6 40\n" |
```

- Only `<name>` where `name` is a heading counts as a placeholder, so `<` and `>` keep their Rust meaning elsewhere in a step.
- A placeholder is replaced by its cell's text before the step is read, so it may stand for a whole value, as `<reorders>` does, or for part of a path, a string or a text block, as `"<left>"` does.
- Headings are free text, without `|`, `<` or `>`, so they can read as English.
- The compiler checks every row's steps, so a cell of the wrong type is a compile error.

### Properties

A rule that holds for every input is written with `for all` before the steps, and the compiler generates the inputs. The `where` condition limits them. Both are Rust, so they follow Rust's typing, as in `dec!(0)`. The steps use the generated value as the placeholder `<name>`, with the same replacement as in examples, using the value's `Display` text. In a check, the placeholder is the value itself.

```
requirement "The gross price is never below the net price"
  for all net: Decimal where net >= dec!(0) && net.scale() <= 2
  given products is empty
  when PriceImport receives POST /price-lists with
    """
    sku,name,net
    bolt-m6,M6 bolt,<net>
    """
  then PriceImport replies OK with "Products imported: 1"
  and products["bolt-m6"].gross >= <net>
```

- `for all a: A, b: B where ...` generates several inputs. *(Draft.)*
- A property that fails reports the smallest input it found that breaks it.

## Requirements and coverage

**Status:** The layout and coverage rules are Used. `#[defensive]` is Draft.

A program's directory holds its code in `src/` and its requirements in `requirements/`. Together they are one program. The compiler rejects a `*.req.inset` file in `src/` and a `*.inset` code file in `requirements/`.

*Why requirements have their own directory:* the requirements are the statement of intent, which the reviewer reads and approves, and `src/` is what the diagram is drawn from, which the reviewer never reads. Separate directories make a change of intent visible in any diff, let a repository require a human to approve changes under `requirements/`, and guard against an AI quietly changing a requirement to match what it built. They also let requirements be agreed before the code exists.

**Coverage.** Every branch of the code should be reached by some requirement. Code that no requirement reaches is either a missing requirement or behaviour nobody asked for, and the coverage overlay marks it on the diagram.

- Coverage counts branches, not statements. A branch is each way out of an `if`, `else`, `match` arm or `let ... else`, the error path of each `?`, and running a loop's body or not.
- Each arm of a `failure` block is a branch too. A requirement reaches it by making the failure happen, as the Stock tracker's `and items fails` reaches both of its arms.
- The branches are the edges of the flowcharts, and the overlay colours those edges, so what counts as covered is exactly what the reviewer sees.

*Why branches:* an `if` with no `else` can have every statement run without the case where its condition is false ever being tested. Statement coverage would call it covered, and the missing requirement would go unseen.

**Defensive code.** *(Draft: no example needs it yet.)* Some code cannot be reached from outside by design, such as a check for a state that the rest of the program rules out. It is marked with `#[defensive("reason")]`:

- on a `match` arm, or on a `failure` arm;
- as the first line of the block of an `if` or `else` branch, or of a `let ... else`, marking that branch;
- on a statement, marking the branches inside it, such as the error path of a `?`.

```rust
let Some(net) = parse_price(&fields[2]) else {
    #[defensive("parse_row has already checked every price")]
    return Err(ImportError::Internal);
};
```

- The overlay shows defensive code as excused, not as covered, with its reason, so the reviewer judges each excuse.
- Defensive code that a requirement does reach is reported, because the reason given for it is then wrong.
- A whole function or component cannot be marked. Code that no requirement can reach at all is not defensive but unused, and should be deleted.

*Why not whole functions:* a mark on a function would excuse everything in it at once, which is the easiest way to hide behaviour nobody asked for. Marking single branches keeps each excuse small enough to judge.

## Grammar of the used subset

Informal EBNF for the constructs marked Used. Code-dialect tokens follow Rust's lexical rules. In the requirements dialect, `INDENT` and `DEDENT` come from the layout rule, and `NEWLINE` ends a line.

Code dialect (`*.inset`):

```
program      = module ;
module       = { use_decl | mod_decl | item } ;
use_decl     = a Rust `use` declaration ;
mod_decl     = [ "pub" ] "mod" ident ";" ;
item         = { doc } [ "pub" ] ( app | component | extern_comp | struct | enum | algorithm ) ;

app          = "app" Ident "{" { { doc } setting | grant | start } "}" ;
setting      = "setting" ident ":" type "=" literal ";" ;
grant        = "grant" ident ":" type "{" [ fields ] "}" ";" ;
start        = "start" [ ident ":" ] Ident "{" [ handoffs ] "}" ";" ;
handoffs     = handoff { "," handoff } [ "," ] ;
handoff      = ident ":" ( "move" place | "&" ident | "send" ( "self" | ident ) "." ident | expr ) ;

component    = "component" Ident "{" [ field_decls ] { start } [ failure ] { { doc } function } "}" ;
failure      = "failure" "{" { doc { doc } condition "=>" response "," } "}" ;
condition    = "Panic" | "Overflow" | "Capability" "(" ident ")" | "Message" "(" ident ")" | "Child" "(" ident ")" | "_" ;
response     = "rollback" | "restart" ;
field_decl   = { doc } [ "#[handed_down]" ] ident ":" type ;
function     = [ route_attr | "#[message]" ] [ "pub" ] [ "async" ] "fn" ident "(" fn_params ")" [ "->" type ] block ;
route_attr   = "#[" METHOD "(" STRING "," "via" "=" ident [ "," "body" "=" ident ] ")" "]" ;
fn_params    = [ receiver [ "," ] ] [ params ] ;
receiver     = "&self" | "&mut self" ;

extern_comp  = rust_attr "extern" "component" Ident "{" [ field_decls ] { { doc } signature } "}" ;
rust_attr    = "#[" "rust" "(" "path" "=" STRING "," "crates" "=" "[" STRING { "," STRING } "]" ")" "]" ;
signature    = "#[message]" [ "pub" ] [ "async" ] "fn" ident "(" fn_params ")" [ "->" type ] ";" ;

struct       = [ derive ] "struct" Ident "{" [ field_decls ] "}" ;
enum         = [ derive ] "enum" Ident "{" variant { "," variant } [ "," ] "}" ;
variant      = { doc } [ "#[" "status" "(" Ident ")" "]" ] Ident [ "(" type { "," type } ")" ] ;
derive       = "#[" "derive" "(" Ident { "," Ident } ")" "]" ;

algorithm    = "fn" ident "(" [ params ] ")" [ "->" type ] block ;

field_decls  = field_decl { "," field_decl } [ "," ] ;
fields       = field { "," field } [ "," ] ;
field        = ident ":" expr ;
params       = param { "," param } [ "," ] ;
param        = ident ":" type ;
type         = [ "&" ] Ident [ "<" type { "," type } ">" ] | "(" type { "," type } ")" ;
place        = ident { "." ident } ;

block        = a Rust block, using the features Inset keeps ;
expr         = a Rust expression, using the features Inset keeps
             | "send" expr
             | "fork" "for" pattern "in" expr block ;
```

`METHOD` is one of `get`, `post`, `put`, `patch` or `delete`. `Ident` is an `UpperCamelCase` identifier and `ident` a `lower_snake_case` one. An algorithm inside a component uses the `function` rule with no attribute and no receiver.

Requirements dialect (`*.req.inset`):

```
file         = { requirement } ;
requirement  = "requirement" STRING NEWLINE INDENT [ for_all ] step { step } [ examples ] DEDENT ;
for_all      = "for" "all" ident ":" type [ "where" EXPR ] NEWLINE ;

step         = ( "given" fact
               | "when" Ident "receives" HTTP_METHOD path [ "with" text ]
               | "then" ( Ident "replies" Ident [ "with" text ] | fact | EXPR )
               | "and" step_rest ) NEWLINE ;
fact         = ident "[" value "]" "is" value | ident "is" "empty" | ident "fails" ;
text         = STRING | NEWLINE INDENT '"""' NEWLINE { LINE NEWLINE } '"""' DEDENT ;
path         = "/" segment { "/" segment } ;

examples     = "examples" NEWLINE heading row { row } ;
heading      = "|" HEADING { "|" HEADING } "|" NEWLINE ;
row          = "|" CELL { "|" CELL } "|" NEWLINE ;

value        = a Rust literal, or a struct, enum or list built from values ;
```

`HTTP_METHOD` is one of `GET`, `POST`, `PUT`, `PATCH` or `DELETE`. A `segment` is one or more characters other than whitespace and `/`. `step_rest` is the rest of a step of the same kind as the one before. `EXPR` is a Rust expression. A `HEADING` is free text, and a `CELL` the text that replaces its placeholder. Placeholders, `<` free text `>`, are replaced before a step is read, so they may appear anywhere in a step.
