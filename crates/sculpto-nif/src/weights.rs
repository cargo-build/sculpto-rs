//! Authored skin weights read from the source bytes, before any projection.
//!
//! The vendored importer renormalizes an inline SE vertex stream
//! (`renormalize_skin_weights`, `blocks/tri_shape/mod.rs:83-96`) and truncates
//! plus renormalizes LE sparse data to four lanes
//! (`import/mesh/skin.rs:828-905`). Neither value is the authored one, so this
//! module reads the skin channels itself:
//!
//! - LE: every `NiSkinData` sparse `(vertex, weight)` entry, all influences.
//! - SE: the packed vertex lanes, either the retained
//!   `NiSkinPartition.global_vertex_data.raw_bytes` when the shape carries no
//!   inline stream, or the inline run located by re-reading the shape's block
//!   prefix from the original bytes with the public header/stream/AV-object API.
//!
//! A shape that cannot be matched unambiguously, or whose required raw payload
//! is missing or malformed, is a named refusal. Nothing is normalized, dropped
//! or rebound here. @req DECL0008

use std::ops::Range;

use byroredux_nif::blocks::base::NiAVObjectData;
use byroredux_nif::blocks::node::NiNode;
use byroredux_nif::blocks::skin::{
    BsDismemberSkinInstance, NiSkinData, NiSkinInstance, NiSkinPartition,
};
use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape, NiTriShapeData};
use byroredux_nif::header::NifHeader;
use byroredux_nif::import::ImportedMesh;
use byroredux_nif::scene::NifScene;
use byroredux_nif::stream::NifStream;
use byroredux_nif::types::BlockRef;
use sculpto_core::skin::{FourLane, SkinWeights};

use crate::NifError;

/// Vertex attribute flag: the vertex carries position data.
const VF_VERTEX: u16 = 0x001;
/// Vertex attribute flag: UV data.
const VF_UVS: u16 = 0x002;
/// Vertex attribute flag: normal data.
const VF_NORMALS: u16 = 0x008;
/// Vertex attribute flag: tangent data (present only with normals).
const VF_TANGENTS: u16 = 0x010;
/// Vertex attribute flag: vertex colours.
const VF_VERTEX_COLORS: u16 = 0x020;
/// Vertex attribute flag: packed skin weights and bone indices.
const VF_SKINNED: u16 = 0x040;
/// Vertex attribute flag: full-precision positions (Fallout 4 and later).
const VF_FULL_PRECISION: u16 = 0x400;
/// BSVER of Fallout 4; Skyrim is 100 (SE) and 83 (LE).
const BSVER_FALLOUT4: u32 = 130;

/// Bytes of one vertex's skin channel: four `f16` weights then four `u8` bones.
const SKIN_CHANNEL_BYTES: usize = 12;

/// Capture the authored influences of every imported mesh, in mesh order.
///
/// `Some` for a skinned mesh, `None` for an unskinned one. The row count of a
/// captured skin must equal the mesh's vertex count; the caller checks that.
pub(crate) fn capture(
    bytes: &[u8],
    scene: &NifScene,
    meshes: &[ImportedMesh],
) -> Result<Vec<Option<SkinWeights>>, NifError> {
    let (header, data_start) =
        NifHeader::parse(bytes).map_err(|error| weights_error(format_args!("header: {error}")))?;
    if !header.block_sizes.is_empty() && header.block_sizes.len() != scene.blocks.len() {
        return Err(NifError::Malformed(format!(
            "weights: header records {} block sizes for {} blocks",
            header.block_sizes.len(),
            scene.blocks.len()
        )));
    }
    let blocks = block_ranges(&header, data_start);
    let sources = source_shapes(scene);
    meshes
        .iter()
        .map(|mesh| match &mesh.skin {
            None => Ok(None),
            Some(skin) => {
                let palette: Vec<String> = skin
                    .bones
                    .iter()
                    .map(|bone| bone.name.to_string())
                    .collect();
                let source = match_shapes(
                    &sources,
                    mesh.name.as_deref(),
                    &palette,
                    mesh.positions.len(),
                )?;
                let rows = source.rows(bytes, &header, &blocks, scene)?;
                build_rows(rows, palette.len(), &source.name).map(Some)
            }
        })
        .collect()
}

/// One source shape that could carry the skin of an imported mesh.
struct SourceShape {
    /// Block index in the file.
    block: usize,
    /// `NiObjectNET` name of the shape block, empty when unnamed.
    name: String,
    /// The shape's skin instance, when it has one.
    skin_ref: BlockRef,
    /// Palette node names in skin-instance order.
    palette: Vec<String>,
    /// Vertex count the shape declares, when it declares one.
    vertices: Option<usize>,
}

impl SourceShape {
    /// Read this shape's authored rows: `(palette index, weight)` per vertex.
    fn rows(
        &self,
        bytes: &[u8],
        header: &NifHeader,
        blocks: &[Range<usize>],
        scene: &NifScene,
    ) -> Result<Vec<Vec<(u16, f32)>>, NifError> {
        let index = self
            .skin_ref
            .index()
            .ok_or_else(|| shape_error(&self.name, "shape has no skin instance"))?;
        let instance = instance_of(scene, index)
            .ok_or_else(|| shape_error(&self.name, "skin instance is unreadable"))?;
        match scene.get_as::<BsTriShape>(self.block) {
            Some(shape) => {
                let range = blocks
                    .get(self.block)
                    .ok_or_else(|| shape_error(&self.name, "block has no recorded size"))?;
                packed_rows(bytes, header, range, scene, instance, shape, &self.name)
            }
            None => sparse_rows(scene, instance, self.vertices, &self.name),
        }
    }
}

/// The instance body, whether the block is a plain `NiSkinInstance` or the LE
/// dismemberment subclass that embeds one.
///
/// A body mesh usually carries `BSDismemberSkinInstance`, so reading only the
/// plain type would leave every LE body without a source palette — and a skinned
/// mesh with no readable skin is a refusal, not a silent fallback. @req DECL0008
fn instance_of(scene: &NifScene, index: usize) -> Option<&NiSkinInstance> {
    match scene.get_as::<NiSkinInstance>(index) {
        Some(instance) => Some(instance),
        None => scene
            .get_as::<BsDismemberSkinInstance>(index)
            .map(|dismember| &dismember.base),
    }
}

/// Every skinnable shape block with its palette identity.
fn source_shapes(scene: &NifScene) -> Vec<SourceShape> {
    let mut shapes = Vec::new();
    for block in 0..scene.blocks.len() {
        let (name, skin_ref, vertices) = if let Some(shape) = scene.get_as::<BsTriShape>(block) {
            (
                shape.av.net.name.as_deref().unwrap_or_default().to_string(),
                shape.skin_ref,
                match shape.num_vertices {
                    0 => None,
                    count => Some(count as usize),
                },
            )
        } else if let Some(shape) = scene.get_as::<NiTriShape>(block) {
            let vertices = shape
                .data_ref
                .index()
                .and_then(|index| scene.get_as::<NiTriShapeData>(index))
                .map(|data| data.vertices.len());
            (
                shape.av.net.name.as_deref().unwrap_or_default().to_string(),
                shape.skin_instance_ref,
                vertices,
            )
        } else {
            continue;
        };
        let Some(index) = skin_ref.index() else {
            continue;
        };
        let Some(instance) = instance_of(scene, index) else {
            continue;
        };
        // Same spelling as the importer's palette (`resolve_node_name`, with the
        // positional fallback), so identity and palette compare like for like.
        let palette = instance
            .bone_refs
            .iter()
            .enumerate()
            .map(|(index, bone)| {
                bone.index()
                    .and_then(|index| scene.get_as::<NiNode>(index))
                    .and_then(|node| node.av.net.name.as_ref().map(|name| name.to_string()))
                    .unwrap_or_else(|| format!("Bone{index}"))
            })
            .collect();
        shapes.push(SourceShape {
            block,
            name,
            skin_ref,
            palette,
            vertices,
        });
    }
    shapes
}

/// Find the source shape that carries an imported mesh's skin.
///
/// Identity first: the importer copies the shape block's `NiObjectNET` name onto
/// the mesh, so a present name selects that block and its declared vertex count
/// is then verified. Without an identity the palette must be unique; several
/// matches are disambiguated by vertex count and otherwise refused with every
/// candidate listed. @req DECL0008
fn match_shapes<'a>(
    shapes: &'a [SourceShape],
    name: Option<&str>,
    palette: &[String],
    vertices: usize,
) -> Result<&'a SourceShape, NifError> {
    let candidates: Vec<&SourceShape> = shapes
        .iter()
        .filter(|shape| shape.palette == palette)
        .collect();
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        let named: Vec<&SourceShape> = candidates
            .iter()
            .copied()
            .filter(|shape| shape.name == name)
            .collect();
        return match named.as_slice() {
            [shape] => match shape.vertices {
                Some(declared) if declared != vertices => Err(NifError::Malformed(format!(
                    "weights: shape `{name}` declares {declared} vertices but the imported mesh \
                     has {vertices}"
                ))),
                _ => Ok(*shape),
            },
            [] => Err(NifError::Malformed(format!(
                "weights: no source shape named `{name}` carries the {}-bone palette",
                palette.len()
            ))),
            many => Err(NifError::Malformed(format!(
                "weights: {} source shapes are named `{name}` ({})",
                many.len(),
                list_shapes(many)
            ))),
        };
    }
    match candidates.as_slice() {
        [shape] => Ok(*shape),
        [] => Err(NifError::Malformed(format!(
            "weights: no source shape carries the {}-bone palette",
            palette.len()
        ))),
        many => {
            let counted: Vec<&SourceShape> = many
                .iter()
                .copied()
                .filter(|shape| match shape.vertices {
                    None => true,
                    Some(declared) => declared == vertices,
                })
                .collect();
            match counted.as_slice() {
                [shape] => Ok(*shape),
                _ => Err(NifError::Malformed(format!(
                    "weights: {} source shapes share the palette and none is identified by \
                     vertex count ({})",
                    many.len(),
                    list_shapes(many)
                ))),
            }
        }
    }
}

/// Name every candidate shape for a refusal message.
fn list_shapes(shapes: &[&SourceShape]) -> String {
    shapes
        .iter()
        .map(|shape| match shape.name.is_empty() {
            true => format!("block {}", shape.block),
            false => format!("`{}`", shape.name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// LE: every sparse `NiSkinData` entry, all influences, source order.
fn sparse_rows(
    scene: &NifScene,
    instance: &NiSkinInstance,
    vertices: Option<usize>,
    name: &str,
) -> Result<Vec<Vec<(u16, f32)>>, NifError> {
    let data = instance
        .data_ref
        .index()
        .and_then(|index| scene.get_as::<NiSkinData>(index))
        .ok_or_else(|| shape_error(name, "LE skin data is unreadable"))?;
    sparse_rows_from(data, vertices).map_err(|detail| shape_error(name, &detail))
}

/// Rows from a parsed `NiSkinData`, keeping every non-zero sparse entry.
///
/// LE data is sparse, so a vertex with no entry has no influence at all rather
/// than a missing row. The declared vertex count therefore owns the row count:
/// rows are padded to it, and an entry past it is a refusal. Without a declared
/// count the highest weighted vertex bounds the rows. @req DECL0008
fn sparse_rows_from(
    data: &NiSkinData,
    vertices: Option<usize>,
) -> Result<Vec<Vec<(u16, f32)>>, String> {
    let weighted = data
        .bones
        .iter()
        .flat_map(|bone| bone.vertex_weights.iter())
        .map(|entry| entry.vertex_index as usize + 1)
        .max()
        .unwrap_or(0);
    let count = match vertices {
        Some(declared) if weighted > declared => {
            return Err(format!(
                "LE weight names vertex {} past the declared {declared}",
                weighted - 1
            ));
        }
        Some(declared) => declared,
        None => weighted,
    };
    let mut rows: Vec<Vec<(u16, f32)>> = vec![Vec::new(); count];
    for (bone, data) in data.bones.iter().enumerate() {
        let bone = u16::try_from(bone)
            .map_err(|_| "bone index exceeds the u16 palette index range".to_string())?;
        for entry in &data.vertex_weights {
            let vertex = entry.vertex_index as usize;
            if vertex >= rows.len() {
                return Err("LE weight names a vertex past the shape".to_string());
            }
            if entry.weight != 0.0 {
                rows[vertex].push((bone, entry.weight));
            }
        }
    }
    Ok(rows)
}

/// SE: the packed vertex lanes, read before any importer renormalization.
fn packed_rows(
    bytes: &[u8],
    header: &NifHeader,
    range: &Range<usize>,
    scene: &NifScene,
    instance: &NiSkinInstance,
    shape: &BsTriShape,
    name: &str,
) -> Result<Vec<Vec<(u16, f32)>>, NifError> {
    let attrs = ((shape.vertex_desc >> 44) & 0xFFF) as u16;
    if attrs & VF_SKINNED == 0 {
        return Err(shape_error(name, "vertex descriptor is not skinned"));
    }
    let full_precision = header.user_version_2 < BSVER_FALLOUT4 || attrs & VF_FULL_PRECISION != 0;
    // Presence of the decoded inline channel is the importer's own test: SSE
    // bodies ship `num_vertices == 0` with the packed run in the partition.
    let lanes = match shape.bone_weights.is_empty() {
        true => global_buffer_rows(scene, instance, attrs, full_precision, name)?,
        false => inline_rows(bytes, header, range, shape, attrs, full_precision, name)?,
    };
    Ok(lanes
        .into_iter()
        .map(|FourLane { weights, joints }| {
            joints
                .into_iter()
                .zip(weights)
                .filter_map(|(joint, weight)| match weight != 0.0 {
                    true => Some((joint, weight)),
                    false => None,
                })
                .collect()
        })
        .collect())
}

/// SE partition-only shapes keep the packed run in `global_vertex_data`.
fn global_buffer_rows(
    scene: &NifScene,
    instance: &NiSkinInstance,
    attrs: u16,
    full_precision: bool,
    name: &str,
) -> Result<Vec<FourLane>, NifError> {
    let partition = instance
        .skin_partition_ref
        .index()
        .and_then(|index| scene.get_as::<NiSkinPartition>(index))
        .ok_or_else(|| shape_error(name, "SE shape has no readable skin partition"))?;
    let buffer = partition
        .global_vertex_data
        .as_ref()
        .ok_or_else(|| shape_error(name, "SE shape has no inline stream and no packed buffer"))?;
    let stride = usize::try_from(buffer.vertex_size)
        .map_err(|_| shape_error(name, "SE packed stride exceeds the address space"))?;
    decode_lanes(&buffer.raw_bytes, stride, attrs, full_precision)
        .map_err(|detail| shape_error(name, &detail))
}

/// SE inline shapes: locate the vertex run in the original block bytes.
fn inline_rows(
    bytes: &[u8],
    header: &NifHeader,
    range: &Range<usize>,
    shape: &BsTriShape,
    attrs: u16,
    full_precision: bool,
    name: &str,
) -> Result<Vec<FourLane>, NifError> {
    if header.user_version_2 >= BSVER_FALLOUT4 {
        return Err(shape_error(
            name,
            "inline vertex recovery supports the Skyrim packed layout only",
        ));
    }
    let mut stream = NifStream::new(bytes, header);
    stream.set_position(range.start as u64);
    let start = read_shape_prefix(&mut stream)
        .map_err(|error| shape_error(name, &format!("inline vertex prefix: {error}")))?;
    let stride = usize::try_from(shape.vertex_desc & 0xF)
        .map_err(|_| shape_error(name, "vertex stride exceeds the address space"))?
        * 4;
    let needed = stride
        .checked_mul(usize::from(shape.num_vertices))
        .ok_or_else(|| shape_error(name, "inline vertex run overflows the address space"))?;
    let end = (start as usize)
        .checked_add(needed)
        .ok_or_else(|| shape_error(name, "inline vertex run overflows the address space"))?;
    if end > bytes.len() || end > range.end || end < range.start {
        return Err(shape_error(name, "inline vertex run exceeds the block"));
    }
    decode_lanes(&bytes[start as usize..end], stride, attrs, full_precision)
        .map_err(|detail| shape_error(name, &detail))
}

/// Read the `BSTriShape` fields that precede the inline vertex stream, and
/// return the stream's file offset.
///
/// This mirrors the prefix of the vendored `BsTriShape::parse` exactly; the
/// public piece (position, radius, refs, descriptor, counts) is all that is
/// needed to locate the vertex run. Skyrim only — the caller gates the version.
fn read_shape_prefix(stream: &mut NifStream<'_>) -> std::io::Result<u64> {
    let _av = NiAVObjectData::parse_no_properties(stream)?;
    let _center = stream.read_ni_point3()?;
    let _radius = stream.read_f32_le()?;
    let _skin_ref = stream.read_block_ref()?;
    let _shader_ref = stream.read_block_ref()?;
    let _alpha_ref = stream.read_block_ref()?;
    let _vertex_desc = stream.read_u64_le()?;
    let _num_triangles = stream.read_u16_le()?;
    let _num_vertices = stream.read_u16_le()?;
    let _data_size = stream.read_u32_le()?;
    Ok(stream.position())
}

/// Decode four-lane skin channels from a packed vertex buffer.
///
/// Walks records of `stride` bytes with the native field order; the skin channel
/// sits after every enabled attribute prefix. Values are widened from `f16`
/// exactly and never normalized.
fn decode_lanes(
    buffer: &[u8],
    stride: usize,
    attrs: u16,
    full_precision: bool,
) -> Result<Vec<FourLane>, String> {
    let offset = skin_offset(attrs, full_precision)
        .ok_or_else(|| "vertex descriptor has no skinned channel".to_string())?;
    if stride == 0 || offset + SKIN_CHANNEL_BYTES > stride {
        return Err(format!(
            "skinned vertex needs {} bytes but the stride is {stride}",
            offset + SKIN_CHANNEL_BYTES
        ));
    }
    if buffer.is_empty() || !buffer.len().is_multiple_of(stride) {
        return Err(format!(
            "packed buffer of {} bytes is not a multiple of the {stride}-byte stride",
            buffer.len()
        ));
    }
    let mut lanes = Vec::with_capacity(buffer.len() / stride);
    for vertex in buffer.chunks_exact(stride) {
        let channel = &vertex[offset..offset + SKIN_CHANNEL_BYTES];
        let mut weights = [0.0_f32; 4];
        for (slot, weight) in weights.iter_mut().enumerate() {
            let mut bits = [0_u8; 2];
            bits.copy_from_slice(&channel[slot * 2..slot * 2 + 2]);
            *weight = f32::from(half::f16::from_bits(u16::from_le_bytes(bits)));
        }
        let joints = [channel[8], channel[9], channel[10], channel[11]].map(u16::from);
        lanes.push(FourLane { joints, weights });
    }
    Ok(lanes)
}

/// Byte offset of the skin channel inside a packed vertex record.
fn skin_offset(attrs: u16, full_precision: bool) -> Option<usize> {
    if attrs & VF_SKINNED == 0 {
        return None;
    }
    let mut offset = 0;
    if attrs & VF_VERTEX != 0 {
        offset += match full_precision {
            true => 16,
            false => 8,
        };
    }
    if attrs & VF_UVS != 0 {
        offset += 4;
    }
    if attrs & VF_NORMALS != 0 {
        offset += 4;
    }
    if attrs & VF_TANGENTS != 0 && attrs & VF_NORMALS != 0 {
        offset += 4;
    }
    if attrs & VF_VERTEX_COLORS != 0 {
        offset += 4;
    }
    Some(offset)
}

/// Build one CSR value from per-vertex `(palette index, weight)` rows.
fn build_rows(
    rows: Vec<Vec<(u16, f32)>>,
    palette: usize,
    name: &str,
) -> Result<SkinWeights, NifError> {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut joints = Vec::new();
    let mut values = Vec::new();
    offsets.push(0_u32);
    for row in rows {
        for (joint, weight) in row {
            joints.push(joint);
            values.push(weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| shape_error(name, "weight entries exceed the offset range"))?,
        );
    }
    SkinWeights::new(offsets, joints, values, palette)
        .map_err(|error| shape_error(name, &error.to_string()))
}

/// Byte range of every block, from the header's per-block sizes.
fn block_ranges(header: &NifHeader, data_start: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::with_capacity(header.block_sizes.len());
    let mut start = data_start;
    for size in &header.block_sizes {
        let end = start.saturating_add(*size as usize);
        ranges.push(start..end);
        start = end;
    }
    ranges
}

fn shape_error(name: &str, detail: &str) -> NifError {
    match name.is_empty() {
        true => NifError::Malformed(format!("weights: {detail}")),
        false => NifError::Malformed(format!("weights: shape `{name}`: {detail}")),
    }
}

fn weights_error(detail: std::fmt::Arguments<'_>) -> NifError {
    NifError::Malformed(format!("weights: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use byroredux_nif::blocks::skin::{BoneData, BoneVertWeight};
    use byroredux_nif::types::{NiMatrix3, NiPoint3, NiTransform};

    fn transform() -> NiTransform {
        NiTransform {
            rotation: NiMatrix3 {
                rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            },
            translation: NiPoint3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            scale: 1.0,
        }
    }

    #[test]
    fn the_skin_channel_sits_after_every_enabled_attribute() {
        let everything =
            VF_VERTEX | VF_UVS | VF_NORMALS | VF_TANGENTS | VF_VERTEX_COLORS | VF_SKINNED;
        assert_eq!(skin_offset(everything, true), Some(32));
        assert_eq!(skin_offset(everything, false), Some(24));
        assert_eq!(skin_offset(VF_VERTEX | VF_SKINNED, true), Some(16));
        assert_eq!(skin_offset(VF_SKINNED, true), Some(0));
        assert_eq!(skin_offset(VF_UVS, true), None);
    }

    #[test]
    fn packed_lanes_keep_the_authored_half_float_values() {
        // One vertex: position (full precision), UV, normals, tangents, colours,
        // then weights that deliberately sum to 0.9999 rather than 1.
        let mut vertex = vec![0_u8; 16 + 4 + 4 + 4 + 4];
        for weight in [0.5_f32, 0.25, 0.2499, 0.0] {
            vertex.extend_from_slice(&half::f16::from_f32(weight).to_bits().to_le_bytes());
        }
        vertex.extend_from_slice(&[3, 4, 5, 0]);
        let attrs = VF_VERTEX | VF_UVS | VF_NORMALS | VF_TANGENTS | VF_VERTEX_COLORS | VF_SKINNED;

        let lanes = decode_lanes(&vertex, vertex.len(), attrs, true).expect("one packed vertex");
        assert_eq!(lanes.len(), 1);
        let FourLane { weights, joints } = lanes[0];
        assert_eq!(joints, [3, 4, 5, 0]);
        assert_eq!(weights[0], 0.5);
        assert_eq!(weights[1], 0.25);
        // Exactly the stored half value: 0.2499 is not representable, but the
        // decode must not rescale the row to sum to one.
        assert_eq!(weights[2], half::f16::from_f32(0.2499).to_f32());
        assert!((weights.iter().sum::<f32>() - 1.0).abs() > 1.0e-5);
    }

    #[test]
    fn a_stride_that_cannot_hold_the_channel_is_refused() {
        let attrs = VF_VERTEX | VF_SKINNED;
        assert!(decode_lanes(&[0_u8; 16], 16, attrs, true).is_err());
        assert!(decode_lanes(&[0_u8; 30], 15, attrs, true).is_err());
        assert!(decode_lanes(&[], 28, attrs, true).is_err());
    }

    #[test]
    fn sparse_rows_keep_five_influences_and_source_order() {
        let mut bones = Vec::new();
        for entries in [
            vec![(0_u16, 0.2_f32), (1, 0.3)],
            vec![(0, 0.1)],
            vec![(0, 0.1)],
            vec![(0, 0.15)],
            vec![(0, 0.15)],
        ] {
            bones.push(BoneData {
                skin_transform: transform(),
                bounding_sphere: [0.0; 4],
                vertex_weights: entries
                    .into_iter()
                    .map(|(vertex_index, weight)| BoneVertWeight {
                        vertex_index,
                        weight,
                    })
                    .collect(),
            });
        }
        let data = NiSkinData {
            skin_transform: transform(),
            bones,
        };
        let rows = sparse_rows_from(&data, None).expect("a five-influence row");
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            [(0, 0.2), (1, 0.1), (2, 0.1), (3, 0.15), (4, 0.15)]
        );
        assert_eq!(rows[1], [(0, 0.3)]);

        // A declared count owns the row count: an unweighted tail vertex keeps
        // its empty row, and an entry past the declaration is refused.
        let padded = sparse_rows_from(&data, Some(4)).expect("a declared count pads the tail");
        assert_eq!(padded.len(), 4);
        assert!(padded[2].is_empty() && padded[3].is_empty());
        assert!(sparse_rows_from(&data, Some(1)).is_err());
    }
}
