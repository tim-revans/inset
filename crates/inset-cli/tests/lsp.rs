//! Drives `inset lsp` over JSON-RPC on its standard input and output, as an
//! editor does: diagnostics for the checker's negative cases, which must
//! match `inset check`, then hover, go-to-definition, the outline and the
//! diagram.

#[path = "../../inset-check/tests/cases/mod.rs"]
mod cases;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use inset_check::{Program, SourceFile, check};
use serde_json::{Value, json};

struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    /// Notifications from the server, oldest first, not yet looked at.
    notifications: Vec<Value>,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_inset"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start inset lsp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut client = Client {
            child,
            stdin,
            stdout,
            next_id: 0,
            notifications: Vec::new(),
        };
        let result = client.request("initialize", json!({ "capabilities": {} }));
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        client.notify("initialized", json!({}));
        client
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
        self.stdin.flush().expect("flush");
    }

    fn read(&mut self) -> Value {
        let mut length = None;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).expect("read header");
            assert!(!line.is_empty(), "the server closed its output");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length: ") {
                length = Some(value.parse::<usize>().expect("a length"));
            }
        }
        let mut body = vec![0; length.expect("a Content-Length header")];
        self.stdout.read_exact(&mut body).expect("read body");
        serde_json::from_slice(&body).expect("JSON")
    }

    /// Sends a request and returns its response, keeping the notifications
    /// that arrive before it.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.read();
            if message.get("method").is_some() {
                self.notifications.push(message);
            } else if message["id"] == id {
                return message;
            }
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let response = self.call(method, params);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn open(&mut self, path: &Path, text: &str) {
        let language = if path.to_string_lossy().ends_with(".req.inset") {
            "inset-req"
        } else {
            "inset"
        };
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(path), "languageId": language, "version": 1, "text": text,
            }}),
        );
    }

    /// The diagnostics last published for each document. The server answers
    /// in order, so once it answers a request every earlier change is checked.
    fn diagnostics(&mut self) -> BTreeMap<String, Vec<Value>> {
        let response = self.call("inset/unknown", json!({}));
        assert_eq!(response["error"]["code"], -32601);
        let mut latest = BTreeMap::new();
        for message in self.notifications.drain(..) {
            if message["method"] == "textDocument/publishDiagnostics" {
                let params = &message["params"];
                let uri = params["uri"].as_str().expect("a URI").to_string();
                let list = params["diagnostics"].as_array().expect("a list").clone();
                latest.insert(uri, list);
            }
        }
        latest
    }

    fn at(&mut self, method: &str, path: &Path, (line, character): (usize, usize)) -> Value {
        self.request(
            method,
            json!({
                "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character },
            }),
        )
    }

    fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let status = self.child.wait().expect("wait for inset lsp");
        assert!(status.success(), "inset lsp exited with {status}");
    }
}

/// The `file:` URI of a path, encoded as the server encodes it.
fn uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    let text = path.to_string_lossy().replace('\\', "/");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .canonicalize()
        .expect("the example exists")
}

/// The line and column of the first `needle` in `text`, counted from zero.
fn find(text: &str, needle: &str) -> (usize, usize) {
    let at = text.find(needle).unwrap_or_else(|| panic!("no `{needle}`"));
    let line = text[..at].matches('\n').count();
    let column = at - text[..at].rfind('\n').map_or(0, |i| i + 1);
    (line, column)
}

fn start(range: &Value) -> (usize, usize) {
    let at = |key: &str| range["start"][key].as_u64().expect("a number") as usize;
    (at("line"), at("character"))
}

// ---- Diagnostics -----------------------------------------------------------

/// Each case's diagnostics, as the server publishes them, are snapshotted
/// and must be those `inset check` prints: the same file, the same line and
/// column, and the same message, label and help.
#[test]
fn diagnostics_match_inset_check() {
    let mut client = Client::start();
    for case in cases::all() {
        // A directory that need not exist: the documents are open, so the
        // server reads nothing from disk.
        let root = std::env::temp_dir().join("inset-lsp-cases").join(case.name);
        let files = case.files();
        for &(path, text) in &files {
            client.open(&root.join(path), text);
        }
        let published = client.diagnostics();

        let mut snapshot = String::new();
        let mut from_server = Vec::new();
        for &(path, _) in &files {
            let list = &published[&uri(&root.join(path))];
            for diagnostic in list {
                let (line, column) = start(&diagnostic["range"]);
                let range = &diagnostic["range"]["end"];
                let message = diagnostic["message"].as_str().expect("a message");
                snapshot.push_str(&format!(
                    "{path}:{}:{}-{}:{}\n{message}\n\n",
                    line + 1,
                    column + 1,
                    range["line"].as_u64().unwrap() + 1,
                    range["character"].as_u64().unwrap() + 1,
                ));
                from_server.push(format!("{path}:{}:{}\n{message}", line + 1, column + 1));
            }
        }

        let program = Program::from_files(
            files
                .iter()
                .map(|(path, text)| SourceFile {
                    path: path.to_string(),
                    text: text.to_string(),
                })
                .collect(),
        );
        let from_check: Vec<String> = check(&program)
            .iter()
            .map(|report| {
                // Where `inset check` says the problem is, from its output.
                let rendered = program.render(report, false);
                let header = rendered.lines().nth(1).expect("a location line");
                let location = header
                    .split("─[ ")
                    .nth(1)
                    .and_then(|l| l.strip_suffix(" ]"))
                    .expect("a location");
                let d = &report.diagnostic;
                let help = d
                    .help
                    .as_ref()
                    .map_or(String::new(), |h| format!("\nhelp: {h}"));
                format!("{location}\n{}\n{}{help}", d.message, d.label)
            })
            .collect();
        // The server publishes by file in the case's order, and `inset
        // check` reports in order of path.
        let (mut from_server, mut from_check) = (from_server, from_check);
        from_server.sort();
        from_check.sort();
        assert_eq!(from_server, from_check, "{}", case.name);
        insta::with_settings!({ omit_expression => true }, {
            insta::assert_snapshot!(case.name, snapshot);
        });
    }
    client.shutdown();
}

/// After a syntax error the parser carries on, so the errors after it, and
/// the semantic errors elsewhere, still show. A declaration that does not
/// parse is left out, so the names it would declare are reported where
/// they are used.
#[test]
fn reports_every_error_not_just_the_first() {
    let mut client = Client::start();
    let main = cases::edit(
        cases::HELLO,
        "setting port: u16 = 8080;",
        "setting port: u16 = ;",
    );
    let main = cases::edit(&main, "http: HttpServer,", "http HttpServer,");
    let main = cases::edit(&main, "{name}!", "{nme}!");
    let root = std::env::temp_dir().join("inset-lsp-recovery");
    let path = root.join("src/main.inset");
    client.open(&path, &main);
    let published = client.diagnostics();
    let messages: Vec<&str> = published[&uri(&path)]
        .iter()
        .map(|d| d["message"].as_str().unwrap().lines().next().unwrap())
        .collect();
    assert_eq!(
        messages,
        [
            "expected an expression, found `;`",
            "no setting `port`",
            "`Greeter` has no field `http`",
            "expected `:` after the field's name, found `HttpServer`",
            "`via = http` names no field of `Greeter`",
            "cannot find `nme` in this scope",
        ]
    );
    client.shutdown();
}

#[test]
fn hello_http_is_clean_and_a_fix_clears_errors() {
    let mut client = Client::start();
    let dir = example("hello-http");
    let requirements = dir.join("requirements/hello.req.inset");
    let text = std::fs::read_to_string(&requirements).unwrap();
    client.open(&requirements, &text);
    let published = client.diagnostics();
    // Both files are published, the code read from disk, and neither has errors.
    assert_eq!(published.len(), 2, "{published:?}");
    assert!(published.values().all(Vec::is_empty), "{published:?}");

    let broken = text.replace("/hello/Ada", "/helo/Ada");
    let change = |text: &str, version| {
        json!({
            "textDocument": { "uri": uri(&requirements), "version": version },
            "contentChanges": [{ "text": text }],
        })
    };
    client.notify("textDocument/didChange", change(&broken, 2));
    let published = client.diagnostics();
    assert_eq!(published[&uri(&requirements)].len(), 1);
    client.notify("textDocument/didChange", change(&text, 3));
    let published = client.diagnostics();
    assert!(published[&uri(&requirements)].is_empty());
    client.shutdown();
}

#[test]
fn a_file_outside_a_program_gets_its_parse_errors() {
    let mut client = Client::start();
    let path = std::env::temp_dir().join("inset-lsp-lone/scratch.inset");
    client.open(&path, "component Lone {\n    fn f( {}\n}\n");
    let published = client.diagnostics();
    assert!(!published[&uri(&path)].is_empty(), "{published:?}");
    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri(&path) } }),
    );
    assert!(client.diagnostics()[&uri(&path)].is_empty());
    client.shutdown();
}

// ---- Go to definition and hover ------------------------------------------

#[test]
fn a_request_step_leads_to_its_handler() {
    let mut client = Client::start();
    let dir = example("hello-http");
    let requirements = dir.join("requirements/hello.req.inset");
    let text = std::fs::read_to_string(&requirements).unwrap();
    client.open(&requirements, &text);
    let main = dir.join("src/main.inset");
    let code = std::fs::read_to_string(&main).unwrap();

    let (line, column) = find(&text, "/hello/Ada");
    let location = client.at("textDocument/definition", &requirements, (line, column + 3));
    assert_eq!(location["uri"], uri(&main));
    let (line, column) = start(&location["range"]);
    let lines: Vec<&str> = code.lines().collect();
    assert_eq!(&lines[line][column..column + 5], "hello");
    assert_eq!(
        lines[line - 1].trim(),
        r#"#[get("/hello/{name}", via = http)]"#
    );

    // The reply step leads to the handler that replies.
    let (line, _) = find(&text, "then HelloHttp replies");
    let reply = client.at("textDocument/definition", &requirements, (line, 26));
    assert_eq!(reply["range"], location["range"]);

    // The app's name leads to the app.
    let (line, column) = find(&text, "HelloHttp receives");
    let app = client.at("textDocument/definition", &requirements, (line, column + 2));
    assert_eq!(start(&app["range"]), find(&code, "HelloHttp {"));
    client.shutdown();
}

#[test]
fn an_app_name_in_a_step_leads_to_the_app() {
    let mut client = Client::start();
    let dir = example("stock-tracker");
    let requirements = dir.join("requirements/stock.req.inset");
    let text = std::fs::read_to_string(&requirements).unwrap();
    client.open(&requirements, &text);
    let code = std::fs::read_to_string(dir.join("src/main.inset")).unwrap();
    let (line, column) = find(&text, "StockTracker receives");
    let location = client.at("textDocument/definition", &requirements, (line, column));
    assert_eq!(location["uri"], uri(&dir.join("src/main.inset")));
    assert_eq!(start(&location["range"]), find(&code, "StockTracker {"));
    client.shutdown();
}

#[test]
fn a_fact_leads_to_its_grant_and_its_value_to_its_type() {
    let mut client = Client::start();
    let dir = example("stock-tracker");
    let requirements = dir.join("requirements/stock.req.inset");
    let text = std::fs::read_to_string(&requirements).unwrap();
    client.open(&requirements, &text);
    let main = dir.join("src/main.inset");
    let code = std::fs::read_to_string(&main).unwrap();

    let (line, column) = find(&text, "items[\"bolt-m6\"] is Item");
    let grant = client.at("textDocument/definition", &requirements, (line, column + 1));
    assert_eq!(grant["uri"], uri(&main));
    assert_eq!(
        start(&grant["range"]),
        find(&code, "items: Table<String, Item> {")
    );

    let (line, column) = find(&text, "Item { on_hand: 40");
    let item = client.at("textDocument/definition", &requirements, (line, column + 1));
    assert_eq!(start(&item["range"]), find(&code, "Item {\n"));
    let field = client.at("textDocument/definition", &requirements, (line, column + 8));
    assert_eq!(start(&field["range"]), find(&code, "on_hand: u32"));
    client.shutdown();
}

#[test]
fn names_in_code_lead_to_their_declarations() {
    let mut client = Client::start();
    let root = std::env::temp_dir().join("inset-lsp-definitions");
    let path = root.join("src/main.inset");
    let code = cases::COUNTING;
    client.open(&path, code);
    let mut definition = |from: &str, offset: usize| {
        let (line, column) = find(code, from);
        let location = client.at("textDocument/definition", &path, (line, column + offset));
        start(&location["range"])
    };
    // A component started, a capability moved and a field handed it.
    assert_eq!(definition("Front {", 0), find(code, "Front {\n    http"));
    assert_eq!(definition("move http", 5), find(code, "http: HttpServer {"));
    assert_eq!(definition("http: move", 0), find(code, "http: HttpServer,"));
    // An outlet connected to a sibling's message.
    assert_eq!(definition("tally.add", 6), find(code, "add(&mut self"));
    // A route's server, and its path parameter.
    assert_eq!(definition("via = http", 6), find(code, "http: HttpServer,"));
    assert_eq!(definition("{n}", 1), find(code, "n: u32) -> String"));
    // A child, a message sent to it, and a variable.
    assert_eq!(definition("self.log", 5), find(code, "log: Log"));
    assert_eq!(definition("log.record", 4), find(code, "record(&mut self"));
    assert_eq!(definition("{total}", 1), find(code, "total = self"));
    client.shutdown();
}

#[test]
fn a_use_path_leads_to_the_item_in_its_module() {
    let mut client = Client::start();
    let dir = example("stock-tracker");
    let main = dir.join("src/main.inset");
    let warehouse = dir.join("src/warehouse.inset");
    let main_code = std::fs::read_to_string(&main).unwrap();
    let warehouse_code = std::fs::read_to_string(&warehouse).unwrap();
    client.open(&main, &main_code);
    client.open(&warehouse, &warehouse_code);

    // From the root module to a component in `warehouse`.
    let (line, column) = find(&main_code, "warehouse::Warehouse");
    let location = client.at("textDocument/definition", &main, (line, column + 12));
    assert_eq!(location["uri"], uri(&warehouse));
    assert_eq!(
        start(&location["range"]),
        find(&warehouse_code, "Warehouse {")
    );
    // The module's name leads to its `mod` declaration.
    let location = client.at("textDocument/definition", &main, (line, column + 2));
    assert_eq!(location["uri"], uri(&main));
    assert_eq!(start(&location["range"]), find(&main_code, "warehouse;"));
    // From `warehouse` back up to a struct in the root module, and to a
    // field of it used in a body.
    let (line, column) = find(&warehouse_code, "Item, Reorder");
    let location = client.at("textDocument/definition", &warehouse, (line, column));
    assert_eq!(location["uri"], uri(&main));
    assert_eq!(start(&location["range"]), find(&main_code, "Item {"));
    let (line, column) = find(&warehouse_code, "item.on_hand.to_string");
    let location = client.at("textDocument/definition", &warehouse, (line, column + 5));
    assert_eq!(location["uri"], uri(&main));
    assert_eq!(start(&location["range"]), find(&main_code, "on_hand: u32"));
    client.shutdown();
}

#[test]
fn hover_shows_a_struct_field_and_an_error_variant() {
    let mut client = Client::start();
    let dir = example("stock-tracker");
    let main = dir.join("src/main.inset");
    let warehouse = dir.join("src/warehouse.inset");
    let main_code = std::fs::read_to_string(&main).unwrap();
    let warehouse_code = std::fs::read_to_string(&warehouse).unwrap();
    client.open(&main, &main_code);
    client.open(&warehouse, &warehouse_code);
    let mut hover = |path: &Path, code: &str, from: &str, offset: usize| {
        let (line, column) = find(code, from);
        let hover = client.at("textDocument/hover", path, (line, column + offset));
        hover["contents"]["value"].as_str().unwrap().to_string()
    };
    let hovers = [
        hover(&main, &main_code, "on_hand: u32", 0),
        hover(&warehouse, &warehouse_code, "item.on_hand.to_string", 5),
        hover(&main, &main_code, "Item {", 0),
        hover(&main, &main_code, "ShipError {", 0),
        hover(&warehouse, &warehouse_code, "ShipError::UnknownItem", 11),
        hover(&main, &main_code, "warehouse;", 0),
    ];
    insta::with_settings!({ omit_expression => true }, {
        insta::assert_snapshot!(hovers.join("\n\n----\n\n"));
    });
    client.shutdown();
}

#[test]
fn hover_shows_types_capabilities_and_docs() {
    let mut client = Client::start();
    let dir = example("hello-http");
    let main = dir.join("src/main.inset");
    let code = std::fs::read_to_string(&main).unwrap();
    client.open(&main, &code);
    let mut hover = |from: &str, offset: usize| {
        let (line, column) = find(&code, from);
        let hover = client.at("textDocument/hover", &main, (line, column + offset));
        assert_eq!(hover["contents"]["kind"], "markdown");
        hover["contents"]["value"].as_str().unwrap().to_string()
    };
    let hovers = [
        hover("Greeter {", 0),
        hover("via = http", 6),
        hover("move http", 5),
        hover("{name}!", 1),
        hover("port }", 0),
        hover("hello(", 0),
        hover("HelloHttp {", 0),
    ];
    insta::with_settings!({ omit_expression => true }, {
        insta::assert_snapshot!(hovers.join("\n\n----\n\n"));
    });
    client.shutdown();
}

// ---- Outline -------------------------------------------------------------

#[test]
fn outline_lists_app_components_handlers_and_requirements() {
    let mut client = Client::start();
    let mut outline = String::new();
    for (example, path) in [
        ("hello-http", "src/main.inset"),
        ("hello-http", "requirements/hello.req.inset"),
        ("stock-tracker", "src/main.inset"),
    ] {
        let dir = self::example(example);
        let symbols = client.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri(&dir.join(path)) } }),
        );
        outline.push_str(&format!("{path}\n"));
        tree(&symbols, 1, &mut outline);
    }
    insta::with_settings!({ omit_expression => true }, {
        insta::assert_snapshot!(outline);
    });
    client.shutdown();
}

fn tree(symbols: &Value, depth: usize, out: &mut String) {
    for symbol in symbols.as_array().expect("a list") {
        let (line, _) = start(&symbol["selectionRange"]);
        out.push_str(&format!(
            "{}{} ({}) line {}\n",
            "  ".repeat(depth),
            symbol["name"].as_str().unwrap(),
            symbol["detail"].as_str().unwrap_or(""),
            line + 1
        ));
        if let Some(children) = symbol.get("children") {
            tree(children, depth + 1, out);
        }
    }
}

// ---- Diagrams ----------------------------------------------------------------

fn diagram(client: &mut Client, path: &Path, options: &[&str]) -> Value {
    client.request(
        "inset/diagram",
        json!({ "textDocument": { "uri": uri(path) }, "options": options }),
    )
}

/// The picture is the one `inset diagram` draws, and the ID on each element
/// leads to its source: clicking `Greeter` opens the component.
#[test]
fn the_diagram_is_the_one_inset_diagram_draws_and_leads_to_the_source() {
    let mut client = Client::start();
    let dir = example("hello-http");
    let main = dir.join("src/main.inset");
    let code = std::fs::read_to_string(&main).unwrap();
    client.open(&main, &code);
    let expected = |name: &str| std::fs::read_to_string(dir.join("expected").join(name)).unwrap();

    let picture = diagram(&mut client, &main, &[]);
    assert_eq!(picture["app"], "HelloHttp");
    assert_eq!(picture["svg"], expected("diagram.svg"));
    assert_eq!(picture["problems"], json!([]));
    assert_eq!(picture["functions"], json!(["HelloHttp/Greeter/fn:hello"]));
    let flowchart = ["--flowchart", "HelloHttp/Greeter/fn:hello"];
    assert_eq!(
        diagram(&mut client, &main, &flowchart)["svg"],
        expected("hello.svg")
    );

    let options = ["--theme", "dark", "--expand-all"];
    let drawn = Command::new(env!("CARGO_BIN_EXE_inset"))
        .arg("diagram")
        .arg(&dir)
        .args(options)
        .output()
        .expect("run inset diagram");
    assert!(drawn.status.success());
    let svg = String::from_utf8(drawn.stdout).unwrap();
    assert_eq!(diagram(&mut client, &main, &options)["svg"], svg);

    let sources = &picture["sources"];
    let greeter = &sources["HelloHttp/Greeter"];
    assert_eq!(greeter["uri"], uri(&main));
    let (line, column) = find(&code, "component Greeter");
    assert_eq!(
        start(&greeter["range"]),
        (line, column + "component ".len())
    );
    assert_eq!(
        start(&sources["HelloHttp/Greeter/fn:hello"]["range"]),
        find(&code, "hello(")
    );
    assert_eq!(
        start(&sources["HelloHttp/grant:http/outside"]["range"]),
        find(&code, "http: HttpServer {")
    );
    client.shutdown();
}

/// A program with problems gets them, each where it is, instead of a
/// picture. Unknown view options are refused, and a file outside a program
/// has no diagram.
#[test]
fn a_diagram_that_cannot_be_drawn_says_why() {
    let mut client = Client::start();
    let dir = example("hello-http");
    let main = dir.join("src/main.inset");
    let code = std::fs::read_to_string(&main).unwrap();
    client.open(&main, &code.replace("http: move http", "http: move web"));
    let picture = diagram(&mut client, &main, &[]);
    assert!(picture["svg"].is_null(), "{picture}");
    let problem = &picture["problems"][0];
    assert!(
        problem["message"].as_str().unwrap().contains("web"),
        "{problem}"
    );
    assert_eq!(problem["location"]["uri"], uri(&main));
    assert_eq!(
        start(&problem["location"]["range"]).0,
        find(&code, "start Greeter").0
    );

    let response = client.call(
        "inset/diagram",
        json!({ "textDocument": { "uri": uri(&main) }, "options": ["--sideways"] }),
    );
    assert_eq!(response["error"]["code"], -32602, "{response}");

    let lone = std::env::temp_dir().join("inset-lsp-lone/drawing.inset");
    client.open(&lone, "component Lone {}\n");
    assert!(diagram(&mut client, &lone, &[]).is_null());
    client.shutdown();
}
