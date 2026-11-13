//! Skyrim LE multi-shape writer: one file, many `NiTriShape`/`NiTriShapeData`
//! pairs.
//!
//! A bundle is gated by [`validate_bundle`] first (at least one shape, unique
//! names, valid partition palettes, one shared bone list), then the file gets a
//! single string table, block type table and block-size array covering every
//! block, one root `NiNode` whose children are all the shape blocks, one
//! `NiNode` per bone of the shapes' shared palette, and per shape its paired data
//! block plus its own `NiSkinInstance`/`NiSkinData`/`NiSkinPartition` and its own
//! shader property and texture set.
//!
//! Block order is not part of the format, but it is part of what the readers
//! assume: the scene root is the first `NiNode` block
//! (`vendor/byroredux/nif/src/lib.rs:869`), so the root is written before the
//! bone nodes. Each shape keeps the single-shape order inside its own group, so
//! `data_ref`, `skin_ref` and every size stay exactly what [`super::le_shape`]
//! already emits for one shape.
//!
//! The per-shape facts come from [`PreparedShape`] and the container tail from
//! [`super::layout`], both shared with the SE bundle writer.
//!
//! `MeshAsset::transform` is a bundle-local placement this writer does not
//! serialize yet: a skinned shape's node placement and its skinning frame have
//! to be decided together, so a non-identity placement is refused by name
//! instead of being dropped or written inconsistently.

use sculpto_core::mesh::{MeshAsset, validate_bundle};

use super::extra;
use super::header::{NifTarget, NifWriteError};
use super::layout::{Block, BoneUnion, write_file};
use super::le_shape::{self, ShapeRefs};
use super::prepared::{PreparedShape, refuse_placements, single_physics_link};
use super::scene::NativeTransform;
use super::shader;
use super::skin;
use super::strings::StringTable;
use super::{InfluencePolicy, WrittenNif};

/// Write one LE NIF carrying every shape of a bundle.
///
/// The shapes must share one skeleton: [`validate_bundle`] refuses differing
/// bone lists and duplicate names. Each shape keeps its own geometry data
/// block, skin partition and shader; the emitted file has one root node, so a
/// reader opens it as one scene instead of one file per body part.
///
/// # Errors
/// [`NifWriteError::InvalidBundle`] names a bundle defect,
/// [`NifWriteError::TransformUnsupported`] a placement this writer cannot
/// serialize yet, and the per-shape refusals of [`super::le_shape`] apply
/// unchanged ([`NifWriteError::InvalidModel`], [`NifWriteError::TangentsUnsupported`],
/// too many vertices, an unrepresentable influence, an unsupported texture
/// slot, ...).
pub fn write_le_bundle(shapes: &[MeshAsset]) -> Result<Vec<u8>, NifWriteError> {
    write(shapes, InfluencePolicy::Preserve).map(|output| output.bytes)
}

fn write(shapes: &[MeshAsset], policy: InfluencePolicy) -> Result<WrittenNif, NifWriteError> {
    validate_bundle(shapes).map_err(NifWriteError::InvalidBundle)?;
    refuse_placements(shapes)?;
    let prepared = shapes
        .iter()
        .map(|shape| PreparedShape::new(shape, policy, NifTarget::Le))
        .collect::<Result<Vec<_>, _>>()?;
    let physics = single_physics_link(&prepared)?;

    // Block indices are assigned in write order: each shape's shape/data pair,
    // its skin group and shader pair, then the root, the bone union and the
    // physics link.
    let mut next = 0i32;
    let mut layout = Vec::with_capacity(prepared.len());
    for shape in &prepared {
        let geometry = next;
        next += 2; // NiTriShape, NiTriShapeData
        let skin_group = match shape.skinned() {
            true => {
                let instance = next;
                next += 3;
                Some((instance, instance + 1, instance + 2))
            }
            false => None,
        };
        let shader_block = match shape.has_material() {
            true => {
                let index = next;
                next += 1;
                index
            }
            false => -1,
        };
        let texture = match shader_block >= 0 && !shape.texture.is_empty() {
            true => {
                let index = next;
                next += 1;
                index
            }
            false => -1,
        };
        layout.push(ShapeBlocks {
            geometry,
            data: geometry + 1,
            skin: skin_group,
            shader: shader_block,
            texture,
        });
    }
    let root = next;
    next += 1;

    // One string table for the file: shapes, then the bone union, then physics.
    let mut strings = StringTable::new();
    let names: Vec<i32> = shapes
        .iter()
        .map(|shape| match shape.name.is_empty() {
            true => -1,
            false => strings.intern(&shape.name) as i32,
        })
        .collect();
    let bones = BoneUnion::plan(&prepared, &mut next, &mut strings);
    let extra_block = match physics.is_some() {
        true => next,
        false => -1,
    };

    let root_children: Vec<i32> = layout.iter().map(|blocks| blocks.geometry).collect();
    let root_extra: Vec<i32> = match extra_block {
        -1 => Vec::new(),
        index => vec![index],
    };
    let mut blocks: Vec<Block> = Vec::new();
    for ((shape, prepared), (index, name)) in
        shapes.iter().zip(&prepared).zip(layout.iter().zip(&names))
    {
        let skin_ref = match index.skin {
            Some((instance, _, _)) => instance,
            None => -1,
        };
        blocks.push(Block {
            type_name: le_shape::SHAPE_TYPE,
            body: le_shape::shape_body(
                ShapeRefs {
                    name: *name,
                    // The physics link belongs to the file root: the game reads
                    // it only from there (`OutfitProject.cpp:250-252`).
                    extra: -1,
                    data: index.data,
                    skin: skin_ref,
                    shader: index.shader,
                },
                NativeTransform::IDENTITY,
            ),
        });
        blocks.push(Block {
            type_name: le_shape::DATA_TYPE,
            body: le_shape::data_body(shape, prepared),
        });
        if let (Some(plan), Some((_, data, partition))) = (prepared.skin.as_ref(), index.skin) {
            let palette: Vec<i32> = plan.names.iter().map(|bone| bones.block(bone)).collect();
            blocks.push(Block {
                type_name: skin::INSTANCE_TYPE,
                body: skin::instance(data, partition, root, &palette),
            });
            blocks.push(Block {
                type_name: skin::DATA_TYPE,
                body: skin::data(plan, &skin::identity())?,
            });
            blocks.push(Block {
                type_name: skin::PARTITION_TYPE,
                body: skin::partitions(shape, plan, None)?,
            });
        }
        if index.shader >= 0 {
            blocks.push(Block {
                type_name: shader::SHADER_TYPE,
                body: prepared.shader_body(*name, index.texture),
            });
            if index.texture >= 0 {
                blocks.push(Block {
                    type_name: shader::TEXTURE_TYPE,
                    body: prepared.texture.clone(),
                });
            }
        }
    }
    blocks.push(Block {
        type_name: skin::NODE_TYPE,
        body: skin::node(-1, &root_children, &root_extra, NativeTransform::IDENTITY),
    });
    for bone in bones.bones() {
        blocks.push(Block {
            type_name: skin::NODE_TYPE,
            body: skin::node(bone.name_index, &[], &[], NativeTransform::IDENTITY),
        });
    }
    if let Some(link) = physics {
        blocks.push(Block {
            type_name: extra::BLOCK_TYPE,
            body: extra::body(link, &mut strings),
        });
    }
    Ok(WrittenNif {
        bytes: write_file(NifTarget::Le, &blocks, &strings),
        dropped_influences: prepared
            .iter()
            .filter_map(|shape| shape.skin.as_ref())
            .flat_map(|plan| plan.dropped_influences.iter().cloned())
            .collect(),
    })
}

/// Block indices of one shape's blocks, in write order.
struct ShapeBlocks {
    /// The `NiTriShape` block.
    geometry: i32,
    /// The paired `NiTriShapeData` block.
    data: i32,
    /// `NiSkinInstance`, `NiSkinData`, `NiSkinPartition`, or `None`.
    skin: Option<(i32, i32, i32)>,
    /// `BSLightingShaderProperty`, or `-1`.
    shader: i32,
    /// `BSShaderTextureSet`, or `-1`.
    texture: i32,
}
