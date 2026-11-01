//! The program as the host sees it: its manifest, its start, and requests in.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Once;

use crate::actor::{self, Running, Scope};
use crate::coverage;
use crate::executor;
use crate::failure::{Failure, Response, Stopped};
use crate::http::{HttpServer, Reply, Request};
use crate::stored::{KeyType, Stored};
use crate::table::Table;

/// What the compiler emits for the app.
pub trait App {
    const NAME: &'static str;
    const SETTINGS: &'static [Setting];
    const GRANTS: &'static [Grant];

    /// The app's front end, if it has one.
    const FRONT_END: Option<FrontEnd> = None;

    /// The external components the program starts, by name, which the host
    /// loads beside it.
    const EXTERNALS: &'static [&'static str] = &[];

    /// The elements coverage counts in the top-level algorithms, by their
    /// full IDs, which [`crate::coverage::algorithm`] marks by index.
    const BRANCHES: &'static [&'static str] = &[];

    /// The app's `start` statements.
    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped>;
}

/// `front end via server calls [...];`: the files of `web/`, compiled in.
#[derive(Clone, Copy)]
pub struct FrontEnd {
    /// The `HttpServer` grant that serves them.
    pub server: &'static str,
    /// Each file's path from `web/`, its content type, and its bytes, in
    /// order of path.
    pub files: &'static [(&'static str, &'static str, &'static [u8])],
}

impl FrontEnd {
    /// The file a `GET` of `path` asks for: `index.html` for `/` or a path
    /// ending in `/`, and otherwise the file at the path.
    fn file(&self, path: &str) -> Option<Reply> {
        let path = path.split_once('?').map_or(path, |(path, _)| path);
        let path = crate::http::percent_decode(path.strip_prefix('/')?)?;
        let path = if path.is_empty() || path.ends_with('/') {
            format!("{path}index.html")
        } else {
            path
        };
        let (_, content_type, bytes) = self.files.iter().find(|(p, _, _)| *p == path)?;
        Some(Reply {
            status: 200,
            content_type: Some(content_type),
            body: bytes.to_vec(),
        })
    }
}

/// `setting name: Type = default;`
pub struct Setting {
    pub name: &'static str,
    pub ty: SettingType,
    /// The default, written as `--set` would write it.
    pub default: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub enum SettingType {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    String,
}

/// `grant name: Capability { fields };`
pub struct Grant {
    pub name: &'static str,
    pub capability: &'static str,
    pub fields: &'static [(&'static str, GrantValue)],
}

pub enum GrantValue {
    /// Fixed in the source, written as `--set` would write it.
    Literal(&'static str),
    /// The value of a setting.
    Setting(&'static str),
}

/// The capabilities the host granted, taken by the app's `start` statements.
pub struct Grants(());

impl Grants {
    pub fn http_server(&mut self, grant: &str) -> HttpServer {
        HttpServer::new(grant)
    }

    /// Opens a table grant, and refuses it if anything it holds is not a
    /// `K` and a `V`. The host names the file and the table.
    pub fn table<K: KeyType, V: Stored>(&mut self, grant: &str) -> Result<Table<K, V>, Stopped> {
        Table::open(grant).map_err(|message| Stopped {
            component: format!("grant:{grant}"),
            failure: "Capability".into(),
            message,
        })
    }
}

#[derive(Default)]
struct Program {
    /// The component serving each `HttpServer` grant, and the field holding it.
    servers: BTreeMap<String, (&'static str, Rc<dyn Running>)>,
    /// Every component, in start order.
    actors: Vec<Rc<dyn Running>>,
    front_end: Option<FrontEnd>,
    stopped: Option<Stopped>,
}

thread_local! {
    static PROGRAM: RefCell<Program> = RefCell::default();
}

pub(crate) fn serve(grant: &str, field: &'static str, actor: Rc<dyn Running>) {
    PROGRAM.with_borrow_mut(|p| p.servers.insert(grant.into(), (field, actor)));
}

pub(crate) fn register(actor: Rc<dyn Running>) {
    PROGRAM.with_borrow_mut(|p| p.actors.push(actor));
}

pub(crate) fn stop(stopped: Stopped) {
    PROGRAM.with_borrow_mut(|p| {
        p.stopped.get_or_insert(stopped);
    });
}

fn stopped() -> Option<Stopped> {
    PROGRAM.with_borrow(|p| p.stopped.clone())
}

fn run_until_idle() -> Result<(), Stopped> {
    executor::run_until_idle(|| PROGRAM.with_borrow(|p| p.stopped.is_some()));
    stopped().map_or(Ok(()), Err)
}

/// Starts the app, and restores the state a recovered panic handed the host.
pub(crate) fn start<A: App>(restore: Option<Vec<u8>>) -> Result<(), Stopped> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            panic_hook(info);
            previous(info);
        }));
    });
    PROGRAM.with_borrow_mut(|p| p.front_end = A::FRONT_END);
    coverage::start(A::BRANCHES);
    let started = start_app::<A>(restore);
    report_reached();
    started
}

fn start_app<A: App>(restore: Option<Vec<u8>>) -> Result<(), Stopped> {
    A::start(&Scope::root(A::NAME), &mut Grants(()))?;
    if let Some(state) = restore {
        let Recovery {
            mut fields,
            mut mailboxes,
        } = Recovery::read(&state).map_err(restore_failed)?;
        for actor in PROGRAM.with_borrow(|p| p.actors.clone()) {
            if let Some(saved) = fields.remove(actor.id()) {
                actor.restore(saved).map_err(restore_failed)?;
            }
            if let Some(messages) = mailboxes.remove(actor.id()) {
                actor.requeue(messages).map_err(restore_failed)?;
            }
        }
    }
    run_until_idle()
}

/// What a panic hands the host to start a fresh instance from, by each
/// component's diagram ID.
/// Written as `{"fields": {...}, "mailboxes": {...}}`.
#[derive(Default)]
struct Recovery {
    /// Each component's state fields, in their stored form.
    fields: BTreeMap<String, serde_json::Value>,
    /// The messages waiting in each mailbox that is not empty.
    mailboxes: BTreeMap<String, Vec<serde_json::Value>>,
}

impl Recovery {
    fn write(self) -> Vec<u8> {
        let mut json = serde_json::Map::new();
        json.insert("fields".into(), self.fields.into_iter().collect());
        if !self.mailboxes.is_empty() {
            let mailboxes = self
                .mailboxes
                .into_iter()
                .map(|(id, messages)| (id, serde_json::Value::Array(messages)));
            json.insert("mailboxes".into(), mailboxes.collect());
        }
        serde_json::Value::Object(json).to_string().into_bytes()
    }

    fn read(bytes: &[u8]) -> Result<Recovery, String> {
        let json: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let object = |name: &str| match json.get(name) {
            None => Ok(serde_json::Map::new()),
            Some(serde_json::Value::Object(map)) => Ok(map.clone()),
            Some(_) => Err(format!("`{name}` is not an object")),
        };
        let mailboxes = object("mailboxes")?
            .into_iter()
            .map(|(id, messages)| match messages {
                serde_json::Value::Array(messages) => Ok((id, messages)),
                _ => Err(format!("the mailbox of {id} is not a list")),
            })
            .collect::<Result<_, String>>()?;
        Ok(Recovery {
            fields: object("fields")?.into_iter().collect(),
            mailboxes,
        })
    }
}

fn restore_failed(message: String) -> Stopped {
    Stopped {
        component: "start".into(),
        failure: "Restore".into(),
        message,
    }
}

/// Delivers a request, and runs until the program is idle again.
pub(crate) fn handle(server: &str, request: Request) -> Result<Reply, Stopped> {
    let handled = handle_request(server, request);
    report_reached();
    handled
}

/// Tells the host which elements have been reached since it was last told.
fn report_reached() {
    let Ok(actors) = PROGRAM.with(|p| p.try_borrow().map(|p| p.actors.clone())) else {
        return;
    };
    let mut reached = Vec::new();
    for actor in actors {
        actor.take_reached(&mut reached);
    }
    coverage::take_algorithms(&mut reached);
    if !reached.is_empty() {
        report::reached(&reached);
    }
}

fn handle_request(server: &str, request: Request) -> Result<Reply, Stopped> {
    if let Some(stopped) = stopped() {
        return Err(stopped);
    }
    // A `GET` that no route takes may be for a file of the front end.
    let file = || {
        let front_end = PROGRAM.with_borrow(|p| p.front_end)?;
        (request.method == "GET" && front_end.server == server)
            .then(|| front_end.file(&request.path))
            .flatten()
    };
    let Some((field, actor)) = PROGRAM.with_borrow(|p| p.servers.get(server).cloned()) else {
        return Ok(file().unwrap_or_else(|| Reply::empty(404)));
    };
    let reply = match actor.serve(field, &request) {
        Ok(reply) => reply,
        Err(answered) if answered.status == 404 => return Ok(file().unwrap_or(answered)),
        Err(answered) => return Ok(answered),
    };
    run_until_idle()?;
    match reply.try_take() {
        Ok(Some(reply)) => Ok(reply),
        // The handler failed and was rolled back.
        Ok(None) => Ok(Reply::empty(500)),
        Err(()) => unreachable!("an idle program has finished every handler"),
    }
}

pub(crate) fn report_rolled_back(component: &str, failure: &Failure, message: &str) {
    report::rolled_back(component, &failure.to_string(), message);
}

pub(crate) fn report_restarted(component: &str, failure: &Failure, message: &str, restarted: &str) {
    report::restarted(component, &failure.to_string(), message, restarted);
}

/// Runs before a panic traps the instance. If every handler the panic
/// interrupts rolls back, it hands the host the state to start again from.
fn panic_hook(info: &std::panic::PanicHookInfo<'_>) {
    let message = match info.payload_as_str() {
        Some(text) => text.to_string(),
        None => "panic".to_string(),
    };
    let failure = Failure::of_panic(&message);
    let message = match info.location() {
        Some(at) => format!("{message} at {}:{}", at.file(), at.line()),
        None => message,
    };
    let Ok(actors) = PROGRAM.with(|p| p.try_borrow().map(|p| p.actors.clone())) else {
        return report::trapping("start", &failure.to_string(), &message, None);
    };
    let panicked = actor::try_current().and_then(|scope| {
        actors
            .iter()
            .find(|a| a.running().is_some_and(|s| Rc::ptr_eq(&s, &scope)))
            .cloned()
    });
    let component = panicked.as_ref().map_or("start", |a| a.id()).to_string();
    let recovery = panicked.and_then(|panicked| recovery(&actors, &*panicked, &failure));
    // What ran before the panic, including the `failure` arms that answer it.
    report_reached();
    report::trapping(&component, &failure.to_string(), &message, recovery);
}

/// The state to start a fresh instance from, if the panic can be survived:
/// every running handler either panicked or awaits a reply from one that
/// did, and each is rolled back or restarted, and no message in a mailbox
/// has a sender waiting for its reply. With it, the component that a
/// `restart` of the panicked handler resets, if any.
fn recovery(
    actors: &[Rc<dyn Running>],
    panicked: &dyn Running,
    failure: &Failure,
) -> Option<(Vec<u8>, Option<String>)> {
    let mut restarted: Vec<String> = Vec::new();
    let mut panicked_restarts = None;
    for actor in actors {
        if let Some(scope) = actor.running() {
            let is_panicked = actor.id() == panicked.id();
            let condition = if is_panicked {
                failure.clone()
            } else {
                Failure::Message(scope.awaiting()?)
            };
            if let (Response::Restart, target) = actor::resolve(actor.clone(), &condition)? {
                if is_panicked {
                    panicked_restarts = Some(target.id().to_string());
                }
                restarted.push(target.id().to_string());
            }
        }
    }
    let mut recovery = Recovery::default();
    for actor in actors {
        let id = actor.id();
        let is_restarted = restarted.iter().any(|r| {
            id == r
                || id
                    .strip_prefix(r.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        recovery
            .fields
            .insert(id.to_string(), actor.snapshot(is_restarted)?);
        let queued = actor.queued()?;
        if !queued.is_empty() {
            recovery.mailboxes.insert(id.to_string(), queued);
        }
    }
    Some((recovery.write(), panicked_restarts))
}

/// Reports to the host. Outside WebAssembly, where the runtime's own tests
/// run, reports are kept for the tests to read.
#[cfg(target_family = "wasm")]
mod report {
    use crate::bindings::inset::host::report;

    pub(super) fn rolled_back(component: &str, failure: &str, message: &str) {
        report::rolled_back(component, failure, message);
    }

    pub(super) fn restarted(component: &str, failure: &str, message: &str, restarted: &str) {
        report::restarted(component, failure, message, restarted);
    }

    pub(super) fn reached(elements: &[String]) {
        report::reached(elements);
    }

    pub(super) fn trapping(
        component: &str,
        failure: &str,
        message: &str,
        recovery: Option<(Vec<u8>, Option<String>)>,
    ) {
        let recovery = recovery.map(|(state, restarted)| report::Recovery { state, restarted });
        report::trapping(component, failure, message, recovery.as_ref());
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) mod report {
    use std::cell::RefCell;

    thread_local! {
        pub(crate) static REPORTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        pub(crate) static REACHED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn reached(elements: &[String]) {
        REACHED.with_borrow_mut(|r| r.extend_from_slice(elements));
    }

    pub(super) fn rolled_back(component: &str, failure: &str, message: &str) {
        REPORTS
            .with_borrow_mut(|r| r.push(format!("{component} rolled back {failure}: {message}")));
    }

    pub(super) fn restarted(component: &str, failure: &str, message: &str, restarted: &str) {
        REPORTS.with_borrow_mut(|r| {
            r.push(format!(
                "{restarted} restarted after {component} failed with {failure}: {message}"
            ))
        });
    }

    pub(super) fn trapping(
        component: &str,
        failure: &str,
        message: &str,
        recovery: Option<(Vec<u8>, Option<String>)>,
    ) {
        let recovery = recovery.map(|(state, _)| String::from_utf8(state).expect("JSON"));
        REPORTS.with_borrow_mut(|r| {
            r.push(format!(
                "{component} trapping {failure}: {message}; recovery {recovery:?}"
            ))
        });
    }
}

/// Converts between the runtime's types and the generated bindings, for [`crate::export!`].
pub mod glue {
    use crate::bindings::exports::inset::host::app as wit;

    use super::{App, GrantValue, SettingType};
    use crate::failure::Stopped;
    use crate::http::Request;

    pub fn describe<A: App>() -> wit::Manifest {
        wit::Manifest {
            app: A::NAME.into(),
            settings: A::SETTINGS
                .iter()
                .map(|s| wit::Setting {
                    name: s.name.into(),
                    type_: setting_type(s.ty),
                    default: s.default.into(),
                })
                .collect(),
            grants: A::GRANTS
                .iter()
                .map(|g| wit::Grant {
                    name: g.name.into(),
                    capability: g.capability.into(),
                    fields: g
                        .fields
                        .iter()
                        .map(|(name, value)| wit::GrantField {
                            name: (*name).into(),
                            value: match value {
                                GrantValue::Literal(v) => wit::GrantValue::Literal((*v).into()),
                                GrantValue::Setting(s) => wit::GrantValue::Setting((*s).into()),
                            },
                        })
                        .collect(),
                })
                .collect(),
            externals: A::EXTERNALS.iter().map(|&name| name.into()).collect(),
        }
    }

    fn setting_type(ty: SettingType) -> wit::SettingType {
        match ty {
            SettingType::Bool => wit::SettingType::Bool,
            SettingType::U8 => wit::SettingType::U8,
            SettingType::U16 => wit::SettingType::U16,
            SettingType::U32 => wit::SettingType::U32,
            SettingType::U64 => wit::SettingType::U64,
            SettingType::I8 => wit::SettingType::I8,
            SettingType::I16 => wit::SettingType::I16,
            SettingType::I32 => wit::SettingType::I32,
            SettingType::I64 => wit::SettingType::I64,
            SettingType::String => wit::SettingType::String,
        }
    }

    fn stopped(stopped: Stopped) -> wit::Stopped {
        wit::Stopped {
            component: stopped.component,
            failure: stopped.failure,
            message: stopped.message,
        }
    }

    pub fn start<A: App>(restore: Option<Vec<u8>>) -> Result<(), wit::Stopped> {
        super::start::<A>(restore).map_err(stopped)
    }

    pub fn handle(server: String, request: wit::Request) -> Result<wit::Response, wit::Stopped> {
        let request = Request {
            method: request.method,
            path: request.path,
            body: request.body,
        };
        let reply = super::handle(&server, request).map_err(stopped)?;
        Ok(wit::Response {
            status: reply.status,
            content_type: reply.content_type.map(Into::into),
            body: reply.body,
        })
    }
}
