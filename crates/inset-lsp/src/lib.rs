//! The Inset language server, which the `inset lsp` command runs over
//! standard input and output.
//!
//! It shows in the editor what `inset check` reports, as the author types,
//! and adds hover, go-to-definition and a document outline. It also answers
//! `inset/diagram`, a request of its own that `diagram.rs` describes, with
//! the program's picture for an editor to show. It parses and
//! checks with `inset-syntax` and `inset-check`, so the editor and the
//! command line cannot disagree, and there is no second parser to keep in
//! step with the language.
//!
//! A document belongs to the program whose `src/` or `requirements/`
//! directory holds it, found by walking up from the file to a directory
//! with a `src/main.inset`. The whole program is checked on every change,
//! from the open documents' text and the other files as they are on disk,
//! and the diagnostics of every one of its files are published, because a
//! change to one file can fix or break another: renaming a route breaks
//! the requirement steps that reach it. Inset programs are small, so this
//! is quicker than tracking what depends on what. A `.inset` file outside
//! any program, such as a scratch file, gets its parse errors only.
//!
//! Each diagnostic's message is the one `inset check` prints, then the
//! label it prints at the span and its help, one to a line, so a reader of
//! the editor's problem list, person or agent, gets the whole report.
//!
//! The server is synchronous on one thread, built on `lsp-server` and
//! `lsp-types`: requests are answered in order, and checking a program
//! takes far less time than typing a character.

mod diagram;
mod outline;
mod position;
mod uri;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};

use inset_check::{Analysis, MAIN, Program, SourceFile, analyze};
use inset_syntax::{Diagnostic, is_requirements_file, parse_code, parse_requirements};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{DocumentSymbolRequest, GotoDefinition, HoverRequest, Request as _};
use lsp_types::{
    DiagnosticSeverity, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, Location, MarkupContent, MarkupKind, OneOf, PublishDiagnosticsParams,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};

use position::{Encoding, LineIndex};

type BoxError = Box<dyn Error + Send + Sync>;

/// Serves the protocol over standard input and output until the client
/// asks the server to exit.
pub fn serve_stdio() -> Result<(), BoxError> {
    let (connection, io_threads) = Connection::stdio();
    run(&connection)?;
    drop(connection);
    io_threads.join()?;
    Ok(())
}

/// Serves the protocol over a connection until the client asks the server
/// to shut down.
pub fn run(connection: &Connection) -> Result<(), BoxError> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let offered = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_deref());
    let encoding = Encoding::negotiate(offered);
    let capabilities = ServerCapabilities {
        position_encoding: Some(encoding.kind()),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        ..ServerCapabilities::default()
    };
    let result = serde_json::json!({
        "capabilities": capabilities,
        "serverInfo": ServerInfo {
            name: "inset".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        },
    });
    connection.initialize_finish(id, result)?;
    let mut server = Server {
        connection,
        encoding,
        open: HashMap::new(),
        published: HashMap::new(),
    };
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                server.request(request)?;
            }
            Message::Notification(notification) => server.notification(notification)?,
            Message::Response(_) => {}
        }
    }
    Ok(())
}

struct Server<'c> {
    connection: &'c Connection,
    encoding: Encoding,
    /// Each open document's URI, as the client wrote it, and text, by the
    /// URI's text. (`Uri` caches its parts inside, so it is not a key.)
    open: HashMap<String, (Uri, String)>,
    /// The documents last given diagnostics, by the program, or the lone
    /// file, they were checked as part of.
    published: HashMap<PathBuf, HashSet<String>>,
}

/// What a document is checked as part of.
enum Unit {
    /// A program, by its directory.
    Program(PathBuf),
    /// A file outside any program, which is only parsed.
    Lone,
}

impl Server<'_> {
    fn send(&self, message: impl Into<Message>) -> Result<(), BoxError> {
        self.connection.sender.send(message.into())?;
        Ok(())
    }

    fn request(&mut self, request: Request) -> Result<(), BoxError> {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            HoverRequest::METHOD => {
                let params: HoverParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.hover(params))?
            }
            GotoDefinition::METHOD => {
                let params: GotoDefinitionParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.definition(params))?
            }
            DocumentSymbolRequest::METHOD => {
                let params: DocumentSymbolParams = serde_json::from_value(request.params)?;
                serde_json::to_value(self.outline(params))?
            }
            diagram::METHOD => {
                let params: diagram::Params = serde_json::from_value(request.params)?;
                match self.diagram(params) {
                    Ok(picture) => serde_json::to_value(picture)?,
                    Err(message) => {
                        let code = ErrorCode::InvalidParams as i32;
                        return self.send(Response::new_err(id, code, message));
                    }
                }
            }
            method => return self.unknown(id, method),
        };
        self.send(Response::new_ok(id, result))
    }

    fn unknown(&self, id: RequestId, method: &str) -> Result<(), BoxError> {
        self.send(Response::new_err(
            id,
            ErrorCode::MethodNotFound as i32,
            format!("inset lsp does not handle `{method}`"),
        ))
    }

    fn notification(&mut self, notification: Notification) -> Result<(), BoxError> {
        let params = notification.params;
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: lsp_types::DidOpenTextDocumentParams = serde_json::from_value(params)?;
                let document = params.text_document;
                let key = document.uri.as_str().to_string();
                self.open.insert(key, (document.uri.clone(), document.text));
                self.refresh(&document.uri)
            }
            DidChangeTextDocument::METHOD => {
                let params: lsp_types::DidChangeTextDocumentParams =
                    serde_json::from_value(params)?;
                // Full sync: the last change holds the whole text.
                let uri = params.text_document.uri;
                if let Some(change) = params.content_changes.into_iter().last() {
                    self.open
                        .insert(uri.as_str().to_string(), (uri.clone(), change.text));
                }
                self.refresh(&uri)
            }
            DidCloseTextDocument::METHOD => {
                let params: lsp_types::DidCloseTextDocumentParams = serde_json::from_value(params)?;
                self.open.remove(params.text_document.uri.as_str());
                self.refresh(&params.text_document.uri)
            }
            DidSaveTextDocument::METHOD => {
                let params: lsp_types::DidSaveTextDocumentParams = serde_json::from_value(params)?;
                self.refresh(&params.text_document.uri)
            }
            DidChangeWatchedFiles::METHOD => {
                let params: lsp_types::DidChangeWatchedFilesParams =
                    serde_json::from_value(params)?;
                let mut roots = HashSet::new();
                for change in params.changes {
                    if let Some(path) = uri::to_path(&change.uri)
                        && let Unit::Program(root) = self.unit(&path)
                        && roots.insert(root)
                    {
                        self.refresh(&change.uri)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ---- Programs ----------------------------------------------------------

    /// The program a file belongs to: the nearest directory above it whose
    /// `src/` or `requirements/` holds the file, and which has a
    /// `src/main.inset`, on disk or open.
    fn unit(&self, path: &Path) -> Unit {
        for dir in path.ancestors().skip(1) {
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };
            let top = relative.components().next().map(|c| c.as_os_str());
            if top != Some("src".as_ref()) && top != Some("requirements".as_ref()) {
                continue;
            }
            let main = dir.join(MAIN);
            if main.is_file() || self.open_paths().any(|(path, _)| path == main) {
                return Unit::Program(dir.to_path_buf());
            }
        }
        Unit::Lone
    }

    /// A program's files: those on disk, with the open documents' text in
    /// place of what is saved.
    fn program(&self, root: &Path) -> Program {
        let (mut files, web) =
            Program::load(root).map_or_else(|_| (Vec::new(), None), |p| (p.files, p.web));
        for (path, text) in self.open_paths() {
            let Some(relative) = program_path(root, &path) else {
                continue;
            };
            match files.iter_mut().find(|f| f.path == relative) {
                Some(file) => file.text.clone_from(text),
                None => files.push(SourceFile {
                    path: relative,
                    text: text.clone(),
                }),
            }
        }
        let mut program = Program::from_files(files);
        program.web = web;
        program
    }

    /// The text of a document: as open, or as saved.
    fn text(&self, uri: &Uri) -> Option<String> {
        if let Some((_, text)) = self.open.get(uri.as_str()) {
            return Some(text.clone());
        }
        std::fs::read_to_string(uri::to_path(uri)?).ok()
    }

    /// Checks whatever a document is part of, and publishes the diagnostics.
    fn refresh(&mut self, uri: &Uri) -> Result<(), BoxError> {
        let unit = uri::to_path(uri).map_or(Unit::Lone, |path| self.unit(&path));
        match unit {
            Unit::Program(root) => {
                let program = self.program(&root);
                let reports = analyze(&program).reports;
                let mut by_file: Vec<Vec<&Diagnostic>> = vec![Vec::new(); program.files.len()];
                for report in &reports {
                    by_file[report.file].push(&report.diagnostic);
                }
                let documents = program
                    .files
                    .iter()
                    .zip(by_file)
                    .map(|(file, diagnostics)| {
                        let uri = self.uri_of(&root.join(&file.path));
                        (uri, &file.text, diagnostics)
                    })
                    .collect();
                self.publish(root, documents)
            }
            Unit::Lone => {
                let key = uri::to_path(uri).unwrap_or_else(|| PathBuf::from(uri.as_str()));
                let Some((_, text)) = self.open.get(uri.as_str()).cloned() else {
                    return self.publish(key, Vec::new());
                };
                let diagnostics = if is_requirements_file(uri.as_str()) {
                    parse_requirements(&text).diagnostics
                } else {
                    parse_code(&text).diagnostics
                };
                let diagnostics = diagnostics.iter().collect();
                self.publish(key, vec![(uri.clone(), &text, diagnostics)])
            }
        }
    }

    /// Publishes the diagnostics of a program's documents, and clears those
    /// of any document it published before that is no longer in it.
    fn publish(
        &mut self,
        key: PathBuf,
        documents: Vec<(Uri, &String, Vec<&Diagnostic>)>,
    ) -> Result<(), BoxError> {
        let now: HashSet<String> = documents
            .iter()
            .map(|(uri, ..)| uri.as_str().to_string())
            .collect();
        let before = self.published.insert(key, now.clone()).unwrap_or_default();
        for uri in before.difference(&now) {
            self.send_diagnostics(uri.parse()?, Vec::new())?;
        }
        for (uri, text, diagnostics) in documents {
            let lines = LineIndex::new(text, self.encoding);
            let diagnostics = diagnostics.iter().map(|d| to_lsp(d, &lines)).collect();
            self.send_diagnostics(uri, diagnostics)?;
        }
        Ok(())
    }

    fn send_diagnostics(
        &self,
        uri: Uri,
        diagnostics: Vec<lsp_types::Diagnostic>,
    ) -> Result<(), BoxError> {
        let params = PublishDiagnosticsParams {
            uri,
            diagnostics,
            version: None,
        };
        self.send(Notification::new(PublishDiagnostics::METHOD.into(), params))
    }

    /// The URI of a file: as the client wrote it, if it is open, so the
    /// client matches it to its document.
    fn uri_of(&self, path: &Path) -> Uri {
        self.open
            .values()
            .find(|(uri, _)| uri::to_path(uri).as_deref() == Some(path))
            .map_or_else(|| uri::from_path(path), |(uri, _)| uri.clone())
    }

    /// The open documents that are files, by path.
    fn open_paths(&self) -> impl Iterator<Item = (PathBuf, &String)> {
        self.open
            .values()
            .filter_map(|(uri, text)| Some((uri::to_path(uri)?, text)))
    }

    /// The analysis of the program a document is part of, with the document's
    /// index among its files and the byte offset of a position in it.
    fn analysis(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<(PathBuf, Program, Analysis, usize, usize)> {
        let path = uri::to_path(uri)?;
        let Unit::Program(root) = self.unit(&path) else {
            return None;
        };
        let program = self.program(&root);
        let relative = program_path(&root, &path)?;
        let file = program.files.iter().position(|f| f.path == relative)?;
        let offset = LineIndex::new(&program.files[file].text, self.encoding).offset(position);
        let analysis = analyze(&program);
        Some((root, program, analysis, file, offset))
    }

    // ---- Requests ----------------------------------------------------------

    fn hover(&self, params: HoverParams) -> Option<Hover> {
        let at = params.text_document_position_params;
        let (_, _, analysis, file, offset) = self.analysis(&at.text_document.uri, at.position)?;
        let symbol = analysis.index.at(file, offset)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: symbol.hover.clone(),
            }),
            range: None,
        })
    }

    fn definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let at = params.text_document_position_params;
        let (root, program, analysis, file, offset) =
            self.analysis(&at.text_document.uri, at.position)?;
        let symbol = analysis.index.at(file, offset)?;
        let file = &program.files[symbol.file];
        let lines = LineIndex::new(&file.text, self.encoding);
        Some(GotoDefinitionResponse::Scalar(Location {
            uri: self.uri_of(&root.join(&file.path)),
            range: lines.range(symbol.name.span),
        }))
    }

    fn outline(&self, params: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let uri = &params.text_document.uri;
        let text = self.text(uri)?;
        let lines = LineIndex::new(&text, self.encoding);
        let symbols = outline::outline(&text, is_requirements_file(uri.as_str()), &lines);
        Some(DocumentSymbolResponse::Nested(symbols))
    }
}

/// A file's path within a program, such as `src/main.inset`, if it is one
/// of the program's `.inset` files.
fn program_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let in_program = matches!(parts.first(), Some(&"src" | &"requirements"));
    (in_program && path.extension().is_some_and(|e| e == "inset")).then(|| parts.join("/"))
}

/// A diagnostic as the protocol carries it, with [`message`].
fn to_lsp(diagnostic: &Diagnostic, lines: &LineIndex) -> lsp_types::Diagnostic {
    lsp_types::Diagnostic {
        range: lines.range(diagnostic.span),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("inset".into()),
        message: message(diagnostic),
        ..lsp_types::Diagnostic::default()
    }
}

/// A diagnostic's message in the editor: the command line's, then its
/// label and help, one to a line.
fn message(diagnostic: &Diagnostic) -> String {
    let mut message = format!("{}\n{}", diagnostic.message, diagnostic.label);
    if let Some(help) = &diagnostic.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    message
}
