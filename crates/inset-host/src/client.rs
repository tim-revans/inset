//! `HttpClient` grants: the calls a program makes to other services.
//!
//! A program never makes a call itself. It hands the host each call when it
//! is idle (`Running::deliver` and `Running::resume`), and goes on with
//! other requests while the host makes it. A real grant calls the service at
//! its base address over plain HTTP. A fake one, for requirement tests,
//! answers from the replies the test set up, and fails any call it was not
//! told how to answer.

use std::collections::BTreeMap;

use http_body_util::{BodyExt, Full};
use hyper::Uri;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;

/// A fake `HttpClient`, which a requirement test sets up and reads back.
#[derive(Clone, Debug, Default)]
pub struct FakeClient {
    /// The reply to each call, by method and path: its status and body.
    pub replies: BTreeMap<(String, String), (u16, Vec<u8>)>,
    /// Every call fails, as `given name fails` says.
    pub fails: bool,
    /// The calls made that no reply was set up for, as `GET /path`.
    pub unanswered: Vec<String>,
}

impl FakeClient {
    /// The fake's reply to a call: its status and body, or why it failed.
    pub fn answer(
        &mut self,
        method: &str,
        path: &str,
        grant: &str,
    ) -> Result<(u16, Vec<u8>), String> {
        if self.fails {
            return Err("the client failed, as the test asked".into());
        }
        match self.replies.get(&(method.to_string(), path.to_string())) {
            Some(reply) => Ok(reply.clone()),
            None => {
                let call = format!("{method} {path}");
                if !self.unanswered.contains(&call) {
                    self.unanswered.push(call.clone());
                }
                Err(format!("no `given` fact says how `{grant}` answers {call}"))
            }
        }
    }
}

/// Checks a real grant's base address: plain HTTP, with a host. Returns
/// it without a trailing `/`.
pub(crate) fn check_base(base: &str) -> Result<String, String> {
    let uri: Uri = base
        .parse()
        .map_err(|e| format!("`{base}` is not an address: {e}"))?;
    match uri.scheme_str() {
        Some("http") => {}
        Some("https") => {
            return Err(format!(
                "`{base}` is HTTPS, which an `HttpClient` cannot call yet"
            ));
        }
        _ => return Err(format!("`{base}` does not start with `http://`")),
    }
    if uri.host().is_none() {
        return Err(format!("`{base}` names no host"));
    }
    Ok(base.trim_end_matches('/').to_string())
}

/// Makes a call to a real service: `path` is added to `base`, which
/// [`check_base`] accepted. The reply's status and body, or why there is none.
pub(crate) async fn call(
    base: &str,
    method: &str,
    path: &str,
    body: Vec<u8>,
) -> Result<(u16, Vec<u8>), String> {
    let uri: Uri = format!("{base}{path}")
        .parse()
        .map_err(|e| format!("{base}{path} is not an address: {e}"))?;
    let host = uri.host().unwrap_or("localhost").to_string();
    let port = uri.port_u16().unwrap_or(80);
    let stream = tokio::net::TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| format!("cannot connect to {host}:{port}: {e}"))?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| e.to_string())?;
    tokio::spawn(connection);
    let authority = match uri.port_u16() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let request = hyper::Request::builder()
        .method(method)
        .uri(uri.path_and_query().map_or("/", |p| p.as_str()))
        .header("host", authority)
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|e| e.to_string())?
        .to_bytes()
        .to_vec();
    Ok((status, body))
}
