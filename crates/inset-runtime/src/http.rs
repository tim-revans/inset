//! The `HttpServer` capability: requests in, replies out.

use std::str::FromStr;

use crate::actor::{Call, LocalBoxFuture};

/// The `HttpServer` capability. The host listens on the granted port and
/// hands each request to the component holding this, by the grant's name.
pub struct HttpServer {
    grant: String,
}

impl HttpServer {
    pub(crate) fn new(grant: &str) -> HttpServer {
        HttpServer {
            grant: grant.into(),
        }
    }

    pub(crate) fn grant(&self) -> &str {
        &self.grant
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    fn parse(method: &str) -> Option<Method> {
        Some(match method {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            _ => return None,
        })
    }
}

/// A request, as the host delivered it.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// The path and query, as sent.
    pub path: String,
    pub body: Vec<u8>,
}

impl Request {
    /// The path parameters, if the request matches a route. `template` is the
    /// route attribute's path, such as `/hello/{name}`.
    pub fn route(&self, method: Method, template: &'static str) -> Option<PathParams> {
        if Method::parse(&self.method) != Some(method) {
            return None;
        }
        let path = self
            .path
            .split_once('?')
            .map_or(&*self.path, |(path, _)| path);
        let mut segments = path.strip_prefix('/')?.split('/');
        let mut params = Vec::new();
        for pattern in template.strip_prefix('/')?.split('/') {
            let segment = segments.next()?;
            match pattern.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                Some(name) => params.push((name, percent_decode(segment)?)),
                None if pattern == segment => {}
                None => return None,
            }
        }
        if segments.next().is_some() {
            return None;
        }
        Some(PathParams(params))
    }

    /// The body converted as a path segment is, or `None` if it cannot be.
    pub fn body<T: FromStr>(&self) -> Option<T> {
        std::str::from_utf8(&self.body).ok()?.parse().ok()
    }
}

/// Decodes `%XX` escapes. `None` if the result is not UTF-8.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| segment.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// The path parameters of a matched route.
pub struct PathParams(Vec<(&'static str, String)>);

impl PathParams {
    /// The parameter converted to the handler's parameter type, or `None` if
    /// it cannot be, in which case the route does not match.
    pub fn get<T: FromStr>(&self, name: &str) -> Option<T> {
        let (_, value) = self.0.iter().find(|(n, _)| *n == name)?;
        value.parse().ok()
    }
}

/// An HTTP reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn empty(status: u16) -> Reply {
        Reply {
            status,
            content_type: None,
            body: Vec::new(),
        }
    }

    pub fn text(status: u16, text: String) -> Reply {
        Reply {
            status,
            content_type: Some("text/plain; charset=utf-8"),
            body: text.into_bytes(),
        }
    }
}

/// A route handler's return type, which decides the reply.
pub trait IntoResponse {
    fn into_reply(self) -> Reply;
}

impl IntoResponse for String {
    fn into_reply(self) -> Reply {
        Reply::text(200, self)
    }
}

impl<T: IntoResponse> IntoResponse for Option<T> {
    fn into_reply(self) -> Reply {
        self.map_or_else(|| Reply::empty(404), T::into_reply)
    }
}

impl<T: IntoResponse, E: Status> IntoResponse for Result<T, E> {
    fn into_reply(self) -> Reply {
        self.map_or_else(E::into_reply, T::into_reply)
    }
}

/// An error enum whose variants carry `#[status(...)]`.
pub trait Status {
    fn into_reply(self) -> Reply;
}

/// Where a request goes.
pub enum Route<C> {
    NotFound,
    BadRequest,
    To(Call<C, Reply>),
}

impl<C: 'static> Route<C> {
    /// A route to a handler, whose return value is the reply.
    pub fn to<R: IntoResponse + 'static>(
        handler: impl for<'a> FnOnce(&'a mut C) -> LocalBoxFuture<'a, R> + 'static,
    ) -> Route<C> {
        Route::To(Box::new(move |component| {
            let reply = handler(component);
            Box::pin(async move { reply.await.into_reply() })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Request {
        Request {
            method: "GET".into(),
            path: path.into(),
            body: Vec::new(),
        }
    }

    #[test]
    fn matches_path_parameters_and_decodes_them() {
        let params = get("/hello/Ada%20Lovelace?x=1")
            .route(Method::Get, "/hello/{name}")
            .unwrap();
        assert_eq!(params.get::<String>("name").unwrap(), "Ada Lovelace");
    }

    #[test]
    fn rejects_other_methods_lengths_and_literals() {
        let route = |r: Request| r.route(Method::Get, "/hello/{name}").is_some();
        assert!(!route(Request {
            method: "POST".into(),
            ..get("/hello/Ada")
        }));
        assert!(!route(get("/hello")));
        assert!(!route(get("/hello/Ada/more")));
        assert!(!route(get("/goodbye/Ada")));
    }

    #[test]
    fn a_parameter_of_the_wrong_type_does_not_convert() {
        let params = get("/items/many").route(Method::Get, "/items/{n}").unwrap();
        assert_eq!(params.get::<u32>("n"), None);
    }
}
