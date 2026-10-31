//! The `Table<K, V>` capability: a stored map, kept by the host.

use std::borrow::Borrow;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::actor::{self, Transaction};
use crate::bindings::inset::host::capabilities;
use crate::failure::Failure;
use crate::stored::{Key, KeyType, Stored, key_bytes, read_key};

/// The `Table<K, V>` capability. Keys and values are given to the host in
/// their stored form (see `inset-value`), which the host never looks inside.
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

impl<K: KeyType, V: Stored> Table<K, V> {
    /// Opens the table granted as `grant`, and refuses it if anything it
    /// already holds is not a `K` and a `V`.
    pub(crate) fn open(grant: &str) -> Result<Self, String> {
        let table = capabilities::open_table(grant)?;
        for (key, value) in table.entries()? {
            let key: K = read_key(&key).map_err(|e| format!("a stored key does not fit: {e}"))?;
            decode::<V>(&value).map_err(|e| {
                format!("the value stored under {} does not fit: {e}", key.to_json())
            })?;
        }
        Ok(Table {
            field: "",
            table: Rc::new(Resource(table)),
            types: PhantomData,
        })
    }

    /// Names the field that holds the table, which a failure reports.
    pub fn label(&mut self, field: &'static str) {
        self.field = field;
    }

    /// Reads the value stored under `key`.
    pub async fn get<Q: Key + ?Sized>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        match self.table.0.get(&key_bytes(key)) {
            Ok(None) => None,
            Ok(Some(value)) => match decode(&value) {
                Ok(value) => Some(value),
                Err(e) => self.fail(e).await,
            },
            Err(e) => self.fail(e).await,
        }
    }

    /// Stores `value` under `key`, replacing any value already there. The
    /// write takes effect when the handler completes.
    pub async fn insert(&mut self, key: K, value: V) {
        actor::current().wrote(self.table.clone());
        let value = serde_json::to_vec(&value.to_json()).expect("JSON always encodes");
        if let Err(e) = self.table.0.insert(&key_bytes(&key), &value) {
            self.fail(e).await
        }
    }

    /// Removes the value under `key`, and returns it. The removal takes
    /// effect when the handler completes.
    pub async fn remove<Q: Key + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        actor::current().wrote(self.table.clone());
        match self.table.0.remove(&key_bytes(key)) {
            Ok(None) => None,
            Ok(Some(value)) => match decode(&value) {
                Ok(value) => Some(value),
                Err(e) => self.fail(e).await,
            },
            Err(e) => self.fail(e).await,
        }
    }

    /// Every key and value, in key order.
    pub async fn entries(&self) -> Vec<(K, V)> {
        let entries = match self.table.0.entries() {
            Ok(entries) => entries,
            Err(e) => return self.fail(e).await,
        };
        let mut out = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            match read_key(&key).and_then(|k| Ok((k, decode(&value)?))) {
                Ok(entry) => out.push(entry),
                Err(e) => return self.fail(e).await,
            }
        }
        out
    }

    async fn fail<T>(&self, message: String) -> T {
        actor::fail(Failure::Capability(self.field), message).await
    }
}

fn decode<V: Stored>(value: &[u8]) -> Result<V, String> {
    let json: serde_json::Value =
        serde_json::from_slice(value).map_err(|e| format!("not stored as JSON: {e}"))?;
    V::from_json(&json)
}
