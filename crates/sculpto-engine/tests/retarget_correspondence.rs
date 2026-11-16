//! The correspondence layer and the two unit corrections, on synthetic
//! skeletons whose bodies, names and units are chosen by the test.
//!
//! Every skeleton here is built in the test — hierarchy, rest, unit node and a
//! clip — so a failure names the property that broke rather than a fixture's
//! history: a different body, a different naming convention, a centimetre file
//! under a 0.01 ancestor, and a pose the source really performs.
//!
//! @req DECL0013

use std::borrow::Cow;

use glam::{DMat4, DQuat, DVec3};
use gltf::binary::Glb;
use sculpto_core::rig::{Joint, LocalTransform, Rig, RigFrame};
use sculpto_engine::export_actor::admit_clip;
use sculpto_engine::export_glb::{
    ClipAnchor, ClipSemantics, ClipTolerances, PaletteFrames, RootMotion, plan_clip_with,
};
use sculpto_engine::retarget::{
    Correspondence, Method, Part, Side, correspondence_report, role_of,
};
use sculpto_mesh::actor::Actor;
use serde_json::{Value, json};

/// One synthetic bone: name, parent, and its rest local transform.
#[derive(Clone, Copy)]
struct Bone {
    name: &'static str,
    parent: Option<usize>,
    translation: [f64; 3],
    rotation: [f64; 4],
}

const IDENTITY: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

fn bone(name: &'static str, parent: Option<usize>, translation: [f64; 3]) -> Bone {
    Bone {
        name,
        parent,
        translation,
        rotation: IDENTITY,
    }
}

/// One animation channel over a synthetic skeleton.
struct Channel {
    node: usize,
    path: &'static str,
    values: Vec<Vec<f64>>,
}

/// Frame a document as a GLB with the binary chunk it references.
fn frame(document: Value, bin: Vec<u8>) -> Vec<u8> {
    Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(serde_json::to_vec(&document).expect("a document serialises")),
        bin: Some(Cow::Owned(bin)),
    }
    .to_vec()
    .expect("a framed container")
}

/// Pad the binary chunk to a four-byte boundary.
fn align(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}

/// Build one skinned, optionally animated GLB from a bone list.
///
/// `root_scale` and `root_rotation` go on the armature node above every bone,
/// which is where a Blender/FBX export puts its centimetre factor; `clip` adds
/// one animation named `walk`.
fn skeleton(bones: &[Bone], root_scale: f64, root_rotation: [f64; 4], clip: &[Channel]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let push = |bin: &mut Vec<u8>,
                views: &mut Vec<Value>,
                accessors: &mut Vec<Value>,
                bytes: Vec<u8>,
                kind: &str,
                count: usize,
                width: usize|
     -> usize {
        align(bin);
        let offset = bin.len();
        bin.extend_from_slice(&bytes);
        views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
        }));
        accessors.push(json!({
            "bufferView": views.len() - 1,
            "componentType": match kind {
                "f32" => 5126,
                _ => 5123,
            },
            "count": count,
            "type": match width {
                3 => "VEC3",
                4 => "VEC4",
                16 => "MAT4",
                _ => "SCALAR",
            },
        }));
        accessors.len() - 1
    };

    // World rest transforms, so the inverse bind matrices are the real ones.
    let mut worlds = vec![DMat4::IDENTITY; bones.len()];
    for (index, definition) in bones.iter().enumerate() {
        let local = DMat4::from_scale_rotation_translation(
            DVec3::ONE,
            DQuat::from_array(definition.rotation),
            DVec3::from_array(definition.translation),
        );
        worlds[index] = match definition.parent {
            Some(parent) => worlds[parent] * local,
            None => local,
        };
    }
    let armature = DMat4::from_scale_rotation_translation(
        DVec3::splat(root_scale),
        DQuat::from_array(root_rotation),
        DVec3::ZERO,
    );
    let mut bind: Vec<f64> = Vec::with_capacity(bones.len() * 16);
    for world in &worlds {
        let inverse = (armature * *world).inverse();
        bind.extend(inverse.to_cols_array());
    }
    let bind_bytes: Vec<u8> = bind
        .iter()
        .flat_map(|value| (*value as f32).to_le_bytes())
        .collect();
    let bind_accessor = push(
        &mut bin,
        &mut views,
        &mut accessors,
        bind_bytes,
        "f32",
        bones.len(),
        16,
    );

    // One triangle, weighted to the first bone.
    let positions: Vec<u8> = [[0.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    let position_accessor = push(&mut bin, &mut views, &mut accessors, positions, "f32", 3, 3);
    accessors[position_accessor]["min"] = json!([0.0, 0.0, 0.0]);
    accessors[position_accessor]["max"] = json!([0.0, 1.0, 1.0]);
    let joints: Vec<u8> = [0_u16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let joint_accessor = push(&mut bin, &mut views, &mut accessors, joints, "u16", 3, 4);
    let weights: Vec<u8> = [[1.0_f32, 0.0, 0.0, 0.0]; 3]
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    let weight_accessor = push(&mut bin, &mut views, &mut accessors, weights, "f32", 3, 4);
    let indices: Vec<u8> = [0_u16, 1, 2]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let index_accessor = push(&mut bin, &mut views, &mut accessors, indices, "u16", 3, 1);

    // The hierarchy: armature node 0, bones from node 1, mesh node last.
    let mut nodes: Vec<Value> = Vec::with_capacity(bones.len() + 2);
    let roots: Vec<usize> = bones
        .iter()
        .enumerate()
        .filter(|(_, definition)| definition.parent.is_none())
        .map(|(index, _)| index + 1)
        .collect();
    nodes.push(
        json!({"name": "Armature", "scale": [root_scale, root_scale, root_scale],
        "rotation": [root_rotation[0], root_rotation[1], root_rotation[2], root_rotation[3]],
        "children": roots}),
    );
    for (index, definition) in bones.iter().enumerate() {
        let children: Vec<usize> = bones
            .iter()
            .enumerate()
            .filter(|(_, other)| other.parent == Some(index))
            .map(|(child, _)| child + 1)
            .collect();
        nodes.push(json!({
            "name": definition.name,
            "translation": definition.translation,
            "rotation": definition.rotation,
            "children": children,
        }));
    }
    let mesh_node = nodes.len();
    nodes.push(json!({"name": "Body", "mesh": 0, "skin": 0}));
    nodes[0]["children"]
        .as_array_mut()
        .expect("armature children")
        .push(json!(mesh_node));

    let mut samplers: Vec<Value> = Vec::new();
    let mut channels: Vec<Value> = Vec::new();
    let mut animations: Vec<Value> = Vec::new();
    if !clip.is_empty() {
        let times: Vec<f32> = (0..clip[0].values.len())
            .map(|index| index as f32 / 30.0)
            .collect();
        let time_bytes: Vec<u8> = times.iter().flat_map(|value| value.to_le_bytes()).collect();
        let time_accessor = push(
            &mut bin,
            &mut views,
            &mut accessors,
            time_bytes,
            "f32",
            times.len(),
            1,
        );
        for channel in clip {
            let flat: Vec<f32> = channel
                .values
                .iter()
                .flatten()
                .map(|value| *value as f32)
                .collect();
            let width = channel.values.first().map(Vec::len).unwrap_or(3);
            let values: Vec<u8> = flat.iter().flat_map(|value| value.to_le_bytes()).collect();
            let accessor = push(
                &mut bin,
                &mut views,
                &mut accessors,
                values,
                "f32",
                channel.values.len(),
                width,
            );
            let sampler = samplers.len();
            samplers.push(
                json!({"input": time_accessor, "output": accessor, "interpolation": "LINEAR"}),
            );
            channels.push(json!({
                "sampler": sampler,
                "target": {"node": channel.node, "path": channel.path},
            }));
        }
        animations.push(json!({"name": "walk", "channels": channels, "samplers": samplers}));
    }

    let document = json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
        "meshes": [{"name": "Body", "primitives": [{
            "attributes": {
                "POSITION": position_accessor,
                "JOINTS_0": joint_accessor,
                "WEIGHTS_0": weight_accessor,
            },
            "indices": index_accessor,
        }]}],
        "skins": [{"joints": (1..=bones.len()).collect::<Vec<_>>(),
                    "inverseBindMatrices": bind_accessor}],
        "nodes": nodes,
        "scenes": [{"nodes": [0]}],
        "scene": 0,
        "animations": animations,
    });
    frame(document, bin)
}

/// A generic humanoid with a name supplied per bone: `names` short-circuits the
/// convention, so the same body can be tested under Mixamo, SSE, bare and
/// meaningless names.
fn humanoid(names: &[&'static str; 13]) -> Vec<Bone> {
    vec![
        bone(names[0], None, [0.0, 1.0, 0.0]),
        bone(names[1], Some(0), [0.0, 0.10, 0.0]),
        bone(names[2], Some(1), [0.0, 0.10, 0.0]),
        bone(names[3], Some(2), [0.0, 0.10, 0.0]),
        bone(names[4], Some(3), [0.0, 0.10, 0.0]),
        bone(names[5], Some(4), [0.0, 0.10, 0.0]),
        bone(names[6], Some(2), [-0.20, 0.0, 0.0]),
        bone(names[7], Some(6), [-0.28, 0.0, 0.0]),
        bone(names[8], Some(7), [-0.25, 0.0, 0.0]),
        bone(names[9], Some(0), [-0.10, -0.10, 0.0]),
        bone(names[10], Some(9), [0.0, -0.45, 0.0]),
        bone(names[11], Some(10), [0.0, -0.45, 0.0]),
        bone(names[12], Some(11), [0.0, -0.05, 0.10]),
    ]
}

/// A walk-shaped clip: the pelvis travels and the arm swings, so a conversion
/// has moving channels to write and a root trajectory to measure. `unit` is the
/// skeleton's local unit, so a centimetre skeleton gets a centimetre clip.
fn walk_in(unit: f64) -> Vec<Channel> {
    vec![
        Channel {
            node: 1,
            path: "translation",
            values: (0..8)
                .map(|index| vec![0.0, 1.0 * unit, 0.4 * unit * index as f64 / 7.0])
                .collect(),
        },
        Channel {
            node: 8,
            path: "rotation",
            values: (0..8)
                .map(|index| {
                    let angle = 40.0_f64.to_radians() * index as f64 / 7.0;
                    DQuat::from_rotation_z(angle).to_array().to_vec()
                })
                .collect(),
        },
    ]
}

fn walk() -> Vec<Channel> {
    walk_in(1.0)
}

const MIXAMO: [&str; 13] = [
    "mixamorig:Hips",
    "mixamorig:Spine",
    "mixamorig:Spine1",
    "mixamorig:Spine2",
    "mixamorig:Neck",
    "mixamorig:Head",
    "mixamorig:LeftShoulder",
    "mixamorig:LeftArm",
    "mixamorig:LeftForeArm",
    "mixamorig:LeftUpLeg",
    "mixamorig:LeftLeg",
    "mixamorig:LeftFoot",
    "mixamorig:LeftToeBase",
];

const SSE: [&str; 13] = [
    "NPC Pelvis [Pelv]",
    "NPC Spine [Spn0]",
    "NPC Spine1 [Spn1]",
    "NPC Spine2 [Spn2]",
    "NPC Neck [Neck]",
    "NPC Head [Head]",
    "NPC L Clavicle [LClv]",
    "NPC L UpperArm [LUar]",
    "NPC L Forearm [LLar]",
    "NPC L Thigh [LThg]",
    "NPC L Calf [LClf]",
    "NPC L Foot [Lft ]",
    "NPC L Toe0 [LToe]",
];

const OPAQUE: [&str; 13] = [
    "Bone_000", "Bone_001", "Bone_002", "Bone_003", "Bone_004", "Bone_005", "Bone_006", "Bone_007",
    "Bone_008", "Bone_009", "Bone_010", "Bone_011", "Bone_012",
];

/// The actor's palette as a rig, the same construction `adapt` performs.
fn actor_rig(actor: &Actor) -> (Rig, Vec<String>) {
    let joints = actor.skin_joints().expect("a skin");
    let palette: Vec<String> = joints
        .iter()
        .map(|node| actor.node_name(*node).expect("named joints").to_owned())
        .collect();
    let mut worlds = vec![DMat4::IDENTITY; actor.node_parents().len()];
    fn resolve(actor: &Actor, node: usize, worlds: &mut Vec<DMat4>) -> DMat4 {
        let local = DMat4::from_cols_array_2d(&actor.rest_local_matrix(node).expect("rest"));
        let world = match actor.node_parents()[node] {
            Some(parent) => resolve(actor, parent, worlds) * local,
            None => local,
        };
        worlds[node] = world;
        world
    }
    for node in 0..worlds.len() {
        resolve(actor, node, &mut worlds);
    }
    let mut rows = Vec::with_capacity(joints.len());
    for (slot, node) in joints.iter().enumerate() {
        let mut parent = actor.node_parents()[*node];
        let mut parent_slot = None;
        while let Some(candidate) = parent {
            match joints.iter().position(|other| *other == candidate) {
                Some(found) => {
                    parent_slot = Some(found);
                    break;
                }
                None => parent = actor.node_parents()[candidate],
            }
        }
        let parent_world = match parent_slot {
            Some(slot) => worlds[joints[slot]],
            None => DMat4::IDENTITY,
        };
        let local = parent_world.inverse() * worlds[*node];
        let (scale, rotation, translation) = local.to_scale_rotation_translation();
        rows.push(Joint {
            name: Some(palette[slot].clone()),
            parent: parent_slot.map(|slot| sculpto_core::rig::JointId::new(slot as u32)),
            rest: LocalTransform {
                translation: translation.to_array().map(|value| value as f32),
                rotation: rotation.to_array().map(|value| value as f32),
                scale: scale.x as f32,
            },
        });
    }
    (
        Rig::new(rows, RigFrame::YUp).expect("a valid palette rig"),
        palette,
    )
}

/// Plan a clip from the source GLB onto the target actor.
fn plan(
    target: &[u8],
    source: &[u8],
    semantics: ClipSemantics,
) -> (
    Correspondence,
    sculpto_engine::export_glb::ClipPlan,
    Vec<String>,
) {
    let actor = Actor::from_glb(target).expect("target decodes");
    let (rig, palette) = actor_rig(&actor);
    let source_actor = Actor::from_glb(source).expect("source decodes");
    let correspondence =
        Correspondence::build(&source_actor, &rig, &palette).expect("a correspondence");
    let clip = source_actor.clip("walk").expect("the source clip");
    let anchor = ClipAnchor::of(&source_actor, &clip).expect("an anchor");
    let plan = plan_clip_with(
        &clip,
        &source_actor,
        &anchor,
        &rig,
        &palette,
        &ClipTolerances::default(),
        Some(&correspondence),
        None,
        semantics,
        RootMotion::Keep,
    )
    .expect("the clip plans");
    (correspondence, plan, palette)
}

/// Plan the same clip with the target file's own frames measured, which is what
/// `actor adapt` does before it writes.
fn plan_framed(
    target: &[u8],
    source: &[u8],
    semantics: ClipSemantics,
    root_motion: RootMotion,
) -> (
    sculpto_engine::export_glb::ClipPlan,
    PaletteFrames,
    Vec<String>,
) {
    let actor = Actor::from_glb(target).expect("target decodes");
    let (rig, palette) = actor_rig(&actor);
    let frames = PaletteFrames::of(&actor).expect("the file's frames measure");
    let source_actor = Actor::from_glb(source).expect("source decodes");
    let correspondence =
        Correspondence::build(&source_actor, &rig, &palette).expect("a correspondence");
    let clip = source_actor.clip("walk").expect("the source clip");
    let anchor = ClipAnchor::of(&source_actor, &clip).expect("an anchor");
    let plan = plan_clip_with(
        &clip,
        &source_actor,
        &anchor,
        &rig,
        &palette,
        &ClipTolerances::default(),
        Some(&correspondence),
        Some(&frames),
        semantics,
        root_motion,
    )
    .expect("the clip plans");
    (plan, frames, palette)
}

#[test]
fn names_resolve_in_every_convention_the_product_reads() {
    let expected = [
        ("mixamorig:LeftArm", Side::Left, Part::UpperArm),
        ("mixamorig1:RightUpLeg", Side::Right, Part::Thigh),
        ("NPC L Forearm [LLar]", Side::Left, Part::Forearm),
        ("RightHand", Side::Right, Part::Hand),
        ("upper_arm.L", Side::Left, Part::UpperArm),
        ("thigh.R", Side::Right, Part::Thigh),
        (
            "NPC L Finger10 [LF10]",
            Side::Left,
            Part::Digit {
                digit: 1,
                segment: 0,
            },
        ),
        (
            "mixamorig:LeftHandIndex3",
            Side::Left,
            Part::Digit {
                digit: 1,
                segment: 2,
            },
        ),
        (
            "mixamorig:LeftHandThumb1",
            Side::Left,
            Part::Digit {
                digit: 0,
                segment: 0,
            },
        ),
        ("mixamorig:Hips", Side::Centre, Part::Pelvis),
        ("mixamorig:Spine2", Side::Centre, Part::Spine(2)),
        ("NPC Head [Head]", Side::Centre, Part::Head),
        (
            "NPC L UpperarmTwist1 [LUt1]",
            Side::Left,
            Part::Twist {
                forearm: false,
                index: 1,
            },
        ),
        ("mixamorig:LeftToeBase", Side::Left, Part::Toe),
        ("mixamorig:HeadTop_End", Side::Centre, Part::Marker),
        ("L Breast03", Side::Centre, Part::Soft),
    ];
    for (name, side, part) in expected {
        let role = role_of(name).unwrap_or_else(|| panic!("{name} resolves"));
        assert_eq!(role.side, side, "{name}");
        assert_eq!(role.part, part, "{name}");
    }
    assert!(
        role_of("Armature").is_some(),
        "scaffolding is named, not resolved"
    );
    assert!(role_of("Bone_042").is_none());
}

#[test]
fn a_mixamo_named_source_resolves_by_convention() {
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let source = skeleton(&humanoid(&MIXAMO), 1.0, IDENTITY, &[]);
    let actor = Actor::from_glb(&target).expect("target");
    let (rig, palette) = actor_rig(&actor);
    let source_actor = Actor::from_glb(&source).expect("source");
    let correspondence = Correspondence::build(&source_actor, &rig, &palette).expect("built");
    let report = correspondence_report(&correspondence);
    assert_eq!(report["mapped"], json!(13));
    assert_eq!(report["unmatched_palette"], json!([]));
    // `Spine`, `Spine1` and `Spine2` keep their own names through the prefix, so
    // they resolve as names while the sided bones resolve by convention.
    for pair in &correspondence.pairs {
        assert!(
            matches!(pair.method, Method::Name | Method::Convention),
            "{} resolved by {}",
            pair.palette,
            pair.method.token()
        );
        let source_name = pair.source_name.as_deref().expect("a source bone");
        assert!(source_name.starts_with("mixamorig:"));
    }
}

#[test]
fn an_unnamed_skeleton_resolves_by_geometry_and_reports_what_it_could_not() {
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    // A source with meaningless names but the same body, plus one extra bone the
    // palette has no place for.
    let mut bones = humanoid(&OPAQUE);
    bones.push(bone("Bone_999", Some(0), [0.5, 0.2, 0.0]));
    let source = skeleton(&bones, 1.0, IDENTITY, &[]);
    let actor = Actor::from_glb(&target).expect("target");
    let (rig, palette) = actor_rig(&actor);
    let source_actor = Actor::from_glb(&source).expect("source");
    let correspondence = Correspondence::build(&source_actor, &rig, &palette).expect("built");
    let geometry = correspondence
        .pairs
        .iter()
        .filter(|pair| pair.method == Method::Geometry)
        .count();
    assert!(
        geometry >= 12,
        "the geometry fallback must carry a source whose names mean nothing, got {geometry}: {:?}",
        correspondence
            .pairs
            .iter()
            .map(|pair| (pair.palette.as_str(), pair.method.token()))
            .collect::<Vec<_>>()
    );
    assert!(correspondence.unmatched_palette().is_empty());
    assert!(
        correspondence
            .unmatched
            .iter()
            .any(|row| row.name == "Bone_999"),
        "an extra source bone is reported, never guessed"
    );
}

#[test]
fn a_centimetre_source_under_a_scale_node_passes_the_rest_gate() {
    // The same skeleton, one in metres and one in centimetres under a 0.01
    // ancestor. The previous form multiplied a *world* position by a per-local
    // unit ratio and reported a residual near 1.0 on this exact conversion.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let mut source = humanoid(&MIXAMO);
    for definition in &mut source {
        for axis in &mut definition.translation {
            *axis *= 100.0;
        }
    }
    let source = skeleton(&source, 0.01, [0.0, 0.0, 0.0, 1.0], &walk_in(100.0));
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Pose);
    assert!(
        plan.max_rest_translation_residual < 1e-3,
        "a correct centimetre conversion must reconstruct the rest, residual {}",
        plan.max_rest_translation_residual
    );
    assert!(
        plan.rest_pose_comparable,
        "the same body in another unit is comparable"
    );
    // The unit is palette units per *world* unit, and both worlds are metres: the
    // centimetre factor lives on the source's ancestor, not in the clip.
    assert!((plan.scale - 1.0).abs() < 1e-6, "unit {}", plan.scale);
    // The travel keeps its authored length through the conversion, which is the
    // property the old double-scaled form destroyed.
    assert!(
        (plan.root_travel - 0.4).abs() < 0.02,
        "root travel {} m over the clip",
        plan.root_travel
    );
}

#[test]
fn a_different_body_reports_its_proportions_instead_of_refusing() {
    // The same names and the same joints, but the source's clavicles and thighs
    // are three times the length: a different body, which the old per-bone unit
    // floor refused outright.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let mut source = humanoid(&MIXAMO);
    source[6].translation = [-0.60, 0.0, 0.0];
    source[7].translation = [-0.85, 0.0, 0.0];
    source[8].translation = [-0.75, 0.0, 0.0];
    source[9].translation = [-0.30, -0.30, 0.0];
    let source = skeleton(&source, 1.0, IDENTITY, &walk());
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Motion);
    assert!(
        plan.scale_shared < 0.8,
        "a different body shares no per-bone ratio, measured {}",
        plan.scale_shared
    );
    assert!(
        !plan.rest_pose_comparable,
        "a different body is not rest-comparable; the declaration is the honest record"
    );
}

#[test]
fn the_pose_copy_carries_the_source_stance_and_the_motion_copy_does_not() {
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    // The clip holds the forearm 40 degrees from its own rest for the whole clip —
    // a *stance*, not a motion. A pose copy carries it; a motion copy anchored at
    // the clip's first sample sees no change and keeps our rest.
    let held = DQuat::from_rotation_x(40.0_f64.to_radians()).to_array();
    let mut clip = walk();
    clip.push(Channel {
        node: 9,
        path: "rotation",
        values: vec![held.to_vec(); 8],
    });
    let source = skeleton(&humanoid(&MIXAMO), 1.0, IDENTITY, &clip);
    let (_, pose, _) = plan(&target, &source, ClipSemantics::Pose);
    let (_, motion, _) = plan(&target, &source, ClipSemantics::Motion);
    // The synthetic palette is built in `humanoid` order: node 9 is bone 8, the
    // forearm, palette slot 8.
    let forearm = 8_usize;
    let first_local = |plan: &sculpto_engine::export_glb::ClipPlan| -> DQuat {
        for channel in &plan.clip.channels {
            if let sculpto_gltf::ClipChannel::Rotation { bone, values } = channel
                && *bone == forearm
            {
                return DQuat::from_array(values[0].map(f64::from));
            }
        }
        panic!("the forearm carries a rotation channel")
    };
    let angle = |value: DQuat| -> f64 { 2.0 * value.w.abs().clamp(-1.0, 1.0).acos().to_degrees() };
    let pose_angle = angle(first_local(&pose));
    let motion_angle = angle(first_local(&motion));
    assert!(
        pose_angle > 20.0,
        "the pose copy carries the held stance, measured {pose_angle:.1}°"
    );
    assert!(
        motion_angle < 5.0,
        "the motion copy keeps the rig's own rest instead, measured {motion_angle:.1}°"
    );
}

#[test]
fn the_motion_copy_keeps_this_rigs_rest_and_transfers_the_swing() {
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    // A source whose rest differs from ours by a 30 degree bone frame, performing
    // the same 40 degree swing the palette then has to show.
    let mut source = humanoid(&MIXAMO);
    source[6].rotation = DQuat::from_rotation_x(30.0_f64.to_radians()).to_array();
    let source = skeleton(&source, 1.0, IDENTITY, &walk());
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Motion);
    // The mixer is not the source's pose: the rest difference is *not* carried,
    // which is the whole point of this semantics on a different body.
    assert!(
        plan.max_motion_degrees > 30.0,
        "the arm swing reached the palette, measured {}",
        plan.max_motion_degrees
    );
    assert!(
        plan.moving_channels >= 2,
        "both the travel and the swing are written, {} moving channels",
        plan.moving_channels
    );
    // Every written channel is a legal rotation: a scaled world matrix read as a
    // quaternion is the failure this asserts against.
    for channel in &plan.clip.channels {
        if let sculpto_gltf::ClipChannel::Rotation { values, .. } = channel {
            for value in values {
                let norm: f32 = value.iter().map(|lane| lane * lane).sum();
                assert!(
                    (norm.sqrt() - 1.0).abs() < 1e-5,
                    "unit quaternion, got {norm}"
                );
            }
        }
    }
}

#[test]
fn a_clip_that_covers_no_required_role_is_refused_by_the_admission_gate() {
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let source = skeleton(&humanoid(&MIXAMO), 1.0, IDENTITY, &walk());
    let (_, plan, palette) = plan(&target, &source, ClipSemantics::Pose);
    let required = vec!["hands".to_owned()];
    let error = admit_clip(
        &plan,
        &palette,
        &required,
        true,
        true,
        ClipTolerances::default().translation_relative,
    )
    .expect_err("a role no mapped bone carries refuses the export");
    assert!(
        error.to_string().contains("hands"),
        "the refusal names the role: {error}"
    );
}

#[test]
fn duplicated_roles_are_reported_with_the_rule_that_resolved_them() {
    // The ghost `CME` family keys a second skeleton at the same rest; a clip that
    // drives both is ambiguous about which bone answers the role.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let mut bones = humanoid(&MIXAMO);
    bones.push(bone("CME Pelvis", None, [0.0, 1.0, 0.0]));
    let source = skeleton(&bones, 1.0, IDENTITY, &[]);
    let actor = Actor::from_glb(&target).expect("target");
    let (rig, palette) = actor_rig(&actor);
    let source_actor = Actor::from_glb(&source).expect("source");
    let correspondence = Correspondence::build(&source_actor, &rig, &palette).expect("built");
    let report = correspondence_report(&correspondence);
    let ambiguous = report["ambiguous"].as_array().expect("the ambiguity table");
    assert!(
        ambiguous.iter().any(|row| row["role"] == "pelvis"),
        "a doubled role is reported: {report}"
    );
    let chosen = ambiguous
        .iter()
        .find(|row| row["role"] == "pelvis")
        .and_then(|row| row["chosen"].as_str())
        .expect("a chosen bone");
    assert_eq!(chosen, "mixamorig:Hips", "the shortest claimed name wins");
}

/// A spine whose `Spine2` carries both shoulders *before* the neck in node order,
/// the Mixamo layout (`LeftShoulder` 101 < `Neck` 104). `shoulder_deg` is the rest
/// angle of the clavicle away from the vertical neck direction; the two bodies
/// differ in it exactly as SSE (17.4°) and Mixamo (33.8°) do on the real fixtures.
fn spine_shoulder_first(names: &[&'static str; 16], shoulder_deg: f64) -> Vec<Bone> {
    let angle = shoulder_deg.to_radians();
    let shoulder = |sign: f64| [sign * 0.10 * angle.sin(), 0.10 * angle.cos(), 0.0];
    vec![
        bone(names[0], None, [0.0, 1.0, 0.0]),
        bone(names[1], Some(0), [0.0, 0.10, 0.0]),
        bone(names[2], Some(1), [0.0, 0.10, 0.0]),
        bone(names[3], Some(2), [0.0, 0.10, 0.0]),
        bone(names[4], Some(3), shoulder(-1.0)),
        bone(names[5], Some(3), shoulder(1.0)),
        bone(names[6], Some(3), [0.0, 0.12, 0.0]),
        bone(names[7], Some(6), [0.0, 0.08, 0.0]),
        bone(names[8], Some(4), [-0.20, 0.0, 0.0]),
        bone(names[9], Some(8), [-0.25, 0.0, 0.0]),
        bone(names[10], Some(5), [0.20, 0.0, 0.0]),
        bone(names[11], Some(10), [0.25, 0.0, 0.0]),
        bone(names[12], Some(0), [-0.10, -0.10, 0.0]),
        bone(names[13], Some(12), [0.0, -0.45, 0.0]),
        bone(names[14], Some(0), [0.10, -0.10, 0.0]),
        bone(names[15], Some(14), [0.0, -0.45, 0.0]),
    ]
}

const MIXAMO_SPINE: [&str; 16] = [
    "mixamorig:Hips",
    "mixamorig:Spine",
    "mixamorig:Spine1",
    "mixamorig:Spine2",
    "mixamorig:LeftShoulder",
    "mixamorig:RightShoulder",
    "mixamorig:Neck",
    "mixamorig:Head",
    "mixamorig:LeftArm",
    "mixamorig:LeftForeArm",
    "mixamorig:RightArm",
    "mixamorig:RightForeArm",
    "mixamorig:LeftUpLeg",
    "mixamorig:LeftLeg",
    "mixamorig:RightUpLeg",
    "mixamorig:RightLeg",
];

const SSE_SPINE: [&str; 16] = [
    "NPC Pelvis [Pelv]",
    "NPC Spine [Spn0]",
    "NPC Spine1 [Spn1]",
    "NPC Spine2 [Spn2]",
    "NPC L Clavicle [LClv]",
    "NPC R Clavicle [RClv]",
    "NPC Neck [Neck]",
    "NPC Head [Head]",
    "NPC L UpperArm [LUar]",
    "NPC L Forearm [LLar]",
    "NPC R UpperArm [RUar]",
    "NPC R Forearm [RLar]",
    "NPC L Thigh [LThg]",
    "NPC L Calf [LClf]",
    "NPC R Thigh [RThg]",
    "NPC R Calf [RClf]",
];

#[test]
fn the_spine_aims_at_its_successor_not_the_shoulder_that_sorts_first() {
    // 0146: `direction_reference` took the first mapped child in node order, which
    // under Mixamo's Spine2 is LeftShoulder, not Neck. The aim then matched the
    // clavicle direction (0.001° on the written fixture) and dragged the neck
    // 26.12° off the source's sagittal direction, because the two bodies' rest
    // neck-vs-clavicle angles differ (17.41° ours, 33.78° source). Here the source
    // holds its rest (identity spine channels) and the clavicle rest angles differ
    // 40° vs 10°: aiming at the shoulder would tilt the written neck ~30° away
    // from the source's vertical neck; aiming at the successor keeps it aligned.
    let target = skeleton(&spine_shoulder_first(&SSE_SPINE, 10.0), 1.0, IDENTITY, &[]);
    let held: Vec<Vec<f64>> = (0..8).map(|_| IDENTITY.to_vec()).collect();
    let clip = vec![
        Channel {
            node: 1,
            path: "translation",
            values: (0..8)
                .map(|index| vec![0.0, 1.0, 0.4 * index as f64 / 7.0])
                .collect(),
        },
        Channel {
            node: 1,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 2,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 3,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 4,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 5,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 6,
            path: "rotation",
            values: held.clone(),
        },
        Channel {
            node: 7,
            path: "rotation",
            values: held,
        },
    ];
    let source = skeleton(
        &spine_shoulder_first(&MIXAMO_SPINE, 40.0),
        1.0,
        IDENTITY,
        &clip,
    );
    let (correspondence, plan, _) = plan(&target, &source, ClipSemantics::Pose);
    // The written world rotation of Spine2 is the composition of the written
    // locals down the chain (the planner derives each local against its written
    // parent, so FK composes them back exactly).
    let local = |slot: usize| -> DQuat {
        plan.clip
            .channels
            .iter()
            .find_map(|channel| match channel {
                sculpto_gltf::ClipChannel::Rotation { bone, values } if *bone == slot => {
                    Some(DQuat::from_array(values[0].map(f64::from)))
                }
                _ => None,
            })
            .unwrap_or(DQuat::IDENTITY)
    };
    let spine2_world = local(0) * local(1) * local(2) * local(3);
    // The neck rest offset is straight up in the palette; the source's own neck
    // direction is straight up in its world, taken through the same fitted frame
    // the aim uses.
    let neck_direction = spine2_world * DVec3::Y;
    let expected = correspondence.similarity.rotation * DVec3::Y;
    let error = neck_direction.angle_between(expected).to_degrees();
    assert!(
        error < 3.0,
        "Spine2 must carry the source's neck direction, measured {error:.2}° off \
         (aiming at the shoulder that sorts first gives ~30° on this fixture)"
    );
}

/// One palette bone's written local rotation over the clip: the angle between
/// its first sample and its furthest sample, degrees.
fn written_amplitude(plan: &sculpto_engine::export_glb::ClipPlan, slot: usize) -> f64 {
    let values = plan.clip.channels.iter().find_map(|channel| match channel {
        sculpto_gltf::ClipChannel::Rotation { bone, values } if *bone == slot => Some(values),
        _ => None,
    });
    let Some(values) = values else { return 0.0 };
    let first = DQuat::from_array(values[0].map(f64::from));
    values.iter().fold(0.0_f64, |worst, value| {
        let sample = DQuat::from_array(value.map(f64::from));
        let angle = 2.0 * first.dot(sample).abs().clamp(-1.0, 1.0).acos().to_degrees();
        worst.max(angle)
    })
}

/// A clavicle channel: the source swings its shoulder `degrees` over the clip.
fn clavicle_swing(degrees: f64) -> Channel {
    Channel {
        node: 7,
        path: "rotation",
        values: (0..8)
            .map(|index| {
                let angle = degrees.to_radians() * index as f64 / 7.0;
                DQuat::from_rotation_z(angle).to_array().to_vec()
            })
            .collect(),
    }
}

/// A source skeleton whose clavicle lever is `1 / ratio` of ours and points
/// `tilt` degrees away from our own clavicle direction.
fn clavicle_source(ratio: f64, tilt_degrees: f64) -> Vec<u8> {
    let mut bones = humanoid(&MIXAMO);
    let lever = 0.28 / ratio;
    let tilt = tilt_degrees.to_radians();
    bones[7].translation = [-lever * tilt.cos(), -lever * tilt.sin(), 0.0];
    let mut clip = walk();
    clip.push(clavicle_swing(30.0));
    skeleton(&bones, 1.0, IDENTITY, &clip)
}

#[test]
fn a_short_source_lever_transfers_the_swing_by_tip_travel_not_by_angle() {
    // Mixamo's shape on the measured pair: our clavicle lever is 285.5 mm to its
    // 109.9 mm (2.598x) and the two rest directions differ by 32.7 degrees, so an
    // absolute aim re-seats our shoulder 126 mm off this body's seat. The rule
    // must keep the seat and write the source's *tip travel*.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let source = clavicle_source(2.6, 20.0);
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Pose);
    let row = plan
        .lever_rules
        .iter()
        .find(|row| row.bone.contains("Clavicle"))
        .expect("the clavicle carries a lever row");
    assert!(
        (row.ratio - 2.6).abs() < 0.05,
        "2.6x measured as {:.3} ({} {:.3} m against {:.3} m)",
        row.ratio,
        row.bone,
        row.our_lever,
        row.source_lever
    );
    assert!(
        row.seat_error_mm > 25.0,
        "the tilted rest direction is a re-seat: {:.1} mm",
        row.seat_error_mm
    );
    assert_eq!(
        row.branch, "seat",
        "the lever rule must take the seat branch"
    );
    let written = written_amplitude(&plan, 6);
    let expected = 30.0 / 2.6;
    assert!(
        (written - expected).abs() < 1.0,
        "the clavicle writes {written:.2}° against the expected {expected:.2}° \
         (the source's 30.0° attenuated by the lever ratio)"
    );
    let our_travel = row.our_lever * written.to_radians().sin();
    let source_travel = row.source_lever * 30.0_f64.to_radians().sin();
    assert!(
        (our_travel - source_travel).abs() <= 0.10 * source_travel,
        "tip travel {:.4} m against the source's {source_travel:.4} m",
        our_travel
    );
}

#[test]
fn an_equal_lever_keeps_the_source_angle() {
    // A pair whose clavicles agree in length and direction is untouched: the
    // direction *is* the pose, and the rule must not attenuate it.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let source = clavicle_source(1.0, 0.0);
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Pose);
    let row = plan
        .lever_rules
        .iter()
        .find(|row| row.bone.contains("Clavicle"))
        .expect("the clavicle carries a lever row");
    assert!((row.ratio - 1.0).abs() < 1e-6, "ratio {:.6}", row.ratio);
    assert!(
        row.seat_error_mm < 1e-6,
        "an agreeing rest seat is no re-seat: {:.3} mm",
        row.seat_error_mm
    );
    assert_eq!(row.branch, "aim", "an equal lever keeps the absolute aim");
    let written = written_amplitude(&plan, 6);
    assert!(
        (written - 30.0).abs() < 0.5,
        "the source's 30° reaches the palette, measured {written:.2}°"
    );
}

#[test]
fn a_joint_the_source_barely_moves_is_held_at_rest() {
    // 1 degree of source articulation is noise, not a pose: over a 2.6x lever it
    // would still be a visible tick, so the rule holds the joint at rest instead
    // of transferring it.
    let target = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &[]);
    let mut bones = humanoid(&MIXAMO);
    let lever = 0.28 / 2.6;
    let tilt = 20.0_f64.to_radians();
    bones[7].translation = [-lever * tilt.cos(), -lever * tilt.sin(), 0.0];
    let mut clip = walk();
    clip.push(clavicle_swing(1.0));
    let source = skeleton(&bones, 1.0, IDENTITY, &clip);
    let (_, plan, _) = plan(&target, &source, ClipSemantics::Pose);
    let row = plan
        .lever_rules
        .iter()
        .find(|row| row.bone.contains("Clavicle"))
        .expect("the clavicle carries a lever row");
    assert_eq!(
        row.branch, "hold",
        "a 1.0° source range is held, not aimed at"
    );
    assert!(
        written_amplitude(&plan, 6) < 0.2,
        "a held joint stays at rest, measured {:.3}°",
        written_amplitude(&plan, 6)
    );
}

/// One slot's written rotation samples.
fn written_rotations(plan: &sculpto_engine::export_glb::ClipPlan, slot: usize) -> Vec<DQuat> {
    plan.clip
        .channels
        .iter()
        .find_map(|channel| match channel {
            sculpto_gltf::ClipChannel::Rotation { bone, values } if *bone == slot => Some(
                values
                    .iter()
                    .map(|value| DQuat::from_array(value.map(f64::from)))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The angle between two unit quaternions, degrees.
fn between(first: DQuat, second: DQuat) -> f64 {
    2.0 * first.dot(second).abs().clamp(-1.0, 1.0).acos().to_degrees()
}

/// A target exported through a centimetre FBX pipeline: the armature node carries
/// the Z-up turn and the 0.01 unit factor, and the pelvis node carries the
/// opposite turn, so the body is upright in *world* and a quarter turn out in the
/// pelvis node's own *local*.
fn turned_target() -> Vec<Bone> {
    let mut bones = humanoid(&MIXAMO);
    for bone in &mut bones {
        bone.translation = bone.translation.map(|value| value * 100.0);
    }
    bones[0].rotation = DQuat::from_rotation_x(-std::f64::consts::FRAC_PI_2).to_array();
    bones
}

/// The written local must be anchored in the file parent's frame, not the palette
/// parent's. The rig folds the armature turn out of the pelvis; a player puts it
/// back, so a local written without the fold tips the whole body by 90° while
/// every limb still articulates (0159).
#[test]
fn the_written_root_local_is_anchored_at_the_files_own_parent_frame() {
    let armature = DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2).to_array();
    let target = skeleton(&turned_target(), 0.01, armature, &[]);
    // The pelvis carries its own bend as well as its travel: the frame the player
    // composes is only readable on a bone that has a rotation channel.
    let mut clip = walk();
    clip.push(Channel {
        node: 1,
        path: "rotation",
        values: (0..8)
            .map(|index| {
                let angle = 2.0_f64.to_radians() * index as f64 / 7.0;
                DQuat::from_rotation_z(angle).to_array().to_vec()
            })
            .collect(),
    });
    let source = skeleton(&humanoid(&SSE), 1.0, IDENTITY, &clip);
    let (framed, _frames, _palette) =
        plan_framed(&target, &source, ClipSemantics::Pose, RootMotion::Zero);

    // The fold is measured, not assumed: 90° between the palette joint and the
    // file parent node.
    assert!(
        (framed.palette_fold_degrees - 90.0).abs() < 1.0,
        "the armature turn is measured, got {:.2}° on {:?}",
        framed.palette_fold_degrees,
        framed.palette_fold_bone
    );
    assert_eq!(
        framed.palette_fold_slots, 1,
        "only the pelvis sits under a fold"
    );

    // The pelvis carries no rotation in this clip, so the frame the player will
    // compose must stay at rest through every sample.
    let fold = DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2);
    let rest = DQuat::IDENTITY;
    let worst = written_rotations(&framed, 0)
        .into_iter()
        .fold(0.0_f64, |worst, local| {
            worst.max(between(fold * local, rest))
        });
    assert!(
        worst <= 5.0,
        "the composed root frame must stay at rest, measured {worst:.2}°"
    );

    // The witness: the same clip planned without the file's frames composes to a
    // quarter turn, which is the defect this test exists for.
    let (_, unframed, _) = plan(
        &target,
        &skeleton(&humanoid(&SSE), 1.0, IDENTITY, &clip),
        ClipSemantics::Pose,
    );
    let tilt = written_rotations(&unframed, 0)
        .into_iter()
        .fold(0.0_f64, |worst, local| {
            worst.max(between(fold * local, rest))
        });
    assert!(
        (tilt - 90.0).abs() < 5.0,
        "an unanchored local must tilt the body 90°, measured {tilt:.2}°"
    );
}
