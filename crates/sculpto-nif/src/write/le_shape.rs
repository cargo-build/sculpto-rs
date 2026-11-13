//! Skyrim LE `NiTriShape` + `NiTriShapeData` writer for one shape.
//!
//! The LE wire format keeps geometry in a separate data block. C04 wrote
//! positions and triangles, C05 added UVs, C06 `f32` normals, C07 added the
//! skin: `NiSkinInstance` + sparse `NiSkinData` weights and one named `NiNode`
//! per bone, C10 the shader and texture set, and C11 the HDT-SMP physics link as
//! one `NiStringExtraData`. Tangents need the NBT layout, which this writer does
//! not emit yet, so they are refused instead of dropped.
//!
//! The per-shape facts live in [`super::prepared::PreparedShape`] and the
//! container tail in [`super::layout`], both shared with the SE writer;
//! [`shape_body`] and [`data_body`] are the only LE-specific encoders here.
//! [`super::le_bundle`] reuses them for a multi-shape file, so a single shape and
//! a bundle cannot disagree about either block.

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

/// Block type table name for the shape.
pub(super) const SHAPE_TYPE: &str = "NiTriShape";
/// Block type table name for the shape's geometry.
pub(super) const DATA_TYPE: &str = "NiTriShapeData";

/// Write a complete LE NIF containing one `NiTriShape`.
pub fn write_le_shape(asset: &MeshAsset) -> Result<Vec<u8>, NifWriteError> {
    write(asset, InfluencePolicy::Preserve, None).map(|output| output.bytes)
}

pub(super) fn write(
    asset: &MeshAsset,
    policy: InfluencePolicy,
    checked: Option<&CheckedScene<'_>>,
) -> Result<WrittenNif, NifWriteError> {
    let prepared = PreparedShape::new(asset, policy, NifTarget::Le)?;
    let skinned = prepared.skinned();

    let mut strings = StringTable::new();
    let name_index = match asset.name.is_empty() {
        true => -1,
        false => strings.intern(&asset.name) as i32,
    };
    let skin_nodes = prepared
        .skin
        .as_ref()
        .map(|plan| SkinNodes::plan(checked, &plan.names, 5, &mut strings))
        .transpose()?;
    let node_count = skin_nodes.as_ref().map_or(0, SkinNodes::len);
    // Blocks 0 and 1 are the shape and its data; a skin group follows at 2.
    let mut next_block = match skinned {
        true => 5 + node_count as i32,
        false => 2,
    };
    let shader_index = match prepared.has_material() {
        true => take(&mut next_block),
        false => -1,
    };
    let texture_index = match shader_index >= 0 && !prepared.texture.is_empty() {
        true => take(&mut next_block),
        false => -1,
    };
    let extra_index = match prepared.physics.is_some() {
        true => next_block,
        false => -1,
    };

    // NiObjectNET: name, extra data, no controller. With a skin the file has a
    // root `NiNode`, and the game only reads the physics link from there
    // (`OutfitProject.cpp:250-252`); without one the shape is the root.
    let shape_extra = match skinned {
        true => -1,
        false => extra_index,
    };
    let mut blocks = vec![
        Block {
            type_name: SHAPE_TYPE,
            body: shape_body(
                ShapeRefs {
                    name: name_index,
                    extra: shape_extra,
                    data: 1,
                    skin: if skinned { 2 } else { -1 },
                    shader: shader_index,
                },
                checked.map_or(NativeTransform::IDENTITY, |scene| scene.placement),
            ),
        },
        Block {
            type_name: DATA_TYPE,
            body: data_body(asset, &prepared),
        },
    ];
    let root_extra: Vec<i32> = match extra_index {
        -1 => Vec::new(),
        index => vec![index],
    };
    if let Some((plan, nodes)) = prepared.skin.as_ref().zip(skin_nodes) {
        blocks.push(Block {
            type_name: skin::INSTANCE_TYPE,
            body: skin::instance(3, 4, nodes.root, &nodes.bones),
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
            body: skin::partitions(asset, plan, None)?,
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
        bytes: write_file(NifTarget::Le, &blocks, &strings),
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
    /// Block index of the paired `NiTriShapeData`, or `-1`.
    pub(super) data: i32,
    /// Block index of `NiSkinInstance`, or `-1` for an unskinned shape.
    pub(super) skin: i32,
    /// Block index of `BSLightingShaderProperty`, or `-1`.
    pub(super) shader: i32,
}

/// Encode one `NiTriShape` block body.
///
/// LE keeps the mesh on its own data block, so the shape carries references and
/// no geometry: name, optional extra data, the paired data block, the optional
/// skin instance and shader property.
pub(super) fn shape_body(refs: ShapeRefs, transform: NativeTransform) -> Vec<u8> {
    let mut shape = Vec::new();
    push_i32(&mut shape, refs.name);
    push_u32(&mut shape, u32::from(refs.extra >= 0));
    if refs.extra >= 0 {
        push_i32(&mut shape, refs.extra);
    }
    push_i32(&mut shape, -1); // controller
    push_u32(&mut shape, 0); // flags
    transform.write_av(&mut shape);
    push_i32(&mut shape, -1); // collision
    push_i32(&mut shape, refs.data);
    push_i32(&mut shape, refs.skin);
    push_u32(&mut shape, 0); // num_materials
    push_u32(&mut shape, 0); // active_material_index
    shape.push(0); // dirty flag
    push_i32(&mut shape, refs.shader);
    push_i32(&mut shape, -1); // alpha
    shape
}

/// Encode one `NiTriShapeData` block body.
///
/// Field order is the LE geometry layout the reader expects: counts, flags,
/// positions, UV flag and material CRC, normals, bounds, colours, UV sets,
/// consistency, then the triangle list.
pub(super) fn data_body(asset: &MeshAsset, prepared: &PreparedShape) -> Vec<u8> {
    let mut data = Vec::new();
    push_i32(&mut data, 0); // group id
    push_u16(&mut data, prepared.vertices());
    data.push(0); // keep flags
    data.push(0); // compress flags
    data.push(1); // has vertices
    for position in &asset.positions {
        push_f32(&mut data, position[0]);
        push_f32(&mut data, position[1]);
        push_f32(&mut data, position[2]);
    }
    push_u16(&mut data, u16::from(prepared.has_uvs())); // data flags: bit 0 = has UV
    push_u32(&mut data, 0); // material CRC
    data.push(u8::from(prepared.has_normals()));
    if prepared.has_normals() {
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
    if prepared.has_uvs() {
        for uv in &asset.uvs {
            push_f32(&mut data, uv[0]);
            push_f32(&mut data, uv[1]);
        }
    }
    push_u16(&mut data, 0); // consistency flags
    push_i32(&mut data, -1); // additional data
    let triangles = prepared.triangles();
    push_u16(&mut data, triangles);
    push_u32(&mut data, u32::from(triangles) * 3);
    data.push(1); // has triangles
    for triangle in &asset.triangles {
        push_u16(&mut data, triangle[0] as u16);
        push_u16(&mut data, triangle[1] as u16);
        push_u16(&mut data, triangle[2] as u16);
    }
    push_u16(&mut data, 0); // match groups
    data
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
