//! Real `HttpServer` grants: listening on their ports.
//!
//! Connections are accepted on a Tokio thread, and every request is passed to
//! the thread running the program, which handles them one at a time.

use std::net::TcpListener;
use std::sync::mpsc;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::oneshot;

use crate::{Error, Granted, Request, Response, Running, Stopped, describe};

type Delivery = (String, Request, oneshot::Sender<Result<Response, Stopped>>);

/// Serves every `HttpServer` grant on its port, on the loopback interface,
/// until the program stops. Rollbacks are reported on stderr as they happen.
pub fn serve(mut running: Running<'_>) -> Result<(), Error> {
    let mut listeners = Vec::new();
    for (grant, granted) in running.grants() {
        if let Granted::HttpServer { port } = granted {
            let listener = TcpListener::bind(("127.0.0.1", *port))
                .and_then(|l| l.set_nonblocking(true).map(|()| l))
                .map_err(|e| {
                    Error(format!(
                        "grant `{grant}`: cannot listen on port {port}: {e}"
                    ))
                })?;
            eprintln!("serving `{grant}` on http://127.0.0.1:{port}");
            listeners.push((grant.clone(), listener));
        }
    }
    let (deliver, requests) = mpsc::channel();
    std::thread::spawn(move || accept(listeners, deliver));

    let mut reported = running.reports().len();
    for (grant, request, reply) in requests {
        let outcome = running.request(&grant, &request);
        for report in &running.reports()[reported..] {
            eprintln!("{report}");
        }
        reported = running.reports().len();
        let stopped = outcome.as_ref().err().map(describe);
        let _ = reply.send(outcome);
        if let Some(stopped) = stopped {
            return Err(Error(format!("the program stopped: {stopped}")));
        }
    }
    Ok(())
}

fn accept(listeners: Vec<(String, TcpListener)>, deliver: mpsc::Sender<Delivery>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a Tokio runtime starts");
    runtime.block_on(async move {
        for (grant, listener) in listeners {
            let listener =
                tokio::net::TcpListener::from_std(listener).expect("the listener is non-blocking");
            let deliver = deliver.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let grant = grant.clone();
                    let deliver = deliver.clone();
                    let service =
                        service_fn(move |request| handle(grant.clone(), request, deliver.clone()));
                    tokio::spawn(
                        http1::Builder::new().serve_connection(TokioIo::new(stream), service),
                    );
                }
            });
        }
        std::future::pending::<()>().await;
    });
}

async fn handle(
    grant: String,
    request: hyper::Request<Incoming>,
    deliver: mpsc::Sender<Delivery>,
) -> Result<hyper::Response<Full<Bytes>>, hyper::Error> {
    let method = request.method().as_str().to_string();
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_string();
    let body = request.into_body().collect().await?.to_bytes().to_vec();
    let (reply, outcome) = oneshot::channel();
    let request = Request { method, path, body };
    let response = match deliver.send((grant, request, reply)) {
        Ok(()) => outcome.await.ok().and_then(Result::ok),
        Err(_) => None,
    };
    let response = response.unwrap_or(Response {
        status: 500,
        content_type: None,
        body: Vec::new(),
    });
    let mut builder = hyper::Response::builder().status(response.status);
    if let Some(content_type) = response.content_type {
        builder = builder.header("content-type", content_type);
    }
    Ok(builder
        .body(Full::new(Bytes::from(response.body)))
        .expect("a status from the program is valid"))
}
