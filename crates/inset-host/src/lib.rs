//! The WebAssembly host that runs a built Inset program.
//!
//! A built program is one WebAssembly component of the `program` world in
//! `crates/inset-runtime/wit/inset.wit`. The host learns what the program
//! needs from its manifest, applies `--set` overrides to its settings,
//! resolves its grants, and then runs one long-lived instance of it, handing
//! it one request at a time. `docs/language/runtime.md` records why.
//!
//! **Capabilities.** A program gets only what its grants name. Rust's
//! standard library imports WASI's command interfaces whatever the program
//! does, so they are linked with nothing behind them: no files, no network,
//! no environment, a clock fixed at zero and random numbers from a fixed
//! seed. A call to any of them is refused, or answers the same every time, so
//! a requirement test is repeatable. An import the host does not link at all,
//! such as `wasi:http/outgoing-handler`, fails at instantiation.
//!
//! **Fakes.** [`Mode::Fake`] puts a fake behind every grant, which is how
//! requirement tests run: requests go straight to the program through
//! [`Running::request`] instead of a socket, and each table is held in memory,
//! where a test can set and read it.
//!
//! **Failures.** A panic traps the instance. If the program's panic hook
//! handed over the state to recover from, the host rolls back the tables'
//! uncommitted writes, starts a fresh instance from that state and answers
//! the request with `500 Internal Server Error`. Otherwise the program stops.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rand::SeedableRng;
use rand::rngs::StdRng;
use wasmtime::component::{Component, HasSelf, Linker, Resource, ResourceTable};
use wasmtime::{Engine, Store};
use wasmtime_wasi::clocks::{HostMonotonicClock, HostWallClock};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

mod server;

pub use server::serve;

mod bindings {
    wasmtime::component::bindgen!({
        path: "../inset-runtime/wit",
        world: "program",
        with: { "inset:host/capabilities.table": super::OpenTable },
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::exports::inset::host::app;
use bindings::inset::host::{capabilities, report};

pub use app::{Grant, GrantValue, Manifest, Request, Response, Setting, SettingType, Stopped};

/// An error loading or starting a program.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<wasmtime::Error> for Error {
    fn from(e: wasmtime::Error) -> Error {
        Error(format!("{e:?}"))
    }
}

/// A built program, compiled and ready to start.
pub struct Program {
    engine: Engine,
    pre: bindings::ProgramPre<State>,
    manifest: Manifest,
}

impl Program {
    /// Compiles the component at `path` and reads its manifest. Compiled
    /// code is cached, so a program loads quickly the second time.
    pub fn load(path: &Path) -> Result<Program, Error> {
        Program::compile(path, true)
    }

    /// Loads without the cache, as for a program built a moment ago.
    pub fn load_uncached(path: &Path) -> Result<Program, Error> {
        Program::compile(path, false)
    }

    fn compile(path: &Path, cached: bool) -> Result<Program, Error> {
        let mut config = wasmtime::Config::new();
        if let Some(cache) = cached
            .then(|| wasmtime::Cache::from_file(None).ok())
            .flatten()
        {
            config.cache(Some(cache));
        }
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, path)
            .map_err(|e| Error(format!("cannot load {}: {e:?}", path.display())))?;
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Program::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)?;
        let pre = bindings::ProgramPre::new(linker.instantiate_pre(&component)?)?;
        let mut store = Store::new(&engine, State::new(BTreeMap::new()));
        let manifest = pre
            .instantiate(&mut store)?
            .inset_host_app()
            .call_describe(&mut store)?;
        Ok(Program {
            engine,
            pre,
            manifest,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Resolves the settings and grants, and starts the program.
    pub fn start(&self, options: &Options) -> Result<Running<'_>, Error> {
        let settings = resolve_settings(&self.manifest, &options.settings)?;
        let mut grants = BTreeMap::new();
        let mut tables = BTreeMap::new();
        for grant in &self.manifest.grants {
            let granted = resolve_grant(grant, &settings, &options.working_dir)?;
            if let Granted::Table { .. } = granted {
                if options.mode == Mode::Real {
                    return Err(Error(format!(
                        "grant `{}`: stored tables are not supported yet, only fakes",
                        grant.name
                    )));
                }
                tables.insert(grant.name.clone(), Arc::default());
            }
            grants.insert(grant.name.clone(), granted);
        }
        let (store, instance) = self.instantiate(&tables, Vec::new(), None)?;
        Ok(Running {
            program: self,
            store,
            instance,
            grants,
            tables,
            stopped: None,
        })
    }

    fn instantiate(
        &self,
        tables: &BTreeMap<String, Arc<Mutex<FakeTable>>>,
        reports: Vec<Report>,
        restore: Option<&[u8]>,
    ) -> Result<(Store<State>, bindings::Program), Error> {
        let mut state = State::new(tables.clone());
        state.reports = reports;
        let mut store = Store::new(&self.engine, state);
        let instance = self.pre.instantiate(&mut store)?;
        if let Err(stopped) = instance.inset_host_app().call_start(&mut store, restore)? {
            return Err(Error(format!(
                "the program stopped as it started: {}",
                describe(&stopped)
            )));
        }
        Ok((store, instance))
    }
}

/// How to start a program.
#[derive(Clone, Debug)]
pub struct Options {
    /// `--set name=value` overrides.
    pub settings: Vec<(String, String)>,
    pub mode: Mode,
    /// Where relative file paths in grants are resolved: the directory the
    /// host was started in, as for any command-line tool.
    pub working_dir: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Real capabilities: HTTP servers listen on their ports.
    Real,
    /// A fake behind every grant, for requirement tests.
    Fake,
}

/// A grant with its values resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Granted {
    HttpServer { port: u16 },
    Table { file: PathBuf, name: String },
}

/// Applies `--set` overrides to the settings' defaults, checking each value
/// against its setting's type.
pub fn resolve_settings(
    manifest: &Manifest,
    overrides: &[(String, String)],
) -> Result<BTreeMap<String, String>, Error> {
    let mut values: BTreeMap<String, String> = manifest
        .settings
        .iter()
        .map(|s| (s.name.clone(), s.default.clone()))
        .collect();
    for (name, value) in overrides {
        let Some(setting) = manifest.settings.iter().find(|s| &s.name == name) else {
            return Err(Error(format!("{} has no setting `{name}`", manifest.app)));
        };
        if !fits(setting.type_, value) {
            return Err(Error(format!(
                "setting `{name}` is {}, so it cannot be `{value}`",
                type_name(setting.type_)
            )));
        }
        values.insert(name.clone(), value.clone());
    }
    Ok(values)
}

fn fits(ty: SettingType, value: &str) -> bool {
    match ty {
        SettingType::Bool => value.parse::<bool>().is_ok(),
        SettingType::U8 => value.parse::<u8>().is_ok(),
        SettingType::U16 => value.parse::<u16>().is_ok(),
        SettingType::U32 => value.parse::<u32>().is_ok(),
        SettingType::U64 => value.parse::<u64>().is_ok(),
        SettingType::I8 => value.parse::<i8>().is_ok(),
        SettingType::I16 => value.parse::<i16>().is_ok(),
        SettingType::I32 => value.parse::<i32>().is_ok(),
        SettingType::I64 => value.parse::<i64>().is_ok(),
        SettingType::String => true,
    }
}

fn type_name(ty: SettingType) -> &'static str {
    match ty {
        SettingType::Bool => "a bool",
        SettingType::U8 => "a u8",
        SettingType::U16 => "a u16",
        SettingType::U32 => "a u32",
        SettingType::U64 => "a u64",
        SettingType::I8 => "an i8",
        SettingType::I16 => "an i16",
        SettingType::I32 => "an i32",
        SettingType::I64 => "an i64",
        SettingType::String => "a String",
    }
}

/// Resolves a grant's fields. A relative file path is resolved against
/// `working_dir`.
pub fn resolve_grant(
    grant: &Grant,
    settings: &BTreeMap<String, String>,
    working_dir: &Path,
) -> Result<Granted, Error> {
    let field = |name: &str| -> Result<&str, Error> {
        let value = grant
            .fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| Error(format!("grant `{}` has no field `{name}`", grant.name)))?;
        match &value.value {
            GrantValue::Literal(literal) => Ok(literal),
            GrantValue::Setting(setting) => {
                settings.get(setting).map(String::as_str).ok_or_else(|| {
                    Error(format!(
                        "grant `{}` names no setting `{setting}`",
                        grant.name
                    ))
                })
            }
        }
    };
    if grant.capability == "HttpServer" {
        let port = field("port")?;
        let port = port
            .parse()
            .map_err(|_| Error(format!("grant `{}`: `{port}` is not a port", grant.name)))?;
        Ok(Granted::HttpServer { port })
    } else if grant.capability.starts_with("Table<") {
        Ok(Granted::Table {
            file: working_dir.join(field("file")?),
            name: field("name")?.to_string(),
        })
    } else {
        Err(Error(format!(
            "grant `{}`: the host grants no capability `{}`",
            grant.name, grant.capability
        )))
    }
}

/// A running program.
pub struct Running<'p> {
    program: &'p Program,
    store: Store<State>,
    instance: bindings::Program,
    grants: BTreeMap<String, Granted>,
    tables: BTreeMap<String, Arc<Mutex<FakeTable>>>,
    stopped: Option<Stopped>,
}

impl Running<'_> {
    pub fn grants(&self) -> &BTreeMap<String, Granted> {
        &self.grants
    }

    /// The fake behind a table grant.
    pub fn table(&self, grant: &str) -> Option<MutexGuard<'_, FakeTable>> {
        Some(lock(self.tables.get(grant)?))
    }

    /// What the program has reported so far.
    pub fn reports(&self) -> &[Report] {
        &self.store.data().reports
    }

    /// Delivers a request through the `HttpServer` grant `server`, and
    /// returns once the program is idle again.
    pub fn request(&mut self, server: &str, request: &Request) -> Result<Response, Stopped> {
        if let Some(stopped) = &self.stopped {
            return Err(stopped.clone());
        }
        let result = self
            .instance
            .inset_host_app()
            .call_handle(&mut self.store, server, request);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(trap) => self.recover(trap),
        };
        if let Err(stopped) = &outcome {
            self.stopped = Some(stopped.clone());
        }
        outcome
    }

    /// Starts a fresh instance after a trap, if the program said how.
    fn recover(&mut self, trap: wasmtime::Error) -> Result<Response, Stopped> {
        for table in self.tables.values() {
            lock(table).roll_back();
        }
        let state = self.store.data_mut();
        let Some(trapping) = state.trapping.take() else {
            return Err(Stopped {
                component: self.program.manifest.app.clone(),
                failure: "Trap".into(),
                message: format!("{trap:?}"),
            });
        };
        let Some(recovery) = trapping.recovery else {
            return Err(Stopped {
                component: trapping.component,
                failure: trapping.failure,
                message: trapping.message,
            });
        };
        let mut reports = std::mem::take(&mut state.reports);
        reports.push(Report {
            component: trapping.component.clone(),
            failure: trapping.failure.clone(),
            message: trapping.message.clone(),
        });
        match self
            .program
            .instantiate(&self.tables, reports, Some(&recovery))
        {
            Ok((store, instance)) => {
                self.store = store;
                self.instance = instance;
                Ok(Response {
                    status: 500,
                    content_type: None,
                    body: Vec::new(),
                })
            }
            Err(e) => Err(Stopped {
                component: trapping.component,
                failure: trapping.failure,
                message: format!("{}; then it could not start again: {e}", trapping.message),
            }),
        }
    }
}

/// A handler the program rolled back and carried on after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub component: String,
    pub failure: String,
    pub message: String,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} rolled back after {}: {}",
            self.component, self.failure, self.message
        )
    }
}

/// Describes why a program stopped.
pub fn describe(stopped: &Stopped) -> String {
    format!(
        "{} failed with {}: {}",
        stopped.component, stopped.failure, stopped.message
    )
}

/// A table held in memory. Writes wait in a transaction until the handler
/// that made them completes.
#[derive(Debug, Default)]
pub struct FakeTable {
    committed: BTreeMap<Vec<u8>, Vec<u8>>,
    pending: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Every call fails, as in the requirement fact `name fails`.
    pub fails: bool,
}

impl FakeTable {
    /// Sets an entry, as `given name[key] is value` does. Keys and values
    /// are the program's encoding, JSON.
    pub fn set(&mut self, key: &str, value: &str) {
        self.committed.insert(key.into(), value.into());
    }

    /// The committed entry under `key`, as `then name[key] is value` reads it.
    pub fn get(&self, key: &str) -> Option<String> {
        let value = self.committed.get(key.as_bytes())?;
        Some(String::from_utf8_lossy(value).into_owned())
    }

    pub fn is_empty(&self) -> bool {
        self.committed.is_empty()
    }

    fn roll_back(&mut self) {
        self.pending.clear();
    }

    fn check(&self) -> Result<(), String> {
        match self.fails {
            true => Err("the table failed, as the test asked".into()),
            false => Ok(()),
        }
    }
}

fn lock(table: &Mutex<FakeTable>) -> MutexGuard<'_, FakeTable> {
    table.lock().unwrap_or_else(|e| e.into_inner())
}

/// A table the program opened, as a resource in its instance.
pub struct OpenTable(Arc<Mutex<FakeTable>>);

/// What a panic hook handed over before the instance trapped.
struct Trapping {
    component: String,
    failure: String,
    message: String,
    recovery: Option<Vec<u8>>,
}

/// One instance's state in the store.
struct State {
    wasi: WasiCtx,
    resources: ResourceTable,
    tables: BTreeMap<String, Arc<Mutex<FakeTable>>>,
    reports: Vec<Report>,
    trapping: Option<Trapping>,
}

impl State {
    fn new(tables: BTreeMap<String, Arc<Mutex<FakeTable>>>) -> State {
        let wasi = WasiCtx::builder()
            .wall_clock(FixedClock)
            .monotonic_clock(FixedClock)
            .secure_random(StdRng::seed_from_u64(0))
            .insecure_random(StdRng::seed_from_u64(0))
            .insecure_random_seed(0)
            .allow_tcp(false)
            .allow_udp(false)
            .build();
        State {
            wasi,
            resources: ResourceTable::new(),
            tables,
            reports: Vec::new(),
            trapping: None,
        }
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

/// WASI's clocks, stopped at zero. Time is an effect, which a program will
/// reach through a capability of its own.
struct FixedClock;

impl HostWallClock for FixedClock {
    fn resolution(&self) -> Duration {
        Duration::from_nanos(1)
    }

    fn now(&self) -> Duration {
        Duration::ZERO
    }
}

impl HostMonotonicClock for FixedClock {
    fn resolution(&self) -> u64 {
        1
    }

    fn now(&self) -> u64 {
        0
    }
}

impl capabilities::Host for State {
    fn open_table(&mut self, grant: String) -> Result<Resource<OpenTable>, String> {
        let Some(table) = self.tables.get(&grant) else {
            return Err(format!("no table was granted as `{grant}`"));
        };
        let open = OpenTable(table.clone());
        self.resources.push(open).map_err(|e| e.to_string())
    }
}

impl State {
    fn open(&self, table: &Resource<OpenTable>) -> MutexGuard<'_, FakeTable> {
        let open = self
            .resources
            .get(table)
            .expect("the program holds the table it uses");
        lock(&open.0)
    }
}

impl capabilities::HostTable for State {
    fn get(&mut self, table: Resource<OpenTable>, key: Vec<u8>) -> Result<Option<Vec<u8>>, String> {
        let table = self.open(&table);
        table.check()?;
        Ok(table
            .pending
            .get(&key)
            .or_else(|| table.committed.get(&key))
            .cloned())
    }

    fn insert(
        &mut self,
        table: Resource<OpenTable>,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> Result<(), String> {
        let mut table = self.open(&table);
        table.check()?;
        table.pending.insert(key, value);
        Ok(())
    }

    fn commit(&mut self, table: Resource<OpenTable>) {
        let mut table = self.open(&table);
        let pending = std::mem::take(&mut table.pending);
        table.committed.extend(pending);
    }

    fn rollback(&mut self, table: Resource<OpenTable>) {
        self.open(&table).roll_back();
    }

    fn drop(&mut self, table: Resource<OpenTable>) -> wasmtime::Result<()> {
        self.resources.delete(table)?;
        Ok(())
    }
}

impl report::Host for State {
    fn rolled_back(&mut self, component: String, failure: String, message: String) {
        self.reports.push(Report {
            component,
            failure,
            message,
        });
    }

    fn trapping(
        &mut self,
        component: String,
        failure: String,
        message: String,
        recovery: Option<Vec<u8>>,
    ) {
        self.trapping = Some(Trapping {
            component,
            failure,
            message,
            recovery,
        });
    }
}
