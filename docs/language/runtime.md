# Inset runtime

How a built Inset program runs: the Rust the compiler emits, the runtime library linked into it, and the WebAssembly host that runs it with only the capabilities it is granted.

`inset build` compiles a program's Inset source to Rust with [`crates/inset-codegen/`](../../crates/inset-codegen/), and builds that Rust to a WebAssembly component. `inset run` builds it and runs it in the host. Hello HTTP's generated Rust is pinned by a snapshot test, [`crates/inset-codegen/tests/snapshots/hello__hello_http_rust.snap`](../../crates/inset-codegen/tests/snapshots/hello__hello_http_rust.snap), rather than kept with the example, because nobody reviewing a program reads its Rust. The runtime spike's program, [`crates/inset-host/tests/programs/spike/`](../../crates/inset-host/tests/programs/spike/), was first written by hand in Rust, in the shape the compiler would emit. It is now compiled from Inset like any other program. The decisions below come from building and measuring the spike and Hello HTTP. `cargo xtask e2e hello` builds Hello HTTP from its source, serves it with `inset run`, and asks it for a greeting over a real socket. `cargo xtask e2e stock-tracker` does the same for the Stock tracker: it stocks an item with a JSON body, ships some, lists low stock as JSON and fetches the front end's page. It then starts the program again, which finds what the first run stored.

## Pieces

| Piece | What it is |
| --- | --- |
| [`crates/inset-runtime/wit/inset.wit`](../../crates/inset-runtime/wit/inset.wit) | The interface between a program and the host: the `program` world |
| [`crates/inset-runtime`](../../crates/inset-runtime/) | The library every program links: components as actors, the executor, capabilities, failures |
| [`crates/inset-value`](../../crates/inset-value/) | The stored form of values, which the runtime and the test runner share (see [Tables](#tables)) |
| [`crates/inset-host`](../../crates/inset-host/) | The Wasmtime host: loads a program, resolves its settings and grants, serves its HTTP servers, keeps its tables in SQLite, or puts fakes behind its grants for tests |

A built program is one WebAssembly component, with one more for each external component it declares (see [External components](#external-components)). It exports `describe`, which returns its manifest of settings, grants and external components, `start`, and `handle`, which takes one HTTP request through a named `HttpServer` grant. It imports Inset's capabilities, an `externals` interface through which it sends messages to its external components, and a `report` interface, through which it tells the host about rollbacks and which parts of it have run (see [Coverage](#coverage)).

The host starts one instance of the program and keeps it for as long as the program runs. It calls `handle` for one request at a time, and `handle` returns once every component is idle again.

*Why one instance:* components keep state between requests, as the Stock tracker's `Reorders` does. `wasmtime serve` starts a fresh instance for every request, which would lose it.

*Why not `wasi:http`'s incoming handler:* it is built for an instance per request, it streams bodies that Inset reads whole anyway, and a component exports it once, while an Inset app may hold several `HttpServer` grants. Inset's own `handle` names the grant the request came through, and requirement tests call the same function with no socket in between.

## The emitted Rust

Each Inset item becomes Rust items in source order, each with its doc comment. Each Inset module becomes a Rust module of the same name, written inline in the one generated `lib.rs`, so `crate::`, `super::`, `use` and `pub` mean in the emitted Rust exactly what they mean in the source, and rustc's privacy rules agree with the checker's.

- **The app** becomes a type implementing `App`: its settings and grants as constants, which `describe` returns, and its `start` statements as `start`.
- **A component** becomes a struct of its fields, without the `#[handed_down]` ones, and a handle for each named child. It implements `Component`. The trait carries what the runtime needs to know: `save` and `restore` for the fields that hold state, `respond` for the `failure` block, `label` to name capability fields, `servers` and `route` for route handlers, and `Message`, `Messages` and `receive` for its messages. Handlers and algorithms stay as they were written, in an `impl` block of their own.
- **Starting a component** is an associated function, `start`, which takes a scope and every field in the order they are declared, starts the component's children, and builds the struct. `start` is an Inset keyword, so no function in a program has the name. The function takes the component's visibility, because whoever can name the component can start it.
- **`start` statements** first make a handle for each child, `scope.child::<Child>("name")`, and then start each one, `scope.start(&name, |child| Child::start(child, ...))`. Every handle exists before any child starts, so an outlet can be connected to a sibling started later. The handoffs are read in the parent's scope, where `send self.message` finds the parent through `scope.this::<Self>()`.
- **Messages** become an enum beside the component, with a variant for each `#[message]` function holding its arguments. The enum implements `Stored`. A handle derefs to a type with one method per message, which builds the variant and returns a `Message`. `.await` sends a `Message` and waits for the reply, and `send(...)` sends it without waiting. A lent parameter is copied into its variant with `ToOwned`, so the receiver's handler gets `&T` to a copy (see [Lending](#lending)). `receive` calls the handler for a variant and boxes its reply, and the sender's `Message` turns the reply back into the handler's return type.
- **An outlet field** is an `Outlet<T>`. Its connection, `field: send target.message`, becomes `Outlet::to(&target, |messages, value| messages.message(value))`, and `send self.field.emit(value)` delivers the value as `send` delivers a message.
- **A struct or an enum** is emitted as written, with its derives and its `#[serde]` attributes. It also implements `Stored`, which writes it in its stored form and reads it back (see [Tables](#tables)). An enum with `#[status(...)]` on its variants also implements `Status`, which replies with each variant's status: an empty body for a unit variant, and the `String` a variant holds as a text body. A route that returns `Result<String, E>` replies through it.
- **A component's fields** take the component's own visibility, because whoever can name the component can start it, and must supply its fields. The fields that hold state, which are those that hold no capability, are saved and restored in their stored form, so a field's type needs no `Clone`.
- **A `failure` block** becomes `respond`, with one arm for each of its arms, and makes the component save its state before each handler.
- **A `Table` grant** is opened with `grants.table("name")?` and moved into the component that holds it, which names the field for failure reports in `label`.
- **`Decimal`** is `rust_decimal`'s, which the runtime re-exports. `dec!(1.20)` is emitted as the number it is, `Decimal::from_i128_with_scale(120, 2)`, so the generated crate needs no macro crate of its own.
- **Routes** become `route`, which matches the request against each route attribute in order, converts the path parameters, and returns the handler to call. A parameter that does not convert means the route does not match, so the request gets `404 Not Found`, as `syntax.md` says. A query parameter or a body that does not convert gets `400 Bad Request`, and so does a missing query parameter that is not an `Option`.
- **JSON** is read and written by serde and `serde_json`, which the runtime re-exports. A data type deriving `Serialize` or `Deserialize` names the runtime's derives, `::inset_runtime::serde::Serialize`, with `#[serde(crate = "::inset_runtime::serde")]`, and keeps its own `#[serde]` attributes. `Json<T>` is the runtime's: a `Json<T>` body is read with `request.json()`, and a `Json<T>` reply is written with `application/json`. Serde's form never changes the stored form, which `Stored` writes.
- **The front end**'s files are copied into the generated project's `web/` and compiled into the component with `include_bytes!`, as the app's `FRONT_END`, with each file's content type from its extension. A `GET` that no route of the front end's server takes is answered from them (see [The front end](#the-front-end)).
- **Coverage** is marked as the code runs: a statement at the start of each function and branch, and `coverage::exit` around the value of each `?` (see [Coverage](#coverage)).
- `inset_runtime::export!(App)` exports the program.

*Why messages are values:* a closure cannot be written out, and a value can. When a panic traps the instance, the messages waiting in mailboxes go to the fresh instance with the components' state (see [Failures and rollback](#failures-and-rollback)).

*Why a handle derefs to the methods:* `Handle<C>` belongs to the runtime, so a program cannot add methods to it, and a trait of methods would need importing wherever a message is sent. Rust finds a method through `Deref` with no import, so `self.inventory.ship(&sku, quantity)` compiles as written. Each handler is called by its path, `Inventory::ship(self, ...)`, so a handler named like a method of `Component`, such as the Stock tracker's `receive`, is still the one called.

The compiler emits every construct the Stock tracker uses. It reports a child lent to a later sibling, `field: &child`, as not compiled yet, because no example needs one (see [`syntax.md`](syntax.md#starting-components)).

The emitted Rust names everything from the runtime by its full path, such as `::inset_runtime::Route`, rather than importing a prelude, so a component may have any name, `Route` included, without clashing with the runtime. It is printed by `prettyplease`, which drops comments, so unlike the hand-written fixture it does not repeat each Inset statement above what it became. Each piece is still recognisable from its source: a setting's name, type and default, a grant's fields, and `scope.start(...)` for each `start`.

`inset build <dir>` writes the generated Cargo project to `<dir>/target/inset/` and copies the component there as `<package>.wasm`, where the package name comes from the app's: `HelloHttp` is `hello-http`. The project depends on `inset-runtime` by path, at the copy the `inset` command was built from, until the runtime is published. It starts from Inset's own `Cargo.lock` and `rust-toolchain.toml`, so every program builds with the dependency versions and the toolchain that Inset's tests use. Rust's errors and warnings in generated code are compiler bugs, so `inset build` shows them only when the build fails, and the tests require every compiled example to build without either.

Release builds keep Rust's overflow checks, because Inset checks arithmetic in every build. The workspace's release profile sets `overflow-checks = true`, and so does the Cargo project the compiler generates.

## The executor

Each component is a task. It owns its fields, and takes one envelope at a time from its mailbox: a request routed to it, or a message from another component. The executor is a small single-threaded one written in `inset-runtime`, in safe Rust. It polls woken tasks in the order they were woken, until none is left.

*Why not Tokio:* a probe built with Tokio 1.54 for `wasm32-wasip2` found:

| Finding | Effect on Inset |
| --- | --- |
| The current-thread runtime works. The multi-thread runtime does not compile for `wasm32-wasip2` | Tokio would give one thread, which is all the target has anyway |
| `block_on` returns as soon as its own future completes, with spawned tasks still waiting to run. They carry on only in a later `block_on` | Tokio cannot run until idle, so a message sent while handling one request would be handled during the next |
| 100,000 messages to an actor through an unbounded channel took 4.6 ms. An awaited round trip took 1.25 µs | Fast enough, but not faster than the runtime's own executor, whose awaited round trip is 0.34 µs |

The runtime's executor runs until idle, which Tokio cannot do, and that gives Inset three things:

- **Causal order.** A message that one request leads to is handled before the next request arrives.
- **Repeatable tests.** Tasks run in the order they were woken, never by timing, so a requirement test runs the same way every time.
- **One place for rollback.** When a request returns, every handler it led to has completed or been rolled back.

Inset needs nothing else from an async runtime. All I/O is a call to the host, so there is no reactor to drive, and no timers until a clock capability exists. The emitted Rust never names the executor, so replacing it changes only `inset-runtime`.

*What it costs:* the host hands the program one request at a time, so one slow handler delays every other request. That suits locally hosted tools. WASI 0.3's async exports would let the host deliver concurrent requests to one instance, and Wasmtime supports them, so this can be revisited when a program needs it. A capability call is a synchronous call into the host, and holds up the whole program while it runs.

### Message order

`syntax.md` promises that an awaited message is in its receiver's mailbox before its sender carries on, and that a message sent with `send` is delivered when the sending handler completes, before its reply. The runtime keeps both promises:

- Awaiting a `Message` puts it in the receiver's mailbox before the sender's future is first suspended.
- `send` holds the message in the running handler's scope. When the handler completes, the runtime commits its table writes, delivers its held messages, and only then delivers its reply.

The runtime's tests check both, including that a handler which sends a message and then awaits another to the same receiver does not see the first one: it has not been delivered yet.

## Capabilities

A program can reach only what its imports reach, and the host decides what is behind each import.

| What the program tries | What happens |
| --- | --- |
| Open a table it was granted | The host opens it. Every write is held in a transaction until the handler that made it completes |
| Open a table by any other name | Refused, with the error that no table was granted by that name |
| Read a file or list a directory | Refused: no directory is opened for the program, so it finds no files at all (`No such file or directory`) |
| Connect to or listen on a network address | Refused: `Permission denied` |
| Read the environment | It is empty |
| Read the clock | It stands at zero, as both the wall clock and the monotonic clock |
| Draw random numbers | They come from a fixed seed |
| Import an interface the host does not provide, such as `wasi:http/outgoing-handler` | The program fails to instantiate, and the error names the import |

Rust's standard library on `wasm32-wasip2` imports WASI's files, sockets, clocks and random numbers whatever the program does, so the host links them, with nothing behind them. The tests in `crates/inset-host/tests/capabilities.rs` run a program that tries each of these.

*Why clocks and random numbers are fixed:* they are effects, and `syntax.md` leaves them out until they come as capabilities, so that a requirement test can fake them. Until then, nothing a program does depends on when or how often it runs. The fixed seed also seeds the hasher of every `HashMap`, which loses Rust's protection against inputs chosen to collide. That is acceptable for a locally hosted tool, and needs revisiting before a program serves untrusted clients.

`HttpServer` grants listen on the loopback interface only, at `127.0.0.1`.

## Tables

A `Table<K, V>` grant is a table in an SQLite database file. The host opens the file and the table, creating either if it does not exist, and the program knows the table only by its grant's name.

**The stored form.** The program gives the host each key and value already encoded, and the host stores them without looking inside. The form is defined once, in [`crates/inset-value`](../../crates/inset-value/), and used by both the runtime and `inset test`, which sets up and reads fake tables from requirement facts without running the program's code.

| Part | Stored as | Why |
| --- | --- | --- |
| A value | JSON text: a struct as an object of its fields, an enum's unit variant as its name and a tuple variant as `{"Name": [fields]}`, a `Decimal` as a string that keeps its scale, such as `"0.10"` | A person or another tool can read the table, and a `Decimal` keeps exactly what was stored |
| A key | Bytes in an order-preserving form: integers big-endian with a signed one's sign bit flipped, and a `String` that is the whole key as its UTF-8 bytes | Comparing two stored keys byte by byte gives Rust's order of the keys, so SQLite returns `entries` in key order |

The compiler emits the conversion for each struct and enum, as an implementation of the runtime's `Stored` trait. It never uses serde's derives, so a serde attribute that shapes a JSON body, such as `rename_all`, cannot change what a table holds.

Each table has two columns, `key BLOB PRIMARY KEY` and `value TEXT`. SQLite compares `BLOB`s byte by byte, so its rows are in key order.

**Opening a table.** As the program starts, the runtime reads every entry of each table it is granted, and refuses to start if a key or a value does not decode as the table's types. The host then names the file and the table, as in `cannot use the table "items" in /srv/stock.db for the grant "items: Table<String, Item>": the value stored under "bolt-m6" does not fit: missing field "place" of "Item"`. A table another program wrote, or an older version of this one, is found out at once instead of at the first request that reads the entry.

**Writes.** The host holds a handler's writes in memory, and reads see them, until the program commits them when the handler completes. A commit writes them in one SQLite transaction. A rollback discards them. A commit that SQLite refuses, such as on a full disk, stops the program, and the host reports the table and SQLite's error when the request returns.

**Fakes.** In a requirement test, each table is a map in memory with the same transactions. A `given` fact sets an entry or empties the table, a `then` fact reads the committed entries, and `name fails` makes every call the program makes to the table fail.

## Settings and grants

The host reads the manifest, applies each `--set name=value`, and rejects a setting the app does not declare or a value that does not fit its type, such as `--set port=65536` for a `u16`. It then resolves each grant's fields: a literal, or the value of a setting.

A relative file path in a grant is resolved against the directory `inset run` was run from, as any command-line tool resolves the paths it is given. So `inset run --set catalogue=prices.db` writes `prices.db` where the command was run, and the Stock tracker's `stock.db`, which is fixed in its source, lands in the directory it is run from. Requirement tests put a fake behind every grant, so they never touch a file.

## Failures and rollback

A failure is rolled back in one of two ways, depending on whether WebAssembly can carry on after it.

**The rollback rule.** A handler's effects take hold together when it completes, or not at all. Before a handler of a component with a `failure` block runs, the runtime saves the component's state fields in their stored form. When the handler completes, the runtime commits its table writes, delivers the messages it sent with `send`, and then delivers its reply. When it is rolled back, the runtime restores the saved fields, the host discards the table writes it was holding, and the messages are dropped. So no field, table entry or message changed by a rolled-back handler is seen by anything that runs after it.

**A failed capability call or message** does not panic. The call records the failure in the running handler's scope and never completes. The component's task then drops the handler's future, which is how Rust cancels async work, and applies the response.

**The response.** The component's own `failure` block responds first. A component that does not respond passes the failure to its parent as `Child(name)`, and so on up. A failure that reaches the app stops the program, and the host reports the component it happened in, the failure condition and its message.

- `rollback` follows the rule above, whichever component's block gives it. The caller's await fails with `Message(field)`, or the HTTP request gets `500 Internal Server Error`. The program carries on, and the host reports the rollback.
- `restart` rolls the handler back too, and then resets a component with all of its children. For a component's own arm, the component is the failed one. For a parent's `Child(name)` arm, it is the child. Each component is reset to the state fields it started with, before it takes its next envelope, so a component busy with a handler finishes the handler first. Messages already in its mailbox were sent by handlers that completed, so they stay and are delivered. The host reports which component was restarted, and the failure that caused the restart.

Only a component that may be rolled back pays for this, with a copy of its state fields before each handler. A component may be rolled back if it has a `rollback` arm, or if an ancestor rolls back its failures through `Child(name)`. A restart needs no copy, because each component keeps the state it started with.

**A panic or an overflow** cannot be caught. `wasm32-wasip2` aborts on a panic, because unwinding is not available on stable Rust there, and in the component model a trap leaves the instance unusable. So the runtime's panic hook, which runs before the trap, decides whether the program survives:

1. If every running handler either panicked or is awaiting one that did, and each one's failure is answered with `rollback` or `restart`, the hook writes the program's state in its stored form and hands it to the host. Each component's state is its fields from before the running handler, or the fields it started with if it is restarted. The messages waiting in each mailbox are written too. A waiting message whose sender awaits its reply cannot be handed on, so it stops the program.
2. The host rolls back every table's uncommitted writes, starts a fresh instance, and calls `start` with that state. The fresh instance restores each component by its diagram ID, and posts the messages that were waiting. The request gets `500 Internal Server Error`.
3. Otherwise the program stops, and the host reports where the panic happened.

An overflow is told apart from other panics by its message, such as `attempt to add with overflow`.

This costs nothing until a panic: a fresh instance starts in about 50 µs, plus the time to write and read the program's state.

### Left for later

- Each table commits in a transaction of its own, so a handler that writes two tables commits them one after the other. A crash between the two commits would keep the first table's writes and lose the second's. One transaction across every table a handler writes in the same database file would close the gap.
- Reading every entry of a table as the program starts costs time in proportion to the table. That suits the local tools Inset builds now, and a large table would want the check made once and recorded.

## Lending

A value lent across a boundary, `&x`, reaches the receiver as a copy. Measured per message, an awaited round trip between two components, with a `String` of each size moved to the receiver and back, or lent:

| Bytes | Moved | Lent | Cost of the copy |
| ---: | ---: | ---: | ---: |
| 0 | 0.34 µs | 0.36 µs | 0.02 µs |
| 16 | 0.35 µs | 0.38 µs | 0.03 µs |
| 1 KiB | 0.34 µs | 0.39 µs | 0.05 µs |
| 64 KiB | 0.34 µs | 1.3 µs | 0.95 µs |
| 1 MiB | 0.73 µs | 19.7 µs | 19 µs |

Up to a few kilobytes the copy costs less than the message itself. A copy of a megabyte costs about 20 µs, still small beside a request. Sharing one immutable copy for large values, which `syntax.md` allows, can wait until a program needs it.

## The front end

The program serves its front end itself, from the files compiled into it, through the `HttpServer` grant its `front end` names. The host does nothing new for it: a request reaches the program's `handle` as before. When no route of that server takes a `GET`, the runtime answers with the file at the request's path, or `index.html` for `/` and any path ending in `/`. Any other method, and a path with no file, still gets `404 Not Found`.

*Why the files are compiled in:* the built component is then the whole program, so `inset run`, a requirement test and a deployed copy serve the same page. The program reads no file at run time, and the host grants no file access for the front end.

*Why the content type of a request is not checked:* the host passes a request's method, path and body to the program, but not its headers. A `Json<T>` body is read as JSON whatever `Content-Type` it was sent with. Checking it comes with request headers, which wait for an example that needs them.

## External components

An `extern component` is code written in another language, built as a WebAssembly component of its own. The compiler and the host treat it in three parts, so that a second language needs only the middle one.

| Part | Done by | What it is |
| --- | --- | --- |
| The WIT world | The compiler, from the declaration alone ([`crates/inset-codegen/src/external.rs`](../../crates/inset-codegen/src/external.rs)) | `target/inset/externals/<package>/wit/world.wit`: one interface named after the component, holding the program's types its messages carry, as records, variants and enums, and a resource with a constructor and one method per message |
| The language adapter | Rust's, in [`external/rust.rs`](../../crates/inset-codegen/src/external/rust.rs) | Generates the language's bindings from the world and builds the component. For Rust: `wit-bindgen`'s bindings, wrapped in the `interface` crate's trait and `export!`, and the program's Rust file built as a `cdylib` for `wasm32-wasip2` |
| The boundary | The program and the host | The program sends a message through the `externals` import, with its arguments in their stored form. The host copies each into the component's own types, as the world describes them, and copies the reply back out |

Nothing outside the adapter knows the component's language, and no adapter may reach the program by any other way than the world. A Python adapter, through `componentize-py`, would build from the same world.

**The program's side.** The compiler emits an external component as an ordinary component, holding an `inset_runtime::External`, with one handler per message that hands the arguments to the host and awaits the reply. So it has a mailbox and takes one message at a time, it is sent messages and awaited like any component, and a failure in it is a failure of that component, which passes to its parent as `Child(name)`. Its handlers mark no coverage, because the diagram draws no flowchart for them.

**The host's side.** The program's manifest names its external components, and the host loads each from `externals/<Name>.wasm` beside the program's component, which `inset build` writes. It links nothing for one but WASI with nothing behind it, as for the program: no files, no network, fixed clocks and seeded random numbers. A component that imports anything else, such as the program's own capabilities, is refused as the program loads. The host runs one instance for each place the program starts the component, keyed by its diagram ID, such as `PriceImport/Importer/csv`. It makes the instance, and calls the constructor, with the first message.

- A trap in the instance, such as a Rust panic, fails the message. The host takes the panic's message and location from what the instance wrote to its standard error, and the program reports them as a `Panic`, or an `Overflow`. The instance is then dropped, so the next message starts it afresh.
- A restart of the component, or of any component above it, drops its instance too.
- Each start of the program, such as each requirement in `inset test`, starts with no instances. A fresh instance of the program after a panic keeps them.

**Values.** The host converts between the stored form and WIT's values by matching names. A record's fields are the struct's in kebab case, a variant's cases the enum's, and a `None` or a `Result` take their stored forms. The checker refuses a name that would not come back the same from WIT, so the conversion needs no table of names. Values are copied both ways, so the external component shares no memory with the program.

*Why the host converts, rather than the program:* the program then imports one fixed interface whatever external components it has, and every external component exports a world of its own declaration's shape. The host can read any world's types at run time, so it needs no code generated for each program.

**Building.** `inset build` writes every external component of a program into one Cargo workspace, `target/inset/externals/`, and builds each before the program. The workspace's lock file is the program's `inset.lock`, beside `src/`. The first build starts it from Inset's own `Cargo.lock`, so `wit-bindgen` and any crate Inset also uses come at the versions Inset is tested with, and writes the result back. Later builds start from `inset.lock`, so a version it holds is used even after a newer one is published. A Rust file that does not build is shown with its declaration, then rustc's own errors.

## Coverage

The emitted code counts its own coverage. At the start of each function and of each branch, the compiler adds a statement that marks the element reached, by its index in a list of the diagram IDs coverage counts. A `?` passes its value through `coverage::exit`, which marks the error path when the value is an `Err` or `None` and returns it unchanged. A loop is wrapped in a block with a flag that its body sets, so the end of the loop can tell whether the body ran at all. The runtime marks each `failure` arm itself, when it uses the arm's response.

A component's list holds IDs relative to a running component, such as `fn:ship/1/b0`, in `Component::BRANCHES`, and each running component keeps a flag for each, because a component type started twice runs as two components with IDs of their own. The top-level algorithms' elements have full IDs, in `App::BRANCHES`. A mark is a load and a store, made only in the running handler's own component: a component's function that runs while another component's handler is running marks nothing.

When a request returns, when the program has started, and in the panic hook before the instance traps, the runtime reports what has been reached since its last report, through `report.reached`, by full diagram ID. So each element is reported once per instance, and a long-running program soon reports nothing. The host keeps the set, and carries it into the fresh instance it starts after a panic. `inset test` reads it after each case, and credits each element to the requirement whose case reached it.

*Why instrument the Rust rather than trace inside Wasmtime:* the compiler knows which Rust it emitted for each branch of the flowchart, and Wasmtime does not. Tracing would need a map from WebAssembly code back through rustc's optimisations to Inset's branches, which no tool keeps. Marking the branches in the source makes the IDs exact, and a test checks that the compiler marks exactly the elements the diagram draws, by the same IDs.

*Why always, and not only in tests:* the program `inset test` runs is the program `inset run` serves, so a test never runs a different build from the one deployed. A mark costs a load and a store, and reports stop once everything that runs has run.

*Why a flag, not a count:* coverage asks only whether a requirement reached an element. A count would cost the same, and would say how often a test happened to run a loop, which no review asks.

## Starting a program

Measured for Hello HTTP (a 280 KB component), with the host built in release mode:

| Step | Time |
| --- | ---: |
| Load, compiling the component | 25 ms |
| Load, from Wasmtime's cache of compiled code | 1.6 ms |
| Start: instantiate, and start the components | 0.05 ms |
| First request | 0.008 ms |
| Each later request | 0.0015 ms |

A host built in debug mode, as `cargo test` builds it, takes 325 ms to compile, 12 ms to load from the cache and 0.3 ms to start. Either way a requirement test can start the program from scratch: a fresh instance for each requirement costs well under a millisecond once the program is loaded.

The measurements were taken on an AMD Ryzen 7 5800X. They are reproduced with:

```sh
cargo test --release -p inset-host --test measure -- --ignored --nocapture --test-threads 1
```
