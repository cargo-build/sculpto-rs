//! NIF writing. The reader and the writer live in one crate so the header and
//! block gates cannot drift apart.

use sculpto_core::mesh::MeshAsset;

use self::header::{NifTarget, NifWriteError};

pub mod extra;
pub mod header;
pub mod le_shape;
pub mod se_shape;
pub mod shader;
pub mod skin;
pub mod strings;
pub mod tri;

pub use tri::{read_tri, read_tri_with_limit, write_tri};

/// Write a complete NIF for one shape on the requested target.
///
/// The SE and LE writers own the block layouts; this is the target seam every
/// caller uses instead of naming one writer, so a target switch cannot pair a
/// header with the wrong body.
pub fn write_shape(asset: &MeshAsset, target: NifTarget) -> Result<Vec<u8>, NifWriteError> {
    match target {
        NifTarget::Se => se_shape::write_se_shape(asset),
        NifTarget::Le => le_shape::write_le_shape(asset),
    }
}
