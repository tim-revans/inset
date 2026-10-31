//! The tables behind `Table` grants: an SQLite table for a real run, or a
//! map in memory for a requirement test.
//!
//! Either way the host holds keys and values in the stored form the program
//! gives them, without looking inside: an order-preserving key and a JSON
//! value (see the `inset-value` crate). An SQLite table has two columns,
//! `key BLOB PRIMARY KEY` and `value TEXT`, and SQLite compares `BLOB`s byte
//! by byte, so its rows sort in key order as the program's keys do.
//!
//! Writes wait in memory until the handler that made them completes and the
//! program commits them, so a rolled-back handler leaves nothing behind.
//! A commit writes them in one SQLite transaction.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

/// A table's entries in their stored form: each key and its value, in key order.
pub type Entries = Vec<(Vec<u8>, Vec<u8>)>;

/// A table the host holds for a program.
pub struct Table {
    backing: Backing,
    /// Writes not yet committed: a value, or `None` for a removal.
    pending: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    /// Every call fails, as in the requirement fact `name fails`.
    pub fails: bool,
    /// Why the last commit failed, which stops the program.
    pub(crate) commit_error: Option<String>,
}

enum Backing {
    Memory(BTreeMap<Vec<u8>, Vec<u8>>),
    Sqlite {
        connection: Connection,
        /// The table's name, quoted for SQL.
        quoted: String,
        file: PathBuf,
        name: String,
    },
}

impl Table {
    /// An empty table in memory, for a test.
    pub(crate) fn fake() -> Table {
        Table::new(Backing::Memory(BTreeMap::new()))
    }

    /// The table `name` in the SQLite database `file`, created if the file
    /// or the table does not exist yet.
    pub(crate) fn open(file: &Path, name: &str) -> Result<Table, String> {
        let connection = Connection::open(file).map_err(|e| e.to_string())?;
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        connection
            .execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {quoted} \
                     (key BLOB PRIMARY KEY NOT NULL, value TEXT NOT NULL) WITHOUT ROWID"
                ),
                [],
            )
            .map_err(|e| e.to_string())?;
        // A table some other tool made may lack the columns.
        connection
            .prepare(&format!("SELECT key, value FROM {quoted} LIMIT 0"))
            .map_err(|e| format!("it is not a table Inset stores: {e}"))?;
        Ok(Table::new(Backing::Sqlite {
            connection,
            quoted,
            file: file.to_path_buf(),
            name: name.to_string(),
        }))
    }

    fn new(backing: Backing) -> Table {
        Table {
            backing,
            pending: BTreeMap::new(),
            fails: false,
            commit_error: None,
        }
    }

    /// Sets a committed entry, as `given name[key] is value` does.
    pub fn set(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.write(&BTreeMap::from([(key.to_vec(), Some(value.to_vec()))]))
    }

    /// Removes every committed entry, as `given name is empty` does.
    pub fn clear(&mut self) -> Result<(), String> {
        let keys = self
            .entries()?
            .into_iter()
            .map(|(k, _)| (k, None))
            .collect();
        self.write(&keys)
    }

    /// The committed value under `key`, as a `then` fact reads it.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        match &self.backing {
            Backing::Memory(map) => Ok(map.get(key).cloned()),
            Backing::Sqlite {
                connection, quoted, ..
            } => connection
                .query_row(
                    &format!("SELECT value FROM {quoted} WHERE key = ?1"),
                    [key],
                    |row| bytes(row.get_ref(0)?),
                )
                .optional()
                .map_err(|e| e.to_string()),
        }
    }

    /// Every committed entry, in key order.
    pub fn entries(&self) -> Result<Entries, String> {
        match &self.backing {
            Backing::Memory(map) => Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            Backing::Sqlite {
                connection, quoted, ..
            } => {
                let mut statement = connection
                    .prepare(&format!("SELECT key, value FROM {quoted} ORDER BY key"))
                    .map_err(|e| e.to_string())?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((bytes(row.get_ref(0)?)?, bytes(row.get_ref(1)?)?))
                    })
                    .map_err(|e| e.to_string())?;
                rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
            }
        }
    }

    /// Where a stored table is: its file and its name.
    pub fn location(&self) -> Option<(&Path, &str)> {
        match &self.backing {
            Backing::Memory(_) => None,
            Backing::Sqlite { file, name, .. } => Some((file, name)),
        }
    }

    fn check(&self) -> Result<(), String> {
        match self.fails {
            true => Err("the table failed, as the test asked".into()),
            false => Ok(()),
        }
    }

    // ---- What the program calls ------------------------------------------

    pub(crate) fn read(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.check()?;
        match self.pending.get(key) {
            Some(pending) => Ok(pending.clone()),
            None => self.get(key),
        }
    }

    pub(crate) fn insert(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<(), String> {
        self.check()?;
        self.pending.insert(key, Some(value));
        Ok(())
    }

    pub(crate) fn remove(&mut self, key: Vec<u8>) -> Result<Option<Vec<u8>>, String> {
        let old = self.read(&key)?;
        self.pending.insert(key, None);
        Ok(old)
    }

    /// Every entry, with the writes not yet committed, in key order.
    pub(crate) fn all(&self) -> Result<Entries, String> {
        self.check()?;
        let mut entries: BTreeMap<Vec<u8>, Vec<u8>> = self.entries()?.into_iter().collect();
        for (key, value) in &self.pending {
            match value {
                Some(value) => entries.insert(key.clone(), value.clone()),
                None => entries.remove(key),
            };
        }
        Ok(entries.into_iter().collect())
    }

    pub(crate) fn commit(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        if let Err(e) = self.write(&pending) {
            self.commit_error.get_or_insert(e);
        }
    }

    pub(crate) fn roll_back(&mut self) {
        self.pending.clear();
    }

    /// Writes entries, and removes those that are `None`, at once.
    fn write(&mut self, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Result<(), String> {
        match &mut self.backing {
            Backing::Memory(map) => {
                for (key, value) in writes {
                    match value {
                        Some(value) => map.insert(key.clone(), value.clone()),
                        None => map.remove(key),
                    };
                }
                Ok(())
            }
            Backing::Sqlite {
                connection, quoted, ..
            } => {
                let transaction = connection.transaction().map_err(|e| e.to_string())?;
                for (key, value) in writes {
                    match value {
                        Some(value) => transaction.execute(
                            &format!(
                                "INSERT OR REPLACE INTO {quoted} (key, value) VALUES (?1, ?2)"
                            ),
                            params![key, String::from_utf8_lossy(value)],
                        ),
                        None => transaction
                            .execute(&format!("DELETE FROM {quoted} WHERE key = ?1"), [key]),
                    }
                    .map_err(|e| e.to_string())?;
                }
                transaction.commit().map_err(|e| e.to_string())
            }
        }
    }
}

/// A column's bytes, whether SQLite holds them as text or as a blob.
fn bytes(value: rusqlite::types::ValueRef<'_>) -> rusqlite::Result<Vec<u8>> {
    match value {
        rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => Ok(b.to_vec()),
        other => Err(rusqlite::Error::InvalidColumnType(
            0,
            "key or value".into(),
            other.data_type(),
        )),
    }
}
