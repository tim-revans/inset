# Inset diagrams

What the generated diagrams show, the data they are drawn from, and how a reader chooses what to see.

The diagram data format is defined, and `crates/inset-diagram` reads it and draws it as SVG. `inset diagram` emits it for the programs the compiler covers. Each example's `expected/diagram.json` is written by hand, and is the target the compiler's output must equal. The SVGs beside it are drawn from it, and the tests keep them up to date.

## Review levels

A reviewer starts at the outside and drills in:

| Level | Shows | Drawn with |
| --- | --- | --- |
| Component view | The app, what it is granted, the components it starts, and what is outside: HTTP clients on the left, stored data and the services the program calls on the right | `inset diagram <dir>` |
| Inside a component | The same picture with one component expanded, its children drawn inside it, and the messages between them | `--expand <component id>` |
| Flowchart | One function's body: its steps, decisions, loops and forks | `--flowchart <function id>` |
| Coverage | The same pictures, with what no requirement reached marked | `--coverage`, after `inset test --coverage` (see [Coverage](#coverage)) |

A component with children is drawn collapsed unless the view expands it, with `[+]` and the names of its children. A collapsed component still shows its own routes, messages and failure section. An arrow that ends inside a collapsed component ends at its box, and an arrow with both ends inside it is not drawn. So collapsing hides how a component is built, never what it does at its edge.

*Why collapsed by default:* the first question a reviewer asks is what the program touches and what it serves. The component view answers it in one screen, whatever the size of the program. Nesting is where the detail is, and the reviewer opens it where they want to look.

## Style

Inset's diagrams borrow from three traditions and conform to none of them.

- **From C4:** levels of zoom. The component view is a context and container view in one, with the app as the system boundary and the people and systems outside it drawn outside it. Expanding a component is C4's step down a level, with no fixed number of levels.
- **From hardware schematics:** boxes nested like modules, ports on a box's edge for outlets, and wiring decided by the box around them. Arrows run in lanes beside the boxes rather than across them, as wires do on a schematic.
- **From UML activity diagrams:** the flowchart: a pill for the start and for each return, a box for each action, a six-sided box for each decision and loop head, and a bar for each fork and join.

And they leave things out:

- No UML class diagrams. Data types are shown only as the types on the labels.
- No sequence diagrams. Messages are arrows on the component view, and their order is in the flowcharts.
- No icons or shapes that need a legend. A box is a component, a double box an external component, a dashed pill something outside the program. The coverage marks are the one exception, and a picture that shows them carries their key.
- No layout chosen by hand. Boxes are placed in source order, so the same source always gives the same picture.

The drawing conventions:

| Element | Drawn as |
| --- | --- |
| App | The outer box, listing its settings, its grants and the top-level algorithms |
| Component | A box inside its parent, headed `name: Type`, or `Type` when `start` binds no name |
| External component | A double box, with the language, file and crates it comes from, and `holds no capabilities` when it holds none |
| Outside the program | A dashed pill: clients on the left, stored data on the right |
| Front end | A dashed box labelled `front end web/`, on the left below the clients, with a thin arrow to the row of each route it calls. An arrow to a route of a collapsed component ends at that component's box |
| Capability edge | An orange line between the component that holds the capability and what is outside, with the operations the component calls under it |
| Message that waits | A solid arrow from the sending function's row to the message's row |
| `send`, and an outlet's connection | A dashed arrow |
| Outlet | A port `○` on the left edge of its component's box, at its row |
| Handoffs | The `start { ... }` row of each component, as written in its parent |

Rows inside a box are grouped under small headings: `fields`, `routes`, `messages`, `algorithms` and `failure`. A capability that a component uses is shown as its edge, not as a field row. A field that is only handed down to a child is a row marked `handed down`.

### Colour

By default a picture carries both palettes and follows the reader's setting: light when the reader's system asks for light, and dark otherwise, including in renderers that ignore the setting. `--theme dark` or `--theme light` fixes one palette, for a picture that must look the same everywhere, such as one pasted into a document. `--transparent` leaves the background off, so a picture sits on whatever page holds it. Every role has one colour in each palette:

| Role | Used for | Dark | Light |
| --- | --- | --- | --- |
| Component | Component boxes, outlet ports, keywords | `#5797f8` | `#196df7` |
| Capability | Capability edges and what is outside | `#eba566` | `#e2760f` |
| External component | The double box | `#e057c5` | `#a41faa` |
| Failure | Failure arms and their responses | `#f64234` | `#9c200b` |
| Ink | Text, message arrows and flowchart outlines | `#e7ecf0` | `#182029` |
| Background | Behind everything, unless `--transparent` | `#0c1015` | `#fdfdfd` |

Each colour was chosen by measurement against its palette's background. Every pair of role colours stays apart under simulated protanopia and deuteranopia, and with normal vision. Every role colour has at least 3:1 contrast with the background, and every text colour at least 4.5:1 on every box it sits on. Colour only repeats what the shape already says: an external component keeps its double box, `send` keeps its dashed line, and a failure arm keeps its text. So a reader who cannot tell two colours apart loses nothing. The full set of values is in `crates/inset-diagram/src/theme.rs`.

Coverage has three colours of its own, measured in the same way against the roles above and each other, and carried only by a picture that shows coverage:

| Coverage | Dark | Light |
| --- | --- | --- |
| Covered: a requirement reached it | `#2fd4b4` | `#0b7d74` |
| Uncovered: no requirement reached it | `#e8e337` | `#8f6b09` |
| Excused by `#[defensive]` | `#7f8a7a` | `#4a4858` |

*Why follow the reader, falling back to dark:* developers mostly work in dark editors, and requirements reviewers often prefer light pages, so each reader gets the palette their system already asks for. Where a renderer cannot tell, dark is the safer guess for the developers who read most of the pictures.

An SVG shown as an image usually follows the reader's operating system or browser setting rather than a website's own theme switch, so on GitHub the committed pictures may not change with GitHub's own light or dark choice.

## Coverage

`inset test --coverage` records which requirements reached each element that coverage counts (see [`syntax.md`](syntax.md#requirements-and-coverage)): entering a function, each branch, each error path of a `?`, and each arm of a `failure` block. `inset diagram --coverage` draws any view with it, and the editor's diagram panel has a **Coverage** box that does the same with the last run of the requirements.

| Status | Means | Drawn as |
| --- | --- | --- |
| Covered | A requirement reached it | In the covered colour |
| Uncovered | No requirement reached it | In the uncovered colour, thick, and a row's band shaded |
| Excused | Marked `#[defensive]`, or inside a branch that is, and not reached | In the excused colour, dotted, with the reason on the branch's label |
| Defensive, yet reached | Marked `#[defensive]`, but a requirement reached it, which fails `inset test` | As uncovered, and dotted |

On a flowchart, each branch's edge and label, each `?` error path with its `return`, and the function's start take the colour of their status. On the component view, each row that holds something coverage counts gets a mark in the box's left margin: `✓` covered, `✗` uncovered, `~` excused, and `!` defensive yet reached. A function's row takes the worst of everything in the function, so a function that runs but has a branch no requirement reached is marked `✗`. A component's heading takes the worst of everything in it and in its children, so a collapsed component that hides uncovered code is marked before the reader opens it. Rows marked `✗` or `!` are shaded across the box. A key to the marks is written under the picture.

*Why the worst, not a count:* the reviewer's question at the component level is where to look, and one uncovered branch is enough reason to look. A count needs room the layout does not have.

*Why the marks take no room:* the marks sit in the margin and the colours change only strokes and fills, so turning coverage on in a viewer moves nothing but the key below the picture, and the reader keeps their place.

The coverage is written to `<dir>/target/inset/coverage.json`, as the `coverage` field below. Coverage that names an element the diagram no longer has is from an older version of the program, and `inset diagram --coverage` refuses it rather than draw a stale picture.

## Side by side

Which requests can run at the same time follows from what each route holds: the component that serves it and every component it can reach (see [`runtime.md`](runtime.md#concurrent-requests)). Two requests run side by side when their routes hold no component in common, and otherwise take turns. It is shown in two places.

- **Always, as a table.** `inset test` ends with a table of every pair of routes, `yes` where requests to the two can run at the same time and `no` where one waits for the other, for a program with two routes or more. The JSON report gives each route with the components it holds, as `routes`. Pinned in a snapshot, the table makes a change that turns `yes` into `no` show up in review.
- **On request, in a viewer.** The editor's diagram panel has a **Side by side** box. With it ticked, clicking a route picks it: the components it holds are shaded, every other route is outlined in the covered colour with `alongside` if it can run meanwhile, or dashed in the uncovered colour with `waits` if it takes turns with it, and a line above the picture names them. The panel gets each route's components from the language server with the picture, so another viewer can do the same.

*Why the table is always shown and the picture is not:* the table answers every pair at once, in text that a terminal, a test report and a snapshot all hold, so nothing has to be asked for. It says what, but not why. The picture answers why, by showing the component two routes share, but only for the route picked, which suits a viewer where picking is a click. `inset diagram` has no such view, so a picture never depends on which route a reader thought to ask about.

## Diagram data and views

The compiler emits diagram data that holds everything the source says about the program's structure. A view is drawn from that data, and only the view chooses what to show.

- Diagram data is complete and deterministic. It never leaves anything out to make a picture simpler.
- `inset diagram` draws a view as SVG from a program's source, and `inset diagram --json` prints the program's diagram data. Its options choose what the view shows. With no options it shows every label below. `inset-diagram render` draws a view of hand-written data with the same options.
- An interactive viewer offers the same choices as toggles, and draws from the same data. A static picture and an interactive one with the same choices are the same picture.
- Hiding something never changes the layout's meaning: a hidden label is left off, and nothing is merged or moved to stand for it. The layout is measured with every label in place, so hiding one moves nothing else, and a test checks that each option only removes what it names.

*Why the data holds everything and views choose:* most reviewers will use an interactive viewer, and different reviews need different detail. Someone checking what a program can touch wants every capability's type. Someone checking the flow of requests wants only names. If the generator decided, one of them would be shown less than the source says. Keeping every fact in the data means a view can only leave things out, never get them wrong.

*Why hiding moves nothing:* when a reviewer toggles a label in a viewer, the picture must not jump about, or they lose their place. It also makes a picture with fewer labels easy to trust: it is the full picture, less exactly those labels.

## Labels

| Element | Shown by default | Hidden with `--hide` |
| --- | --- | --- |
| Setting, in the app box | `setting port: u16 = 8080` | `setting-types` |
| Grant, in the app box | The whole declaration, with where each value comes from: `grant items: Table<String, Item> { file: "stock.db", name: "items" }`, or `{ port: setting port }` | `grant-types`, `grant-values` |
| Capability edge | The field's name and its type: `items: Table<String, Item>` | `capability-names`, `capability-types` |
| Capability operations | The methods called, under the edge: `get, insert` | `capability-operations` |
| External end of a capability edge | What is outside: `stock.db`, `HTTP clients`, the address an `HttpClient` calls, such as `http://127.0.0.1:9000`, or `catalogue.db or setting catalogue` when a setting chooses it | No |
| Handoffs | `start { items: move items, low_stock: send reorders.raise }` | `handoffs` |
| Field | `orders: Vec<Reorder>` | `field-types` |
| Outlet | A port on the component's box, with its name and type: `low_stock: Outlet<Reorder>` | `outlet-types` |
| Route, message or algorithm | `GET /items/{sku}  stock(&self, sku: String) -> Option<String>` | `signatures` |
| Message arrow | The message and its boundary notation: `ship(&sku, quantity)`, `send reorders.raise` | `message-params` |
| Doc comments | As the description of the element they document: under a box's heading, or as the tooltip of a row | `docs` |
| Failure section | Each arm and its reason | `failure-reasons`, or `failures` for the whole section |

*Why the default shows both name and type:* the name is what the code and the requirements use, as in `given items[...]`, so the reviewer can match a step to an edge. The type says what kind of access it is. A reviewer who wants a lighter picture can hide either.

## Flowcharts

Every function with a body has a flowchart: route handlers and message handlers as well as algorithms, because coverage counts branches in all of them. A step is labelled with its source text, without its `;`, because that is what the compiler knows to be true. The doc comment above the function says what it is for.

| Step | Source | Drawn as |
| --- | --- | --- |
| Action | A statement that runs and carries on | A box |
| Exit | The error path of a `?` | A `?` arrow from the action to a `return` pill beside it |
| Decision | `if`, `if let`, `match`, `let ... else` | A six-sided box, with one labelled branch per way out, side by side. Branches that carry on join below. An `if` branches `yes` and `no`, a `match` once per arm, labelled with the arm's pattern and any guard, and a `let ... else` with its pattern and `else` |
| Loop | `for`, `while`, `while let` | A six-sided head, the body below with the way back on the left, and `none` on the right for not running the body. The body is labelled `each` for a `for` and `repeat` for a `while` |
| Fork | `fork for` | A bar, the body below, a `join` bar, and `none` on the right for an empty collection |
| Return | `return`, or the function's final value | A pill. A function that returns nothing ends in `end` |

An iterator chain such as `map(...).collect()` is one step, because its closure cannot send, await or call a capability. A call to another function is one step too: the reviewer opens that function's flowchart to see inside. An awaited capability call, such as `self.items.get(&sku).await`, is a step linked to the capability's edge, so a viewer can lead from the step to what it touches.

Every branch is an element with its own ID: each branch of a decision, a loop's body and its skip, and each exit. These are exactly what coverage counts, so the overlay can mark the edge that a reviewer sees.

## The data format

`docs/language/diagram.schema.json` is the JSON Schema of the format, generated from the Rust model in `crates/inset-diagram`. Its descriptions are the field reference. In outline:

- `version` is 1.
- `app` holds the settings, the grants with what is outside each one, the `front_end` with the server that serves it and the handler of each route it calls, and the tree of `components`. Each component lists its `start` handoffs, `fields`, `failure` arms, `routes`, `messages`, `algorithms` and child `components`, all in source order.
- `algorithms` holds the functions declared at the top level of a module.
- `edges` holds what connects elements across the tree: `capability` (a field to its grant, with the operations used), `message` (a function to a message handler, with each argument's boundary notation, and whether it is a `send`) and `emit` (a function to its own component's outlet). An outlet's connection is a `connect` handoff in the `start` of the component that owns the outlet. The compiler lists the capability edges first, then the message edges, then the emit edges.
- Each function's `body` is its flowchart, as nested `action`, `decision`, `loop`, `fork` and `return` steps.
- `coverage`, when present, maps each element ID reached to the sentences of the requirements that reached it, in the order of the requirements. `inset diagram --json --coverage` fills it in from the last `inset test --coverage`.

A list or optional field that is empty is left out of the JSON. Data is written in source order, so the same source always gives the same file.

A component appears once for each `start` statement that starts it, so a component type started twice appears twice, each with its own ID and its own copy of its functions. The diagram shows running components, and `definition` names the type's Rust path in the source.

### IDs

Every element has an ID derived from where it is in the source, so an ID stays the same while the code around it changes, and a viewer can go from an element to its source.

| Element | ID | Example |
| --- | --- | --- |
| App | Its name | `StockTracker` |
| Setting, grant | The app, then `setting:` or `grant:` and the name | `StockTracker/grant:items` |
| Component | Its parent, then the name its `start` binds, or its type's name | `StockTracker/Warehouse/inventory` |
| Field | The component, then `field:` and the name | `…/inventory/field:low_stock` |
| Function | Its owner, then `fn:` and the name. A top-level algorithm's owner is the app, followed by `mod:<path>` outside the root module, with `::` between the names of nested modules | `…/inventory/fn:ship`, `PriceImport/fn:parse_row` |
| Failure arm | The component, then `failure:` and `panic`, `overflow`, `any`, or `capability:`, `message:` or `child:` and the name | `…/inventory/failure:capability:items` |
| Step | Its block, then its index in the block. A function's body is the block with the function's ID | `…/fn:ship/5` |
| Branch | Its step, then `b0`, `b1` and so on for a decision or fork, or `body` and `skip` for a loop | `…/fn:ship/5/b0` |
| Exit | Its step, then `err` | `…/fn:stock/0/err` |
| Edge | Its two ends, `<from>-><to>`, with `#2` and so on added if the same two ends are joined again | `…/fn:ship->…/inventory/field:low_stock` |

The `kind:` prefixes keep a field, a function and a child with the same name apart, as Rust allows. The JSON Schema checks the shape of a file, and `inset-diagram check` also checks that every ID follows these rules, that none is used twice, and that every reference names an element of the right kind.

In a rendered SVG, each element that has an ID carries it in a `data-id` attribute. The VS Code extension reads this attribute to open the source of a clicked element.

*Why IDs follow the source and not the picture:* the coverage overlay, a requirement report and an editor all need to name the same element. The source is the one thing they share, and a path through it is readable in a test failure.
