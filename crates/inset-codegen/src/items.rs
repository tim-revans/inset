//! The app, components and their functions, which become Rust items that
//! implement the runtime's `App` and `Component` traits.

use inset_check::Modules;
use inset_syntax::Span;
use inset_syntax::ast::{
    App, AttributeKind, Component, Enum, Expr, ExprKind, Function, HandoffValue, HttpMethod, Item,
    ItemKind, LitKind, PatternKind, Route, TypeKind, UnaryOp, UseTree, Visibility,
};
use proc_macro2::TokenStream;
use quote::quote;

use crate::Emitter;
use crate::body::{docs, ident, pattern};

/// Names the generated route code gives its own variables. A handler
/// parameter with one of these names is renamed where it is read.
const ROUTE_LOCALS: [&str; 4] = ["field", "request", "path", "component"];

impl Emitter {
    /// A module's items, with each module it declares inside it, as a Rust
    /// module of the same name. Rust's paths and privacy then work in the
    /// emitted code exactly as in the Inset source.
    pub(crate) fn module(&mut self, modules: &Modules, index: usize) -> TokenStream {
        let module = &modules.list[index];
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
        quote!(#(#items)*)
    }

    fn item(&mut self, item: &Item) -> TokenStream {
        let item_docs = docs(&item.docs);
        match &item.kind {
            ItemKind::App(app) => self.app(item_docs, app),
            ItemKind::Component(component) => {
                let vis = self.vis(&item.vis);
                self.component(item_docs, vis, component)
            }
            ItemKind::Function(function) => self.function(function),
            ItemKind::Struct(s) => {
                let vis = self.vis(&item.vis);
                let derive = self.derive(item);
                let name = ident(&s.name.name);
                let fields: Vec<_> = s
                    .fields
                    .iter()
                    .map(|field| {
                        let docs = docs(&field.docs);
                        let vis = self.vis(&field.vis);
                        let name = ident(&field.name.name);
                        let ty = self.ty(&field.ty);
                        quote!(#docs #vis #name: #ty)
                    })
                    .collect();
                quote! {
                    #item_docs
                    #derive
                    #vis struct #name {
                        #(#fields),*
                    }
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
                        let name = ident(&variant.name.name);
                        if variant.fields.is_empty() {
                            quote!(#docs #name)
                        } else {
                            let fields: Vec<_> =
                                variant.fields.iter().map(|t| self.ty(t)).collect();
                            quote!(#docs #name(#(#fields),*))
                        }
                    })
                    .collect();
                let status = status_impl(e);
                quote! {
                    #item_docs
                    #derive
                    #vis enum #name {
                        #(#variants),*
                    }

                    #status
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

    /// `#[derive(...)]`, with the traits as written.
    fn derive(&mut self, item: &Item) -> Option<TokenStream> {
        let mut traits = Vec::new();
        for attr in &item.attrs {
            if let AttributeKind::Derive(names) = &attr.kind {
                for name in names {
                    if matches!(name.name.as_str(), "Serialize" | "Deserialize") {
                        self.unsupported(
                            &format!("deriving `{}`", name.name),
                            name.span,
                            "derived here",
                        );
                    }
                    traits.push(ident(&name.name));
                }
            }
        }
        (!traits.is_empty()).then(|| quote!(#[derive(#(#traits),*)]))
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
                if !is_http_server(&grant.ty.kind) {
                    let what = match &grant.ty.kind {
                        TypeKind::Path(p) => format!("a `{}` grant", p.segments[0].ident.name),
                        _ => "this grant".into(),
                    };
                    self.unsupported(&what, grant.ty.span, "granted here");
                }
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
                    capability: "HttpServer",
                    fields: &[#(#fields),*],
                })
            })
            .collect();

        // Each grant a `start` moves, taken from the host's grants first.
        let mut taken = Vec::new();
        let mut starts = Vec::new();
        for start in &app.starts {
            if let Some(binding) = &start.binding {
                self.unsupported("a named child", binding.span, "named here");
            }
            let component = ident(&start.component.name);
            let fields: Vec<_> = start
                .handoffs
                .iter()
                .map(|handoff| {
                    let field = ident(&handoff.field.name);
                    match &handoff.value {
                        HandoffValue::Move(place) if place.parts.len() == 1 => {
                            let grant = &place.parts[0].name;
                            taken.push(grant.clone());
                            let grant = ident(grant);
                            if field == grant {
                                quote!(#field)
                            } else {
                                quote!(#field: #grant)
                            }
                        }
                        _ => {
                            self.unsupported("this handoff", handoff.span, "handed off here");
                            quote!(#field: todo!())
                        }
                    }
                })
                .collect();
            starts.push(quote!(scope.start(|_| #component { #(#fields),* });));
        }
        let takes = app
            .grants
            .iter()
            .filter(|g| taken.contains(&g.name.name))
            .map(|g| {
                let var = ident(&g.name.name);
                let name = &g.name.name;
                quote!(let #var = grants.http_server(#name);)
            });
        let scope = if starts.is_empty() {
            quote!(_)
        } else {
            quote!(scope)
        };
        let grants_param = if taken.is_empty() {
            quote!(_)
        } else {
            quote!(grants)
        };

        quote! {
            #docs
            struct #name;

            impl ::inset_runtime::App for #name {
                const NAME: &'static str = #app_name;

                const SETTINGS: &'static [::inset_runtime::Setting] = &[#(#settings),*];

                const GRANTS: &'static [::inset_runtime::Grant] = &[#(#grants),*];

                fn start(
                    #scope: &::inset_runtime::Scope,
                    #grants_param: &mut ::inset_runtime::Grants,
                ) -> Result<(), ::inset_runtime::Stopped> {
                    #(#takes)*
                    #(#starts)*
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
        c: &Component,
    ) -> TokenStream {
        let name = ident(&c.name.name);
        let component_name = &c.name.name;
        let mut servers = Vec::new();
        let fields: Vec<_> = c
            .fields
            .iter()
            .map(|field| {
                if is_http_server(&field.ty.kind) {
                    servers.push(&field.name.name);
                } else {
                    self.unsupported(
                        "a field that is not an `HttpServer`",
                        field.ty.span,
                        "this field's type",
                    );
                }
                let docs = docs(&field.docs);
                let name = ident(&field.name.name);
                let ty = self.ty(&field.ty);
                // Whoever can name the component can start it, and so must
                // be able to supply its fields.
                quote!(#docs #vis #name: #ty)
            })
            .collect();
        for start in &c.starts {
            self.unsupported("`start` in a component", start.span, "starts a child");
        }
        if let Some(failure) = &c.failure {
            let keyword = Span::new(failure.span.start, failure.span.start + "failure".len());
            self.unsupported("a `failure` block", keyword, "this block");
        }

        let functions: Vec<_> = c.functions.iter().map(|f| self.function(f)).collect();
        let methods = (!functions.is_empty()).then(|| quote!(impl #name { #(#functions)* }));

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
        let route_fn = self.route_fn(c);

        quote! {
            #item_docs
            #vis struct #name {
                #(#fields),*
            }

            #methods

            impl ::inset_runtime::Component for #name {
                const NAME: &'static str = #component_name;

                type Saved = ();

                fn save(&self) {}

                fn restore(&mut self, (): ()) {}

                #servers_fn

                #route_fn
            }
        }
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

    fn function(&mut self, f: &Function) -> TokenStream {
        for attr in &f.attrs {
            if attr.kind == AttributeKind::Message {
                self.unsupported("a message handler", attr.span, "this attribute");
            }
        }
        if f.is_async {
            self.unsupported("an `async fn`", f.name.span, "this function");
        }
        let docs = docs(&f.docs);
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
        let body = self.block(&f.body);
        quote! {
            #docs
            #vis fn #name(#receiver #(#params),*) #ret #body
        }
    }
}

/// One route: match the method and path, convert the path parameters and
/// the body, and call the handler.
fn route_arm(route: &Route, f: &Function) -> TokenStream {
    let template = &route.path.value;
    let path_params: Vec<&str> = template
        .split('/')
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
    let mut from_body = None;
    for (name, local) in names.iter().zip(&locals) {
        if path_params.contains(&name.as_str()) {
            from_path.0.push(quote!(Some(#local)));
            from_path.1.push(quote!(path.get(#name)));
        } else if route.body.as_ref().is_some_and(|b| b.name == *name) {
            from_body = Some(local);
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
    let read_body = from_body.map(|local| {
        quote!(let Some(#local) = request.body() else { return ::inset_runtime::Route::BadRequest; };)
    });
    let name = ident(&f.name.name);
    let (component, call) = match f.receiver {
        Some(_) => (quote!(component), quote!(component.#name(#(#locals),*))),
        None => (quote!(_), quote!(Self::#name(#(#locals),*))),
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
                #read_body
                #to
            }
        }
    } else {
        quote! {
            if #matched.is_some() {
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
