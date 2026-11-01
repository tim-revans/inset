//! External components: code written in another language, each a
//! WebAssembly component of its own, which the host runs beside the program.
//!
//! The program sends an external component a message through the
//! `externals` import, with the arguments in their stored form. The host
//! runs one instance of the component for each place the program starts
//! it, named by the running component's diagram ID, and copies each value
//! into the component's own types, as its WIT world describes them, and the
//! reply back out. So the external code shares no memory with the program,
//! and reaches nothing but what the host links for it: WASI, with nothing
//! behind it, as for the program. An import it needs beyond that, such as
//! one of the program's capabilities, stops it from loading.
//!
//! An instance is made with the first message, by calling the component's
//! constructor, the `new` of the Rust trait. A failure in it, such as a
//! panic, traps the instance, which is then dropped, so the next message
//! starts it afresh. A restart of the component, or of a component above
//! it, drops it too. The message a panic printed is kept from the
//! instance's standard error, and given to the program as the failure's.
//!
//! Values are matched by name: a record's fields and a variant's cases have
//! the names the program gives them, in WIT's kebab case, so `on_hand` is
//! `on-hand` and `InvalidRows` is `invalid-rows`. The checker refuses any
//! name that would not come back the same.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use inset_value::json;
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde_json::{Map, Value as Json};
use wasmtime::component::types::Type;
use wasmtime::component::{Component, InstancePre, Linker, ResourceAny, ResourceTable, Val};
use wasmtime::{Engine, Store};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::{Error, FixedClock};

/// The external components a program starts, and their running instances.
pub(crate) struct Externals {
    engine: Engine,
    /// Each component by its name, such as `CsvReader`.
    components: Arc<BTreeMap<String, InstancePre<ExternalState>>>,
    /// Each running instance by the diagram ID of the component it runs as.
    running: Mutex<BTreeMap<String, Instance>>,
}

/// One running external component.
struct Instance {
    store: Store<ExternalState>,
    instance: wasmtime::component::Instance,
    /// The resource its constructor made, which each message is a method of.
    resource: ResourceAny,
}

pub(crate) struct ExternalState {
    wasi: WasiCtx,
    resources: ResourceTable,
    stderr: MemoryOutputPipe,
}

impl WasiView for ExternalState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

impl Externals {
    /// Loads the external components `names` from `dir`, as `<name>.wasm`.
    pub(crate) fn load(engine: &Engine, dir: &Path, names: &[String]) -> Result<Externals, Error> {
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        let mut components = BTreeMap::new();
        for name in names {
            let path = dir.join(format!("{name}.wasm"));
            let component = Component::from_file(engine, &path).map_err(|e| {
                Error(format!(
                    "cannot load the external component `{name}` from {}: {e:?}",
                    path.display()
                ))
            })?;
            let pre = linker.instantiate_pre(&component).map_err(|e| {
                Error(format!(
                    "the external component `{name}` needs what the host does not give it: {e}"
                ))
            })?;
            components.insert(name.clone(), pre);
        }
        Ok(Externals {
            engine: engine.clone(),
            components: Arc::new(components),
            running: Mutex::new(BTreeMap::new()),
        })
    }

    /// The same components, with none running, for a program starting afresh.
    pub(crate) fn fresh(&self) -> Externals {
        Externals {
            engine: self.engine.clone(),
            components: self.components.clone(),
            running: Mutex::new(BTreeMap::new()),
        }
    }

    /// Drops the instance running as `component`, and those of the
    /// components it started, so each starts afresh with its next message.
    pub(crate) fn reset(&self, component: &str) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let prefix = format!("{component}/");
        running.retain(|id, _| id != component && !id.starts_with(&prefix));
    }

    /// Sends a message, with its arguments as a JSON list in their stored
    /// form, and returns the reply in its stored form. An error is a
    /// failure in the external component.
    pub(crate) fn call(
        &self,
        component: &str,
        external: &str,
        message: &str,
        args: &[u8],
    ) -> Result<Vec<u8>, String> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let interface = interface_name(external);
        if !running.contains_key(component) {
            let instance = self.start(external, &interface)?;
            running.insert(component.to_string(), instance);
        }
        let instance = running.get_mut(component).expect("started above");
        match instance.send(&interface, message, args) {
            Ok(reply) => Ok(reply),
            Err(Failed::Trapped(e)) => {
                running.remove(component);
                Err(e)
            }
            Err(Failed::Refused(e)) => Err(e),
        }
    }

    fn start(&self, external: &str, interface: &str) -> Result<Instance, String> {
        let pre = self
            .components
            .get(external)
            .ok_or_else(|| format!("the host has no external component `{external}`"))?;
        let stderr = MemoryOutputPipe::new(64 * 1024);
        let wasi = WasiCtx::builder()
            .stderr(stderr.clone())
            .wall_clock(FixedClock)
            .monotonic_clock(FixedClock)
            .secure_random(StdRng::seed_from_u64(0))
            .insecure_random(StdRng::seed_from_u64(0))
            .insecure_random_seed(0)
            .allow_tcp(false)
            .allow_udp(false)
            .build();
        let mut store = Store::new(
            &self.engine,
            ExternalState {
                wasi,
                resources: ResourceTable::new(),
                stderr,
            },
        );
        let instance = pre
            .instantiate(&mut store)
            .map_err(|e| format!("`{external}` could not start: {e}"))?;
        let constructor = func(
            &instance,
            &mut store,
            interface,
            &format!("[constructor]{}", resource(interface)),
        )
        .ok_or_else(|| format!("`{external}` has no constructor"))?;
        let mut results = [Val::Bool(false)];
        if let Err(e) = constructor.call(&mut store, &[], &mut results) {
            return Err(failure(&store, &e));
        }
        let [Val::Resource(resource)] = results else {
            return Err(format!("`{external}`'s constructor made no resource"));
        };
        Ok(Instance {
            store,
            instance,
            resource,
        })
    }
}

enum Failed {
    /// The instance trapped, and cannot be used again.
    Trapped(String),
    /// The message did not fit the component, which is untouched.
    Refused(String),
}

impl Instance {
    fn send(&mut self, interface: &str, message: &str, args: &[u8]) -> Result<Vec<u8>, Failed> {
        let method = format!(
            "[method]{}.{}",
            resource(interface),
            message.replace('_', "-")
        );
        let func = func(&self.instance, &mut self.store, interface, &method).ok_or_else(|| {
            Failed::Refused(format!("the external component has no message `{message}`"))
        })?;
        let args: Vec<Json> = serde_json::from_slice(args)
            .map_err(|e| Failed::Refused(format!("the arguments are not JSON: {e}")))?;
        let ty = func.ty(&self.store);
        let mut params = vec![Val::Resource(self.resource)];
        for ((name, ty), arg) in ty.params().skip(1).zip(&args) {
            params.push(to_val(arg, &ty).map_err(|e| Failed::Refused(format!("`{name}`: {e}")))?);
        }
        let results_ty: Vec<Type> = ty.results().collect();
        let mut results = vec![Val::Bool(false); results_ty.len()];
        if let Err(e) = func.call(&mut self.store, &params, &mut results) {
            return Err(Failed::Trapped(failure(&self.store, &e)));
        }
        let reply = match results.first() {
            Some(value) => from_val(value),
            None => Json::Null,
        };
        Ok(serde_json::to_vec(&reply).expect("JSON always encodes"))
    }
}

fn func(
    instance: &wasmtime::component::Instance,
    store: &mut Store<ExternalState>,
    interface: &str,
    name: &str,
) -> Option<wasmtime::component::Func> {
    let interface = instance.get_export_index(&mut *store, None, interface)?;
    let index = instance.get_export_index(&mut *store, Some(&interface), name)?;
    instance.get_func(&mut *store, index)
}

/// What a trap says: the message a panic printed, if it printed one, and
/// otherwise the trap itself.
fn failure(store: &Store<ExternalState>, trap: &wasmtime::Error) -> String {
    let stderr = store.data().stderr.contents();
    let stderr = String::from_utf8_lossy(&stderr);
    panic_message(&stderr).unwrap_or_else(|| format!("{trap}"))
}

/// The message of a Rust panic, from what the panic printed:
/// `thread '<unnamed>' panicked at src/csv_reader.rs:9:5:\nmessage`, as
/// `message, at src/csv_reader.rs:9:5`.
fn panic_message(stderr: &str) -> Option<String> {
    let start = stderr.rfind("panicked at ")?;
    let rest = &stderr[start + "panicked at ".len()..];
    let (location, message) = rest.split_once(":\n")?;
    let message: Vec<&str> = message
        .lines()
        .take_while(|line| !line.starts_with("note: "))
        .collect();
    Some(format!("{}, at {location}", message.join("\n").trim_end()))
}

/// The interface the external component `name` exports: `inset:csv-reader/csv-reader`.
fn interface_name(name: &str) -> String {
    let kebab = kebab(name);
    format!("inset:{kebab}/{kebab}")
}

/// The resource inside an interface, which has the interface's name.
fn resource(interface: &str) -> &str {
    interface.rsplit_once('/').map_or(interface, |(_, r)| r)
}

/// `CsvReader` as `csv-reader`, as the compiler names it in WIT.
fn kebab(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// A WIT case's name as the program writes the variant: `invalid-rows` as `InvalidRows`.
fn pascal(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

/// A WIT field's name as the program writes it: `on-hand` as `on_hand`.
fn snake(name: &str) -> String {
    name.replace('-', "_")
}

fn nullable(ty: &Type) -> bool {
    matches!(ty, Type::Option(_))
}

/// A value in its stored form as the external component's type `ty`.
fn to_val(value: &Json, ty: &Type) -> Result<Val, String> {
    let int = || json::to_int(value).ok_or_else(|| format!("expected an integer, found {value}"));
    let wrong = |what: &str| format!("expected {what}, found {value}");
    Ok(match ty {
        Type::Bool => Val::Bool(value.as_bool().ok_or_else(|| wrong("a bool"))?),
        Type::S8 => Val::S8(int()?.try_into().map_err(|_| wrong("an i8"))?),
        Type::U8 => Val::U8(int()?.try_into().map_err(|_| wrong("a u8"))?),
        Type::S16 => Val::S16(int()?.try_into().map_err(|_| wrong("an i16"))?),
        Type::U16 => Val::U16(int()?.try_into().map_err(|_| wrong("a u16"))?),
        Type::S32 => Val::S32(int()?.try_into().map_err(|_| wrong("an i32"))?),
        Type::U32 => Val::U32(int()?.try_into().map_err(|_| wrong("a u32"))?),
        Type::S64 => Val::S64(int()?.try_into().map_err(|_| wrong("an i64"))?),
        Type::U64 => Val::U64(int()?.try_into().map_err(|_| wrong("a u64"))?),
        Type::Float32 => {
            Val::Float32(json::to_float(value).ok_or_else(|| wrong("a number"))? as f32)
        }
        Type::Float64 => Val::Float64(json::to_float(value).ok_or_else(|| wrong("a number"))?),
        Type::Char => Val::Char(json::to_char(value).ok_or_else(|| wrong("a char"))?),
        Type::String => Val::String(value.as_str().ok_or_else(|| wrong("a string"))?.to_string()),
        Type::List(list) => {
            let items = value.as_array().ok_or_else(|| wrong("a list"))?;
            let ty = list.ty();
            Val::List(
                items
                    .iter()
                    .map(|i| to_val(i, &ty))
                    .collect::<Result<_, _>>()?,
            )
        }
        Type::Tuple(tuple) => {
            let items = value.as_array().ok_or_else(|| wrong("a tuple"))?;
            if items.len() != tuple.types().len() {
                return Err(wrong("a tuple"));
            }
            Val::Tuple(
                items
                    .iter()
                    .zip(tuple.types())
                    .map(|(i, ty)| to_val(i, &ty))
                    .collect::<Result<_, _>>()?,
            )
        }
        Type::Record(record) => {
            let object = value.as_object().ok_or_else(|| wrong("a struct"))?;
            let mut fields = Vec::new();
            for field in record.fields() {
                let name = snake(field.name);
                let inner = object
                    .get(&name)
                    .ok_or_else(|| format!("missing field `{name}`"))?;
                fields.push((field.name.to_string(), to_val(inner, &field.ty)?));
            }
            Val::Record(fields)
        }
        Type::Variant(variant) => {
            let (name, values) = json::to_variant(value)?;
            let case = variant
                .cases()
                .find(|c| pascal(c.name) == name)
                .ok_or_else(|| format!("no variant `{name}`"))?;
            let payload = match (&case.ty, values) {
                (None, []) => None,
                (Some(ty), [inner]) => Some(Box::new(to_val(inner, ty)?)),
                _ => return Err(format!("the variant `{name}` holds other values")),
            };
            Val::Variant(case.name.to_string(), payload)
        }
        Type::Enum(e) => {
            let (name, _) = json::to_variant(value)?;
            let case = e
                .names()
                .find(|c| pascal(c) == name)
                .ok_or_else(|| format!("no variant `{name}`"))?;
            Val::Enum(case.to_string())
        }
        Type::Option(option) => {
            let ty = option.ty();
            match json::unwrap_some(value, nullable(&ty))? {
                None => Val::Option(None),
                Some(inner) => Val::Option(Some(Box::new(to_val(inner, &ty)?))),
            }
        }
        Type::Result(result) => {
            let (ok, inner) = json::to_result(value)?;
            let ty = if ok { result.ok() } else { result.err() };
            let inner = match ty {
                Some(ty) => Some(Box::new(to_val(inner, &ty)?)),
                None => None,
            };
            Val::Result(if ok { Ok(inner) } else { Err(inner) })
        }
        _ => return Err("the host cannot pass this type".into()),
    })
}

/// A value of the external component's, in its stored form.
fn from_val(value: &Val) -> Json {
    match value {
        Val::Bool(b) => Json::Bool(*b),
        Val::S8(n) => json::from_int((*n).into()),
        Val::U8(n) => json::from_int((*n).into()),
        Val::S16(n) => json::from_int((*n).into()),
        Val::U16(n) => json::from_int((*n).into()),
        Val::S32(n) => json::from_int((*n).into()),
        Val::U32(n) => json::from_int((*n).into()),
        Val::S64(n) => json::from_int((*n).into()),
        Val::U64(n) => json::from_int((*n).into()),
        Val::Float32(x) => json::from_float((*x).into()),
        Val::Float64(x) => json::from_float(*x),
        Val::Char(c) => json::from_char(*c),
        Val::String(s) => Json::String(s.clone()),
        Val::List(items) | Val::Tuple(items) => Json::Array(items.iter().map(from_val).collect()),
        Val::Record(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, v)| (snake(name), from_val(v)))
                .collect::<Map<_, _>>(),
        ),
        Val::Variant(name, payload) => {
            json::variant(&pascal(name), payload.iter().map(|v| from_val(v)).collect())
        }
        Val::Enum(name) => json::variant(&pascal(name), Vec::new()),
        Val::Option(None) => Json::Null,
        Val::Option(Some(inner)) => json::some(from_val(inner), matches!(**inner, Val::Option(_))),
        Val::Result(result) => {
            let (ok, inner) = match result {
                Ok(inner) => (true, inner),
                Err(inner) => (false, inner),
            };
            json::result(ok, inner.as_deref().map_or(Json::Null, from_val))
        }
        _ => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::{interface_name, panic_message, pascal, snake};

    #[test]
    fn names_as_wit_writes_them() {
        assert_eq!(interface_name("CsvReader"), "inset:csv-reader/csv-reader");
        assert_eq!(pascal("invalid-rows"), "InvalidRows");
        assert_eq!(pascal("h-t-t-p-error"), "HTTPError");
        assert_eq!(snake("on-hand"), "on_hand");
    }

    #[test]
    fn a_panic_message_with_its_location() {
        let stderr = "\nthread '<unnamed>' panicked at src/reader.rs:9:5:\nno rows\nnote: run with `RUST_BACKTRACE=1`\n";
        assert_eq!(
            panic_message(stderr).as_deref(),
            Some("no rows, at src/reader.rs:9:5")
        );
    }
}
