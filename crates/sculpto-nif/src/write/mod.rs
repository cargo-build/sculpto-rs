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

/// Whether a shape may lose influences in its four-lane native view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfluencePolicy {
    /// Refuse a row with more than four positive influences.
    Preserve,
    /// Keep the four strongest influences, report the rest, and normalize
    /// the retained lanes. LE still writes the original sparse skin data.
    NativeFour,
}

/// One authored influence omitted from the native four-lane view.
#[derive(Debug, Clone, PartialEq)]
pub struct DroppedInfluence {
    /// Vertex in source shape order.
    pub vertex: usize,
    /// Source palette name.
    pub bone: String,
    /// Authored weight before rescaling the retained lanes.
    pub weight: f32,
}

/// A complete shape encoding and its native weight reduction evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenNif {
    /// Complete NIF payload.
    pub bytes: Vec<u8>,
    /// Influences omitted from hardware lanes, in vertex order.
    pub dropped_influences: Vec<DroppedInfluence>,
}

/// Write a complete NIF for one shape on the requested target.
///
/// The SE and LE writers own the block layouts; this is the target seam every
/// caller uses instead of naming one writer, so a target switch cannot pair a
/// header with the wrong body.
pub fn write_shape(asset: &MeshAsset, target: NifTarget) -> Result<Vec<u8>, NifWriteError> {
    write_shape_with_policy(asset, target, InfluencePolicy::Preserve).map(|output| output.bytes)
}

/// Encode a shape with a caller-selected influence policy. @req DECL0008
pub fn write_shape_with_policy(
    asset: &MeshAsset,
    target: NifTarget,
    policy: InfluencePolicy,
) -> Result<WrittenNif, NifWriteError> {
    match target {
        NifTarget::Se => se_shape::write(asset, policy),
        NifTarget::Le => le_shape::write(asset, policy),
    }
}
