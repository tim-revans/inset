//! The negative cases of the semantic checks, shared by the checker's tests
//! and the language server's, which must report the same diagnostics. Each
//! case is a small change to Hello HTTP, to `COUNTING`, a program with
//! messages and an outlet between three components, to `SHELF`, a
//! program of two modules with a struct and an error enum, or to `STORE`,
//! a program that keeps a table, with requirements that state its facts.

pub const HELLO: &str = r#"app HelloHttp {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Greeter { http: move http };
}

component Greeter {
    http: HttpServer,

    #[get("/hello/{name}", via = http)]
    fn hello(name: String) -> String {
        format!("Hello, {name}!")
    }
}
"#;

pub const HELLO_REQ: &str = r#"requirement "Visitors are greeted by the name in the web address"
    when HelloHttp receives GET /hello/Ada
    then HelloHttp replies OK with "Hello, Ada!"
"#;

pub const COUNTING: &str = r#"app Counting {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Front { http: move http };
}

component Front {
    http: HttpServer,

    start tally: Tally { total: 0 };
    start log: Log { entries: Vec::new(), counted: send tally.add };

    #[get("/count/{n}", via = http)]
    async fn count(&self, n: u32) -> String {
        send self.log.record(n);
        let total = self.tally.total().await;
        format!("{total}")
    }
}

component Tally {
    total: u32,

    #[message]
    fn add(&mut self, n: u32) {
        self.total += n;
    }

    #[message]
    fn note(&mut self, text: String) {}

    #[message]
    fn total(&self) -> u32 {
        self.total
    }
}

component Log {
    entries: Vec<u32>,
    counted: Outlet<u32>,

    #[message]
    fn record(&mut self, n: u32) {
        self.entries.push(n);
        send self.counted.emit(n);
    }
}
"#;

pub const COUNTING_REQ: &str = r#"requirement "Each count adds to the total"
    when Counting receives GET /count/2
    then Counting replies OK with "2"
"#;

pub const SHELF: &str = r#"mod library;

use library::Desk;

app Shelf {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    start Desk { http: move http };
}

/// A book on the shelf.
#[derive(Clone, Debug, PartialEq)]
struct Book {
    title: String,
    /// How many copies are on the shelf.
    copies: u32,
}
"#;

pub const SHELF_LIBRARY: &str = r#"use crate::Book;

/// Why a book is not lent.
pub enum LendError {
    #[status(NotFound)]
    Unknown,
    #[status(Conflict)]
    AllOut(String),
}

pub component Desk {
    http: HttpServer,

    #[get("/lend/{title}", via = http)]
    fn lend(title: String) -> Result<String, LendError> {
        let book = find(&title)?;
        match book.copies {
            0 => Err(LendError::AllOut(book.title)),
            n => Ok(format!("{} left", n - 1)),
        }
    }
}

fn find(title: &str) -> Result<Book, LendError> {
    for book in shelf() {
        if book.title == title {
            return Ok(book);
        }
    }
    Err(LendError::Unknown)
}

fn shelf() -> Vec<Book> {
    vec![Book { title: "Dune".to_string(), copies: 2 }]
}
"#;

pub const SHELF_REQ: &str = r#"requirement "A book with a copy on the shelf is lent"
    when Shelf receives GET /lend/Dune
    then Shelf replies OK with "1 left"
"#;

pub const STORE: &str = r#"app Store {
    setting port: u16 = 8080;
    grant http: HttpServer { port };
    grant items: Table<String, Item> { file: "store.db", name: "items" };
    start Counter { http: move http, items: move items };
}

/// An item, stored under its SKU.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    /// How many are on hand.
    on_hand: u32,
    price: Decimal,
}

component Counter {
    http: HttpServer,
    items: Table<String, Item>,

    failure {
        /// A database error fails only its own request.
        Capability(items) => rollback,
    }

    #[post("/items/{sku}/receipts", via = http, body = quantity)]
    async fn receive(&mut self, sku: String, quantity: u32) -> Option<String> {
        let mut item = self.items.get(&sku).await?;
        item.on_hand += quantity;
        self.items.insert(sku, item.clone()).await;
        Some(item.on_hand.to_string())
    }
}
"#;

pub const STORE_REQ: &str = r#"requirement "Received stock is added to what is on hand"
    given items["bolt-m6"] is Item { on_hand: 3, price: 0.10 }
    when Store receives POST /items/bolt-m6/receipts with "20"
    then Store replies OK with "23"
    and items["bolt-m6"] is Item { on_hand: 23, price: 0.10 }
    and items["bolt-m6"].on_hand > 20

requirement "A database failure changes nothing"
    given items is empty
    and items fails
    when Store receives POST /items/bolt-m6/receipts with "20"
    then Store replies InternalServerError
    and items is empty
"#;

pub const CATALOGUE: &str = r#"app Catalogue {
    grant http: HttpServer { port: 8080 };
    grant products: Table<String, Product> { file: "catalogue.db", name: "products" };
    /// The page that lists the catalogue.
    front end via http calls ["GET /products?{limit}", "PUT /products/{sku}"];
    start Shop { http: move http, products: move products };
}

/// A product for sale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Product {
    /// What the product is called.
    display_name: String,
    #[serde(rename = "net")]
    net_price: Decimal,
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Part,
    BoxOf(u32),
}

component Shop {
    http: HttpServer,
    products: Table<String, Product>,

    #[get("/products?{limit}", via = http)]
    async fn list(&self, limit: Option<usize>) -> Json<Vec<Product>> {
        let mut products = Vec::new();
        for (_, product) in self.products.entries().await {
            products.push(product);
        }
        products.truncate(limit.unwrap_or(10));
        Json(products)
    }

    #[put("/products/{sku}", via = http, body = product)]
    async fn put(&mut self, sku: String, product: Json<Product>) -> String {
        let Json(product) = product;
        self.products.insert(sku, product).await;
        "stored".to_string()
    }
}
"#;

pub const CATALOGUE_REQ: &str = r#"requirement "A product is listed as it was stored"
    given products is empty
    when Catalogue receives PUT /products/bolt-m6 with Product { display_name: "M6 bolt", net_price: 0.10, kind: Kind::BoxOf(50) }
    then Catalogue replies OK with "stored"
    when Catalogue receives GET /products?limit=5
    then Catalogue replies OK with vec![Product { display_name: "M6 bolt", net_price: 0.10, kind: Kind::BoxOf(50) }]
    when Catalogue receives GET /products?limit=many
    then Catalogue replies BadRequest
"#;

/// A program with an external component written in Rust.
pub const READER: &str = r#"app Reader {
    grant http: HttpServer { port: 8080 };
    start Front { http: move http };
}

/// A line of text.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    number: u64,
    text: String,
}

component Front {
    http: HttpServer,

    start lines: Lines {};

    #[post("/lines", via = http, body = text)]
    async fn count(&self, text: String) -> String {
        let lines = self.lines.split(&text).await;
        lines.len().to_string()
    }
}

/// Splits text into lines.
#[rust(path = "lines.rs", crates = ["memchr@2.7"])]
extern component Lines {
    /// Each line of the text.
    #[message]
    fn split(&self, text: &str) -> Vec<Line>;
}
"#;

pub const READER_REQ: &str = r#"requirement "Lines are counted"
    when Reader receives POST /lines with "a"
    then Reader replies OK with "1"
"#;

/// A broken program: its code and its requirements.
pub struct Case {
    pub name: &'static str,
    pub main: String,
    /// Its other code files, by path.
    pub modules: Vec<(&'static str, String)>,
    pub requirements: String,
}

impl Case {
    /// Every file of the program, by path.
    pub fn files(&self) -> Vec<(&'static str, &str)> {
        let mut files = vec![("src/main.inset", self.main.as_str())];
        files.extend(
            self.modules
                .iter()
                .map(|(path, text)| (*path, text.as_str())),
        );
        files.push(("requirements/main.req.inset", self.requirements.as_str()));
        files
    }
}

/// Replaces one piece of `source`, which must be there.
pub fn edit(source: &str, from: &str, to: &str) -> String {
    assert!(source.contains(from), "no `{from}` to replace");
    source.replacen(from, to, 1)
}

/// Every case, each of which has diagnostics.
pub fn all() -> Vec<Case> {
    let mut cases = Vec::new();
    let mut modules = Vec::new();
    let mut add = |name, main: String, requirements: String| {
        cases.push(Case {
            name,
            main,
            modules: Vec::new(),
            requirements,
        })
    };
    // A change to the shelf's library module, or with `main` its root.
    let mut shelf = |name, main: String, library: String| {
        modules.push(Case {
            name,
            main,
            modules: vec![("src/library.inset", library)],
            requirements: SHELF_REQ.to_string(),
        })
    };
    let library = |from: &str, to: &str| edit(SHELF_LIBRARY, from, to);
    shelf(
        "variant_without_status",
        SHELF.to_string(),
        library("    #[status(Conflict)]\n", ""),
    );
    shelf(
        "error_variant_holding_a_number",
        SHELF.to_string(),
        edit(
            &library("AllOut(String)", "AllOut(u32)"),
            "LendError::AllOut(book.title)",
            "LendError::AllOut(book.copies)",
        ),
    );
    shelf(
        "unknown_status_name",
        SHELF.to_string(),
        library("#[status(Conflict)]", "#[status(Taken)]"),
    );
    shelf(
        "private_item_from_another_module",
        SHELF.to_string(),
        library("pub component Desk", "component Desk"),
    );
    shelf(
        "module_without_a_file",
        edit(SHELF, "mod library;", "mod library;\nmod archive;"),
        SHELF_LIBRARY.to_string(),
    );
    shelf(
        "derive_not_allowed",
        edit(SHELF, "PartialEq)]", "PartialEq, Display)]"),
        SHELF_LIBRARY.to_string(),
    );
    shelf(
        "match_leaves_out_a_case",
        SHELF.to_string(),
        library(
            "n => Ok(format!(\"{} left\", n - 1)),",
            "1 => Ok(String::from(\"0 left\")),",
        ),
    );
    shelf(
        "private_field_from_another_module",
        format!("{SHELF}\nfn days(loan: &library::Loan) -> u32 {{\n    loan.days\n}}\n"),
        format!(
            "{SHELF_LIBRARY}\n/// A book lent out.\npub struct Loan {{\n    pub title: String,\n    days: u32,\n}}\n"
        ),
    );
    {
        let main = edit(
            HELLO,
            "    fn hello(name: String) -> String {\n        format!(\"Hello, {name}!\")",
            "    async fn hello(&self, name: String) -> Option<String> {\n        self.greetings.get(&name).await",
        );
        add("undeclared_capability", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "        send self.log.record(n);",
            "        let mut n = n;\n        send self.tally.add(&mut n);",
        );
        add(
            "mutable_borrow_across_boundary",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /helo/Ada");
        add("unknown_route_in_step", HELLO.to_string(), req);
    }
    {
        let req = edit(HELLO_REQ, "GET /hello/Ada", "GET /hello/Ada via https");
        add("unknown_capability_in_step", HELLO.to_string(), req);
    }
    {
        let req = format!(
            "{HELLO_REQ}\n{}",
            edit(HELLO_REQ, "/hello/Ada", "/hello/Bo")
        );
        add("duplicate_requirement", HELLO.to_string(), req);
    }
    {
        let main = edit(
            COUNTING,
            "    start log: Log { entries: Vec::new(), counted: send tally.add };\n",
            "    start log: Log { entries: Vec::new(), counted: send tally.add };\n\n    \
             failure {\n        Message(tally) => rollback,\n    }\n",
        );
        add("failure_arm_without_reason", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "        let total = self.tally.total().await;",
            "        let total: u32 = vec![1, 2].iter().map(|_| self.tally.total().await).sum();",
        );
        add("closure_that_awaits", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(COUNTING, ", counted: send tally.add", "");
        add("unconnected_outlet", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "counted: send tally.add",
            "counted: send tally.note",
        );
        add(
            "outlet_connected_to_wrong_type",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(
            COUNTING,
            "        send self.log.record(n);",
            "        let text = n.to_string();\n        send self.tally.note(text);",
        );
        add(
            "value_across_boundary_without_move",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(COUNTING, "self.tally.total().await", "self.tally.total()");
        add(
            "message_neither_awaited_nor_sent",
            main,
            COUNTING_REQ.to_string(),
        );
    }
    {
        let main = edit(
            COUNTING,
            "send self.log.record(n);",
            "send self.tally.total();",
        );
        add("send_with_reply", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(HELLO, "http: move http", "http: &http");
        add("capability_lent", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "    http: HttpServer,",
            "    #[handed_down]\n    http: HttpServer,",
        );
        add("handed_down_field_kept", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            COUNTING,
            "    total: u32,\n",
            "    total: u32,\n\n    start inner: Tally { total: 0 };\n",
        );
        add("component_starts_itself", main, COUNTING_REQ.to_string());
    }
    {
        let main = edit(HELLO, "\"/hello/{name}\"", "\"/hello\"");
        let req = edit(HELLO_REQ, "/hello/Ada", "/hello");
        add("route_parameter_unsupplied", main, req);
    }
    {
        let main = edit(HELLO, "{name}!", "{nme}!");
        add("format_names_no_variable", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(HELLO, "format!(\"Hello, {name}!\")", "\"Hello!\"");
        add("reply_type_mismatch", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "        format!(",
            "        println!(\"{name}\");\n        let _ = std::fs::read_to_string(&name);\n        format!(",
        );
        add("printing_and_std_io", main, HELLO_REQ.to_string());
    }
    {
        let main = edit(
            HELLO,
            "setting port: u16 = 8080;",
            "setting port: u16 = 80800;\n    setting host: String = \"localhost\";",
        );
        let main = edit(&main, "HttpServer { port }", "HttpServer { port: host }");
        add("setting_and_grant_types", main, HELLO_REQ.to_string());
    }
    {
        let req = edit(COUNTING_REQ, "/count/2", "/count/two");
        add("step_path_of_wrong_type", COUNTING.to_string(), req);
    }
    {
        let req = format!(
            "{}\n{}",
            edit(HELLO_REQ, "replies OK", "replies Ok"),
            edit(
                &edit(
                    HELLO_REQ,
                    "replies OK with \"Hello, Ada!\"",
                    "replies NotFound"
                ),
                "Visitors",
                "Strangers"
            ),
        );
        add("statuses", HELLO.to_string(), req);
    }
    {
        let main = edit(
            HELLO,
            "format!(\"Hello, {name}!\")",
            "format!(\"{}\", self.http)",
        );
        add("self_without_receiver", main, HELLO_REQ.to_string());
    }
    {
        let req = edit(STORE_REQ, "    and items fails", "    and stock fails");
        let req = edit(&req, "    and items is empty", "    and http is empty");
        add("fact_naming_unknown_grant", STORE.to_string(), req);
    }
    {
        let req = edit(
            STORE_REQ,
            "given items[\"bolt-m6\"] is Item { on_hand: 3, price: 0.10 }",
            "given items[6] is Item { on_hand: \"3\", price: 0.10 }",
        );
        let req = edit(
            &req,
            "price: 0.10 }\n    and",
            "price: dec!(1.2.3) }\n    and",
        );
        add("fact_of_wrong_type", STORE.to_string(), req);
    }
    {
        let req = edit(
            STORE_REQ,
            "    then Store replies InternalServerError\n",
            "    then Store replies InternalServerError\n    and items fails\n",
        );
        add("fails_after_then", STORE.to_string(), req);
    }
    {
        let req = edit(
            STORE_REQ,
            "    and items[\"bolt-m6\"].on_hand > 20",
            "    and items[\"bolt-m6\"].on_hand > dec!(20)\n    and items[\"bolt-m6\"].price\n    \
             and 1 + 1 == 2\n    and items[\"bolt-m6\"].weight == 0",
        );
        add("check_of_wrong_type", STORE.to_string(), req);
    }
    {
        let main = edit(
            STORE,
            "#[derive(Clone, Debug, PartialEq)]\n",
            "#[derive(Clone)]\n",
        );
        add("compared_without_debug", main, STORE_REQ.to_string());
    }
    {
        let main = edit(
            STORE,
            "grant items: Table<String, Item>",
            "grant items: Table<f64, Item>",
        );
        let main = edit(
            &main,
            "    items: Table<String, Item>,",
            "    items: Table<f64, Item>,",
        );
        add("table_key_not_orderable", main, STORE_REQ.to_string());
    }
    {
        // The second row's cell is text where a number goes, and the
        // third's reply is not one the route can give.
        let req = format!(
            "{STORE_REQ}\n{}",
            r#"requirement "Received stock is added, whatever is on hand"
    given items["bolt-m6"] is Item { on_hand: <on hand>, price: 0.10 }
    when Store receives POST /items/bolt-m6/receipts with "20"
    then Store replies <status> with "<after>"
    examples
    | on hand | status | after |
    | 3       | OK     | 23    |
    | three   | OK     | 23    |
    | 3       | Teapot | 23    |
"#
        );
        add("examples_cell_of_wrong_type", STORE.to_string(), req);
    }
    // ---- JSON bodies, query parameters and the front end ----
    let catalogue = |from: &str, to: &str| edit(CATALOGUE, from, to);
    let catalogue_req = |from: &str, to: &str| edit(CATALOGUE_REQ, from, to);
    add(
        "json_body_without_deserialize",
        catalogue(
            "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\n#[serde(rename_all = \"snake_case\")]",
            "#[derive(Clone, Debug, PartialEq, Serialize)]\n#[serde(rename_all = \"snake_case\")]",
        ),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "json_with_a_map_keyed_by_numbers",
        catalogue(
            "    kind: Kind,\n}",
            "    kind: Kind,\n    stock: HashMap<u32, u32>,\n}",
        ),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "serde_rule_and_argument_not_supported",
        catalogue(
            "#[serde(rename = \"net\")]",
            "#[serde(rename = \"net\", default)]",
        )
        .replace("rename_all = \"snake_case\"", "rename_all = \"snake\""),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "serde_without_a_serde_derive",
        CATALOGUE.replace(
            "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\n#[serde(rename_all = \"snake_case\")]",
            "#[derive(Clone, Debug, PartialEq)]\n#[serde(rename_all = \"snake_case\")]",
        ),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "two_fields_with_one_json_name",
        catalogue("#[serde(rename = \"net\")]", "#[serde(rename = \"kind\")]"),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "query_parameter_without_function_parameter",
        catalogue(
            "#[get(\"/products?{limit}\"",
            "#[get(\"/products?{limit}&{after}\"",
        ),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "front_end_calls_unknown_route",
        catalogue(
            "calls [\"GET /products?{limit}\", \"PUT /products/{sku}\"]",
            "calls [\"GET /products\", \"get /products/{sku}\", \"PUT /products/{sku}\", \"PUT /products/{sku}\"]",
        ),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "front_end_via_not_a_server",
        catalogue("front end via http", "front end via products"),
        CATALOGUE_REQ.to_string(),
    );
    add(
        "json_value_of_wrong_type",
        CATALOGUE.to_string(),
        catalogue_req(
            "kind: Kind::BoxOf(50) }\n    then",
            "kind: Kind::Crate }\n    then",
        ),
    );
    add(
        "value_for_a_text_body",
        CATALOGUE.to_string(),
        catalogue_req(
            "then Catalogue replies OK with \"stored\"",
            "then Catalogue replies OK with 20",
        ),
    );
    {
        // `#[defensive]` on a whole function, on a statement that is no
        // branch and has no `?`, and inside a closure.
        let main = edit(
            COUNTING,
            "    #[message]\n    fn add(",
            "    #[message]\n    #[defensive(\"nothing adds\")]\n    fn add(",
        );
        let main = edit(
            &main,
            "        let total = self.tally.total().await;",
            "        #[defensive(\"the total is always there\")]\n        \
             let total = self.tally.total().await;\n        \
             let doubled: Vec<u32> = vec![n].iter().map(|x| {\n            \
             #[defensive(\"never zero\")]\n            x * 2\n        }).collect();",
        );
        add("defensive_marks_nothing", main, COUNTING_REQ.to_string());
    }
    add(
        "unknown_query_parameter",
        CATALOGUE.to_string(),
        catalogue_req("GET /products?limit=5", "GET /products?limit=5&sort=name"),
    );
    {
        let case = Case {
            name: "file_not_a_module",
            main: SHELF.to_string(),
            modules: vec![
                ("src/library.inset", SHELF_LIBRARY.to_string()),
                ("src/archive.inset", "struct Old {}\n".to_string()),
            ],
            requirements: SHELF_REQ.to_string(),
        };
        cases.push(case);
    }
    {
        let reader = |from: &str, to: &str| edit(READER, from, to);
        let mut add = |name, main: String, requirements: String| {
            cases.push(Case {
                name,
                main,
                modules: Vec::new(),
                requirements,
            })
        };
        add(
            "external_without_rust",
            reader(
                "#[rust(path = \"lines.rs\", crates = [\"memchr@2.7\"])]\n",
                "",
            ),
            READER_REQ.to_string(),
        );
        add(
            "rust_attribute_arguments",
            reader(
                "#[rust(path = \"lines.rs\", crates = [\"memchr@2.7\"])]",
                "#[rust(path = \"../lines.rs\", crates = [\"memchr\", \"csv@one\", \"interface@1\"])]",
            ),
            READER_REQ.to_string(),
        );
        let main = reader(
            "    fn split(&self, text: &str) -> Vec<Line>;",
            "    fn split(&self, text: &str) -> Vec<Line>;\n\n    \
             #[message]\n    fn total(&self, prices: Vec<Decimal>, counts: HashMap<String, u32>) -> u128;\n\n    \
             fn new(line_2: Line) -> Line;\n\n    \
             #[message]\n    async fn wait(&mut self);",
        );
        let main = edit(&main, "    number: u64,", "    number_2: u64,");
        add(
            "external_message_types_and_names",
            main,
            READER_REQ.to_string(),
        );
        let main = reader(
            "    start lines: Lines {};",
            "    start lines: Lines {};\n\n    failure {\n        \
             /// The lines are read again.\n        Child(lines) => rollback,\n    }",
        );
        add("external_child_rolled_back", main, READER_REQ.to_string());
        add(
            "external_with_a_field",
            edit(
                &reader(
                    "    start lines: Lines {};",
                    "    start lines: Lines { limit: 10 };",
                ),
                "extern component Lines {\n",
                "extern component Lines {\n    limit: u32,\n",
            ),
            READER_REQ.to_string(),
        );
    }
    {
        // ---- `fork` and properties ----
        let mut add = |name, main: String, requirements: String| {
            cases.push(Case {
                name,
                main,
                modules: Vec::new(),
                requirements,
            })
        };
        // A branch that leaves with `?`, changes a variable every branch
        // sees, pushes to one, and writes to a table.
        let main = edit(
            STORE,
            "        Some(item.on_hand.to_string())\n    }",
            r#"        Some(item.on_hand.to_string())
    }

    #[post("/counts", via = http, body = text)]
    async fn count(&mut self, text: String) -> Option<String> {
        let mut total = 0;
        let mut seen = Vec::new();
        let skus: Vec<String> = text.lines().map(|l| l.to_string()).collect();
        let found = fork for sku in &skus {
            let item = self.items.get(sku).await?;
            total += item.on_hand;
            seen.push(sku.clone());
            let mut mine = Vec::new();
            mine.push(item.on_hand);
            self.items.insert(sku.clone(), item.clone()).await;
            mine
        };
        Some(found.len().to_string())
    }"#,
        );
        add(
            "fork_branch_changes_what_others_see",
            main,
            STORE_REQ.to_string(),
        );
        let property = |inputs: &str, steps: &str| {
            format!(
                "{STORE_REQ}\nrequirement \"Any quantity received is added\"\n    for all {inputs}\n{steps}"
            )
        };
        let steps = r#"    given items["bolt-m6"] is Item { on_hand: 3, price: 0.10 }
    when Store receives POST /items/bolt-m6/receipts with "<quantity>"
    then Store replies OK with "<quantity>"
    and items["bolt-m6"].price == <price>
"#;
        add(
            "property_inputs",
            STORE.to_string(),
            property(
                "quantity: u32, price: Decimal, name: String, quantity: u8, unused: bool",
                steps,
            ),
        );
        add(
            "property_condition",
            STORE.to_string(),
            property(
                "quantity: u32, price: Decimal where quantity.scale() <= 2 && 5 > 3 && \
                 items[\"bolt-m6\"].on_hand > 0",
                steps,
            ),
        );
        add(
            "property_value_of_wrong_type",
            STORE.to_string(),
            property("quantity: u32, price: bool where quantity > 0", steps),
        );
    }
    cases.extend(modules);
    cases
}
