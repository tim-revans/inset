//! The `Table<K, V>` capability: a stored map, kept by the host.

use std::borrow::Borrow;
use std::marker::PhantomData;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::actor::{self, Transaction};
use crate::bindings::inset::host::capabilities;
use crate::failure::Failure;

/// The `Table<K, V>` capability. Keys and values are encoded as JSON for the
/// host, which never looks inside them.
pub struct Table<K, V> {
    field: &'static str,
    table: Rc<Resource>,
    types: PhantomData<fn() -> (K, V)>,
}

struct Resource(capabilities::Table);

impl Transaction for Resource {
    fn commit(&self) {
        self.0.commit();
    }

    fn roll_back(&self) {
        self.0.rollback();
    }
}

impl<K: Serialize, V: Serialize + DeserializeOwned> Table<K, V> {
    pub(crate) fn open(grant: &str) -> Result<Self, String> {
        Ok(Table {
            field: "",
            table: Rc::new(Resource(capabilities::open_table(grant)?)),
            types: PhantomData,
        })
    }

    /// Names the field that holds the table, which a failure reports.
    pub fn label(&mut self, field: &'static str) {
        self.field = field;
    }

    /// Reads the value stored under `key`.
    pub async fn get<Q: Serialize + ?Sized>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let stored = self.table.0.get(&encode(key));
        match stored.map(|value| value.map(|v| serde_json::from_slice(&v))) {
            Ok(None) => None,
            Ok(Some(Ok(value))) => Some(value),
            Ok(Some(Err(e))) => self.fail(e.to_string()).await,
            Err(e) => self.fail(e).await,
        }
    }

    /// Stores `value` under `key`, replacing any value already there. The
    /// write takes effect when the handler completes.
    pub async fn insert(&mut self, key: K, value: V) {
        actor::current().wrote(self.table.clone());
        if let Err(e) = self.table.0.insert(&encode(&key), &encode(&value)) {
            self.fail(e).await
        }
    }

    async fn fail<T>(&self, message: String) -> T {
        actor::fail(Failure::Capability(self.field), message).await
    }
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("Inset data types always encode")
}
