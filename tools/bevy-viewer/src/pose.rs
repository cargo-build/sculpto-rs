//! Pose mode for the review window: turn a joint by hand, watch the skin
//! follow, and read the result with your own eyes.
//!
//! The rule this module exists to keep is stated once and enforced everywhere:
//! **a pose is a rotation delta on the file's rest, never an absolute local
//! write.** `local.rotation = delta * rest.rotation`, `local.translation =
//! rest.translation`, `local.scale = rest.scale`. A rotation delta cannot move a
//! child's bone offset or inflate it, so the skin cannot detach or scale, and a
//! joint nobody touched is written back as the captured rest bits.
//!
//! `T` poses the same skeleton to a reference file's bone **directions** (the
//! Mixamo T), computed live: no bake, no artifact, no file written.

use bevy::input::mouse::MouseMotion;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{is_bone_name, ReviewModel, ViewerConfig};

/// Degrees a single arrow-key press turns the selected joint.
const ARROW_STEP: f32 = 5.0;
/// Radians per pixel of mouse drag.
const DRAG_RATE: f32 = 0.006;
/// The reference a `T` press reads directions from when the caller names no
/// other. Mixamo's own rest is an exact T-pose (arms 0.0° below horizontal),
/// which is why it is the reference and not a guess.
const DEFAULT_REFERENCE: &str = "Models/sculpto-jokes-20261115/ybot_walking.glb";

/// The file's rest pose plus the deltas a person applied on top of it.
#[derive(Resource, Default)]
pub struct PoseMode {
    /// While this is on the review window holds the file's rest with the
    /// deltas applied, whatever clip is playing underneath.
    pub active: bool,
    /// Rest local transform of every skeleton node, captured before the first
    /// animation pass. It is the only base a delta is ever composed on.
    pub rest: HashMap<Entity, Transform>,
    /// Local rotation delta per joint. A joint with no entry is untouched and
    /// is written back exactly as its captured rest.
    pub deltas: HashMap<Entity, Quat>,
    pub selected: Option<Entity>,
    /// Reference skeleton for `T`; empty when the host has none.
    pub reference: Option<PathBuf>,
    /// Set when the current deltas came from the reference, so the card can say
    /// which pose is on screen.
    pub reference_applied: bool,
    /// The one-shot `SCULPTO_VIEWER_POSE_*` seed has run.
    pub seeded: bool,
    /// Last action, for the window overlay: what the hand just did.
    pub last_action: String,
}

impl PoseMode {
    /// Seed from the environment: `T` needs a reference, and a headless capture
    /// needs to open on the pose a person would have dragged by hand.
    pub fn from_environment() -> Self {
        let reference = std::env::var("SCULPTO_VIEWER_REFERENCE")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                let home = std::env::var("HOME").ok()?;
                let path = PathBuf::from(home).join(DEFAULT_REFERENCE);
                path.exists().then_some(path)
            })
            .filter(|path| path.exists());
        Self {
            active: std::env::var("SCULPTO_VIEWER_POSE_MODE").is_ok(),
            reference,
            ..default()
        }
    }
}

/// Entities that are joints of some skin. The overlay and the pose palette both
/// mean exactly this set: a bone the skin does not drive is not a pose handle.
pub fn driven_joints(skins: &Query<&SkinnedMesh>) -> HashSet<Entity> {
    let mut joints = HashSet::new();
    for skin in skins {
        joints.extend(skin.joints.iter().copied());
    }
    joints
}

/// What the loaded skin actually carries. A viewer may only name a skeleton
/// after counting it: 126 native joints are not a Mixamo seat, and calling them
/// Mixamo is the defect that put orange spikes in front of the operator twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Census {
    pub joints: usize,
    pub mixamo: usize,
    pub native: usize,
}

impl Census {
    /// `65 / 65` is the only shape that may be called Mixamo: the seat's own
    /// palette and nothing native beside it.
    pub fn is_mixamo(&self) -> bool {
        self.joints == 65 && self.mixamo == 65 && self.native == 0
    }

    pub fn line(&self) -> String {
        let carrier = if self.mixamo == 0 {
            " · native carrier"
        } else if self.is_mixamo() {
            " · Mixamo seat"
        } else {
            " · mixed carrier"
        };
        format!(
            "skeleton: {} joints · {} mixamorig · {} native{carrier}",
            self.joints, self.mixamo, self.native
        )
    }
}

pub fn census(skins: &Query<&SkinnedMesh>, names: &Query<&Name>) -> Census {
    let mut census = Census::default();
    let mut seen = HashSet::new();
    for skin in skins {
        for joint in &skin.joints {
            if !seen.insert(*joint) {
                continue;
            }
            let Ok(name) = names.get(*joint) else {
                continue;
            };
            census.joints += 1;
            if name.as_str().starts_with("mixamorig") {
                census.mixamo += 1;
            } else {
                census.native += 1;
            }
        }
    }
    census
}

/// Which limb a palette name belongs to. `Tab` walks that limb's chain, because
/// a 3 mm bone is not a click target and a chain is what a person poses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Limb {
    Arm(char),
    Leg(char),
    Torso,
    Other,
}

pub fn side_of(name: &str) -> Option<char> {
    if name.contains("Left") {
        return Some('L');
    }
    if name.contains("Right") {
        return Some('R');
    }
    // The native palette spells the side as the bracket's first letter:
    // `NPC L UpperArm [LUar]`, `NPC R Calf [RClf]`.
    let open = name.find('[')?;
    name[open + 1..]
        .chars()
        .next()
        .filter(|side| *side == 'L' || *side == 'R')
}

pub fn limb_of(name: &str) -> Limb {
    // `Arm` covers both spellings: our `UpperArm`/`Forearm` and Mixamo's
    // `LeftArm`/`LeftForeArm`.
    let arm = ["Arm", "Hand", "Clavicle", "Shoulder"]
        .iter()
        .any(|key| name.contains(key));
    let leg = ["Thigh", "Calf", "Foot", "Toe"]
        .iter()
        .any(|key| name.contains(key));
    match (arm, leg, side_of(name)) {
        (true, _, Some(side)) => Limb::Arm(side),
        (_, true, Some(side)) => Limb::Leg(side),
        _ if name.contains("Spine")
            || name.contains("Neck")
            || name.contains("Head")
            || name.contains("Pelvis")
            || name.contains("Hips") =>
        {
            Limb::Torso
        }
        _ => Limb::Other,
    }
}

/// Canonical role a bone plays, independent of who spells it: Skyrim's
/// `NPC L UpperArm [LUar]` and Mixamo's `mixamorig:LeftArm` are one role. The
/// product owns the full palette table (`crates/sculpto-engine/src/retarget.rs`);
/// this is the subset a T-pose needs, and it lives here because the viewer is a
/// standalone crate that must not pull the engine in to draw a window.
pub fn role_of(name: &str) -> Option<(&'static str, Option<char>)> {
    const MIXAMO_SIDED: &[(&str, &str)] = &[
        ("Shoulder", "clavicle"),
        ("Arm", "upperarm"),
        ("ForeArm", "forearm"),
        ("Hand", "hand"),
        ("UpLeg", "thigh"),
        ("Leg", "calf"),
        ("Foot", "foot"),
        ("ToeBase", "toe"),
    ];
    const MIXAMO_CENTRE: &[(&str, &str)] = &[
        ("Hips", "pelvis"),
        ("Spine", "spine"),
        ("Spine1", "spine1"),
        ("Spine2", "spine2"),
        ("Neck", "neck"),
        ("Head", "head"),
    ];
    if let Some(stem) = name.strip_prefix("mixamorig") {
        let stem = stem.trim_start_matches([':', '|']);
        let (side, rest) = if let Some(rest) = stem.strip_prefix("Left") {
            (Some('L'), rest)
        } else if let Some(rest) = stem.strip_prefix("Right") {
            (Some('R'), rest)
        } else {
            (None, stem)
        };
        for (token, role) in MIXAMO_SIDED {
            if rest == *token {
                return Some((role, side));
            }
        }
        for (token, role) in MIXAMO_CENTRE {
            if rest == *token {
                return Some((role, None));
            }
        }
        return None;
    }
    // Native names read `<prefix> <side> <Token> [<bracket>]`; only an exact
    // token counts, so `UpperarmTwist1` is not `UpperArm`.
    let bare = name.split('[').next().unwrap_or(name).trim();
    let token = bare.rsplit(' ').next().unwrap_or(bare);
    const OUR_CENTRE: &[(&str, &str)] = &[
        ("Pelvis", "pelvis"),
        // Mixamo's centre bone spells the pelvis `Hips`, and a reference file
        // without its prefix still has to be readable.
        ("Hips", "pelvis"),
        ("Spine", "spine"),
        ("Spine1", "spine1"),
        ("Spine2", "spine2"),
        ("Neck", "neck"),
        ("Head", "head"),
    ];
    const OUR_SIDED: &[(&str, &str)] = &[
        ("Clavicle", "clavicle"),
        ("UpperArm", "upperarm"),
        ("Forearm", "forearm"),
        ("Hand", "hand"),
        ("Thigh", "thigh"),
        ("Calf", "calf"),
        ("Foot", "foot"),
        ("Toe0", "toe"),
        ("Toe1", "toe"),
    ];
    for (token_name, role) in OUR_CENTRE {
        if token == *token_name {
            return Some((role, None));
        }
    }
    for (token_name, role) in OUR_SIDED {
        if token == *token_name {
            return Some((role, side_of(name)));
        }
    }
    None
}

/// The child that gives a role its direction. A bone's direction is the vector
/// to its chain child, so only roles with a mapped child can be aligned.
pub const ROLE_CHILD: &[(&str, &str)] = &[
    ("spine", "spine1"),
    ("spine1", "spine2"),
    ("spine2", "neck"),
    ("neck", "head"),
    ("clavicle", "upperarm"),
    ("upperarm", "forearm"),
    ("forearm", "hand"),
    ("thigh", "calf"),
    ("calf", "foot"),
    ("foot", "toe"),
];

/// Posed local transform: the delta rotates the rest, nothing else changes.
pub fn delta_on_rest(rest: &Transform, delta: Quat) -> Transform {
    Transform {
        translation: rest.translation,
        rotation: delta.normalize() * rest.rotation,
        scale: rest.scale,
    }
}

/// Orthonormal basis from an anatomical left axis and an up axis. Both come
/// from measurements of the skeleton itself, so no yaw constant is baked in:
/// the Mixamo reference and our body are yaw-flipped from each other, and a
/// hard-coded 180° would be right for one pair and silently wrong for the next.
pub fn anatomical_frame(left: Vec3, up: Vec3) -> Option<Mat3> {
    let up = up.try_normalize()?;
    let left = (left - up * left.dot(up)).try_normalize()?;
    let forward = left.cross(up);
    Some(Mat3::from_cols(left, up, forward))
}

/// One joint the reference can drive, with everything the swing needs.
#[derive(Clone, Copy, Debug)]
pub struct SwingJoint {
    pub entity: Entity,
    pub depth: usize,
    pub parent: Option<Entity>,
    /// World rotation at the file's rest.
    pub rest_world_rotation: Quat,
    /// Local rotation at the file's rest.
    pub rest_local_rotation: Quat,
    /// Unit direction from this joint to its chain child, at rest.
    pub direction: Vec3,
    /// Where the reference skeleton points the same joint.
    pub target: Vec3,
}

/// Per-joint local delta that turns every joint's chain direction onto the
/// reference's, top to bottom: each joint's swing is computed on the direction
/// its parent has *already* moved, so a parent's rotation is never applied
/// twice. Every delta is a pure unit rotation.
pub fn solve_swing_chain(joints: &[SwingJoint]) -> Vec<(Entity, Quat)> {
    let mut ordered = joints.to_vec();
    ordered.sort_by_key(|joint| (joint.depth, joint.entity.index()));
    let mut accumulated: HashMap<Entity, Quat> = HashMap::new();
    let mut deltas = Vec::with_capacity(ordered.len());
    for joint in ordered {
        // A parent outside the mapped set still carries its own ancestors'
        // adjustment; unmapped means no adjustment of its own.
        let inherited = joint
            .parent
            .and_then(|parent| accumulated.get(&parent).copied())
            .unwrap_or(Quat::IDENTITY);
        let posed_before = (inherited * joint.direction).normalize();
        let swing = Quat::from_rotation_arc(posed_before, joint.target.normalize());
        let base = inherited * joint.rest_world_rotation;
        let local = joint.rest_local_rotation;
        let delta = (local * (base.inverse() * swing * base) * local.inverse()).normalize();
        accumulated.insert(joint.entity, (swing * inherited).normalize());
        deltas.push((joint.entity, delta));
    }
    deltas
}

/// World position and rotation of the rest pose, resolved through the captured
/// local transforms only: the animation never enters this.
pub fn rest_world(
    entity: Entity,
    rest: &HashMap<Entity, Transform>,
    parents: &Query<&ChildOf>,
    memo: &mut HashMap<Entity, (Vec3, Quat)>,
) -> (Vec3, Quat) {
    if let Some(value) = memo.get(&entity) {
        return *value;
    }
    let local = rest.get(&entity).copied().unwrap_or_default();
    let parent = parents
        .get(entity)
        .ok()
        .map(ChildOf::parent)
        .filter(|parent| rest.contains_key(parent));
    let value = match parent {
        Some(parent) => {
            let (position, rotation) = rest_world(parent, rest, parents, memo);
            (
                position + rotation * local.translation,
                rotation * local.rotation,
            )
        }
        None => (local.translation, local.rotation),
    };
    memo.insert(entity, value);
    value
}

/// Depth of an entity in the captured skeleton, for top-down ordering.
pub fn rest_depth(
    entity: Entity,
    rest: &HashMap<Entity, Transform>,
    parents: &Query<&ChildOf>,
) -> usize {
    let mut depth = 0;
    let mut current = entity;
    while let Some(parent) = parents
        .get(current)
        .ok()
        .map(ChildOf::parent)
        .filter(|parent| rest.contains_key(parent))
    {
        depth += 1;
        current = parent;
        if depth > 512 {
            break;
        }
    }
    depth
}

/// Posed world rotation of a joint, from the captured rest and the deltas.
pub fn posed_world_rotation(
    entity: Entity,
    rest: &HashMap<Entity, Transform>,
    deltas: &HashMap<Entity, Quat>,
    parents: &Query<&ChildOf>,
) -> Quat {
    let mut chain = Vec::new();
    let mut current = Some(entity);
    while let Some(node) = current {
        if !rest.contains_key(&node) {
            break;
        }
        chain.push(node);
        current = parents
            .get(node)
            .ok()
            .map(ChildOf::parent)
            .filter(|parent| rest.contains_key(parent));
        if chain.len() > 512 {
            break;
        }
    }
    let mut world = Quat::IDENTITY;
    for node in chain.iter().rev() {
        world *= deltas.get(node).copied().unwrap_or(Quat::IDENTITY) * rest[node].rotation;
    }
    world
}

/// A reference skeleton read straight from a GLB's node transforms. Only world
/// directions are used, so the file's unit (Mixamo is centimetres) does not
/// matter.
pub struct RefSkeleton {
    positions: HashMap<String, Vec3>,
    /// Nodes in sorted order, so role lookup and drawing are the same on every
    /// run.
    names: Vec<String>,
    /// World position of every joint, which is what an overlay draws.
    bones: Vec<RefBone>,
}

/// One named bone of a reference skeleton, at the file's rest.
pub struct RefBone {
    pub name: String,
    pub parent: Option<usize>,
}

impl RefSkeleton {
    pub fn load(path: &Path) -> Option<Self> {
        let json = glb_json(path)?;
        Self::from_json(&json)
    }

    pub fn from_json(json: &Value) -> Option<Self> {
        let nodes = json.get("nodes")?.as_array()?;
        let mut parents: Vec<Option<usize>> = vec![None; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            for child in node
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(child) = child.as_u64() {
                    if let Some(slot) = parents.get_mut(child as usize) {
                        *slot = Some(index);
                    }
                }
            }
        }
        let number = |list: &[Value], index: usize, default: f32| -> f32 {
            list.get(index)
                .and_then(Value::as_f64)
                .map(|value| value as f32)
                .unwrap_or(default)
        };
        // A Mixamo file puts its armature under a 0.01 scale and a +90° X
        // rotation: centimetres and Z-up inside, metres and Y-up outside.
        // Composing the local matrices is the only way to keep the arm lengths
        // and the bone offsets in one frame — a scalar fudge on the offsets
        // alone puts the hand a hand-width above the hips.
        let local = |node: &Value| -> Mat4 {
            let translation = node
                .get("translation")
                .and_then(Value::as_array)
                .map(|v| Vec3::new(number(v, 0, 0.0), number(v, 1, 0.0), number(v, 2, 0.0)))
                .unwrap_or(Vec3::ZERO);
            let rotation = node
                .get("rotation")
                .and_then(Value::as_array)
                .map(|v| {
                    Quat::from_xyzw(
                        number(v, 0, 0.0),
                        number(v, 1, 0.0),
                        number(v, 2, 0.0),
                        number(v, 3, 1.0),
                    )
                    .normalize()
                })
                .unwrap_or(Quat::IDENTITY);
            let scale = node
                .get("scale")
                .and_then(Value::as_array)
                .map(|v| Vec3::new(number(v, 0, 1.0), number(v, 1, 1.0), number(v, 2, 1.0)))
                .unwrap_or(Vec3::ONE);
            Mat4::from_scale_rotation_translation(scale, rotation, translation)
        };
        let mut positions: HashMap<String, Vec3> = HashMap::new();
        let mut bones: Vec<RefBone> = Vec::new();
        // Which node each kept bone came from. `ybot_walking.glb` carries two
        // armatures with the same bone names at different scales, so a name is
        // answered by exactly one node: the first, the one that was measured.
        let mut node_of: Vec<usize> = Vec::new();
        for (index, node) in nodes.iter().enumerate() {
            let Some(name) = node.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mut chain = vec![index];
            let mut current = index;
            while let Some(Some(parent)) = parents.get(current) {
                chain.push(*parent);
                current = *parent;
                if chain.len() > 512 {
                    break;
                }
            }
            let mut matrix = Mat4::IDENTITY;
            for node_index in chain.iter().rev() {
                matrix *= local(&nodes[*node_index]);
            }
            let position = matrix.w_axis.truncate();
            if positions.contains_key(name) {
                continue;
            }
            positions.insert(name.to_string(), position);
            bones.push(RefBone {
                name: name.to_string(),
                parent: None,
            });
            node_of.push(index);
        }
        // Resolve parents inside the kept list, so the overlay draws one
        // connected skeleton rather than a cloud of points.
        let slot: HashMap<String, usize> = bones
            .iter()
            .enumerate()
            .map(|(slot, bone)| (bone.name.clone(), slot))
            .collect();
        for (bone_slot, index) in node_of.iter().enumerate() {
            let mut current = *index;
            let mut parent_slot = None;
            while let Some(Some(parent)) = parents.get(current) {
                if let Some(parent_name) = nodes[*parent].get("name").and_then(Value::as_str) {
                    // A link only to another bone: `Armature` and the scene root
                    // are scaffolding, and a bar from one to the hips crosses
                    // the whole figure.
                    if is_bone_name(parent_name) {
                        if let Some(found) = slot.get(parent_name).copied() {
                            parent_slot = Some(found);
                            break;
                        }
                    }
                }
                current = *parent;
            }
            bones[bone_slot].parent = parent_slot;
        }
        let mut names: Vec<String> = positions.keys().cloned().collect();
        names.sort();
        Some(Self {
            positions,
            names,
            bones,
        })
    }

    pub fn position(&self, name: &str) -> Option<Vec3> {
        self.positions.get(name).copied()
    }

    /// Census of the reference file: only `65 / 65 mixamorig` may be drawn as
    /// Mixamo's skeleton.
    pub fn census(&self) -> Census {
        let mixamo = self
            .bones
            .iter()
            .filter(|bone| bone.name.starts_with("mixamorig"))
            .count();
        Census {
            joints: self.bones.len(),
            mixamo,
            native: self.bones.len() - mixamo,
        }
    }

    /// Reference bone that plays a role: the first name in sorted order whose
    /// canonical role matches, so the choice cannot change between runs.
    pub fn bone(&self, role: &str, side: Option<char>) -> Option<&String> {
        self.names.iter().find(|name| {
            role_of(name).is_some_and(|(candidate, candidate_side)| {
                candidate == role && candidate_side == side
            })
        })
    }
}

/// JSON chunk of a GLB. The loader already reads it for the clip count; pose
/// mode reads it for the reference skeleton, so a `T` needs no asset pipeline
/// and no extra file on disk.
pub fn glb_json(path: &Path) -> Option<Value> {
    glb_json_bytes(&std::fs::read(path).ok()?)
}

pub fn glb_json_bytes(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 20 || &bytes[0..4] != b"glTF" {
        return None;
    }
    let chunk_len = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let chunk_type = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    if chunk_type != 0x4E4F_534A || 20 + chunk_len > bytes.len() {
        return None;
    }
    serde_json::from_slice(&bytes[20..20 + chunk_len]).ok()
}

/// Capture the rest pose once, before the first animation pass. Every later
/// write is a delta on this, which is what keeps an untouched joint
/// bit-identical and a touched one un-scaled.
pub fn capture_rest(
    mut pose: ResMut<PoseMode>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
    skins: Query<&SkinnedMesh>,
    models: Query<(), With<ReviewModel>>,
) {
    let joints = driven_joints(&skins);
    if joints.is_empty() {
        return;
    }
    for joint in joints {
        if pose.rest.contains_key(&joint) {
            continue;
        }
        let mut current = joint;
        loop {
            if pose.rest.contains_key(&current) || models.get(current).is_ok() {
                break;
            }
            if let Ok(transform) = transforms.get(current) {
                pose.rest.insert(current, *transform);
            }
            match parents.get(current).ok().map(ChildOf::parent) {
                Some(parent) => current = parent,
                None => break,
            }
        }
    }
}

/// Write the pose to the transforms the skin reads. Runs after the animation
/// pass and before propagation, so what is computed here is what renders.
pub fn apply_pose(pose: Res<PoseMode>, mut transforms: Query<&mut Transform>) {
    if !pose.active {
        return;
    }
    for (entity, rest) in pose.rest.iter() {
        let delta = pose.deltas.get(entity).copied().unwrap_or(Quat::IDENTITY);
        let Ok(mut transform) = transforms.get_mut(*entity) else {
            continue;
        };
        // Untouched joints are written back as the captured rest bits: no
        // arithmetic, so no drift can creep in over a session.
        *transform = delta_on_rest(rest, delta);
    }
}

/// Local delta that turns a joint about world axes (the camera's axes in the
/// review window). A world rotation is expressed in the parent's frame first,
/// so the result reads as a rotation of that joint and nothing else.
pub fn rotate_about_world(
    pose: &mut PoseMode,
    entity: Entity,
    world: Quat,
    parents: &Query<&ChildOf>,
) -> Quat {
    let parent = parents
        .get(entity)
        .ok()
        .map(ChildOf::parent)
        .filter(|parent| pose.rest.contains_key(parent));
    let parent_world = parent
        .map(|parent| posed_world_rotation(parent, &pose.rest, &pose.deltas, parents))
        .unwrap_or(Quat::IDENTITY);
    let delta = pose.deltas.get(&entity).copied().unwrap_or(Quat::IDENTITY);
    let next = (parent_world.inverse() * world * parent_world * delta).normalize();
    pose.deltas.insert(entity, next);
    next
}

/// Local delta that turns a joint about its own axes.
pub fn rotate_about_self(pose: &mut PoseMode, entity: Entity, local: Quat) -> Quat {
    let delta = pose.deltas.get(&entity).copied().unwrap_or(Quat::IDENTITY);
    let next = (delta * local).normalize();
    pose.deltas.insert(entity, next);
    next
}

/// What a `T` press measured, for the log and the overlay.
pub struct ReferencePose {
    pub chains: usize,
    pub yaw_degrees: f32,
    pub left_arm_below: Option<f32>,
    pub right_arm_below: Option<f32>,
}

impl ReferencePose {
    pub fn summary(&self, path: &Path, label: &str) -> String {
        let arm = |value: Option<f32>| match value {
            Some(degrees) => format!("{degrees:+.1}°"),
            None => "n/a".to_string(),
        };
        format!(
            "{label} from {}: {} chains, frame yaw {:+.1}°, arms {} / {} below horizontal",
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            self.chains,
            self.yaw_degrees,
            arm(self.left_arm_below),
            arm(self.right_arm_below),
        )
    }
}

/// One entity per canonical role. A duplicate palette (the zero-weight `CME`
/// twins) is not a second answer: the bone the body is named on wins.
pub fn role_entities(
    pose: &PoseMode,
    names: &Query<&Name>,
) -> HashMap<(&'static str, Option<char>), Entity> {
    let rank = |name: &str| -> u8 {
        if name.starts_with("NPC ") {
            2
        } else if name.starts_with("CME ") {
            0
        } else {
            1
        }
    };
    let mut by_role: HashMap<(&'static str, Option<char>), Entity> = HashMap::new();
    for entity in pose.rest.keys() {
        let Ok(name) = names.get(*entity) else {
            continue;
        };
        if !is_bone_name(name.as_str()) {
            continue;
        }
        let Some(role) = role_of(name.as_str()) else {
            continue;
        };
        let better = by_role
            .get(&role)
            .and_then(|current| names.get(*current).ok())
            .map(|current| rank(name.as_str()) > rank(current.as_str()))
            .unwrap_or(true);
        if better {
            by_role.insert(role, *entity);
        }
    }
    by_role
}

/// Rest world transform of every captured joint. The animation never enters
/// this: it is the file's rest, resolved through the captured locals.
pub fn rest_grid(pose: &PoseMode, parents: &Query<&ChildOf>) -> HashMap<Entity, (Vec3, Quat)> {
    let mut memo: HashMap<Entity, (Vec3, Quat)> = HashMap::new();
    let mut grid = HashMap::new();
    for entity in pose.rest.keys() {
        grid.insert(*entity, rest_world(*entity, &pose.rest, parents, &mut memo));
    }
    grid
}

/// A body frame from the legs: the two thigh offsets say which way the body
/// faces, thigh -> calf says which way is up. Our native pelvis -> spine
/// direction is ~45° off, so it is not a source.
/// A joint's own position and its chain child's, per role and side.
type SidedLookup<'a> = dyn Fn(&str, Option<char>) -> Option<(Vec3, Vec3)> + 'a;

fn body_frame(sided: &SidedLookup) -> Option<Mat3> {
    let (left_thigh, left_calf) = sided("thigh", Some('L'))?;
    let (right_thigh, _) = sided("thigh", Some('R'))?;
    let up = (left_thigh - left_calf).try_normalize()?;
    anatomical_frame(left_thigh - right_thigh, up)
}

fn role_child(role: &str) -> Option<&'static str> {
    ROLE_CHILD
        .iter()
        .find(|(parent, _)| *parent == role)
        .map(|(_, child)| *child)
}

/// The similarity that puts a reference skeleton onto our body: yaw and tilt
/// from the two flesh frames, scale from the pelvis -> head length, hip
/// alignment from the pelvis. It is the construction the seat uses, so the
/// overlay and the `T` cannot disagree about where the reference sits.
#[derive(Clone, Copy, Debug)]
pub struct BodyAlign {
    /// The actor's own body frame; `y_axis` is its measured up.
    pub frame: Mat3,
    pub rotation: Mat3,
    pub scale: f32,
    pub reference_origin: Vec3,
    pub actor_origin: Vec3,
}

impl BodyAlign {
    pub fn point(&self, point: Vec3) -> Vec3 {
        self.actor_origin + (self.rotation * (point - self.reference_origin)) * self.scale
    }

    pub fn direction(&self, direction: Vec3) -> Vec3 {
        (self.rotation * direction).normalize()
    }

    pub fn yaw_degrees(&self) -> f32 {
        self.rotation
            .to_euler(bevy::math::EulerRot::YXZ)
            .0
            .to_degrees()
    }

    /// Length scale between the two bodies, measured on one shared span.
    pub fn scale(&self) -> f32 {
        self.scale
    }
}

pub fn body_align(
    pose: &PoseMode,
    reference: &RefSkeleton,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
) -> Result<BodyAlign, String> {
    let by_role = role_entities(pose, names);
    let grid = rest_grid(pose, parents);
    let actor_frame = body_frame(&|role, side| {
        let bone = *by_role.get(&(role, side))?;
        let child = *by_role.get(&(role_child(role)?, side))?;
        Some((grid.get(&bone)?.0, grid.get(&child)?.0))
    })
    .ok_or_else(|| "actor has no left/right thigh pair to build a body frame".to_string())?;
    let reference_frame = body_frame(&|role, side| {
        let bone = reference.bone(role, side)?;
        let child = reference.bone(role_child(role)?, side)?;
        Some((reference.position(bone)?, reference.position(child)?))
    })
    .ok_or_else(|| "reference has no left/right thigh pair to build a body frame".to_string())?;
    let rotation = actor_frame * reference_frame.transpose();
    // Height as two spans a humanoid actually carries: pelvis -> head plus
    // pelvis -> toe. A single head-top bone would be the exact measure, and
    // neither palette has one.
    let height =
        |pelvis: Vec3, head: Vec3, toe: Vec3| (head - pelvis).length() + (pelvis - toe).length();
    let actor_origin = *by_role
        .get(&("pelvis", None))
        .and_then(|entity| grid.get(entity))
        .map(|(position, _)| position)
        .ok_or_else(|| "actor has no pelvis to align the reference to".to_string())?;
    let actor_head = *by_role
        .get(&("head", None))
        .and_then(|entity| grid.get(entity))
        .map(|(position, _)| position)
        .ok_or_else(|| "actor has no head to measure the reference against".to_string())?;
    let actor_toe = by_role
        .get(&("toe", Some('L')))
        .and_then(|entity| grid.get(entity))
        .map(|(position, _)| *position)
        .unwrap_or(actor_origin - (actor_head - actor_origin));
    let reference_origin = reference
        .bone("pelvis", None)
        .and_then(|name| reference.position(name))
        .ok_or_else(|| "reference has no hips".to_string())?;
    let reference_head = reference
        .bone("head", None)
        .and_then(|name| reference.position(name))
        .ok_or_else(|| "reference has no head".to_string())?;
    let reference_toe = reference
        .bone("toe", Some('L'))
        .and_then(|name| reference.position(name))
        .unwrap_or(reference_origin - (reference_head - reference_origin));
    let actor_span = height(actor_origin, actor_head, actor_toe);
    let reference_span = height(reference_origin, reference_head, reference_toe);
    if reference_span <= f32::EPSILON {
        return Err("reference pelvis and head are the same point".to_string());
    }
    Ok(BodyAlign {
        frame: actor_frame,
        rotation,
        scale: actor_span / reference_span,
        reference_origin,
        actor_origin,
    })
}

/// Pose the captured skeleton to a reference file's directions.
///
/// Refuses rather than guesses when a body frame cannot be measured: a
/// reference with no legs cannot say which way the body faces, and that guess
/// is how a T-pose ends up with crossed arms.
pub fn apply_reference(
    pose: &mut PoseMode,
    reference_path: &Path,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
) -> Result<ReferencePose, String> {
    let reference = RefSkeleton::load(reference_path).ok_or_else(|| {
        format!(
            "reference {} is not a readable GLB",
            reference_path.display()
        )
    })?;
    let align = body_align(pose, &reference, names, parents)?;
    let by_role = role_entities(pose, names);
    let grid = rest_grid(pose, parents);
    if std::env::var("SCULPTO_VIEWER_POSE_DEBUG").is_ok() {
        info!(
            "viewer: align yaw={:+.2}° scale={:.4} origin={:?}",
            align.yaw_degrees(),
            align.scale(),
            align.actor_origin
        );
    }

    let mut joints = Vec::new();
    for ((role, side), entity) in by_role.iter() {
        let Some((_, child_role)) = ROLE_CHILD.iter().find(|(parent, _)| parent == role) else {
            continue;
        };
        let Some(child) = by_role.get(&(child_role, *side)) else {
            continue;
        };
        let (Some(bone), Some(reference_child)) = (
            reference.bone(role, *side),
            reference.bone(child_role, *side),
        ) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            grid.get(entity).map(|(position, _)| position),
            grid.get(child).map(|(position, _)| position),
        ) else {
            continue;
        };
        let (Some(reference_start), Some(reference_end)) = (
            reference.position(bone),
            reference.position(reference_child),
        ) else {
            continue;
        };
        let (Some(direction), Some(reference_direction)) = (
            (*end - *start).try_normalize(),
            (reference_end - reference_start).try_normalize(),
        ) else {
            continue;
        };
        let (Some(rest_world_rotation), Some(rest_local_rotation)) = (
            grid.get(entity).map(|(_, rotation)| *rotation),
            pose.rest.get(entity).map(|rest| rest.rotation),
        ) else {
            continue;
        };
        joints.push(SwingJoint {
            entity: *entity,
            depth: rest_depth(*entity, &pose.rest, parents),
            parent: parents
                .get(*entity)
                .ok()
                .map(ChildOf::parent)
                .filter(|parent| pose.rest.contains_key(parent)),
            rest_world_rotation,
            rest_local_rotation,
            direction,
            target: align.direction(reference_direction),
        });
    }
    if joints.is_empty() {
        return Err("no joint is spelled by both palettes".to_string());
    }
    let deltas = solve_swing_chain(&joints);
    pose.deltas.clear();
    for (entity, delta) in deltas {
        pose.deltas.insert(entity, delta);
    }
    pose.active = true;
    pose.reference_applied = true;
    let yaw = align.yaw_degrees();
    // "Horizontal" is the actor's own measured up axis, not world +Y: the
    // pipeline hands us both Y-up and Z-up actor files, and a world-Y measure
    // would report a lie for one of them.
    let up = align.frame.y_axis;
    let below = |role: &str, side: char| -> Option<f32> {
        let joint = joints.iter().find(|joint| {
            names
                .get(joint.entity)
                .ok()
                .and_then(|name| role_of(name.as_str()))
                .is_some_and(|(candidate, candidate_side)| {
                    candidate == role && candidate_side == Some(side)
                })
        })?;
        Some(-joint.target.dot(up).asin().to_degrees())
    };
    if std::env::var("SCULPTO_VIEWER_POSE_DEBUG").is_ok() {
        // The "before" witness: the same measure the product's bake reports, so
        // a live T and a baked T can be compared number to number.
        let at_rest = |role: &str, side: char| -> Option<f32> {
            let joint = joints.iter().find(|joint| {
                names
                    .get(joint.entity)
                    .ok()
                    .and_then(|name| role_of(name.as_str()))
                    .is_some_and(|(candidate, candidate_side)| {
                        candidate == role && candidate_side == Some(side)
                    })
            })?;
            Some(-joint.direction.dot(up).asin().to_degrees())
        };
        info!(
            "viewer: arms at rest {:+.2}° / {:+.2}° below horizontal, target {:+.2}° / {:+.2}°",
            at_rest("upperarm", 'L').unwrap_or(f32::NAN),
            at_rest("upperarm", 'R').unwrap_or(f32::NAN),
            below("upperarm", 'L').unwrap_or(f32::NAN),
            below("upperarm", 'R').unwrap_or(f32::NAN),
        );
    }
    Ok(ReferencePose {
        chains: joints.len(),
        yaw_degrees: yaw,
        left_arm_below: below("upperarm", 'L'),
        right_arm_below: below("upperarm", 'R'),
    })
}

/// `P` enters and leaves pose mode, `T` applies the Mixamo reference, `Tab`
/// walks the selected limb, `R` / `Shift+R` reset one joint or all of them, and
/// the arrow keys turn the selected joint in 5° steps.
pub fn pose_keys(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut pose: ResMut<PoseMode>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    skins: Query<&SkinnedMesh>,
) {
    if !config.interactive {
        return;
    }
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if keys.just_pressed(KeyCode::KeyP) {
        pose.active = !pose.active;
        pose.last_action = if pose.active {
            "pose mode on".to_string()
        } else {
            "pose mode off".to_string()
        };
    }
    if keys.just_pressed(KeyCode::KeyT) {
        // The census decides the name: a native 126-joint carrier is not Mixamo,
        // and a T applied to it is a T toward the reference's directions.
        let census = census(&skins, &names);
        let label = if census.is_mixamo() {
            "Mixamo T".to_string()
        } else {
            format!("T (reference directions on {})", census.line())
        };
        let Some(reference) = pose.reference.clone() else {
            pose.last_action = "no reference (set SCULPTO_VIEWER_REFERENCE)".to_string();
            return;
        };
        match apply_reference(&mut pose, &reference, &names, &parents) {
            Ok(report) => {
                pose.last_action = report.summary(&reference, &label);
                info!("viewer: {}", pose.last_action);
            }
            Err(reason) => {
                pose.last_action = format!("T refused: {reason}");
                warn!("viewer: T refused: {reason}");
            }
        }
    }
    if !pose.active {
        return;
    }
    if keys.just_pressed(KeyCode::Tab) {
        let chain = limb_chain(pose.selected, &names, &parents, &skins, &pose.rest);
        if chain.is_empty() {
            pose.last_action = "no poseable joint yet".to_string();
        } else {
            let next = match pose.selected {
                None => chain[0],
                Some(current) => match chain.iter().position(|entity| *entity == current) {
                    Some(index) => {
                        let step = if shift { chain.len() - 1 } else { 1 };
                        chain[(index + step) % chain.len()]
                    }
                    None => chain[0],
                },
            };
            pose.selected = Some(next);
            pose.last_action = names
                .get(next)
                .map(|name| format!("selected {}", name.as_str()))
                .unwrap_or_else(|_| "selected".to_string());
        }
    }
    if keys.just_pressed(KeyCode::KeyR) {
        match (shift, pose.selected) {
            (true, _) => {
                pose.deltas.clear();
                pose.reference_applied = false;
                pose.last_action = "all joints back to the file rest".to_string();
            }
            (false, Some(selected)) => {
                pose.deltas.remove(&selected);
                pose.reference_applied = false;
                pose.last_action = names
                    .get(selected)
                    .map(|name| format!("reset {}", name.as_str()))
                    .unwrap_or_else(|_| "reset joint".to_string());
            }
            (false, None) => {
                pose.last_action = "no joint selected: Tab picks one".to_string();
            }
        }
    }
    let mut world = Quat::IDENTITY;
    let mut own = Quat::IDENTITY;
    if keys.just_pressed(KeyCode::ArrowLeft) {
        world *= Quat::from_axis_angle(Vec3::Y, ARROW_STEP.to_radians());
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        world *= Quat::from_axis_angle(Vec3::Y, -ARROW_STEP.to_radians());
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        own *= Quat::from_rotation_x(ARROW_STEP.to_radians());
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        own *= Quat::from_rotation_x(-ARROW_STEP.to_radians());
    }
    let Some(selected) = pose.selected else {
        return;
    };
    if world != Quat::IDENTITY {
        rotate_about_world(&mut pose, selected, world, &parents);
        pose.last_action = names
            .get(selected)
            .map(|name| format!("turned {} about the camera axes", name.as_str()))
            .unwrap_or_else(|_| "turned joint".to_string());
    }
    if own != Quat::IDENTITY {
        rotate_about_self(&mut pose, selected, own);
        pose.last_action = names
            .get(selected)
            .map(|name| format!("turned {} about its own axes", name.as_str()))
            .unwrap_or_else(|_| "turned joint".to_string());
    }
}

/// The chain `Tab` walks: the selected joint's limb, ordered by depth so the
/// walk reads clavicle -> hand instead of palette order.
pub fn limb_chain(
    selected: Option<Entity>,
    names: &Query<&Name>,
    parents: &Query<&ChildOf>,
    skins: &Query<&SkinnedMesh>,
    rest: &HashMap<Entity, Transform>,
) -> Vec<Entity> {
    let joints = driven_joints(skins);
    let limb = selected
        .and_then(|entity| names.get(entity).ok())
        .map(|name| limb_of(name.as_str()));
    let mut chain: Vec<(usize, Entity)> = joints
        .iter()
        .filter(|entity| rest.contains_key(entity))
        .filter_map(|entity| {
            let name = names.get(*entity).ok()?;
            if !is_bone_name(name.as_str()) {
                return None;
            }
            match limb {
                Some(limb) if limb_of(name.as_str()) != limb => None,
                _ => Some((rest_depth(*entity, rest, parents), *entity)),
            }
        })
        .collect();
    chain.sort_by_key(|(depth, entity)| (*depth, entity.index()));
    chain.into_iter().map(|(_, entity)| entity).collect()
}

/// Mouse drag: with pose mode on, LMB turns the selected joint about the
/// camera's axes and Shift+LMB about the joint's own axes. Orbiting moves to
/// RMB while the mode holds the left button.
pub fn pose_drag(
    config: Res<ViewerConfig>,
    mut pose: ResMut<PoseMode>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut motion: MessageReader<MouseMotion>,
    cameras: Query<&Transform, With<Camera3d>>,
    parents: Query<&ChildOf>,
) {
    // Ctrl still turns the whole figure and Shift is the joint's own axes, so
    // only a bare left drag belongs to the joint.
    if !config.interactive
        || !pose.active
        || keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
    {
        return;
    }
    let mut delta = Vec2::ZERO;
    for event in motion.read() {
        delta += event.delta;
    }
    if delta == Vec2::ZERO || !buttons.pressed(MouseButton::Left) {
        return;
    }
    let Some(selected) = pose.selected else {
        return;
    };
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        let local = Quat::from_rotation_y(-delta.x * DRAG_RATE)
            * Quat::from_rotation_x(-delta.y * DRAG_RATE);
        rotate_about_self(&mut pose, selected, local);
    } else {
        let Ok(camera) = cameras.single() else {
            return;
        };
        let world = Quat::from_axis_angle(Vec3::Y, -delta.x * DRAG_RATE)
            * Quat::from_axis_angle(camera.rotation * Vec3::X, -delta.y * DRAG_RATE);
        rotate_about_world(&mut pose, selected, world, &parents);
    }
}

/// One-shot environment seed, so a headless capture opens on the pose a person
/// would have dragged by hand: `SCULPTO_VIEWER_POSE_JOINT`,
/// `SCULPTO_VIEWER_POSE_TPOSE=1`, `SCULPTO_VIEWER_POSE_ROTATE=x,y,z` degrees
/// about the camera axes.
#[allow(clippy::too_many_arguments)]
pub fn pose_seed(
    mut pose: ResMut<PoseMode>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    skins: Query<&SkinnedMesh>,
    cameras: Query<&Transform, (With<Camera3d>, Without<Name>)>,
) {
    if pose.seeded || pose.rest.is_empty() {
        return;
    }
    pose.seeded = true;
    if let Ok(spec) = std::env::var("SCULPTO_VIEWER_POSE_JOINT") {
        pose.selected = pose
            .rest
            .keys()
            .find(|entity| {
                names
                    .get(**entity)
                    .is_ok_and(|name| name.as_str().contains(&spec))
            })
            .copied();
    }
    if std::env::var("SCULPTO_VIEWER_POSE_TPOSE").is_ok() {
        let census = census(&skins, &names);
        let label = if census.is_mixamo() {
            "Mixamo T".to_string()
        } else {
            format!("T (reference directions on {})", census.line())
        };
        if let Some(reference) = pose.reference.clone() {
            match apply_reference(&mut pose, &reference, &names, &parents) {
                Ok(report) => {
                    pose.last_action = report.summary(&reference, &label);
                    info!("viewer: {}", pose.last_action);
                }
                Err(reason) => warn!("viewer: T refused: {reason}"),
            }
        }
    }
    if let Ok(spec) = std::env::var("SCULPTO_VIEWER_POSE_ROTATE") {
        let angles: Vec<f32> = spec
            .split(',')
            .filter_map(|value| value.trim().parse().ok())
            .collect();
        if angles.len() == 3 {
            if let (Some(selected), Ok(camera)) = (pose.selected, cameras.single()) {
                pose.active = true;
                let world =
                    Quat::from_axis_angle(camera.rotation * Vec3::X, angles[0].to_radians())
                        * Quat::from_axis_angle(camera.rotation * Vec3::Y, angles[1].to_radians())
                        * Quat::from_axis_angle(camera.rotation * Vec3::Z, angles[2].to_radians());
                rotate_about_world(&mut pose, selected, world, &parents);
                pose.last_action = format!("seeded rotation {spec}");
            }
        }
    }
    info!(
        "viewer: pose seeded active={} rest_joints={} deltas={} selected={}",
        pose.active,
        pose.rest.len(),
        pose.deltas.len(),
        pose.selected
            .and_then(|entity| names.get(entity).ok())
            .map(|name| name.as_str().to_string())
            .unwrap_or_else(|| "none".to_string()),
    );
}

/// The selected joint's marker: a bright sphere at the joint and its bone name
/// drawn next to where it projects on screen. The name is what makes the marker
/// usable — the bone itself is 3 mm wide.
#[derive(Component)]
pub struct PoseMarker;

#[derive(Component)]
pub struct PoseLabel;

/// Live pose state for the operator: ON/off, delta count, which pose, joint.
#[derive(Component)]
pub struct PoseStatus;

/// The unmissable banner: pose mode takes the left button, so entering it has to
/// be visible without reading a card.
#[derive(Component)]
pub struct PoseBanner;

pub fn spawn_overlay(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(0.014))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.1, 0.85),
            unlit: true,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        PoseMarker,
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.35, 0.9)),
        PoseLabel,
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(30.0),
            right: Val::Percent(30.0),
            top: Val::Px(10.0),
            justify_content: JustifyContent::Center,
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::srgba(0.55, 0.02, 0.02, 0.85)),
        Text::new("POSE MODE ON — drag turns a joint — P to exit"),
        TextFont {
            font_size: bevy::text::FontSize::Px(17.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.9, 0.2)),
        PoseBanner,
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(12.0),
            top: Val::Px(12.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Text::new(""),
        TextFont {
            font_size: bevy::text::FontSize::Px(13.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.55, 0.25)),
        PoseStatus,
    ));

    // The reference skeleton: a distinct colour from our own overlay (our bones
    // are cyan and our joints yellow), drawn at Mixamo's alignment onto this
    // body. Pools, because a 65-bone reference must not spawn per frame.
    let joint_mesh = meshes.add(Sphere::new(0.006));
    let line_mesh = meshes.add(Cylinder::new(0.0022, 1.0));
    let octahedral = meshes.add(Cone::new(1.0, 1.0));
    let reference_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.35, 0.05),
        unlit: true,
        cull_mode: None,
        ..default()
    });
    let joint_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.75, 0.15),
        unlit: true,
        ..default()
    });
    let mut pool = RefPool::default();
    for _ in 0..REF_BONES {
        pool.joints.push(
            commands
                .spawn((
                    Mesh3d(joint_mesh.clone()),
                    MeshMaterial3d(joint_material.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    RefJoint,
                ))
                .id(),
        );
        pool.bones.push(
            commands
                .spawn((
                    Mesh3d(line_mesh.clone()),
                    MeshMaterial3d(reference_material.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    RefBoneMarker,
                ))
                .id(),
        );
    }
    for _ in 0..REF_LABELS {
        pool.labels.push(
            commands
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        display: Display::None,
                        ..default()
                    },
                    Text::new(""),
                    TextFont {
                        font_size: bevy::text::FontSize::Px(11.0),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 0.62, 0.2)),
                    RefLabel,
                ))
                .id(),
        );
    }
    commands.insert_resource(pool);
    commands.insert_resource(RefMeshes {
        line: Some(line_mesh),
        octahedral: Some(octahedral),
    });
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn pose_marker(
    config: Res<ViewerConfig>,
    pose: Res<PoseMode>,
    names: Query<&Name>,
    joints: Query<&GlobalTransform>,
    cameras: Query<(&Camera, &GlobalTransform), (With<Camera3d>, Without<PoseMarker>)>,
    mut marker: Query<(&mut Transform, &mut Visibility), With<PoseMarker>>,
    mut label: Query<
        (&mut Node, &mut Text),
        (With<PoseLabel>, Without<PoseMarker>, Without<PoseStatus>),
    >,
    mut status: Query<&mut Text, (With<PoseStatus>, Without<PoseLabel>, Without<PoseMarker>)>,
    mut banner: Query<&mut Node, (With<PoseBanner>, Without<PoseLabel>, Without<PoseMarker>)>,
    skins: Query<&SkinnedMesh>,
    reference: Res<RefReport>,
    reference_style: Res<ReferenceOverlay>,
) {
    if !config.interactive {
        return;
    }
    for mut node in banner.iter_mut() {
        node.display = if pose.active {
            Display::Flex
        } else {
            Display::None
        };
    }
    let selected = pose.selected.filter(|_| pose.active);
    let world =
        selected.and_then(|entity| joints.get(entity).ok().map(GlobalTransform::translation));
    for (mut transform, mut visibility) in marker.iter_mut() {
        match world {
            Some(position) => {
                transform.translation = position;
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    let projected = world.and_then(|position| {
        let (camera, transform) = cameras.single().ok()?;
        camera.world_to_viewport(transform, position).ok()
    });
    for (mut node, mut text) in label.iter_mut() {
        match (selected, projected) {
            (Some(entity), Some(point)) => {
                node.display = Display::Flex;
                node.left = Val::Px(point.x + 12.0);
                node.top = Val::Px(point.y - 8.0);
                let name = names
                    .get(entity)
                    .map(|name| name.as_str().to_string())
                    .unwrap_or_default();
                if text.0 != name {
                    text.0 = name;
                }
            }
            _ => node.display = Display::None,
        }
    }
    let joint = selected
        .and_then(|entity| names.get(entity).ok())
        .map(|name| name.as_str().to_string())
        .unwrap_or_else(|| "none".to_string());
    let pose_name = if pose.reference_applied {
        // Never "Mixamo" on the strength of a key press: the census says what
        // the file carries, and a native carrier gets a neutral name.
        "T from reference"
    } else if pose.deltas.is_empty() {
        "file rest"
    } else {
        "hand pose"
    };
    let skeleton_census = census(&skins, &names);
    let reference_line = if reference.shown {
        format!(
            "{} · Δ max {:.0} mm · mean {:.0} mm · {}",
            reference.status,
            reference.max_mm,
            reference.mean_mm,
            match reference_style.style {
                BoneStyle::Lines => "dots and lines",
                BoneStyle::Mixamo => "octahedral",
            }
        )
    } else if reference.status.is_empty() {
        "reference: off (Y)".to_string()
    } else {
        reference.status.clone()
    };
    let line = format!(
        "pose {} · {} joint(s) moved · {pose_name} · {joint}\n{}\n{reference_line}\n{}",
        if pose.active { "ON" } else { "off" },
        pose.deltas.len(),
        pose.last_action,
        skeleton_census.line(),
    );
    for mut text in status.iter_mut() {
        if text.0 != line {
            text.0 = line.clone();
        }
    }
}

/// Which way a bone is drawn. `Lines` is the review default that a lane reads as
/// "dots and bars"; `Mixamo` is the octahedral bone Mixamo's own views use. The
/// drawing is a decision about how to read the same joints, not a rig change.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum BoneStyle {
    #[default]
    Lines,
    Mixamo,
}

/// The reference skeleton drawn over ours, and how its bones are drawn.
#[derive(Resource)]
pub struct ReferenceOverlay {
    pub on: bool,
    pub style: BoneStyle,
}

impl Default for ReferenceOverlay {
    fn default() -> Self {
        Self {
            on: std::env::var("SCULPTO_VIEWER_REFERENCE_OVERLAY").is_ok(),
            style: match std::env::var("SCULPTO_VIEWER_BONES").ok().as_deref() {
                Some("mixamo") => BoneStyle::Mixamo,
                _ => BoneStyle::Lines,
            },
        }
    }
}

/// Parsed reference skeleton plus the similarity onto our body, cached: parsing
/// a 2 MB GLB per frame would make the overlay cost more than the render.
#[derive(Resource, Default)]
pub struct RefCache {
    pub path: Option<PathBuf>,
    pub skeleton: Option<RefSkeleton>,
    pub align: Option<BodyAlign>,
}

/// Matched-pair distances, in millimetres, for the window card.
#[derive(Resource, Default)]
pub struct RefReport {
    pub pairs: usize,
    pub max_mm: f32,
    pub mean_mm: f32,
    pub shown: bool,
    /// Why nothing is drawn, when nothing is drawn.
    pub status: String,
}

/// Fixed pools, so nothing is spawned per frame.
#[derive(Resource, Default)]
pub struct RefPool {
    pub joints: Vec<Entity>,
    pub bones: Vec<Entity>,
    pub labels: Vec<Entity>,
}

/// Meshes the pools swap between: the review sphere/cylinder and the
/// octahedral bone.
#[derive(Resource, Default)]
pub struct RefMeshes {
    pub line: Option<Handle<Mesh>>,
    pub octahedral: Option<Handle<Mesh>>,
}

#[derive(Component)]
pub struct RefJoint;

#[derive(Component)]
pub struct RefBoneMarker;

#[derive(Component)]
pub struct RefLabel;

/// 160 bones covers Mixamo's 65 with room for a fuller reference; the extra
/// pooled entities stay hidden.
const REF_BONES: usize = 160;
const REF_LABELS: usize = 24;

/// `Y` draws the reference skeleton over ours, `M` switches between dots+lines
/// and Mixamo's octahedral bones.
pub fn reference_keys(
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut overlay: ResMut<ReferenceOverlay>,
    mut pose: ResMut<PoseMode>,
) {
    if !config.interactive {
        return;
    }
    if keys.just_pressed(KeyCode::KeyY) {
        overlay.on = !overlay.on;
        pose.last_action = if overlay.on {
            "reference overlay on".to_string()
        } else {
            "reference overlay off".to_string()
        };
    }
    if keys.just_pressed(KeyCode::KeyM) {
        overlay.style = match overlay.style {
            BoneStyle::Lines => BoneStyle::Mixamo,
            BoneStyle::Mixamo => BoneStyle::Lines,
        };
        pose.last_action = match overlay.style {
            BoneStyle::Lines => "bones: dots and lines".to_string(),
            BoneStyle::Mixamo => "bones: Mixamo octahedral".to_string(),
        };
    }
}

/// Draw the reference skeleton over ours, on the similarity the seat uses.
///
/// Only the *matched* joints are drawn — the pairs whose distance is printed —
/// and only when both files answer the census `65 / 65 mixamorig`. A path that
/// cannot say what it renders renders nothing: unlabelled geometry is how a
/// native rig got shown as "Mixamo" twice.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw_reference(
    overlay: Res<ReferenceOverlay>,
    pose: Res<PoseMode>,
    mut cache: ResMut<RefCache>,
    mut report: ResMut<RefReport>,
    pool: Res<RefPool>,
    meshes: Res<RefMeshes>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    skins: Query<&SkinnedMesh>,
    models: Query<&GlobalTransform, With<ReviewModel>>,
    mut transforms: Query<&mut Transform>,
    mut visibilities: Query<&mut Visibility>,
    mut nodes: Query<&mut Node, With<RefLabel>>,
    mut mesh_handles: Query<&mut Mesh3d, With<RefBoneMarker>>,
    mut debugged: Local<bool>,
) {
    let hide = |transforms: &mut Query<&mut Transform>,
                visibilities: &mut Query<&mut Visibility>,
                nodes: &mut Query<&mut Node, With<RefLabel>>| {
        for entity in pool.joints.iter().chain(pool.bones.iter()) {
            if let Ok(mut visibility) = visibilities.get_mut(*entity) {
                *visibility = Visibility::Hidden;
            }
        }
        for entity in &pool.labels {
            if let Ok(mut node) = nodes.get_mut(*entity) {
                node.display = Display::None;
            }
        }
        let _ = transforms;
    };
    if !overlay.on || pose.rest.is_empty() {
        hide(&mut transforms, &mut visibilities, &mut nodes);
        report.shown = false;
        report.status = "reference: off (Y)".to_string();
        return;
    }
    let actor_census = census(&skins, &names);
    if !actor_census.is_mixamo() {
        hide(&mut transforms, &mut visibilities, &mut nodes);
        report.shown = false;
        report.status = "reference: unavailable — the loaded file is a native carrier".to_string();
        return;
    }
    let Some(reference_path) = pose.reference.clone() else {
        hide(&mut transforms, &mut visibilities, &mut nodes);
        report.shown = false;
        report.status = "reference: unavailable — set SCULPTO_VIEWER_REFERENCE".to_string();
        return;
    };
    if cache.path.as_ref() != Some(&reference_path) || cache.align.is_none() {
        match RefSkeleton::load(&reference_path) {
            Some(skeleton) => {
                let reference_census = skeleton.census();
                if !reference_census.is_mixamo() {
                    cache.skeleton = None;
                    cache.align = None;
                    hide(&mut transforms, &mut visibilities, &mut nodes);
                    report.shown = false;
                    report.status = format!(
                        "reference: unavailable — {} is {}",
                        reference_path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        reference_census.line()
                    );
                    return;
                }
                match body_align(&pose, &skeleton, &names, &parents) {
                    Ok(align) => {
                        cache.skeleton = Some(skeleton);
                        cache.align = Some(align);
                        cache.path = Some(reference_path);
                    }
                    Err(reason) => {
                        hide(&mut transforms, &mut visibilities, &mut nodes);
                        report.shown = false;
                        report.status = format!("reference: unavailable — {reason}");
                        return;
                    }
                }
            }
            None => {
                hide(&mut transforms, &mut visibilities, &mut nodes);
                report.shown = false;
                report.status = format!(
                    "reference: unavailable — {} is not a readable GLB",
                    reference_path.display()
                );
                return;
            }
        }
    }
    let (Some(skeleton), Some(align)) = (cache.skeleton.as_ref(), cache.align) else {
        return;
    };
    let by_role = role_entities(&pose, &names);
    // Matched pairs only: the same joints whose millimetres are printed.
    let mut segments: Vec<(Vec3, Option<Vec3>)> = Vec::new();
    for (role, side) in by_role.keys() {
        let Some(child_role) = role_child(role) else {
            continue;
        };
        let (Some(bone), Some(child)) =
            (skeleton.bone(role, *side), skeleton.bone(child_role, *side))
        else {
            continue;
        };
        let (Some(start), Some(end)) = (skeleton.position(bone), skeleton.position(child)) else {
            continue;
        };
        segments.push((start, Some(end)));
    }
    if segments.is_empty() {
        hide(&mut transforms, &mut visibilities, &mut nodes);
        report.shown = false;
        report.status = "reference: unavailable — no joint is spelled by both palettes".to_string();
        return;
    }
    let model = models
        .single()
        .copied()
        .unwrap_or(GlobalTransform::IDENTITY);
    let place = |point: Vec3| model.transform_point(align.point(point));
    let style = overlay.style;
    let bone_mesh = match style {
        BoneStyle::Lines => meshes.line.clone(),
        BoneStyle::Mixamo => meshes.octahedral.clone(),
    };
    let drawn = segments.len();
    let mut index = 0;
    for (start, end) in segments {
        if index >= pool.joints.len() {
            break;
        }
        let start = place(start);
        if let Ok(mut transform) = transforms.get_mut(pool.joints[index]) {
            transform.translation = start;
            transform.rotation = Quat::IDENTITY;
            transform.scale = Vec3::ONE;
        }
        if let Ok(mut visibility) = visibilities.get_mut(pool.joints[index]) {
            *visibility = Visibility::Inherited;
        }
        match end.map(place) {
            Some(end) => {
                let delta = end - start;
                let length = delta.length().max(1e-5);
                if let Ok(mut transform) = transforms.get_mut(pool.bones[index]) {
                    transform.translation = start + delta * 0.5;
                    transform.rotation = Quat::from_rotation_arc(Vec3::Y, delta / length);
                    transform.scale = match style {
                        BoneStyle::Lines => Vec3::new(1.0, length, 1.0),
                        // A cone is a known-good primitive; its width is a fixed
                        // fraction of the bone and never the file's own units.
                        BoneStyle::Mixamo => {
                            let width = (length * 0.10).clamp(0.003, 0.012);
                            Vec3::new(width, length, width)
                        }
                    };
                }
                if let Some(handle) = &bone_mesh {
                    if let Ok(mut mesh) = mesh_handles.get_mut(pool.bones[index]) {
                        if mesh.0.id() != handle.id() {
                            mesh.0 = handle.clone();
                        }
                    }
                }
                if let Ok(mut visibility) = visibilities.get_mut(pool.bones[index]) {
                    *visibility = Visibility::Inherited;
                }
            }
            None => {
                if let Ok(mut visibility) = visibilities.get_mut(pool.bones[index]) {
                    *visibility = Visibility::Hidden;
                }
            }
        }
        index += 1;
    }
    for entity in pool
        .joints
        .iter()
        .skip(index)
        .chain(pool.bones.iter().skip(index))
    {
        if let Ok(mut visibility) = visibilities.get_mut(*entity) {
            *visibility = Visibility::Hidden;
        }
    }
    if std::env::var("SCULPTO_VIEWER_POSE_DEBUG").is_ok() && !*debugged {
        info!(
            "viewer: reference {} drawn as {} matched bones, scale {:.4}",
            skeleton.census().line(),
            drawn,
            align.scale
        );
        *debugged = true;
    }
    report.shown = true;
    report.status = format!(
        "reference: {} matched joints · {} · scale {:.3}",
        drawn,
        skeleton.census().line(),
        align.scale
    );
}

/// Millimetres between each matched joint pair, drawn next to the joint, plus
/// the compact total for the card. This is the number that replaces "our elbow
/// sits higher than Mixamo's": same frame, same scale, printed.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn draw_reference_labels(
    config: Res<ViewerConfig>,
    overlay: Res<ReferenceOverlay>,
    pose: Res<PoseMode>,
    cache: Res<RefCache>,
    mut report: ResMut<RefReport>,
    pool: Res<RefPool>,
    names: Query<&Name>,
    posed: Query<&GlobalTransform>,
    models: Query<&GlobalTransform, With<ReviewModel>>,
    cameras: Query<(&Camera, &GlobalTransform), (With<Camera3d>, Without<RefLabel>)>,
    mut labels: Query<(&mut Node, &mut Text), With<RefLabel>>,
) {
    // The deltas are measured in a headless capture too: that is how a still
    // carries its own numbers. Only the on-screen labels need a viewport.
    let _ = &config;
    if !overlay.on {
        report.shown = false;
        for entity in &pool.labels {
            if let Ok((mut node, _)) = labels.get_mut(*entity) {
                node.display = Display::None;
            }
        }
        return;
    }
    let (Some(skeleton), Some(align)) = (cache.skeleton.as_ref(), cache.align) else {
        return;
    };
    let by_role = role_entities(&pose, &names);
    // Both sides are compared in the model's frame: the figure's own turn is not
    // a placement defect and must not be counted as one.
    let model = models
        .single()
        .copied()
        .unwrap_or(GlobalTransform::IDENTITY);
    let mut pairs: Vec<(f32, Vec3, String)> = Vec::new();
    for ((role, side), entity) in by_role.iter() {
        let (Ok(name), Ok(posed)) = (names.get(*entity), posed.get(*entity)) else {
            continue;
        };
        let Some(reference_name) = skeleton.bone(role, *side) else {
            continue;
        };
        let Some(reference_position) = skeleton.position(reference_name) else {
            continue;
        };
        let target = align.point(reference_position);
        let local = model
            .affine()
            .inverse()
            .transform_point3(posed.translation());
        let delta = (local - target).length() * 1000.0;
        pairs.push((delta, posed.translation(), name.as_str().to_string()));
    }
    if pairs.is_empty() {
        report.shown = false;
        return;
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let pairs_count = pairs.len();
    report.pairs = pairs_count;
    report.max_mm = pairs[0].0;
    report.mean_mm = pairs.iter().map(|(delta, _, _)| *delta).sum::<f32>() / pairs_count as f32;
    report.shown = true;

    if std::env::var("SCULPTO_VIEWER_POSE_DEBUG").is_ok() {
        // The numbers the overlay exists to produce, in the one place a reader
        // can check them: pelvis first, then the worst five pairs.
        for (delta, _, name) in pairs.iter().take(3) {
            info!("viewer: reference delta {name}: {delta:.1} mm");
        }
        info!(
            "viewer: reference overlay {} pairs, max {:.1} mm, mean {:.1} mm",
            report.pairs, report.max_mm, report.mean_mm
        );
    }
    let viewport = cameras.single().ok();
    for (slot, entity) in pool.labels.iter().enumerate() {
        let Ok((mut node, mut text)) = labels.get_mut(*entity) else {
            continue;
        };
        let Some((delta, position, _)) = pairs.get(slot) else {
            node.display = Display::None;
            continue;
        };
        let projected = viewport
            .and_then(|(camera, transform)| camera.world_to_viewport(transform, *position).ok());
        match projected {
            Some(point) => {
                node.display = Display::Flex;
                node.left = Val::Px(point.x + 6.0);
                node.top = Val::Px(point.y + 4.0);
                let label = format!("{delta:.0} mm");
                if text.0 != label {
                    text.0 = label;
                }
            }
            None => node.display = Display::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small skeleton with the *measured* side convention of our real actors:
    /// the left thigh sits at +X and the left arm at -X. That is the pairing the
    /// Mixamo seat lanes found and the reason a T can cross the arms.
    const BONES: &[(&str, &str, [f32; 3])] = &[
        ("NPC Pelvis [Pelv]", "", [0.0, 1.0, 0.0]),
        (
            "NPC L Thigh [LThg]",
            "NPC Pelvis [Pelv]",
            [0.12, -0.05, 0.0],
        ),
        ("NPC L Calf [LClf]", "NPC L Thigh [LThg]", [0.0, -0.45, 0.0]),
        ("NPC L Foot [Lft ]", "NPC L Calf [LClf]", [0.0, -0.05, 0.2]),
        ("NPC L Toe0 [LToe]", "NPC L Foot [Lft ]", [0.0, 0.0, 0.1]),
        (
            "NPC R Thigh [RThg]",
            "NPC Pelvis [Pelv]",
            [-0.12, -0.05, 0.0],
        ),
        ("NPC R Calf [RClf]", "NPC R Thigh [RThg]", [0.0, -0.45, 0.0]),
        ("NPC R Foot [Rft ]", "NPC R Calf [RClf]", [0.0, -0.05, 0.2]),
        ("NPC R Toe0 [RToe]", "NPC R Foot [Rft ]", [0.0, 0.0, 0.1]),
        ("NPC Spine [Spn0]", "NPC Pelvis [Pelv]", [0.0, 0.05, 0.0]),
        ("NPC Spine1 [Spn1]", "NPC Spine [Spn0]", [0.0, 0.15, 0.0]),
        ("NPC Spine2 [Spn2]", "NPC Spine1 [Spn1]", [0.0, 0.15, 0.0]),
        ("NPC Neck [Neck]", "NPC Spine2 [Spn2]", [0.0, 0.1, 0.0]),
        ("NPC Head [Head]", "NPC Neck [Neck]", [0.0, 0.1, 0.0]),
        (
            "NPC L Clavicle [LClv]",
            "NPC Spine2 [Spn2]",
            [-0.05, 0.05, 0.0],
        ),
        (
            "NPC L UpperArm [LUar]",
            "NPC L Clavicle [LClv]",
            [-0.2, 0.0, 0.0],
        ),
        // 45° below horizontal: an A-pose arm the T has to lift.
        (
            "NPC L Forearm [LLar]",
            "NPC L UpperArm [LUar]",
            [-0.2, -0.2, 0.0],
        ),
        (
            "NPC L Hand [LHnd]",
            "NPC L Forearm [LLar]",
            [-0.15, -0.15, 0.0],
        ),
        (
            "NPC R Clavicle [RClv]",
            "NPC Spine2 [Spn2]",
            [0.05, 0.05, 0.0],
        ),
        (
            "NPC R UpperArm [RUar]",
            "NPC R Clavicle [RClv]",
            [0.2, 0.0, 0.0],
        ),
        (
            "NPC R Forearm [RLar]",
            "NPC R UpperArm [RUar]",
            [0.2, -0.2, 0.0],
        ),
        (
            "NPC R Hand [RHnd]",
            "NPC R Forearm [RLar]",
            [0.15, -0.15, 0.0],
        ),
    ];

    fn rig() -> (App, HashMap<&'static str, Entity>, Entity) {
        let mut app = App::new();
        app.insert_resource(PoseMode::default())
            .insert_resource(ViewerConfig {
                report_mode: false,
                debug_scene: false,
                interactive: true,
                flat: false,
                asset_file: String::new(),
                status: "InProgress".to_string(),
            })
            .add_systems(Update, (capture_rest, apply_pose));
        let root = app
            .world_mut()
            .spawn((Transform::default(), ReviewModel))
            .id();
        let mut ids: HashMap<&'static str, Entity> = HashMap::new();
        for (name, parent, translation) in BONES {
            let parent = if parent.is_empty() {
                root
            } else {
                ids[*parent]
            };
            let entity = app
                .world_mut()
                .spawn((
                    Name::new(*name),
                    Transform::from_translation(Vec3::from_array(*translation)),
                    ChildOf(parent),
                ))
                .id();
            ids.insert(name, entity);
        }
        let joints: Vec<Entity> = ids.values().copied().collect();
        app.world_mut().spawn((
            SkinnedMesh {
                inverse_bindposes: Handle::default(),
                joints,
            },
            Transform::default(),
        ));
        app.update();
        (app, ids, root)
    }

    fn world(app: &App, entity: Entity) -> (Vec3, Quat) {
        let mut chain = Vec::new();
        let mut current = Some(entity);
        while let Some(node) = current {
            chain.push(node);
            current = app.world().get::<ChildOf>(node).map(ChildOf::parent);
        }
        let mut position = Vec3::ZERO;
        let mut rotation = Quat::IDENTITY;
        for node in chain.iter().rev() {
            let transform = app
                .world()
                .get::<Transform>(*node)
                .copied()
                .unwrap_or_default();
            position += rotation * transform.translation;
            rotation *= transform.rotation;
        }
        (position, rotation)
    }

    fn direction(app: &App, from: Entity, to: Entity) -> Vec3 {
        (world(app, to).0 - world(app, from).0)
            .try_normalize()
            .expect("direction")
    }

    fn bits(transform: &Transform) -> ([u32; 3], [u32; 4], [u32; 3]) {
        (
            transform.translation.to_array().map(f32::to_bits),
            transform.rotation.to_array().map(f32::to_bits),
            transform.scale.to_array().map(f32::to_bits),
        )
    }

    #[test]
    fn the_rest_is_captured_for_every_driven_joint() {
        let (app, ids, _) = rig();
        let pose = app.world().resource::<PoseMode>();
        for (name, _, _) in BONES {
            assert!(
                pose.rest.contains_key(&ids[*name]),
                "{name} is a joint the skin drives and must be captured"
            );
        }
    }

    #[test]
    fn a_turn_leaves_translation_and_scale_and_its_children_attached() {
        let (mut app, ids, _) = rig();
        let upper = ids["NPC L UpperArm [LUar]"];
        let forearm = ids["NPC L Forearm [LLar]"];
        let thigh = ids["NPC L Thigh [LThg]"];
        let rest_upper = *app.world().get::<Transform>(upper).expect("upper");
        let rest_thigh = *app.world().get::<Transform>(thigh).expect("thigh");
        let before = world(&app, forearm).0;
        let reach = (before - world(&app, upper).0).length();

        {
            let mut pose = app.world_mut().resource_mut::<PoseMode>();
            pose.active = true;
            pose.selected = Some(upper);
            pose.deltas
                .insert(upper, Quat::from_rotation_z(0.9).normalize());
        }
        app.update();

        let posed_upper = *app.world().get::<Transform>(upper).expect("upper");
        assert_eq!(
            posed_upper.translation.to_array().map(f32::to_bits),
            rest_upper.translation.to_array().map(f32::to_bits),
            "a rotation delta must not move the joint"
        );
        assert_eq!(
            posed_upper.scale.to_array().map(f32::to_bits),
            rest_upper.scale.to_array().map(f32::to_bits),
            "a rotation delta must not scale the joint"
        );
        assert!(
            (posed_upper.rotation.length() - 1.0).abs() < 1e-6,
            "the posed rotation stays a unit quaternion"
        );
        assert_ne!(
            bits(&posed_upper).1,
            bits(&rest_upper).1,
            "the turned joint really turned"
        );

        let untouched = *app.world().get::<Transform>(thigh).expect("thigh");
        assert_eq!(
            bits(&untouched),
            bits(&rest_thigh),
            "an untouched joint is bit-identical to the file rest"
        );

        let child = *app.world().get::<Transform>(forearm).expect("forearm");
        assert_eq!(
            child.translation.to_array().map(f32::to_bits),
            Vec3::new(-0.2, -0.2, 0.0).to_array().map(f32::to_bits),
            "the child keeps its own bone offset"
        );
        let after = world(&app, forearm).0;
        let reach_after = (after - world(&app, upper).0).length();
        assert!(
            (reach_after - reach).abs() < 1e-5,
            "the child stayed attached to its parent: {reach_after} vs {reach}"
        );
        assert!(
            (after - before).length() > 1e-3,
            "the child followed the parent instead of staying put"
        );
    }

    #[test]
    fn the_pose_is_a_delta_on_the_rest_not_an_absolute_write() {
        let rest = Transform {
            translation: Vec3::new(0.1, 0.2, 0.3),
            rotation: Quat::from_rotation_z(0.5),
            scale: Vec3::splat(2.0),
        };
        let delta = Quat::from_rotation_x(0.3);
        let posed = delta_on_rest(&rest, delta);
        assert_eq!(
            posed.rotation,
            (delta * rest.rotation).normalize(),
            "an absolute local write would drop the rest rotation"
        );
        assert_ne!(posed.rotation, delta);
        assert_eq!(posed.translation, rest.translation);
        assert_eq!(posed.scale, rest.scale);
    }

    #[test]
    fn the_swing_chain_reaches_every_target_direction() {
        // Two joints in one chain: A points +X, its child B points +Y.
        let a = Entity::from_raw_u32(1).expect("entity");
        let b = Entity::from_raw_u32(2).expect("entity");
        let joints = vec![
            SwingJoint {
                entity: a,
                depth: 1,
                parent: None,
                rest_world_rotation: Quat::from_rotation_z(0.7),
                rest_local_rotation: Quat::from_rotation_z(0.7),
                direction: Vec3::X,
                target: Vec3::Y,
            },
            SwingJoint {
                entity: b,
                depth: 2,
                parent: Some(a),
                rest_world_rotation: Quat::from_rotation_z(0.7) * Quat::from_rotation_x(0.4),
                rest_local_rotation: Quat::from_rotation_x(0.4),
                direction: Vec3::Y,
                target: Vec3::Z,
            },
        ];
        let deltas: HashMap<Entity, Quat> = solve_swing_chain(&joints).into_iter().collect();
        // Compose the posed world rotations exactly the way the renderer does:
        // world = parent_world * delta * rest_local, and the joint's direction
        // is its rest world direction carried by that world rotation.
        let mut posed: HashMap<Entity, Quat> = HashMap::new();
        for joint in &joints {
            let parent_world = joint
                .parent
                .and_then(|parent| posed.get(&parent).copied())
                .unwrap_or(Quat::IDENTITY);
            let world = parent_world * deltas[&joint.entity] * joint.rest_local_rotation;
            let applied = world * joint.rest_world_rotation.inverse();
            let reached = (applied * joint.direction).normalize();
            assert!(
                reached.dot(joint.target.normalize()) > 0.999_9,
                "joint depth {} points {reached:?}, not {:?}",
                joint.depth,
                joint.target
            );
            posed.insert(joint.entity, world);
        }
    }

    #[test]
    fn the_body_frame_is_measured_and_carries_the_yaw_flip() {
        // Our actor: left thigh at +X, up +Y. Reference: the same body yawed
        // 180°, exactly the measured relation between our bodies and Mixamo.
        let actor = anatomical_frame(Vec3::X, Vec3::Y).expect("actor frame");
        let reference = anatomical_frame(-Vec3::X, Vec3::Y).expect("reference frame");
        let transfer = actor * reference.transpose();
        assert!(
            (transfer * Vec3::X).dot(-Vec3::X) > 0.999_9,
            "the measured frame flips the reference onto our body: {:?}",
            transfer * Vec3::X
        );
        assert!((transfer * Vec3::Y).dot(Vec3::Y) > 0.999_9, "up stays up");
    }

    /// Mixamo's own convention, measured on `ybot_walking.glb`: the left leg at
    /// -X, the left arm at +X, arms exactly horizontal (0.0° below).
    const REFERENCE_BONES: &[(&str, Option<&str>, [f32; 3])] = &[
        ("Hips", None, [0.0, 100.0, 0.0]),
        ("mixamorig:LeftUpLeg", Some("Hips"), [-10.0, -5.0, 0.0]),
        (
            "mixamorig:LeftLeg",
            Some("mixamorig:LeftUpLeg"),
            [0.0, -45.0, 0.0],
        ),
        (
            "mixamorig:LeftFoot",
            Some("mixamorig:LeftLeg"),
            [0.0, -5.0, 20.0],
        ),
        (
            "mixamorig:LeftToeBase",
            Some("mixamorig:LeftFoot"),
            [0.0, 0.0, 10.0],
        ),
        ("mixamorig:RightUpLeg", Some("Hips"), [10.0, -5.0, 0.0]),
        (
            "mixamorig:RightLeg",
            Some("mixamorig:RightUpLeg"),
            [0.0, -45.0, 0.0],
        ),
        (
            "mixamorig:RightFoot",
            Some("mixamorig:RightLeg"),
            [0.0, -5.0, 20.0],
        ),
        (
            "mixamorig:RightToeBase",
            Some("mixamorig:RightFoot"),
            [0.0, 0.0, 10.0],
        ),
        ("mixamorig:Spine", Some("Hips"), [0.0, 5.0, 0.0]),
        (
            "mixamorig:Spine1",
            Some("mixamorig:Spine"),
            [0.0, 15.0, 0.0],
        ),
        (
            "mixamorig:Spine2",
            Some("mixamorig:Spine1"),
            [0.0, 15.0, 0.0],
        ),
        ("mixamorig:Neck", Some("mixamorig:Spine2"), [0.0, 10.0, 0.0]),
        ("mixamorig:Head", Some("mixamorig:Neck"), [0.0, 10.0, 0.0]),
        (
            "mixamorig:LeftShoulder",
            Some("mixamorig:Spine2"),
            [5.0, 5.0, 0.0],
        ),
        (
            "mixamorig:LeftArm",
            Some("mixamorig:LeftShoulder"),
            [10.0, 0.0, 0.0],
        ),
        (
            "mixamorig:LeftForeArm",
            Some("mixamorig:LeftArm"),
            [20.0, 0.0, 0.0],
        ),
        (
            "mixamorig:LeftHand",
            Some("mixamorig:LeftForeArm"),
            [15.0, 0.0, 0.0],
        ),
        (
            "mixamorig:RightShoulder",
            Some("mixamorig:Spine2"),
            [-5.0, 5.0, 0.0],
        ),
        (
            "mixamorig:RightArm",
            Some("mixamorig:RightShoulder"),
            [-10.0, 0.0, 0.0],
        ),
        (
            "mixamorig:RightForeArm",
            Some("mixamorig:RightArm"),
            [-20.0, 0.0, 0.0],
        ),
        (
            "mixamorig:RightHand",
            Some("mixamorig:RightForeArm"),
            [-15.0, 0.0, 0.0],
        ),
    ];

    fn reference_json() -> Value {
        let index = |name: &str| {
            REFERENCE_BONES
                .iter()
                .position(|(bone, _, _)| *bone == name)
                .expect("bone is in the table")
        };
        let nodes: Vec<Value> = REFERENCE_BONES
            .iter()
            .map(|(name, parent, translation)| {
                let _ = parent;
                let children: Vec<usize> = REFERENCE_BONES
                    .iter()
                    .filter(|(_, candidate, _)| *candidate == Some(*name))
                    .map(|(child, _, _)| index(child))
                    .collect();
                serde_json::json!({
                    "name": name,
                    "translation": translation,
                    "children": children,
                })
            })
            .collect();
        serde_json::json!({"asset": {"version": "2.0"}, "nodes": nodes})
    }

    fn write_reference(path: &Path) {
        let mut text = serde_json::to_vec(&reference_json()).expect("json");
        while text.len() % 4 != 0 {
            text.push(b' ');
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(12 + 8 + text.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
        bytes.extend_from_slice(&text);
        std::fs::write(path, bytes).expect("write reference");
    }

    #[derive(Resource, Default)]
    struct ReferenceProbe {
        path: PathBuf,
        outcome: Option<Result<(usize, f32), String>>,
    }

    fn run_reference(
        mut pose: ResMut<PoseMode>,
        names: Query<&Name>,
        parents: Query<&ChildOf>,
        mut probe: ResMut<ReferenceProbe>,
    ) {
        let path = probe.path.clone();
        probe.outcome = Some(
            apply_reference(&mut pose, &path, &names, &parents)
                .map(|report| (report.chains, report.yaw_degrees)),
        );
    }

    #[test]
    fn the_mixamo_t_lifts_the_arms_to_the_reference_direction() {
        let path = std::env::temp_dir().join("sculpto-pose-t-reference.glb");
        write_reference(&path);
        let (mut app, ids, _) = rig();
        app.insert_resource(ReferenceProbe {
            path: path.clone(),
            outcome: None,
        })
        .add_systems(Update, run_reference);
        app.update();

        let outcome = app
            .world()
            .resource::<ReferenceProbe>()
            .outcome
            .clone()
            .expect("the reference ran");
        let (chains, yaw) = outcome.expect("the reference is readable");
        assert!(
            chains >= 14,
            "the T drives the whole body, got {chains} chains"
        );
        assert!(
            (yaw.abs() - 180.0).abs() < 1.0,
            "the measured frame yaw is the 180° the seat lanes found, got {yaw}"
        );

        let left_upper = ids["NPC L UpperArm [LUar]"];
        let left_fore = ids["NPC L Forearm [LLar]"];
        let right_upper = ids["NPC R UpperArm [RUar]"];
        let right_fore = ids["NPC R Forearm [RLar]"];
        let pose = app.world_mut().resource_mut::<PoseMode>();
        assert!(pose.active, "T turns pose mode on");
        assert!(pose.reference_applied);
        assert_eq!(
            pose.deltas.get(&ids["NPC Pelvis [Pelv]"]),
            None,
            "the pelvis is not a T handle: an odd native pelvis->spine would tilt the body"
        );

        app.update();
        let left = direction(&app, left_upper, left_fore);
        let right = direction(&app, right_upper, right_fore);
        assert!(
            left.y.abs() < 1e-3,
            "the left arm is horizontal after T, got {left:?}"
        );
        assert!(
            left.x < -0.99,
            "the left arm went to the left side: {left:?}"
        );
        assert!(
            right.x > 0.99,
            "the right arm went to the right side, not across the chest: {right:?}"
        );

        // The children of a turned joint keep their own offsets, and the
        // untouched joints keep their rest bits.
        for (name, _, _) in BONES {
            let entity = ids[*name];
            let transform = *app.world().get::<Transform>(entity).expect("bone");
            assert!(
                (transform.rotation.length() - 1.0).abs() < 1e-6,
                "{name} keeps a unit rotation"
            );
            assert_eq!(
                transform.scale,
                Vec3::ONE,
                "{name} is never scaled: that would inflate the skin"
            );
        }
        let thigh = *app
            .world()
            .get::<Transform>(ids["NPC L Thigh [LThg]"])
            .expect("thigh");
        let rest = PoseMode::default();
        let _ = rest;
        let thigh_delta = app
            .world()
            .resource::<PoseMode>()
            .deltas
            .get(&ids["NPC L Thigh [LThg]"])
            .copied()
            .unwrap_or(Quat::IDENTITY);
        assert!(
            thigh_delta.angle_between(Quat::IDENTITY) < 1e-3,
            "a leg already pointing down is left alone, got {:?}",
            thigh_delta
        );
        assert_eq!(thigh.translation, Vec3::new(0.12, -0.05, 0.0));
        std::fs::remove_file(&path).ok();
    }

    #[derive(Resource, Default)]
    struct AlignProbe {
        path: PathBuf,
        align: Option<BodyAlign>,
        hips: Option<Vec3>,
    }

    fn run_body_align(
        pose: Res<PoseMode>,
        names: Query<&Name>,
        parents: Query<&ChildOf>,
        mut probe: ResMut<AlignProbe>,
    ) {
        let Some(skeleton) = RefSkeleton::load(&probe.path) else {
            return;
        };
        probe.hips = skeleton
            .bone("pelvis", None)
            .and_then(|name| skeleton.position(name));
        probe.align = body_align(&pose, &skeleton, &names, &parents).ok();
    }

    #[test]
    fn the_reference_alignment_puts_the_reference_hips_on_our_hips() {
        let path = std::env::temp_dir().join("sculpto-pose-align-reference.glb");
        write_reference(&path);
        let (mut app, ids, _) = rig();
        app.insert_resource(AlignProbe {
            path: path.clone(),
            align: None,
            hips: None,
        })
        .add_systems(Update, run_body_align);
        app.update();
        let align = app
            .world()
            .resource::<AlignProbe>()
            .align
            .expect("the two skeletons align");
        // The reference's hips land on our pelvis: hip alignment is the anchor.
        let reference_hips = app
            .world()
            .resource::<AlignProbe>()
            .hips
            .expect("the reference has hips");
        let hips = align.point(reference_hips);
        let pelvis = world(&app, ids["NPC Pelvis [Pelv]"]).0;
        assert!(
            (hips - pelvis).length() < 1e-5,
            "aligned hips {hips:?} vs our pelvis {pelvis:?}"
        );
        // Scale is the ratio of two measured heights (pelvis -> head plus
        // pelvis -> toe): 1.02 m on this rig against 102 cm on the reference.
        assert!(
            (align.scale() - 0.01).abs() < 5e-4,
            "scale {}",
            align.scale()
        );
        // Same yaw the T applies: the reference is mirrored onto our flesh.
        assert!(
            (align.yaw_degrees().abs() - 180.0).abs() < 1.0,
            "yaw {}",
            align.yaw_degrees()
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_scaled_armature_is_read_in_metres_and_y_up() {
        // Mixamo's own layout: a centimetre, Z-up armature under a 0.01 scale and
        // a +90° X rotation. Reading it without the matrix puts the hips at
        // z = -100 and the hand a hundred times too far out.
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "nodes": [
                {
                    "name": "Armature",
                    "rotation": [0.70710678, 0.0, 0.0, 0.70710678],
                    "scale": [0.01, 0.01, 0.01],
                    "children": [1],
                },
                {"name": "mixamorig:Hips", "translation": [0.0, 0.0, -100.0], "children": [2]},
                {"name": "mixamorig:Head", "translation": [0.0, 0.0, -60.0], "children": []},
            ]
        });
        let skeleton = RefSkeleton::from_json(&json).expect("parses");
        let hips = skeleton.position("mixamorig:Hips").expect("hips");
        let head = skeleton.position("mixamorig:Head").expect("head");
        assert!(
            (hips - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5,
            "hips at {hips:?}, not a metre up in Y"
        );
        assert!(
            (head - Vec3::new(0.0, 1.6, 0.0)).length() < 1e-5,
            "head at {head:?}, not 0.6 m above the hips"
        );
    }

    #[test]
    fn the_role_table_reads_both_palettes() {
        assert_eq!(
            role_of("NPC L UpperArm [LUar]"),
            Some(("upperarm", Some('L')))
        );
        assert_eq!(role_of("mixamorig:LeftArm"), Some(("upperarm", Some('L'))));
        assert_eq!(
            role_of("mixamorig:RightForeArm"),
            Some(("forearm", Some('R')))
        );
        assert_eq!(role_of("NPC Spine1 [Spn1]"), Some(("spine1", None)));
        assert_eq!(role_of("NPC L UpperarmTwist1 [LUt1]"), None);
        assert_eq!(role_of("NPC L ForearmTwist2 [LLt2]"), None);
        assert_eq!(role_of("mixamorig:HeadTop_End"), None);
        assert_eq!(limb_of("NPC L Calf [LClf]"), Limb::Leg('L'));
        assert_eq!(limb_of("mixamorig:LeftArm"), Limb::Arm('L'));
        assert_eq!(limb_of("NPC Head [Head]"), Limb::Torso);
    }

    #[test]
    fn a_broken_reference_is_refused_not_guessed() {
        let path = std::env::temp_dir().join("sculpto-pose-broken-reference.glb");
        std::fs::write(&path, b"not a glb").expect("write");
        let (mut app, _, _) = rig();
        app.insert_resource(ReferenceProbe {
            path: path.clone(),
            outcome: None,
        })
        .add_systems(Update, run_reference);
        app.update();
        let outcome = app
            .world()
            .resource::<ReferenceProbe>()
            .outcome
            .clone()
            .expect("ran");
        assert!(
            outcome.is_err(),
            "an unreadable reference must refuse, not pose from nothing"
        );
        assert!(
            app.world().resource::<PoseMode>().deltas.is_empty(),
            "a refusal leaves the pose alone"
        );
        std::fs::remove_file(&path).ok();
    }
}
