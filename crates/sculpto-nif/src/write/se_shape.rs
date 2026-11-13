//! Skyrim SE `BSTriShape` writer: one shape's block and its block layout.
//!
//! C03 wrote positions and triangles, C05 added UVs, C06 byte normals and SE
//! tangents, C07 added the skin: one `NiSkinInstance` + `NiSkinData`, one named
//! `NiNode` per bone, and the per-vertex influences (`VF_SKINNED`). C10 writes
//! the shader and texture set; C11 adds the HDT-SMP physics link as one
//! `NiStringExtraData`. A skinned shape keeps no inline geometry: the packed
//! vertex stream lives once in `NiSkinPartition.global_vertex_data`, which is
//! where nifly reads and writes it for `IsSkinned()` SSE shapes. Morph blocks
//! arrive later (C12+). `sculpto_nif::read` undoes the reader's frame
//! conversion, so every written attribute round-trips.
//!
//! The per-shape facts live in [`super::prepared::PreparedShape`] and the
//! container tail in [`super::layout`], both shared with the LE writer; [`body`]
//! is the only SE-specific encoder here. [`super::se_bundle`] reuses the same
//! owners for a multi-shape file, so the two entry points cannot disagree about
//! a shape's bytes.

use sculpto_core::mesh::MeshAsset;

use super::extra;
use super::header::{NifTarget, NifWriteError};
use super::layout::{Block, write_file};
use super::prepared::PreparedShape;
use super::scene::{CheckedScene, NativeTransform, SkinNodes};
use super::shader;
use super::skin;
use super::strings::StringTable;
use super::{InfluencePolicy, WrittenNif};

/// Block type table name for a `BSTriShape`.
pub(super) const BLOCK_TYPE: &str = "BSTriShape";

/// Write a complete SE NIF containing one `BSTriShape`.
///
/// The model is validated first; a refused model leaves no bytes. Normals are
/// byte-encoded, so they round-trip within the format's 8-bit precision;
/// tangents are only written when the model carries both normals and tangents.
/// A skinned model writes the packed vertex stream into
/// `NiSkinPartition.global_vertex_data` and the palette/bind matrices into
/// `NiSkinData`, leaving the shape's own vertex/triangle counts and
/// `data_size` at zero; morphs are deliberately not written yet (C08 onward).
pub fn write_se_shape(asset: &MeshAsset) -> Result<Vec<u8>, NifWriteError> {
    write(asset, InfluencePolicy::Preserve, None).map(|output| output.bytes)
}

pub(super) fn write(
    asset: &MeshAsset,
    policy: InfluencePolicy,
    checked: Option<&CheckedScene<'_>>,
) -> Result<WrittenNif, NifWriteError> {
    let prepared = PreparedShape::new(asset, policy, NifTarget::Se)?;
    let skinned = prepared.skinned();

    let mut strings = StringTable::new();
    let name_index = match asset.name.is_empty() {
        true => -1,
        false => strings.intern(&asset.name) as i32,
    };
    let skin_nodes = prepared
        .skin
        .as_ref()
        .map(|plan| SkinNodes::plan(checked, &plan.names, 4, &mut strings))
        .transpose()?;
    let node_count = skin_nodes.as_ref().map_or(0, SkinNodes::len);
    // Without a skin this file has no root node: the shape is the file root, so
    // it is the only block before the optional shader pair and physics link.
    let mut next_block = match skinned {
        true => 4 + node_count as i32,
        false => 1,
    };
    let shader_index = match prepared.has_material() {
        true => take(&mut next_block),
        false => -1,
    };
    let texture_index = match shader_index >= 0 && !prepared.texture.is_empty() {
        true => take(&mut next_block),
        false => -1,
    };
    // Nothing follows the physics link, so its index is the last block.
    let extra_index = match prepared.physics.is_some() {
        true => next_block,
        false => -1,
    };

    let transform = checked.map_or(NativeTransform::IDENTITY, |scene| scene.placement);
    // NiObjectNET: name, extra data, no controller. With a skin the file has a
    // root `NiNode`, and the game only reads the physics link from there
    // (`OutfitProject.cpp:250-252`); without one the shape is the root.
    let shape_extra = match skinned {
        true => -1,
        false => extra_index,
    };
    let mut blocks = vec![Block {
        type_name: BLOCK_TYPE,
        body: body(
            asset,
            &prepared,
            ShapeRefs {
                name: name_index,
                extra: shape_extra,
                skin: if skinned { 1 } else { -1 },
                shader: shader_index,
            },
            transform,
        ),
    }];
    let root_extra: Vec<i32> = match extra_index {
        -1 => Vec::new(),
        index => vec![index],
    };
    if let Some((plan, nodes)) = prepared.skin.as_ref().zip(skin_nodes) {
        blocks.push(Block {
            type_name: skin::INSTANCE_TYPE,
            body: skin::instance(2, 3, nodes.root, &nodes.bones),
        });
        blocks.push(Block {
            type_name: skin::DATA_TYPE,
            body: skin::data(
                plan,
                &checked.map_or_else(skin::identity, CheckedScene::global_skin),
            )?,
        });
        blocks.push(Block {
            type_name: skin::PARTITION_TYPE,
            body: skin::partitions(asset, plan, Some(&prepared.global()))?,
        });
        for node in nodes.encode(&root_extra) {
            blocks.push(Block {
                type_name: skin::NODE_TYPE,
                body: node,
            });
        }
    }
    if shader_index >= 0 {
        blocks.push(Block {
            type_name: shader::SHADER_TYPE,
            body: prepared.shader_body(name_index, texture_index),
        });
        if texture_index >= 0 {
            blocks.push(Block {
                type_name: shader::TEXTURE_TYPE,
                body: prepared.texture.clone(),
            });
        }
    }
    if let Some(link) = prepared.physics.as_ref() {
        blocks.push(Block {
            type_name: extra::BLOCK_TYPE,
            body: extra::body(link, &mut strings),
        });
    }
    Ok(WrittenNif {
        bytes: write_file(NifTarget::Se, &blocks, &strings),
        dropped_influences: prepared
            .skin
            .map_or_else(Vec::new, |plan| plan.dropped_influences),
    })
}

/// Assign the next block index and advance the cursor.
fn take(next: &mut i32) -> i32 {
    let index = *next;
    *next += 1;
    index
}

/// One shape's block references inside its file.
pub(super) struct ShapeRefs {
    /// String-table index of the shape name, or `-1`.
    pub(super) name: i32,
    /// Block index of this shape's extra data, or `-1`.
    pub(super) extra: i32,
    /// Block index of `NiSkinInstance`, or `-1` for an unskinned shape.
    pub(super) skin: i32,
    /// Block index of `BSLightingShaderProperty`, or `-1`.
    pub(super) shader: i32,
}

/// Encode one `BSTriShape` block body.
///
/// A skinned shape leaves `vertices`, `triangles` and `data_size` at zero: its
/// packed stream lives in the linked `NiSkinPartition` and the reader
/// reconstructs from there, exactly as nifly writes and reads `IsSkinned()`
/// shapes. An unskinned shape keeps the inline stream.
pub(super) fn body(
    asset: &MeshAsset,
    prepared: &PreparedShape,
    refs: ShapeRefs,
    transform: NativeTransform,
) -> Vec<u8> {
    let mut body = Vec::new();
    push_i32(&mut body, refs.name);
    push_u32(&mut body, u32::from(refs.extra >= 0));
    if refs.extra >= 0 {
        push_i32(&mut body, refs.extra);
    }
    push_i32(&mut body, -1);
    // NiAVObject: flags, transform, no collision.
    push_u32(&mut body, 0);
    transform.write_av(&mut body);
    push_i32(&mut body, -1);
    // BSTriShape: bounding sphere, refs, vertex descriptor.
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        push_f32(&mut body, value);
    }
    push_i32(&mut body, refs.skin);
    push_i32(&mut body, refs.shader);
    push_i32(&mut body, -1); // alpha
    push_u64(&mut body, prepared.descriptor());
    let data_size = asset.positions.len() as u64 * u64::from(prepared.packed_size())
        + asset.triangles.len() as u64 * 6;
    let (triangles, vertices, data_size) = match prepared.skinned() {
        true => (0, 0, 0),
        false => (prepared.triangles(), prepared.vertices(), data_size as u32),
    };
    push_u16(&mut body, triangles);
    push_u16(&mut body, vertices);
    push_u32(&mut body, data_size);
    if !prepared.skinned() {
        body.extend_from_slice(prepared.vertex_bytes());
        for triangle in &asset.triangles {
            push_u16(&mut body, triangle[0] as u16);
            push_u16(&mut body, triangle[1] as u16);
            push_u16(&mut body, triangle[2] as u16);
        }
    }
    push_u32(&mut body, 0); // SSE particle_data_size
    body
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

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_core::mesh::{MaterialSlot, PartitionId, ShaderKind, Skin};
    use sculpto_core::skin::SkinWeights;

    fn triangle() -> MeshAsset {
        MeshAsset {
            name: "Body".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            ..MeshAsset::default()
        }
    }

    #[test]
    fn se_shape_roundtrips_positions() {
        let asset = triangle();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("our SE NIF parses");
        assert_eq!(document.shapes.len(), 1);
        let shape = &document.shapes[0];
        assert_eq!(shape.triangles, asset.triangles);
        for (actual, expected) in shape.positions.iter().zip(&asset.positions) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-6, "position drift");
            }
        }
    }

    /// A flat-shaded cube: 24 vertices, six per-face unit normals, per-face
    /// tangents with alternating bitangent signs.
    fn cube() -> (MeshAsset, Vec<[f32; 3]>, Vec<[f32; 4]>) {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut tangents = Vec::new();
        let mut triangles = Vec::new();
        let mut partitions = Vec::new();
        for face in 0..6_u32 {
            let axis = (face / 2) as usize;
            let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
            let mut normal = [0.0f32; 3];
            normal[axis] = sign;
            let mut u = [0.0f32; 3];
            u[(axis + 1) % 3] = 1.0;
            let mut v = [0.0f32; 3];
            v[(axis + 2) % 3] = 1.0;
            let base = positions.len() as u32;
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut position = [0.0f32; 3];
                position[axis] = sign;
                for k in 0..3 {
                    position[k] += su * u[k] + sv * v[k];
                }
                positions.push(position);
                normals.push(normal);
                tangents.push([u[0], u[1], u[2], sign]);
            }
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
            partitions.push(PartitionId(0));
            partitions.push(PartitionId(0));
        }
        let asset = MeshAsset {
            name: "Cube".into(),
            positions,
            normals: normals.clone(),
            tangents: tangents.clone(),
            triangles,
            triangle_partitions: partitions,
            ..MeshAsset::default()
        };
        (asset, normals, tangents)
    }

    #[test]
    fn flat_shaded_cube_roundtrips_unit_normals() {
        let (asset, expected, _) = cube();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.normals.len(), expected.len());
        for (actual, expected) in shape.normals.iter().zip(&expected) {
            let length =
                (actual[0] * actual[0] + actual[1] * actual[1] + actual[2] * actual[2]).sqrt();
            assert!((length - 1.0).abs() <= 0.02, "normal not unit: {length}");
            let dot = actual[0] * expected[0] + actual[1] * expected[1] + actual[2] * expected[2];
            assert!(dot > 0.999, "normal direction drifted: {dot}");
        }
    }

    #[test]
    fn se_tangents_roundtrip_with_their_sign() {
        let (asset, _, expected) = cube();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.tangents.len(), expected.len());
        for (actual, expected) in shape.tangents.iter().zip(&expected) {
            assert_eq!(actual[3].signum(), expected[3].signum(), "sign flipped");
            let dot = actual[0] * expected[0] + actual[1] * expected[1] + actual[2] * expected[2];
            assert!(dot > 0.99, "tangent direction drifted: {dot}");
        }
    }

    fn skinned() -> MeshAsset {
        let mut asset = triangle();
        asset.name = "Skinned".into();
        let mut bind = vec![skin::identity(), skin::identity()];
        bind[1][3] = [0.0, 0.0, 1.0, 1.0];
        asset.skin = Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: bind,
            influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 3], &[[0.75, 0.25, 0.0, 0.0]; 3])
                .expect("four-lane rows"),
        });
        asset
    }

    #[test]
    fn se_skin_roundtrips_names_weights_and_bind_matrices() {
        let asset = skinned();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.name, "Skinned");
        let authored = asset.skin.as_ref().expect("skin");
        let read = shape.skin.as_ref().expect("the shape carries one skin");
        assert_eq!(read.bones, authored.bones);
        for vertex in 0..authored.influences.rows() {
            let read_row = read.influences.row(vertex).expect("read row");
            let authored_row = authored.influences.row(vertex).expect("authored row");
            assert_eq!(read_row.joints, authored_row.joints);
            for (left, right) in read_row.weights.iter().zip(authored_row.weights) {
                assert!((left - right).abs() <= 1e-3, "weight drift");
            }
        }
        assert_eq!(read.bind_inverse.len(), 2);
        for (actual, expected) in read.bind_inverse.iter().zip(&authored.bind_inverse) {
            for column in 0..4 {
                for row in 0..4 {
                    assert!(
                        (actual[column][row] - expected[column][row]).abs() <= 1e-5,
                        "bind matrix drift at [{column}][{row}]"
                    );
                }
            }
        }
    }

    #[test]
    fn se_skin_partition_block_matches_the_model_palette() {
        let asset = skinned();
        let bytes = write_se_shape(&asset).expect("writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let partition = scene
            .get_as::<byroredux_nif::blocks::skin::NiSkinPartition>(3)
            .expect("partition block");
        assert_eq!(partition.partitions.len(), 1);
        assert_eq!(partition.partitions[0].bones, vec![0, 1]);
        assert_eq!(partition.partitions[0].num_weights_per_vertex, 4);
        assert_eq!(partition.partitions[0].vertex_map.len(), 3);
        // Palette-local bone indices: 0.75 -> palette slot 0, 0.25 -> slot 1.
        assert_eq!(&partition.partitions[0].bone_indices[..4], &[0, 1, 0, 0]);
        assert_eq!(partition.partitions[0].triangles, vec![[0, 1, 2]]);
        assert!(partition.global_vertex_data.is_some());
    }

    #[test]
    fn se_skinned_geometry_lives_in_the_partition_global_buffer() {
        let asset = skinned();
        let bytes = write_se_shape(&asset).expect("writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shape_block = scene
            .get_as::<byroredux_nif::blocks::tri_shape::BsTriShape>(0)
            .expect("shape block");
        assert_eq!(shape_block.data_size, 0);
        assert!(shape_block.vertices.is_empty());
        assert!(shape_block.triangles.is_empty());
        let partition = scene
            .get_as::<byroredux_nif::blocks::skin::NiSkinPartition>(3)
            .expect("partition block");
        let global = partition
            .global_vertex_data
            .as_ref()
            .expect("skinned SE geometry needs the global buffer");
        assert_eq!(global.vertex_size, 28, "16 position + 12 skin bytes");
        assert_eq!(global.raw_bytes.len(), 3 * 28);
        assert_eq!(global.vertex_desc & 0xF, 7);

        let document = crate::read(&bytes).expect("our reader reconstructs it");
        let shape = &document.shapes[0];
        assert_eq!(shape.positions.len(), asset.positions.len());
        for (vertex, (actual, expected)) in shape.positions.iter().zip(&asset.positions).enumerate()
        {
            for (lane, (left, right)) in actual.iter().zip(expected).enumerate() {
                assert!(
                    (left - right).abs() <= 1e-6,
                    "position[{vertex}][{lane}] drifted: {left} vs {right}"
                );
            }
        }
        assert_eq!(shape.triangles, asset.triangles);
        let read = shape.skin.as_ref().expect("the shape carries one skin");
        let authored = asset.skin.as_ref().expect("skin");
        for vertex in 0..authored.influences.rows() {
            let read_row = read.influences.row(vertex).expect("read row");
            let authored_row = authored.influences.row(vertex).expect("authored row");
            assert_eq!(read_row.joints, authored_row.joints);
            for (left, right) in read_row.weights.iter().zip(authored_row.weights) {
                assert!((left - right).abs() <= 1e-3, "weight drift");
            }
        }
    }

    fn textured() -> MeshAsset {
        let mut asset = triangle();
        asset.name = "Robe".into();
        asset.materials = vec![MaterialSlot {
            name: "robe".into(),
            shader: ShaderKind::Cloth,
            diffuse: Some("textures/robe_d.dds".into()),
            normal: Some("textures/robe_n.dds".into()),
            specular: None,
            double_sided: true,
        }];
        asset
    }

    #[test]
    fn se_texture_set_roundtrips_through_the_reader_blocks() {
        let asset = textured();
        let bytes = write_se_shape(&asset).expect("writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shader = scene
            .get_as::<byroredux_nif::blocks::shader::BSLightingShaderProperty>(1)
            .expect("shader block");
        assert_eq!(shader.shader_flags_2 & 0x10, 0x10, "double-sided bit");
        let texture_index = shader.texture_set_ref.index().expect("texture set ref");
        let textures = scene
            .get_as::<byroredux_nif::blocks::shader::BSShaderTextureSet>(texture_index)
            .expect("texture set block");
        assert_eq!(
            textures.textures,
            vec!["textures/robe_d.dds", "textures/robe_n.dds"]
        );
    }

    #[test]
    fn specular_textures_are_refused_until_the_slot_is_measured() {
        let mut asset = textured();
        asset.materials[0].specular = Some("textures/robe_s.dds".into());
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::TextureSlotUnsupported { slot: "specular" })
        ));
    }

    #[test]
    fn multiple_material_slots_are_refused() {
        let mut asset = textured();
        let mut second = asset.materials[0].clone();
        second.name = "robe2".into();
        asset.materials.push(second);
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::MultipleMaterialSlots { count: 2 })
        ));
    }

    #[test]
    fn unnamed_bones_are_refused() {
        let mut asset = skinned();
        asset.skin.as_mut().expect("skin").bones[1] = "  ".into();
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::UnnamedBone { index: 1 })
        ));
    }

    #[test]
    fn duplicate_bones_are_refused() {
        let mut asset = skinned();
        asset.skin.as_mut().expect("skin").bones[1] = "Root".into();
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::DuplicateBone { .. })
        ));
    }

    #[test]
    fn zero_normals_are_refused() {
        let mut asset = triangle();
        asset.normals = vec![[0.0; 3]; 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::ZeroVector {
                attribute: "normal",
                vertex: 0
            })
        ));
    }

    #[test]
    fn tangents_without_normals_are_refused() {
        let mut asset = triangle();
        asset.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::TangentsWithoutNormals)
        ));
    }

    #[test]
    fn se_uv_seam_roundtrips() {
        let mut asset = triangle();
        asset.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.uvs.len(), asset.uvs.len());
        for (actual, expected) in shape.uvs.iter().zip(&asset.uvs) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-3, "UV drift at the seam");
            }
        }
    }

    #[test]
    fn invalid_model_is_refused_before_bytes() {
        let mut asset = triangle();
        asset.triangles[0] = [0, 1, 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::InvalidModel(_))
        ));
    }
}
