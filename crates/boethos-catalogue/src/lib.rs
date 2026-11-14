//! Asset, preset and face catalogue kinds for [`boethos`].
//!
//! The store owns identity and links; this crate owns the domain vocabulary a
//! Sculpto or game project uses when indexing supplied files. It performs no
//! network access and never copies payloads into version control.

mod archive;
mod body_part;
mod index;
mod model;
mod raw_part;

pub use archive::{ArchiveEntry, list_archive};
pub use body_part::{BodyPartInput, body_part_asset};
pub use index::{IndexOptions, IndexReport, index_root};
pub use model::{Asset, Assets, Category, Sex, Status};
pub use raw_part::{RawPartInput, raw_part_asset};

/// Origin label shared by directory indexing and manifest imports.
///
/// `text` is the relative path/entry, when the caller has one; a manifest row
/// passes an empty string. The rules stay one owner so a new token cannot land
/// on only one ingestion route.
pub(crate) fn origin_tag(source: &str, text: &str) -> Option<&'static str> {
    let source = source.to_lowercase();
    let text = text.to_lowercase();
    if source.contains("from-skyrim") || source.contains("skyrimmods") {
        Some("origin:skyrimmods")
    } else if source.contains("tripo") || text.contains("tripo") {
        Some("origin:tripo3d")
    } else if source == "underwear-extract" || source.contains("sculpto") {
        Some("origin:sculpto")
    } else {
        None
    }
}

/// Stable record identity from ordered fields.
pub(crate) fn identity(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(parts.join("\u{1f}").as_bytes());
    format!("{:x}", hasher.finalize())
}
