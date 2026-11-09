//! Repairs on a GLB written by an external exporter — JSON chunk edits only.
//!
//! Blender 5.2 leaves two defects in the container, and neither is expressible
//! through the writer's model: `"texCoord": -1` on a material texture whose UV
//! set does not resolve (the spec wants an unsigned set index), and a non-zero
//! default `mesh.weights` sampled from the current frame, which applies every
//! morph target at load time. The binary chunk is copied byte for byte, and a
//! document that already satisfies a rule is returned unchanged, so a re-run
//! cannot perturb the bytes.

use std::fmt;

use serde_json::{Value, json};

/// Chunk type of the GLB JSON chunk (`JSON` little-endian).
const JSON_CHUNK: u32 = 0x4E4F_534A;

/// Why a GLB cannot be repaired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanitizeError {
    /// Shorter than the 12-byte header plus one chunk header.
    TooShort {
        /// Bytes in the input.
        len: usize,
    },
    /// The first four bytes are not the GLB magic.
    NotGlb {
        /// The bytes found instead.
        magic: [u8; 4],
    },
    /// The first chunk is not the JSON chunk, so reframing would corrupt it.
    FirstChunkNotJson {
        /// The chunk type found instead.
        kind: u32,
    },
    /// The JSON chunk declares more bytes than the file holds.
    JsonTruncated {
        /// Bytes the chunk header declares.
        declared: usize,
        /// Bytes available after the chunk header.
        available: usize,
    },
    /// The JSON chunk is not a JSON document.
    JsonInvalid(String),
    /// The JSON chunk holds JSON that is not an object.
    JsonNotObject,
    /// The rewritten container does not fit the `u32` GLB length field.
    TooLarge {
        /// Bytes the repaired container would occupy.
        bytes: usize,
    },
}

impl fmt::Display for SanitizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { len } => {
                write!(formatter, "GLB is {len} bytes, shorter than its header")
            }
            Self::NotGlb { magic } => write!(
                formatter,
                "not a GLB: magic is {:?}",
                String::from_utf8_lossy(magic)
            ),
            Self::FirstChunkNotJson { kind } => write!(
                formatter,
                "first GLB chunk is {kind:#010x}, not the JSON chunk"
            ),
            Self::JsonTruncated {
                declared,
                available,
            } => write!(
                formatter,
                "JSON chunk declares {declared} bytes, {available} available"
            ),
            Self::JsonInvalid(message) => {
                write!(formatter, "JSON chunk is not JSON: {message}")
            }
            Self::JsonNotObject => write!(formatter, "JSON chunk is not a JSON object"),
            Self::TooLarge { bytes } => write!(
                formatter,
                "repaired GLB of {bytes} bytes exceeds the u32 length field"
            ),
        }
    }
}

impl std::error::Error for SanitizeError {}

/// Outcome of one repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repair {
    /// The repaired container, or `None` when the input already satisfied the
    /// rule and must stay byte-identical.
    pub bytes: Option<Vec<u8>>,
    /// JSON sites the rule changed.
    pub changed: usize,
}

/// A GLB split into its JSON chunk and every byte after it.
struct Container {
    magic: [u8; 4],
    version: u32,
    chunk_type: u32,
    document: Value,
    /// The binary chunk and any chunk after it, copied verbatim.
    tail: Vec<u8>,
}

impl Container {
    fn parse(bytes: &[u8]) -> Result<Self, SanitizeError> {
        let Some(header) = bytes.get(..20) else {
            return Err(SanitizeError::TooShort { len: bytes.len() });
        };
        let magic = [header[0], header[1], header[2], header[3]];
        if magic != *b"glTF" {
            return Err(SanitizeError::NotGlb { magic });
        }
        let chunk_type = u32_at(header, 16);
        if chunk_type != JSON_CHUNK {
            return Err(SanitizeError::FirstChunkNotJson { kind: chunk_type });
        }
        let declared = u32_at(header, 12) as usize;
        let available = bytes.len() - 20;
        if declared > available {
            return Err(SanitizeError::JsonTruncated {
                declared,
                available,
            });
        }
        let document: Value = serde_json::from_slice(&bytes[20..20 + declared])
            .map_err(|error| SanitizeError::JsonInvalid(error.to_string()))?;
        if !document.is_object() {
            return Err(SanitizeError::JsonNotObject);
        }
        Ok(Self {
            magic,
            version: u32_at(header, 4),
            chunk_type,
            document,
            tail: bytes[20 + declared..].to_vec(),
        })
    }

    /// Reframe the JSON chunk at a four-byte boundary and recompute the total.
    fn encode(self) -> Result<Vec<u8>, SanitizeError> {
        let mut json = serde_json::to_vec(&self.document).expect("a parsed document serialises");
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + self.tail.len();
        let total = u32::try_from(total).map_err(|_| SanitizeError::TooLarge { bytes: total })?;
        // The chunk length cannot exceed the container length checked above.
        let length = json.len() as u32;
        let mut out = Vec::with_capacity(total as usize);
        out.extend_from_slice(&self.magic);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&self.chunk_type.to_le_bytes());
        out.extend_from_slice(&json);
        out.extend_from_slice(&self.tail);
        Ok(out)
    }

    /// Re-encode only when a rule changed something.
    fn repair(self, changed: usize) -> Result<Repair, SanitizeError> {
        Ok(Repair {
            bytes: match changed {
                0 => None,
                _ => Some(self.encode()?),
            },
            changed,
        })
    }
}

/// One little-endian `u32` at an offset of a header that already has four bytes
/// there.
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("four-byte field"),
    )
}

/// Zero every mesh's default morph weights.
///
/// The exporter samples the animated shape-key values at the current frame, so
/// a non-zero default applies that target at load time (Bevy renders the cloth
/// as a crumpled sheet). The animation channels keep their own values, and the
/// target count is preserved.
pub fn reset_mesh_weights(bytes: &[u8]) -> Result<Repair, SanitizeError> {
    let mut container = Container::parse(bytes)?;
    let mut changed = 0;
    if let Some(meshes) = container
        .document
        .get_mut("meshes")
        .and_then(Value::as_array_mut)
    {
        for mesh in meshes {
            let Some(weights) = mesh.get_mut("weights").and_then(Value::as_array_mut) else {
                continue;
            };
            let mut reset = false;
            for weight in weights {
                if weight.as_f64() != Some(0.0) {
                    *weight = json!(0.0);
                    reset = true;
                }
            }
            changed += usize::from(reset);
        }
    }
    container.repair(changed)
}

/// Clamp every `texCoord: -1` to UV set 0 through each material subtree.
///
/// Blender writes `-1` for a material texture whose UV map does not resolve on
/// a mesh; glTF wants an unsigned set index, so a reader either drops the
/// texture or refuses the asset. UV set 0 is the only set the cloth shells
/// carry.
pub fn clamp_texcoord(bytes: &[u8]) -> Result<Repair, SanitizeError> {
    let mut container = Container::parse(bytes)?;
    let mut changed = 0;
    if let Some(materials) = container
        .document
        .get_mut("materials")
        .and_then(Value::as_array_mut)
    {
        for material in materials {
            changed += clamp_set(material);
        }
    }
    container.repair(changed)
}

/// Walk one material subtree, clamping every `-1` set index to 0.
fn clamp_set(node: &mut Value) -> usize {
    match node {
        Value::Object(map) => {
            let changed = match map.get_mut("texCoord") {
                Some(value) if value.as_f64() == Some(-1.0) => {
                    *value = json!(0);
                    1
                }
                _ => 0,
            };
            changed + map.values_mut().map(clamp_set).sum::<usize>()
        }
        Value::Array(values) => values.iter_mut().map(clamp_set).sum(),
        _ => 0,
    }
}
