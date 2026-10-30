# Hello HTTP

A web server with one address. A request for `/hello/Ada` gets the text reply `Hello, Ada!`.

## Files

| File | Contents |
| --- | --- |
| `src/hello.inset` | The app, which receives the network capability from the host, and the `Greeter` component it starts |
| `src/hello.req.inset` | The one requirement test |

## What it exercises

| Construct | Where |
| --- | --- |
| App and host grant | `grant http: HttpServer { port: 8080 }` asks the host for an HTTP server on port 8080 |
| Capability hand-down | `start Greeter { http: move http }` moves the capability into the component |
| Component | `Greeter`, whose fields list the only capability it holds |
| Route | `#[get("/hello/{name}", via = http)]` on `fn hello`, a handler for requests arriving through `http` |
| Reply | The function's value, `format!("Hello, {name}!")`, sent as `200 OK` text |
| Requirement test | One `when` / `then` test at the HTTP boundary of `Greeter` |

It covers the brief's Hello HTTP row: one component, one route, one requirement test, and the network capability. It has no `given` step because `Greeter` holds no state.

## Expected diagram

A sketch of what the component view should show. Stage 3 replaces it with `expected/diagram.json` and a rendered SVG.

```
                     ┌─ HelloHttp ────────────────────────────────┐
                     │                                            │
                     │  grant http: HttpServer (port 8080)        │
                     │        │                                   │
                     │        │ move                              │
                     │        ▼                                   │
 ( HTTP clients ) ───┼── GET /hello/{name} ──▶ ┌───────────┐      │
                     │        via http         │  Greeter  │      │
                  ◀──┼── String ───────────────┤           │      │
                     │                         └───────────┘      │
                     └────────────────────────────────────────────┘
```

- The grant is the only place a capability enters the program, so the reviewer sees every external access at the app boundary.
- The request arrow is labelled with the route, and the reply arrow with its type.
- The requirement test covers the route and its reply, so the coverage overlay should leave nothing uncovered.
