//! `fork for`: a body run once for each element of a collection, whose
//! values are gathered in the collection's order.
//!
//! The compiler emits a branch that waits for nothing as a plain closure
//! passed to [`each`], and one that awaits a message as an async closure
//! passed to [`each_async`]. Both closures are `Fn`, not `FnMut`, so rustc
//! holds the branches to what the checker already requires: none changes
//! what the others can see.
//!
//! The executor has one thread, so branches that wait for nothing run one
//! after another, in order: a pure algorithm gives the same values either
//! way. Branches that await run concurrently on that thread: [`each_async`]
//! polls every unfinished branch, in order, each time it is woken, so the
//! messages they send are queued in the collection's order and the result
//! never depends on timing.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Runs `branch` for each element of `items`, and returns the values in
/// the order of the elements.
pub fn each<I: IntoIterator, T>(items: I, branch: impl Fn(I::Item) -> T) -> Vec<T> {
    items.into_iter().map(branch).collect()
}

/// Starts `branch` for each element of `items`, waits until every branch
/// has finished, and returns the values in the order of the elements.
pub async fn each_async<I: IntoIterator, T>(
    items: I,
    branch: impl AsyncFn(I::Item) -> T,
) -> Vec<T> {
    let branches = items
        .into_iter()
        .map(|item| (Box::pin(branch(item)), None))
        .collect();
    Join { branches }.await
}

/// A branch, with its value once it has finished.
type Branch<F> = (Pin<Box<F>>, Option<<F as Future>::Output>);

/// Each branch, in the order of the elements.
struct Join<F: Future> {
    branches: Vec<Branch<F>>,
}

// Every branch is boxed and no value is ever pinned, so moving a `Join`
// moves nothing that is pinned.
impl<F: Future> Unpin for Join<F> {}

impl<F: Future> Future for Join<F> {
    type Output = Vec<F::Output>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut pending = false;
        for (branch, value) in &mut this.branches {
            if value.is_none() {
                match branch.as_mut().poll(cx) {
                    Poll::Ready(v) => *value = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            return Poll::Pending;
        }
        Poll::Ready(
            this.branches
                .drain(..)
                .map(|(_, value)| value.expect("every branch has finished"))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::task::Waker;

    /// Yields once, so the other branches run before it carries on.
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

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    #[test]
    fn each_keeps_the_order_of_the_elements() {
        assert_eq!(each([3, 1, 2], |n| n * 10), [30, 10, 20]);
        assert_eq!(each(Vec::<i32>::new(), |n| n), Vec::<i32>::new());
    }

    #[test]
    fn async_branches_interleave_and_keep_the_order_of_the_elements() {
        let log = RefCell::new(Vec::new());
        let values = block_on(each_async(["a", "b"], async |name| {
            log.borrow_mut().push(format!("{name}1"));
            yield_now().await;
            log.borrow_mut().push(format!("{name}2"));
            name.len()
        }));
        assert_eq!(values, [1, 1]);
        assert_eq!(*log.borrow(), ["a1", "b1", "a2", "b2"]);
    }
}
