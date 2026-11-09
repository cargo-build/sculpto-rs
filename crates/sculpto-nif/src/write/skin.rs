//! Skin blocks shared by the SE and LE writers.
//!
//! The model carries bone names, per-bone bind matrices and four influences per
//! vertex. Both targets write `NiSkinInstance` + `NiSkinData`; SE additionally
//! packs every vertex once into `NiSkinPartition.global_vertex_data`
//! (`VF_SKINNED` included) and leaves the shape without inline geometry, while
//! LE keeps geometry on `NiTriShapeData` and reads weights from the sparse
//! `NiSkinData` arrays. Bones are `NiNode` blocks so the reader can resolve
//! each palette entry to its name.

use std::collections::{BTreeMap, BTreeSet};

use sculpto_core::mesh::MeshAsset;

use super::header::NifWriteError;

/// Validated palette, bind matrices and per-bone sparse weights.
pub struct SkinPlan {
    /// Bone names in palette order.
    pub names: Vec<String>,
    /// Per-bone bind matrix, column-major (mesh -> bone at bind).
    pub bind: Vec<[[f32; 4]; 4]>,
    /// Per bone: `(vertex, weight)` entries with a non-zero influence.
    pub weights: Vec<Vec<(u16, f32)>>,
}

/// Build the skin plan, refusing a palette the file format cannot express.
pub fn plan(asset: &MeshAsset) -> Result<Option<SkinPlan>, NifWriteError> {
    let Some(skin) = &asset.skin else {
        return Ok(None);
    };
    if skin.bones.is_empty() {
        return Err(NifWriteError::UnnamedBone { index: 0 });
    }
    if skin.bones.len() > u8::MAX as usize {
        return Err(NifWriteError::TooManyBones {
            bones: skin.bones.len(),
        });
    }
    let mut seen = BTreeSet::new();
    for (index, name) in skin.bones.iter().enumerate() {
        if name.trim().is_empty() {
            return Err(NifWriteError::UnnamedBone { index });
        }
        if name.len() > u8::MAX as usize {
            return Err(NifWriteError::NameTooLong { name: name.clone() });
        }
        if !seen.insert(name.as_str()) {
            return Err(NifWriteError::DuplicateBone { name: name.clone() });
        }
    }
    let bind = match skin.bind_inverse.is_empty() {
        true => vec![identity(); skin.bones.len()],
        false => skin.bind_inverse.clone(),
    };
    let mut weights = vec![Vec::new(); skin.bones.len()];
    for (vertex, (indices, quad)) in skin.indices.iter().zip(&skin.weights).enumerate() {
        let vertex = u16::try_from(vertex).map_err(|_| NifWriteError::TooManyVertices {
            vertices: skin.indices.len(),
        })?;
        for (slot, bone) in indices.iter().enumerate() {
            let weight = quad[slot];
            if weight > 0.0 {
                weights[*bone as usize].push((vertex, weight));
            }
        }
    }
    Ok(Some(SkinPlan {
        names: skin.bones.clone(),
        bind,
        weights,
    }))
}

/// A named `NiNode` body with an identity transform, the given children and
/// extra-data references.
pub fn node(name: i32, children: &[i32], extra_data: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_i32(&mut bytes, name);
    push_u32(&mut bytes, extra_data.len() as u32);
    for index in extra_data {
        push_i32(&mut bytes, *index);
    }
    push_i32(&mut bytes, -1); // controller
    push_u32(&mut bytes, 0); // flags
    for value in [0.0f32, 0.0, 0.0] {
        push_f32(&mut bytes, value);
    }
    for row in 0..3 {
        for column in 0..3 {
            push_f32(&mut bytes, if row == column { 1.0 } else { 0.0 });
        }
    }
    push_f32(&mut bytes, 1.0);
    push_i32(&mut bytes, -1); // collision
    push_u32(&mut bytes, children.len() as u32);
    for child in children {
        push_i32(&mut bytes, *child);
    }
    push_u32(&mut bytes, 0); // effects
    bytes
}

/// A `NiSkinInstance` body pointing at `NiSkinData` and the bone nodes.
pub fn instance(data: i32, partition: i32, skeleton_root: i32, bones: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_i32(&mut bytes, data);
    push_i32(&mut bytes, partition);
    push_i32(&mut bytes, skeleton_root);
    push_u32(&mut bytes, bones.len() as u32);
    for bone in bones {
        push_i32(&mut bytes, *bone);
    }
    bytes
}

/// A `NiSkinData` body with an identity global transform and sparse weights.
pub fn data(plan: &SkinPlan) -> Result<Vec<u8>, NifWriteError> {
    let mut bytes = Vec::new();
    push_transform(&mut bytes, &identity(), 0)?;
    push_u32(&mut bytes, plan.names.len() as u32);
    push_u8(&mut bytes, 1); // has vertex weights
    for (bone, entries) in plan.weights.iter().enumerate() {
        push_transform(&mut bytes, &plan.bind[bone], bone)?;
        for _ in 0..4 {
            push_f32(&mut bytes, 0.0); // bounding sphere
        }
        push_u16(&mut bytes, entries.len() as u16);
        for (vertex, weight) in entries {
            push_u16(&mut bytes, *vertex);
            push_f32(&mut bytes, *weight);
        }
    }
    Ok(bytes)
}

fn push_transform(
    bytes: &mut Vec<u8>,
    matrix: &[[f32; 4]; 4],
    bone: usize,
) -> Result<(), NifWriteError> {
    let scale =
        (matrix[0][0] * matrix[0][0] + matrix[0][1] * matrix[0][1] + matrix[0][2] * matrix[0][2])
            .sqrt();
    if scale == 0.0 || !scale.is_finite() {
        return Err(NifWriteError::SingularBind { bone });
    }
    for row in &[0, 1, 2] {
        for column in &[0, 1, 2] {
            push_f32(bytes, matrix[*column][*row] / scale);
        }
    }
    push_f32(bytes, matrix[3][0]);
    push_f32(bytes, matrix[3][1]);
    push_f32(bytes, matrix[3][2]);
    push_f32(bytes, scale);
    Ok(())
}

/// The identity bind matrix.
pub fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0f32; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// The `NiSkinPartition` SSE global packed-vertex buffer.
///
/// Skyrim SE stores a skinned shape's geometry once, in the linked partition
/// block: `data_size`, `vertex_size`, `vertex_desc`, then `vertex_size` bytes
/// per mesh vertex. Every entry's `triangles` (and the trailing SSE copy) index
/// that buffer globally, not through the partition's `vertex_map`.
#[derive(Debug, Clone, Copy)]
pub struct GlobalVertices<'a> {
    /// `BSVertexDesc` shared by the shape and every partition entry.
    pub vertex_desc: u64,
    /// Packed stride per vertex, in bytes.
    pub vertex_size: u32,
    /// Packed vertices in mesh order, `vertex_size * mesh vertices` bytes.
    pub bytes: &'a [u8],
}

/// A `NiSkinPartition` body: one entry per triangle partition.
///
/// Each entry's palette is exactly the bones its vertices reference
/// ([`MeshAsset::partition_palettes`]); the call is refused if that projection
/// cannot cover the mesh. A `global` buffer selects the SSE layout: the packed
/// vertices are written once before the entries, and both triangle copies use
/// mesh-global indices (nifly forces `bMappedIndices = false` on stream 100).
/// Without one the LE layout stores palette-local indices and no global buffer.
pub fn partitions(
    mesh: &MeshAsset,
    global: Option<&GlobalVertices<'_>>,
) -> Result<Vec<u8>, NifWriteError> {
    partitions_from_palettes(mesh, &mesh.partition_palettes(), global)
}

/// Same as [`partitions`], but with a caller-supplied palette set.
///
/// The set is validated first, so an authored palette that omits a referenced
/// bone is a typed refusal rather than a mesh that deforms wrongly at runtime.
pub fn partitions_from_palettes(
    mesh: &MeshAsset,
    palettes: &[sculpto_core::mesh::PartitionPalette],
    global: Option<&GlobalVertices<'_>>,
) -> Result<Vec<u8>, NifWriteError> {
    mesh.validate_partition_palettes(palettes)
        .map_err(NifWriteError::InvalidModel)?;
    let skin = mesh
        .skin
        .as_ref()
        .ok_or(NifWriteError::UnnamedBone { index: 0 })?;
    if let Some(global) = global {
        let expected = mesh.positions.len() * global.vertex_size as usize;
        if global.vertex_size == 0
            || global.bytes.len() != expected
            || ((global.vertex_desc & 0xF) as u32) * 4 != global.vertex_size
        {
            return Err(NifWriteError::GlobalVertexMismatch {
                vertices: mesh.positions.len(),
                vertex_size: global.vertex_size,
                bytes: global.bytes.len(),
            });
        }
    }

    let mut entries = Vec::with_capacity(palettes.len());
    for palette in palettes {
        let triangles: Vec<[u32; 3]> = mesh
            .triangles
            .iter()
            .zip(&mesh.triangle_partitions)
            .filter(|(_, partition)| **partition == palette.partition)
            .map(|(triangle, _)| *triangle)
            .collect();
        let mut map: Vec<u16> = Vec::new();
        let mut local: BTreeMap<u32, u16> = BTreeMap::new();
        for triangle in &triangles {
            for vertex in triangle {
                if !local.contains_key(vertex) {
                    let index =
                        u16::try_from(map.len()).map_err(|_| NifWriteError::TooManyVertices {
                            vertices: map.len(),
                        })?;
                    local.insert(*vertex, index);
                    map.push(u16::try_from(*vertex).map_err(|_| {
                        NifWriteError::TooManyVertices {
                            vertices: *vertex as usize,
                        }
                    })?);
                }
            }
        }
        let local_triangles: Vec<[u16; 3]> = triangles
            .iter()
            .map(|triangle| {
                [
                    local[&triangle[0]],
                    local[&triangle[1]],
                    local[&triangle[2]],
                ]
            })
            .collect();
        // SSE triangles address the partition's global buffer (nifly sets
        // `bMappedIndices = false` on stream 100), so the same indices land in
        // both copies. LE keeps the `vertex_map`-local form.
        let written_triangles: Vec<[u16; 3]> = match global.is_some() {
            true => triangles
                .iter()
                .map(|triangle| {
                    let index = |vertex: u32| {
                        u16::try_from(vertex).map_err(|_| NifWriteError::TooManyVertices {
                            vertices: vertex as usize,
                        })
                    };
                    Ok([
                        index(triangle[0])?,
                        index(triangle[1])?,
                        index(triangle[2])?,
                    ])
                })
                .collect::<Result<_, NifWriteError>>()?,
            false => local_triangles.clone(),
        };

        let mut entry = Vec::new();
        push_u16(&mut entry, map.len() as u16);
        push_u16(&mut entry, local_triangles.len() as u16);
        push_u16(&mut entry, palette.bones.len() as u16);
        push_u16(&mut entry, 0); // strips
        push_u16(&mut entry, 4); // weights per vertex
        for bone in &palette.bones {
            push_u16(&mut entry, *bone);
        }
        push_u8(&mut entry, 1); // has vertex map
        for vertex in &map {
            push_u16(&mut entry, *vertex);
        }
        // Weights in map order, four per vertex.
        push_u8(&mut entry, 1); // has vertex weights
        for vertex in &map {
            for weight in skin.weights[*vertex as usize] {
                push_f32(&mut entry, weight);
            }
        }
        push_u8(&mut entry, 1); // has faces
        for triangle in &written_triangles {
            push_u16(&mut entry, triangle[0]);
            push_u16(&mut entry, triangle[1]);
            push_u16(&mut entry, triangle[2]);
        }
        // Bone indices are palette-local.
        push_u8(&mut entry, 1); // has bone indices
        for vertex in &map {
            for bone in skin.indices[*vertex as usize] {
                let position = palette
                    .bones
                    .iter()
                    .position(|candidate| *candidate == bone)
                    .ok_or(NifWriteError::InvalidModel(
                        sculpto_core::mesh::MeshError::PaletteMissingBone {
                            partition: palette.partition,
                            bone,
                        },
                    ))?;
                push_u8(&mut entry, position as u8);
            }
        }
        push_u8(&mut entry, 0); // lod level
        push_u8(&mut entry, 0); // global vertex buffer
        if let Some(global) = global {
            push_u64(&mut entry, global.vertex_desc);
            for triangle in &written_triangles {
                push_u16(&mut entry, triangle[0]);
                push_u16(&mut entry, triangle[1]);
                push_u16(&mut entry, triangle[2]);
            }
        }
        entries.push(entry);
    }

    let mut bytes = Vec::new();
    push_u32(&mut bytes, entries.len() as u32);
    if let Some(global) = global {
        push_u32(&mut bytes, global.bytes.len() as u32);
        push_u32(&mut bytes, global.vertex_size);
        push_u64(&mut bytes, global.vertex_desc);
        bytes.extend_from_slice(global.bytes);
    }
    for entry in entries {
        bytes.extend_from_slice(&entry);
    }
    Ok(bytes)
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_core::mesh::{PartitionId, PartitionPalette, Skin};

    fn two_partitions() -> MeshAsset {
        MeshAsset {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [1, 2, 3]],
            triangle_partitions: vec![PartitionId(0), PartitionId(1)],
            skin: Some(Skin {
                bones: vec!["a".into(), "b".into(), "c".into()],
                bind_inverse: Vec::new(),
                indices: vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0], [2, 0, 0, 0]],
                weights: vec![
                    [1.0, 0.0, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                ],
            }),
            ..MeshAsset::default()
        }
    }

    #[test]
    fn partitions_use_the_exact_palette_per_triangle_range() {
        let mesh = two_partitions();
        mesh.validate().expect("valid");
        let palettes = mesh.partition_palettes();
        let bytes = partitions(&mesh, None).expect("partitions write");
        assert!(!bytes.is_empty());
        // Two entries: each palette is the derived one, in partition order.
        assert_eq!(palettes.len(), 2);
        assert_eq!(palettes[0].bones, vec![0, 1]);
        assert_eq!(palettes[1].bones, vec![0, 1, 2]);
    }

    #[test]
    fn a_palette_that_misses_a_bone_is_refused() {
        let mesh = two_partitions();
        let mut palettes = mesh.partition_palettes();
        palettes[0].bones = vec![0];
        let error = partitions_from_palettes(&mesh, &palettes, None)
            .expect_err("a missing bone must be refused");
        assert_eq!(
            error,
            NifWriteError::InvalidModel(sculpto_core::mesh::MeshError::PaletteMissingBone {
                partition: PartitionId(0),
                bone: 1
            })
        );
    }

    #[test]
    fn a_missing_partition_is_refused() {
        let mesh = two_partitions();
        let mut palettes: Vec<PartitionPalette> = mesh.partition_palettes();
        palettes.pop();
        assert!(matches!(
            partitions_from_palettes(&mesh, &palettes, None),
            Err(NifWriteError::InvalidModel(
                sculpto_core::mesh::MeshError::MissingPartitionPalette { .. }
            ))
        ));
    }

    #[test]
    fn a_global_buffer_that_does_not_pack_the_mesh_is_refused() {
        let mesh = two_partitions();
        let short = vec![0u8; 63];
        let global = GlobalVertices {
            vertex_desc: (0x001u64 << 44) | 4,
            vertex_size: 16,
            bytes: &short,
        };
        assert!(matches!(
            partitions(&mesh, Some(&global)),
            Err(NifWriteError::GlobalVertexMismatch {
                vertices: 4,
                vertex_size: 16,
                bytes: 63
            })
        ));
        let stride = vec![0u8; 64];
        let mismatched = GlobalVertices {
            vertex_desc: (0x001u64 << 44) | 8,
            vertex_size: 16,
            bytes: &stride,
        };
        assert!(partitions(&mesh, Some(&mismatched)).is_err());
    }
}
