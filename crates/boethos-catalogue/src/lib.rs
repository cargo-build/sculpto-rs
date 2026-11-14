//! Asset, preset and face catalogue kinds for [`boethos`].
//!
//! The store owns identity and links; this crate owns the domain vocabulary a
//! Sculpto or game project uses when indexing supplied files. It performs no
//! network access and never copies payloads into version control.

mod archive;
mod body_part;
mod index;
mod model;

pub use archive::{ArchiveEntry, list_archive};
pub use body_part::{BodyPartInput, body_part_asset};
pub use index::{IndexOptions, IndexReport, index_root};
pub use model::{Asset, Assets, Category, Sex, Status};
