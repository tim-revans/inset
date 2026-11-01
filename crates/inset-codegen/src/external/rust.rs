//! The Rust adapter: builds an external component written in Rust from
//! its WIT world.
//!
//! Each component is a Cargo package whose library is the program's own
//! Rust file, such as `src/csv_reader.rs`, built as a `cdylib` for
//! `wasm32-wasip2`, which rustc links as a WebAssembly component. Its one
//! generated dependency is the `interface` crate, which runs `wit-bindgen`
//! on the world and offers what syntax.md promises the Rust author:
//!
//! - a trait named after the component, with `fn new() -> Self` and one
//!   method per message, exactly as declared, so a parameter declared `&str`
//!   is a `&str`;
//! - the program's types the messages carry, which are `wit-bindgen`'s;
//! - `export!`, which implements `wit-bindgen`'s traits for the author's
//!   type by calling the trait, lending each lent argument and holding the
//!   value in a `RefCell` for a message that takes `&mut self`.
//!
//! A file that does not match the declaration fails with rustc's own error
//! about the trait, which `inset build` shows under the declaration.
//!
//! Every external component of a program is a member of one Cargo
//! workspace, whose lock file is the program's `inset.lock`. The first
//! build starts it from Inset's own `Cargo.lock`, so `wit-bindgen` and any
//! crate Inset also uses come at the versions Inset is tested with, and
//! writes the result back beside `src/`. Later builds start from
//! `inset.lock`, so a version it holds is used even after a newer one is
//! published, as Cargo honours a `Cargo.lock`.

use std::path::{Path, PathBuf};
use std::process::Command;

use inset_check::{External, Program, Report, WitType};
use inset_syntax::Diagnostic;

use super::{type_name, world};
use crate::project::{BuildError, LOCK, TOOLCHAIN, artifact, write};

/// The program's lock file for the crates its external components use.
pub const LOCK_FILE: &str = "inset.lock";

/// The Cargo package of an external component, such as `csv-reader`.
pub(crate) fn package(external: &External) -> String {
    type_name(&external.name)
}

/// Writes and builds every external component of the program, in
/// `out_dir/externals/`, and copies each to `out_dir/externals/<Name>.wasm`,
/// where the host finds it. Writes `inset.lock` beside `src/`.
pub(crate) fn build(
    program: &Program,
    externals: &[External],
    out_dir: &Path,
    target_dir: Option<&Path>,
) -> Result<(), BuildError> {
    if externals.is_empty() {
        return Ok(());
    }
    let Some(dir) = &program.dir else {
        return Err(BuildError::Failed(
            "a program with an external component is built from its directory".into(),
        ));
    };
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    let workspace = out_dir.join("externals");
    let mut members = Vec::new();
    for external in externals {
        let package = package(external);
        let interface = format!("{package}-interface");
        write(
            &workspace.join(&package).join("wit/world.wit"),
            &world(external),
        )?;
        write(
            &workspace.join(&interface).join("Cargo.toml"),
            &interface_toml(&interface),
        )?;
        write(
            &workspace.join(&interface).join("src/lib.rs"),
            &interface_lib(external),
        )?;
        let source = dir.join(&external.path);
        write(
            &workspace.join(&package).join("Cargo.toml"),
            &component_toml(&package, &interface, &source, &external.crates),
        )?;
        members.push(package);
        members.push(interface);
    }
    write(&workspace.join("Cargo.toml"), &workspace_toml(&members))?;
    write(&workspace.join("rust-toolchain.toml"), TOOLCHAIN)?;
    let lock_path = dir.join(LOCK_FILE);
    let lock = std::fs::read_to_string(&lock_path).unwrap_or_else(|_| LOCK.to_string());
    write(&workspace.join("Cargo.lock"), &lock)?;

    let cargo = |args: &[&str]| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command
            .args(args)
            .args(["--release", "--target", "wasm32-wasip2", "--quiet"])
            .current_dir(&workspace);
        if let Some(target_dir) = target_dir {
            command.arg("--target-dir").arg(target_dir);
        }
        command
            .output()
            .map_err(|e| BuildError::Failed(format!("cannot run cargo: {e}")))
    };
    // As for the program's own package (see `project.rs`): two programs'
    // external components may share a package name in a shared target
    // directory, so each is built afresh there.
    if target_dir.is_some() || std::env::var_os("CARGO_TARGET_DIR").is_some() {
        for external in externals {
            let cleaned = cargo(&["clean", "--package", &package(external)])?;
            if !cleaned.status.success() {
                return Err(BuildError::Failed(format!(
                    "cargo could not clean {}:\n\n{}",
                    package(external),
                    String::from_utf8_lossy(&cleaned.stderr)
                )));
            }
        }
    }
    for external in externals {
        let package = package(external);
        let output = cargo(&[
            "build",
            "--package",
            &package,
            "--message-format",
            "json-render-diagnostics",
        ])?;
        if !output.status.success() {
            let rustc =
                String::from_utf8_lossy(&output.stderr).replace(&format!("{}/", dir.display()), "");
            return Err(BuildError::External {
                report: Report {
                    file: external.file,
                    diagnostic: Diagnostic::new(
                        format!("the Rust that implements `{}` does not build", external.name),
                        external.span,
                        format!("the trait `interface::{}` comes from this declaration", external.name),
                    )
                    .with_help(format!(
                        "make `{}` match the declaration, or change the declaration; rustc's errors follow",
                        external.path
                    )),
                },
                rustc,
            });
        }
        let built =
            artifact(&String::from_utf8_lossy(&output.stdout), &package).ok_or_else(|| {
                BuildError::Failed(format!("cargo built no component for `{}`", external.name))
            })?;
        let component = out_dir
            .join("externals")
            .join(format!("{}.wasm", external.name));
        copy(&built, &component)?;
    }
    let locked = std::fs::read_to_string(workspace.join("Cargo.lock")).map_err(|e| {
        BuildError::Failed(format!(
            "cannot read the external components' lock file: {e}"
        ))
    })?;
    if std::fs::read_to_string(&lock_path).ok().as_deref() != Some(locked.as_str()) {
        std::fs::write(&lock_path, locked).map_err(|e| {
            BuildError::Failed(format!("cannot write {}: {e}", lock_path.display()))
        })?;
    }
    Ok(())
}

fn copy(from: &PathBuf, to: &Path) -> Result<(), BuildError> {
    std::fs::copy(from, to).map(|_| ()).map_err(|e| {
        BuildError::Failed(format!(
            "cannot copy {} to {}: {e}",
            from.display(),
            to.display()
        ))
    })
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn workspace_toml(members: &[String]) -> String {
    let members: Vec<String> = members.iter().map(|m| quoted(m)).collect();
    format!(
        r#"# Generated by `inset build`. It is written again on every build.
# The lock file is the program's `inset.lock`.

[workspace]
resolver = "3"
members = [{}]

# Inset checks integer arithmetic in every build, in external code too.
[profile.release]
overflow-checks = true
"#,
        members.join(", ")
    )
}

fn interface_toml(package: &str) -> String {
    format!(
        r#"# Generated by `inset build`. It is written again on every build.

[package]
name = "{package}"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
name = "interface"

[dependencies]
wit-bindgen = {{ version = "0.63.0", default-features = false, features = ["macros", "realloc"] }}
"#
    )
}

fn component_toml(
    package: &str,
    interface: &str,
    source: &Path,
    crates: &[(String, String)],
) -> String {
    let source = quoted(&source.to_string_lossy());
    let crates: String = crates
        .iter()
        .map(|(name, version)| format!("{name} = {}\n", quoted(version)))
        .collect();
    format!(
        r#"# Generated by `inset build`. It is written again on every build.

[package]
name = "{package}"
version = "0.0.0"
edition = "2024"
publish = false

# The program's own Rust file.
[lib]
path = {source}
crate-type = ["cdylib"]

[dependencies]
interface = {{ package = "{interface}", path = "../{interface}" }}
{crates}"#
    )
}

/// The `interface` crate's `src/lib.rs`.
pub fn interface_lib(external: &External) -> String {
    let package = package(external);
    let module = format!(
        "exports::inset::{}::{}",
        package.replace('-', "_"),
        package.replace('-', "_")
    );
    let name = &external.name;
    let mut out = format!(
        r#"//! The bindings of the external component `{name}`, generated by `inset
//! build` from its declaration. It is written again on every build.
//!
//! Implement [`{name}`] for a type of your own, and name the type with
//! [`export!`].

#[doc(hidden)]
#[allow(unsafe_code, clippy::all)]
pub mod bindings {{
    wit_bindgen::generate!({{
        path: "../{package}/wit",
        world: "external",
        pub_export_macro: true,
        export_macro_name: "__export",
        default_bindings_module: "interface::bindings",
    }});
}}
"#
    );
    if !external.types.is_empty() {
        let types: Vec<&str> = external.types.iter().map(|t| t.name.as_str()).collect();
        let types = match types.as_slice() {
            [one] => one.to_string(),
            _ => format!("{{{}}}", types.join(", ")),
        };
        out.push_str(&format!("\npub use bindings::{module}::{types};\n"));
    }
    out.push('\n');
    super::doc(&mut out, external.doc.as_deref(), "");
    out.push_str(&format!("pub trait {name}: 'static {{\n"));
    out.push_str(
        "    /// Starts the component, before its first message and again after it restarts.\n",
    );
    out.push_str("    fn new() -> Self;\n");
    for message in &external.messages {
        out.push('\n');
        super::doc(&mut out, message.doc.as_deref(), "    ");
        let receiver = if message.mutable {
            "&mut self"
        } else {
            "&self"
        };
        let params: String = message
            .params
            .iter()
            .map(|p| format!(", {}: {}", p.name, p.rust))
            .collect();
        let ret = message
            .ret
            .as_ref()
            .map(|r| format!(" -> {}", r.rust))
            .unwrap_or_default();
        out.push_str(&format!(
            "    fn {}({receiver}{params}){ret};\n",
            message.name
        ));
    }
    out.push_str("}\n");

    let methods: String = external
        .messages
        .iter()
        .map(|message| {
            let params: String = message
                .params
                .iter()
                .map(|p| format!(", {}: {}", p.name, bound(&p.ty)))
                .collect();
            let args: String = message
                .params
                .iter()
                .map(|p| {
                    if p.lent {
                        format!(", &{}", p.name)
                    } else {
                        format!(", {}", p.name)
                    }
                })
                .collect();
            let ret = message
                .ret
                .as_ref()
                .map(|r| format!(" -> {}", bound(&r.ty)))
                .unwrap_or_default();
            let borrow = if message.mutable {
                "&mut *self.0.borrow_mut()"
            } else {
                "&*self.0.borrow()"
            };
            format!(
                "                fn {0}(&self{params}){ret} {{\n\
                 \x20                   <$ty as $crate::{name}>::{0}({borrow}{args})\n\
                 \x20               }}\n",
                message.name
            )
        })
        .collect();
    out.push_str(&format!(
        r#"
/// Makes a type that implements [`{name}`] the component's code.
#[macro_export]
macro_rules! export {{
    ($ty:ty) => {{
        const _: () = {{
            struct __InsetExported;

            struct __InsetInstance(::core::cell::RefCell<$ty>);

            impl $crate::bindings::{module}::Guest for __InsetExported {{
                type {name} = __InsetInstance;
            }}

            impl $crate::bindings::{module}::Guest{name} for __InsetInstance {{
                fn new() -> Self {{
                    __InsetInstance(::core::cell::RefCell::new(<$ty as $crate::{name}>::new()))
                }}

{methods}            }}

            $crate::bindings::__export!(__InsetExported with_types_in $crate::bindings);
        }};
    }};
}}
"#
    ));
    out
}

/// A type as the bindings hold it, named from where `export!` is used.
fn bound(ty: &WitType) -> String {
    match ty {
        WitType::Bool => "bool".into(),
        WitType::Int { signed, bits } => format!("{}{bits}", if *signed { "i" } else { "u" }),
        WitType::F32 => "f32".into(),
        WitType::F64 => "f64".into(),
        WitType::Char => "char".into(),
        WitType::String => "::std::string::String".into(),
        WitType::List(inner) => format!("::std::vec::Vec<{}>", bound(inner)),
        WitType::Option(inner) => format!("::core::option::Option<{}>", bound(inner)),
        WitType::Result(ok, err) => {
            let side = |t: &Option<Box<WitType>>| t.as_deref().map_or("()".into(), bound);
            format!("::core::result::Result<{}, {}>", side(ok), side(err))
        }
        WitType::Tuple(elems) => {
            let elems: Vec<String> = elems.iter().map(bound).collect();
            if elems.len() == 1 {
                format!("({},)", elems[0])
            } else {
                format!("({})", elems.join(", "))
            }
        }
        WitType::Data(name) => format!("$crate::{name}"),
    }
}
