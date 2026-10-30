//! Components as actors: mailboxes, handlers, messages and failures.
//!
//! Each component is one task that owns its fields and takes envelopes from
//! its mailbox one at a time. An envelope is a handler call: a request routed
//! to the component, or a message from another component. While a handler
//! runs, its [`HandlerScope`] collects what it sent with `send` and which
//! tables it wrote, so that they take effect only when it completes.
//!
//! A failed capability call or message does not panic. It records the failure
//! in the scope and never completes, and the component's task then drops the
//! handler's future, which is Rust's way of cancelling it.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::executor;
use crate::failure::{Failure, Response, Stopped};
use crate::http::{HttpServer, Reply, Request, Route};
use crate::program;

/// A boxed future that need not be `Send`: the program has one thread.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A call to a handler of `C`, which borrows the component while it runs.
pub type Call<C, R> = Box<dyn for<'a> FnOnce(&'a mut C) -> LocalBoxFuture<'a, R>>;

/// A handler call waiting in a mailbox. Its future yields what delivers the reply.
type Envelope<C> = Call<C, Deliver>;
type Deliver = Box<dyn FnOnce()>;

/// What the compiler emits for each Inset component.
pub trait Component: Sized + 'static {
    /// The component's name in the source. A child started without a name
    /// has it as the last part of its diagram ID.
    const NAME: &'static str;

    /// Whether any failure is rolled back, so each handler must save the fields first.
    const ROLLS_BACK: bool = false;

    /// The fields that hold state, as opposed to capabilities, outlets and handles.
    type Saved: Serialize + DeserializeOwned;

    fn save(&self) -> Self::Saved;

    fn restore(&mut self, saved: Self::Saved);

    /// The response to a failure, from the `failure` block. `None` stops the program.
    fn respond(_failure: &Failure) -> Option<Response> {
        None
    }

    /// Tells each capability field its name, which a failure in it reports.
    fn label(&mut self) {}

    /// The `HttpServer` fields and their names.
    fn servers(&self) -> Vec<(&'static str, &HttpServer)> {
        Vec::new()
    }

    /// The handler for a request that arrived through the server in `field`.
    fn route(_field: &str, _request: &Request) -> Route<Self> {
        Route::NotFound
    }
}

/// Where components are started: the app, or a component starting its children.
pub struct Scope {
    path: String,
}

impl Scope {
    pub(crate) fn root(app: &str) -> Scope {
        Scope { path: app.into() }
    }

    /// `start Component { ... };`: a child nothing sends messages to.
    pub fn start<C: Component>(&self, build: impl FnOnce(&Scope) -> C) {
        self.spawn(C::NAME, build);
    }

    /// `start name: Component { ... };`: a child its parent can message as `name`.
    pub fn start_as<C: Component>(
        &self,
        name: &'static str,
        build: impl FnOnce(&Scope) -> C,
    ) -> Handle<C> {
        Handle {
            field: name,
            actor: self.spawn(name, build),
        }
    }

    fn spawn<C: Component>(&self, name: &str, build: impl FnOnce(&Scope) -> C) -> Rc<Actor<C>> {
        let path = format!("{}/{name}", self.path);
        let mut component = build(&Scope { path: path.clone() });
        component.label();
        let actor = Rc::new(Actor {
            id: path,
            component: RefCell::new(component),
            mailbox: RefCell::default(),
            waker: RefCell::default(),
            saved: RefCell::default(),
            running: RefCell::default(),
        });
        for (field, server) in actor.component.borrow().servers() {
            program::serve(server.grant(), field, actor.clone());
        }
        program::register(actor.clone());
        executor::spawn(run(actor.clone()));
        actor
    }
}

/// A running component, which its holder can send messages to.
pub struct Handle<C: Component> {
    field: &'static str,
    actor: Rc<Actor<C>>,
}

impl<C: Component> Handle<C> {
    /// A message to the component. The compiler emits one method per
    /// `#[message]` function, each built on this.
    pub fn message<R: 'static>(
        &self,
        call: impl for<'a> FnOnce(&'a mut C) -> LocalBoxFuture<'a, R> + 'static,
    ) -> Message<C, R> {
        Message {
            field: self.field,
            actor: self.actor.clone(),
            call: Box::new(call),
        }
    }
}

/// A message not yet sent. Awaiting it sends it and waits for the reply;
/// [`send`] sends it without waiting.
pub struct Message<C: Component, R> {
    field: &'static str,
    actor: Rc<Actor<C>>,
    call: Call<C, R>,
}

impl<C: Component, R: 'static> IntoFuture for Message<C, R> {
    type Output = R;
    type IntoFuture = LocalBoxFuture<'static, R>;

    /// Puts the message in the receiver's mailbox straight away, so it is
    /// there before the sender carries on.
    fn into_future(self) -> Self::IntoFuture {
        let (sender, reply) = reply_channel();
        let call = self.call;
        self.actor.post(Box::new(move |component| {
            Box::pin(async move {
                let value = call(component).await;
                Box::new(move || sender.send(value)) as Deliver
            })
        }));
        let field = self.field;
        let callee = self.actor.id.clone();
        Box::pin(async move {
            current().set_awaiting(Some(field));
            match reply.await {
                Some(value) => {
                    current().set_awaiting(None);
                    value
                }
                None => fail(Failure::Message(field), format!("{callee} failed")).await,
            }
        })
    }
}

/// `send`: delivers a message when the sending handler completes, without
/// waiting for it to be handled. A handler that fails never delivers it.
pub fn send<C: Component>(message: Message<C, ()>) {
    let Message { actor, call, .. } = message;
    current().hold(Box::new(move || {
        actor.post(Box::new(move |component| {
            Box::pin(async move {
                call(component).await;
                Box::new(|| ()) as Deliver
            })
        }));
    }));
}

pub(crate) struct Actor<C: Component> {
    pub(crate) id: String,
    component: RefCell<C>,
    mailbox: RefCell<VecDeque<Envelope<C>>>,
    waker: RefCell<Option<Waker>>,
    /// The fields as they were before the running handler, if it can be rolled back.
    saved: RefCell<Option<C::Saved>>,
    running: RefCell<Option<Rc<HandlerScope>>>,
}

impl<C: Component> Actor<C> {
    fn post(&self, envelope: Envelope<C>) {
        self.mailbox.borrow_mut().push_back(envelope);
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }

    /// Routes a request, and puts its handler in the mailbox. A request that
    /// matches no handler is answered straight away.
    pub(crate) fn serve(
        &self,
        field: &str,
        request: &Request,
    ) -> Result<ReplyReceiver<Reply>, Reply> {
        let handler = match C::route(field, request) {
            Route::NotFound => return Err(Reply::empty(404)),
            Route::BadRequest => return Err(Reply::empty(400)),
            Route::To(handler) => handler,
        };
        let (sender, reply) = reply_channel();
        self.post(Box::new(move |component| {
            Box::pin(async move {
                let value = handler(component).await;
                Box::new(move || sender.send(value)) as Deliver
            })
        }));
        Ok(reply)
    }
}

/// The component's task: one handler at a time, in the order they arrive.
#[expect(
    clippy::await_holding_refcell_ref,
    reason = "the task owns the component; the RefCell only lets the panic hook see that a handler holds it"
)]
async fn run<C: Component>(actor: Rc<Actor<C>>) {
    loop {
        let envelope = std::future::poll_fn(|cx| match actor.mailbox.borrow_mut().pop_front() {
            Some(envelope) => Poll::Ready(envelope),
            None => {
                *actor.waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await;

        let scope = Rc::new(HandlerScope::default());
        if C::ROLLS_BACK {
            *actor.saved.borrow_mut() = Some(actor.component.borrow().save());
        }
        *actor.running.borrow_mut() = Some(scope.clone());
        let outcome = {
            let mut component = actor.component.borrow_mut();
            InScope {
                scope: &scope,
                future: envelope(&mut component),
            }
            .await
        };
        actor.running.take();
        let saved = actor.saved.take();

        match outcome {
            Ok(deliver) => {
                scope.complete();
                deliver();
            }
            Err((failure, message)) => match C::respond(&failure) {
                Some(Response::Rollback) => {
                    if let Some(saved) = saved {
                        actor.component.borrow_mut().restore(saved);
                    }
                    scope.roll_back();
                    program::report_rolled_back(&actor.id, &failure, &message);
                }
                None => {
                    scope.roll_back();
                    program::stop(Stopped {
                        component: actor.id.clone(),
                        failure: failure.to_string(),
                        message,
                    });
                }
            },
        }
    }
}

/// Polls a handler with its scope as the current one, and ends it when it fails.
struct InScope<'s, F> {
    scope: &'s Rc<HandlerScope>,
    future: F,
}

impl<F: Future<Output = Deliver> + Unpin> Future for InScope<'_, F> {
    type Output = Result<Deliver, (Failure, String)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let previous = CURRENT.replace(Some(self.scope.clone()));
        let poll = Pin::new(&mut self.future).poll(cx);
        CURRENT.set(previous);
        match poll {
            Poll::Ready(deliver) => Poll::Ready(Ok(deliver)),
            Poll::Pending => match self.scope.failure.take() {
                Some(failure) => Poll::Ready(Err(failure)),
                None => Poll::Pending,
            },
        }
    }
}

/// A capability whose writes are held until the handler that made them completes.
pub(crate) trait Transaction {
    fn commit(&self);
    fn roll_back(&self);
}

/// What one running handler has done that takes effect only if it completes.
#[derive(Default)]
pub(crate) struct HandlerScope {
    failure: RefCell<Option<(Failure, String)>>,
    held: RefCell<Vec<Box<dyn FnOnce()>>>,
    written: RefCell<Vec<Rc<dyn Transaction>>>,
    /// The field of the component whose reply the handler is waiting for.
    awaiting: RefCell<Option<&'static str>>,
}

impl HandlerScope {
    fn hold(&self, delivery: Box<dyn FnOnce()>) {
        self.held.borrow_mut().push(delivery);
    }

    pub(crate) fn wrote(&self, capability: Rc<dyn Transaction>) {
        let mut written = self.written.borrow_mut();
        if !written.iter().any(|c| Rc::ptr_eq(c, &capability)) {
            written.push(capability);
        }
    }

    fn set_awaiting(&self, field: Option<&'static str>) {
        *self.awaiting.borrow_mut() = field;
    }

    pub(crate) fn awaiting(&self) -> Option<&'static str> {
        *self.awaiting.borrow()
    }

    /// Commits the writes, then delivers the held messages, before the reply.
    fn complete(&self) {
        for capability in self.written.take() {
            capability.commit();
        }
        for delivery in self.held.take() {
            delivery();
        }
    }

    fn roll_back(&self) {
        for capability in self.written.take() {
            capability.roll_back();
        }
        self.held.take();
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Rc<HandlerScope>>> = const { RefCell::new(None) };
}

/// The scope of the handler being polled.
pub(crate) fn current() -> Rc<HandlerScope> {
    try_current().expect("only a handler sends messages or calls capabilities")
}

pub(crate) fn try_current() -> Option<Rc<HandlerScope>> {
    CURRENT.with_borrow(|current| current.clone())
}

/// Fails the running handler. The future never completes, and the
/// component's task drops it.
pub(crate) fn fail<T>(failure: Failure, message: String) -> impl Future<Output = T> {
    *current().failure.borrow_mut() = Some((failure, message));
    std::future::pending()
}

/// A one-shot reply. A sender dropped without sending means the handler failed.
fn reply_channel<T>() -> (ReplySender<T>, ReplyReceiver<T>) {
    let slot = Rc::new(RefCell::new(Slot {
        value: None,
        closed: false,
        waker: None,
    }));
    (ReplySender(slot.clone()), ReplyReceiver(slot))
}

struct Slot<T> {
    value: Option<T>,
    closed: bool,
    waker: Option<Waker>,
}

struct ReplySender<T>(Rc<RefCell<Slot<T>>>);

impl<T> ReplySender<T> {
    fn send(self, value: T) {
        self.0.borrow_mut().value = Some(value);
    }
}

impl<T> Drop for ReplySender<T> {
    fn drop(&mut self) {
        let mut slot = self.0.borrow_mut();
        slot.closed = true;
        if let Some(waker) = slot.waker.take() {
            waker.wake();
        }
    }
}

pub(crate) struct ReplyReceiver<T>(Rc<RefCell<Slot<T>>>);

impl<T> ReplyReceiver<T> {
    /// The reply, or `None` if the handler failed. `Err` while it is still running.
    pub(crate) fn try_take(&self) -> Result<Option<T>, ()> {
        let mut slot = self.0.borrow_mut();
        match (slot.value.take(), slot.closed) {
            (Some(value), _) => Ok(Some(value)),
            (None, true) => Ok(None),
            (None, false) => Err(()),
        }
    }
}

impl<T> Future for ReplyReceiver<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        match self.try_take() {
            Ok(reply) => Poll::Ready(reply),
            Err(()) => {
                self.0.borrow_mut().waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// An actor as the program sees it, whatever its component type.
pub(crate) trait Running {
    fn id(&self) -> &str;
    fn serve(&self, field: &str, request: &Request) -> Result<ReplyReceiver<Reply>, Reply>;
    /// The running handler's scope, if a handler is running.
    fn running(&self) -> Option<Rc<HandlerScope>>;
    fn queued(&self) -> usize;
    fn responds(&self, failure: &Failure) -> Option<Response>;
    /// The state to start again from: the fields before the running handler,
    /// or the current fields when none is running. `None` if a running handler
    /// cannot be rolled back.
    fn snapshot(&self) -> Option<serde_json::Value>;
    fn restore(&self, state: serde_json::Value) -> Result<(), String>;
}

impl<C: Component> Running for Actor<C> {
    fn id(&self) -> &str {
        &self.id
    }

    fn serve(&self, field: &str, request: &Request) -> Result<ReplyReceiver<Reply>, Reply> {
        Actor::serve(self, field, request)
    }

    fn running(&self) -> Option<Rc<HandlerScope>> {
        self.running.borrow().clone()
    }

    fn queued(&self) -> usize {
        self.mailbox.borrow().len()
    }

    fn responds(&self, failure: &Failure) -> Option<Response> {
        C::respond(failure)
    }

    fn snapshot(&self) -> Option<serde_json::Value> {
        let saved = match self.component.try_borrow() {
            Ok(component) => component.save(),
            // A handler is running, and holds the fields.
            Err(_) => return serde_json::to_value(self.saved.borrow().as_ref()?).ok(),
        };
        serde_json::to_value(saved).ok()
    }

    fn restore(&self, state: serde_json::Value) -> Result<(), String> {
        let saved = serde_json::from_value(state).map_err(|e| format!("{}: {e}", self.id))?;
        self.component.borrow_mut().restore(saved);
        Ok(())
    }
}
