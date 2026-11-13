//! Skyrim SE multi-shape writer: one file, many `BSTriShape` blocks.
//!
//! A bundle is gated by [`validate_bundle`] first (at least one shape, unique
//! names, valid partition palettes, one shared bone list), then the file gets a
//! single string table, block type table and block-size array covering every
//! block, one root `NiNode` whose children are all the shape blocks, one
//! `NiNode` per bone of the shapes' shared palette, and per shape its own
//! `NiSkinInstance`/`NiSkinData`/`NiSkinPartition` — carrying that shape's SSE
//! packed-vertex buffer — plus its own shader property and texture set.
//!
//! Block order is not part of the format, but it is part of what the readers
//! assume: the scene root is the first `NiNode` block
//! (`vendor/byroredux/nif/src/lib.rs:869`), so the root is written before the
//! bone nodes. Each shape keeps the single-shape order inside its own group, so
//! `skin_ref`, the partition triangle space and every size stay exactly what
//! [`super::se_shape`] already emits for one shape.
//!
//! `MeshAsset::transform` is a bundle-local placement this writer does not
//! serialize yet: a skinned shape's node placement and its skinning frame have
//! to be decided together, so a non-identity placement is refused by name
//! instead of being dropped or written inconsistently.

use sculpto_core::mesh::{MeshAsset, Transform, validate_bundle};

use super::extra;
use super::header::{NifTarget, NifWriteError, write_header};
use super::scene::NativeTransform;
use super::se_shape::{self, PreparedShape};
use super::shader;
use super::skin;
use super::strings::StringTable;
use super::{InfluencePolicy, WrittenNif};

/// One ordered block body with its type name.
pub(super) struct Block {
    /// Block type table name, e.g. `BSTriShape`.
    pub(super) type_name: &'static str,
    /// Serialized block body.
    pub(super) body: Vec<u8>,
}

/// Write one SE NIF carrying every shape of a bundle.
///
/// The shapes must share one skeleton: [`validate_bundle`] refuses differing
/// bone lists and duplicate names. Each shape keeps its own geometry, skin
/// partition and shader; the emitted file has one root node, so a reader opens
/// it as one scene instead of one file per body part.
///
/// # Errors
/// [`NifWriteError::InvalidBundle`] names a bundle defect,
/// [`NifWriteError::TransformUnsupported`] a placement this writer cannot
/// serialize yet, and the per-shape refusals of [`super::se_shape`] apply
/// unchanged ([`NifWriteError::InvalidModel`], too many vertices, an
/// unrepresentable influence, an unsupported texture slot, ...).
pub fn write_se_bundle(shapes: &[MeshAsset]) -> Result<Vec<u8>, NifWriteError> {
    write(shapes, InfluencePolicy::Preserve).map(|output| output.bytes)
}

fn write(shapes: &[MeshAsset], policy: InfluencePolicy) -> Result<WrittenNif, NifWriteError> {
    validate_bundle(shapes).map_err(NifWriteError::InvalidBundle)?;
    for (index, shape) in shapes.iter().enumerate() {
        if shape
            .transform
            .is_some_and(|transform| transform != Transform::default())
        {
            return Err(NifWriteError::TransformUnsupported { shape: index });
        }
    }
    let prepared = shapes
        .iter()
        .map(|shape| PreparedShape::new(shape, policy))
        .collect::<Result<Vec<_>, _>>()?;
    let physics = single_physics_link(&prepared)?;

    // Block indices are assigned in write order: each shape's geometry, skin
    // group and shader pair, then the root, the bone union and the physics link.
    let mut next = 0i32;
    let mut layout = Vec::with_capacity(prepared.len());
    for shape in &prepared {
        let geometry = next;
        next += 1;
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
    let mut bones: Vec<BoneNode> = Vec::new();
    for shape in &prepared {
        let Some(plan) = shape.skin.as_ref() else {
            continue;
        };
        for name in &plan.names {
            if bones.iter().any(|bone| &bone.name == name) {
                continue;
            }
            let block = next;
            next += 1;
            bones.push(BoneNode {
                name: name.clone(),
                name_index: strings.intern(name) as i32,
                block,
            });
        }
    }
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
            type_name: se_shape::BLOCK_TYPE,
            body: se_shape::body(
                shape,
                prepared,
                se_shape::ShapeRefs {
                    name: *name,
                    // The physics link belongs to the file root: the game reads
                    // it only from there (`OutfitProject.cpp:250-252`).
                    extra: -1,
                    skin: skin_ref,
                    shader: index.shader,
                },
                NativeTransform::IDENTITY,
            ),
        });
        if let (Some(plan), Some((_, data, partition))) = (prepared.skin.as_ref(), index.skin) {
            let palette: Vec<i32> = plan
                .names
                .iter()
                .map(|bone| bone_block(&bones, bone))
                .collect();
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
                body: skin::partitions(shape, plan, Some(&prepared.global()))?,
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
    for bone in &bones {
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
        bytes: write_file(&blocks, &strings),
        dropped_influences: prepared
            .iter()
            .filter_map(|shape| shape.skin.as_ref())
            .flat_map(|plan| plan.dropped_influences.iter().cloned())
            .collect(),
    })
}

/// Encode the SE container: header, block type table, block sizes, strings.
///
/// Both SE writers end here, so a file cannot disagree with itself about the
/// block count, the type table order or the string table it describes.
pub(super) fn write_file(blocks: &[Block], strings: &StringTable) -> Vec<u8> {
    let mut type_names: Vec<&'static str> = Vec::new();
    let mut type_indices: Vec<u16> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let type_index = match type_names.iter().position(|name| *name == block.type_name) {
            Some(index) => index,
            None => {
                type_names.push(block.type_name);
                type_names.len() - 1
            }
        };
        type_indices.push(type_index as u16);
    }

    let mut file = write_header(NifTarget::Se);
    // Block count precedes BSStreamHeader in the parsed layout.
    push_u32(&mut file, blocks.len() as u32);
    // BSStreamHeader: SE bsver, empty author/process/export strings.
    push_u32(&mut file, 100);
    file.push(0); // author
    file.push(0); // process script
    file.push(0); // export script
    // Block type table.
    push_u16(&mut file, type_names.len() as u16);
    for name in &type_names {
        push_u32(&mut file, name.len() as u32);
        file.extend_from_slice(name.as_bytes());
    }
    for index in &type_indices {
        push_u16(&mut file, *index);
    }
    for block in blocks {
        push_u32(&mut file, block.body.len() as u32);
    }
    // String table: index 0 is empty, then the shape, bone and physics names.
    let max_length = strings.iter().map(str::len).max().unwrap_or(0);
    push_u32(&mut file, strings.len() as u32);
    push_u32(&mut file, max_length as u32);
    for value in strings.iter() {
        push_u32(&mut file, value.len() as u32);
        file.extend_from_slice(value.as_bytes());
    }
    push_u32(&mut file, 0); // num_groups
    for block in blocks {
        file.extend_from_slice(&block.body);
    }
    file
}

/// One bone `NiNode` in the file's shared union.
struct BoneNode {
    /// Bone name as authored.
    name: String,
    /// String-table index of that name.
    name_index: i32,
    /// Block index of the bone node.
    block: i32,
}

/// Block indices of one shape's blocks, in write order.
struct ShapeBlocks {
    /// The `BSTriShape` block.
    geometry: i32,
    /// `NiSkinInstance`, `NiSkinData`, `NiSkinPartition`, or `None`.
    skin: Option<(i32, i32, i32)>,
    /// `BSLightingShaderProperty`, or `-1`.
    shader: i32,
    /// `BSShaderTextureSet`, or `-1`.
    texture: i32,
}

/// The block of one union bone by name.
fn bone_block(bones: &[BoneNode], name: &str) -> i32 {
    bones
        .iter()
        .find(|bone| bone.name == name)
        .map_or(-1, |bone| bone.block)
}

/// The one root physics link a bundle may carry.
///
/// HDT-SMP scans the model root and the reference keeps one link per model
/// (`extra.rs`), so shapes naming different files cannot share this file; the
/// count is reported instead of picking one at random.
fn single_physics_link(
    prepared: &[PreparedShape],
) -> Result<Option<&extra::PhysicsLink>, NifWriteError> {
    let mut links: Vec<&extra::PhysicsLink> = Vec::new();
    for link in prepared.iter().filter_map(|shape| shape.physics.as_ref()) {
        if !links.iter().any(|known| known.path == link.path) {
            links.push(link);
        }
    }
    match links.as_slice() {
        [] => Ok(None),
        [link] => Ok(Some(*link)),
        many => Err(NifWriteError::MultiplePhysicsLinks { links: many.len() }),
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
