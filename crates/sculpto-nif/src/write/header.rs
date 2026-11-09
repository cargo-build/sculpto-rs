//! NIF header bytes and the game-version gate.
//!
//! Skyrim SE and LE share the `20.2.0.7` header and differ only in the Bethesda
//! user version (12 and 11). Fallout 4 and Starfield are refused with a typed
//! error instead of being written as a Skyrim file.

use std::fmt;

use crate::NifHeader;
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
    /// Skyrim Special Edition, user version 12.
    Se,
    /// Skyrim Legendary Edition, user version 11.
    Le,
}

impl NifTarget {
    /// Bethesda user version for the target.
    pub fn user_version(self) -> u32 {
        match self {
            NifTarget::Se => 12,
            NifTarget::Le => 11,
        }
    }
}

/// Why a header cannot be written or read.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// A shape has more vertices than the `u16` count can hold.
    TooManyVertices { vertices: usize },
    /// A shape has more triangles than the `u16` count can hold.
    TooManyTriangles { triangles: usize },
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
pub fn write_header_for(header: NifHeader) -> Result<Vec<u8>, NifWriteError> {
    match header.user_version {
        12 | 11 => {}
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

/// Read the header and apply the same game gate as the writer.
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
                &[0x07, 0x00, 0x02, 0x14, 0x01, 0x0b, 0x00, 0x00, 0x00],
            ]
            .concat()
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
