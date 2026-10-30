# Stock tracker

Tracks how many of each item a warehouse holds. Staff record stock that arrives and stock that is shipped, over HTTP. When shipping leaves an item at or below its reorder level, the tracker raises a reorder that brings it back up to its target level.

| Request | Reply |
| --- | --- |
| `GET /items/{sku}` | The quantity on hand, or `404 Not Found` |
| `POST /items/{sku}/receipts`, body a quantity | The new quantity on hand, or `404 Not Found` |
| `POST /items/{sku}/shipments`, body a quantity | The new quantity on hand, `404 Not Found`, or `409 Conflict` if there is not enough stock |
| `GET /reorders` | One line per reorder waiting to be placed: the SKU, then the quantity |

## Files

| File | Contents |
| --- | --- |
| `src/stock.inset` | The app, which is granted the network and the stock table, and the data types |
| `src/warehouse.inset` | `Warehouse` and the two components nested in it, `Inventory` and `Reorders` |
| `src/stock.req.inset` | The requirement tests |

The program is split across two files to show that all the files of a program share one namespace.

## What it exercises

| Construct | Where |
| --- | --- |
| Database capability | `grant items: Table<String, Item> { ... }` asks the host for a stored table |
| Several actors | `Warehouse`, `Inventory` and `Reorders` each run as a component |
| Nested components | `Warehouse` starts `Inventory` and `Reorders` with `start` statements |
| Capability hand-down | The app moves `items` into `Warehouse`, which moves it on into `Inventory`. Only `Inventory` uses it |
| Messages that wait for a reply | `self.inventory.ship(&sku, quantity).await` |
| A message that does not wait | `send self.reorders.raise(...)` |
| Boundary notation | `move` for capabilities, `&sku` for a lent string, a bare `quantity` for a copied number, and `Reorder { ... }` for a value built in place |
| Component state | `Reorders` holds its list of reorders in a field and changes it in `&mut self` handlers |
| Request bodies | `body = quantity` reads the request body as a `u32` |
| Error replies | `Option` replies `404 Not Found` for `None`. `ShipError` names a status on each variant |
| Algorithm in a non-leaf component | `Warehouse::reorder_quantity`, which takes no `self` and so touches no capability |
| Table test | The last requirement, one row per case of `reorder_quantity` |
| Capability facts in tests | `given items["bolt-m6"] is Item { ... }` and `and items is empty` |
| Message-level test | `when Inventory receives ship("bolt-m6", 5)` |

It covers the brief's Stock tracker row: several actors, a database capability, an algorithm flowchart, and table tests.

Every branch of the code is reached by some requirement, so the coverage overlay should leave nothing uncovered.

## Expected diagrams

Sketches of what each review level should show. Stage 3 replaces them with `expected/diagram.json` and rendered SVGs. Solid arrows are messages that wait for a reply. Dashed arrows are messages sent without waiting.

**Component view.** `Warehouse` is collapsed, so the `items` edge leaves from it.

```
                     ┌─ StockTracker ──────────────────────────────────────┐
                     │                                                     │
                     │  grant http: HttpServer (port 8080)                 │
                     │  grant items: Table<String, Item> (stock.db)        │
                     │        │                                            │
                     │        │ move http, move items                      │
                     │        ▼                                            │
 ( HTTP clients ) ───┼── GET  /items/{sku} ──────────▶ ┌──────────────┐    │
                     │   POST /items/{sku}/receipts    │  Warehouse   │    │
                     │   POST /items/{sku}/shipments   │     [+]      │ ───┼── items ──▶ ( stock.db )
                     │   GET  /reorders                └──────────────┘    │
                     │        via http                                     │
                     └─────────────────────────────────────────────────────┘
```

**Inside `Warehouse`.** The reviewer expands it to see its children.

```
 ┌─ Warehouse ───────────────────────────────────────────────────────────────┐
 │                                                                           │
 │  routes (via http)                       items ── move ──┐                │
 │     │                                                    ▼                │
 │     ├── item(&sku) ──────────────────────▶ ┌─────────────────┐   items    │
 │     ├── receive(&sku, quantity) ─────────▶ │    Inventory    │ ───────────┼──▶ ( stock.db )
 │     ├── ship(&sku, quantity) ────────────▶ └─────────────────┘            │
 │     │                                                                     │
 │     ├╌╌ send raise(Reorder) ╌╌╌╌╌╌╌╌╌╌╌╌╌▶ ┌─────────────────┐            │
 │     └── pending() ───────────────────────▶ │    Reorders     │            │
 │                                            └─────────────────┘            │
 │  algorithm: reorder_quantity                                              │
 └───────────────────────────────────────────────────────────────────────────┘
```

**Flowchart of `reorder_quantity`.**

```
          ( on_hand, reorder_level, target_level )
                            │
                            ▼
              ◇ on_hand > reorder_level ? ◇
                 │ yes                │ no
                 ▼                    ▼
              [ 0 ]     [ target_level − on_hand, at least 0 ]
                 └─────────┬──────────┘
                           ▼
                    ( quantity to order )
```

- `items` enters at the app, passes through `Warehouse` without being used, and is used only by `Inventory`. The reviewer sees exactly one component touching the database.
- Messages only go from a parent to its children, so every arrow inside `Warehouse` points away from the routes.
- The table test covers both branches of the flowchart.
