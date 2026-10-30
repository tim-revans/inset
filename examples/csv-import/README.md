# CSV import

Stores a supplier's price list in a product catalogue. A client posts the price list as CSV to `POST /price-lists`. Each row gives a SKU, a product name and a net price, and the importer adds VAT before storing the product. If any row is invalid, nothing is stored, and the reply lists every invalid line.

```
sku,name,net
bolt-m6,M6 bolt,0.10
nut-m6,M6 nut,0.05
```

## Files

| File | Contents |
| --- | --- |
| `src/import.inset` | The app, the data types, the `CsvReader` declaration, the `Importer` component, and the algorithms |
| `src/csv_reader.rs` | The Rust that implements `CsvReader` with the `csv` crate |
| `src/import.req.inset` | The requirement tests |

## What it exercises

| Construct | Where |
| --- | --- |
| External Rust crate | `extern component CsvReader` declares the messages and capabilities of a component written in Rust. `#[rust(...)]` names the file and the crate |
| Declared capabilities | `CsvReader` has no fields, so it holds no capabilities and can only compute |
| External component as a child | `Importer` starts it with `start csv: CsvReader {}` and sends it messages like any component |
| Structured concurrency | `fork for row in rows { ... }` in `parse_price_list` reads every row concurrently and joins the results in order |
| Top-level algorithms | `parse_price_list`, `parse_row`, `parse_price` and `add_vat`, which take no `self` |
| Property tests | "Any price ... reads back as itself" and "The gross price ... is never below the net price" |
| Table test | `parse_price`, with `Option` results |
| Test against an external boundary | The `CsvReader` requirement, which treats the Rust code as a black box |
| Text blocks in tests | The `"""` blocks holding a CSV body and a multi-line reply |
| Error replies with a body | `ImportError::InvalidRows(String)` replies `400 Bad Request` with the string as its body |

It covers the brief's CSV import row: an external Rust crate, structured concurrency, and property tests.

The `add_vat` property only holds for whole numbers of pence. For a net price of `0.001`, VAT rounds the gross price down to `0.00`. The property's `where` clause says so, and `parse_price` enforces it.

## Expected diagrams

Sketches of what each review level should show. Stage 3 replaces them with `expected/diagram.json` and rendered SVGs.

**Component view.** `Importer` is small, so it is shown expanded. The double-lined box is external: the reviewer sees its interface and its capabilities, which are none, but not its code.

```
                     ┌─ PriceImport ──────────────────────────────────────────────┐
                     │                                                            │
                     │  grant http: HttpServer (port 8080)                        │
                     │  grant products: Table<String, Product> (catalogue.db)     │
                     │        │                                                   │
                     │        │ move http, move products                          │
                     │        ▼                                                   │
                     │  ┌─ Importer ───────────────────────────────────────┐      │
 ( HTTP clients ) ───┼──┼▶ POST /price-lists (via http)                    │      │
                     │  │      │                                           │      │
                     │  │      ├── read(&text) ──▶ ╔═ CsvReader ═════════╗ │      │
                     │  │      │                   ║  external: csv 1.3  ║ │      │
                     │  │      │                   ║  capabilities: none ║ │      │
                     │  │      │                   ╚═════════════════════╝ │      │
                     │  │      │                                           │      │
                     │  │      └── products ───────────────────────────────┼──────┼──▶ ( catalogue.db )
                     │  │                                                  │      │
                     │  │  algorithms: parse_price_list, parse_row,        │      │
                     │  │              parse_price, add_vat                │      │
                     │  └──────────────────────────────────────────────────┘      │
                     └────────────────────────────────────────────────────────────┘
```

**Flowchart of `parse_price_list`.** The fork runs one branch per row, and the join waits for them all.

```
                      ( rows )
                         │
                         ▼
           ═══════ fork: for each row ═══════
              │            │            │
              ▼            ▼            ▼
         [parse_row]  [parse_row]  [parse_row]
              │            │            │
           ═══════════════ join ═════════════
                         │
                         ▼
         [ sort results into products and errors ]
                         │
                         ▼
                  ◇ any errors? ◇
                   │ no       │ yes
                   ▼          ▼
            Ok(products)   Err(errors)
```

- The only external system the program touches is `catalogue.db`, through `Importer`. The Rust code cannot reach it, because it holds nothing.
- The flowchart shows that rows are read independently, which is what makes reporting every invalid line possible.
