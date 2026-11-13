//! Typed records and links over an embedded [redb] store.
//!
//! This crate owns storage mechanics only: identity, schema version, atomic
//! uniqueness and links. It knows nothing about characters, garments, presets
//! or BodySlide; a consumer declares a [`Kind`] and the store enforces the
//! record contract.
//!
//! Storage is a single redb file. Writers are serialized by redb, readers see a
//! consistent snapshot, and `insert`/`link` refuse duplicates inside the write
//! transaction instead of relying on a caller's read-first check.
//!
//! [redb]: https://docs.rs/redb

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod error;
pub mod kind;
pub mod store;

pub use error::{StoreError, StoreResult};
pub use kind::{Kind, RecordEnvelope};
pub use store::{Put, Ref, Store};
