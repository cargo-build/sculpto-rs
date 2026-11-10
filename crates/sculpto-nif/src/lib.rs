//! NIF adapter: read Skyrim bodies into Sculpto shapes.
//!
//! The vendored `byroredux-nif` tree owns parsing; this crate is the only place
//! that calls it, and it exposes a narrow, Sculpto-shaped result
//! (DECL0008). Format quirks stay
//! here: the upstream scene is Y-up, meshes carry renderer attributes, and a
//! truncated parse must never look like a complete body.
//!
//! [`read`] also returns the imported node graph and one raw source record per
//! shape (see [`ShapeSource`]). That capture is read-only: nothing is bound,
//! posed or applied to a vertex.

use std::fmt;

use byroredux_core::string::StringPool;
use byroredux_nif::import::{ImportedMesh, import_nif_scene};
use byroredux_nif::parse_nif;
use sculpto_core::mesh::Skin;
use sculpto_core::rig::Rig;
use sculpto_core::skin::SkinWeights;

mod mesh;
mod rig;
mod weights;
pub mod write;

pub use rig::{NifRigError, ShapeSource, yup_matrix_to_zup, yup_transform_to_zup};

pub use write::tri::{
    PositionOffset, TriError, TriFile, TriPositionMorph, TriShape, TriUvMorph, UvOffset,
    compute_position_multiplier, compute_uv_multiplier, read_tri, read_tri_with_limit, write_tri,
};

/// Header facts a caller needs to gate a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NifHeader {
    /// Raw header version, e.g. `0x14020007` for `20.2.0.7`.
    pub version: u32,
    /// Bethesda user version: 12 for Skyrim SE, 11 for LE, 0 for Oblivion.
    pub user_version: u32,
}

/// One body or garment shape.
///
/// Positions are in the source NIF frame (Skyrim Z-up), not the renderer's
/// Y-up frame: morph containers store deltas in source space
/// (DECL0008).
#[derive(Debug, Clone, PartialEq)]
pub struct NifShape {
    /// Node name, when the source names the mesh.
    pub name: String,
    /// Vertex positions in source space.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex UVs; empty when the mesh authors none.
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex normals in source space; empty when the mesh authors none.
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex tangents in source space as `[x, y, z, bitangent sign]`;
    /// empty when the mesh authors none.
    pub tangents: Vec<[f32; 4]>,
    /// Triangles as vertex-index triples.
    pub triangles: Vec<[u32; 3]>,
    /// One skin: palette names, per-bone binds in the source Z-up frame and the
    /// authored influence rows read from the file's own skin channel.
    pub skin: Option<Skin>,
}

impl NifShape {
    /// Highest authored weight sum across vertices.
    ///
    /// Stored sums are data, not a unit-sum gate: SE packs half-float lanes, so
    /// a well-formed body drifts by the format's own rounding.
    pub fn max_weight_sum(&self) -> f32 {
        let Some(skin) = &self.skin else {
            return 0.0;
        };
        (0..skin.influences.rows())
            .filter_map(|row| skin.influences.row_sum(row))
            .fold(0.0, f32::max)
    }
}

/// A decoded NIF.
#[derive(Debug, Clone, PartialEq)]
pub struct NifDocument {
    /// Header version facts.
    pub header: NifHeader,
    /// Bethesda BS stream version (`user_version_2`): 100 for Skyrim SE, 83
    /// for LE. Both games share user version 12, so this is the only header
    /// fact that separates them.
    pub bsver: u32,
    /// Shapes in scene order.
    pub shapes: Vec<NifShape>,
    /// Complete imported node graph, or `None` when the file has no imported
    /// node (for example a bare written shape with no `NiNode`).
    ///
    /// Raw capture: joint ids are the imported node indexes and nothing has
    /// been bound to [`Self::shapes`]. Use it for bind proofs and later rig
    /// work, not as an applied pose.
    pub rig: Option<Rig>,
    /// One raw source record per shape, parallel to [`Self::shapes`].
    pub sources: Vec<ShapeSource>,
    /// The parser dropped blocks; geometry may be incomplete.
    pub truncated: bool,
}

/// Why a NIF could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NifError {
    /// Fewer than eight header bytes.
    TooShort,
    /// The upstream parser rejected the file.
    Malformed(String),
    /// The parser reported dropped blocks.
    Truncated {
        /// Blocks the parser could not decode.
        dropped: usize,
    },
}

impl fmt::Display for NifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "file is shorter than a NIF header"),
            Self::Malformed(detail) => write!(f, "NIF parse failed: {detail}"),
            Self::Truncated { dropped } => {
                write!(
                    f,
                    "NIF parse dropped {dropped} blocks; refusing a partial body"
                )
            }
        }
    }
}

impl std::error::Error for NifError {}

fn parse_header(bytes: &[u8]) -> Result<(NifHeader, u32), NifError> {
    let newline = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or(NifError::TooShort)?;
    if !bytes.starts_with(b"Gamebryo File Format") {
        return Err(NifError::Malformed(
            "not a Gamebryo NIF header line".to_string(),
        ));
    }
    let rest = &bytes[newline + 1..];
    // version (4) + endian (1) + user version (4) + block count (4) + BS
    // stream version (4).
    if rest.len() < 17 {
        return Err(NifError::TooShort);
    }
    let header = NifHeader {
        version: u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]),
        user_version: u32::from_le_bytes([rest[5], rest[6], rest[7], rest[8]]),
    };
    let bsver = u32::from_le_bytes([rest[13], rest[14], rest[15], rest[16]]);
    Ok((header, bsver))
}

/// Read a NIF into Sculpto shapes and its raw source capture.
///
/// A NIF starts with an ASCII line (`Gamebryo File Format, Version 20.2.0.7`),
/// then the numeric header: `u32` version, `u8` endian, `u32` user version.
///
/// Geometry, UV and normal arrays keep their existing projection. Skin weights
/// are read from the file's own channel — LE sparse entries and SE packed lanes —
/// so the stored values are the authored ones, not the importer's renormalized
/// view. The document also carries the imported node graph and one raw source
/// record per shape; an invalid graph, placement or weight payload is a
/// contextual refusal, never an identity substitute.
pub fn read(bytes: &[u8]) -> Result<NifDocument, NifError> {
    let (header, bsver) = parse_header(bytes)?;
    let scene = parse_nif(bytes).map_err(|error| NifError::Malformed(error.to_string()))?;
    if scene.truncated {
        return Err(NifError::Truncated {
            dropped: scene.dropped_block_count,
        });
    }
    let mut pool = StringPool::new();
    let imported = import_nif_scene(&scene, &mut pool);
    let weights = weights::capture(bytes, &scene, &imported.meshes)?;
    let nodes: Vec<rig::NodeView<'_>> = imported.nodes.iter().map(rig::NodeView::from).collect();
    let graph = rig::capture_graph(&nodes)?;
    let mut shapes = Vec::with_capacity(imported.meshes.len());
    let mut sources = Vec::with_capacity(imported.meshes.len());
    for (mesh, influences) in imported.meshes.iter().zip(weights) {
        shapes.push(project_shape(mesh, influences)?);
        sources.push(rig::capture_shape(
            &rig::MeshView::from(mesh),
            imported.nodes.len(),
        )?);
    }
    Ok(NifDocument {
        header,
        bsver,
        shapes,
        rig: graph,
        sources,
        truncated: false,
    })
}

/// Project one imported shape onto the adapter's shape type.
fn project_shape(
    mesh: &ImportedMesh,
    influences: Option<SkinWeights>,
) -> Result<NifShape, NifError> {
    let chunks = mesh.indices.as_chunks::<3>();
    if !chunks.1.is_empty() {
        return Err(NifError::Malformed(format!(
            "shape `{}` has {} indices, which is not a triangle list",
            mesh.name.as_deref().unwrap_or("-"),
            mesh.indices.len()
        )));
    }
    let name = mesh.name.as_deref().unwrap_or_default();
    let skin = match (&mesh.skin, influences) {
        (Some(skin), Some(influences)) => {
            if influences.rows() != mesh.positions.len() {
                return Err(NifError::Malformed(format!(
                    "shape `{name}` has {} vertices but {} weight rows",
                    mesh.positions.len(),
                    influences.rows()
                )));
            }
            Some(Skin {
                bones: skin
                    .bones
                    .iter()
                    .map(|bone| bone.name.to_string())
                    .collect(),
                bind_inverse: skin
                    .bones
                    .iter()
                    .map(|bone| yup_matrix_to_zup(&bone.bind_inverse))
                    .collect(),
                influences,
            })
        }
        (None, None) => None,
        (Some(_), None) => {
            return Err(NifError::Malformed(format!(
                "shape `{name}` is skinned but carries no readable weight payload"
            )));
        }
        (None, Some(_)) => {
            return Err(NifError::Malformed(format!(
                "shape `{name}` carries weight rows but no skin"
            )));
        }
    };
    Ok(NifShape {
        name: name.to_string(),
        // Upstream converts to a Y-up renderer frame; morph deltas are
        // authored in the source Z-up frame, so undo it here:
        // (x, y, z) -> (x, -z, y). Applying `.osd` offsets to Y-up
        // positions rotates every displacement by 90 degrees.
        positions: mesh
            .positions
            .iter()
            .map(|[x, y, z]| [*x, -*z, *y])
            .collect(),
        uvs: mesh.uvs.clone(),
        // Same Y-up -> Z-up undo as positions; the sign lane is
        // rotation-invariant and passes through unchanged.
        normals: mesh.normals.iter().map(|[x, y, z]| [*x, -*z, *y]).collect(),
        tangents: mesh
            .tangents
            .iter()
            .map(|[x, y, z, sign]| [*x, -*z, *y, *sign])
            .collect(),
        triangles: chunks
            .0
            .iter()
            .map(|tri| [tri[0], tri[1], tri[2]])
            .collect(),
        skin,
    })
}
