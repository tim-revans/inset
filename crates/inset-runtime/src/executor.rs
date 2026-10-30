//! A single-threaded executor that runs until nothing is left to do.
//!
//! A WebAssembly instance has one thread, and the host calls into it for one
//! request at a time, so the executor only runs while the host is waiting for
//! it. [`run_until_idle`] polls woken tasks in the order they were woken
//! until none is left, so every message a request leads to has been handled
//! before the request returns, and the order never depends on timing.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Wake, Waker};

type Task = Pin<Box<dyn Future<Output = ()>>>;

#[derive(Default)]
struct Executor {
    /// Tasks by ID. A slot is `None` while its task is being polled, and
    /// after it finishes. Tasks are components, which run as long as the
    /// program, so slots are not reused.
    tasks: RefCell<Vec<Option<(Task, Waker)>>>,
    ready: Arc<Ready>,
}

/// Task IDs in the order they were woken. Wakers must be `Send` and `Sync`,
/// so this is behind a `Mutex`, which on one thread is never contended.
#[derive(Default)]
struct Ready(Mutex<VecDeque<usize>>);

impl Ready {
    fn push(&self, id: usize) {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !queue.contains(&id) {
            queue.push_back(id);
        }
    }

    fn pop(&self) -> Option<usize> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
    }
}

struct TaskWaker {
    id: usize,
    ready: Arc<Ready>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.ready.push(self.id);
    }
}

thread_local! {
    static EXECUTOR: Executor = Executor::default();
}

/// Adds a task, to be first polled by the next [`run_until_idle`].
pub(crate) fn spawn(future: impl Future<Output = ()> + 'static) {
    EXECUTOR.with(|executor| {
        let mut tasks = executor.tasks.borrow_mut();
        let id = tasks.len();
        let waker = Waker::from(Arc::new(TaskWaker {
            id,
            ready: executor.ready.clone(),
        }));
        tasks.push(Some((Box::pin(future), waker)));
        executor.ready.push(id);
    });
}

/// Polls woken tasks until none is woken, or until `stop` says to stop.
pub(crate) fn run_until_idle(stop: impl Fn() -> bool) {
    while !stop() {
        let Some(id) = EXECUTOR.with(|executor| executor.ready.pop()) else {
            return;
        };
        // Take the task out while it runs, so it can spawn others.
        let Some((mut task, waker)) =
            EXECUTOR.with(|executor| executor.tasks.borrow_mut()[id].take())
        else {
            continue;
        };
        if task
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
        {
            EXECUTOR.with(|executor| executor.tasks.borrow_mut()[id] = Some((task, waker)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::task::Poll;

    /// Yields once, so other woken tasks run before it carries on.
    async fn yield_now() {
        let mut yielded = false;
        std::future::poll_fn(|cx| {
            if yielded {
                Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }

    #[test]
    fn runs_woken_tasks_in_order_until_idle() {
        let log = Rc::new(RefCell::new(Vec::new()));
        for name in ["a", "b"] {
            let log = log.clone();
            spawn(async move {
                log.borrow_mut().push(format!("{name}1"));
                yield_now().await;
                log.borrow_mut().push(format!("{name}2"));
            });
        }
        run_until_idle(|| false);
        assert_eq!(*log.borrow(), ["a1", "b1", "a2", "b2"]);
    }

    #[test]
    fn stops_when_told_to() {
        let polls = Rc::new(Cell::new(0));
        let counted = polls.clone();
        spawn(async move {
            loop {
                counted.set(counted.get() + 1);
                yield_now().await;
            }
        });
        run_until_idle(|| polls.get() == 3);
        assert_eq!(polls.get(), 3);
    }
}
