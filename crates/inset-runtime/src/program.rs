//! The program as the host sees it: its manifest, its start, and requests in.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Once;

use crate::actor::{self, Running, Scope};
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

    /// The app's `start` statements.
    fn start(scope: &Scope, grants: &mut Grants) -> Result<(), Stopped>;
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
    A::start(&Scope::root(A::NAME), &mut Grants(()))?;
    if let Some(state) = restore {
        let mut state: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&state).map_err(|e| restore_failed(e.to_string()))?;
        for actor in PROGRAM.with_borrow(|p| p.actors.clone()) {
            if let Some(saved) = state.remove(actor.id()) {
                actor.restore(saved).map_err(restore_failed)?;
            }
        }
    }
    run_until_idle()
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
    if let Some(stopped) = stopped() {
        return Err(stopped);
    }
    let Some((field, actor)) = PROGRAM.with_borrow(|p| p.servers.get(server).cloned()) else {
        return Ok(Reply::empty(404));
    };
    let reply = match actor.serve(field, &request) {
        Ok(reply) => reply,
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
    report::trapping(&component, &failure.to_string(), &message, recovery);
}

/// The state to start a fresh instance from, if the panic can be survived:
/// nothing is waiting in a mailbox, and every running handler either
/// panicked or awaits a reply from one that did, and rolls back.
fn recovery(
    actors: &[Rc<dyn Running>],
    panicked: &dyn Running,
    failure: &Failure,
) -> Option<Vec<u8>> {
    let mut state = BTreeMap::new();
    for actor in actors {
        if actor.queued() > 0 {
            return None;
        }
        if let Some(scope) = actor.running() {
            let condition = if actor.id() == panicked.id() {
                failure.clone()
            } else {
                Failure::Message(scope.awaiting()?)
            };
            if actor.responds(&condition) != Some(Response::Rollback) {
                return None;
            }
        }
        state.insert(actor.id().to_string(), actor.snapshot()?);
    }
    serde_json::to_vec(&state).ok()
}

/// Reports to the host. Outside WebAssembly, where the runtime's own tests
/// run, reports are kept for the tests to read.
#[cfg(target_family = "wasm")]
mod report {
    use crate::bindings::inset::host::report;

    pub(super) fn rolled_back(component: &str, failure: &str, message: &str) {
        report::rolled_back(component, failure, message);
    }

    pub(super) fn trapping(
        component: &str,
        failure: &str,
        message: &str,
        recovery: Option<Vec<u8>>,
    ) {
        report::trapping(component, failure, message, recovery.as_deref());
    }
}

#[cfg(not(target_family = "wasm"))]
pub(crate) mod report {
    use std::cell::RefCell;

    thread_local! {
        pub(crate) static REPORTS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn rolled_back(component: &str, failure: &str, message: &str) {
        REPORTS
            .with_borrow_mut(|r| r.push(format!("{component} rolled back {failure}: {message}")));
    }

    pub(super) fn trapping(
        component: &str,
        failure: &str,
        message: &str,
        recovery: Option<Vec<u8>>,
    ) {
        let recovery = recovery.map(|state| String::from_utf8(state).expect("JSON"));
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
