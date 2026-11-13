//! NIF writing. The reader and the writer live in one crate so the header and
//! block gates cannot drift apart.

use sculpto_core::mesh::MeshAsset;
use sculpto_core::rig::Rig;

use crate::NifDocument;

use self::header::{NifTarget, NifWriteError};
use self::scene::CheckedScene;

pub mod extra;
pub mod header;
pub mod le_shape;
mod scene;
pub mod se_bundle;
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
    encode(asset, target, policy, None)
}

/// Write one imported skinned shape through a checked source or family rig.
///
/// Geometry may carry same-topology slider edits. The source shape's ordered
/// triangles and skin still identify its binding. @req DECL0008
///
/// # Errors
/// Refuses missing or inconsistent source binding, changes to its topology or
/// skin, and transforms or weights the selected native target cannot encode.
pub fn write_rigged_shape(
    asset: &MeshAsset,
    document: &NifDocument,
    rig: &Rig,
    shape: usize,
    target: NifTarget,
    policy: InfluencePolicy,
) -> Result<WrittenNif, NifWriteError> {
    let source = document.shapes.get(shape).ok_or(NifWriteError::RigLayout {
        shape,
        reason: "shape is outside the source document",
    })?;
    if source.skin != asset.skin
        || source.triangles != asset.triangles
        || source.positions.len() != asset.positions.len()
    {
        return Err(NifWriteError::RigLayout {
            shape,
            reason: "the export topology or skin differs from its source binding",
        });
    }
    let binding = document
        .bind_rest_on(rig)
        .map_err(NifWriteError::InvalidRig)?;
    let scene = CheckedScene::new(&binding, &document.sources[shape], shape)?;
    encode(asset, target, policy, Some(&scene))
}

fn encode(
    asset: &MeshAsset,
    target: NifTarget,
    policy: InfluencePolicy,
    scene: Option<&CheckedScene<'_>>,
) -> Result<WrittenNif, NifWriteError> {
    match target {
        NifTarget::Se => se_shape::write(asset, policy, scene),
        NifTarget::Le => le_shape::write(asset, policy, scene),
    }
}
