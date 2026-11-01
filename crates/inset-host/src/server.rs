//! Real `HttpServer` and `HttpClient` grants: listening on their ports, and
//! calling other services.
//!
//! Connections are accepted on a Tokio thread, and every request is passed
//! to the thread running the program. The program takes each request as it
//! arrives, and hands back the calls it waits for, which are made on the
//! Tokio thread while the program goes on with other requests. A call's
//! reply is passed back to the program thread in turn.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::sync::mpsc;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::oneshot;

use crate::{Completion, Error, Granted, Progress, Request, Response, Running, client, describe};

/// What reaches the program thread.
enum Event {
    /// A request through a grant, with where its reply goes.
    Request(String, Request, oneshot::Sender<Response>),
    /// An outgoing call has completed.
    Completed(Completion),
}

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
    let (events, received) = mpsc::channel();
    let (handle_sent, handle) = mpsc::channel();
    let accepting = events.clone();
    std::thread::spawn(move || accept(listeners, accepting, handle_sent));
    let tokio = handle
        .recv()
        .map_err(|_| Error("the server thread did not start".into()))?;

    let mut replies: BTreeMap<u64, oneshot::Sender<Response>> = BTreeMap::new();
    let mut reported = running.reports().len();
    for event in received {
        let outcome = match event {
            Event::Request(grant, request, reply) => {
                running.deliver(&grant, &request).map(|(id, progress)| {
                    replies.insert(id, reply);
                    progress
                })
            }
            Event::Completed(completion) => running.resume(vec![completion]),
        };
        for report in &running.reports()[reported..] {
            eprintln!("{report}");
        }
        reported = running.reports().len();
        let Progress {
            replies: finished,
            calls,
        } = match outcome {
            Ok(progress) => progress,
            Err(stopped) => {
                return Err(Error(format!(
                    "the program stopped: {}",
                    describe(&stopped)
                )));
            }
        };
        for (id, response) in finished {
            if let Some(reply) = replies.remove(&id) {
                let _ = reply.send(response);
            }
        }
        for call in calls {
            let events = events.clone();
            let base = running.base(&call.grant);
            tokio.spawn(async move {
                let outcome = match base {
                    Ok(base) => client::call(&base, &call.method, &call.path, call.body).await,
                    Err(e) => Err(e),
                };
                let completion = Completion {
                    id: call.id,
                    outcome: outcome.map(|(status, body)| crate::Incoming { status, body }),
                };
                let _ = events.send(Event::Completed(completion));
            });
        }
    }
    Ok(())
}

fn accept(
    listeners: Vec<(String, TcpListener)>,
    deliver: mpsc::Sender<Event>,
    runtime_sent: mpsc::Sender<tokio::runtime::Handle>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("a Tokio runtime starts");
    let _ = runtime_sent.send(runtime.handle().clone());
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
    deliver: mpsc::Sender<Event>,
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
    let response = match deliver.send(Event::Request(grant, request, reply)) {
        Ok(()) => outcome.await.ok(),
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
