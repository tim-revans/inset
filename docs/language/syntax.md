# Inset syntax

The living reference for Inset's syntax. It is a draft: the parser in [`crates/inset-syntax/`](../../crates/inset-syntax/) and the checker in [`crates/inset-check/`](../../crates/inset-check/) cover everything the three examples use: components that start children and send them messages, outlets, `failure` blocks that roll back and restart, modules, structs and enums, error enums, `match` and loops, `fork for`, JSON bodies and query parameters, a front end, external components written in Rust, and requirements that set up and check what a table holds, with `examples` tables and properties. They report the rest as not supported yet. The syntax changes as each example program is compiled.

**What Inset makes.** An Inset program is a long-running local service with an HTTP boundary, optionally with a web front end served from `web/`. A program's only way in is an `HttpServer`, and its requirements are written in HTTP terms. Command-line tools and terminal (TUI) applications are planned: commands, a clock and a terminal will be boundaries of their own, with requirement steps of their own. A desktop GUI is written in another language. It calls the program's HTTP routes today, using the contract `inset contract` generates, and is planned to run the program's commands instead. The GUI is reviewed by using it, as a web front end is.

Each construct is marked with its status:

- **Used**: an example program uses it.
- **Draft**: planned, but no example uses it yet, so expect it to change.

How the generated diagrams look is in [`diagrams.md`](diagrams.md), and how a built program runs is in [`runtime.md`](runtime.md). The example programs are in [`examples/`](../../examples/): Hello HTTP, Stock tracker and CSV import.

The VS Code extension in [`editors/vscode/`](../../editors/vscode/) highlights this syntax, so its grammars change with it, in the same commit. Its snapshot tests cover every example program and every code block on this page marked `inset` (code) or `inset-req` (requirements).

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
- Inset adds these keywords to Rust's: `app`, `component`, `setting`, `grant`, `start`, `send`, `fork`, and `failure` with its responses `rollback` and `restart`. It also gives Rust's `extern` a new use, in `extern component`. In an app, `front end`, `via` and `calls` declare the front end (see [Front end](#front-end)).
- Inset adds these attributes: the route attributes `#[get]`, `#[post]`, `#[put]`, `#[patch]` and `#[delete]`, and `#[message]`, `#[handed_down]`, `#[status]`, `#[rust]` and `#[defensive]`. Attributes that change what an item does are familiar from Rust frameworks, so these keep Rust's meaning of an attribute.
- Inset adds three standard types alongside the capability types: `Handle<C>`, for a running component, `Outlet<T>`, for announcing events (see [Outlets](#outlets)), and `Json<T>`, for a JSON body or reply (see [Routes](#routes)).
- Serde's `#[serde(...)]` attribute keeps its Rust meaning on a data type that derives `Serialize` or `Deserialize` (see [Types](#types)).

### What Inset keeps from Rust

Inside a function body, Inset is Rust, and it keeps what models use every day: control flow (`if`, `match`, `for`, `while`, `loop`, `let ... else`, `?`), closures and iterator adaptors, slices and `&str`, the standard collections (`Vec`, `HashMap`, `BTreeMap`, `HashSet`, `BTreeSet`), `impl` blocks on data types, generic functions and data types, lifetimes in algorithms and data types, and `#[derive]`. Data types may implement standard traits such as `Display`, `From`, `FromStr`, `Default` and `Ord`. The pure standard macros `vec!`, `format!`, `matches!`, `assert!` and `write!` are kept, along with `panic!`, which raises a `Panic` failure (see [Failures](#failures)), and `dec!` from `rust_decimal`. Any other macro is an error.

Some of these carry a restriction, so that every effect stays visible:

- A closure cannot `await`, `send`, or call a capability. Every message arrow and capability edge is then a statement of its own, which the flowchart can show. A flowchart draws an iterator chain such as `map` as one "for each" step.
- A method on a data type is an algorithm. Data types cannot hold capabilities or component handles, so such methods are pure.
- Components and messages are not generic and have no lifetimes, so every diagram arrow has a concrete type.

A `match` must cover every case, as in Rust. The compiler reports one that leaves out a variant of an enum, `Some`, `None`, `Ok`, `Err`, `true` or `false`, or that matches a number or text with no arm for the other values. The ranges `a..b` and `a..=b`, `if let` and `while let` are kept. `loop`, `break` and `continue` are not supported yet.

### Rust features Inset leaves out

Inset leaves out only what would hide an effect, a data flow, or which code runs. The compiler rejects each with an error that names the Inset alternative.

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

```inset
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
- `front end via server calls [...];` declares the program's browser front end and the routes it calls (see [Front end](#front-end)).
- `grant` is the only way to create a capability. `setting` and `grant` may appear only in an app.
- Settings may be `bool`, an integer type, or `String`. Passing a setting to a component in `start` is Draft.
- A relative file path in a grant is resolved against the directory the program is run from, as any command-line tool resolves the paths it is given (see [`runtime.md`](runtime.md#settings-and-grants)).

*Why settings are declared:* some configuration is part of what the reviewer approves, and some belongs to whoever runs the program. The Stock tracker's database is its own, so its file is fixed in the grant. CSV import writes a catalogue that other tools read, so where it lives is a setting. Inset does not choose between the two. The source states which values can change at run time, and the diagram lists them with their defaults, so the reviewer can see exactly what the person running the program controls. A setting can only change a value in a grant. It cannot add a grant or change a capability's type.

*Why an app separate from components:* the security-relevant facts of a program, which is everything it can touch outside itself, sit in one short block that the diagram draws as the outer boundary. All three examples keep the app to a few lines of grants and starts. An app has no functions, so it cannot grow logic that hides among the grants.

### Component

**Status:** Used.

A component is an actor. It runs concurrently with other components, holds its own state, and talks to the outside only through messages and the capabilities it was handed.

```inset
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

```inset
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
- *(Draft: no example needs it yet, so the checker accepts it and the compiler reports it as not compiled yet.)* A handoff can lend a child started earlier in the same body to a later one: `reorders: &reorders`. The later child declares the field as a borrowed handle, `reorders: &Handle<Reorders>`, and can send it messages and await their replies. Like all borrows across a boundary, it is written without a lifetime: the compiler knows the parent outlives both children. The parent still owns the lent child.
- A child can only be lent to siblings started after it, never to itself or to its parent.
- A component lends its server to a child with `&`: `start notes: Notebook { http: &http }`. The child declares the field as `http: &HttpServer`, and serves routes of its own on the same port, through `via = http`. The parent keeps the server and may serve routes on it too, and a child may lend it on to its own children. Only a component lends a server: the app moves each grant into one component. Two components may not serve the same method and path on one server. *(Status: implemented and tested; no example needs it yet, so Draft.)*

  *Why servers are lent:* a component handles one envelope at a time, from start to finish, so every request to the routes of one component waits for the one before it to finish. A program whose requests should not wait on each other spreads its routes over components, each serving its own on a lent server. Each request then holds only the component that serves it, and the components it reaches (see [`runtime.md`](runtime.md#concurrent-requests)). Lending a server shares no state: the parent and child each see only their own requests, so the reason capabilities are otherwise never lent does not apply.
- A component handle cannot be moved, copied, stored in data or sent in a message. The only handles a component holds are its own children and the siblings lent to it at `start`. A component that only needs to tell another something is given an outlet instead.
- A component's children start before it handles anything, and stop after it stops.
- A component started by a parent is nested inside the parent on the diagram. The app's children are the top-level components.

*Why the parent wires its children:* who can message whom is then fixed in the source, so the diagram can draw every arrow. Hardware description languages such as Verilog and VHDL wire submodules the same way, and the WebAssembly component model, which Inset compiles to, composes component instances in the same order. Actor languages such as Erlang and Akka instead let a handle travel in any message. Their diagrams cannot be drawn from the code, and two calls waiting on each other can deadlock.

*Why lending only goes to later siblings:* a component that waits for a reply then waits only on its own children or on components started before it, which can never wait on it in turn. So the one-at-a-time rule cannot deadlock. The parent still owns the lent child and outlives the sibling it was lent to, so `&` keeps its Rust meaning.

*Why `start name: Component { ... }`:* it reads like `grant name: Capability { ... }`, the other statement that brings something into being, and like a Rust struct literal for the handoffs. `start` marks it as new: a child is a running actor, not a value.

### Capabilities

**Status:** Used.

A capability is an owned handle to something outside the program, such as a network listener or a stored table. Capabilities cannot be copied, cloned or lent, except that a component may lend its `HttpServer` to a child (see [Starting components](#starting-components)). A component can use only the capabilities in its fields.

| Capability | Granted with | Gives the holder |
| --- | --- | --- |
| `HttpServer` | `HttpServer { port: u16 }` | Incoming HTTP requests on the port, answered by route handlers |
| `HttpClient` | `HttpClient { base: String }` | Calls to the service at the base address, such as `http://127.0.0.1:9000`, over plain HTTP |
| `Table<K, V>` | `Table<K, V> { file: String, name: String }` | A stored map from keys of type `K` to values of type `V`, kept in the table `name` of the database file `file` |

A program cannot declare its own capability types. A capability is something the host grants and enforces, so only the host can provide one. The standard library declares those above.

**Calling a capability.** A capability's methods are async Rust methods, called with the usual Rust rules: `self.items.get(sku).await`. Their arguments are not component boundaries, so they need no boundary notation. The diagram shows one edge per capability, labelled with the operations used.

| `Table<K, V>` method | Does |
| --- | --- |
| `get(&self, key: &Q) -> Option<V>` | Reads the value stored under `key`. As with `HashMap::get`, `key` may be any borrowed form of `K`, such as a `&str` for a `String` key |
| `insert(&mut self, key: K, value: V)` | Stores `value` under `key`, replacing any value already there |
| `remove(&mut self, key: &Q) -> Option<V>` | Removes and returns the value under `key`, which may be any borrowed form of `K`, as for `get` |
| `entries(&self) -> Vec<(K, V)>` | Every key and value, in the order Rust's `Ord` gives the keys, as a `BTreeMap` would |

- `K` and `V` must be data types: they cannot hold capabilities or component handles.
- A key is an integer type, `bool`, `char`, `String`, or a tuple of these. A value owns its data, so it holds no references, `&str` or slices.
- A handler's writes take effect when it completes, all together, or not at all if it is rolled back (see [Failures](#failures)).

| `HttpClient` method | Does |
| --- | --- |
| `get(&self, path: &str) -> Result<String, u16>` | Calls `GET` on the path, added to the base address |
| `delete(&self, path: &str) -> Result<String, u16>` | Calls `DELETE` on the path |
| `post(&self, path: &str, body: &str) -> Result<String, u16>` | Calls `POST` on the path, with the text body |
| `put(&self, path: &str, body: &str) -> Result<String, u16>` | Calls `PUT` on the path, with the text body |

- A reply whose status is from 200 to 299 is `Ok` with its body as text. Any other is `Err` with its status, which the program handles as it handles any value.
- A call that gets no reply, such as one to a service that is not running, fails the handler as a failed table does, with the failure `Capability(field)` (see [Failures](#failures)). So does a reply whose body is not UTF-8.
- While a call waits for its reply, the program goes on with other requests, as long as they do not need a component the waiting request holds (see [`runtime.md`](runtime.md#concurrent-requests)).
- The address is checked as the program starts. An `https://` address is not supported yet.
- *(Not supported yet: request and reply headers, JSON bodies, and timeouts.)*

*Why the status is a value:* a service that answers `404 Not Found` has answered, and what the program does then is part of its design, which the flowchart shows as a `match`. A service that does not answer at all is a failure of the capability, which the `failure` block deals with in one place, as it does for a database.

*Why keys are limited:* the host stores keys in a form whose bytes sort in the keys' order, so the database returns `entries` in order without knowing the key's type (see [`runtime.md`](runtime.md#tables)). The types above have such a form that is simple to define. Others, such as a `Decimal` or a struct, can be added when a program needs them as keys.

*Why capabilities are never lent:* a lent capability would give two components the same database, and the diagram would show two edges to it with no way to say which writes what. With one holder, any component that needs the data asks the holder by message, so the holder is the single place to review all access to that system. In the Stock tracker, only `Inventory` touches `items`.

### Routes

**Status:** Used.

A route attribute turns a component function into an HTTP route handler. The attribute style follows Rust web frameworks such as actix-web and Rocket.

```inset
#[post("/items/{sku}/shipments", via = http, body = quantity)]
async fn ship(&self, sku: String, quantity: u32) -> Result<String, ShipError> {
    ...
}
```

- The attributes are `get`, `post`, `put`, `patch` and `delete`, one for each HTTP method.
- `via = field` names the `HttpServer` capability the requests arrive through. It is always required.
- A path segment in `{` `}` is a path parameter. It binds to the function parameter of the same name, and takes that parameter's type.
- `body = name` binds the request body to the function parameter `name`. The body is read as UTF-8 text and converted to the parameter's type as a path segment is, so a `u32` body is the digits of the number. A body parameter of type `Json<T>` is read as JSON instead (see below).
- The compiler rejects a path parameter or `body` with no matching function parameter, and a function parameter that nothing supplies.
- Each path segment is URL-decoded before it is converted to the parameter's type. A request whose path or parameter types match no route gets `404 Not Found` without reaching a handler. A body that cannot be converted gets `400 Bad Request`.
- The function's value is the reply, as with any Rust function: its final expression, or a `return`.

The return type decides the response:

| Return type | Response |
| --- | --- |
| `String` | `200 OK` with the string as a `text/plain; charset=utf-8` body |
| `Json<T>` | `200 OK` with the value as an `application/json` body, written by serde |
| `Option<String>` or `Option<Json<T>>` | `Some` as for what it holds. `None` replies `404 Not Found` with an empty body |
| `Result<String, E>` or `Result<Json<T>, E>` | `Ok` as for what it holds. `Err` replies with the status named on the error's variant |

The error type `E` is an enum whose every variant has a `#[status(Name)]` attribute, using the status names of the requirements dialect. A unit variant replies with an empty body. A variant holding one `String` replies with that string as the body.

- An enum with `#[status]` on any variant needs it on every variant, and each variant holds nothing or one `String`, whether or not a route uses the enum yet. The compiler reports a variant without a status, a status name that is not one of the requirements dialect's, and a variant holding anything else.
- A route whose error type has no statuses is an error, because the compiler could not say what it replies.

```inset
enum ShipError {
    #[status(NotFound)]
    UnknownItem,
    #[status(Conflict)]
    NotEnoughStock,
}
```

**JSON and query parameters.** *(Used: the Stock tracker lists its stock as JSON, filtered by a query parameter, and stocks an item from a JSON body.)* They take the forms of Rust web frameworks.

```inset
#[get("/items?{low}", via = http)]
async fn stock_levels(&self, low: Option<bool>) -> Json<Vec<StockLevel>> {
    let levels = self.inventory.levels(low.unwrap_or(false)).await;
    Json(levels)
}

#[put("/items/{sku}", via = http, body = item)]
async fn stock_item(&self, sku: String, item: Json<Item>) -> String {
    let Json(item) = item;
    ...
}
```

- `Json<T>` is axum's and actix-web's type of the same name, a tuple struct around the value. It is built with `Json(value)`, taken apart with the pattern `Json(value)`, and its value is also `.0`.
- A body parameter of type `Json<T>` reads the request body as JSON. A request with a body that is not valid JSON for `T` gets `400 Bad Request` without reaching the handler. The request's `Content-Type` is not checked, because the host does not pass request headers yet.
- `T` must derive serde's `Serialize` to be replied, and `Deserialize` to be read, as it would in Rust. Serde's `rename_all` and `rename` mean what they mean in Rust, and the API contract shows their effect (see [Types](#types)).
- A map in a JSON body has `String` keys, because a JSON object's keys are text.
- A query parameter is written after `?` in the path template, in `{` `}` like a path parameter, and binds to the function parameter of the same name. Several are separated by `&`: `"/items?{limit}&{after}"`. A parameter of type `Option<T>` may be left out of the request. Any other must be present, or the request gets `400 Bad Request`.
- A query parameter is converted as a path segment is, after `%XX` escapes are decoded and `+` is read as a space, as browsers write a form. A value that does not convert gets `400 Bad Request`. A parameter the route does not name is ignored, and of two with the same name the first is read.
- *(Draft: no example needs one yet.)* `header("Name") = param` binds a request header to a parameter, converted as a path segment is. As for query parameters, an `Option` header may be missing and any other must be present. The parser reports it as not supported yet.
- Headers in the reply, other than `Content-Type`, are not yet supported.

*Why query parameters go in the path template:* every input of a route is then visible in one string, which is what the diagram labels the route with and what the API contract describes. Rocket writes them the same way, with `<` `>` where Inset uses `{` `}` for its path parameters.

*Why headers wait:* no example program reads a request header, and the host passes a request's method, path and body to the program but not its headers. Headers come with the first program that needs one, with the WebAssembly interface change that carries them.

*Why `via` is required:* the capability is the source of the diagram arrow, so it is always written down. It also lets a component hold two servers, such as a public one and an admin one.

*Why statuses are attributes on the error type:* the same enum is often both a message's error and a route's, as `ShipError` is. The `?` operator then carries an error from a child straight to the HTTP reply, and the status sits next to the variant that means it.

### Front end

**Status:** Used.

A program's browser front end is free-form HTML and JavaScript in `web/`, beside `src/`. The app declares the front end, the server for its files, and the routes the page calls.

```inset
app StockTracker {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    grant items: Table<String, Item> { file: "stock.db", name: "items" };
    /// The page staff use in a browser to see stock levels and stock new items.
    front end via http calls ["GET /items?{low}", "PUT /items/{sku}"];
    start Warehouse { http: move http, items: move items };
}
```

- `front end via server calls [...];` goes in the app. `server` is an `HttpServer` grant, and an app has at most one front end.
- Each call names a route of that server by its method and its path, exactly as the route's attribute writes it, with any query.
- The compiler builds every file under `web/` into the program. A `GET` that no route of the server takes is answered with a file. The path `/`, or any path ending in `/`, gets `index.html`, and any other path gets the file at that path under `web/`. The content type follows the file's extension. A path with no file gets `404 Not Found`.
- `web/` must hold `index.html`, the page at `/`. A `web/` that holds files when the app declares no front end is an error, as a file under `src/` that no `mod` declares is.
- The diagram draws the front end as one box outside the app, on the side of the clients, with an arrow to each route it calls.
- `inset build` writes the program's API contract beside the component, as `target/inset/<package>.d.ts` and `target/inset/<package>.openapi.json`. `inset contract` writes either one alone. The front end imports the TypeScript types to check what it sends and reads, as the Stock tracker's `web/app.js` does with JSDoc.

*Why the calls are declared:* the compiler does not read JavaScript, and a call found by reading it would depend on how the code was written, such as a path built from parts. A list in the app is exact, and the checker holds each entry to a real route, so every arrow on the diagram is true. The list cannot show a call the front end makes without listing it, so the arrows are the author's statement, checked against the routes. The reviewer, who reviews the front end by using it, sees which routes it claims to call.

*Why `web/` and not a grant:* the files are part of the program, written and reviewed with it, so they are built into it as its source is, and the program never reads a file at run time. They are served through the `HttpServer` the app already grants, so the front end adds no capability. The server's holder still answers every route, and the files answer only what no route takes.

*Why the front end is opaque:* business logic stays in the back end, where requirements test it, and the front end is reviewed by using it. The diagram shows only what the front end can reach, which is the routes it calls.

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

```inset
component Inventory {
    items: Table<String, Item>,
    /// A reorder for each item that shipping leaves at or below its reorder level.
    low_stock: Outlet<Reorder>,
    ...
            send self.low_stock.emit(Reorder { sku: sku.to_string(), quantity: order });
    ...
}
```

```inset
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

```inset
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
- `restart` resets the component's state fields, and each child's, to what they were given at `start`, before each takes its next message. A component busy with a handler when its parent restarts it finishes that handler first. Messages already waiting in a mailbox are still delivered, because the handlers that sent them completed. A component awaiting the restarted child gets `Message(name)`, as for any failed message, so a parent that restarts a child it also calls usually responds to both: `Child(tally) => restart` and `Message(tally) => rollback`.
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
- In a message's arguments, a move is written as in a handoff: `send self.reorders.raise(move order)`. `move` anywhere else is an error, because nothing crosses a boundary there.
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

```inset
// src/main.inset
mod warehouse;

use warehouse::Warehouse;
```

- The root module is `src/main.inset`, which holds the app.
- `mod name;` declares a module, found where Rust would find it: `src/name.inset`, or `src/name/mod.inset`.
- `use` brings names into scope, with Rust's paths: `crate::`, `super::` and `self::`. A path starts at the module the `use` is written in, as in Rust 2018 and later. `as` renames, and `{` `}` groups.
- `pub`, `pub(crate)` and `pub(super)` mean what they mean in Rust. An item without `pub` is visible in its own module and the modules inside it, so the Stock tracker's `warehouse` module can `use crate::{Item, Reorder, ShipError}`. A struct's fields follow the same rules, each with a visibility of its own.
- Paths into the standard library, such as `use std::collections::HashMap;`, are allowed, apart from the parts Inset leaves out, such as `std::fs`.
- Modules organise the source. They do not appear on the diagrams, which show components.
- The compiler reports a `mod` with no file, a `mod` with both possible files, and a file under `src/` that no `mod` declares. Rust ignores such a file, but a reviewer would expect it to be part of the program.
- The app is in the root module.
- *(Not supported yet.)* A module written inline, `mod name { ... }`, and a glob import, `use name::*`.

*Why Rust's modules exactly:* every model that writes Rust writes `mod`, `use` and `pub` without thinking. Any other module system would be one more thing to get wrong, and a look-alike with different rules would be worse.

### Types

**Status:** Used.

Inset uses Rust's types: `bool`, the integer types, `String`, tuples, `Option`, `Result`, the standard collections, `struct` and `enum`. It adds `Decimal`, an exact decimal number for money and quantities, written with the `dec!` macro as in the `rust_decimal` crate: `dec!(1.20)`. Its methods are those of `rust_decimal`, such as `scale` and `round_dp`.

```inset
/// A stocked item, stored under its SKU.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    /// How many the warehouse holds.
    on_hand: u32,
    ...
}
```

- `struct` and `enum` are declared at the top level of a module, as in Rust. A struct has named fields, and an enum's variants hold nothing or a list of types.
- A struct or enum holds data. It cannot hold a capability, a handle or an outlet.
- `#[derive(...)]` may list `Clone`, `Copy`, `Debug`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` and `Default`, and serde's `Serialize` and `Deserialize` for JSON bodies. Other standard traits are implemented by hand (see [What Inset keeps from Rust](#what-inset-keeps-from-rust)).
- Of serde's attributes, a type that derives `Serialize` or `Deserialize` may take `#[serde(rename_all = "...")]`, with any of serde's rules such as `"camelCase"`, and a field or variant may take `#[serde(rename = "...")]`. They mean what they mean in Rust, and change only the JSON form. The compiler reports any other serde argument as not supported here, and two fields or variants that end up with one name in JSON.
- A data type cannot be called `Json`, which is the JSON body type.
- A derive needs what Rust needs: `Copy` needs `Clone`, `Eq` and `PartialOrd` need `PartialEq`, and `Ord` needs `PartialOrd` and `Eq`. Each field must have the trait too, so a struct holding a `String` cannot derive `Copy`, and one holding an `f64` cannot derive `Eq`. The compiler reports these, rather than leaving them to rustc.
- Serde writes a struct as an object of its fields, a unit variant as its name, a variant holding one value as `{"name": value}`, one holding several as `{"name": [values]}`, `None` as `null`, and a `Decimal` as text, such as `"0.10"`. Serde reads a missing `Option` field as `None`. The API contract and requirement tests use the same form.
- *(Not supported yet.)* Generic structs and enums, tuple and unit structs, variants with named fields, and deriving `Default` for an enum.
- A type compared in a requirement step must derive `PartialEq` and `Debug`, so that a failed test can show both values.
- The compiler decides how a stored type is written to a `Table` (see [`runtime.md`](runtime.md#tables)). No derive is needed for it, because the stored form belongs to the program. A JSON body is seen outside, so its form is stated with serde, as in Rust, and serde's attributes never change what is stored.

### Algorithms

**Status:** Used.

An algorithm is a function that takes no `self`, written as in Rust. Its body becomes a flowchart.

```inset
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

**Status:** `fork for` is Used. `fork { ... }` is Draft: the parser reports it as not supported yet.

`fork` runs work concurrently and waits for all of it to finish before going on. The fork and its join appear on the flowchart.

```inset
let results = fork for row in rows {
    parse_row(row)
};
```

- `fork for pattern in collection { body }` runs the body once for each element, concurrently, and evaluates to a `Vec` of the body's values in the order of the collection.
- `fork { a, b, ... }` runs each expression concurrently and evaluates to a tuple of their values. *(Draft: first needed by a handler that awaits two children at once.)*
- The branches follow Rust's rules for scoped threads: they may share borrowed data, but none may change data the others can see. The compiler reports a branch that assigns to, or takes `&mut` of, a variable from outside the branch or a field of `self`, one that calls a method such as `push` or `insert` on one, and one that calls a table's `insert` or `remove`. Each branch gives a value instead, and the code after the `fork` gathers them.
- A branch cannot `return` or use `?`, which would leave one branch while the others run. A branch gives a `Result` or an `Option` as its value, as `parse_row` does, and the code after the `fork` handles them, where every branch has finished.
- `fork` can be used in algorithms and in handlers. In a handler, its branches may await messages and capability calls, and may `send`.
- `fork for` is an expression, so a statement that is only a `fork for` ends with `;`, unless it is the block's value.
- A program runs on one thread (see [`runtime.md`](runtime.md#the-executor)). Branches that wait for nothing run one after another, in order. Branches that await run concurrently on that thread: each runs until it waits, then the next starts, so every message the branches send is queued in the order of the collection, and the program behaves the same every time.

*Why branches may not change shared data:* every branch is meant to see the same world, so the order in which they run cannot change the result. A reviewer can then read the body once, as the flowchart draws it, and know it holds for every element. The compiler emits each branch as a closure that Rust allows to read, but not to change, what it captures, so rustc would refuse such a branch too.

*Why branches cannot `return`:* a `return` from one branch would have to decide what happens to the others, which may have sent messages already. Giving each branch a value keeps the join in one place, and lets the code after the `fork` report every failure rather than the first, as CSV import reports every invalid row.

*Why `fork` is a keyword:* Rust's structured concurrency, such as `std::thread::scope` and `join!`, uses closures and macros, which Inset leaves out. Concurrency is also what a reviewer most needs to see on a flowchart, so it gets syntax that looks new. `fork for` reads like the `for` loop it parallelises.

### External components

**Status:** Used for Rust. External components written in other languages, as ready-built WebAssembly components, are Draft.

An external component is written in another language. Inset declares its messages and capabilities, and the diagram shows it as an opaque box with its interface.

```inset
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
- Its functions are `#[message]` signatures taking `&self` or `&mut self`, and are not `async`: the external code runs each message to completion, and the sender still awaits the reply. A message cannot be named `new`, which the trait below takes.
- An external component's message parameters and replies must be types the WebAssembly component model can carry: `bool`, `char`, `String` (lent as `&str`), integers of 8 to 64 bits, `f32` and `f64`, `Vec<T>` (lent as `&[T]`), `Option`, `Result`, tuples, and the program's own `struct` and `enum` types, each holding at least one field or variant. A type cannot contain itself there, because WIT has no recursive types. Messages already allow no generics or lifetimes. *(Draft: `Decimal`, maps, sets, `VecDeque` and a variant holding more than one value, which WIT could carry but the generated bindings would hand the Rust code as a different type: a `String`, a `Vec` of pairs or a tuple.)*
- The names in those types, and the messages and their parameters, must come back unchanged from WIT's kebab case, so the Rust author and the host see the names the program uses: a field such as `on_hand` is `on-hand` in WIT, and a type or variant such as `InvalidRows` is `invalid-rows`. A field `line_2` or a type `Csv_Row` has no such name, and the checker says so.

**The Rust side.** The Rust file is the root of a crate of its own. The compiler generates a crate named `interface` from the declaration, holding:

- a trait with the component's name, with one method per declared message, exactly as declared, and `fn new(...) -> Self`, which takes the component's fields in order;
- the program's types that the messages use, such as `Row`, as `wit-bindgen` declares them, deriving `Clone` and implementing `Debug`;
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
- `new` is called before the component's first message, and again before its first message after it restarts. A component that fails, such as by panicking, loses its instance and its state, so it is always started again; its parent responds with `Child(name) => restart` to carry on, and a component awaiting it gets `Message(name)`.
- A rollback in another component does not undo what an external component did, because its state is Rust the compiler cannot see.
- The trait exists only in the generated Rust. Inset code still cannot declare traits.

- The diagram shows its messages but not their code, so coverage does not count them: requirements reach an external component through the Inset code that sends it messages.

*Why a generated trait:* it is how WebAssembly's own Rust bindings, from `wit-bindgen`, ask for an implementation, so a model has seen it many times, and a mismatch is reported in terms a model already knows how to fix.

**Enforcement.** Each external component is built as a WebAssembly component of its own. It imports only the capabilities in its fields, so the host gives it nothing else: a crate that tries to open a file gets an error from the host, not the file. Values crossing into and out of it are copied, as message values are.

*Why a component of its own:* Rust compiled into the same WebAssembly component as the rest of the program could call anything the program was granted, whatever its declaration says, and the diagram would then be wrong. A separate component lets the host enforce the declaration. This is what the WebAssembly component model is for, and why Inset compiles to it.
- An external component is started and sent messages like any component. Like any component, requirements reach it only through the app, as CSV import's "A field in quotes may contain commas" does.

*Why `extern`:* in Rust, `extern` declares something whose body is written elsewhere, which is what this is. `component` after it says what kind of thing is declared.

## Requirements dialect

**Status:** Used.

Each requirement is exactly one test of what the program does, seen from outside. The test opens with the requirement as a sentence, then states the behaviour in steps.

```inset-req
requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
```

**Layout.** A requirement starts at the left margin. Its lines are indented under it by four spaces, one line per step. A step continues onto following lines that are indented further. Tabs are an error. `//` starts a comment.

*Why four spaces:* the code dialect indents by four, so both dialects look alike, and the deeper indent sets the requirement sentence clear of its steps.

**Name.** The text after `requirement` is the requirement. It is unique in the program, and it is what test reports show.

**What a requirement can see.** A requirement tests the app as a black box. Names in its values, such as `Item`, resolve as they would in `src/main.inset`. It can send the app HTTP requests, set up what the app's granted capabilities hold, and observe the replies and what the capabilities hold afterwards. It cannot name a component, a message or an algorithm. The test runs the built program in the same WebAssembly host that runs it for real, with a fake behind each `grant`, and every component starts for real.

*Why tests run in the host:* there is then one way to run a requirement, and it tests what ships, including the host's enforcement of each component's capabilities. A faster native mode would add a second answer to "what did this test run?", and a program could pass in one and fail in the other. An AI author spends nothing while it waits for tests, and the host starts a built component in well under a millisecond once it is loaded (see [`runtime.md`](runtime.md#starting-a-program)), so the cost is small.

*Why requirements see only the outside:* review splits into jobs. The diagrams show how the program is built, and the requirements state what it must do. A requirement that named `Inventory` or `reorder_quantity` would write the AI's own design into the statement of intent, which is where a hidden assumption does the most harm, because the reviewer approves it as a requirement. A requirement written against the outside also survives any restructuring of the inside. The cost is that a failure names the behaviour that broke, not the code that broke it. The diagram and the coverage overlay show where to look.

### Steps

The compiler resolves every step against the program, so a step that names an unknown route or capability is a compile error, not a test failure.

| Step | Meaning |
| --- | --- |
| `given <fact>` | Sets up what a fake capability holds before the test |
| `when App receives METHOD /path` | Delivers an HTTP request to the app |
| `when App receives METHOD /path with body` | Delivers an HTTP request with a body: text, or a value sent as JSON |
| `then App replies Status` | The response has the status and an empty body |
| `then App replies Status with body` | The response has the status, and its body equals `body`, as text or as a value read from JSON |
| `then <fact>` | A fake capability holds this now |
| `then <check>` | A Rust condition on what fake capabilities hold is true |
| `and ...` | Repeats the kind of the step before it |

- `App` is the name of the program's app, so the steps read as sentences about the program.
- Steps run in order. A requirement may have several `when` steps, each followed by the `then` steps that check it, as in "A new reorder for an item replaces its earlier one". A `then ... replies` step checks the reply to the most recent `when`.
- Each requirement runs against a freshly started program, with its settings at their defaults and every fake empty. No requirement depends on another having run first.
- A requirement fails at the first step that does not hold. `inset test` reports the requirement's sentence, the failing step, and what happened instead: the actual reply, the failure that stopped the program, or what a capability holds. A fact that does not hold shows the expected value and the value found, as `assert_eq!` does, each written as Rust's `{:?}` writes it. A check that compares two values shows both of them.
- A fake capability starts empty, so `given items is empty` states the default. It is written when the empty state is part of the requirement.
- The path is written without quotes, as it would appear in a browser's address bar.
- Statuses are written with their standard names from the HTTP specification (RFC 9110) in `UpperCamelCase`, such as `OK`, `NotFound`, `Conflict`, `BadRequest` and `InternalServerError`.
- When the app grants more than one `HttpServer`, the `when` step names the one to use: `when HelloHttp receives GET /hello/Ada via http`. `via` comes before any `with`. *(Draft.)*
- The request must match a route, with each path parameter converting to its type, so a requirement cannot ask for a path the program has no route for. A route that returns `Option<String>` is how a requirement tests `NotFound`.
- The status must be one the route can reply: `OK` for a `String` or a `Json<T>`, `OK` or `NotFound` for an `Option`, `OK` or any status named on the error type for a `Result`, `BadRequest` too when the route reads a body or a query parameter, and `InternalServerError` for any route, since a rolled-back failure replies with it.

*Why steps use HTTP terms:* requirements are reviewed by software engineers, so steps use the precise terms of the boundary they test. At an HTTP boundary, the method and the status are often the requirement itself, such as a missing item giving `NotFound`. Plain verbs such as "asks for" would hide them behind a vocabulary the reviewer has to learn anyway. The plain-English part of a requirement is its sentence, which is what test reports show.

### Capability facts

A fact states what a fake capability holds. The capability is named by its `grant`. Each capability type defines the facts that can be stated about it, in a fixed form, so no phrase needs glue code.

| Capability | Fact | Meaning |
| --- | --- | --- |
| `Table<K, V>` | `name[key] is value` | The table holds `value` under `key` |
| `Table<K, V>` | `name is empty` | The table holds nothing |
| `Table<K, V>` | `name fails` | Every call to the table fails, after `given` only |
| `HttpClient` | `name["METHOD /path"] replies Status`, or `... replies Status with "body"` | The fake answers that call with the status and the text body, after `given` only |
| `HttpClient` | `name fails` | Every call through the client gets no reply, after `given` only |
| `HttpServer` | None | Its requests and replies are the `when` and `then` steps |

The same fact sets up state after `given`, and checks it after `then`. Facts after `then` are how a requirement observes what the program did, such as `and items["bolt-m6"] is Item { on_hand: 35, ... }`. Replies and capabilities are everything a program can do that anyone outside it could observe.

- After `then`, `name[key] is value` compares the stored value with `value` as the type's `PartialEq` does, so a `Decimal` of `0.1` equals one of `0.10`, as in `rust_decimal`. The type must derive `PartialEq` and `Debug` (see [Types](#types)).
- `name fails` is a fact about what the program calls, so it follows `given`, before the request it fails. A failure is not something a program can be checked to have done.
- `and` repeats the kind of the step before it, so `and items fails` after a `given` step sets up a failure too.

- A call to a fake `HttpClient` that no `replies` fact answers fails the requirement, naming the call: `but weather was called with GET /forecast/London, which no given fact answers`. The program sees the call fail, but the requirement fails anyway, because a requirement that does not say how the other service answers has a gap.
- The method is `GET`, `POST`, `PUT` or `DELETE`, and the path is written from the base address, starting with `/`, with any query.

```inset-req
requirement "A forecast is relayed as the forecast service gives it"
    given weather["GET /forecast/London"] replies OK with "sunny"
    when Relay receives GET /forecast/London
    then Relay replies OK with "sunny"
```

*Why only a table and a client can fail:* `fails` makes the program's calls to a capability fail. A program never calls its `HttpServer`: requests arrive through it, and a requirement sends them itself.

*Why requirements cannot check the calls a program made:* a call to another service is how the program does its job, not something the requirement's reader can observe from outside the program, unless the call changes something there. When it does, a later version may add a `then` fact about the calls, as tables have. *(Not supported yet.)*

A **check** is a Rust condition, for a `then` step whose expected value is not known exactly, as in a property. In a check, `name[key]` is the value the table holds under `key`, and the test fails if it holds none: `and products["bolt-m6"].gross >= <net>`.

- A check reads at least one entry, and is a `bool`.
- It may use entries and their fields, `.len()` and `.is_empty()`, the comparison and arithmetic operators, `&&`, `||` and `!`, and values. *(Not supported yet: any other Rust in a check, such as method calls on a `Decimal`.)*
- A value on one side of an operator takes the type of the other side, as in a fact: `items["bolt-m6"].price >= 0.10` compares two `Decimal`s.

*Why facts look like indexing:* `items["bolt-m6"]` is how a Rust programmer reads one entry of a map, and `is` reads as a statement of fact both when setting up and when checking.

### Values

Values in facts are written as Rust literals and constructors: `40`, `"M6 bolt"`, `Item { on_hand: 40, ... }`.

- A value takes the type the program expects in that place. `"M6 bolt"` is a `String` in a `String` field, and `0.10` is a `Decimal` in a `Decimal` field.
- A value is a literal, `dec!(...)`, a struct that gives every field, an enum's variant, `Some`, `None`, `Ok` or `Err` with a value, a tuple, or a list as `vec![...]`. Names resolve as they would in `src/main.inset`, so a variant is written with its enum, as in `Place::Shelf(3)`. *(Not supported yet: maps and sets.)*
- An HTTP body is text. It is written as a string literal, or as a text block.
- A JSON body is written as a value of the route's `Json<T>` type, such as `with Item { on_hand: 40, reorder_level: 10, target_level: 50 }`. The test sends it as JSON in serde's form for the type. After `replies`, a value is compared with the reply once the reply is read back as the route's type, so the order of keys and the spacing do not matter, and a `Decimal` compares by value. A string literal or text block is still sent, or compared, exactly as written, for when the exact JSON is the requirement.
- A value may run onto the lines below its step, indented further, as a step can.
- A query parameter is part of the path, as in an address bar: `when StockTracker receives GET /items?low=true`. Each parameter must be one the route names. Its value is not checked, so a requirement can show that a value that does not convert gets `BadRequest`.
- *(Draft)* A request header is a line of its own after the `when` step: `and header If-Match is "3"`.

**Text blocks.** A body that spans lines is written as a text block, after `with` at the end of the step's line. The block starts and ends with `"""` on lines of their own, indented under the step. Each line between them is one line of the text, ending in a newline, with the block's indentation removed. Text blocks come from Gherkin's doc strings.

```inset-req
    when PriceImport receives POST /price-lists with
        """
        sku,name,net
        bolt-m6,M6 bolt,0.10
        """
```

### Examples

A requirement with several cases writes its steps once, with `<name>` placeholders, and lists the cases in an `examples` table after the steps. Each heading names a placeholder, each row is one case, and the report shows each row's result. This is Gherkin's Scenario Outline.

```inset-req
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
- The compiler checks every row's steps, so a cell of the wrong type is a compile error. The error points at the placeholder, and names the row's line.
- A requirement has at most one table, after its steps. A cell cannot hold `|`.
- Each row runs as a requirement of its own, against a freshly started program. `inset test` reports each row's result on a line of its own under the requirement, and the requirement passes when every row passes.

### Properties

A rule that holds for every input is written with `for all` before the steps, and the compiler generates the inputs. The `where` condition limits them. Both are Rust, so they follow Rust's typing, as in `dec!(0)`. The steps use the generated value as the placeholder `<name>`, with the same replacement as in examples, using the value's `Display` text. That text is the value as Rust writes it, so in a check the placeholder is the value itself.

```inset-req
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

- An input is an integer type other than `u128`, `bool` or `Decimal`. *(Not supported yet: `String`, `char`, floats and the program's own types.)*
- `for all a: A, b: B where ...` generates several inputs, each named once. *(Draft: implemented and tested, but no example needs it yet.)*
- Every input is used as a placeholder in a step. Only `<name>` where `name` is an input counts as a placeholder, as for an examples table.
- The `where` condition compares inputs with values, using `==`, `<` and the like, `&&`, `||`, `!` and arithmetic, and may call `.scale()` on a `Decimal`. It reads no capability: it limits the inputs before the steps run. *(Not supported yet: any other method.)*
- A property cannot have an examples table.
- The steps are checked once with a sample value for each input, such as `0` or `0.00`, so a mistake in them is a compile error. The note on an error at a placeholder names the sample.
- `inset test` runs each property for 100 generated cases that meet the `where` condition, each against a freshly started program. The cases come from a seed fixed for each property, made from its sentence, so the report is the same every time and only changes when the sentence does.
- A property that fails reports the smallest input it found that breaks it, after shrinking the first failing case towards zero: `fails for net = 1, the smallest failing input found`, then the failing step as for any requirement. The JSON report gives the input as `inputs`, with each input's `name` and `value`, and every property's `cases`.
- A property passes with `held in 100 cases`. One whose `where` condition leaves out too many of the generated values fails, saying how many cases held.
- A property is credited with what any of its cases reached, as an examples table is with its rows.

*Why only these input types:* a placeholder is replaced by text before the step is read. An integer, a `bool` or a `Decimal` reads back as itself anywhere in a step: in a path, in a body, or as a value in a check. A `String` would need quotes in a value and none in a path, so its text would depend on where it is used. That waits for an example that needs one.

*Why a fixed seed:* a requirement test that passes once and fails the next time would teach the reviewer to ignore failures. The seed changes with the sentence, so two properties do not test the same values, and a new seed is chosen only by rewording the requirement, which the reviewer sees.

## Requirements and coverage

**Status:** The layout and coverage rules are Used. `#[defensive]` is Draft: the compiler implements it and a test program uses it, but no example needs one yet.

A program's directory holds its code in `src/` and its requirements in `requirements/`. Together they are one program. The compiler rejects a `*.req.inset` file in `src/` and a `*.inset` code file in `requirements/`.

*Why requirements have their own directory:* the requirements are the statement of intent, which the reviewer reads and approves, and `src/` is what the diagram is drawn from, which the reviewer never reads. Separate directories make a change of intent visible in any diff, let a repository require a human to approve changes under `requirements/`, and guard against an AI quietly changing a requirement to match what it built. They also let requirements be agreed before the code exists.

**Coverage.** Every branch of the code should be reached by some requirement. Code that no requirement reaches is either a missing requirement or behaviour nobody asked for, and the coverage overlay marks it on the diagram.

- Coverage counts branches, not statements. A branch is each way out of an `if`, `else`, `match` arm or `let ... else`, the error path of each `?`, and running a loop's body or not.
- Each arm of a `failure` block is a branch too. A requirement reaches it by making the failure happen, as the Stock tracker's `and items fails` reaches both of its arms.
- Entering a function counts too, so a function with no branches, such as the Stock tracker's `pending_reorders`, is uncovered when no requirement calls it.
- The branches are the edges of the flowcharts, and the overlay colours those edges, so what counts as covered is exactly what the reviewer sees.
- A requirement with an examples table covers what any of its rows reached. A requirement that fails still covers what it reached: its sentence says the code is wanted, and the failure is reported anyway.
- `inset test --coverage` writes which requirements reached each branch, and lists what none reached. `inset diagram --coverage` draws it (see [`diagrams.md`](diagrams.md#coverage)).

*Why branches:* an `if` with no `else` can have every statement run without the case where its condition is false ever being tested. Statement coverage would call it covered, and the missing requirement would go unseen.

**Defensive code.** *(Draft: no example needs it yet.)* Some code cannot be reached from outside by design, such as a check for a state that the rest of the program rules out. It is marked with `#[defensive("reason")]`:

- on a `match` arm, or on a `failure` arm;
- as the first line of the block of an `if` or `else` branch, or of a `let ... else`, marking that branch;
- on a statement with a `?`, marking the `?`'s error path.

A mark anywhere else excuses nothing, so the checker refuses it: on a statement that is not the first line of a branch and has no `?`, and inside a closure, whose branches coverage does not count.

```inset
let Some(net) = parse_price(&fields[2]) else {
    #[defensive("parse_row has already checked every price")]
    return Err(ImportError::Internal);
};
```

- The overlay shows defensive code as excused, not as covered, with its reason, so the reviewer judges each excuse. Everything inside an excused branch is excused with it, because nothing can reach it without reaching the branch.
- Defensive code that a requirement does reach fails `inset test`, which reports the code, its reason and the requirements that reached it, because the reason given for it is then wrong.
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

app          = "app" Ident "{" { { doc } ( setting | grant | start | front_end ) } "}" ;
setting      = "setting" ident ":" type "=" literal ";" ;
grant        = "grant" ident ":" type "{" [ fields ] "}" ";" ;
start        = "start" [ ident ":" ] Ident "{" [ handoffs ] "}" ";" ;
front_end    = "front" "end" "via" ident "calls" "[" [ STRING { "," STRING } [ "," ] ] "]" ";" ;
handoffs     = handoff { "," handoff } [ "," ] ;
handoff      = ident ":" ( "move" place | "&" ident | "send" ( "self" | ident ) "." ident | expr ) ;

component    = "component" Ident "{" [ field_decls ] { start } [ failure ] { { doc } function } "}" ;
failure      = "failure" "{" { doc { doc } condition "=>" response "," } "}" ;
condition    = "Panic" | "Overflow" | "Capability" "(" ident ")" | "Message" "(" ident ")" | "Child" "(" ident ")" | "_" ;
response     = "rollback" | "restart" ;
field_decl   = { doc } [ "#[handed_down]" ] ident ":" type ;
function     = [ route_attr | "#[message]" ] [ "pub" ] [ "async" ] "fn" ident "(" fn_params ")" [ "->" type ] block ;
route_attr   = "#[" METHOD "(" ROUTE_PATH "," "via" "=" ident [ "," "body" "=" ident ] ")" "]" ;
fn_params    = [ receiver [ "," ] ] [ params ] ;
receiver     = "&self" | "&mut self" ;

extern_comp  = rust_attr "extern" "component" Ident "{" [ field_decls ] { { doc } signature } "}" ;
rust_attr    = "#[" "rust" "(" "path" "=" STRING "," "crates" "=" "[" STRING { "," STRING } "]" ")" "]" ;
signature    = "#[message]" [ "pub" ] [ "async" ] "fn" ident "(" fn_params ")" [ "->" type ] ";" ;

struct       = [ derive ] [ serde ] "struct" Ident "{" [ struct_field { "," struct_field } [ "," ] ] "}" ;
struct_field = { doc } [ serde ] [ "pub" [ "(" ( "crate" | "super" ) ")" ] ] ident ":" type ;
enum         = [ derive ] [ serde ] "enum" Ident "{" variant { "," variant } [ "," ] "}" ;
variant      = { doc } [ "#[" "status" "(" Ident ")" "]" ] [ serde ] Ident [ "(" type { "," type } ")" ] ;
derive       = "#[" "derive" "(" Ident { "," Ident } ")" "]" ;
serde        = "#[" "serde" "(" ident "=" STRING ")" "]" ;

algorithm    = "fn" ident "(" [ params ] ")" [ "->" type ] block ;

field_decls  = field_decl { "," field_decl } [ "," ] ;
fields       = field { "," field } [ "," ] ;
field        = ident ":" expr ;
params       = param { "," param } [ "," ] ;
param        = ident ":" type ;
type         = [ "&" ] Ident [ "<" type { "," type } ">" ] | "(" type { "," type } ")" ;
place        = ident { "." ident } ;

block        = a Rust block, using the features Inset keeps ;
expr         = a Rust expression, using the features Inset keeps, such as `match`, `for` and `while`
             | "send" expr
             | "move" place, as an argument of a message
             | "fork" "for" pattern "in" expr block ;
```

`METHOD` is one of `get`, `post`, `put`, `patch` or `delete`. A `ROUTE_PATH` is a string: the path, with `{name}` segments, then optionally `?` and `{name}` query parameters separated by `&`. The `serde` argument is `rename_all` on a struct or enum, and `rename` on a field or variant. `Ident` is an `UpperCamelCase` identifier and `ident` a `lower_snake_case` one. An algorithm inside a component uses the `function` rule with no attribute and no receiver.

Requirements dialect (`*.req.inset`):

```
file         = { requirement } ;
requirement  = "requirement" STRING NEWLINE INDENT ( for_all step { step } | step { step } [ examples ] ) DEDENT ;
for_all      = "for" "all" ident ":" type { "," ident ":" type } [ "where" EXPR ] NEWLINE ;

step         = ( "given" fact
               | "when" Ident "receives" HTTP_METHOD path [ "via" ident ] [ "with" body ]
               | "then" ( Ident "replies" Ident [ "with" body ] | fact | EXPR )
               | "and" step_rest ) NEWLINE ;
fact         = ident "[" value "]" "is" value | ident "is" "empty" | ident "fails"
             | ident "[" STRING "]" "replies" Ident [ "with" STRING ] ;
body         = text | value ;
text         = STRING | NEWLINE INDENT '"""' NEWLINE { LINE NEWLINE } '"""' DEDENT ;
path         = "/" segment { "/" segment } [ "?" query { "&" query } ] ;

examples     = "examples" NEWLINE heading row { row } ;
heading      = "|" HEADING { "|" HEADING } "|" NEWLINE ;
row          = "|" CELL { "|" CELL } "|" NEWLINE ;

value        = a Rust literal, or a struct, enum or list built from values ;
```

`HTTP_METHOD` is one of `GET`, `POST`, `PUT`, `PATCH` or `DELETE`. A `segment` is one or more characters other than whitespace, `/` and `?`, and a `query` is `name=value`. `step_rest` is the rest of a step of the same kind as the one before. `EXPR` is a Rust expression. A `HEADING` is free text, and a `CELL` the text that replaces its placeholder. Placeholders, `<` free text `>`, are replaced before a step is read, so they may appear anywhere in a step.

Expressions, types, patterns and blocks may nest at most 64 levels deep. Deeper code is an error, whose help suggests moving the inner part into a function of its own.

*Why a limit:* the parser reads nested code by recursion, and without a limit, input nested deeply enough would overflow its stack and crash the compiler, or the language server as someone types. No code a reviewer could follow nests anywhere near 64 levels.
