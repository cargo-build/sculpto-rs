//! NIF adapter: read Skyrim bodies into Sculpto shapes.
//!
//! The vendored `byroredux-nif` tree owns parsing; this crate is the only place
//! that calls it, and it exposes a narrow, Sculpto-shaped result
//! (DECL0008). Format quirks stay
//! here: the upstream scene is Y-up, meshes carry renderer attributes, and a
//! truncated parse must never look like a complete body.

use std::fmt;

use byroredux_core::string::StringPool;
use byroredux_nif::import::import_nif_scene;
use byroredux_nif::parse_nif;

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
    /// Triangles as vertex-index triples.
    pub triangles: Vec<[u32; 3]>,
    /// Bone names, parallel to `bone_indices`.
    pub bones: Vec<String>,
    /// Per-vertex bone indices into `bones`.
    pub bone_indices: Vec<[u16; 4]>,
    /// Per-vertex weights, parallel to `bone_indices`.
    pub weights: Vec<[f32; 4]>,
    /// Base-color texture path from the shader, when authored.
    pub texture_base_color: String,
    /// Normal-map texture path from the shader, when authored.
    pub texture_normal: String,
}

impl NifShape {
    /// Highest weight sum across vertices, useful as a normalization check.
    pub fn max_weight_sum(&self) -> f32 {
        self.weights
            .iter()
            .map(|weights| weights.iter().sum::<f32>())
            .fold(0.0, f32::max)
    }
}

/// A decoded NIF.
#[derive(Debug, Clone, PartialEq)]
pub struct NifDocument {
    /// Header version facts.
    pub header: NifHeader,
    /// Shapes in scene order.
    pub shapes: Vec<NifShape>,
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

fn parse_header(bytes: &[u8]) -> Result<NifHeader, NifError> {
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
    if rest.len() < 9 {
        return Err(NifError::TooShort);
    }
    Ok(NifHeader {
        version: u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]),
        user_version: u32::from_le_bytes([rest[5], rest[6], rest[7], rest[8]]),
    })
}

/// Read a NIF into Sculpto shapes.
///
/// A NIF starts with an ASCII line (`Gamebryo File Format, Version 20.2.0.7`),
/// then the numeric header: `u32` version, `u8` endian, `u32` user version.
pub fn read(bytes: &[u8]) -> Result<NifDocument, NifError> {
    let header = parse_header(bytes)?;
    let scene = parse_nif(bytes).map_err(|error| NifError::Malformed(error.to_string()))?;
    if scene.truncated {
        return Err(NifError::Truncated {
            dropped: scene.dropped_block_count,
        });
    }
    let mut pool = StringPool::new();
    let imported = import_nif_scene(&scene, &mut pool);
    let shapes = imported
        .meshes
        .iter()
        .map(|mesh| -> Result<NifShape, NifError> {
            let chunks = mesh.indices.as_chunks::<3>();
            if !chunks.1.is_empty() {
                return Err(NifError::Malformed(format!(
                    "shape `{}` has {} indices, which is not a triangle list",
                    mesh.name.as_deref().unwrap_or("-"),
                    mesh.indices.len()
                )));
            }
            Ok(NifShape {
                name: mesh.name.as_deref().map(str::to_string).unwrap_or_default(),
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
                triangles: chunks
                    .0
                    .iter()
                    .map(|tri| [tri[0], tri[1], tri[2]])
                    .collect(),
                bones: mesh
                    .skin
                    .as_ref()
                    .map(|skin| {
                        skin.bones
                            .iter()
                            .map(|bone| bone.name.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                texture_base_color: mesh
                    .material
                    .textures
                    .base_color
                    .and_then(|symbol| pool.resolve(symbol))
                    .unwrap_or_default()
                    .to_string(),
                texture_normal: mesh
                    .material
                    .textures
                    .normal
                    .and_then(|symbol| pool.resolve(symbol))
                    .unwrap_or_default()
                    .to_string(),
                bone_indices: mesh
                    .skin
                    .as_ref()
                    .map(|skin| skin.vertex_bone_indices.clone())
                    .unwrap_or_default(),
                weights: mesh
                    .skin
                    .as_ref()
                    .map(|skin| skin.vertex_bone_weights.clone())
                    .unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NifDocument {
        header,
        shapes,
        truncated: false,
    })
}
