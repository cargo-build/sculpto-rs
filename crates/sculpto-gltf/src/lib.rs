//! glTF adapter: write a Sculpto [`MeshAsset`](sculpto_core::mesh::MeshAsset)
//! as a single-buffer GLB.
//!
//! Geometry, skin, materials and morph targets:
//! - Materials: neutral [`MaterialSlot`](sculpto_core::mesh::MaterialSlot)
//!   maps diffuse to `pbrMetallicRoughness.baseColorTexture` and normal to
//!   `normalTexture` using relative URIs. Specular textures have no core glTF
//!   slot and are refused as [`GltfError::TextureSlotUnsupported`].
//!   Today, `MeshAsset` represents a single authored shape with at most one
//!   material slot (`asset.materials.len() <= 1`); multiple slots are refused
//!   as [`GltfError::MultipleMaterialSlots`].
//! - Morphs: sparse [`MorphTarget`](sculpto_core::mesh::MorphTarget) deltas
//!   are expanded to full-primitive POSITION accessors with min/max bounds and
//!   zero default weights (`mesh.weights`). Target names are recorded under
//!   `extras.targetNames`. Any morph whose vertex index exceeds the primitive's
//!   vertex count is refused as [`MeshError::MorphVertexOutOfRange`].
//!   Normal deltas are omitted as `MorphDelta` carries only position deltas.
//!   glTF morph targets stay whole-mesh: [`MorphScope`](sculpto_core::mesh::MorphScope)
//!   narrows the `.tri` shape projection, not the glTF target list.
//! - Physics: a [`PhysicsRef::Path`](sculpto_core::mesh::PhysicsRef) sidecar
//!   travels as the documented `extras.sculpto.physics` string on the root
//!   node — glTF has no native slot for Skyrim-style physics data. An inline
//!   document has no representation in the file and is refused as
//!   [`GltfError::PhysicsInlineUnsupported`].
//! - Clips: [`write_glb_with`] emits one glTF `animation` per [`Clip`] with
//!   LINEAR samplers whose channels target the joint node of the named bone.
//!   Zero-length or single-sample clips are refused as
//!   [`GltfError::ConstantClip`]; the E09 animation gate exists precisely
//!   because a constant clip is a defect.
//! - Packing: [`pack::PackWriter`] is the only writer of the append path that
//!   adds baked cloth geometry to an existing GLB; the actor admission stays
//!   in `sculpto-mesh`.
//!
//! Layout contract of [`write_glb_with`]:
//! - One mesh with one primitive per triangle partition, primitives in
//!   ascending partition-id order. The model is one flat vertex arena, so all
//!   primitives share the same vertex accessors and each keeps only its own
//!   index accessor; the material slice maps slots per primitive.
//! - POSITION (with `min`/`max`), plus NORMAL, TEXCOORD_0 and TANGENT when the
//!   model carries them. Indices are `u32`. Mesh names come from
//!   `MeshAsset::name` and joint nodes from the bone names.
//! - The scene is one root node named from `MeshAsset::name`; it carries the
//!   mesh and skin, and the joint nodes hang under it as children (one
//!   identity node per bone; the skin's `skeleton` root is the first joint),
//!   inverse bind matrices from [`sculpto_core::mesh::Skin::bind_inverse`].
//!   The model stores bind matrices column-major, which is already the glTF
//!   `MAT4` layout, so they are flattened without transposing; an empty array
//!   means the identity per bone. A clip channel drives bone `b` through its
//!   joint node `b + 1`: node 0 is the root/mesh node and the joints follow in
//!   model bone order.
//! - `JOINTS_0` is `u16` and `WEIGHTS_0` is `f32`: `f32` keeps the model's
//!   weights bit-exact instead of quantizing them into normalized `u16`.

pub mod pack;

pub mod profile;

pub mod sanitize;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use sculpto_core::mesh::{MeshAsset, MeshError, PartitionId, PhysicsRef, TextureSlot};
use serde_json::{Map, Value, json};

use profile::ConsumerProfile;

/// `f32` accessor component type.
const FLOAT: u32 = 5126;
/// `u16` accessor component type.
const UNSIGNED_SHORT: u32 = 5123;
/// `u32` accessor component type.
const UNSIGNED_INT: u32 = 5125;
/// `TRIANGLES` primitive mode.
const TRIANGLES: u32 = 4;
/// Column-major identity, the bind fallback for a skin without matrices.
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Why a model cannot be exported as GLB.
#[derive(Debug, Clone, PartialEq)]
pub enum GltfError {
    /// `MeshAsset::validate()` refused the model; nothing was written.
    InvalidMesh(MeshError),
    /// The model has no triangles, so there is no primitive to write.
    NoTriangles,
    /// An export size exceeds the glTF `u32` field it must fit in.
    TooLarge {
        /// The field that overflowed.
        field: &'static str,
        /// The size that does not fit.
        size: usize,
    },
    /// A material channel has no standard glTF slot and was refused.
    TextureSlotUnsupported {
        /// The unsupported texture channel name.
        slot: &'static str,
    },
    /// The model declared more than one material slot.
    MultipleMaterialSlots {
        /// The number of material slots declared.
        count: usize,
    },
    /// A texture channel has no UV coordinates to sample it with: glTF would
    /// emit `baseColorTexture`/`normalTexture` for a mesh without
    /// `TEXCOORD_0`, and consumers reject the target later (mikktspace on
    /// Bevy). Ship the model with UVs or without the texture.
    TextureWithoutUv {
        /// The texture channel name.
        slot: &'static str,
    },
    /// The model carries more morph targets than the caller-selected
    /// [`profile::ConsumerProfile`] measured on its consumer build; nothing
    /// was written.
    ConsumerCapExceeded {
        /// The profile name.
        profile: String,
        /// The consumer version requirement the profile targets.
        requirement: String,
        /// The exact consumer version that was measured.
        resolved: String,
        /// The measured morph-target cap.
        cap: usize,
        /// The model's morph-target count that exceeds it.
        count: usize,
    },
    /// A clip has fewer than two samples, so a player could only render it
    /// constant; constant clips are defects (the E09 animation gate refuses
    /// them on written bytes too).
    ConstantClip {
        /// The clip's position in the clip slice.
        clip: usize,
        /// The clip name.
        name: String,
        /// The sample count that is too few.
        samples: usize,
    },
    /// A channel's value count differs from its clip's sample count, which no
    /// sampler can pair.
    ClipChannelSampleMismatch {
        /// The clip's position in the clip slice.
        clip: usize,
        /// The channel's position in the clip.
        channel: usize,
        /// The clip's sample count.
        samples: usize,
        /// The channel's value count.
        values: usize,
    },
    /// A channel targets a bone the skin has no joint node for (a model
    /// without a skin has no joints at all).
    ClipBoneOutOfRange {
        /// The clip's position in the clip slice.
        clip: usize,
        /// The channel's position in the clip.
        channel: usize,
        /// The targeted bone index.
        bone: usize,
        /// The number of joint nodes the model offers.
        joints: usize,
    },
    /// The model carries an inline physics document; glTF has no slot for it,
    /// so the sidecar must ship as a file referenced by path.
    PhysicsInlineUnsupported,
    /// The JSON chunk failed to serialize.
    Serialize(String),
    /// The GLB container failed to frame the chunks.
    Encode(String),
}

impl fmt::Display for GltfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMesh(error) => write!(f, "mesh refused: {error:?}"),
            Self::NoTriangles => write!(f, "mesh has no triangles to export"),
            Self::TooLarge { field, size } => {
                write!(f, "{field} ({size}) exceeds the glTF u32 limit")
            }
            Self::TextureSlotUnsupported { slot } => {
                write!(
                    f,
                    "texture slot '{slot}' is not supported in glTF 2.0 core PBR"
                )
            }
            Self::MultipleMaterialSlots { count } => {
                write!(
                    f,
                    "model has {count} material slots; glTF export supports at most one"
                )
            }
            Self::ConsumerCapExceeded {
                profile,
                requirement,
                resolved,
                cap,
                count,
            } => write!(
                f,
                "profile '{profile}' (requirement {requirement}, resolved {resolved}) \
                 caps morph targets at {cap}; the model carries {count}"
            ),
            Self::TextureWithoutUv { slot } => write!(
                f,
                "texture slot '{slot}' has no UV coordinates to sample with; \
                 export UVs with the mesh or drop the texture"
            ),
            Self::ConstantClip {
                clip,
                name,
                samples,
            } => write!(
                f,
                "clip {clip} ('{name}') has {samples} samples; a constant clip is a \
                 defect and at least two samples are required"
            ),
            Self::ClipChannelSampleMismatch {
                clip,
                channel,
                samples,
                values,
            } => write!(
                f,
                "clip {clip} channel {channel} has {values} values for {samples} samples"
            ),
            Self::ClipBoneOutOfRange {
                clip,
                channel,
                bone,
                joints,
            } => write!(
                f,
                "clip {clip} channel {channel} targets bone {bone} but the model has \
                 {joints} joint nodes"
            ),
            Self::PhysicsInlineUnsupported => write!(
                f,
                "inline physics document has no glTF slot; ship the sidecar as a file \
                 and reference it with PhysicsRef::Path (written as \
                 extras.sculpto.physics)"
            ),
            Self::Serialize(detail) => write!(f, "GLB JSON failed: {detail}"),
            Self::Encode(detail) => write!(f, "GLB framing failed: {detail}"),
        }
    }
}

impl std::error::Error for GltfError {}

/// One named animation clip: shared sample times and per-channel values.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Clip name; an empty name is written without an animation name.
    pub name: String,
    /// Keyframe times, one per sample, shared by every channel.
    pub times: Vec<f32>,
    /// Channels; each carries one value per time.
    pub channels: Vec<ClipChannel>,
}

/// One clip channel: the bone it drives and the property, with one value per
/// clip sample.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipChannel {
    /// Local-space translation per sample.
    Translation {
        /// The bone, indexed into [`sculpto_core::mesh::Skin::bones`].
        bone: usize,
        /// One vector per sample.
        values: Vec<[f32; 3]>,
    },
    /// Local-space rotation quaternion (`x, y, z, w`) per sample.
    Rotation {
        /// The bone, indexed into [`sculpto_core::mesh::Skin::bones`].
        bone: usize,
        /// One quaternion per sample.
        values: Vec<[f32; 4]>,
    },
    /// Local-space scale per sample.
    Scale {
        /// The bone, indexed into [`sculpto_core::mesh::Skin::bones`].
        bone: usize,
        /// One vector per sample.
        values: Vec<[f32; 3]>,
    },
}

/// The bone a channel drives.
fn channel_bone(channel: &ClipChannel) -> usize {
    match channel {
        ClipChannel::Translation { bone, .. }
        | ClipChannel::Rotation { bone, .. }
        | ClipChannel::Scale { bone, .. } => *bone,
    }
}

/// How many samples a channel carries.
fn channel_samples(channel: &ClipChannel) -> usize {
    match channel {
        ClipChannel::Translation { values, .. } | ClipChannel::Scale { values, .. } => values.len(),
        ClipChannel::Rotation { values, .. } => values.len(),
    }
}

/// Write one model as a GLB with an embedded single buffer and no clips.
///
/// The model is validated first, so a refused asset returns a typed
/// [`GltfError`] and never bytes.
pub fn write_glb(asset: &MeshAsset) -> Result<Vec<u8>, GltfError> {
    write_glb_with(asset, &[])
}

/// [`write_glb`] for a caller-selected [`ConsumerProfile`]: the model's
/// morph-target count is compared against the profile's measured cap
/// (`profile.max_morph_targets.value`) before any byte is produced, and a
/// count above it is refused as [`GltfError::ConsumerCapExceeded`], naming
/// the profile.
///
/// Format limits stay adapter-enforced `FormatLimits`, not profile data;
/// see [`profile`].
pub fn write_glb_with_profile(
    asset: &MeshAsset,
    profile: &ConsumerProfile,
) -> Result<Vec<u8>, GltfError> {
    asset.validate().map_err(GltfError::InvalidMesh)?;
    if asset.morphs.len() > profile.max_morph_targets.value {
        return Err(GltfError::ConsumerCapExceeded {
            profile: profile.name.clone(),
            requirement: profile.requirement.clone(),
            resolved: profile.resolved.clone(),
            cap: profile.max_morph_targets.value,
            count: asset.morphs.len(),
        });
    }
    write_glb_with(asset, &[])
}

/// [`write_glb`] with one glTF animation per [`Clip`].
///
/// Every refusal check runs before any byte is produced.
pub fn write_glb_with(asset: &MeshAsset, clips: &[Clip]) -> Result<Vec<u8>, GltfError> {
    asset.validate().map_err(GltfError::InvalidMesh)?;
    if asset.triangles.is_empty() {
        return Err(GltfError::NoTriangles);
    }
    if matches!(asset.physics, Some(PhysicsRef::Inline(_))) {
        return Err(GltfError::PhysicsInlineUnsupported);
    }
    // A clip channel drives bone `b` through joint node `b + 1`: node 0 is
    // the root/mesh node and the joints follow in model bone order.
    let joints = asset.skin.as_ref().map_or(0, |skin| skin.bones.len());
    for (clip_idx, clip) in clips.iter().enumerate() {
        if clip.times.len() < 2 {
            return Err(GltfError::ConstantClip {
                clip: clip_idx,
                name: clip.name.clone(),
                samples: clip.times.len(),
            });
        }
        for (channel_idx, channel) in clip.channels.iter().enumerate() {
            let bone = channel_bone(channel);
            if bone >= joints {
                return Err(GltfError::ClipBoneOutOfRange {
                    clip: clip_idx,
                    channel: channel_idx,
                    bone,
                    joints,
                });
            }
            let values = channel_samples(channel);
            if values != clip.times.len() {
                return Err(GltfError::ClipChannelSampleMismatch {
                    clip: clip_idx,
                    channel: channel_idx,
                    samples: clip.times.len(),
                    values,
                });
            }
        }
    }
    // Accessor counts are glTF u32 fields; the writer refuses what they cannot hold.
    u32::try_from(asset.positions.len()).map_err(|_| GltfError::TooLarge {
        field: "vertex count",
        size: asset.positions.len(),
    })?;

    // The single-shape model allows at most one material slot today.
    let material_slot = match asset.materials.len() {
        0 => None,
        1 => {
            let slot = &asset.materials[0];
            if slot.texture(TextureSlot::Specular).is_some() {
                return Err(GltfError::TextureSlotUnsupported { slot: "specular" });
            }
            // A texture without UVs would emit a sampler consumers cannot
            // evaluate (Bevy's mikktspace refuses the target later), so the
            // writer refuses it up front; an untextured model still writes.
            if asset.uvs.is_empty() {
                for channel in [TextureSlot::Diffuse, TextureSlot::Normal] {
                    if slot.texture(channel).is_some() {
                        return Err(GltfError::TextureWithoutUv {
                            slot: channel.as_str(),
                        });
                    }
                }
            }
            Some(slot)
        }
        count => return Err(GltfError::MultipleMaterialSlots { count }),
    };

    let mut writer = Writer::default();
    let mut attributes = Map::new();

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in &asset.positions {
        for (axis, value) in position.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
        }
    }
    let position_view = writer.view(&bytes_f32(
        asset.positions.iter().flat_map(|row| row.iter().copied()),
    ))?;
    writer.accessors.push(json!({
        "bufferView": position_view,
        "componentType": FLOAT,
        "type": "VEC3",
        "count": asset.positions.len(),
        "min": min,
        "max": max,
    }));
    attributes.insert("POSITION".into(), json!(writer.accessors.len() - 1));

    if !asset.normals.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.normals.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "NORMAL".into(),
            json!(writer.accessor(view, FLOAT, "VEC3", asset.normals.len())),
        );
    }
    if !asset.uvs.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.uvs.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "TEXCOORD_0".into(),
            json!(writer.accessor(view, FLOAT, "VEC2", asset.uvs.len())),
        );
    }
    if !asset.tangents.is_empty() {
        let view = writer.view(&bytes_f32(
            asset.tangents.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "TANGENT".into(),
            json!(writer.accessor(view, FLOAT, "VEC4", asset.tangents.len())),
        );
    }

    let mut skin_json = None;
    if let Some(skin) = &asset.skin {
        let joints_view = writer.view(&bytes_u16(
            skin.indices.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "JOINTS_0".into(),
            json!(writer.accessor(joints_view, UNSIGNED_SHORT, "VEC4", skin.indices.len())),
        );
        let weights_view = writer.view(&bytes_f32(
            skin.weights.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "WEIGHTS_0".into(),
            json!(writer.accessor(weights_view, FLOAT, "VEC4", skin.weights.len())),
        );

        let matrices =
            (0..skin.bones.len()).map(|bone| skin.bind_inverse.get(bone).unwrap_or(&IDENTITY));
        let ibm_view =
            writer.view(&bytes_f32(matrices.flat_map(|matrix| {
                matrix.iter().flat_map(|column| column.iter().copied())
            })))?;
        let ibm = writer.accessor(ibm_view, FLOAT, "MAT4", skin.bones.len());
        let joint_nodes: Vec<usize> = (1..=skin.bones.len()).collect();
        skin_json = Some(json!({
            "inverseBindMatrices": ibm,
            "joints": joint_nodes,
            "skeleton": joint_nodes[0],
        }));
    }

    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut materials = Vec::new();

    if let Some(slot) = material_slot {
        let mut pbr = Map::new();
        if let Some(diffuse) = slot.texture(TextureSlot::Diffuse) {
            let image_idx = images.len();
            images.push(json!({ "uri": diffuse }));
            let texture_idx = textures.len();
            textures.push(json!({ "source": image_idx }));
            pbr.insert("baseColorTexture".into(), json!({ "index": texture_idx }));
        }

        let mut material = Map::new();
        if !slot.name.is_empty() {
            material.insert("name".into(), json!(slot.name));
        }
        if !pbr.is_empty() {
            material.insert("pbrMetallicRoughness".into(), Value::Object(pbr));
        }
        if let Some(normal) = slot.texture(TextureSlot::Normal) {
            let image_idx = images.len();
            images.push(json!({ "uri": normal }));
            let texture_idx = textures.len();
            textures.push(json!({ "source": image_idx }));
            material.insert("normalTexture".into(), json!({ "index": texture_idx }));
        }
        if slot.double_sided {
            material.insert("doubleSided".into(), json!(true));
        }
        materials.push(Value::Object(material));
    }

    let mut target_names = Vec::with_capacity(asset.morphs.len());
    let mut primitive_targets = Vec::with_capacity(asset.morphs.len());
    for (target_idx, morph) in asset.morphs.iter().enumerate() {
        target_names.push(morph.name.clone());
        let mut dense = vec![[0.0_f32; 3]; asset.positions.len()];
        for delta in &morph.deltas {
            if delta.vertex as usize >= asset.positions.len() {
                return Err(GltfError::InvalidMesh(MeshError::MorphVertexOutOfRange {
                    target: target_idx,
                    vertex: delta.vertex,
                }));
            }
            dense[delta.vertex as usize] = delta.position;
        }

        let mut min = [0.0_f32; 3];
        let mut max = [0.0_f32; 3];
        for row in &dense {
            for axis in 0..3 {
                min[axis] = min[axis].min(row[axis]);
                max[axis] = max[axis].max(row[axis]);
            }
        }

        let view = writer.view(&bytes_f32(dense.iter().flat_map(|row| row.iter().copied())))?;
        let accessor_idx = writer.accessors.len();
        writer.accessors.push(json!({
            "bufferView": view,
            "componentType": FLOAT,
            "type": "VEC3",
            "count": dense.len(),
            "min": min,
            "max": max,
        }));
        primitive_targets.push(json!({
            "POSITION": accessor_idx,
        }));
    }

    let mut animations = Vec::with_capacity(clips.len());
    for clip in clips {
        let times_min = clip.times.iter().copied().fold(f32::INFINITY, f32::min);
        let times_max = clip.times.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let times_view = writer.view(&bytes_f32(clip.times.iter().copied()))?;
        writer.accessors.push(json!({
            "bufferView": times_view,
            "componentType": FLOAT,
            "type": "SCALAR",
            "count": clip.times.len(),
            "min": [times_min],
            "max": [times_max],
        }));
        let input = writer.accessors.len() - 1;

        let mut samplers = Vec::with_capacity(clip.channels.len());
        let mut channels = Vec::with_capacity(clip.channels.len());
        for channel in &clip.channels {
            let (path, component, payload, count) = match channel {
                ClipChannel::Translation { values, .. } => (
                    "translation",
                    "VEC3",
                    bytes_f32(values.iter().flat_map(|row| row.iter().copied())),
                    values.len(),
                ),
                ClipChannel::Rotation { values, .. } => (
                    "rotation",
                    "VEC4",
                    bytes_f32(values.iter().flat_map(|row| row.iter().copied())),
                    values.len(),
                ),
                ClipChannel::Scale { values, .. } => (
                    "scale",
                    "VEC3",
                    bytes_f32(values.iter().flat_map(|row| row.iter().copied())),
                    values.len(),
                ),
            };
            let output_view = writer.view(&payload)?;
            let output = writer.accessor(output_view, FLOAT, component, count);
            samplers.push(json!({
                "input": input,
                "interpolation": "LINEAR",
                "output": output,
            }));
            let bone = channel_bone(channel);
            // Validated above: `bone + 1` is that bone's joint node.
            channels.push(json!({
                "sampler": samplers.len() - 1,
                "target": { "node": bone + 1, "path": path },
            }));
        }

        let mut animation = Map::new();
        if !clip.name.is_empty() {
            animation.insert("name".into(), json!(clip.name));
        }
        animation.insert("channels".into(), json!(channels));
        animation.insert("samplers".into(), json!(samplers));
        animations.push(Value::Object(animation));
    }

    let mut by_partition: BTreeMap<PartitionId, Vec<usize>> = BTreeMap::new();
    for (index, partition) in asset.triangle_partitions.iter().enumerate() {
        by_partition.entry(*partition).or_default().push(index);
    }
    let primitives: Vec<Value> = by_partition
        .values()
        .map(|triangle_indices| {
            let indices: Vec<u32> = triangle_indices
                .iter()
                .flat_map(|&triangle| asset.triangles[triangle])
                .collect();
            let view = writer.view(&bytes_u32(indices.iter().copied()))?;
            let mut prim = json!({
                "attributes": attributes.clone(),
                "indices": writer.accessor(view, UNSIGNED_INT, "SCALAR", indices.len()),
                "mode": TRIANGLES,
            });
            if material_slot.is_some() {
                prim["material"] = json!(0);
            }
            if !primitive_targets.is_empty() {
                prim["targets"] = json!(primitive_targets);
            }
            Ok(prim)
        })
        .collect::<Result<Vec<_>, GltfError>>()?;

    let mut mesh = Map::new();
    if !asset.name.is_empty() {
        mesh.insert("name".into(), json!(asset.name));
    }
    mesh.insert("primitives".into(), json!(primitives));
    if !asset.morphs.is_empty() {
        let weights = vec![0.0_f32; asset.morphs.len()];
        mesh.insert("weights".into(), json!(weights));
        mesh.insert(
            "extras".into(),
            json!({
                "targetNames": target_names,
            }),
        );
        mesh.insert("targetNames".into(), json!(target_names));
    }

    let mut mesh_node = Map::new();
    if !asset.name.is_empty() {
        mesh_node.insert("name".into(), json!(asset.name));
    }
    mesh_node.insert("mesh".into(), json!(0));
    if skin_json.is_some() {
        mesh_node.insert("skin".into(), json!(0));
    }
    if let Some(PhysicsRef::Path(path)) = &asset.physics {
        // glTF has no native physics slot; the sidecar travels as a
        // documented extras string on the root node.
        mesh_node.insert("extras".into(), json!({ "sculpto": { "physics": path } }));
    }
    let mut nodes = vec![Value::Object(mesh_node)];
    if let Some(skin) = &asset.skin {
        let mut children = Vec::with_capacity(skin.bones.len());
        for bone in &skin.bones {
            let mut node = Map::new();
            if !bone.is_empty() {
                node.insert("name".into(), json!(bone));
            }
            children.push(nodes.len());
            nodes.push(Value::Object(node));
        }
        nodes[0]["children"] = json!(children);
    }
    let scene_nodes = [0_usize];

    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({"version": "2.0", "generator": "sculpto-gltf"}),
    );
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{ "nodes": scene_nodes }]));
    root.insert("nodes".into(), json!(nodes));
    root.insert("meshes".into(), json!([Value::Object(mesh)]));
    if let Some(skin) = skin_json {
        root.insert("skins".into(), json!([skin]));
    }
    if !animations.is_empty() {
        root.insert("animations".into(), json!(animations));
    }
    if !materials.is_empty() {
        root.insert("materials".into(), json!(materials));
    }
    if !textures.is_empty() {
        root.insert("textures".into(), json!(textures));
    }
    if !images.is_empty() {
        root.insert("images".into(), json!(images));
    }
    root.insert("accessors".into(), json!(writer.accessors));
    root.insert("bufferViews".into(), json!(writer.views));
    root.insert("buffers".into(), json!([{"byteLength": writer.bin.len()}]));

    let json = serde_json::to_vec(&Value::Object(root))
        .map_err(|error| GltfError::Serialize(error.to_string()))?;
    let json_padded = json.len() + (4 - json.len() % 4) % 4;
    let bin_padded = writer.bin.len() + (4 - writer.bin.len() % 4) % 4;
    let total = 12 + 8 + json_padded + 8 + bin_padded;
    if u32::try_from(total).is_err() {
        return Err(GltfError::TooLarge {
            field: "GLB total length",
            size: total,
        });
    }
    gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(json),
        bin: Some(Cow::Owned(writer.bin)),
    }
    .to_vec()
    .map_err(|error| GltfError::Encode(error.to_string()))
}

/// The accumulating GLB: a growing binary buffer with its views and accessors.
#[derive(Default)]
struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Writer {
    /// Append one tightly packed view, four-aligned, returning its index.
    fn view(&mut self, data: &[u8]) -> Result<usize, GltfError> {
        let padding = (4 - self.bin.len() % 4) % 4;
        let offset = self.bin.len() + padding;
        let end = offset + data.len();
        u32::try_from(end).map_err(|_| GltfError::TooLarge {
            field: "binary buffer",
            size: end,
        })?;
        self.bin.resize(offset, 0);
        self.bin.extend_from_slice(data);
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        }));
        Ok(self.views.len() - 1)
    }

    /// Wrap one view in an accessor, returning its index.
    fn accessor(
        &mut self,
        view: usize,
        component_type: u32,
        gltf_type: &str,
        count: usize,
    ) -> usize {
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": component_type,
            "type": gltf_type,
            "count": count,
        }));
        self.accessors.len() - 1
    }
}

/// Little-endian bytes of a flat `f32` payload.
fn bytes_f32(values: impl Iterator<Item = f32>) -> Vec<u8> {
    values.flat_map(f32::to_le_bytes).collect()
}

/// Little-endian bytes of a flat `u16` payload.
fn bytes_u16(values: impl Iterator<Item = u16>) -> Vec<u8> {
    values.flat_map(u16::to_le_bytes).collect()
}

/// Little-endian bytes of a flat `u32` payload.
fn bytes_u32(values: impl Iterator<Item = u32>) -> Vec<u8> {
    values.flat_map(u32::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::{UNSIGNED_INT, Writer, bytes_u32};

    #[test]
    fn views_pad_to_four_byte_boundaries() {
        let mut writer = Writer::default();
        assert_eq!(writer.view(&[1, 2, 3]).expect("first view"), 0);
        assert_eq!(writer.view(&[4]).expect("second view"), 1);
        assert_eq!(writer.views[0]["byteOffset"].as_u64(), Some(0));
        assert_eq!(
            writer.views[1]["byteOffset"].as_u64(),
            Some(4),
            "the second view is padded"
        );
        assert_eq!(writer.bin, [1, 2, 3, 0, 4]);
    }

    #[test]
    fn an_accessor_records_its_view_component_type_and_count() {
        let mut writer = Writer::default();
        let view = writer.view(&bytes_u32([7, 9].into_iter())).expect("view");
        let accessor = writer.accessor(view, UNSIGNED_INT, "SCALAR", 2);
        assert_eq!(accessor, 0);
        assert_eq!(
            writer.accessors[0]["bufferView"].as_u64(),
            Some(view as u64)
        );
        assert_eq!(
            writer.accessors[0]["componentType"].as_u64(),
            Some(u64::from(UNSIGNED_INT))
        );
        assert_eq!(writer.accessors[0]["type"].as_str(), Some("SCALAR"));
        assert_eq!(writer.accessors[0]["count"].as_u64(), Some(2));
    }
}
