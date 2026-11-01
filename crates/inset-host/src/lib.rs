//! The WebAssembly host that runs a built Inset program.
//!
//! A built program is one WebAssembly component of the `program` world in
//! `crates/inset-runtime/wit/inset.wit`. The host learns what the program
//! needs from its manifest, applies `--set` overrides to its settings,
//! resolves its grants, and then runs one long-lived instance of it,
//! delivering each request as it arrives. The program admits a request when
//! no request it holds has reserved a component the new one can reach, and
//! hands the host the calls it makes through its `HttpClient` grants, which
//! the host makes and hands back while the program goes on with other
//! requests ([`Running::deliver`], [`Running::resume`]).
//! `docs/language/runtime.md` records why.
//!
//! **Capabilities.** A program gets only what its grants name. Rust's
//! standard library imports WASI's command interfaces whatever the program
//! does, so they are linked with nothing behind them: no files, no network,
//! no environment, a clock fixed at zero and random numbers from a fixed
//! seed. A call to any of them is refused, or answers the same every time, so
//! a requirement test is repeatable. An import the host does not link at all,
//! such as `wasi:http/outgoing-handler`, fails at instantiation.
//!
//! **Tables.** A `Table` grant is a table in an SQLite database file, opened
//! by the host, which the program knows only by its grant's name. The host
//! refuses to start a program when a table holds an entry that is not of the
//! table's key and value types, naming the file and the table. Writes are
//! held until the handler that made them completes, and a commit writes them
//! in one SQLite transaction (`table.rs`).
//!
//! **Fakes.** [`Mode::Fake`] puts a fake behind every grant, which is how
//! requirement tests run: requests go straight to the program through
//! [`Running::request`] instead of a socket, each table is held in memory,
//! where a test can set and read it, and each `HttpClient` answers from the
//! replies the test set up ([`FakeClient`]).
//!
//! **Failures.** A panic traps the instance. If the program's panic hook
//! handed over the state to recover from, the host rolls back the tables'
//! uncommitted writes, starts a fresh instance from that state and answers
//! every request in flight with `500 Internal Server Error`. Otherwise the
//! program stops.

use std::collections::{BTreeMap, BTreeSet};
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

mod client;
mod external;
mod server;
mod table;

pub use client::FakeClient;
pub use server::serve;
pub use table::{Entries, Table};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../inset-runtime/wit",
        world: "program",
        with: { "inset:host/capabilities.table": super::OpenTable },
        imports: { default: trappable },
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::exports::inset::host::app;
use bindings::inset::host::{capabilities, externals, report};
use external::Externals;

pub use app::{
    Completion, Grant, GrantValue, Incoming, Manifest, Outgoing, Request, Response, Setting,
    SettingType, Stopped,
};

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

/// A built program, compiled and ready to start, with its external components.
pub struct Program {
    engine: Engine,
    pre: bindings::ProgramPre<State>,
    manifest: Manifest,
    /// The external components the manifest names, loaded from
    /// `externals/` beside the program's component. Each start of the
    /// program runs instances of its own.
    externals: Externals,
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
        let dir = path.parent().unwrap_or(Path::new(".")).join("externals");
        let externals = Externals::load(&engine, &dir, &manifest.externals)?;
        Ok(Program {
            engine,
            pre,
            manifest,
            externals,
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
            if let Granted::Table { file, name } = &granted {
                let table = match options.mode {
                    Mode::Fake => Table::fake(),
                    Mode::Real => Table::open(file, name).map_err(|e| {
                        Error(format!(
                            "cannot open the table `{name}` in {} for the grant `{}`: {e}",
                            file.display(),
                            grant.name
                        ))
                    })?,
                };
                tables.insert(grant.name.clone(), Arc::new(Mutex::new(table)));
            }
            grants.insert(grant.name.clone(), granted);
        }
        let clients = grants
            .iter()
            .filter(|(_, g)| matches!(g, Granted::HttpClient { .. }))
            .map(|(name, _)| (name.clone(), FakeClient::default()))
            .collect();
        let externals = Arc::new(self.externals.fresh());
        let (store, instance) = self
            .instantiate(&tables, &externals, Vec::new(), BTreeSet::new(), None)
            .map_err(|e| match e {
                Started::Failed(e) => e,
                Started::Stopped(stopped) => self.refused(&stopped, &grants),
            })?;
        Ok(Running {
            program: self,
            store,
            instance,
            grants,
            tables,
            externals,
            mode: options.mode,
            clients,
            stopped: None,
            next_request: 0,
            in_flight: BTreeSet::new(),
            next_call: 0,
            calls: BTreeMap::new(),
        })
    }

    fn instantiate(
        &self,
        tables: &BTreeMap<String, Arc<Mutex<Table>>>,
        externals: &Arc<Externals>,
        reports: Vec<Report>,
        reached: BTreeSet<String>,
        restore: Option<&[u8]>,
    ) -> Result<(Store<State>, bindings::Program), Started> {
        let mut state = State::new(tables.clone());
        state.externals = Some(externals.clone());
        state.reports = reports;
        state.reached = reached;
        let mut store = Store::new(&self.engine, state);
        let failed = |e: wasmtime::Error| Started::Failed(e.into());
        let instance = self.pre.instantiate(&mut store).map_err(failed)?;
        if let Err(stopped) = instance
            .inset_host_app()
            .call_start(&mut store, restore)
            .map_err(failed)?
        {
            return Err(Started::Stopped(stopped));
        }
        Ok((store, instance))
    }

    /// Why the program refused to start. A table that holds an entry of
    /// the wrong type is named by its file and table.
    fn refused(&self, stopped: &Stopped, grants: &BTreeMap<String, Granted>) -> Error {
        let table = stopped
            .component
            .strip_prefix("grant:")
            .and_then(|grant| Some((grant, grants.get(grant)?)));
        match table {
            Some((grant, Granted::Table { file, name })) => {
                let capability = self
                    .manifest
                    .grants
                    .iter()
                    .find(|g| g.name == grant)
                    .map_or("Table", |g| g.capability.as_str());
                Error(format!(
                    "cannot use the table `{name}` in {} for the grant `{grant}: {capability}`: {}",
                    file.display(),
                    stopped.message
                ))
            }
            _ => Error(format!(
                "the program stopped as it started: {}",
                describe(stopped)
            )),
        }
    }
}

/// Why an instance did not start.
enum Started {
    Failed(Error),
    Stopped(Stopped),
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
    HttpServer {
        port: u16,
    },
    /// Calls to the service at `base`, such as `http://127.0.0.1:9000`.
    HttpClient {
        base: String,
    },
    Table {
        file: PathBuf,
        name: String,
    },
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
    } else if grant.capability == "HttpClient" {
        let base = client::check_base(field("base")?)
            .map_err(|e| Error(format!("grant `{}`: {e}", grant.name)))?;
        Ok(Granted::HttpClient { base })
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
    tables: BTreeMap<String, Arc<Mutex<Table>>>,
    /// The program's running external components, which a fresh instance
    /// after a panic keeps.
    externals: Arc<Externals>,
    mode: Mode,
    /// The fake behind each `HttpClient` grant, in [`Mode::Fake`].
    clients: BTreeMap<String, FakeClient>,
    stopped: Option<Stopped>,
    next_request: u64,
    /// The requests delivered and not yet answered, by ID.
    in_flight: BTreeSet<u64>,
    next_call: u64,
    /// The outgoing calls the program waits for, by the ID the host gave
    /// each one, with the ID the instance gave it. A fresh instance after a
    /// panic waits for none, so a late completion is dropped.
    calls: BTreeMap<u64, u64>,
}

/// What the program has done by the time it is idle again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The requests that finished, by the ID [`Running::deliver`] gave each one.
    pub replies: Vec<(u64, Response)>,
    /// The outgoing calls the program now waits for, each with an ID of the
    /// host's, which [`Running::resume`] takes back with its result.
    pub calls: Vec<Outgoing>,
}

impl Running<'_> {
    pub fn grants(&self) -> &BTreeMap<String, Granted> {
        &self.grants
    }

    /// The table behind a grant.
    pub fn table(&self, grant: &str) -> Option<MutexGuard<'_, Table>> {
        Some(lock(self.tables.get(grant)?))
    }

    /// What the program has reported so far.
    pub fn reports(&self) -> &[Report] {
        &self.store.data().reports
    }

    /// The diagram elements the program has reached since it started, by
    /// ID: functions entered, branches taken, error paths of `?` and arms of
    /// `failure` blocks. A fresh instance started after a panic adds to them.
    pub fn reached(&self) -> &BTreeSet<String> {
        &self.store.data().reached
    }

    /// The fake behind an `HttpClient` grant, in [`Mode::Fake`].
    pub fn client(&mut self, grant: &str) -> Option<&mut FakeClient> {
        self.clients.get_mut(grant)
    }

    /// Delivers a request through the `HttpServer` grant `server`, and
    /// waits for its reply, making the calls it leads to: through the fakes
    /// in [`Mode::Fake`], and for real otherwise. Requests delivered with
    /// [`Running::deliver`] and not yet answered may finish meanwhile.
    pub fn request(&mut self, server: &str, request: &Request) -> Result<Response, Stopped> {
        let (id, mut progress) = self.deliver(server, request)?;
        loop {
            if let Some((_, response)) = progress.replies.iter().find(|(i, _)| *i == id) {
                return Ok(response.clone());
            }
            if progress.calls.is_empty() {
                // Another request holds what this one needs, and waits on a
                // call the caller has not completed.
                return Err(Stopped {
                    component: self.program.manifest.app.clone(),
                    failure: "Waiting".into(),
                    message: "the request waits behind another that waits on a call".into(),
                });
            }
            let completions = progress
                .calls
                .iter()
                .map(|call| Completion {
                    id: call.id,
                    outcome: self
                        .make(call)
                        .map(|(status, body)| Incoming { status, body }),
                })
                .collect();
            progress = self.resume(completions)?;
        }
    }

    /// Makes an outgoing call: through its fake in [`Mode::Fake`], and for
    /// real otherwise, waiting for the reply.
    pub fn make(&mut self, call: &Outgoing) -> Result<(u16, Vec<u8>), String> {
        match self.mode {
            Mode::Fake => match self.clients.get_mut(&call.grant) {
                Some(fake) => fake.answer(&call.method, &call.path, &call.grant),
                None => Err(format!("the program has no client `{}`", call.grant)),
            },
            Mode::Real => {
                let base = self.base(&call.grant)?;
                tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .build()
                    .map_err(|e| e.to_string())?
                    .block_on(client::call(
                        &base,
                        &call.method,
                        &call.path,
                        call.body.clone(),
                    ))
            }
        }
    }

    /// The base address of an `HttpClient` grant.
    pub(crate) fn base(&self, grant: &str) -> Result<String, String> {
        match self.grants.get(grant) {
            Some(Granted::HttpClient { base }) => Ok(base.clone()),
            _ => Err(format!("the program has no client `{grant}`")),
        }
    }

    /// Delivers a request through the `HttpServer` grant `server`, and
    /// returns its ID with what the program has done once it is idle again.
    /// The reply may come in this progress or a later one.
    pub fn deliver(&mut self, server: &str, request: &Request) -> Result<(u64, Progress), Stopped> {
        let id = self.next_request;
        self.next_request += 1;
        self.in_flight.insert(id);
        let result =
            self.instance
                .inset_host_app()
                .call_deliver(&mut self.store, id, server, request);
        self.progress(result).map(|progress| (id, progress))
    }

    /// Hands the program the outgoing calls that have completed, and returns
    /// what it has done once it is idle again. A completion for a call the
    /// program no longer waits for is dropped.
    pub fn resume(&mut self, completions: Vec<Completion>) -> Result<Progress, Stopped> {
        let completions: Vec<Completion> = completions
            .into_iter()
            .filter_map(|c| {
                let id = self.calls.remove(&c.id)?;
                Some(Completion { id, ..c })
            })
            .collect();
        let result = self
            .instance
            .inset_host_app()
            .call_resume(&mut self.store, &completions);
        self.progress(result)
    }

    /// The progress an export returned, with the host's IDs for its calls,
    /// after a trap the recovery, and after a failed commit the stop.
    fn progress(
        &mut self,
        result: wasmtime::Result<Result<app::Progress, Stopped>>,
    ) -> Result<Progress, Stopped> {
        if let Some(stopped) = &self.stopped {
            return Err(stopped.clone());
        }
        let mut outcome = match result {
            Ok(Ok(progress)) => Ok(self.host_ids(progress)),
            Ok(Err(stopped)) => Err(stopped),
            Err(trap) => self.recover(trap),
        };
        for (grant, table) in &self.tables {
            if let Some(error) = lock(table).commit_error.take() {
                let at = match lock(table).location() {
                    Some((file, name)) => format!(" `{name}` in {}", file.display()),
                    None => String::new(),
                };
                outcome = Err(Stopped {
                    component: format!("grant:{grant}"),
                    failure: "Commit".into(),
                    message: format!("cannot write to the table{at}: {error}"),
                });
            }
        }
        match &outcome {
            Ok(progress) => {
                for (id, _) in &progress.replies {
                    self.in_flight.remove(id);
                }
            }
            Err(stopped) => {
                self.stopped = Some(stopped.clone());
                self.in_flight.clear();
            }
        }
        outcome
    }

    fn host_ids(&mut self, progress: app::Progress) -> Progress {
        let calls = progress
            .calls
            .into_iter()
            .map(|call| {
                let id = self.next_call;
                self.next_call += 1;
                self.calls.insert(id, call.id);
                Outgoing { id, ..call }
            })
            .collect();
        Progress {
            replies: progress.replies,
            calls,
        }
    }

    /// Starts a fresh instance after a trap, if the program said how, and
    /// answers every request in flight with `500 Internal Server Error`.
    fn recover(&mut self, trap: wasmtime::Error) -> Result<Progress, Stopped> {
        self.calls.clear();
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
            restarted: recovery.restarted,
        });
        let reached = std::mem::take(&mut state.reached);
        match self.program.instantiate(
            &self.tables,
            &self.externals,
            reports,
            reached,
            Some(&recovery.state),
        ) {
            Ok((store, instance)) => {
                self.store = store;
                self.instance = instance;
                let failed = Response {
                    status: 500,
                    content_type: None,
                    body: Vec::new(),
                };
                Ok(Progress {
                    replies: self
                        .in_flight
                        .iter()
                        .map(|id| (*id, failed.clone()))
                        .collect(),
                    calls: Vec::new(),
                })
            }
            Err(e) => {
                let e = match e {
                    Started::Failed(e) => e.to_string(),
                    Started::Stopped(stopped) => describe(&stopped),
                };
                Err(Stopped {
                    component: trapping.component,
                    failure: trapping.failure,
                    message: format!("{}; then it could not start again: {e}", trapping.message),
                })
            }
        }
    }
}

/// A failed handler the program rolled back and carried on after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The component the failure happened in.
    pub component: String,
    pub failure: String,
    pub message: String,
    /// The component a `restart` reset, with its children: the one the
    /// failure happened in, or the ancestor whose parent restarted it.
    /// `None` for a `rollback`.
    pub restarted: Option<String>,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.restarted {
            None => write!(
                f,
                "{} rolled back after {}: {}",
                self.component, self.failure, self.message
            ),
            Some(restarted) if *restarted == self.component => write!(
                f,
                "{} restarted after {}: {}",
                self.component, self.failure, self.message
            ),
            Some(restarted) => write!(
                f,
                "{restarted} restarted after {} in {}: {}",
                self.failure, self.component, self.message
            ),
        }
    }
}

/// Describes why a program stopped.
pub fn describe(stopped: &Stopped) -> String {
    format!(
        "{} failed with {}: {}",
        stopped.component, stopped.failure, stopped.message
    )
}

fn lock(table: &Mutex<Table>) -> MutexGuard<'_, Table> {
    table.lock().unwrap_or_else(|e| e.into_inner())
}

/// A table the program opened, as a resource in its instance.
pub struct OpenTable(Arc<Mutex<Table>>);

/// What a panic hook handed over before the instance trapped.
struct Trapping {
    component: String,
    failure: String,
    message: String,
    recovery: Option<report::Recovery>,
}

/// One instance's state in the store.
struct State {
    wasi: WasiCtx,
    resources: ResourceTable,
    tables: BTreeMap<String, Arc<Mutex<Table>>>,
    reports: Vec<Report>,
    reached: BTreeSet<String>,
    trapping: Option<Trapping>,
    /// The program's external components, once it is started.
    externals: Option<Arc<Externals>>,
}

impl State {
    fn new(tables: BTreeMap<String, Arc<Mutex<Table>>>) -> State {
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
            reached: BTreeSet::new(),
            trapping: None,
            externals: None,
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
pub(crate) struct FixedClock;

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
    fn open_table(
        &mut self,
        grant: String,
    ) -> wasmtime::Result<Result<Resource<OpenTable>, String>> {
        let Some(table) = self.tables.get(&grant) else {
            return Ok(Err(format!("no table was granted as `{grant}`")));
        };
        let open = OpenTable(table.clone());
        Ok(self.resources.push(open).map_err(|e| e.to_string()))
    }
}

impl State {
    fn open(&self, table: &Resource<OpenTable>) -> wasmtime::Result<MutexGuard<'_, Table>> {
        let open = self.resources.get(table)?;
        Ok(lock(&open.0))
    }
}

impl capabilities::HostTable for State {
    fn get(
        &mut self,
        table: Resource<OpenTable>,
        key: Vec<u8>,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, String>> {
        Ok(self.open(&table)?.read(&key))
    }

    fn insert(
        &mut self,
        table: Resource<OpenTable>,
        key: Vec<u8>,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), String>> {
        Ok(self.open(&table)?.insert(key, value))
    }

    fn remove(
        &mut self,
        table: Resource<OpenTable>,
        key: Vec<u8>,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, String>> {
        Ok(self.open(&table)?.remove(key))
    }

    fn entries(&mut self, table: Resource<OpenTable>) -> wasmtime::Result<Result<Entries, String>> {
        Ok(self.open(&table)?.all())
    }

    fn commit(&mut self, table: Resource<OpenTable>) -> wasmtime::Result<()> {
        self.open(&table)?.commit();
        Ok(())
    }

    fn rollback(&mut self, table: Resource<OpenTable>) -> wasmtime::Result<()> {
        self.open(&table)?.roll_back();
        Ok(())
    }

    fn drop(&mut self, table: Resource<OpenTable>) -> wasmtime::Result<()> {
        self.resources.delete(table)?;
        Ok(())
    }
}

impl externals::Host for State {
    fn call(
        &mut self,
        component: String,
        external: String,
        message: String,
        args: Vec<u8>,
    ) -> wasmtime::Result<Result<Vec<u8>, String>> {
        Ok(match &self.externals {
            Some(externals) => externals.call(&component, &external, &message, &args),
            None => Err("the program has not started".into()),
        })
    }
}

impl State {
    /// A restart reset `component` and its children, so any external
    /// component among them starts afresh.
    fn reset_externals(&self, component: &str) {
        if let Some(externals) = &self.externals {
            externals.reset(component);
        }
    }
}

impl report::Host for State {
    fn rolled_back(
        &mut self,
        component: String,
        failure: String,
        message: String,
    ) -> wasmtime::Result<()> {
        self.reports.push(Report {
            component,
            failure,
            message,
            restarted: None,
        });
        Ok(())
    }

    fn restarted(
        &mut self,
        component: String,
        failure: String,
        message: String,
        restarted: String,
    ) -> wasmtime::Result<()> {
        self.reset_externals(&restarted);
        self.reports.push(Report {
            component,
            failure,
            message,
            restarted: Some(restarted),
        });
        Ok(())
    }

    fn reached(&mut self, elements: Vec<String>) -> wasmtime::Result<()> {
        self.reached.extend(elements);
        Ok(())
    }

    fn trapping(
        &mut self,
        component: String,
        failure: String,
        message: String,
        recovery: Option<report::Recovery>,
    ) -> wasmtime::Result<()> {
        if let Some(restarted) = recovery.as_ref().and_then(|r| r.restarted.as_deref()) {
            self.reset_externals(restarted);
        }
        self.trapping = Some(Trapping {
            component,
            failure,
            message,
            recovery,
        });
        Ok(())
    }
}
