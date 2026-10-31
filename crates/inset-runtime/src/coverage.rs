//! Coverage: which parts of the program have run, by diagram ID, so that
//! `inset test` can tell which requirement reached which branch.
//!
//! The compiler gives each component type a list of the elements coverage
//! counts in it, [`Component::BRANCHES`], by their IDs relative to a running
//! component: entering each function, each branch of a decision or loop,
//! each error path of a `?`, and each arm of the `failure` block. Every
//! running component keeps a flag for each, which the emitted code sets with
//! [`branch`] and [`exit`] as it passes. The top-level algorithms have no
//! component, so their elements are listed whole in [`App::BRANCHES`] and
//! set with [`algorithm`] and [`algorithm_exit`].
//!
//! When a request has been handled, when the program has started, and
//! before a panic traps the instance, the runtime reports what was reached
//! since the last report to the host, each element once, by its full ID.
//! So a program that runs for a long time reports each element only the
//! first time it runs, and a flag costs one load and one store after that.
//!
//! [`App::BRANCHES`]: crate::App::BRANCHES

use std::any::TypeId;
use std::cell::{Cell, RefCell};

use crate::actor::{self, Component};

/// Not reached since the program started.
const UNREACHED: u8 = 0;
/// Reached, and not reported yet.
const REACHED: u8 = 1;
/// Reached and reported.
const REPORTED: u8 = 2;

/// One flag for each element a component type lists, or the app's.
pub(crate) struct Reached {
    /// The component type the flags are for, so a component's function
    /// called while another component's handler runs marks nothing.
    owner: TypeId,
    names: &'static [&'static str],
    flags: Box<[Cell<u8>]>,
}

impl Reached {
    pub(crate) fn of<C: Component>() -> Reached {
        Reached::new(TypeId::of::<C>(), C::BRANCHES)
    }

    fn new(owner: TypeId, names: &'static [&'static str]) -> Reached {
        Reached {
            owner,
            names,
            flags: names.iter().map(|_| Cell::new(UNREACHED)).collect(),
        }
    }

    pub(crate) fn mark(&self, index: usize) {
        if let Some(flag) = self.flags.get(index)
            && flag.get() == UNREACHED
        {
            flag.set(REACHED);
        }
    }

    /// The elements reached since the last call, each as `prefix/name`, or
    /// as `name` with no prefix.
    pub(crate) fn take(&self, prefix: Option<&str>, out: &mut Vec<String>) {
        for (name, flag) in self.names.iter().zip(&self.flags) {
            if flag.get() == REACHED {
                flag.set(REPORTED);
                out.push(match prefix {
                    Some(prefix) => format!("{prefix}/{name}"),
                    None => (*name).to_string(),
                });
            }
        }
    }
}

thread_local! {
    /// The top-level algorithms' elements, by their full IDs.
    static ALGORITHMS: RefCell<Option<Reached>> = const { RefCell::new(None) };
}

pub(crate) fn start(algorithms: &'static [&'static str]) {
    struct App;
    ALGORITHMS.set(Some(Reached::new(TypeId::of::<App>(), algorithms)));
}

/// The top-level algorithms' elements reached since the last call.
pub(crate) fn take_algorithms(out: &mut Vec<String>) {
    ALGORITHMS.with_borrow(|reached| {
        if let Some(reached) = reached {
            reached.take(None, out);
        }
    });
}

/// Marks element `index` of `C::BRANCHES` reached in the running component.
pub fn branch<C: Component>(index: usize) {
    if let Some(scope) = actor::try_current()
        && let Some(reached) = scope.reached()
        && reached.owner == TypeId::of::<C>()
    {
        reached.mark(index);
    }
}

/// The value of `expr` in `expr?`, marking element `index` of
/// `C::BRANCHES`, the `?`'s error path, if the `?` will return early.
pub fn exit<C: Component, T: Exits>(value: T, index: usize) -> T {
    if value.exits() {
        branch::<C>(index);
    }
    value
}

/// Marks element `index` of the app's `BRANCHES` reached.
pub fn algorithm(index: usize) {
    ALGORITHMS.with_borrow(|reached| {
        if let Some(reached) = reached {
            reached.mark(index);
        }
    });
}

/// As [`exit`], in a top-level algorithm.
pub fn algorithm_exit<T: Exits>(value: T, index: usize) -> T {
    if value.exits() {
        algorithm(index);
    }
    value
}

/// A value that `?` can return early with.
pub trait Exits {
    /// Whether `?` returns early with this value.
    fn exits(&self) -> bool;
}

impl<T> Exits for Option<T> {
    fn exits(&self) -> bool {
        self.is_none()
    }
}

impl<T, E> Exits for Result<T, E> {
    fn exits(&self) -> bool {
        self.is_err()
    }
}
