//! Per-shape facts both Skyrim writers need before any block index exists.
//!
//! A bundle assigns block indices only after every shape is validated, so the
//! facts that do not depend on an index — counts, attribute presence, the skin
//! plan, the material slot and its texture set, the physics link — are computed
//! once here. The SE writer additionally packs the `BSVertexDataSSE` stream
//! because its skinned shapes keep geometry in `NiSkinPartition`; the LE writer
//! keeps geometry on `NiTriShapeData` and reads only the counts and attribute
//! presence.
//!
//! Sharing this owner is what keeps the two targets from disagreeing about a
//! shape: the bytes of a `BSTriShape` and the bytes of its `NiTriShape` pair both
//! come from one validated description.

use sculpto_core::mesh::{MaterialSlot, MeshAsset, Transform};
use sculpto_core::skin::FourLane;

use super::InfluencePolicy;
use super::extra;
use super::header::{NifTarget, NifWriteError};
use super::shader;
use super::skin;

/// SE vertex attribute flag: position.
const VF_VERTEX: u64 = 0x001;
/// SE vertex attribute flag: UV.
const VF_UVS: u64 = 0x002;
/// SE vertex attribute flag: normal.
const VF_NORMALS: u64 = 0x008;
/// SE vertex attribute flag: tangent.
const VF_TANGENTS: u64 = 0x010;
/// SE vertex attribute flag: packed skin lane.
const VF_SKINNED: u64 = 0x040;

/// Everything one shape contributes before a block index is known.
pub(super) struct PreparedShape {
    /// Vertex count narrowed to the block's `u16` field.
    vertices: u16,
    /// Triangle count narrowed to the block's `u16` field.
    triangles: u16,
    /// Attribute flags and packed stride.
    attributes: Attributes,
    /// Packed `BSVertexDataSSE` stream in mesh order; empty for LE.
    vertex_bytes: Vec<u8>,
    /// The validated skin plan, when the shape is skinned.
    pub(super) skin: Option<skin::SkinPlan>,
    /// The single material slot, when the shape carries one.
    material: Option<MaterialSlot>,
    /// `BSShaderTextureSet` body, empty when the slot declares no texture.
    pub(super) texture: Vec<u8>,
    /// The HDT-SMP physics link this shape names.
    pub(super) physics: Option<extra::PhysicsLink>,
}

impl PreparedShape {
    /// Validate the model and plan every per-shape fact that no block index
    /// affects: counts, geometry, skin, material and physics.
    ///
    /// The target selects what has to be prepared: SE packs the vertex stream,
    /// LE only records which attributes exist. Tangents are refused for LE
    /// before anything else because the LE layout has no tangent channel, so a
    /// shape that cannot be represented never reaches the file.
    pub(super) fn new(
        asset: &MeshAsset,
        policy: InfluencePolicy,
        target: NifTarget,
    ) -> Result<Self, NifWriteError> {
        asset.validate().map_err(NifWriteError::InvalidModel)?;
        let vertices =
            u16::try_from(asset.positions.len()).map_err(|_| NifWriteError::TooManyVertices {
                vertices: asset.positions.len(),
            })?;
        let triangles =
            u16::try_from(asset.triangles.len()).map_err(|_| NifWriteError::TooManyTriangles {
                triangles: asset.triangles.len(),
            })?;
        if target == NifTarget::Le && !asset.tangents.is_empty() {
            return Err(NifWriteError::TangentsUnsupported { target });
        }
        let base = Attributes::of(asset)?;
        let skin = skin::plan(asset, target, policy)?;
        let (attributes, vertex_bytes) = match target {
            NifTarget::Se => {
                let attributes = match skin.is_some() {
                    true => base.skinned(),
                    false => base,
                };
                let lanes = skin.as_ref().map(|plan| plan.lanes.as_slice());
                let bytes = packed_vertices(
                    asset,
                    lanes,
                    base.has(VF_UVS),
                    base.has(VF_NORMALS),
                    base.has(VF_TANGENTS),
                )?;
                (attributes, bytes)
            }
            NifTarget::Le => (base, Vec::new()),
        };
        let material = shader::single_slot(asset)?.cloned();
        let texture = match &material {
            Some(slot) => shader::texture_set(slot)?,
            None => Vec::new(),
        };
        let physics = extra::physics_link(asset)?;
        Ok(Self {
            vertices,
            triangles,
            attributes,
            vertex_bytes,
            skin,
            material,
            texture,
            physics,
        })
    }

    /// Whether the shape writes a skin group.
    pub(super) fn skinned(&self) -> bool {
        self.skin.is_some()
    }

    /// Whether the shape writes a shader property.
    pub(super) fn has_material(&self) -> bool {
        self.material.is_some()
    }

    /// Whether the shape authors UVs.
    pub(super) fn has_uvs(&self) -> bool {
        self.attributes.has(VF_UVS)
    }

    /// Whether the shape authors normals.
    pub(super) fn has_normals(&self) -> bool {
        self.attributes.has(VF_NORMALS)
    }

    /// Vertex count as the blocks store it.
    pub(super) fn vertices(&self) -> u16 {
        self.vertices
    }

    /// Triangle count as the blocks store it.
    pub(super) fn triangles(&self) -> u16 {
        self.triangles
    }

    /// `BSVertexDesc` word for the SE shape and its partitions.
    pub(super) fn descriptor(&self) -> u64 {
        self.attributes.descriptor()
    }

    /// Packed stride per vertex, in bytes.
    pub(super) fn packed_size(&self) -> u32 {
        self.attributes.size
    }

    /// The packed `BSVertexDataSSE` stream, empty for LE.
    pub(super) fn vertex_bytes(&self) -> &[u8] {
        &self.vertex_bytes
    }

    /// This shape's SSE packed-vertex view, for its own partition block.
    pub(super) fn global(&self) -> skin::GlobalVertices<'_> {
        skin::GlobalVertices {
            vertex_desc: self.attributes.descriptor(),
            vertex_size: self.attributes.size,
            bytes: &self.vertex_bytes,
        }
    }

    /// `BSLightingShaderProperty` body for the assigned blocks.
    pub(super) fn shader_body(&self, name: i32, texture_set: i32) -> Vec<u8> {
        match &self.material {
            Some(slot) => shader::shader(slot, texture_set, name),
            None => Vec::new(),
        }
    }
}

/// SE vertex attribute flags and packed stride, before the skin lane.
#[derive(Clone, Copy)]
struct Attributes {
    flags: u64,
    size: u32,
}

impl Attributes {
    /// Attribute flags for the model's authored attributes.
    ///
    /// Tangents need normals because the packed frame derives the bitangent from
    /// them, so that pair is refused rather than written inconsistently.
    fn of(asset: &MeshAsset) -> Result<Self, NifWriteError> {
        let mut attributes = Self {
            flags: VF_VERTEX,
            size: 16,
        };
        for (present, flag, bytes) in [
            (!asset.uvs.is_empty(), VF_UVS, 4),
            (!asset.normals.is_empty(), VF_NORMALS, 4),
            (!asset.tangents.is_empty(), VF_TANGENTS, 4),
        ] {
            if present {
                attributes.flags |= flag;
                attributes.size += bytes;
            }
        }
        if attributes.has(VF_TANGENTS) && !attributes.has(VF_NORMALS) {
            return Err(NifWriteError::TangentsWithoutNormals);
        }
        Ok(attributes)
    }

    /// Record the packed skin lane the shape writes after its attributes.
    fn skinned(mut self) -> Self {
        self.flags |= VF_SKINNED;
        self.size += 12;
        self
    }

    /// Whether the packed stream carries `flag`'s attribute.
    fn has(self, flag: u64) -> bool {
        self.flags & flag != 0
    }

    /// The `BSVertexDesc` word: flags in the high 44 bits, stride in words.
    fn descriptor(self) -> u64 {
        (self.flags << 44) | u64::from(self.size / 4)
    }
}

/// Refuse a bundle shape whose local placement the bundle writers cannot
/// serialize.
///
/// The placement and the skinning frame have to be decided together, and the
/// model has no global-skin field, so a non-identity placement is named instead
/// of being dropped or written beside an identity skin. Identity — including an
/// explicit `Some(Transform::default())` — is accepted.
pub(super) fn refuse_placements(shapes: &[MeshAsset]) -> Result<(), NifWriteError> {
    shapes.iter().enumerate().try_for_each(|(index, shape)| {
        match shape
            .transform
            .is_some_and(|transform| transform != Transform::default())
        {
            true => Err(NifWriteError::TransformUnsupported { shape: index }),
            false => Ok(()),
        }
    })
}

/// The one root physics link a bundle may carry.
///
/// HDT-SMP scans the model root and the reference keeps one link per model
/// (`extra.rs`), so shapes naming different files cannot share this file; the
/// count is reported instead of picking one at random.
pub(super) fn single_physics_link(
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

/// Pack every mesh vertex into the `BSVertexDataSSE` stream the shape's
/// `vertex_desc` describes.
///
/// The same bytes serve both layouts: inline on an unskinned `BSTriShape`, and
/// as the `NiSkinPartition` global buffer for a skinned one. Field order and
/// gates match nifly's decoder (`Skin.cpp`, `NiSkinPartition::Sync`):
/// position + W, UV, normal + bitangent Y, tangent + bitangent Z, four half
/// weights, four bone bytes.
fn packed_vertices(
    asset: &MeshAsset,
    skin: Option<&[FourLane]>,
    has_uvs: bool,
    has_normals: bool,
    has_tangents: bool,
) -> Result<Vec<u8>, NifWriteError> {
    let mut bytes = Vec::new();
    for (index, position) in asset.positions.iter().enumerate() {
        let normal = match has_normals {
            true => Some(unit(asset.normals[index], "normal", index)?),
            false => None,
        };
        let tangent = match has_tangents {
            true => {
                let raw = asset.tangents[index];
                let direction = unit([raw[0], raw[1], raw[2]], "tangent", index)?;
                let sign = if raw[3].is_sign_negative() { -1.0 } else { 1.0 };
                Some((direction, sign))
            }
            false => None,
        };

        push_f32(&mut bytes, position[0]);
        push_f32(&mut bytes, position[1]);
        push_f32(&mut bytes, position[2]);
        // Position W carries Bitangent X when VF_TANGENTS is set.
        push_f32(&mut bytes, tangent.map_or(0.0, |(b, _)| b[0]));
        if has_uvs {
            let uv = asset.uvs[index];
            push_u16(&mut bytes, half::f16::from_f32(uv[0]).to_bits());
            push_u16(&mut bytes, half::f16::from_f32(uv[1]).to_bits());
        }
        if let Some(normal) = normal {
            push_u8(&mut bytes, encode_normal(normal[0]));
            push_u8(&mut bytes, encode_normal(normal[1]));
            push_u8(&mut bytes, encode_normal(normal[2]));
            // Normal W carries Bitangent Y when VF_TANGENTS is set.
            push_u8(
                &mut bytes,
                encode_normal(tangent.map_or(0.0, |(b, _)| b[1])),
            );
        }
        if let Some((bitangent, sign)) = tangent {
            // On-disk Tangent is Bethesda's dP/dV; the reader keeps it only to
            // derive the sign of `dot(B, cross(N, T))`. `sign * cross(B, N)`
            // reproduces the requested sign exactly.
            let normal = normal.expect("tangents require normals");
            let on_disk = normalize(cross(normal, bitangent), "tangent", index)?;
            push_u8(&mut bytes, encode_normal(sign * on_disk[0]));
            push_u8(&mut bytes, encode_normal(sign * on_disk[1]));
            push_u8(&mut bytes, encode_normal(sign * on_disk[2]));
            // Tangent W carries Bitangent Z.
            push_u8(&mut bytes, encode_normal(bitangent[2]));
        }
        if let Some(skin) = skin {
            let lanes = skin[index];
            for weight in lanes.weights {
                push_u16(&mut bytes, half::f16::from_f32(weight).to_bits());
            }
            for bone in lanes.joints {
                push_u8(&mut bytes, bone as u8);
            }
        }
    }
    Ok(bytes)
}

fn unit(
    vector: [f32; 3],
    attribute: &'static str,
    vertex: usize,
) -> Result<[f32; 3], NifWriteError> {
    normalize(vector, attribute, vertex)
}

fn normalize(
    vector: [f32; 3],
    attribute: &'static str,
    vertex: usize,
) -> Result<[f32; 3], NifWriteError> {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length == 0.0 || !length.is_finite() {
        return Err(NifWriteError::ZeroVector { attribute, vertex });
    }
    Ok([vector[0] / length, vector[1] / length, vector[2] / length])
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn encode_normal(value: f32) -> u8 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 127.5)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
