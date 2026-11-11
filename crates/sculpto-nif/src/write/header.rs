//! NIF header bytes and the game-version gate.
//!
//! Skyrim SE and LE share the `20.2.0.7` header and both use Bethesda user
//! version 12; they differ in the BS stream version (100 and 83). User version
//! 11 belongs to Fallout 3 / New Vegas, whose `NiSkinPartition` omits the two
//! Skyrim trailing bytes, so writing it for an LE file desynchronises nifly's
//! reader (`Skin.cpp`, `User() >= 12` gates `lodLevel` + `globalVB`). Fallout 4
//! and Starfield are refused with a typed error instead of being written as a
//! Skyrim file.

use std::fmt;

use crate::{NifHeader, NifRigError};
use sculpto_core::mesh::MeshError;

/// Numeric NIF version both Skyrim variants use (`20.2.0.7`).
pub const VERSION: u32 = 0x1402_0007;
/// Little-endian marker.
pub const ENDIAN_LITTLE: u8 = 1;
/// The ASCII line every Gamebryo NIF starts with.
pub const HEADER_LINE: &[u8] = b"Gamebryo File Format, Version 20.2.0.7\n";

/// A supported game target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NifTarget {
    /// Skyrim Special Edition, user version 12, BS stream version 100.
    Se,
    /// Skyrim Legendary Edition, user version 12, BS stream version 83.
    Le,
}

impl NifTarget {
    /// Bethesda user version for the target.
    pub fn user_version(self) -> u32 {
        match self {
            NifTarget::Se => 12,
            NifTarget::Le => 12,
        }
    }
}

/// Why a header cannot be written or read.
#[derive(Debug, Clone, PartialEq)]
pub enum NifWriteError {
    /// The user version belongs to a game the writer does not support.
    UnsupportedGame { user_version: u32 },
    /// The numeric version is not the Skyrim `20.2.0.7` layout.
    UnsupportedVersion { version: u32 },
    /// The input is shorter than a header.
    TooShort,
    /// The ASCII line is not a Gamebryo header.
    NotGamebryo,
    /// The endian marker is not little-endian.
    UnsupportedEndian { endian: u8 },
    /// The model failed validation, so no bytes are written.
    InvalidModel(MeshError),
    /// The source document did not supply a checked rig and skin binding.
    InvalidRig(NifRigError),
    /// The selected shape cannot be represented with the supplied native rig.
    RigLayout { shape: usize, reason: &'static str },
    /// A shape has more vertices than the `u16` count can hold.
    TooManyVertices { vertices: usize },
    /// A shape has more triangles than the `u16` count can hold.
    TooManyTriangles { triangles: usize },
    /// Tangents were supplied without normals, so the TBN frame is undefined.
    TangentsWithoutNormals,
    /// The target writer cannot serialize tangents yet.
    TangentsUnsupported { target: NifTarget },
    /// A normal or tangent is the zero vector and cannot be normalized.
    ZeroVector {
        attribute: &'static str,
        vertex: usize,
    },
    /// A skin palette entry has no name, so no consumer can bind it.
    UnnamedBone { index: usize },
    /// Two palette entries share a name, so a consumer cannot tell them apart.
    DuplicateBone { name: String },
    /// A bone name does not fit the string table's length prefix.
    NameTooLong { name: String },
    /// The palette exceeds the `u8` bone index stored per vertex.
    TooManyBones { bones: usize },
    /// A bind matrix has zero scale and cannot be decomposed.
    SingularBind { bone: usize },
    /// A vertex row carries more influences than the four hardware lanes hold.
    TooManyInfluences {
        /// The refused vertex.
        vertex: usize,
        /// Its influence count.
        influences: usize,
    },
    /// A retained influence would become zero or non-finite in the native lane.
    WeightUnrepresentable {
        /// Vertex in source shape order.
        vertex: usize,
        /// Joint index in the shape palette.
        joint: u16,
        /// Positive authored weight.
        weight: f32,
    },
    /// A texture role has no confirmed slot in the target layout.
    TextureSlotUnsupported { slot: &'static str },
    /// More than one material slot needs per-triangle material authoring.
    MultipleMaterialSlots { count: usize },
    /// An inline physics sidecar has no file the HDT link could name.
    InlinePhysicsUnsupported,
    /// The SSE global vertex buffer does not match the mesh or its descriptor.
    GlobalVertexMismatch {
        /// Mesh vertex count.
        vertices: usize,
        /// Declared packed stride.
        vertex_size: u32,
        /// Actual packed bytes.
        bytes: usize,
    },
}

impl fmt::Display for NifWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedGame { user_version } => {
                write!(formatter, "unsupported game user version {user_version}")
            }
            Self::UnsupportedVersion { version } => {
                write!(formatter, "unsupported NIF version {version:#010x}")
            }
            Self::TooShort => write!(formatter, "file is shorter than a NIF header"),
            Self::NotGamebryo => write!(formatter, "not a Gamebryo NIF header line"),
            Self::UnsupportedEndian { endian } => {
                write!(formatter, "unsupported endian marker {endian}")
            }
            Self::InvalidModel(error) => write!(formatter, "model is invalid: {error:?}"),
            Self::InvalidRig(error) => write!(formatter, "invalid rig: {error}"),
            Self::RigLayout { shape, reason } => write!(formatter, "shape {shape}: {reason}"),
            Self::TooManyVertices { vertices } => {
                write!(
                    formatter,
                    "shape has {vertices} vertices, above the u16 count"
                )
            }
            Self::TooManyTriangles { triangles } => {
                write!(
                    formatter,
                    "shape has {triangles} triangles, above the u16 count"
                )
            }
            Self::TangentsWithoutNormals => {
                write!(formatter, "tangents require normals for a TBN frame")
            }
            Self::TangentsUnsupported { target } => {
                write!(formatter, "tangents are not written for {target:?} yet")
            }
            Self::ZeroVector { attribute, vertex } => {
                write!(formatter, "vertex {vertex} has a zero-length {attribute}")
            }
            Self::UnnamedBone { index } => {
                write!(formatter, "skin palette entry {index} has no name")
            }
            Self::DuplicateBone { name } => {
                write!(formatter, "skin palette repeats the bone name `{name}`")
            }
            Self::NameTooLong { name } => {
                write!(
                    formatter,
                    "bone name `{name}` does not fit the string table"
                )
            }
            Self::TooManyBones { bones } => {
                write!(formatter, "{bones} bones exceed the u8 per-vertex index")
            }
            Self::TooManyInfluences { vertex, influences } => write!(
                formatter,
                "vertex {vertex} has {influences} influences, above the four NIF lanes"
            ),
            Self::WeightUnrepresentable {
                vertex,
                joint,
                weight,
            } => write!(
                formatter,
                "vertex {vertex} joint {joint} weight {weight} cannot be encoded in a native lane"
            ),
            Self::SingularBind { bone } => {
                write!(formatter, "bone {bone} has a singular bind matrix")
            }
            Self::TextureSlotUnsupported { slot } => {
                write!(formatter, "texture role `{slot}` has no confirmed slot")
            }
            Self::MultipleMaterialSlots { count } => {
                write!(
                    formatter,
                    "{count} material slots need per-triangle authoring"
                )
            }
            Self::InlinePhysicsUnsupported => {
                write!(
                    formatter,
                    "inline physics has no file the HDT link can name"
                )
            }
            Self::GlobalVertexMismatch {
                vertices,
                vertex_size,
                bytes,
            } => write!(
                formatter,
                "{bytes} global vertex bytes do not pack {vertices} vertices at {vertex_size} bytes"
            ),
        }
    }
}

impl std::error::Error for NifWriteError {}

/// Write the header for a supported target.
pub fn write_header(target: NifTarget) -> Vec<u8> {
    write_header_for(NifHeader {
        version: VERSION,
        user_version: target.user_version(),
    })
    .expect("supported target writes")
}

/// Write a header, refusing a game or version the writer does not own.
///
/// Only Skyrim's user version 12 is writable: 11 produces a file nifly parses
/// two bytes short at `NiSkinPartition` (see the module note).
pub fn write_header_for(header: NifHeader) -> Result<Vec<u8>, NifWriteError> {
    match header.user_version {
        12 => {}
        user_version => return Err(NifWriteError::UnsupportedGame { user_version }),
    }
    if header.version != VERSION {
        return Err(NifWriteError::UnsupportedVersion {
            version: header.version,
        });
    }
    let mut bytes = HEADER_LINE.to_vec();
    bytes.extend_from_slice(&header.version.to_le_bytes());
    bytes.push(ENDIAN_LITTLE);
    bytes.extend_from_slice(&header.user_version.to_le_bytes());
    Ok(bytes)
}

/// Read the header and gate the user versions this crate can parse.
///
/// The writer only emits 12; 11 is still accepted here because files written
/// before the LE fix exist on disk and this crate's parser keys every
/// `NiSkinPartition` conditional on the BS stream version, not on the user
/// version, so those bytes read exactly as they did. They stay readable; they
/// are just not a file native nifly can parse.
pub fn read_header(bytes: &[u8]) -> Result<NifHeader, NifWriteError> {
    let newline = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or(NifWriteError::TooShort)?;
    if !bytes.starts_with(b"Gamebryo File Format") {
        return Err(NifWriteError::NotGamebryo);
    }
    let rest = &bytes[newline + 1..];
    if rest.len() < 9 {
        return Err(NifWriteError::TooShort);
    }
    let version = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
    let endian = rest[4];
    if endian != ENDIAN_LITTLE {
        return Err(NifWriteError::UnsupportedEndian { endian });
    }
    let user_version = u32::from_le_bytes([rest[5], rest[6], rest[7], rest[8]]);
    match user_version {
        12 | 11 => {}
        user_version => return Err(NifWriteError::UnsupportedGame { user_version }),
    }
    Ok(NifHeader {
        version,
        user_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn se_and_le_headers_roundtrip() {
        for target in [NifTarget::Se, NifTarget::Le] {
            let bytes = write_header(target);
            let header = read_header(&bytes).expect("our header reads");
            assert_eq!(header.version, VERSION);
            assert_eq!(header.user_version, target.user_version());
            assert_eq!(&bytes[..HEADER_LINE.len()], HEADER_LINE);
            let suffix = &bytes[HEADER_LINE.len()..];
            assert_eq!(suffix[..4], VERSION.to_le_bytes());
            assert_eq!(suffix[4], ENDIAN_LITTLE);
            assert_eq!(suffix[5..], target.user_version().to_le_bytes());
        }
    }

    #[test]
    fn golden_header_bytes_match_the_skyrim_layout() {
        let se = write_header(NifTarget::Se);
        assert_eq!(
            se,
            [
                b"Gamebryo File Format, Version 20.2.0.7\n".as_slice(),
                &[0x07, 0x00, 0x02, 0x14, 0x01, 0x0c, 0x00, 0x00, 0x00],
            ]
            .concat()
        );
        let le = write_header(NifTarget::Le);
        assert_eq!(
            le,
            [
                b"Gamebryo File Format, Version 20.2.0.7\n".as_slice(),
                &[0x07, 0x00, 0x02, 0x14, 0x01, 0x0c, 0x00, 0x00, 0x00],
            ]
            .concat()
        );
    }

    #[test]
    fn the_writer_refuses_the_fallout_user_version() {
        assert_eq!(
            write_header_for(NifHeader {
                version: VERSION,
                user_version: 11
            }),
            Err(NifWriteError::UnsupportedGame { user_version: 11 })
        );
    }

    #[test]
    fn a_legacy_user_version_11_header_still_reads() {
        let mut bytes = write_header(NifTarget::Le);
        let len = HEADER_LINE.len();
        bytes[len + 5] = 11;
        assert_eq!(
            read_header(&bytes),
            Ok(NifHeader {
                version: VERSION,
                user_version: 11
            })
        );
    }

    #[test]
    fn unsupported_games_are_refused() {
        for user_version in [1, 2, 0] {
            assert_eq!(
                write_header_for(NifHeader {
                    version: VERSION,
                    user_version
                }),
                Err(NifWriteError::UnsupportedGame { user_version })
            );
        }
        assert_eq!(
            write_header_for(NifHeader {
                version: 0x1402_0008,
                user_version: 12
            }),
            Err(NifWriteError::UnsupportedVersion {
                version: 0x1402_0008
            })
        );
    }

    #[test]
    fn malformed_headers_are_named() {
        assert_eq!(read_header(b""), Err(NifWriteError::TooShort));
        assert_eq!(
            read_header(b"Not a NIF\n\0\0\0\0\0\0\0\0\0"),
            Err(NifWriteError::NotGamebryo)
        );
        let mut bytes = write_header(NifTarget::Se);
        let len = HEADER_LINE.len();
        bytes[len + 4] = 0;
        assert_eq!(
            read_header(&bytes),
            Err(NifWriteError::UnsupportedEndian { endian: 0 })
        );
    }
}
