//! Authored source facts read from the file's own blocks, before projection.
//!
//! The vendored importer renormalizes an inline SE vertex stream
//! (`renormalize_skin_weights`, `blocks/tri_shape/mod.rs:83-96`) and truncates
//! plus renormalizes LE sparse data to four lanes
//! (`import/mesh/skin.rs:828-905`). Neither value is the authored one, so the
//! weight channels are read here:
//!
//! - LE: every `NiSkinData` sparse `(vertex, weight)` entry, all influences.
//! - SE: the packed vertex lanes, either the retained
//!   `NiSkinPartition.global_vertex_data.raw_bytes` when the shape carries no
//!   inline stream, or the inline run located by re-reading the shape's block
//!   prefix from the original bytes with the public header/stream/AV-object API.
//!
//! The importer also projects the source's partition table and shader onto
//! renderer values. The export projection needs the authored ones instead, so
//! this module captures them from the same resolved block: the
//! `NiSkinPartition` entry order with its per-triangle attribution and the
//! `BSDismemberSkinInstance` body-part tags, plus the
//! `BSLightingShaderProperty`'s texture-set slot list.
//!
//! A shape that cannot be matched unambiguously, or whose required raw payload
//! is missing or malformed, is a named refusal. Nothing is normalized, dropped
//! or rebound here. @req DECL0008

use std::ops::Range;

use byroredux_nif::blocks::base::NiAVObjectData;
use byroredux_nif::blocks::node::NiNode;
use byroredux_nif::blocks::properties::NiAlphaProperty;
use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet, ShaderTypeData};
use byroredux_nif::blocks::skin::{
    BsDismemberSkinInstance, NiSkinData, NiSkinInstance, NiSkinPartition,
};
use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape, NiTriShapeData};
use byroredux_nif::header::NifHeader;
use byroredux_nif::import::{
    ImportedMesh, TextureRole, TextureSlotContext, TextureSlotLayout, canonical_shader_type,
    slot_to_role,
};
use byroredux_nif::scene::NifScene;
use byroredux_nif::shader_flags::{skyrim_slsf1, skyrim_slsf2};
use byroredux_nif::stream::NifStream;
use byroredux_nif::types::BlockRef;
use sculpto_core::mesh::{
    AlphaProperty, MaterialSlot, PartitionId, PartitionSlot, ShaderFlags, ShaderKind,
};
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

/// `SkyrimShaderPropertyFlags1` bit 0: specular term enabled.
///
/// The vendored flag module declares only the SLSF1 bits its own importers read
/// (`shader_flags.rs`); nif.xml's `SkyrimShaderPropertyFlags1` is the authority
/// for this position, which `Fallout4ShaderPropertyFlags1` repeats.
const SLSF1_SPECULAR: u32 = 0x0000_0001;
/// `SkyrimShaderPropertyFlags1` bit 3: per-vertex alpha modulates the surface.
const SLSF1_VERTEX_ALPHA: u32 = 0x0000_0008;
/// `SkyrimShaderPropertyFlags1` bit 7: environment reflection map applies.
const SLSF1_ENVIRONMENT_MAPPING: u32 = 0x0000_0080;
/// `NiAlphaProperty.flags` bit 0: colour blending enabled.
const ALPHA_BLEND: u16 = 0x0001;
/// `NiAlphaProperty.flags` bit 9: alpha test enabled.
const ALPHA_TEST: u16 = 0x0200;

/// The source facts one imported mesh's block carries beyond the importer's
/// projection.
pub(crate) struct Captured {
    /// Authored influence rows, `None` for an unskinned mesh.
    pub(crate) weights: Option<SkinWeights>,
    /// One partition id per imported triangle; empty when the source authors
    /// no partition table this projection can attribute.
    pub(crate) triangle_partitions: Vec<PartitionId>,
    /// Authored partition entries in source order with their body-part tags.
    pub(crate) partitions: Vec<PartitionSlot>,
    /// Authored material slots (at most one per block).
    pub(crate) materials: Vec<MaterialSlot>,
}

/// Capture the authored facts of every imported mesh, in mesh order.
///
/// A skinned mesh must resolve to one source block; an unskinned one resolves
/// only when its name and vertex count pick out exactly one block, so an
/// ambiguous static keeps the importer's projection instead of guessing.
pub(crate) fn capture(
    bytes: &[u8],
    scene: &NifScene,
    meshes: &[ImportedMesh],
) -> Result<Vec<Captured>, NifError> {
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
        .map(|mesh| {
            let source = match_shapes(&sources, mesh)?;
            let weights = match (&mesh.skin, source) {
                (None, _) => None,
                (Some(skin), Some(source)) => {
                    let palette: Vec<String> = skin
                        .bones
                        .iter()
                        .map(|bone| bone.name.to_string())
                        .collect();
                    let rows = source.rows(bytes, &header, &blocks, scene)?;
                    build_rows(rows, palette.len(), &source.name).map(Some)?
                }
                (Some(_), None) => {
                    return Err(shape_error(
                        mesh.name.as_deref().unwrap_or_default(),
                        "the imported skin has no source block",
                    ));
                }
            };
            let (partitions, triangle_partitions) = match source {
                Some(source) => partitions(scene, source, mesh)?,
                None => (Vec::new(), Vec::new()),
            };
            let materials = match source {
                Some(source) => materials(scene, source),
                None => Vec::new(),
            };
            Ok(Captured {
                weights,
                triangle_partitions,
                partitions,
                materials,
            })
        })
        .collect()
}

/// One source shape that could carry an imported mesh's authored facts.
struct SourceShape {
    /// Block index in the file.
    block: usize,
    /// `NiObjectNET` name of the shape block, empty when unnamed.
    name: String,
    /// The shape's skin instance, when it has one.
    skin_ref: BlockRef,
    /// The shape's shader property, when it names one.
    shader_ref: BlockRef,
    /// The shape's alpha property, when it names one.
    alpha_ref: BlockRef,
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

/// Every shape block with its palette identity.
fn source_shapes(scene: &NifScene) -> Vec<SourceShape> {
    let mut shapes = Vec::new();
    for block in 0..scene.blocks.len() {
        let (name, skin_ref, shader_ref, alpha_ref, vertices) =
            if let Some(shape) = scene.get_as::<BsTriShape>(block) {
                (
                    shape.av.net.name.as_deref().unwrap_or_default().to_string(),
                    shape.skin_ref,
                    shape.shader_property_ref,
                    shape.alpha_property_ref,
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
                    shape.shader_property_ref,
                    shape.alpha_property_ref,
                    vertices,
                )
            } else {
                continue;
            };
        let palette = match skin_ref.index().and_then(|index| instance_of(scene, index)) {
            Some(instance) => instance
                .bone_refs
                .iter()
                .enumerate()
                .map(|(index, bone)| {
                    bone.index()
                        .and_then(|index| scene.get_as::<NiNode>(index))
                        .and_then(|node| node.av.net.name.as_ref().map(|name| name.to_string()))
                        .unwrap_or_else(|| format!("Bone{index}"))
                })
                .collect(),
            None => Vec::new(),
        };
        shapes.push(SourceShape {
            block,
            name,
            skin_ref,
            shader_ref,
            alpha_ref,
            palette,
            vertices,
        });
    }
    shapes
}

/// Find the source shape that carries an imported mesh's authored facts.
///
/// A skinned mesh keeps the identity-first rule: the importer copies the shape
/// block's `NiObjectNET` name onto the mesh, so a present name selects that
/// block and its declared vertex count is then verified; without an identity the
/// palette must be unique, several matches are disambiguated by vertex count and
/// otherwise refused with every candidate listed.
///
/// An unskinned mesh has no palette to identify it, so only a unique
/// name-and-vertex-count match resolves it; `None` leaves the importer's
/// projection in place rather than guessing between look-alike statics.
fn match_shapes<'a>(
    shapes: &'a [SourceShape],
    mesh: &ImportedMesh,
) -> Result<Option<&'a SourceShape>, NifError> {
    let vertices = mesh.positions.len();
    let name = mesh.name.as_deref().filter(|name| !name.is_empty());
    let Some(skin) = &mesh.skin else {
        return Ok(rigid_match(shapes, name, vertices));
    };
    let palette: Vec<String> = skin
        .bones
        .iter()
        .map(|bone| bone.name.to_string())
        .collect();
    let candidates: Vec<&SourceShape> = shapes
        .iter()
        .filter(|shape| shape.palette == palette)
        .collect();
    if let Some(name) = name {
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
                _ => Ok(Some(*shape)),
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
        [shape] => Ok(Some(*shape)),
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
                [shape] => Ok(Some(*shape)),
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

/// The single unskinned block matching an imported rigid mesh.
fn rigid_match<'a>(
    shapes: &'a [SourceShape],
    name: Option<&str>,
    vertices: usize,
) -> Option<&'a SourceShape> {
    let name = name?;
    let mut matching = shapes.iter().filter(|shape| {
        shape.skin_ref.index().is_none()
            && shape.name == name
            && shape.vertices.is_none_or(|declared| declared == vertices)
    });
    match (matching.next(), matching.next()) {
        (Some(shape), None) => Some(shape),
        _ => None,
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

/// Canonical form of one triangle, so a partition entry matches its imported
/// triangle whatever the winding or vertex order.
fn canonical_triangle(mut triangle: [u32; 3]) -> [u32; 3] {
    triangle.sort_unstable();
    triangle
}

/// Read the source's partition table: the authored entry order with its
/// body-part tags, and the entry each imported triangle belongs to.
///
/// The table is authoritative in both halves. A single-entry table covers the
/// shape exactly when its triangle count matches, so the whole mesh maps to
/// entry 0 without a triangle walk. A multi-entry table is attributed by
/// triangle identity, the same index space the importer uses
/// (`import/mesh/skin.rs` `triangle_body_parts`): SSE triangles address the
/// packed buffer directly, LE triangles address the entry's `vertex_map`. A
/// triangle that no entry claims is a refusal, because writing the collapsed
/// table instead would silently merge authored segments.
fn partitions(
    scene: &NifScene,
    source: &SourceShape,
    mesh: &ImportedMesh,
) -> Result<(Vec<PartitionSlot>, Vec<PartitionId>), NifError> {
    let Some(instance) = source
        .skin_ref
        .index()
        .and_then(|index| instance_of(scene, index))
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let slots = source
        .skin_ref
        .index()
        .and_then(|index| scene.get_as::<BsDismemberSkinInstance>(index))
        .map(|dismember| {
            dismember
                .partitions
                .iter()
                .map(|slot| PartitionSlot {
                    partition: PartitionId(0),
                    part_flag: slot.part_flag,
                    body_part: slot.body_part,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Some(partition) = instance
        .skin_partition_ref
        .index()
        .and_then(|index| scene.get_as::<NiSkinPartition>(index))
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    if partition.partitions.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if !slots.is_empty() && slots.len() != partition.partitions.len() {
        return Err(shape_error(
            &source.name,
            &format!(
                "the dismember table has {} slots for {} partition entries",
                slots.len(),
                partition.partitions.len()
            ),
        ));
    }
    let entries = partition.partitions.len();
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    // SSE entries address the packed buffer; LE entries address their own
    // `vertex_map`. An empty map means the entry's indices are already global.
    let addressable = partition.global_vertex_data.is_some();
    let index_of: Vec<PartitionId> = match entries {
        1 if partition.partitions[0].triangles.len() == triangles.len() => {
            vec![PartitionId(0); triangles.len()]
        }
        _ => {
            let mut by_triangle = std::collections::HashMap::new();
            for (entry, body) in partition.partitions.iter().enumerate() {
                for triangle in &body.triangles {
                    let mut global = [0_u32; 3];
                    let mut valid = true;
                    for (target, &local) in global.iter_mut().zip(triangle) {
                        *target = match addressable || body.vertex_map.is_empty() {
                            true => u32::from(local),
                            false => match body.vertex_map.get(usize::from(local)) {
                                Some(&mapped) => u32::from(mapped),
                                None => {
                                    valid = false;
                                    break;
                                }
                            },
                        };
                    }
                    if valid {
                        by_triangle
                            .entry(canonical_triangle(global))
                            .or_insert(PartitionId(entry as u16));
                    }
                }
            }
            triangles
                .iter()
                .enumerate()
                .map(|(triangle, vertices)| {
                    by_triangle
                        .get(&canonical_triangle(*vertices))
                        .copied()
                        .ok_or_else(|| {
                            shape_error(
                                &source.name,
                                &format!(
                                    "triangle {triangle} belongs to none of the {entries} partition \
                                     entries"
                                ),
                            )
                        })
                })
                .collect::<Result<_, NifError>>()?
        }
    };
    let slots = slots
        .into_iter()
        .enumerate()
        .map(|(entry, slot)| PartitionSlot {
            partition: PartitionId(entry as u16),
            ..slot
        })
        .collect();
    Ok((slots, index_of))
}

/// Read the shape's shader block as one material slot.
///
/// The authored `BSShaderTextureSet` slot list is carried verbatim, including
/// the empty slots that hold their position; the roles whose slot is
/// polymorphic or flag-gated (glow, environment, environment mask, specular)
/// are resolved through the vendored `slot_to_role` table so this adapter never
/// keeps a second copy of it. The fixed-position roles stay in the table alone.
/// A shape whose shader block is absent, or not a `BSLightingShaderProperty`,
/// carries no material. @req DECL0008
fn materials(scene: &NifScene, source: &SourceShape) -> Vec<MaterialSlot> {
    let Some(shader) = source
        .shader_ref
        .index()
        .and_then(|index| scene.get_as::<BSLightingShaderProperty>(index))
    else {
        return Vec::new();
    };
    let texture_set = shader
        .texture_set_ref
        .index()
        .and_then(|index| scene.get_as::<BSShaderTextureSet>(index))
        .map(|set| set.textures.clone())
        .unwrap_or_default();
    let name = match (source.name.is_empty(), shader.net.name.as_deref()) {
        (false, _) => source.name.clone(),
        (true, Some(name)) if !name.is_empty() => name.to_string(),
        (true, _) => "material".to_string(),
    };
    let flags = shader_flags(shader);
    let layout = TextureSlotLayout::from_bsver(scene.bsver);
    let context = TextureSlotContext {
        layout,
        shader_type: canonical_shader_type(layout, shader.shader_type),
        glow_map: flags.glow_map,
        model_space_normals: flags.model_space_normals,
        soft_lighting: flags.soft_lighting,
        rim_lighting: flags.rim_lighting,
        back_lighting: flags.back_lighting,
    };
    let mut roles = RolePaths::default();
    for (slot, path) in texture_set.iter().enumerate() {
        if path.is_empty() {
            continue;
        }
        // Roles the model does not name (the skin-tint mask, the detail/height
        // and inner-layer channels, the lighting and back-light masks) stay in
        // the table alone; first slot wins for a role that two slots can carry.
        match slot_to_role(context, slot as u32) {
            Some(TextureRole::Emissive) => set_once(&mut roles.glow, path),
            Some(TextureRole::Environment) => set_once(&mut roles.environment, path),
            Some(TextureRole::EnvironmentMask) => set_once(&mut roles.environment_mask, path),
            Some(TextureRole::Specular | TextureRole::SmoothSpec) => {
                set_once(&mut roles.specular, path);
            }
            Some(
                TextureRole::BaseColor
                | TextureRole::Normal
                | TextureRole::Tint
                | TextureRole::Detail
                | TextureRole::Height
                | TextureRole::GreyscaleLut
                | TextureRole::InnerLayer
                | TextureRole::LightingMask
                | TextureRole::BackLighting
                | TextureRole::Wrinkle,
            )
            | None => {}
        }
    }
    vec![MaterialSlot {
        name,
        shader: ShaderKind::Default,
        diffuse: None,
        normal: None,
        specular: roles.specular,
        glow: roles.glow,
        environment: roles.environment,
        environment_mask: roles.environment_mask,
        skin_tint: match &shader.shader_type_data {
            ShaderTypeData::SkinTint {
                skin_tint_color, ..
            } => Some(*skin_tint_color),
            _ => None,
        },
        specular_power: shader.specular_strength,
        emissive_color: shader.emissive_color,
        emissive_multiple: shader.emissive_multiple,
        alpha: alpha_property(scene, source),
        flags,
        texture_set,
    }]
}

/// The semantic texture paths one authored table resolves to.
#[derive(Default)]
struct RolePaths {
    specular: Option<String>,
    glow: Option<String>,
    environment: Option<String>,
    environment_mask: Option<String>,
}

/// Record one authored path for a role, first slot wins.
fn set_once(target: &mut Option<String>, path: &str) {
    if target.is_none() {
        *target = Some(path.to_string());
    }
}

/// One shape's authored `NiAlphaProperty`, when it names one.
///
/// Bit layout as the vendored importer decodes it
/// (`import/material/mod.rs`: `blend` bit 0, source factor bits 1-4, destination
/// factor bits 5-8, `test` bit 9, test function bits 10-12). @req DECL0008
fn alpha_property(scene: &NifScene, source: &SourceShape) -> Option<AlphaProperty> {
    let property = source
        .alpha_ref
        .index()
        .and_then(|index| scene.get_as::<NiAlphaProperty>(index))?;
    Some(AlphaProperty {
        blend: property.flags & ALPHA_BLEND != 0,
        test: property.flags & ALPHA_TEST != 0,
        src_blend: ((property.flags >> 1) & 0xF) as u8,
        dst_blend: ((property.flags >> 5) & 0xF) as u8,
        threshold: f32::from(property.threshold) / 255.0,
    })
}

/// The model's neutral view of one shader block's feature gates.
fn shader_flags(shader: &BSLightingShaderProperty) -> ShaderFlags {
    let one = shader.shader_flags_1;
    let two = shader.shader_flags_2;
    ShaderFlags {
        specular: one & SLSF1_SPECULAR != 0,
        environment_mapping: one & SLSF1_ENVIRONMENT_MAPPING != 0,
        emissive: one & skyrim_slsf1::OWN_EMIT != 0,
        model_space_normals: one & skyrim_slsf1::MODEL_SPACE_NORMALS != 0,
        vertex_alpha: one & SLSF1_VERTEX_ALPHA != 0,
        double_sided: two & skyrim_slsf2::DOUBLE_SIDED != 0,
        glow_map: two & skyrim_slsf2::GLOW_MAP != 0,
        soft_lighting: two & skyrim_slsf2::SOFT_LIGHTING != 0,
        rim_lighting: two & skyrim_slsf2::RIM_LIGHTING != 0,
        back_lighting: two & skyrim_slsf2::BACK_LIGHTING != 0,
    }
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
