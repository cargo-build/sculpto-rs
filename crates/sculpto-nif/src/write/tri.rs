//! `.tri` morph file format writer, reader, and index validator.
//!
//! Layout reference: `BodySlide-and-Outfit-Studio/src/files/TriFile.cpp`
//! (`TriFile::Write` and `TriFile::Read`).
//!
//! Binary layout:
//! - Magic header: `TRIP` (`0x5452_4950` as little-endian `u32`, bytes `b"PIRT"`).
//! - Position shape count: `u16` little-endian.
//! - For each shape with position morphs:
//!   - Name length: `u8`.
//!   - Name: UTF-8 / ASCII bytes.
//!   - Morph count: `u16` little-endian.
//!   - For each position morph:
//!     - Morph name length: `u8`.
//!     - Morph name: UTF-8 / ASCII bytes.
//!     - Multiplier `mult`: `f32` little-endian (`max_component / 32767.0`).
//!     - Vertex count: `u16` little-endian.
//!     - For each vertex:
//!       - Vertex id: `u16` little-endian.
//!       - `x`, `y`, `z`: 3 lanes of `i16` little-endian (`round(offset / mult)`).
//! - UV shape count: `u16` little-endian.
//! - For each shape with UV morphs:
//!   - Name length: `u8`.
//!   - Name: UTF-8 / ASCII bytes.
//!   - Morph count: `u16` little-endian.
//!   - For each UV morph:
//!     - Morph name length: `u8`.
//!     - Morph name: UTF-8 / ASCII bytes.
//!     - Multiplier `mult`: `f32` little-endian (`max_component / 32767.0`).
//!     - Vertex count: `u16` little-endian.
//!     - For each vertex:
//!       - Vertex id: `u16` little-endian.
//!       - `u`, `v`: 2 lanes of `i16` little-endian (`round(offset / mult)`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Magic 4-byte header for `.tri` body morph files (`"TRIP"_mci` in BodySlide).
pub const MAGIC: u32 = 0x5452_4950;

/// Magic bytes as stored on disk in little-endian order (`[b'P', b'I', b'R', b'T']`).
pub const MAGIC_BYTES: [u8; 4] = *b"PIRT";

/// Quantisation divisor (`0x7FFF = 32767`).
pub const QUANT_DIVISOR: f32 = 32767.0;

/// One sparse position offset in a `.tri` morph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionOffset {
    /// Vertex index addressed in the base mesh.
    pub vertex: u16,
    /// 3D displacement vector `[x, y, z]`.
    pub offset: [f32; 3],
}

/// One sparse UV offset in a `.tri` morph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvOffset {
    /// Vertex index addressed in the base mesh.
    pub vertex: u16,
    /// 2D texture coordinate displacement `[u, v]`.
    pub offset: [f32; 2],
}

/// One named position morph: sparse 3D offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct TriPositionMorph {
    /// Morph name.
    pub name: String,
    /// Sparse vertex displacements.
    pub offsets: Vec<PositionOffset>,
}

/// One named UV morph: sparse 2D offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct TriUvMorph {
    /// Morph name.
    pub name: String,
    /// Sparse vertex UV displacements.
    pub offsets: Vec<UvOffset>,
}

/// One shape in a `.tri` file, carrying position and/or UV morphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriShape {
    /// Shape name matching the NIF node / geometry.
    pub name: String,
    /// Base mesh vertex count, if known. When present, offsets are validated
    /// to ensure `vertex < vertex_count`.
    pub vertex_count: Option<u32>,
    /// Position morph targets.
    pub position_morphs: Vec<TriPositionMorph>,
    /// UV morph targets.
    pub uv_morphs: Vec<TriUvMorph>,
}

impl TriShape {
    /// Create a shape with only position morphs.
    pub fn new_positions(name: impl Into<String>, position_morphs: Vec<TriPositionMorph>) -> Self {
        Self {
            name: name.into(),
            vertex_count: None,
            position_morphs,
            uv_morphs: Vec::new(),
        }
    }

    /// Validate the shape's bounds, uniqueness, and name lengths.
    pub fn validate(&self, limit: Option<u32>) -> Result<(), TriError> {
        if self.name.len() > 255 {
            return Err(TriError::NameTooLong {
                name: self.name.clone(),
                len: self.name.len(),
            });
        }
        if self.position_morphs.len() > u16::MAX as usize {
            return Err(TriError::TooManyMorphs {
                shape: self.name.clone(),
                count: self.position_morphs.len(),
            });
        }
        let effective_limit = self.vertex_count.or(limit);

        for morph in &self.position_morphs {
            if morph.name.len() > 255 {
                return Err(TriError::NameTooLong {
                    name: morph.name.clone(),
                    len: morph.name.len(),
                });
            }
            if morph.offsets.len() > u16::MAX as usize {
                return Err(TriError::TooManyVertices {
                    morph: morph.name.clone(),
                    count: morph.offsets.len(),
                });
            }
            let mut seen = BTreeSet::new();
            for o in &morph.offsets {
                if let Some(lim) = effective_limit
                    && (o.vertex as u32) >= lim
                {
                    return Err(TriError::VertexOutOfRange {
                        morph: morph.name.clone(),
                        vertex: o.vertex as u32,
                        limit: lim,
                    });
                }
                if !seen.insert(o.vertex) {
                    return Err(TriError::DuplicateVertexId {
                        morph: morph.name.clone(),
                        vertex: o.vertex,
                    });
                }
            }
        }

        if self.uv_morphs.len() > u16::MAX as usize {
            return Err(TriError::TooManyMorphs {
                shape: self.name.clone(),
                count: self.uv_morphs.len(),
            });
        }

        for morph in &self.uv_morphs {
            if morph.name.len() > 255 {
                return Err(TriError::NameTooLong {
                    name: morph.name.clone(),
                    len: morph.name.len(),
                });
            }
            if morph.offsets.len() > u16::MAX as usize {
                return Err(TriError::TooManyVertices {
                    morph: morph.name.clone(),
                    count: morph.offsets.len(),
                });
            }
            let mut seen = BTreeSet::new();
            for o in &morph.offsets {
                if let Some(lim) = effective_limit
                    && (o.vertex as u32) >= lim
                {
                    return Err(TriError::VertexOutOfRange {
                        morph: morph.name.clone(),
                        vertex: o.vertex as u32,
                        limit: lim,
                    });
                }
                if !seen.insert(o.vertex) {
                    return Err(TriError::DuplicateVertexId {
                        morph: morph.name.clone(),
                        vertex: o.vertex,
                    });
                }
            }
        }

        Ok(())
    }
}

impl From<&sculpto_core::mesh::TriShape> for TriShape {
    fn from(source: &sculpto_core::mesh::TriShape) -> Self {
        Self {
            name: source.name.clone(),
            vertex_count: Some(source.vertex_count),
            position_morphs: source
                .morphs
                .iter()
                .map(|m| TriPositionMorph {
                    name: m.name.clone(),
                    offsets: m
                        .offsets
                        .iter()
                        .map(|o| PositionOffset {
                            vertex: o.vertex,
                            offset: o.position,
                        })
                        .collect(),
                })
                .collect(),
            uv_morphs: Vec::new(),
        }
    }
}

impl From<sculpto_core::mesh::TriShape> for TriShape {
    fn from(source: sculpto_core::mesh::TriShape) -> Self {
        Self::from(&source)
    }
}

/// A complete `.tri` document holding shapes and their sparse morphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriFile {
    /// Shapes in document order.
    pub shapes: Vec<TriShape>,
}

impl TriFile {
    /// Create a new document with given shapes.
    pub fn new(shapes: Vec<TriShape>) -> Self {
        Self { shapes }
    }

    /// Validate all shapes, vertex limits, and structure counts.
    pub fn validate(&self) -> Result<(), TriError> {
        let pos_count = self
            .shapes
            .iter()
            .filter(|s| !s.position_morphs.is_empty())
            .count();
        if pos_count > u16::MAX as usize {
            return Err(TriError::TooManyShapes { count: pos_count });
        }
        let uv_count = self
            .shapes
            .iter()
            .filter(|s| !s.uv_morphs.is_empty())
            .count();
        if uv_count > u16::MAX as usize {
            return Err(TriError::TooManyShapes { count: uv_count });
        }

        for shape in &self.shapes {
            shape.validate(None)?;
        }
        Ok(())
    }
}

/// Why a `.tri` document cannot be written, read, or validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriError {
    /// A vertex index is greater than or equal to the shape's vertex count limit.
    VertexOutOfRange {
        morph: String,
        vertex: u32,
        limit: u32,
    },
    /// A morph contains duplicate offsets for the same vertex index.
    DuplicateVertexId { morph: String, vertex: u16 },
    /// A shape or morph name exceeds 255 bytes.
    NameTooLong { name: String, len: usize },
    /// The number of shapes exceeds 65 535 (`u16::MAX`).
    TooManyShapes { count: usize },
    /// The number of morphs in a shape exceeds 65 535 (`u16::MAX`).
    TooManyMorphs { shape: String, count: usize },
    /// The number of vertices in a morph exceeds 65 535 (`u16::MAX`).
    TooManyVertices { morph: String, count: usize },
    /// The file does not start with the expected `TRIP` magic header.
    InvalidMagic { magic: [u8; 4] },
    /// The file ended unexpectedly while reading.
    Truncated { expected: usize, available: usize },
    /// A shape or morph name is not valid UTF-8.
    InvalidUtf8 { detail: String },
}

impl fmt::Display for TriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VertexOutOfRange {
                morph,
                vertex,
                limit,
            } => write!(
                f,
                "vertex {vertex} in morph `{morph}` exceeds vertex count limit {limit}"
            ),
            Self::DuplicateVertexId { morph, vertex } => {
                write!(f, "duplicate vertex id {vertex} in morph `{morph}`")
            }
            Self::NameTooLong { name, len } => {
                write!(
                    f,
                    "name `{name}` has length {len} bytes, exceeding 255 byte maximum"
                )
            }
            Self::TooManyShapes { count } => {
                write!(f, "shape count {count} exceeds 65535 limit")
            }
            Self::TooManyMorphs { shape, count } => {
                write!(
                    f,
                    "morph count {count} in shape `{shape}` exceeds 65535 limit"
                )
            }
            Self::TooManyVertices { morph, count } => {
                write!(
                    f,
                    "vertex count {count} in morph `{morph}` exceeds 65535 limit"
                )
            }
            Self::InvalidMagic { magic } => {
                write!(
                    f,
                    "invalid magic header {magic:?}, expected TRIP (b\"PIRT\")"
                )
            }
            Self::Truncated {
                expected,
                available,
            } => {
                write!(
                    f,
                    "unexpected end of file: expected {expected} bytes, but only {available} available"
                )
            }
            Self::InvalidUtf8 { detail } => {
                write!(f, "invalid UTF-8 in shape or morph name: {detail}")
            }
        }
    }
}

impl std::error::Error for TriError {}

/// Compute the quantisation multiplier for a position morph.
///
/// In the C++ reference (`TriFile.cpp` line 205):
/// `mult = max(|x|, |y|, |z|) / 0x7FFF`.
/// If all offsets are zero (or offsets is empty), `mult` is `0.0`.
pub fn compute_position_multiplier(offsets: &[PositionOffset]) -> f32 {
    let mut max_comp = 0.0f32;
    for o in offsets {
        max_comp = max_comp.max(o.offset[0].abs());
        max_comp = max_comp.max(o.offset[1].abs());
        max_comp = max_comp.max(o.offset[2].abs());
    }
    max_comp / QUANT_DIVISOR
}

/// Compute the quantisation multiplier for a UV morph.
///
/// In the C++ reference (`TriFile.cpp` line 262):
/// `mult = max(|u|, |v|) / 0x7FFF`.
/// If all offsets are zero (or offsets is empty), `mult` is `0.0`.
pub fn compute_uv_multiplier(offsets: &[UvOffset]) -> f32 {
    let mut max_comp = 0.0f32;
    for o in offsets {
        max_comp = max_comp.max(o.offset[0].abs());
        max_comp = max_comp.max(o.offset[1].abs());
    }
    max_comp / QUANT_DIVISOR
}

/// Quantise a single coordinate lane with the given multiplier.
#[inline]
fn quantize_lane(value: f32, mult: f32) -> i16 {
    if mult == 0.0 {
        0
    } else {
        (value / mult).round().clamp(-32767.0, 32767.0) as i16
    }
}

/// Serialize a [`TriFile`] into `.tri` binary format.
pub fn write_tri(file: &TriFile) -> Result<Vec<u8>, TriError> {
    file.validate()?;

    let pos_shapes: Vec<&TriShape> = file
        .shapes
        .iter()
        .filter(|s| !s.position_morphs.is_empty())
        .collect();
    if pos_shapes.len() > u16::MAX as usize {
        return Err(TriError::TooManyShapes {
            count: pos_shapes.len(),
        });
    }

    let uv_shapes: Vec<&TriShape> = file
        .shapes
        .iter()
        .filter(|s| !s.uv_morphs.is_empty())
        .collect();
    if uv_shapes.len() > u16::MAX as usize {
        return Err(TriError::TooManyShapes {
            count: uv_shapes.len(),
        });
    }

    let mut buf = Vec::new();

    // 1. Magic
    buf.extend_from_slice(&MAGIC_BYTES);

    // 2. Position shape count
    buf.extend_from_slice(&(pos_shapes.len() as u16).to_le_bytes());

    // 3. Position shapes
    for shape in &pos_shapes {
        let name_bytes = shape.name.as_bytes();
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);

        buf.extend_from_slice(&(shape.position_morphs.len() as u16).to_le_bytes());

        for morph in &shape.position_morphs {
            let morph_name_bytes = morph.name.as_bytes();
            buf.push(morph_name_bytes.len() as u8);
            buf.extend_from_slice(morph_name_bytes);

            let mult = compute_position_multiplier(&morph.offsets);
            buf.extend_from_slice(&mult.to_le_bytes());

            buf.extend_from_slice(&(morph.offsets.len() as u16).to_le_bytes());

            let mut sorted_offsets = morph.offsets.clone();
            sorted_offsets.sort_by_key(|o| o.vertex);

            for o in &sorted_offsets {
                buf.extend_from_slice(&o.vertex.to_le_bytes());
                let x = quantize_lane(o.offset[0], mult);
                let y = quantize_lane(o.offset[1], mult);
                let z = quantize_lane(o.offset[2], mult);
                buf.extend_from_slice(&x.to_le_bytes());
                buf.extend_from_slice(&y.to_le_bytes());
                buf.extend_from_slice(&z.to_le_bytes());
            }
        }
    }

    // 4. UV shape count
    buf.extend_from_slice(&(uv_shapes.len() as u16).to_le_bytes());

    // 5. UV shapes
    for shape in &uv_shapes {
        let name_bytes = shape.name.as_bytes();
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);

        buf.extend_from_slice(&(shape.uv_morphs.len() as u16).to_le_bytes());

        for morph in &shape.uv_morphs {
            let morph_name_bytes = morph.name.as_bytes();
            buf.push(morph_name_bytes.len() as u8);
            buf.extend_from_slice(morph_name_bytes);

            let mult = compute_uv_multiplier(&morph.offsets);
            buf.extend_from_slice(&mult.to_le_bytes());

            buf.extend_from_slice(&(morph.offsets.len() as u16).to_le_bytes());

            let mut sorted_offsets = morph.offsets.clone();
            sorted_offsets.sort_by_key(|o| o.vertex);

            for o in &sorted_offsets {
                buf.extend_from_slice(&o.vertex.to_le_bytes());
                let u = quantize_lane(o.offset[0], mult);
                let v = quantize_lane(o.offset[1], mult);
                buf.extend_from_slice(&u.to_le_bytes());
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }
    }

    Ok(buf)
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], TriError> {
        if self.pos + n > self.bytes.len() {
            return Err(TriError::Truncated {
                expected: n,
                available: self.remaining(),
            });
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, TriError> {
        let b = self.take(1)?;
        Ok(b[0])
    }

    fn read_u16_le(&mut self) -> Result<u16, TriError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_i16_le(&mut self) -> Result<i16, TriError> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    fn read_f32_le(&mut self) -> Result<f32, TriError> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_string(&mut self) -> Result<String, TriError> {
        let len = self.read_u8()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|e| TriError::InvalidUtf8 {
                detail: e.to_string(),
            })
    }
}

/// Deserialize a `.tri` binary byte slice into a [`TriFile`].
pub fn read_tri(bytes: &[u8]) -> Result<TriFile, TriError> {
    read_tri_with_limit(bytes, None)
}

/// Deserialize a `.tri` binary byte slice into a [`TriFile`], validating that no
/// vertex index exceeds `vertex_limit`.
pub fn read_tri_with_limit(bytes: &[u8], vertex_limit: Option<u32>) -> Result<TriFile, TriError> {
    let mut reader = Reader::new(bytes);

    let magic = reader.take(4)?;
    if magic != MAGIC_BYTES {
        return Err(TriError::InvalidMagic {
            magic: [magic[0], magic[1], magic[2], magic[3]],
        });
    }

    let shape_count = reader.read_u16_le()? as usize;

    let mut shape_indices: BTreeMap<String, usize> = BTreeMap::new();
    let mut shapes: Vec<TriShape> = Vec::new();

    // 1. Read position morphs
    for _ in 0..shape_count {
        let shape_name = reader.read_string()?;
        let morph_count = reader.read_u16_le()? as usize;

        let shape_idx = match shape_indices.get(&shape_name) {
            Some(&idx) => idx,
            None => {
                let idx = shapes.len();
                shapes.push(TriShape {
                    name: shape_name.clone(),
                    vertex_count: vertex_limit,
                    position_morphs: Vec::new(),
                    uv_morphs: Vec::new(),
                });
                shape_indices.insert(shape_name.clone(), idx);
                idx
            }
        };

        for _ in 0..morph_count {
            let morph_name = reader.read_string()?;
            let mult = reader.read_f32_le()?;
            let vert_count = reader.read_u16_le()? as usize;

            let mut offsets = Vec::with_capacity(vert_count);
            let mut seen = BTreeSet::new();

            for _ in 0..vert_count {
                let id = reader.read_u16_le()?;
                let x = reader.read_i16_le()?;
                let y = reader.read_i16_le()?;
                let z = reader.read_i16_le()?;

                if let Some(lim) = vertex_limit
                    && (id as u32) >= lim
                {
                    return Err(TriError::VertexOutOfRange {
                        morph: morph_name.clone(),
                        vertex: id as u32,
                        limit: lim,
                    });
                }

                if !seen.insert(id) {
                    return Err(TriError::DuplicateVertexId {
                        morph: morph_name.clone(),
                        vertex: id,
                    });
                }

                offsets.push(PositionOffset {
                    vertex: id,
                    offset: [x as f32 * mult, y as f32 * mult, z as f32 * mult],
                });
            }

            shapes[shape_idx].position_morphs.push(TriPositionMorph {
                name: morph_name,
                offsets,
            });
        }
    }

    // 2. Read UV morphs
    let shape_count_uv = reader.read_u16_le()? as usize;

    for _ in 0..shape_count_uv {
        let shape_name = reader.read_string()?;
        let morph_count = reader.read_u16_le()? as usize;

        let shape_idx = match shape_indices.get(&shape_name) {
            Some(&idx) => idx,
            None => {
                let idx = shapes.len();
                shapes.push(TriShape {
                    name: shape_name.clone(),
                    vertex_count: vertex_limit,
                    position_morphs: Vec::new(),
                    uv_morphs: Vec::new(),
                });
                shape_indices.insert(shape_name.clone(), idx);
                idx
            }
        };

        for _ in 0..morph_count {
            let morph_name = reader.read_string()?;
            let mult = reader.read_f32_le()?;
            let vert_count = reader.read_u16_le()? as usize;

            let mut offsets = Vec::with_capacity(vert_count);
            let mut seen = BTreeSet::new();

            for _ in 0..vert_count {
                let id = reader.read_u16_le()?;
                let u = reader.read_i16_le()?;
                let v = reader.read_i16_le()?;

                if let Some(lim) = vertex_limit
                    && (id as u32) >= lim
                {
                    return Err(TriError::VertexOutOfRange {
                        morph: morph_name.clone(),
                        vertex: id as u32,
                        limit: lim,
                    });
                }

                if !seen.insert(id) {
                    return Err(TriError::DuplicateVertexId {
                        morph: morph_name.clone(),
                        vertex: id,
                    });
                }

                offsets.push(UvOffset {
                    vertex: id,
                    offset: [u as f32 * mult, v as f32 * mult],
                });
            }

            shapes[shape_idx].uv_morphs.push(TriUvMorph {
                name: morph_name,
                offsets,
            });
        }
    }

    Ok(TriFile { shapes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_bytes_match_u32_le() {
        assert_eq!(MAGIC.to_le_bytes(), MAGIC_BYTES);
        assert_eq!(&MAGIC_BYTES, b"PIRT");
    }

    #[test]
    fn roundtrip_zero_difference_for_exact_fixture() {
        // D01: 1 shape, 2 morphs (1 position, 1 UV), non-trivial offsets, zero difference.
        let shape = TriShape {
            name: "BodyShape".to_string(),
            vertex_count: Some(500),
            position_morphs: vec![TriPositionMorph {
                name: "Breasts".to_string(),
                offsets: vec![
                    PositionOffset {
                        vertex: 0,
                        offset: [100.0, -200.0, 300.0],
                    },
                    PositionOffset {
                        vertex: 10,
                        offset: [32767.0, -16383.0, 0.0],
                    },
                    PositionOffset {
                        vertex: 25,
                        offset: [-32767.0, 100.0, -500.0],
                    },
                ],
            }],
            uv_morphs: vec![TriUvMorph {
                name: "BreastsUV".to_string(),
                offsets: vec![
                    UvOffset {
                        vertex: 0,
                        offset: [10.0, -20.0],
                    },
                    UvOffset {
                        vertex: 5,
                        offset: [32767.0, -100.0],
                    },
                ],
            }],
        };

        let file = TriFile::new(vec![shape.clone()]);
        let bytes = write_tri(&file).expect("write should succeed");
        let decoded = read_tri(&bytes).expect("read should succeed");

        assert_eq!(decoded.shapes.len(), 1);
        let decoded_shape = &decoded.shapes[0];
        assert_eq!(decoded_shape.name, shape.name);
        assert_eq!(decoded_shape.position_morphs, shape.position_morphs);
        assert_eq!(decoded_shape.uv_morphs, shape.uv_morphs);
    }

    #[test]
    fn quantisation_error_under_step() {
        // D02: arbitrary floating point offsets, verify error is strictly <= step
        let max_comp = 15.4321_f32;
        let step = max_comp / QUANT_DIVISOR;

        let shape = TriShape {
            name: "ArbitraryFloat".to_string(),
            vertex_count: None,
            position_morphs: vec![TriPositionMorph {
                name: "Deform".to_string(),
                offsets: vec![
                    PositionOffset {
                        vertex: 1,
                        offset: [1.2345, -6.7891, 15.4321],
                    },
                    PositionOffset {
                        vertex: 2,
                        offset: [-12.3456, 4.5678, -0.0123],
                    },
                ],
            }],
            uv_morphs: vec![TriUvMorph {
                name: "UvDeform".to_string(),
                offsets: vec![UvOffset {
                    vertex: 1,
                    offset: [0.1234, -15.4321],
                }],
            }],
        };

        let file = TriFile::new(vec![shape.clone()]);
        let bytes = write_tri(&file).unwrap();
        let decoded = read_tri(&bytes).unwrap();

        for (orig, dec) in shape.position_morphs[0]
            .offsets
            .iter()
            .zip(&decoded.shapes[0].position_morphs[0].offsets)
        {
            assert_eq!(orig.vertex, dec.vertex);
            for lane in 0..3 {
                let err = (orig.offset[lane] - dec.offset[lane]).abs();
                assert!(
                    err <= step + 1e-6,
                    "lane {lane} err {err} must be <= step {step}"
                );
            }
        }

        let uv_step = 15.4321 / QUANT_DIVISOR;
        for (orig, dec) in shape.uv_morphs[0]
            .offsets
            .iter()
            .zip(&decoded.shapes[0].uv_morphs[0].offsets)
        {
            assert_eq!(orig.vertex, dec.vertex);
            for lane in 0..2 {
                let err = (orig.offset[lane] - dec.offset[lane]).abs();
                assert!(
                    err <= uv_step + 1e-6,
                    "uv lane {lane} err {err} must be <= step {uv_step}"
                );
            }
        }
    }

    #[test]
    fn all_zero_morph_quantisation_and_reference_behavior() {
        // D02: case where mult is 0 (all-zero morph)
        // Reference TriFile.cpp behaviour:
        // In Write, mult = 0.0f / 0x7FFF = 0.0f. Then `v.second.x / mult` divides 0.0f by 0.0f (NaN),
        // casting NaN to short (undefined behavior in C++).
        // In Read, offsets are reconstructed as `x * mult = 0.0f`.
        // The reference then executes `if (!offset.IsZero(true))` which discards all near-zero deltas.
        // If all offsets are zero, `morphOffsets.size() == 0`, and the reference skips adding the morph.
        //
        // Sculpto decision:
        // The writer writes `mult = 0.0f` and quantises to `0_i16` without division by zero.
        // The reader preserves all decoded offsets faithfully.
        let shape = TriShape {
            name: "ZeroTest".to_string(),
            vertex_count: Some(10),
            position_morphs: vec![TriPositionMorph {
                name: "ZeroPos".to_string(),
                offsets: vec![
                    PositionOffset {
                        vertex: 0,
                        offset: [0.0, 0.0, 0.0],
                    },
                    PositionOffset {
                        vertex: 1,
                        offset: [0.0, 0.0, 0.0],
                    },
                ],
            }],
            uv_morphs: vec![TriUvMorph {
                name: "ZeroUv".to_string(),
                offsets: vec![UvOffset {
                    vertex: 0,
                    offset: [0.0, 0.0],
                }],
            }],
        };

        let file = TriFile::new(vec![shape]);
        let bytes = write_tri(&file).unwrap();
        let decoded = read_tri(&bytes).unwrap();

        assert_eq!(decoded.shapes.len(), 1);
        let pos_morph = &decoded.shapes[0].position_morphs[0];
        assert_eq!(pos_morph.name, "ZeroPos");
        for o in &pos_morph.offsets {
            assert_eq!(o.offset, [0.0, 0.0, 0.0]);
        }

        let uv_morph = &decoded.shapes[0].uv_morphs[0];
        assert_eq!(uv_morph.name, "ZeroUv");
        for o in &uv_morph.offsets {
            assert_eq!(o.offset, [0.0, 0.0]);
        }
    }

    #[test]
    fn index_validation_refusals() {
        // D03:
        // 1. Out-of-range vertex id
        let shape_out_of_range = TriShape {
            name: "Test".to_string(),
            vertex_count: Some(5),
            position_morphs: vec![TriPositionMorph {
                name: "Morph".to_string(),
                offsets: vec![PositionOffset {
                    vertex: 5,
                    offset: [1.0, 0.0, 0.0],
                }],
            }],
            uv_morphs: Vec::new(),
        };
        assert_eq!(
            write_tri(&TriFile::new(vec![shape_out_of_range])),
            Err(TriError::VertexOutOfRange {
                morph: "Morph".to_string(),
                vertex: 5,
                limit: 5
            })
        );

        // 2. Duplicate vertex id
        let shape_dup = TriShape {
            name: "Test".to_string(),
            vertex_count: None,
            position_morphs: vec![TriPositionMorph {
                name: "Morph".to_string(),
                offsets: vec![
                    PositionOffset {
                        vertex: 3,
                        offset: [1.0, 0.0, 0.0],
                    },
                    PositionOffset {
                        vertex: 3,
                        offset: [2.0, 0.0, 0.0],
                    },
                ],
            }],
            uv_morphs: Vec::new(),
        };
        assert_eq!(
            write_tri(&TriFile::new(vec![shape_dup])),
            Err(TriError::DuplicateVertexId {
                morph: "Morph".to_string(),
                vertex: 3
            })
        );

        // 3. Name longer than 255 bytes
        let long_name = "a".repeat(256);
        let shape_long_name = TriShape {
            name: long_name.clone(),
            vertex_count: None,
            position_morphs: Vec::new(),
            uv_morphs: Vec::new(),
        };
        assert_eq!(
            write_tri(&TriFile::new(vec![shape_long_name])),
            Err(TriError::NameTooLong {
                name: long_name,
                len: 256
            })
        );

        // 4. Truncated reader
        let valid_bytes = write_tri(&TriFile::new(vec![TriShape {
            name: "S".to_string(),
            vertex_count: None,
            position_morphs: vec![TriPositionMorph {
                name: "M".to_string(),
                offsets: vec![PositionOffset {
                    vertex: 0,
                    offset: [1.0, 2.0, 3.0],
                }],
            }],
            uv_morphs: Vec::new(),
        }]))
        .unwrap();

        // Truncate at each byte length
        for len in 0..valid_bytes.len() - 1 {
            let res = read_tri(&valid_bytes[..len]);
            assert!(
                matches!(
                    res,
                    Err(TriError::Truncated { .. }) | Err(TriError::InvalidMagic { .. })
                ),
                "len {len} must fail with Truncated or InvalidMagic"
            );
        }
    }
}
