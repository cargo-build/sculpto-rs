//! Transactional record and link storage.

use std::fs;
use std::path::Path;

use redb::{ConcurrencyMode, Database, ReadableDatabase, ReadableTable, TableDefinition};

use crate::error::{StoreError, StoreResult};
use crate::kind::{Kind, RecordEnvelope};

const RECORDS: TableDefinition<&str, &[u8]> = TableDefinition::new("records");
const LINKS: TableDefinition<&str, ()> = TableDefinition::new("links");
const SEPARATOR: char = '\u{1f}';

/// Outcome of a write that refuses to overwrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Put {
    /// The identity was free and the record was written.
    Inserted,
    /// The identity existed and the record was replaced by an upsert.
    Replaced,
    /// The identity existed and the insert was refused.
    Exists,
}

/// Borrowed record identity.
#[derive(Debug, Clone, Copy)]
pub struct Ref<'a> {
    /// Family name.
    pub kind: &'a str,
    /// Identity inside the family.
    pub id: &'a str,
}

/// One outgoing link.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Link {
    /// Relation name.
    pub relation: String,
    /// Target family.
    pub to_kind: String,
    /// Target identity.
    pub to_id: String,
}

/// One redb file holding typed records and links.
pub struct Store {
    db: Database,
}

impl Store {
    /// Open or create a store, creating missing parent directories.
    ///
    /// Every handle uses [`ConcurrencyMode::MultiWriter`], so several agent
    /// processes can open the same file; redb still allows one write
    /// transaction at a time and serializes writers.
    pub fn open(path: impl AsRef<Path>) -> StoreResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let db = redb::Builder::new()
            .set_concurrency_mode(ConcurrencyMode::MultiWriter)
            .create(path)?;
        let transaction = db.begin_write()?;
        {
            let _records = transaction.open_table(RECORDS)?;
            let _links = transaction.open_table(LINKS)?;
        }
        transaction.commit()?;
        Ok(Self { db })
    }

    /// Write a record only when its identity is free.
    pub fn insert<K: Kind>(&self, id: &str, record: &K::Record) -> StoreResult<Put> {
        self.write::<K>(id, record, false)
    }

    /// Write a record, replacing an existing identity.
    pub fn upsert<K: Kind>(&self, id: &str, record: &K::Record) -> StoreResult<Put> {
        self.write::<K>(id, record, true)
    }

    /// Read one record.
    pub fn get<K: Kind>(&self, id: &str) -> StoreResult<Option<K::Record>> {
        let key = record_key(K::NAME, id)?;
        let transaction = self.db.begin_read()?;
        let table = transaction.open_table(RECORDS)?;
        let Some(value) = table.get(key.as_str())? else {
            return Ok(None);
        };
        let envelope: RecordEnvelope<K::Record> = serde_json::from_slice(value.value())?;
        check_version::<K>(envelope.version)?;
        Ok(Some(envelope.record))
    }

    /// Remove one record; reports whether it existed.
    pub fn delete<K: Kind>(&self, id: &str) -> StoreResult<bool> {
        let key = record_key(K::NAME, id)?;
        let transaction = self.db.begin_write()?;
        let removed = {
            let mut table = transaction.open_table(RECORDS)?;
            table.remove(key.as_str())?.is_some()
        };
        transaction.commit()?;
        Ok(removed)
    }

    /// Add a directed link; reports whether it was new.
    pub fn link(&self, from: Ref<'_>, relation: &str, to: Ref<'_>) -> StoreResult<bool> {
        let key = link_key(from, relation, to)?;
        let transaction = self.db.begin_write()?;
        let added = {
            let mut table = transaction.open_table(LINKS)?;
            if table.get(key.as_str())?.is_some() {
                false
            } else {
                table.insert(key.as_str(), ())?;
                true
            }
        };
        transaction.commit()?;
        Ok(added)
    }

    /// Remove a directed link; reports whether it existed.
    pub fn unlink(&self, from: Ref<'_>, relation: &str, to: Ref<'_>) -> StoreResult<bool> {
        let key = link_key(from, relation, to)?;
        let transaction = self.db.begin_write()?;
        let removed = {
            let mut table = transaction.open_table(LINKS)?;
            table.remove(key.as_str())?.is_some()
        };
        transaction.commit()?;
        Ok(removed)
    }

    /// Outgoing links of one record, in key order.
    pub fn links_from(&self, from: Ref<'_>) -> StoreResult<Vec<Link>> {
        let prefix = format!(
            "{}{SEPARATOR}{}{SEPARATOR}",
            checked("kind", from.kind)?,
            checked("id", from.id)?
        );
        let transaction = self.db.begin_read()?;
        let table = transaction.open_table(LINKS)?;
        let mut links = Vec::new();
        for row in table.range(prefix.as_str()..)? {
            let (key, _) = row?;
            let key = key.value();
            let Some(rest) = key.strip_prefix(&prefix) else {
                break;
            };
            let mut parts = rest.splitn(3, SEPARATOR);
            let (Some(relation), Some(to_kind), Some(to_id)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(StoreError::InvalidKey {
                    field: "link",
                    value: key.to_string(),
                });
            };
            links.push(Link {
                relation: relation.to_string(),
                to_kind: to_kind.to_string(),
                to_id: to_id.to_string(),
            });
        }
        Ok(links)
    }

    /// Every record of one kind, in identity order.
    pub fn iter_kind<K: Kind>(&self) -> StoreResult<Vec<(String, K::Record)>> {
        let prefix = format!("{}{SEPARATOR}", checked("kind", K::NAME)?);
        let transaction = self.db.begin_read()?;
        let table = transaction.open_table(RECORDS)?;
        let mut records = Vec::new();
        for row in table.range(prefix.as_str()..)? {
            let (key, value) = row?;
            let key = key.value();
            let Some(id) = key.strip_prefix(&prefix) else {
                break;
            };
            let envelope: RecordEnvelope<K::Record> = serde_json::from_slice(value.value())?;
            check_version::<K>(envelope.version)?;
            records.push((id.to_string(), envelope.record));
        }
        Ok(records)
    }

    /// Write every record as one JSON object per line, in key order.
    pub fn export_jsonl(&self, path: impl AsRef<Path>) -> StoreResult<usize> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let transaction = self.db.begin_read()?;
        let table = transaction.open_table(RECORDS)?;
        let mut output = String::new();
        let mut count = 0usize;
        for row in table.iter()? {
            let (_, value) = row?;
            output.push_str(std::str::from_utf8(value.value())?);
            output.push('\n');
            count += 1;
        }
        fs::write(path, output)?;
        Ok(count)
    }

    fn write<K: Kind>(&self, id: &str, record: &K::Record, replace: bool) -> StoreResult<Put> {
        K::validate(record).map_err(|detail| StoreError::Validation {
            kind: K::NAME,
            detail,
        })?;
        let key = record_key(K::NAME, id)?;
        let bytes = encode::<K>(id, record)?;
        let transaction = self.db.begin_write()?;
        let outcome = {
            let mut table = transaction.open_table(RECORDS)?;
            if table.get(key.as_str())?.is_some() {
                if replace {
                    table.insert(key.as_str(), bytes.as_slice())?;
                    Put::Replaced
                } else {
                    Put::Exists
                }
            } else {
                table.insert(key.as_str(), bytes.as_slice())?;
                Put::Inserted
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }
}

fn encode<K: Kind>(id: &str, record: &K::Record) -> StoreResult<Vec<u8>> {
    let envelope = RecordEnvelope {
        kind: K::NAME.to_string(),
        id: id.to_string(),
        version: K::VERSION,
        record,
    };
    Ok(serde_json::to_vec(&envelope)?)
}

fn check_version<K: Kind>(found: u32) -> StoreResult<()> {
    if found == K::VERSION {
        Ok(())
    } else {
        Err(StoreError::VersionMismatch {
            kind: K::NAME,
            found,
            expected: K::VERSION,
        })
    }
}

fn record_key(kind: &str, id: &str) -> StoreResult<String> {
    Ok(format!(
        "{}{SEPARATOR}{}",
        checked("kind", kind)?,
        checked("id", id)?
    ))
}

fn link_key(from: Ref<'_>, relation: &str, to: Ref<'_>) -> StoreResult<String> {
    Ok(format!(
        "{}{SEPARATOR}{}{SEPARATOR}{}{SEPARATOR}{}{SEPARATOR}{}",
        checked("kind", from.kind)?,
        checked("id", from.id)?,
        checked("relation", relation)?,
        checked("kind", to.kind)?,
        checked("id", to.id)?
    ))
}

fn checked<'a>(field: &'static str, value: &'a str) -> StoreResult<&'a str> {
    if value.is_empty() || value.contains(SEPARATOR) {
        return Err(StoreError::InvalidKey {
            field,
            value: value.to_string(),
        });
    }
    Ok(value)
}
