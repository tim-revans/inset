//! The app, components and their functions, which become Rust items that
//! implement the runtime's `App` and `Component` traits.

use inset_check::Modules;
use inset_syntax::ast::{
    App, AttributeKind, Component, Enum, Expr, ExprKind, FailureConditionKind, FailureResponse,
    Function, Handoff, HandoffValue, HttpMethod, Item, ItemKind, LitKind, PatternKind, Route,
    Start, Struct, Type, TypeKind, UnaryOp, UseTree, Visibility,
};
use proc_macro2::TokenStream;
use quote::quote;

use crate::Emitter;
use crate::body::{docs, ident, pattern};
use crate::coverage::Owner;

/// Names the generated route code gives its own variables. A handler
/// parameter with one of these names is renamed where it is read.
const ROUTE_LOCALS: [&str; 5] = ["field", "request", "path", "query", "component"];

impl<'a> Emitter<'a> {
    /// A module's items, with each module it declares inside it, as a Rust
    /// module of the same name. Rust's paths and privacy then work in the
    /// emitted code exactly as in the Inset source.
    pub(crate) fn module(&mut self, modules: &'a Modules, index: usize) -> TokenStream {
        let module = &modules.list[index];
        let outer = std::mem::replace(&mut self.module_path, module.path.clone());
        let items: Vec<_> = module
            .ast
            .items
            .iter()
            .map(|item| match &item.kind {
                ItemKind::Mod(name) => {
                    let child = modules.list.iter().position(|m| {
                        m.parent
                            .as_ref()
                            .is_some_and(|(p, decl)| *p == index && decl.span == name.span)
                    });
                    let vis = self.vis(&item.vis);
                    let docs = docs(&item.docs);
                    let inner = child.map(|c| self.module(modules, c));
                    let name = ident(&name.name);
                    quote! {
                        #docs
                        #vis mod #name {
                            #inner
                        }
                    }
                }
                _ => self.item(item),
            })
            .collect();
        self.module_path = outer;
        quote!(#(#items)*)
    }

    fn item(&mut self, item: &'a Item) -> TokenStream {
        let item_docs = docs(&item.docs);
        match &item.kind {
            ItemKind::App(app) => self.app(item_docs, app),
            ItemKind::Component(component) if component.external.is_some() => {
                let vis = self.vis(&item.vis);
                self.external_component(item_docs, vis, component)
            }
            ItemKind::Component(component) => {
                let vis = self.vis(&item.vis);
                self.component(item_docs, vis, component)
            }
            ItemKind::Function(function) => {
                self.owner = Owner::Algorithms;
                let owner = match self.module_path.as_slice() {
                    [] => self.app.clone(),
                    path => format!("{}/mod:{}", self.app, path.join("::")),
                };
                self.function(function, &format!("{owner}/fn:{}", function.name.name))
            }
            ItemKind::Struct(s) => {
                let vis = self.vis(&item.vis);
                let derive = self.derive(item);
                let name = ident(&s.name.name);
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|field| {
                        let docs = docs(&field.docs);
                        let serde = serde_attrs(&field.attrs);
                        let vis = self.vis(&field.vis);
                        let name = ident(&field.name.name);
                        let ty = self.ty(&field.ty);
                        quote!(#docs #serde #vis #name: #ty)
                    })
                    .collect();
                let stored = stored_struct(s);
                quote! {
                    #item_docs
                    #derive
                    #vis struct #name {
                        #(#fields),*
                    }

                    #stored
                }
            }
            ItemKind::Enum(e) => {
                let vis = self.vis(&item.vis);
                let derive = self.derive(item);
                let name = ident(&e.name.name);
                let variants: Vec<_> = e
                    .variants
                    .iter()
                    .map(|variant| {
                        let docs = docs(&variant.docs);
                        let serde = serde_attrs(&variant.attrs);
                        let name = ident(&variant.name.name);
                        if variant.fields.is_empty() {
                            quote!(#docs #serde #name)
                        } else {
                            let fields: Vec<_> =
                                variant.fields.iter().map(|t| self.ty(t)).collect();
                            quote!(#docs #serde #name(#(#fields),*))
                        }
                    })
                    .collect();
                let status = status_impl(e);
                let stored = stored_enum(e);
                quote! {
                    #item_docs
                    #derive
                    #vis enum #name {
                        #(#variants),*
                    }

                    #status

                    #stored
                }
            }
            ItemKind::Use(tree) => {
                let vis = self.vis(&item.vis);
                let tree = use_tree(tree);
                quote!(#vis use #tree;)
            }
            ItemKind::Mod(_) => unreachable!("modules are emitted by `module`"),
        }
    }

    /// `#[derive(...)]`, with the traits as written, and any `#[serde]`.
    /// Serde's derives are the runtime's re-export, so the generated
    /// project depends on the runtime alone, and `#[serde(crate)]` tells
    /// them where serde is.
    fn derive(&mut self, item: &Item) -> Option<TokenStream> {
        let mut traits = Vec::new();
        let mut serde = false;
        for attr in &item.attrs {
            if let AttributeKind::Derive(names) = &attr.kind {
                for name in names {
                    let name_ident = ident(&name.name);
                    if matches!(name.name.as_str(), "Serialize" | "Deserialize") {
                        serde = true;
                        traits.push(quote!(::inset_runtime::serde::#name_ident));
                    } else {
                        traits.push(quote!(#name_ident));
                    }
                }
            }
        }
        if traits.is_empty() {
            return None;
        }
        let serde_attrs = serde.then(|| {
            let own = serde_attrs(&item.attrs);
            quote! {
                #[serde(crate = "::inset_runtime::serde")]
                #own
            }
        });
        Some(quote!(#[derive(#(#traits),*)] #serde_attrs))
    }

    fn vis(&mut self, vis: &Visibility) -> TokenStream {
        match vis {
            Visibility::Private => quote!(),
            Visibility::Pub(_) => quote!(pub),
            Visibility::PubCrate(_) => quote!(pub(crate)),
            Visibility::PubSuper(_) => quote!(pub(super)),
        }
    }

    fn app(&mut self, docs: TokenStream, app: &App) -> TokenStream {
        let name = ident(&app.name.name);
        let app_name = &app.name.name;
        let settings: Vec<_> = app
            .settings
            .iter()
            .map(|setting| {
                let name = &setting.name.name;
                let ty = match &setting.ty.kind {
                    TypeKind::Path(p) => match p.segments.as_slice() {
                        [only] if only.generics.is_empty() => setting_type(&only.ident.name),
                        _ => None,
                    },
                    _ => None,
                };
                let ty = ty.unwrap_or_else(|| {
                    self.unsupported("a setting of this type", setting.ty.span, "this type");
                    "String"
                });
                let ty = ident(ty);
                let default = literal_text(&setting.default).unwrap_or_else(|| {
                    self.unsupported("this default", setting.default.span, "not a literal");
                    String::new()
                });
                quote!(::inset_runtime::Setting {
                    name: #name,
                    ty: ::inset_runtime::SettingType::#ty,
                    default: #default,
                })
            })
            .collect();
        let grants: Vec<_> = app
            .grants
            .iter()
            .map(|grant| {
                let capability = type_text(&grant.ty);
                let name = &grant.name.name;
                let fields = grant.fields.iter().map(|field| {
                    let name = &field.name.name;
                    let value = match &field.value {
                        None => quote!(::inset_runtime::GrantValue::Setting(#name)),
                        Some(Expr {
                            kind: ExprKind::Path(p),
                            ..
                        }) => {
                            let setting = &p.segments[0].ident.name;
                            quote!(::inset_runtime::GrantValue::Setting(#setting))
                        }
                        Some(value) => {
                            let text = literal_text(value).unwrap_or_else(|| {
                                self.unsupported("this grant field", value.span, "not a literal");
                                String::new()
                            });
                            quote!(::inset_runtime::GrantValue::Literal(#text))
                        }
                    };
                    quote!((#name, #value))
                });
                quote!(::inset_runtime::Grant {
                    name: #name,
                    capability: #capability,
                    fields: &[#(#fields),*],
                })
            })
            .collect();

        // Each grant a `start` moves, taken from the host's grants first.
        let mut taken = Vec::new();
        for start in &app.starts {
            for handoff in &start.handoffs {
                if let HandoffValue::Move(place) = &handoff.value
                    && let [grant] = place.parts.as_slice()
                {
                    taken.push(grant.name.clone());
                }
            }
        }
        let names: Vec<&str> = app
            .grants
            .iter()
            .map(|g| g.name.name.as_str())
            .chain(
                app.starts
                    .iter()
                    .filter_map(|s| s.binding.as_ref().map(|b| b.name.as_str())),
            )
            .collect();
        let scope_var = ident(&free_name("scope", &names));
        let starts = self.starts(&app.starts, &scope_var, None, &names);
        let takes = app
            .grants
            .iter()
            .filter(|g| taken.contains(&g.name.name))
            .map(|g| {
                let var = ident(&g.name.name);
                let name = &g.name.name;
                if is_http_server(&g.ty.kind) {
                    quote!(let #var = grants.http_server(#name);)
                } else {
                    quote!(let #var = grants.table(#name)?;)
                }
            });
        let scope = if app.starts.is_empty() {
            quote!(_)
        } else {
            quote!(#scope_var)
        };
        let grants_param = if taken.is_empty() {
            quote!(_)
        } else {
            quote!(grants)
        };
        // The files are copied beside `src/` into the generated project's
        // `web/`, so the paths here do not depend on where it is.
        let front_end = app.front_ends.first().map(|front| {
            let server = &front.via.name;
            let files = self.web.iter().map(|path| {
                let file = format!("../web/{path}");
                let content_type = content_type(path);
                quote!((#path, #content_type, include_bytes!(#file)))
            });
            quote! {
                const FRONT_END: Option<::inset_runtime::FrontEnd> = Some(::inset_runtime::FrontEnd {
                    server: #server,
                    files: &[#(#files),*],
                });
            }
        });

        let algorithms = self.has_algorithms.then(|| {
            quote!(
                const BRANCHES: &'static [&'static str] = ALGORITHM_BRANCHES;
            )
        });
        let externals = (!self.externals.is_empty()).then(|| {
            let names = &self.externals;
            quote!(
                const EXTERNALS: &'static [&'static str] = &[#(#names),*];
            )
        });
        quote! {
            #docs
            struct #name;

            impl ::inset_runtime::App for #name {
                const NAME: &'static str = #app_name;

                const SETTINGS: &'static [::inset_runtime::Setting] = &[#(#settings),*];

                const GRANTS: &'static [::inset_runtime::Grant] = &[#(#grants),*];

                #front_end

                #externals

                #algorithms

                fn start(
                    #scope: &::inset_runtime::Scope,
                    #grants_param: &mut ::inset_runtime::Grants,
                ) -> Result<(), ::inset_runtime::Stopped> {
                    #(#takes)*
                    #starts
                    Ok(())
                }
            }

            ::inset_runtime::export!(#name);
        }
    }

    fn component(
        &mut self,
        item_docs: TokenStream,
        vis: TokenStream,
        c: &'a Component,
    ) -> TokenStream {
        let name = ident(&c.name.name);
        let component_name = &c.name.name;
        let mut servers = Vec::new();
        let mut tables = Vec::new();
        let mut state = Vec::new();
        // The struct's fields: every field that is kept, then a handle for
        // each named child.
        let mut kept = Vec::new();
        let mut params = Vec::new();
        for field in &c.fields {
            let handed_down = field
                .attrs
                .iter()
                .any(|a| a.kind == AttributeKind::HandedDown);
            let var = ident(&field.name.name);
            let ty = self.ty(&field.ty);
            params.push(quote!(#var: #ty));
            if handed_down {
                continue;
            }
            if is_http_server(&field.ty.kind) {
                servers.push(&field.name.name);
            } else if is_named(&field.ty.kind, "Table") {
                tables.push(&field.name.name);
            } else if is_named(&field.ty.kind, "Outlet") {
            } else if is_handle(&field.ty.kind) {
                self.unsupported("a lent sibling", field.ty.span, "this field's type");
            } else {
                state.push(&field.name.name);
            }
            let docs = docs(&field.docs);
            kept.push((quote!(#docs #var: #ty), var));
        }
        for start in &c.starts {
            if let Some(binding) = &start.binding {
                let var = ident(&binding.name);
                let ty = start_type(start);
                kept.push((quote!(#var: ::inset_runtime::Handle<#ty>), var));
            }
        }
        let fields = kept.iter().map(|(decl, _)| decl);
        let vars = kept.iter().map(|(_, var)| var);
        let taken: Vec<&str> = c
            .fields
            .iter()
            .map(|f| f.name.name.as_str())
            .chain(
                c.starts
                    .iter()
                    .filter_map(|s| s.binding.as_ref().map(|b| b.name.as_str())),
            )
            .collect();
        let scope_var = ident(&free_name("scope", &taken));
        let starts = self.starts(&c.starts, &scope_var, Some(c), &taken);
        let scope_param = if c.starts.is_empty() {
            quote!(_: &::inset_runtime::Scope)
        } else {
            quote!(#scope_var: &::inset_runtime::Scope)
        };
        let what = if c.starts.is_empty() {
            " Builds the component from its fields."
        } else {
            " Starts the component's children, and builds it from its fields."
        };
        let start_fn = quote! {
            #[doc = #what]
            #vis fn start(#scope_param, #(#params),*) -> Self {
                #starts
                #name { #(#vars),* }
            }
        };

        self.owner = Owner::Component;
        self.branches.clear();
        let respond = self.respond_fn(c);
        let rolls_back = c.failure.as_ref().is_some_and(|f| {
            f.arms
                .iter()
                .any(|arm| arm.response == FailureResponse::Rollback)
        });
        let rolls_back = rolls_back.then(|| {
            quote!(
                const ROLLS_BACK: bool = true;
            )
        });

        let functions: Vec<_> = c
            .functions
            .iter()
            .map(|f| self.function(f, &format!("fn:{}", f.name.name)))
            .collect();
        let branches = std::mem::take(&mut self.branches);
        let path: Vec<&str> = std::iter::once("crate")
            .chain(self.module_path.iter().map(String::as_str))
            .chain([component_name.as_str()])
            .collect();
        self.elements
            .components
            .push((path.join("::"), branches.clone()));
        let branches = quote! {
            const BRANCHES: &'static [&'static str] = &[#(#branches),*];
        };
        let (messages, message_types) = self.messages(c, &vis);

        let servers_fn = (!servers.is_empty()).then(|| {
            let entries = servers.iter().map(|field| {
                let var = ident(field);
                quote!((#field, &self.#var))
            });
            quote! {
                fn servers(&self) -> Vec<(&'static str, &::inset_runtime::HttpServer)> {
                    Vec::from([#(#entries),*])
                }
            }
        });
        let label_fn = (!tables.is_empty()).then(|| {
            let labels = tables.iter().map(|field| {
                let var = ident(field);
                quote!(self.#var.label(#field);)
            });
            quote! {
                fn label(&mut self) {
                    #(#labels)*
                }
            }
        });
        let route_fn = self.route_fn(c);
        let saved = if state.is_empty() {
            quote! {
                type Saved = ();

                fn save(&self) {}

                fn restore(&mut self, (): ()) {}
            }
        } else {
            let vars: Vec<_> = state.iter().map(|field| ident(field)).collect();
            quote! {
                type Saved = ::inset_runtime::Saved;

                fn save(&self) -> ::inset_runtime::Saved {
                    ::inset_runtime::Saved::new(Vec::from([
                        #(::inset_runtime::Stored::to_json(&self.#vars)),*
                    ]))
                }

                fn restore(&mut self, saved: ::inset_runtime::Saved) {
                    let mut saved = saved.fields();
                    #(saved.restore(&mut self.#vars);)*
                }
            }
        };

        quote! {
            #item_docs
            #vis struct #name {
                #(#fields),*
            }

            impl #name {
                #start_fn

                #(#functions)*
            }

            #message_types

            impl ::inset_runtime::Component for #name {
                const NAME: &'static str = #component_name;

                #rolls_back

                #branches

                #saved

                #messages

                #respond

                #label_fn

                #servers_fn

                #route_fn
            }
        }
    }

    /// An external component: a component whose handlers pass each message
    /// to the host, which runs the external code (`inset_runtime::External`).
    /// Its messages are sent and awaited, and fail, as any component's are.
    fn external_component(
        &mut self,
        item_docs: TokenStream,
        vis: TokenStream,
        c: &'a Component,
    ) -> TokenStream {
        let name = ident(&c.name.name);
        let component_name = &c.name.name;
        let functions: Vec<_> = c
            .functions
            .iter()
            .map(|f| {
                let docs = docs(&f.docs);
                let fname = ident(&f.name.name);
                let message = &f.name.name;
                let mut params = Vec::new();
                let mut args = Vec::new();
                for (i, p) in f.params.iter().enumerate() {
                    let var = match &p.pattern.kind {
                        PatternKind::Binding { name, .. } => ident(&name.name),
                        _ => ident(&format!("arg{i}")),
                    };
                    let ty = self.ty(&p.ty);
                    params.push(quote!(#var: #ty));
                    args.push(match &p.ty.kind {
                        TypeKind::Ref { .. } => quote!(::inset_runtime::Stored::to_json(
                            &::std::borrow::ToOwned::to_owned(#var)
                        )),
                        _ => quote!(::inset_runtime::Stored::to_json(&#var)),
                    });
                }
                let ret = match &f.ret {
                    Some(t) => self.ty(t),
                    None => quote!(()),
                };
                quote! {
                    #docs
                    async fn #fname(&self, #(#params),*) -> #ret {
                        self.external.call(#message, Vec::from([#(#args),*])).await
                    }
                }
            })
            .collect();
        self.elements.components.push((
            std::iter::once("crate")
                .chain(self.module_path.iter().map(String::as_str))
                .chain([component_name.as_str()])
                .collect::<Vec<_>>()
                .join("::"),
            Vec::new(),
        ));
        let (messages, message_types) = self.messages(c, &vis);
        quote! {
            #item_docs
            #vis struct #name {
                external: ::inset_runtime::External,
            }

            impl #name {
                /// Starts the component, whose code the host runs.
                #vis fn start(scope: &::inset_runtime::Scope) -> Self {
                    #name {
                        external: ::inset_runtime::External::new(scope, #component_name),
                    }
                }

                #(#functions)*
            }

            #message_types

            impl ::inset_runtime::Component for #name {
                const NAME: &'static str = #component_name;

                type Saved = ();

                fn save(&self) {}

                fn restore(&mut self, (): ()) {}

                #messages
            }
        }
    }

    /// The `start` statements of the app, or of the component `owner`:
    /// a handle for each child, made before any child starts so that an
    /// outlet can be connected to a sibling started after it, and then each
    /// child built from its handoffs, in order.
    fn starts(
        &mut self,
        starts: &[Start],
        scope: &proc_macro2::Ident,
        owner: Option<&Component>,
        taken: &[&str],
    ) -> TokenStream {
        let mut handles = Vec::new();
        let mut started = Vec::new();
        let mut taken_here: Vec<String> = taken.iter().map(|s| s.to_string()).collect();
        for start in starts {
            let Some(child) = self.started(start) else {
                continue;
            };
            let ty = start_type(start);
            let (var, name) = match &start.binding {
                Some(binding) => (ident(&binding.name), binding.name.clone()),
                None => {
                    let var = free_name(&snake_case(&child.name.name), &taken_here);
                    taken_here.push(var.clone());
                    (ident(&var), child.name.name.clone())
                }
            };
            handles.push(quote!(let #var = #scope.child::<#ty>(#name);));
            // The child's fields, in the order it declares them.
            let args: Vec<_> = child
                .fields
                .iter()
                .map(|field| {
                    let handoff = start
                        .handoffs
                        .iter()
                        .find(|h| h.field.name == field.name.name);
                    match handoff {
                        Some(handoff) => self.handoff(handoff, scope, owner),
                        None => quote!(todo!()),
                    }
                })
                .collect();
            // The child's own scope, for its children. The handoffs are
            // read in the parent's, where `send self.message` finds the parent.
            let taken: Vec<&str> = taken_here.iter().map(String::as_str).collect();
            let child_scope = ident(&free_name("child", &taken));
            started.push(quote! {
                #scope.start(&#var, |#child_scope| #ty::start(#child_scope, #(#args),*));
            });
        }
        quote! {
            #(#handles)*
            #(#started)*
        }
    }

    /// The value a handoff gives a child's field.
    fn handoff(
        &mut self,
        handoff: &Handoff,
        scope: &proc_macro2::Ident,
        owner: Option<&Component>,
    ) -> TokenStream {
        match &handoff.value {
            HandoffValue::Move(place) if place.parts.len() == 1 => {
                let var = ident(&place.parts[0].name);
                quote!(#var)
            }
            HandoffValue::Expr(expr) => self.expr(expr),
            HandoffValue::Connect { target, message } => {
                let message = ident(&message.name);
                let target = if target.name == "self" && owner.is_some() {
                    quote!(&#scope.this::<Self>())
                } else {
                    let var = ident(&target.name);
                    quote!(&#var)
                };
                quote!(::inset_runtime::Outlet::to(#target, |messages, value| messages.#message(value)))
            }
            HandoffValue::Lend(_) => {
                self.unsupported("lending a child to a sibling", handoff.span, "lent here");
                quote!(todo!())
            }
            HandoffValue::Move(place) => {
                self.unsupported("moving part of a field", place.span, "moved here");
                quote!(todo!())
            }
        }
    }

    /// The component's messages: the `Message` enum, with a variant for each
    /// `#[message]` function holding its arguments, and the type a handle
    /// derefs to, with a method for each that sends it. A lent argument is
    /// copied, so its variant holds an owned value. The first part goes in
    /// the `Component` impl, the second beside it.
    fn messages(&mut self, c: &Component, vis: &TokenStream) -> (TokenStream, TokenStream) {
        let name = ident(&c.name.name);
        let handlers: Vec<&Function> = c
            .functions
            .iter()
            .filter(|f| f.attrs.iter().any(|a| a.kind == AttributeKind::Message))
            .collect();
        if handlers.is_empty() {
            let types = quote! {
                type Message = ::inset_runtime::NoMessage;

                type Messages = ::inset_runtime::Mailbox<Self>;
            };
            return (types, quote!());
        }
        let message = ident(&format!("__{}Message", c.name.name));
        let messages = ident(&format!("__{}Messages", c.name.name));
        let mut variants = Vec::new();
        let mut methods = Vec::new();
        let mut arms = Vec::new();
        let mut writes = Vec::new();
        let mut reads = Vec::new();
        for f in handlers {
            let fname = ident(&f.name.name);
            let fname_text = &f.name.name;
            let fvis = self.vis(&f.vis);
            let docs = docs(&f.docs);
            let mut owned = Vec::new();
            let mut params = Vec::new();
            let mut converted = Vec::new();
            let mut passed = Vec::new();
            let mut vars = Vec::new();
            for (i, p) in f.params.iter().enumerate() {
                let var = match &p.pattern.kind {
                    PatternKind::Binding { name, .. } if !name.name.starts_with('_') => {
                        ident(&name.name)
                    }
                    _ => ident(&format!("arg{i}")),
                };
                let ty = self.ty(&p.ty);
                params.push(quote!(#var: #ty));
                match &p.ty.kind {
                    TypeKind::Ref { inner, .. } => {
                        owned.push(owned_type(inner));
                        converted.push(quote!(::std::borrow::ToOwned::to_owned(#var)));
                        passed.push(quote!(&#var));
                    }
                    _ => {
                        owned.push(ty);
                        converted.push(quote!(#var));
                        passed.push(quote!(#var));
                    }
                }
                vars.push(var);
            }
            let ret = match &f.ret {
                Some(t) => self.ty(t),
                None => quote!(()),
            };
            let (variant, built, pattern) = if owned.is_empty() {
                (
                    quote!(#fname),
                    quote!(#message::#fname),
                    quote!(#message::#fname),
                )
            } else {
                (
                    quote!(#fname(#(#owned),*)),
                    quote!(#message::#fname(#(#converted),*)),
                    quote!(#message::#fname(#(#vars),*)),
                )
            };
            variants.push(variant);
            methods.push(quote! {
                #docs
                #fvis fn #fname(&self, #(#params),*) -> ::inset_runtime::Message<#name, #ret> {
                    self.0.message(#built)
                }
            });
            let call = match f.receiver {
                Some(_) => quote!(#name::#fname(self, #(#passed),*)),
                None => quote!(#name::#fname(#(#passed),*)),
            };
            let call = if f.is_async || c.external.is_some() {
                quote!(#call.await)
            } else {
                call
            };
            // A message that returns nothing replies `()`.
            if f.ret.is_some() {
                arms.push(quote!(#pattern => Box::new(#call) as ::inset_runtime::AnyReply,));
            } else {
                arms.push(quote!(#pattern => {
                    #call;
                    Box::new(()) as ::inset_runtime::AnyReply
                }));
            }
            let fields: Vec<_> = (0..vars.len())
                .map(|i| ident(&format!("field{i}")))
                .collect();
            let count = fields.len();
            let indices = (0..count).map(proc_macro2::Literal::usize_unsuffixed);
            if count == 0 {
                writes.push(quote! {
                    #message::#fname => ::inset_runtime::data::variant(#fname_text, Vec::new()),
                });
                reads.push(quote! {
                    #fname_text => {
                        ::inset_runtime::data::arity(#fname_text, fields, 0)?;
                        Ok(#message::#fname)
                    }
                });
            } else {
                writes.push(quote! {
                    #message::#fname(#(#fields),*) => ::inset_runtime::data::variant(
                        #fname_text,
                        Vec::from([#(::inset_runtime::Stored::to_json(#fields)),*]),
                    ),
                });
                reads.push(quote! {
                    #fname_text => {
                        ::inset_runtime::data::arity(#fname_text, fields, #count)?;
                        Ok(#message::#fname(#(::inset_runtime::Stored::from_json(&fields[#indices])?),*))
                    }
                });
            }
        }
        let message_text = message.to_string();
        let in_impl = quote! {
            type Message = #message;

            type Messages = #messages;

            fn receive(
                &mut self,
                message: #message,
            ) -> ::inset_runtime::LocalBoxFuture<'_, ::inset_runtime::AnyReply> {
                Box::pin(async move {
                    match message {
                        #(#arms)*
                    }
                })
            }
        };
        let beside = quote! {
            /// The messages of a component, each holding its arguments.
            #[doc(hidden)]
            #[allow(non_camel_case_types)]
            #vis enum #message {
                #(#variants),*
            }

            impl ::inset_runtime::Stored for #message {
                fn to_json(&self) -> ::inset_runtime::serde_json::Value {
                    match self {
                        #(#writes)*
                    }
                }

                fn from_json(json: &::inset_runtime::serde_json::Value) -> Result<Self, String> {
                    let (variant, fields) = ::inset_runtime::data::to_variant(json)?;
                    match variant {
                        #(#reads)*
                        other => ::inset_runtime::data::unknown_variant(#message_text, other),
                    }
                }
            }

            /// What a handle to the component offers: a method for each message.
            #[doc(hidden)]
            #vis struct #messages(::inset_runtime::Mailbox<#name>);

            impl From<::inset_runtime::Mailbox<#name>> for #messages {
                fn from(mailbox: ::inset_runtime::Mailbox<#name>) -> Self {
                    #messages(mailbox)
                }
            }

            impl #messages {
                #(#methods)*
            }
        };
        (in_impl, beside)
    }

    /// `respond`: the `failure` block, one arm for each of its arms.
    fn respond_fn(&mut self, c: &Component) -> Option<TokenStream> {
        let failure = c.failure.as_ref()?;
        let mut catch_all = false;
        let arms: Vec<_> = failure
            .arms
            .iter()
            .map(|arm| {
                let pattern = match &arm.condition.kind {
                    FailureConditionKind::Panic => quote!(::inset_runtime::Failure::Panic),
                    FailureConditionKind::Overflow => quote!(::inset_runtime::Failure::Overflow),
                    FailureConditionKind::Capability(field) => {
                        let field = &field.name;
                        quote!(::inset_runtime::Failure::Capability(#field))
                    }
                    FailureConditionKind::Message(field) => {
                        let field = &field.name;
                        quote!(::inset_runtime::Failure::Message(#field))
                    }
                    FailureConditionKind::Child(name) => {
                        let name = &name.name;
                        quote!(::inset_runtime::Failure::Child(#name))
                    }
                    FailureConditionKind::Other => {
                        catch_all = true;
                        quote!(_)
                    }
                };
                let response = match arm.response {
                    FailureResponse::Rollback => quote!(Rollback),
                    FailureResponse::Restart => quote!(Restart),
                };
                let segment = match &arm.condition.kind {
                    FailureConditionKind::Panic => "panic".to_string(),
                    FailureConditionKind::Overflow => "overflow".to_string(),
                    FailureConditionKind::Capability(field) => format!("capability:{}", field.name),
                    FailureConditionKind::Message(field) => format!("message:{}", field.name),
                    FailureConditionKind::Child(name) => format!("child:{}", name.name),
                    FailureConditionKind::Other => "any".to_string(),
                };
                let index =
                    proc_macro2::Literal::usize_unsuffixed(self.list(format!("failure:{segment}")));
                quote!(#pattern => Some((::inset_runtime::Response::#response, #index)),)
            })
            .collect();
        let rest = (!catch_all).then(|| quote!(_ => None,));
        Some(quote! {
            fn respond(
                failure: &::inset_runtime::Failure,
            ) -> Option<(::inset_runtime::Response, usize)> {
                match failure {
                    #(#arms)*
                    #rest
                }
            }
        })
    }

    /// `route`: each route attribute in order, grouped by the server its
    /// requests arrive through.
    fn route_fn(&mut self, c: &Component) -> Option<TokenStream> {
        let mut by_server: Vec<(&str, Vec<TokenStream>)> = Vec::new();
        for function in &c.functions {
            for attr in &function.attrs {
                if let AttributeKind::Route(route) = &attr.kind {
                    let arm = route_arm(route, function);
                    let via = route.via.name.as_str();
                    match by_server.iter_mut().find(|(field, _)| *field == via) {
                        Some((_, arms)) => arms.push(arm),
                        None => by_server.push((via, vec![arm])),
                    }
                }
            }
        }
        if by_server.is_empty() {
            return None;
        }
        let servers = by_server.iter().map(|(field, arms)| {
            quote! {
                if field == #field {
                    #(#arms)*
                }
            }
        });
        Some(quote! {
            fn route(
                field: &str,
                request: &::inset_runtime::Request,
            ) -> ::inset_runtime::Route<Self> {
                #(#servers)*
                ::inset_runtime::Route::NotFound
            }
        })
    }

    /// A function, which marks what coverage counts in it. `id` is its
    /// diagram ID: relative to the running component for a component's.
    fn function(&mut self, f: &Function, id: &str) -> TokenStream {
        let docs = docs(&f.docs);
        let asyncness = f.is_async.then(|| quote!(async));
        let vis = self.vis(&f.vis);
        let name = ident(&f.name.name);
        let receiver = f.receiver.as_ref().map(|r| {
            if r.mutable {
                quote!(&mut self,)
            } else {
                quote!(&self,)
            }
        });
        let params = f.params.iter().map(|p| {
            let pat = pattern(&p.pattern);
            let ty = self.ty(&p.ty);
            quote!(#pat: #ty)
        });
        let params: Vec<_> = params.collect();
        let ret = f.ret.as_ref().map(|t| {
            let t = self.ty(t);
            quote!(-> #t)
        });
        let body = self.covered_body(id, f);
        quote! {
            #docs
            #vis #asyncness fn #name(#receiver #(#params),*) #ret #body
        }
    }
}

/// One route: match the method and path, convert the path and query
/// parameters and the body, and call the handler.
fn route_arm(route: &Route, f: &Function) -> TokenStream {
    let (template, query) = match route.path.value.split_once('?') {
        Some((path, query)) => (path, query),
        None => (route.path.value.as_str(), ""),
    };
    let path_params: Vec<&str> = template
        .split('/')
        .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
        .collect();
    let query_params: Vec<&str> = query
        .split('&')
        .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
        .collect();
    let names: Vec<String> = f
        .params
        .iter()
        .map(|p| match &p.pattern.kind {
            PatternKind::Binding { name, .. } => name.name.clone(),
            _ => String::new(),
        })
        .collect();
    let locals = locals(&names);
    let mut from_path = (Vec::new(), Vec::new());
    let mut from_query = Vec::new();
    let mut read_body = None;
    for ((name, local), param) in names.iter().zip(&locals).zip(&f.params) {
        if path_params.contains(&name.as_str()) {
            from_path.0.push(quote!(Some(#local)));
            from_path.1.push(quote!(path.get(#name)));
        } else if query_params.contains(&name.as_str()) {
            // An `Option` may be left out; any other parameter must be given.
            let pattern = if is_named(&param.ty.kind, "Option") {
                quote!(Ok(#local))
            } else {
                quote!(Ok(Some(#local)))
            };
            from_query.push(quote! {
                let #pattern = query.get(#name) else {
                    return ::inset_runtime::Route::BadRequest;
                };
            });
        } else if route.body.as_ref().is_some_and(|b| b.name == *name) {
            let read = if is_named(&param.ty.kind, "Json") {
                quote!(request.json())
            } else {
                quote!(request.body())
            };
            read_body = Some(
                quote!(let Some(#local) = #read else { return ::inset_runtime::Route::BadRequest; };),
            );
        }
    }
    let method = ident(match route.method {
        HttpMethod::Get => "Get",
        HttpMethod::Post => "Post",
        HttpMethod::Put => "Put",
        HttpMethod::Patch => "Patch",
        HttpMethod::Delete => "Delete",
    });
    let matched = quote!(request.route(::inset_runtime::Method::#method, #template));
    let read_path = match from_path {
        (patterns, _) if patterns.is_empty() => None,
        (patterns, gets) if patterns.len() == 1 => {
            let (pattern, get) = (&patterns[0], &gets[0]);
            Some(quote!(let #pattern = #get else { return ::inset_runtime::Route::NotFound; };))
        }
        (patterns, gets) => Some(quote! {
            let (#(#patterns),*) = (#(#gets),*) else {
                return ::inset_runtime::Route::NotFound;
            };
        }),
    };
    let read_query = (!from_query.is_empty()).then(|| {
        quote! {
            let query = request.query();
            #(#from_query)*
        }
    });
    let name = ident(&f.name.name);
    let (component, call) = match f.receiver {
        // A path, which finds the handler before any method of the
        // `Component` trait, such as `receive`, that has the same name.
        Some(_) => (
            quote!(component),
            quote!(Self::#name(component, #(#locals),*)),
        ),
        None => (quote!(_), quote!(Self::#name(#(#locals),*))),
    };
    let call = if f.is_async {
        quote!(#call.await)
    } else {
        call
    };
    let is_move = (!locals.is_empty()).then(|| quote!(move));
    let to = quote! {
        return ::inset_runtime::Route::to(#is_move |#component: &mut Self| {
            Box::pin(async move { #call })
        });
    };
    if read_path.is_some() {
        quote! {
            if let Some(path) = #matched {
                #read_path
                #read_query
                #read_body
                #to
            }
        }
    } else {
        quote! {
            if #matched.is_some() {
                #read_query
                #read_body
                #to
            }
        }
    }
}

/// The variables holding a handler's arguments: its parameters' names,
/// with `_` added to any that the route code uses for its own.
fn locals(names: &[String]) -> Vec<proc_macro2::Ident> {
    let mut taken: Vec<String> = names.to_vec();
    names
        .iter()
        .map(|name| {
            let mut local = name.clone();
            while ROUTE_LOCALS.contains(&local.as_str()) {
                local.push('_');
                while taken.contains(&local) {
                    local.push('_');
                }
            }
            taken.push(local.clone());
            ident(&local)
        })
        .collect()
}

fn is_http_server(ty: &TypeKind) -> bool {
    matches!(ty, TypeKind::Path(p) if matches!(p.segments.as_slice(), [only] if only.ident.name == "HttpServer" && only.generics.is_empty()))
}

/// Whether a field holds a handle, `Handle<C>` or `&Handle<C>`.
fn is_handle(ty: &TypeKind) -> bool {
    match ty {
        TypeKind::Ref { inner, .. } => is_named(&inner.kind, "Handle"),
        _ => is_named(ty, "Handle"),
    }
}

/// The component a `start` statement names, as written.
fn start_type(start: &Start) -> proc_macro2::Ident {
    ident(&start.component.name)
}

/// What a lent `&T` is copied into: a `String` for a `str`, a `Vec<T>`
/// for a `[T]`, and otherwise `T` itself, as `ToOwned` gives.
fn owned_type(inner: &Type) -> TokenStream {
    match &inner.kind {
        TypeKind::Path(p) if matches!(p.segments.as_slice(), [only] if only.ident.name == "str" && only.generics.is_empty()) =>
        {
            quote!(String)
        }
        TypeKind::Slice(elem) => {
            let elem = crate::body::ty(elem);
            quote!(Vec<#elem>)
        }
        _ => crate::body::ty(inner),
    }
}

/// `UpperCamelCase` as `upper_camel_case`.
fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `name`, or with `_` added until it is none of `taken`.
fn free_name<S: AsRef<str>>(name: &str, taken: &[S]) -> String {
    let mut name = name.to_string();
    while taken.iter().any(|t| t.as_ref() == name) {
        name.push('_');
    }
    name
}

/// Whether a type is the standard type `name`, with any arguments.
/// The `#[serde(...)]` attributes as written.
fn serde_attrs(attrs: &[inset_syntax::ast::Attribute]) -> TokenStream {
    let attrs = attrs.iter().filter_map(|attr| match &attr.kind {
        AttributeKind::Serde(args) => {
            let args = args.iter().map(|arg| {
                let name = ident(&arg.name.name);
                match &arg.value {
                    Some(value) => {
                        let value = &value.value;
                        quote!(#name = #value)
                    }
                    None => quote!(#name),
                }
            });
            Some(quote!(#[serde(#(#args),*)]))
        }
        _ => None,
    });
    quote!(#(#attrs)*)
}

/// The front end's content type for a file, from its extension.
fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
    match extension.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn is_named(ty: &TypeKind, name: &str) -> bool {
    matches!(ty, TypeKind::Path(p) if matches!(p.segments.as_slice(), [only] if only.ident.name == name))
}

/// A type as the source writes it, for the manifest: `Table<String, Item>`.
fn type_text(t: &Type) -> String {
    match &t.kind {
        TypeKind::Path(p) => p
            .segments
            .iter()
            .map(|s| {
                let mut text = s.ident.name.clone();
                if !s.generics.is_empty() {
                    let args: Vec<String> = s.generics.iter().map(type_text).collect();
                    text.push_str(&format!("<{}>", args.join(", ")));
                }
                text
            })
            .collect::<Vec<_>>()
            .join("::"),
        TypeKind::Ref { mutable, inner } => {
            format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                type_text(inner)
            )
        }
        TypeKind::Tuple(elems) => {
            let elems: Vec<String> = elems.iter().map(type_text).collect();
            if elems.len() == 1 {
                format!("({},)", elems[0])
            } else {
                format!("({})", elems.join(", "))
            }
        }
        TypeKind::Slice(inner) => format!("[{}]", type_text(inner)),
        TypeKind::Infer => "_".into(),
    }
}

/// `impl Stored` for a struct: an object of its fields, by name.
fn stored_struct(s: &Struct) -> TokenStream {
    let name = ident(&s.name.name);
    let type_name = &s.name.name;
    let names: Vec<&str> = s.fields.iter().map(|f| f.name.name.as_str()).collect();
    let vars: Vec<_> = names.iter().map(|n| ident(n)).collect();
    let read = if names.is_empty() {
        quote! {
            ::inset_runtime::data::fields(#type_name, json)?;
            Ok(#name {})
        }
    } else {
        quote! {
            let fields = ::inset_runtime::data::fields(#type_name, json)?;
            Ok(#name { #(#vars: fields.get(#names)?),* })
        }
    };
    quote! {
        impl ::inset_runtime::Stored for #name {
            fn to_json(&self) -> ::inset_runtime::serde_json::Value {
                ::inset_runtime::data::object(Vec::from([
                    #((#names, ::inset_runtime::Stored::to_json(&self.#vars))),*
                ]))
            }

            fn from_json(json: &::inset_runtime::serde_json::Value) -> Result<Self, String> {
                #read
            }
        }
    }
}

/// `impl Stored` for an enum: a unit variant's name, or its name and fields.
fn stored_enum(e: &Enum) -> TokenStream {
    let name = ident(&e.name.name);
    let type_name = &e.name.name;
    let mut writes = Vec::new();
    let mut reads = Vec::new();
    for variant in &e.variants {
        let variant_name = &variant.name.name;
        let var = ident(variant_name);
        let count = variant.fields.len();
        let bound: Vec<_> = (0..count).map(|i| ident(&format!("field{i}"))).collect();
        if count == 0 {
            writes.push(quote! {
                Self::#var => ::inset_runtime::data::variant(#variant_name, Vec::new()),
            });
            reads.push(quote! {
                #variant_name => {
                    ::inset_runtime::data::arity(#variant_name, fields, 0)?;
                    Ok(Self::#var)
                }
            });
        } else {
            let indices = (0..count).map(proc_macro2::Literal::usize_unsuffixed);
            writes.push(quote! {
                Self::#var(#(#bound),*) => ::inset_runtime::data::variant(
                    #variant_name,
                    Vec::from([#(::inset_runtime::Stored::to_json(#bound)),*]),
                ),
            });
            reads.push(quote! {
                #variant_name => {
                    ::inset_runtime::data::arity(#variant_name, fields, #count)?;
                    Ok(Self::#var(#(::inset_runtime::Stored::from_json(&fields[#indices])?),*))
                }
            });
        }
    }
    quote! {
        impl ::inset_runtime::Stored for #name {
            fn to_json(&self) -> ::inset_runtime::serde_json::Value {
                match self {
                    #(#writes)*
                }
            }

            fn from_json(json: &::inset_runtime::serde_json::Value) -> Result<Self, String> {
                let (variant, fields) = ::inset_runtime::data::to_variant(json)?;
                match variant {
                    #(#reads)*
                    other => ::inset_runtime::data::unknown_variant(#type_name, other),
                }
            }
        }
    }
}

/// The runtime's name for a setting's type.
fn setting_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "bool" => "Bool",
        "u8" => "U8",
        "u16" => "U16",
        "u32" => "U32",
        "u64" => "U64",
        "i8" => "I8",
        "i16" => "I16",
        "i32" => "I32",
        "i64" => "I64",
        "String" => "String",
        _ => return None,
    })
}

/// A literal written as `--set` would write it: a number in decimal without
/// separators or suffix, `true` or `false`, or a string's text.
fn literal_text(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Lit(lit) => match &lit.kind {
            LitKind::Bool(b) => Some(b.to_string()),
            LitKind::Int(text) => integer(text),
            LitKind::Str(text) => Some(text.clone()),
            _ => None,
        },
        ExprKind::Unary {
            op: UnaryOp::Neg,
            operand,
        } => match &operand.kind {
            ExprKind::Lit(lit) => match &lit.kind {
                LitKind::Int(text) => Some(format!("-{}", integer(text)?)),
                _ => None,
            },
            _ => None,
        },
        ExprKind::Paren(inner) => literal_text(inner),
        _ => None,
    }
}

fn integer(text: &str) -> Option<String> {
    let text = text.replace('_', "");
    let (radix, digits) = match text.get(..2) {
        Some("0x") => (16, &text[2..]),
        Some("0o") => (8, &text[2..]),
        Some("0b") => (2, &text[2..]),
        _ => (10, &text[..]),
    };
    // A suffix starts with `i` or `u`, which no digit in any radix is.
    let digits = digits.split(['i', 'u']).next().unwrap_or(digits);
    u128::from_str_radix(digits, radix)
        .ok()
        .map(|n| n.to_string())
}

/// `impl Status` for an error enum, replying with each variant's status:
/// an empty body for a unit variant, or the `String` it holds.
fn status_impl(e: &Enum) -> Option<TokenStream> {
    let arms: Vec<_> = e
        .variants
        .iter()
        .map(|variant| {
            let status = variant.attrs.iter().find_map(|a| match &a.kind {
                AttributeKind::Status(name) => Some(name),
                _ => None,
            })?;
            let code =
                proc_macro2::Literal::u16_unsuffixed(inset_check::status::code(&status.name)?);
            let name = ident(&variant.name.name);
            Some(if variant.fields.is_empty() {
                quote!(Self::#name => ::inset_runtime::Reply::empty(#code),)
            } else {
                quote!(Self::#name(text) => ::inset_runtime::Reply::text(#code, text),)
            })
        })
        .collect::<Option<_>>()?;
    let name = ident(&e.name.name);
    Some(quote! {
        impl ::inset_runtime::Status for #name {
            fn into_reply(self) -> ::inset_runtime::Reply {
                match self {
                    #(#arms)*
                }
            }
        }
    })
}

fn use_tree(tree: &UseTree) -> TokenStream {
    match tree {
        UseTree::Path { segment, rest } => {
            let segment = ident(&segment.name);
            let rest = use_tree(rest);
            quote!(#segment::#rest)
        }
        UseTree::Name { name, alias } => {
            let name = ident(&name.name);
            match alias {
                Some(alias) => {
                    let alias = ident(&alias.name);
                    quote!(#name as #alias)
                }
                None => quote!(#name),
            }
        }
        UseTree::Group(trees, _) => {
            let trees = trees.iter().map(use_tree);
            quote!({ #(#trees),* })
        }
    }
}
