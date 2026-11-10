//! Skyrim LE `NiTriShape` + `NiTriShapeData` writer for one shape.
//!
//! The LE wire format keeps geometry in a separate data block. C04 wrote
//! positions and triangles, C05 added UVs, C06 `f32` normals, C07 added the
//! skin: `NiSkinInstance` + sparse `NiSkinData` weights and one named `NiNode`
//! per bone, C10 the shader and texture set, and C11 the HDT-SMP physics link as
//! one `NiStringExtraData`. Tangents need the NBT layout, which this writer does
//! not emit yet, so they are refused instead of dropped.

use sculpto_core::mesh::MeshAsset;

use super::extra;
use super::header::{NifTarget, NifWriteError, write_header};
use super::shader;
use super::skin;
use super::strings::StringTable;
use super::{InfluencePolicy, WrittenNif};

/// Write a complete LE NIF containing one `NiTriShape`.
pub fn write_le_shape(asset: &MeshAsset) -> Result<Vec<u8>, NifWriteError> {
    write(asset, InfluencePolicy::Preserve).map(|output| output.bytes)
}

pub(super) fn write(
    asset: &MeshAsset,
    policy: InfluencePolicy,
) -> Result<WrittenNif, NifWriteError> {
    asset.validate().map_err(NifWriteError::InvalidModel)?;
    let vertices =
        u16::try_from(asset.positions.len()).map_err(|_| NifWriteError::TooManyVertices {
            vertices: asset.positions.len(),
        })?;
    let triangles =
        u16::try_from(asset.triangles.len()).map_err(|_| NifWriteError::TooManyTriangles {
            triangles: asset.triangles.len(),
        })?;

    let has_uvs = !asset.uvs.is_empty();
    let has_normals = !asset.normals.is_empty();
    if !asset.tangents.is_empty() {
        return Err(NifWriteError::TangentsUnsupported {
            target: NifTarget::Le,
        });
    }
    let skin_plan = skin::plan(asset, NifTarget::Le, policy)?;
    let mut strings = StringTable::new();
    let name_index = match asset.name.is_empty() {
        true => -1,
        false => strings.intern(&asset.name) as i32,
    };
    let bone_names: Vec<i32> = skin_plan
        .as_ref()
        .map(|plan| {
            plan.names
                .iter()
                .map(|name| strings.intern(name) as i32)
                .collect()
        })
        .unwrap_or_default();
    let bone_count = bone_names.len();
    // Block 0 is the shape, 1 its data; a skin adds the instance (2), the skin
    // data (3), the partition (4), a root node (5) that parents the shape, and
    // one node per bone (6..).
    let bone_refs: Vec<i32> = (0..bone_count).map(|offset| 6 + offset as i32).collect();
    let skeleton_root = bone_refs.first().copied().unwrap_or(-1);
    let skin_ref = skin_plan.as_ref().map_or(-1, |_| 2);
    let material = match asset.materials.len() {
        0 => None,
        1 => Some(&asset.materials[0]),
        count => return Err(NifWriteError::MultipleMaterialSlots { count }),
    };
    let texture_body = match material {
        Some(slot) => shader::texture_set(slot)?,
        None => Vec::new(),
    };
    let mut next_block = if skin_plan.is_some() {
        6 + bone_count as i32
    } else {
        2
    };
    let shader_index = material.map_or(-1, |_| {
        let index = next_block;
        next_block += 1;
        index
    });
    let texture_index = match material.is_some() && !texture_body.is_empty() {
        true => {
            let index = next_block;
            next_block += 1;
            index
        }
        false => -1,
    };
    let physics = extra::physics_link(asset)?;
    let extra_index = physics.as_ref().map_or(-1, |_| {
        let index = next_block;
        next_block += 1;
        index
    });
    let shader_body = material.map_or_else(Vec::new, |slot| {
        shader::shader(slot, texture_index, name_index)
    });
    let extra_body = physics
        .as_ref()
        .map_or_else(Vec::new, |link| extra::body(link, &mut strings));

    let mut shape = Vec::new();
    // NiObjectNET: name, extra data, no controller. With a skin the file has a
    // root `NiNode`, and the game only reads the physics link from there
    // (`OutfitProject.cpp:250-252`); without one the shape is the root.
    let shape_extra = match skin_plan.is_some() {
        true => -1,
        false => extra_index,
    };
    push_i32(&mut shape, name_index);
    push_u32(&mut shape, u32::from(shape_extra >= 0));
    if shape_extra >= 0 {
        push_i32(&mut shape, shape_extra);
    }
    push_i32(&mut shape, -1); // controller
    push_u32(&mut shape, 0); // flags
    for value in [0.0f32, 0.0, 0.0] {
        push_f32(&mut shape, value);
    }
    for row in 0..3 {
        for column in 0..3 {
            push_f32(&mut shape, if row == column { 1.0 } else { 0.0 });
        }
    }
    push_f32(&mut shape, 1.0);
    push_i32(&mut shape, -1); // collision
    push_i32(&mut shape, 1); // data block
    push_i32(&mut shape, skin_ref);
    push_u32(&mut shape, 0); // num_materials
    push_u32(&mut shape, 0); // active_material_index
    shape.push(0); // dirty flag
    push_i32(&mut shape, shader_index);
    push_i32(&mut shape, -1); // alpha

    let mut data = Vec::new();
    push_i32(&mut data, 0); // group id
    push_u16(&mut data, vertices);
    data.push(0); // keep flags
    data.push(0); // compress flags
    data.push(1); // has vertices
    for position in &asset.positions {
        push_f32(&mut data, position[0]);
        push_f32(&mut data, position[1]);
        push_f32(&mut data, position[2]);
    }
    push_u16(&mut data, u16::from(has_uvs)); // data flags: bit 0 = has UV
    push_u32(&mut data, 0); // material CRC
    data.push(u8::from(has_normals));
    if has_normals {
        for normal in &asset.normals {
            push_f32(&mut data, normal[0]);
            push_f32(&mut data, normal[1]);
            push_f32(&mut data, normal[2]);
        }
    }
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        push_f32(&mut data, value); // center + radius
    }
    data.push(0); // has vertex colors
    if has_uvs {
        for uv in &asset.uvs {
            push_f32(&mut data, uv[0]);
            push_f32(&mut data, uv[1]);
        }
    }
    push_u16(&mut data, 0); // consistency flags
    push_i32(&mut data, -1); // additional data
    push_u16(&mut data, triangles);
    push_u32(&mut data, u32::from(triangles) * 3);
    data.push(1); // has triangles
    for triangle in &asset.triangles {
        push_u16(&mut data, triangle[0] as u16);
        push_u16(&mut data, triangle[1] as u16);
        push_u16(&mut data, triangle[2] as u16);
    }
    push_u16(&mut data, 0); // match groups

    let root_extra: Vec<i32> = match extra_index {
        -1 => Vec::new(),
        index => vec![index],
    };
    let (instance, skin_data, partition, root, nodes) = match &skin_plan {
        Some(plan) => (
            skin::instance(3, 4, skeleton_root, &bone_refs),
            skin::data(plan)?,
            skin::partitions(asset, plan, None)?,
            skin::node(-1, &[0], &root_extra),
            bone_names
                .iter()
                .map(|index| skin::node(*index, &[], &[]))
                .collect::<Vec<_>>(),
        ),
        None => (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
    };

    let mut type_names: Vec<&str> = vec!["NiTriShape", "NiTriShapeData"];
    let mut type_indices: Vec<u16> = vec![0, 1];
    if skin_plan.is_some() {
        type_names.extend(["NiSkinInstance", "NiSkinData", "NiSkinPartition", "NiNode"]);
        type_indices.extend([2, 3, 4]);
        type_indices.extend(std::iter::repeat_n(5, bone_count + 1));
    }
    if material.is_some() {
        type_names.push("BSLightingShaderProperty");
        type_indices.push(type_names.len() as u16 - 1);
        if texture_index >= 0 {
            type_names.push("BSShaderTextureSet");
            type_indices.push(type_names.len() as u16 - 1);
        }
    }
    if physics.is_some() {
        type_names.push(extra::BLOCK_TYPE);
        type_indices.push(type_names.len() as u16 - 1);
    }

    let mut file = write_header(NifTarget::Le);
    push_u32(&mut file, type_indices.len() as u32);
    push_u32(&mut file, 83); // LE bsver
    file.push(0); // author
    file.push(0); // process script
    file.push(0); // export script
    push_u16(&mut file, type_names.len() as u16);
    for name in &type_names {
        push_u32(&mut file, name.len() as u32);
        file.extend_from_slice(name.as_bytes());
    }
    for index in &type_indices {
        push_u16(&mut file, *index);
    }
    push_u32(&mut file, shape.len() as u32);
    push_u32(&mut file, data.len() as u32);
    if skin_plan.is_some() {
        push_u32(&mut file, instance.len() as u32);
        push_u32(&mut file, skin_data.len() as u32);
        push_u32(&mut file, partition.len() as u32);
        push_u32(&mut file, root.len() as u32);
        for node in &nodes {
            push_u32(&mut file, node.len() as u32);
        }
    }
    if material.is_some() {
        push_u32(&mut file, shader_body.len() as u32);
        if texture_index >= 0 {
            push_u32(&mut file, texture_body.len() as u32);
        }
    }
    if physics.is_some() {
        push_u32(&mut file, extra_body.len() as u32);
    }
    let max_length = strings.iter().map(str::len).max().unwrap_or(0);
    push_u32(&mut file, strings.len() as u32);
    push_u32(&mut file, max_length as u32);
    for value in strings.iter() {
        push_u32(&mut file, value.len() as u32);
        file.extend_from_slice(value.as_bytes());
    }
    push_u32(&mut file, 0); // num_groups
    file.extend_from_slice(&shape);
    file.extend_from_slice(&data);
    if skin_plan.is_some() {
        file.extend_from_slice(&instance);
        file.extend_from_slice(&skin_data);
        file.extend_from_slice(&partition);
        file.extend_from_slice(&root);
        for node in &nodes {
            file.extend_from_slice(node);
        }
    }
    if material.is_some() {
        file.extend_from_slice(&shader_body);
        if texture_index >= 0 {
            file.extend_from_slice(&texture_body);
        }
    }
    if physics.is_some() {
        file.extend_from_slice(&extra_body);
    }
    Ok(WrittenNif {
        bytes: file,
        dropped_influences: skin_plan.map_or_else(Vec::new, |plan| plan.dropped_influences),
    })
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
    fn le_shape_roundtrips_positions() {
        let asset = triangle();
        let bytes = write_le_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("our LE NIF parses");
        assert_eq!(document.shapes.len(), 1);
        let shape = &document.shapes[0];
        assert_eq!(shape.triangles, asset.triangles);
        for (actual, expected) in shape.positions.iter().zip(&asset.positions) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-6, "position drift");
            }
        }
    }

    #[test]
    fn le_normals_roundtrip() {
        let mut asset = triangle();
        asset.normals = vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        let bytes = write_le_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.normals.len(), asset.normals.len());
        for (actual, expected) in shape.normals.iter().zip(&asset.normals) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-6, "normal drift");
            }
        }
    }

    #[test]
    fn le_tangents_are_refused() {
        let mut asset = triangle();
        asset.normals = vec![[0.0, 0.0, 1.0]; 3];
        asset.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
        assert!(matches!(
            write_le_shape(&asset),
            Err(NifWriteError::TangentsUnsupported {
                target: NifTarget::Le
            })
        ));
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
    fn le_texture_set_roundtrips_through_the_reader_blocks() {
        let mut asset = triangle();
        asset.name = "Robe".into();
        asset.materials = vec![MaterialSlot {
            name: "robe".into(),
            shader: ShaderKind::Cloth,
            diffuse: Some("textures/robe_d.dds".into()),
            normal: None,
            specular: None,
            double_sided: false,
        }];
        let bytes = write_le_shape(&asset).expect("writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shader = scene
            .get_as::<byroredux_nif::blocks::shader::BSLightingShaderProperty>(2)
            .expect("shader block");
        let texture_index = shader.texture_set_ref.index().expect("texture set ref");
        let textures = scene
            .get_as::<byroredux_nif::blocks::shader::BSShaderTextureSet>(texture_index)
            .expect("texture set block");
        assert_eq!(textures.textures, vec!["textures/robe_d.dds"]);
    }

    #[test]
    fn le_skin_roundtrips_names_weights_and_bind_matrices() {
        let asset = skinned();
        let bytes = write_le_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.name, "Skinned");
        let authored = asset.skin.as_ref().expect("skin");
        let read = shape.skin.as_ref().expect("the shape carries one skin");
        assert_eq!(read.bones, authored.bones);
        for vertex in 0..shape.positions.len() {
            let mut weights: std::collections::BTreeMap<&str, f32> =
                std::collections::BTreeMap::new();
            let row = read.influences.row(vertex).expect("read row");
            for (joint, weight) in row.joints.iter().zip(row.weights) {
                *weights
                    .entry(read.bones[*joint as usize].as_str())
                    .or_default() += weight;
            }
            assert!((weights["Root"] - 0.75).abs() <= 1e-5, "root weight");
            assert!((weights["Spine"] - 0.25).abs() <= 1e-5, "spine weight");
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
    fn le_uv_seam_roundtrips() {
        let mut asset = triangle();
        asset.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let bytes = write_le_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.uvs.len(), asset.uvs.len());
        for (actual, expected) in shape.uvs.iter().zip(&asset.uvs) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-6, "UV drift at the seam");
            }
        }
    }

    #[test]
    fn invalid_model_is_refused_before_bytes() {
        let mut asset = triangle();
        asset.triangles[0] = [0, 1, 3];
        assert!(matches!(
            write_le_shape(&asset),
            Err(NifWriteError::InvalidModel(_))
        ));
    }
}
