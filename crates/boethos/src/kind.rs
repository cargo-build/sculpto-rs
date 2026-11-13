//! Stable record families.

use serde::{Serialize, de::DeserializeOwned};

/// A record family stored under one name and schema version.
///
/// `NAME` and `VERSION` are part of the stored identity: a record written by an
/// incompatible version is refused on read instead of being interpreted as the
/// current shape.
pub trait Kind {
    /// Stable family name, unique inside one store.
    const NAME: &'static str;
    /// Schema version of [`Kind::Record`].
    const VERSION: u32;
    /// The stored record.
    type Record: Serialize + DeserializeOwned;

    /// Reject a record before it reaches storage.
    ///
    /// The store treats this as the consumer's schema check; returning a
    /// non-empty detail fails the write by name.
    fn validate(record: &Self::Record) -> Result<(), String>;
}

/// Self-describing envelope used for every stored record and export line.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RecordEnvelope<T> {
    /// Family name the record was written under.
    pub kind: String,
    /// Caller-provided identity inside the family.
    pub id: String,
    /// Schema version at write time.
    pub version: u32,
    /// The record itself.
    pub record: T,
}
