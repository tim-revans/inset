//! Failures, and how a component responds to them.

use std::fmt;

/// A failure condition, as a `failure` block names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A handler panicked.
    Panic,
    /// Integer arithmetic overflowed.
    Overflow,
    /// A call to the capability in this field failed.
    Capability(&'static str),
    /// An awaited message to the child or lent sibling in this field failed.
    Message(&'static str),
}

impl Failure {
    /// The condition a panic message describes. Rust's overflow checks panic
    /// with "attempt to add with overflow" and the like.
    pub(crate) fn of_panic(message: &str) -> Failure {
        if message.starts_with("attempt to") && message.contains("overflow") {
            Failure::Overflow
        } else {
            Failure::Panic
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Panic => f.write_str("Panic"),
            Failure::Overflow => f.write_str("Overflow"),
            Failure::Capability(field) => write!(f, "Capability({field})"),
            Failure::Message(field) => write!(f, "Message({field})"),
        }
    }
}

/// A component's response to a failure, from its `failure` block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// Undo the handler: restore the fields, roll back capability writes and
    /// drop the messages it sent.
    Rollback,
}

/// A failure that no component responded to. It stops the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stopped {
    /// The diagram ID of the component the failure happened in.
    pub component: String,
    pub failure: String,
    pub message: String,
}

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} failed with {}: {}",
            self.component, self.failure, self.message
        )
    }
}
