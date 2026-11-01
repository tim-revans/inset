//! Components as actors: mailboxes, handlers, messages, outlets and failures.
//!
//! Each component is one task that owns its fields and takes envelopes from
//! its mailbox one at a time. An envelope is a request routed to the
//! component, or one of its messages. While a handler runs, its
//! [`HandlerScope`] collects what it sent with `send` and which tables it
//! wrote, so that they take effect only when it completes.
//!
//! A message is a value of the component's `Message` enum, which the
//! compiler emits with one variant per `#[message]` function. Messages are
//! values rather than closures so that the panic hook can write out the
//! messages waiting in a mailbox, and a fresh instance can deliver them.
//!
//! A failed capability call or message does not panic. It records the failure
//! in the scope and never completes, and the component's task then drops the
//! handler's future, which is Rust's way of cancelling it. The response comes
//! from the component's `failure` block, or else from its parent's
//! `Child(name)` arm, and so on up to the app, where the program stops.

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use crate::coverage::Reached;
use crate::executor;
use crate::failure::{Failure, Response, Stopped};
use crate::http::{HttpServer, Reply, Request, Route};
use crate::program;
use crate::stored::Stored;

/// A boxed future that need not be `Send`: the program has one thread.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A call to a handler of `C`, which borrows the component while it runs.
pub type Call<C, R> = Box<dyn for<'a> FnOnce(&'a mut C) -> LocalBoxFuture<'a, R>>;

/// What a handler's reply is delivered by, once its effects have taken hold.
type Deliver = Box<dyn FnOnce()>;

/// A message's reply, before the sender's `Message` gives it its type back.
pub type AnyReply = Box<dyn Any>;

/// What the compiler emits for each Inset component.
pub trait Component: Sized + 'static {
    /// The component's name in the source. A child started without a name
    /// has it as the last part of its diagram ID.
    const NAME: &'static str;

    /// Whether any failure is rolled back, so each handler must save the fields first.
    const ROLLS_BACK: bool = false;

    /// The elements coverage counts in the component, by their IDs relative
    /// to a running component's: entering each function, each branch, each
    /// error path of a `?`, and each arm of the `failure` block. The emitted
    /// code marks them by their index here (see [`crate::coverage`]).
    const BRANCHES: &'static [&'static str] = &[];

    /// The fields that hold state, as opposed to capabilities, outlets and handles.
    type Saved: Stored;

    /// One variant for each `#[message]` function, holding its arguments.
    /// [`NoMessage`] for a component with none.
    type Message: Stored + 'static;

    /// What a [`Handle`] to the component offers: a method for each message.
    /// [`Mailbox`] itself for a component with none.
    type Messages: From<Mailbox<Self>>;

    fn save(&self) -> Self::Saved;

    fn restore(&mut self, saved: Self::Saved);

    /// Handles a message by calling its handler, and returns the reply.
    fn receive(&mut self, message: Self::Message) -> LocalBoxFuture<'_, AnyReply> {
        let _ = message;
        unreachable!("{} has no messages", Self::NAME)
    }

    /// The response to a failure, from the `failure` block, with the index
    /// of the arm that gives it in [`Component::BRANCHES`]. `None` passes it
    /// to the parent.
    fn respond(_failure: &Failure) -> Option<(Response, usize)> {
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

/// The `Message` of a component that has no messages.
pub enum NoMessage {}

impl Stored for NoMessage {
    fn to_json(&self) -> serde_json::Value {
        match *self {}
    }

    fn from_json(_json: &serde_json::Value) -> Result<Self, String> {
        Err("the component has no messages".into())
    }
}

/// Where components are started: the app, or a component starting its children.
pub struct Scope {
    path: String,
    /// The component whose children are started here, once it exists. The
    /// app's is never set, so a failure that reaches it stops the program.
    owner: Rc<OnceCell<Rc<dyn Running>>>,
    /// The component being built, for its own handle: `send self.message`.
    this: Option<Rc<dyn Any>>,
    children: RefCell<Vec<Rc<dyn Running>>>,
}

impl Scope {
    pub(crate) fn root(app: &str) -> Scope {
        Scope {
            path: app.into(),
            owner: Rc::default(),
            this: None,
            children: RefCell::default(),
        }
    }

    /// The diagram ID of the component started in this scope, or the app's.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The handle of a child started in this scope, as `name`. Every handle
    /// is made before any child starts, so a child's outlet can be
    /// connected to a sibling started after it.
    pub fn child<C: Component>(&self, name: &'static str) -> Handle<C> {
        Handle::new(Mailbox {
            field: name,
            actor: Rc::default(),
        })
    }

    /// The handle of the component being built, for connecting a child's
    /// outlet to one of its own messages.
    pub fn this<C: Component>(&self) -> Handle<C> {
        let actor = self
            .this
            .clone()
            .and_then(|this| this.downcast::<OnceCell<Rc<Actor<C>>>>().ok())
            .expect("the component being built is a `C`");
        Handle::new(Mailbox {
            field: "self",
            actor,
        })
    }

    /// `start name: Component { ... };`: builds the child and starts it
    /// behind `handle`. The builder gets the child's scope, for its own
    /// children.
    pub fn start<C: Component>(&self, handle: &Handle<C>, build: impl FnOnce(&Scope) -> C) {
        let name = handle.mailbox.field;
        let slot = handle.mailbox.actor.clone();
        let path = format!("{}/{name}", self.path);
        let scope = Scope {
            path: path.clone(),
            owner: Rc::default(),
            this: Some(slot.clone()),
            children: RefCell::default(),
        };
        let mut component = build(&scope);
        component.label();
        let initial = component.save().to_json();
        let actor = Rc::new(Actor {
            id: path,
            name,
            parent: self.owner.clone(),
            children: scope.children.take(),
            initial,
            reached: Rc::new(Reached::of::<C>()),
            reset: Cell::new(false),
            component: RefCell::new(component),
            mailbox: RefCell::default(),
            waker: RefCell::default(),
            saved: RefCell::default(),
            running: RefCell::default(),
        });
        let running: Rc<dyn Running> = actor.clone();
        if scope.owner.set(running.clone()).is_err() || slot.set(actor.clone()).is_err() {
            unreachable!("each child is started once");
        }
        self.children.borrow_mut().push(running);
        for (field, server) in actor.component.borrow().servers() {
            program::serve(server.grant(), field, actor.clone());
        }
        program::register(actor.clone());
        executor::spawn(run(actor));
    }
}

/// Where a component's messages are posted. A handle and an outlet each
/// hold one.
pub struct Mailbox<C: Component> {
    /// The field of the component holding the handle, which a failed
    /// message names: `Message(field)`.
    field: &'static str,
    actor: Rc<OnceCell<Rc<Actor<C>>>>,
}

impl<C: Component> Mailbox<C> {
    /// A message to the component. The compiler emits one method per
    /// `#[message]` function, each built on this, and `R` is the function's
    /// return type.
    pub fn message<R: 'static>(&self, message: C::Message) -> Message<C, R> {
        Message {
            field: self.field,
            actor: self.actor().clone(),
            message,
            reply: PhantomData,
        }
    }

    fn actor(&self) -> &Rc<Actor<C>> {
        self.actor
            .get()
            .expect("a component starts before anything can send it a message")
    }

    fn copy(&self) -> Mailbox<C> {
        Mailbox {
            field: self.field,
            actor: self.actor.clone(),
        }
    }
}

/// A running component, which its holder can send messages to: a method
/// for each of its messages, through [`Component::Messages`].
pub struct Handle<C: Component> {
    mailbox: Mailbox<C>,
    messages: C::Messages,
}

impl<C: Component> Handle<C> {
    fn new(mailbox: Mailbox<C>) -> Handle<C> {
        Handle {
            messages: C::Messages::from(mailbox.copy()),
            mailbox,
        }
    }
}

impl<C: Component> Deref for Handle<C> {
    type Target = C::Messages;

    fn deref(&self) -> &C::Messages {
        &self.messages
    }
}

/// A message not yet sent. Awaiting it sends it and waits for the reply;
/// [`send`] sends it without waiting.
pub struct Message<C: Component, R> {
    field: &'static str,
    actor: Rc<Actor<C>>,
    message: C::Message,
    reply: PhantomData<fn() -> R>,
}

impl<C: Component, R: 'static> IntoFuture for Message<C, R> {
    type Output = R;
    type IntoFuture = LocalBoxFuture<'static, R>;

    /// Puts the message in the receiver's mailbox straight away, so it is
    /// there before the sender carries on.
    fn into_future(self) -> Self::IntoFuture {
        let (sender, reply) = reply_channel();
        self.actor.post(Envelope::Awaited(self.message, sender));
        let field = self.field;
        let callee = self.actor.id.clone();
        Box::pin(async move {
            current().set_awaiting(Some(field));
            match reply.await {
                Some(value) => {
                    current().set_awaiting(None);
                    *value
                        .downcast::<R>()
                        .expect("a message's reply has its handler's return type")
                }
                None => fail(Failure::Message(field), format!("{callee} failed")).await,
            }
        })
    }
}

/// What `send` can send: a message that returns nothing, or a value
/// emitted through an outlet.
pub trait Sendable {
    /// Posts the message. Called when the sending handler completes.
    fn deliver(self) -> Deliver;
}

impl<C: Component> Sendable for Message<C, ()> {
    fn deliver(self) -> Deliver {
        Box::new(move || self.actor.post(Envelope::Sent(self.message)))
    }
}

/// `send`: delivers a message when the sending handler completes, without
/// waiting for it to be handled. A handler that fails never delivers it.
pub fn send(message: impl Sendable) {
    current().hold(message.deliver());
}

/// `Outlet<T>`: announces values to the message its owner's parent
/// connected it to, without knowing who receives them.
pub struct Outlet<T> {
    target: Box<dyn Fn(T) -> Deliver>,
}

impl<T: 'static> Outlet<T> {
    /// `field: send target.message`, connecting the outlet to `message` of
    /// the component behind `target`.
    pub fn to<C: Component>(
        target: &Handle<C>,
        message: impl Fn(&C::Messages, T) -> Message<C, ()> + 'static,
    ) -> Outlet<T> {
        let messages = C::Messages::from(target.mailbox.copy());
        Outlet {
            target: Box::new(move |value| message(&messages, value).deliver()),
        }
    }

    /// The value to deliver, which `send` delivers when the emitting handler completes.
    pub fn emit(&self, value: T) -> Emitted {
        Emitted((self.target)(value))
    }
}

/// A value emitted through an outlet, which only `send` can deliver.
pub struct Emitted(Deliver);

impl Sendable for Emitted {
    fn deliver(self) -> Deliver {
        self.0
    }
}

/// A handler call waiting in a mailbox.
enum Envelope<C: Component> {
    /// A request, routed to its handler.
    Request(Call<C, Deliver>),
    /// A message whose sender waits for the reply.
    Awaited(C::Message, ReplySender<AnyReply>),
    /// A message sent with `send`, or emitted through an outlet.
    Sent(C::Message),
}

impl<C: Component> Envelope<C> {
    /// The handler's future, which yields what delivers its reply.
    fn open(self, component: &mut C) -> LocalBoxFuture<'_, Deliver> {
        match self {
            Envelope::Request(call) => call(component),
            Envelope::Awaited(message, sender) => Box::pin(async move {
                let reply = component.receive(message).await;
                Box::new(move || sender.send(reply)) as Deliver
            }),
            Envelope::Sent(message) => Box::pin(async move {
                component.receive(message).await;
                Box::new(|| ()) as Deliver
            }),
        }
    }
}

pub(crate) struct Actor<C: Component> {
    pub(crate) id: String,
    /// The last part of `id`: the child's name, or its component's.
    name: &'static str,
    parent: Rc<OnceCell<Rc<dyn Running>>>,
    children: Vec<Rc<dyn Running>>,
    /// The state fields as the component started, which a restart restores.
    initial: serde_json::Value,
    /// What coverage counts in the component, and which of it has run.
    reached: Rc<Reached>,
    /// Set by a restart, and done before the next envelope.
    reset: Cell<bool>,
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
        self.wake();
    }

    fn wake(&self) {
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }

    /// Routes a request to its handler, which is put in the mailbox when
    /// the request is admitted. A request that matches no handler is
    /// answered straight away.
    pub(crate) fn serve(self: Rc<Self>, field: &str, request: &Request) -> Result<Routed, Reply> {
        let (name, handler) = match C::route(field, request) {
            Route::NotFound => return Err(Reply::empty(404)),
            Route::BadRequest => return Err(Reply::empty(400)),
            Route::To(name, handler) => (name, handler),
        };
        Ok(Routed {
            handler: format!("{}/fn:{name}", self.id),
            post: Box::new(move || {
                let (sender, reply) = reply_channel();
                self.post(Envelope::Request(Box::new(move |component| {
                    Box::pin(async move {
                        let value = handler(component).await;
                        Box::new(move || sender.send(value)) as Deliver
                    })
                })));
                reply
            }),
        })
    }

    /// Whether each handler must save the fields first, because a failure
    /// in it may be rolled back: by the component's own `failure` block, or
    /// by an ancestor's `Child(name) => rollback`.
    fn saves(&self) -> bool {
        C::ROLLS_BACK || ancestors_roll_back(self)
    }

    /// Restores the fields the component started with.
    fn reset_now(&self) {
        let initial = C::Saved::from_json(&self.initial)
            .expect("the state a component started with reads back");
        self.component.borrow_mut().restore(initial);
    }

    /// Applies the response to a failed handler, whose future has been dropped.
    fn failed(
        self: &Rc<Self>,
        scope: &HandlerScope,
        saved: Option<C::Saved>,
        failure: Failure,
        message: String,
    ) {
        let this: Rc<dyn Running> = self.clone();
        match resolve(this, &failure) {
            Some((Response::Rollback, _)) => {
                if let Some(saved) = saved {
                    self.component.borrow_mut().restore(saved);
                }
                scope.roll_back();
                program::report_rolled_back(&self.id, &failure, &message);
            }
            Some((Response::Restart, restarted)) => {
                scope.roll_back();
                restarted.restart();
                program::report_restarted(&self.id, &failure, &message, restarted.id());
            }
            None => {
                scope.roll_back();
                program::stop(Stopped {
                    component: self.id.clone(),
                    failure: failure.to_string(),
                    message,
                });
            }
        }
    }
}

/// A request routed to its handler, waiting to be admitted.
pub(crate) struct Routed {
    /// The handler's diagram ID, such as `Shop/Front/fn:add`.
    pub handler: String,
    /// Puts the handler in its component's mailbox, and returns where its
    /// reply will arrive.
    pub post: Box<dyn FnOnce() -> ReplyReceiver<Reply>>,
}

/// The response to a failure in `actor`, and the component it applies to.
/// A component that does not respond passes the failure to its parent as
/// `Child(name)`, and a restart then restarts that child. `None` when the
/// failure reaches the app, which stops the program.
pub(crate) fn resolve(
    actor: Rc<dyn Running>,
    failure: &Failure,
) -> Option<(Response, Rc<dyn Running>)> {
    if let Some((response, arm)) = actor.responds(failure) {
        actor.mark(arm);
        return Some((response, actor));
    }
    let mut child = actor;
    while let Some(parent) = child.parent() {
        if let Some((response, arm)) = parent.responds(&Failure::Child(child.name())) {
            parent.mark(arm);
            return Some((response, child));
        }
        child = parent;
    }
    None
}

/// Whether a failure `actor` does not respond to would be rolled back by
/// an ancestor.
fn ancestors_roll_back<C: Component>(actor: &Actor<C>) -> bool {
    let mut name = actor.name;
    let mut parent = actor.parent.get().cloned();
    while let Some(p) = parent {
        match p.responds(&Failure::Child(name)) {
            Some((Response::Rollback, _)) => return true,
            Some((Response::Restart, _)) => return false,
            None => {
                name = p.name();
                parent = p.parent();
            }
        }
    }
    false
}

/// The component's task: one handler at a time, in the order they arrive.
#[expect(
    clippy::await_holding_refcell_ref,
    reason = "the task owns the component; the RefCell only lets the panic hook see that a handler holds it"
)]
async fn run<C: Component>(actor: Rc<Actor<C>>) {
    loop {
        let envelope = std::future::poll_fn(|cx| {
            if actor.reset.take() {
                actor.reset_now();
            }
            match actor.mailbox.borrow_mut().pop_front() {
                Some(envelope) => Poll::Ready(envelope),
                None => {
                    *actor.waker.borrow_mut() = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await;

        let scope = Rc::new(HandlerScope {
            reached: Some(actor.reached.clone()),
            ..HandlerScope::default()
        });
        if actor.saves() {
            *actor.saved.borrow_mut() = Some(actor.component.borrow().save());
        }
        *actor.running.borrow_mut() = Some(scope.clone());
        let outcome = {
            let mut component = actor.component.borrow_mut();
            InScope {
                scope: &scope,
                future: envelope.open(&mut component),
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
            Err((failure, message)) => actor.failed(&scope, saved, failure, message),
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
    held: RefCell<Vec<Deliver>>,
    written: RefCell<Vec<Rc<dyn Transaction>>>,
    /// The field of the component whose reply the handler is waiting for.
    awaiting: RefCell<Option<&'static str>>,
    /// The field of the `HttpClient` whose call the handler is waiting for.
    calling: RefCell<Option<&'static str>>,
    /// The coverage flags of the component the handler belongs to.
    reached: Option<Rc<Reached>>,
}

impl HandlerScope {
    pub(crate) fn reached(&self) -> Option<&Reached> {
        self.reached.as_deref()
    }

    fn hold(&self, delivery: Deliver) {
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

    pub(crate) fn set_calling(&self, field: Option<&'static str>) {
        *self.calling.borrow_mut() = field;
    }

    pub(crate) fn calling(&self) -> Option<&'static str> {
        *self.calling.borrow()
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
    /// Whether the reply has come, or the handler has failed.
    pub(crate) fn ready(&self) -> bool {
        let slot = self.0.borrow();
        slot.value.is_some() || slot.closed
    }

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
    fn name(&self) -> &'static str;
    fn parent(&self) -> Option<Rc<dyn Running>>;
    fn serve(self: Rc<Self>, field: &str, request: &Request) -> Result<Routed, Reply>;
    /// Whether a handler is running, or an envelope waits in the mailbox.
    fn busy(&self) -> bool;
    /// The running handler's scope, if a handler is running.
    fn running(&self) -> Option<Rc<HandlerScope>>;
    /// The response to a failure, and the index of the arm that gives it.
    fn responds(&self, failure: &Failure) -> Option<(Response, usize)>;
    /// Marks an element of the component reached, by its index.
    fn mark(&self, index: usize);
    /// The elements reached since the last call, by their full IDs.
    fn take_reached(&self, out: &mut Vec<String>);
    /// Resets the component and its children to how they started, each
    /// before it takes its next envelope.
    fn restart(&self);
    /// The state to start again from: the fields before the running handler,
    /// the current fields when none is running, or the fields it started
    /// with if it is `restarted`. `None` if a running handler cannot be
    /// rolled back.
    fn snapshot(&self, restarted: bool) -> Option<serde_json::Value>;
    /// The messages waiting in the mailbox, in their stored form. `None` if
    /// one has a sender waiting for its reply, which a fresh instance could
    /// not give it.
    fn queued(&self) -> Option<Vec<serde_json::Value>>;
    fn restore(&self, state: serde_json::Value) -> Result<(), String>;
    /// Posts messages a fresh instance was handed, as `send` would have.
    fn requeue(&self, messages: Vec<serde_json::Value>) -> Result<(), String>;
}

impl<C: Component> Running for Actor<C> {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn parent(&self) -> Option<Rc<dyn Running>> {
        self.parent.get().cloned()
    }

    fn serve(self: Rc<Self>, field: &str, request: &Request) -> Result<Routed, Reply> {
        Actor::serve(self, field, request)
    }

    fn busy(&self) -> bool {
        self.running.borrow().is_some() || !self.mailbox.borrow().is_empty()
    }

    fn running(&self) -> Option<Rc<HandlerScope>> {
        self.running.borrow().clone()
    }

    fn responds(&self, failure: &Failure) -> Option<(Response, usize)> {
        C::respond(failure)
    }

    fn mark(&self, index: usize) {
        self.reached.mark(index);
    }

    fn take_reached(&self, out: &mut Vec<String>) {
        self.reached.take(Some(&self.id), out);
    }

    fn restart(&self) {
        self.reset.set(true);
        self.wake();
        for child in &self.children {
            child.restart();
        }
    }

    fn snapshot(&self, restarted: bool) -> Option<serde_json::Value> {
        if restarted || self.reset.get() {
            return Some(self.initial.clone());
        }
        let saved = match self.component.try_borrow() {
            Ok(component) => component.save(),
            // A handler is running, and holds the fields.
            Err(_) => return Some(self.saved.borrow().as_ref()?.to_json()),
        };
        Some(saved.to_json())
    }

    fn queued(&self) -> Option<Vec<serde_json::Value>> {
        self.mailbox
            .borrow()
            .iter()
            .map(|envelope| match envelope {
                Envelope::Sent(message) => Some(message.to_json()),
                Envelope::Request(_) | Envelope::Awaited(..) => None,
            })
            .collect()
    }

    fn restore(&self, state: serde_json::Value) -> Result<(), String> {
        let saved = C::Saved::from_json(&state).map_err(|e| format!("{}: {e}", self.id))?;
        self.component.borrow_mut().restore(saved);
        Ok(())
    }

    fn requeue(&self, messages: Vec<serde_json::Value>) -> Result<(), String> {
        for json in messages {
            let message = C::Message::from_json(&json).map_err(|e| format!("{}: {e}", self.id))?;
            self.post(Envelope::Sent(message));
        }
        Ok(())
    }
}
