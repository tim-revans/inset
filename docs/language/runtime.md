# Inset runtime

How a built Inset program runs: the Rust the compiler emits, the runtime library linked into it, and the WebAssembly host that runs it with only the capabilities it is granted.

The compiler does not exist yet, so the emitted Rust is written by hand. [`crates/inset-host/tests/fixtures/hello-http/`](../../crates/inset-host/tests/fixtures/hello-http/) is what the compiler must emit for Hello HTTP. It sits with the host's tests rather than in the example, because nobody reviewing a program reads its Rust; once the compiler exists, a snapshot test of its output replaces it. [`crates/inset-host/tests/fixtures/spike/`](../../crates/inset-host/tests/fixtures/spike/) is a larger program in the same shape, with messages, a table and every kind of rollback. The decisions below come from building and measuring both. `cargo xtask e2e hello-spike` builds Hello HTTP, serves it with `inset-host run`, and asks it for a greeting over a real socket.

## Pieces

| Piece | What it is |
| --- | --- |
| [`crates/inset-runtime/wit/inset.wit`](../../crates/inset-runtime/wit/inset.wit) | The interface between a program and the host: the `program` world |
| [`crates/inset-runtime`](../../crates/inset-runtime/) | The library every program links: components as actors, the executor, capabilities, failures |
| [`crates/inset-host`](../../crates/inset-host/) | The Wasmtime host: loads a program, resolves its settings and grants, serves its HTTP servers, or puts fakes behind its grants for tests |

A built program is one WebAssembly component. It exports `describe`, which returns its manifest of settings and grants, `start`, and `handle`, which takes one HTTP request through a named `HttpServer` grant. It imports Inset's capabilities and a `report` interface, through which it tells the host about rollbacks.

The host starts one instance of the program and keeps it for as long as the program runs. It calls `handle` for one request at a time, and `handle` returns once every component is idle again.

*Why one instance:* components keep state between requests, as the Stock tracker's `Reorders` does. `wasmtime serve` starts a fresh instance for every request, which would lose it.

*Why not `wasi:http`'s incoming handler:* it is built for an instance per request, it streams bodies that Inset reads whole anyway, and a component exports it once, while an Inset app may hold several `HttpServer` grants. Inset's own `handle` names the grant the request came through, and requirement tests call the same function with no socket in between.

## The emitted Rust

Each Inset item becomes one Rust item, in source order, with its doc comment. Each Inset statement that is not plain Rust is written above what it became, as a comment.

- **The app** becomes a type implementing `App`: its settings and grants as constants, which `describe` returns, and its `start` statements as `start`.
- **A component** becomes a struct of its fields, implementing `Component`. The trait carries what the runtime needs to know: `save` and `restore` for the fields that hold state, `respond` for the `failure` block, `label` to name capability fields, and `servers` and `route` for route handlers. Handlers and algorithms stay as they were written, in an `impl` block of their own.
- **A child** is started with `scope.start_as("name", |scope| Child { ... })`, which returns its `Handle`. The closure gets the child's scope, for the child's own children.
- **Messages** become one method each on a trait implemented for `Handle<C>`. A method returns a `Message`, which `.await` sends and waits for, and `send(...)` sends without waiting. A lent parameter is cloned in that method, so the receiver's handler gets `&T` to a copy (see [Lending](#lending)).
- **Routes** become `route`, which matches the request against each route attribute in order, converts the path parameters, and returns the handler to call. A parameter that does not convert means the route does not match, so the request gets `404 Not Found`, as `syntax.md` says.
- `inset_runtime::export!(App)` exports the program.

Release builds keep Rust's overflow checks, because Inset checks arithmetic in every build. The workspace's release profile sets `overflow-checks = true`, and so will the Cargo project the compiler generates.

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

## Settings and grants

The host reads the manifest, applies each `--set name=value`, and rejects a setting the app does not declare or a value that does not fit its type, such as `--set port=65536` for a `u16`. It then resolves each grant's fields: a literal, or the value of a setting.

A relative file path in a grant is resolved against the directory the host was started in, as any command-line tool resolves the paths it is given. So `inset run --set catalogue=prices.db` writes `prices.db` where the command was run, and the Stock tracker's `stock.db`, which is fixed in its source, lands in the directory it is run from. Requirement tests put a fake behind every grant, so they never touch a file.

## Failures and rollback

A failure is rolled back in one of two ways, depending on whether WebAssembly can carry on after it.

**A failed capability call or message** does not panic. The call records the failure in the running handler's scope and never completes. The component's task then drops the handler's future, which is how Rust cancels async work, and applies the component's response:

- `rollback` restores the fields from the copy `save` made before the handler started, rolls back the transaction of every table it wrote, and drops the messages it sent. The caller's await fails with `Message(field)`, or the HTTP request gets `500 Internal Server Error`. The program carries on, and the host reports the rollback.
- With no response, the program stops, and the host reports the component, the failure condition and its message.

Only a component with a `failure` block pays for this, with a copy of its state fields before each handler.

**A panic or an overflow** cannot be caught. `wasm32-wasip2` aborts on a panic, because unwinding is not available on stable Rust there, and in the component model a trap leaves the instance unusable. So the runtime's panic hook, which runs before the trap, decides whether the program survives:

1. If no message is waiting in any mailbox, and every running handler either panicked or is awaiting one that did, and each responds to its failure with `rollback`, the hook writes every component's state as JSON, using the copy from before the handler for each running one, and hands it to the host.
2. The host rolls back every table's uncommitted writes, starts a fresh instance, and calls `start` with that state, which restores each component by its diagram ID. The request gets `500 Internal Server Error`.
3. Otherwise the program stops, and the host reports where the panic happened.

An overflow is told apart from other panics by its message, such as `attempt to add with overflow`.

This costs nothing until a panic: a fresh instance starts in about 50 µs, plus the time to write and read the program's state.

### Left for later

- A message waiting in a mailbox when a panic happens stops the program, because messages are closures and cannot be written out. Emitting each component's messages as an enum would let the state carry them.
- The runtime writes component state with serde's JSON. The compiler will derive the serde traits for each component's state, which Inset source never sees, and a table's keys need an encoding that keeps their order, for `entries`.
- `restart`, and the conditions `Child(name)` and `_`, are not implemented yet.

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
