//! External components, as the program sees them: a component whose
//! messages the host passes to a WebAssembly component of its own.
//!
//! The compiler emits each `extern component` as an ordinary component
//! holding an [`External`], with one handler per message that hands the
//! message's arguments to the host and waits for the reply. So an external
//! component has a mailbox, takes one message at a time, and fails as any
//! component does, which keeps messages, failures and the diagram the same
//! whatever language a component is written in.

use serde_json::Value as Json;

use crate::actor::{self, Scope};
use crate::bindings::inset::host::externals;
use crate::failure::Failure;
use crate::stored::Stored;

/// The running external component a component's messages go to.
pub struct External {
    /// The component's diagram ID, which names its instance to the host.
    id: String,
    /// The external component's name, such as `CsvReader`.
    name: &'static str,
}

impl External {
    /// The external component `name`, started in `scope`.
    pub fn new(scope: &Scope, name: &'static str) -> External {
        External {
            id: scope.path().to_string(),
            name,
        }
    }

    /// Sends `message` with its arguments, each in its stored form, and
    /// waits for the reply. A failure in the external component fails the
    /// handler, as a panic in it would.
    pub async fn call<R: Stored>(&self, message: &str, args: Vec<Json>) -> R {
        let args = serde_json::to_vec(&Json::Array(args)).expect("JSON always encodes");
        let failed = match externals::call(&self.id, self.name, message, &args) {
            Ok(reply) => match serde_json::from_slice::<Json>(&reply)
                .map_err(|e| e.to_string())
                .and_then(|json| R::from_json(&json))
            {
                Ok(reply) => return reply,
                Err(e) => format!("the reply to `{message}` does not fit: {e}"),
            },
            Err(failed) => failed,
        };
        actor::fail(Failure::of_panic(&failed), failed).await
    }
}
