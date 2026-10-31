//! A checked program's HTTP API: every route, with the types of what it
//! reads and replies, for the API contract (`inset-contract`).
//!
//! Types are given as [`Shape`]s over the program's [`Types`], whose JSON
//! names are serde's, so the contract says what the built program reads and
//! writes, as the requirement tests do.

use inset_syntax::ast::{self, HttpMethod};
use inset_value::{Shape, Types};

use crate::modules::Modules;
use crate::values::{shape, types};
use crate::{BodyDecl, Checker, Program, Reply, Report, Segment, finish, status};

/// The HTTP API of a program.
#[derive(Clone, Debug)]
pub struct Api {
    /// The app's name.
    pub app: String,
    /// The app's doc comment.
    pub doc: Option<String>,
    /// Every route of every running component, in the order the components
    /// start and then of their functions.
    pub routes: Vec<ApiRoute>,
    /// The program's structs and enums, which the shapes refer to.
    pub types: Types,
    /// The doc comments of each of [`Api::types`], in the same order.
    pub docs: Vec<TypeDocs>,
}

/// A data type's doc comment, and its fields' or variants'.
#[derive(Clone, Debug)]
pub struct TypeDocs {
    pub doc: Option<String>,
    /// One for each field or variant, in order.
    pub members: Vec<Option<String>>,
}

#[derive(Clone, Debug)]
pub struct ApiRoute {
    /// The `HttpServer` grant its requests arrive through.
    pub server: String,
    /// The component type that handles it, and the handler's name.
    pub component: String,
    pub handler: String,
    pub doc: Option<String>,
    pub method: HttpMethod,
    /// The path as the route attribute writes it, with any query.
    pub template: String,
    pub path: Vec<ApiSegment>,
    pub query: Vec<ApiParam>,
    pub body: Option<ApiBody>,
    /// What a successful reply holds.
    pub reply: ApiBody,
    /// Whether the route replies `404 Not Found` for `None`.
    pub not_found: bool,
    /// The error enum's variants, for a route that returns a `Result`.
    pub errors: Vec<ApiError>,
}

#[derive(Clone, Debug)]
pub enum ApiSegment {
    Literal(String),
    Param(ApiParam),
}

/// A path or query parameter.
#[derive(Clone, Debug)]
pub struct ApiParam {
    pub name: String,
    /// The type it is read as, from text.
    pub shape: Shape,
    /// Whether a request must give it. Path parameters always must.
    pub required: bool,
}

/// A body: text, converted as a path parameter is, or JSON.
#[derive(Clone, Debug)]
pub enum ApiBody {
    Text(Shape),
    Json(Shape),
}

/// A variant of a route's error enum, and the reply it gives.
#[derive(Clone, Debug)]
pub struct ApiError {
    pub variant: String,
    pub doc: Option<String>,
    /// The status's name, such as `NotFound`, and its code.
    pub status: String,
    pub code: u16,
    /// Whether the variant holds a `String`, which is the reply's body.
    pub text: bool,
}

/// Checks the program, and returns its HTTP API if it checks clean.
pub fn api(program: &Program) -> Result<Api, Vec<Report>> {
    let (modules, mut reports) = Modules::load(program);
    let mut checker = Checker::new(&modules);
    checker.check();
    finish(
        program,
        &modules,
        &mut checker,
        &mut reports,
        &mut Vec::new(),
    );
    if !reports.is_empty() {
        return Err(reports);
    }
    let cx = &checker;
    let routes = cx.routes.iter().map(|route| route_api(cx, route)).collect();
    let docs = cx
        .names
        .datas
        .iter()
        .map(|decl| TypeDocs {
            doc: doc(&decl.item.docs),
            members: match decl.ast {
                crate::names::DataAst::Struct(s) => s.fields.iter().map(|f| doc(&f.docs)).collect(),
                crate::names::DataAst::Enum(e) => e.variants.iter().map(|v| doc(&v.docs)).collect(),
            },
        })
        .collect();
    Ok(Api {
        app: cx.app.map_or_else(String::new, |a| a.name.name.clone()),
        doc: cx.names.app.as_ref().and_then(|a| doc(&a.item.docs)),
        routes,
        types: types(cx),
        docs,
    })
}

fn route_api(cx: &Checker, route: &crate::RouteInfo) -> ApiRoute {
    let decl = &route.decl;
    let function = cx
        .components
        .iter()
        .flat_map(|c| &c.functions)
        .find(|f| f.ast.name.span == decl.function)
        .map(|f| f.ast);
    let path_part = decl.template.split('?').next().unwrap_or_default();
    let path = path_part[1..]
        .split('/')
        .zip(&decl.segments)
        .map(|(text, segment)| match segment {
            Segment::Literal(lit) => ApiSegment::Literal(lit.clone()),
            Segment::Param(ty) => ApiSegment::Param(ApiParam {
                name: text
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .to_string(),
                shape: shape(ty),
                required: true,
            }),
        })
        .collect();
    let query = decl
        .query
        .iter()
        .map(|q| ApiParam {
            name: q.name.clone(),
            shape: shape(&q.ty),
            required: q.required,
        })
        .collect();
    let body = decl.body.as_ref().map(|b| match b {
        BodyDecl::Text(ty) => ApiBody::Text(shape(ty)),
        BodyDecl::Json(ty) => ApiBody::Json(shape(ty)),
    });
    let reply = match &decl.reply_json {
        Some(ty) => ApiBody::Json(shape(ty)),
        None => ApiBody::Text(Shape::String),
    };
    let errors = match decl.error {
        Some(id) => {
            let data = &cx.datas[id];
            let docs = match cx.names.datas[id].ast {
                crate::names::DataAst::Enum(e) => e.variants.iter().map(|v| doc(&v.docs)).collect(),
                crate::names::DataAst::Struct(_) => Vec::new(),
            };
            data.variants
                .iter()
                .enumerate()
                .filter_map(|(i, v)| {
                    let status = v.status.as_ref()?.name.clone();
                    Some(ApiError {
                        variant: v.name.name.clone(),
                        doc: docs.get(i).cloned().flatten(),
                        code: status::code(&status)?,
                        status,
                        text: !v.fields.is_empty(),
                    })
                })
                .collect()
        }
        None => Vec::new(),
    };
    ApiRoute {
        server: route.grant.clone().unwrap_or_default(),
        component: route.component.clone(),
        handler: function.map_or_else(String::new, |f| f.name.name.clone()),
        doc: function.and_then(|f| doc(&f.docs)),
        method: decl.method,
        template: decl.template.clone(),
        path,
        query,
        body,
        reply,
        not_found: decl.reply == Reply::Optional,
        errors,
    }
}

fn doc(docs: &[ast::Doc]) -> Option<String> {
    (!docs.is_empty()).then(|| {
        docs.iter()
            .map(|d| d.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}
