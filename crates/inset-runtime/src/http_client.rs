//! The `HttpClient` capability: calls to another service, which the host
//! makes while the program goes on with other requests.
//!
//! A call does not go through an import, because an import would hold up
//! the whole program until it returned. The call is recorded here instead,
//! and its handler waits. When the program is idle, the host is handed every
//! call recorded since it was last idle (see `deliver` and `resume` in
//! `wit/inset.wit`), makes them, and hands back each reply through `resume`,
//! which wakes the handler that made the call.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::task::{Poll, Waker};

use crate::actor;
use crate::failure::Failure;

/// A call waiting for the host to make it.
pub(crate) struct Outgoing {
    pub id: u64,
    pub grant: String,
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

/// What the host reports for a call: the status and body of the reply, or
/// why there is none.
type Completed = Result<(u16, Vec<u8>), String>;

#[derive(Default)]
struct Slot {
    result: Option<Completed>,
    waker: Option<Waker>,
}

#[derive(Default)]
struct Calls {
    next: u64,
    /// Calls made since the host was last handed them.
    outgoing: Vec<Outgoing>,
    /// Calls the host has not completed, by ID.
    slots: BTreeMap<u64, Rc<RefCell<Slot>>>,
}

thread_local! {
    static CALLS: RefCell<Calls> = RefCell::default();
}

/// The calls made since the host was last handed them, in the order made.
pub(crate) fn take_calls() -> Vec<Outgoing> {
    CALLS.with_borrow_mut(|c| std::mem::take(&mut c.outgoing))
}

/// Hands each completed call its result, and wakes the handler waiting for it.
pub(crate) fn complete(completions: impl IntoIterator<Item = (u64, Completed)>) {
    for (id, result) in completions {
        let Some(slot) = CALLS.with_borrow_mut(|c| c.slots.remove(&id)) else {
            continue;
        };
        let mut slot = slot.borrow_mut();
        slot.result = Some(result);
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    }
}

/// The `HttpClient` capability: calls to the service at the grant's base
/// address. A reply with a status from 200 to 299 is `Ok` with its body,
/// and any other is `Err` with its status. A call that gets no reply fails
/// the handler, as a failed table does: `Capability(field)`.
pub struct HttpClient {
    grant: String,
    field: &'static str,
}

impl HttpClient {
    pub(crate) fn new(grant: &str) -> HttpClient {
        HttpClient {
            grant: grant.into(),
            field: "",
        }
    }

    /// Names the field that holds the client, which a failure reports.
    pub fn label(&mut self, field: &'static str) {
        self.field = field;
    }

    pub async fn get(&self, path: &str) -> Result<String, u16> {
        self.call("GET", path, Vec::new()).await
    }

    pub async fn delete(&self, path: &str) -> Result<String, u16> {
        self.call("DELETE", path, Vec::new()).await
    }

    pub async fn post(&self, path: &str, body: &str) -> Result<String, u16> {
        self.call("POST", path, body.as_bytes().to_vec()).await
    }

    pub async fn put(&self, path: &str, body: &str) -> Result<String, u16> {
        self.call("PUT", path, body.as_bytes().to_vec()).await
    }

    async fn call(&self, method: &str, path: &str, body: Vec<u8>) -> Result<String, u16> {
        let slot = Rc::new(RefCell::new(Slot::default()));
        CALLS.with_borrow_mut(|c| {
            let id = c.next;
            c.next += 1;
            c.outgoing.push(Outgoing {
                id,
                grant: self.grant.clone(),
                method: method.into(),
                path: path.into(),
                body,
            });
            c.slots.insert(id, slot.clone());
        });
        let scope = actor::current();
        scope.set_calling(Some(self.field));
        let result = std::future::poll_fn(|cx| {
            let mut slot = slot.borrow_mut();
            match slot.result.take() {
                Some(result) => Poll::Ready(result),
                None => {
                    slot.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await;
        scope.set_calling(None);
        let failed = match result {
            Ok((status, body)) if (200..300).contains(&status) => match String::from_utf8(body) {
                Ok(text) => return Ok(text),
                Err(_) => format!("the reply to {method} {path} is not UTF-8 text"),
            },
            Ok((status, _)) => return Err(status),
            Err(e) => format!("{method} {path} got no reply: {e}"),
        };
        fail(self.field, failed).await
    }
}

fn fail<T>(field: &'static str, message: String) -> impl Future<Output = T> {
    actor::fail(Failure::Capability(field), message)
}
