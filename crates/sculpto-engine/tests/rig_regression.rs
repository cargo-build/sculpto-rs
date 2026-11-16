//! The rig regression's instruments, on data the test owns.
//!
//! The pure cores (chain folding, clearance, gates) run on plain vectors; the
//! end-to-end test builds two synthetic actors — an own-walk artifact whose
//! root-motion bone bobs and banks under a rigid spine, and an adapted
//! artifact that adds a broken spine transfer on top — and asserts the whole
//! verb down to the named failing row.
//!
//! The synthetic layout reproduces the defect class the harness exists for:
//! the spine hangs beside the pelvis under a `COM` root, so a pelvis-carriage
//! leak in the chain frame would masquerade as bending, and a wrong frame
//! resolution (a rig index read as a node id) would pick a joint the clip
//! never drove.
//!
//! @req BODY-01

use std::borrow::Cow;

use glam::{DMat4, DQuat, DVec3};
use gltf::binary::Glb;
use sculpto_engine::regression::{
    ClearanceFrame, RegressionActor, RegressionParams, chain_aligned, chain_local, clearance_gate,
    clearance_metrics, clothing_gate, rest_frame_axes, run_params, spine_gate, spine_metrics,
    stretch_gate,
};
use serde_json::{Value, json};

/// One synthetic bone: name, parent, and its rest local transform.
#[derive(Clone, Copy)]
struct Bone {
    name: &'static str,
    parent: Option<usize>,
    translation: [f64; 3],
}

fn bone(name: &'static str, parent: Option<usize>, translation: [f64; 3]) -> Bone {
    Bone {
        name,
        parent,
        translation,
    }
}

/// One animation channel over a synthetic skeleton; `node` is the document
/// node index (bone index + 1, below the armature).
struct Channel {
    node: usize,
    path: &'static str,
    values: Vec<Vec<f64>>,
}

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

fn align(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}

/// Build one skinned, animated GLB whose clip is named `walk`.
fn skeleton(bones: &[Bone], clip: &[Channel]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let push = |bin: &mut Vec<u8>,
                views: &mut Vec<Value>,
                accessors: &mut Vec<Value>,
                bytes: Vec<u8>,
                count: usize,
                component: usize,
                kind: &str|
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
            "componentType": component,
            "count": count,
            "type": kind,
        }));
        accessors.len() - 1
    };

    let mut worlds = vec![DMat4::IDENTITY; bones.len()];
    for (index, definition) in bones.iter().enumerate() {
        let local = DMat4::from_translation(DVec3::from_array(definition.translation));
        worlds[index] = match definition.parent {
            Some(parent) => worlds[parent] * local,
            None => local,
        };
    }
    let mut bind: Vec<f64> = Vec::with_capacity(bones.len() * 16);
    for world in &worlds {
        bind.extend(world.inverse().to_cols_array());
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
        bones.len(),
        5126,
        "MAT4",
    );

    // One triangle, weighted to the first bone.
    let positions: Vec<u8> = [[0.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    let position_accessor = push(
        &mut bin,
        &mut views,
        &mut accessors,
        positions,
        3,
        5126,
        "VEC3",
    );
    accessors[position_accessor]["min"] = json!([0.0, 0.0, 0.0]);
    accessors[position_accessor]["max"] = json!([0.0, 1.0, 1.0]);
    let joints: Vec<u8> = [0_u16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let joint_accessor = push(
        &mut bin,
        &mut views,
        &mut accessors,
        joints,
        3,
        5123,
        "VEC4",
    );
    let weights: Vec<u8> = [[1.0_f32, 0.0, 0.0, 0.0]; 3]
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    let weight_accessor = push(
        &mut bin,
        &mut views,
        &mut accessors,
        weights,
        3,
        5126,
        "VEC4",
    );
    let indices: Vec<u8> = [0_u16, 1, 2]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    let index_accessor = push(
        &mut bin,
        &mut views,
        &mut accessors,
        indices,
        3,
        5123,
        "SCALAR",
    );

    let mut nodes: Vec<Value> = Vec::with_capacity(bones.len() + 2);
    nodes.push(json!({"name": "Armature", "children": []}));
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
            "children": children,
        }));
    }
    let mesh_node = nodes.len();
    nodes.push(json!({"name": "Body", "mesh": 0, "skin": 0}));
    let roots: Vec<usize> = bones
        .iter()
        .enumerate()
        .filter(|(_, definition)| definition.parent.is_none())
        .map(|(index, _)| index + 1)
        .collect();
    let mut armature_children = roots;
    armature_children.push(mesh_node);
    nodes[0]["children"] = json!(armature_children);

    let mut samplers: Vec<Value> = Vec::new();
    let mut channels: Vec<Value> = Vec::new();
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
            times.len(),
            5126,
            "SCALAR",
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
                channel.values.len(),
                5126,
                match width {
                    4 => "VEC4",
                    _ => "VEC3",
                },
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
        "animations": match clip.is_empty() {
            true => json!([]),
            false => json!([{"name": "walk", "channels": channels, "samplers": samplers}]),
        },
    });
    frame(document, bin)
}

/// The synthetic body: a `COM` root carrying the spine beside the pelvis, so
/// the chain's frame is the root-motion bone and not the pelvis.
fn body() -> Vec<Bone> {
    vec![
        bone("COM", None, [0.0, 1.0, 0.0]),                // 0
        bone("Hips", Some(0), [0.0, -0.05, 0.0]),          // 1: pelvis role
        bone("LeftUpLeg", Some(1), [-0.09, -0.05, 0.0]),   // 2
        bone("RightUpLeg", Some(1), [0.09, -0.05, 0.0]),   // 3
        bone("LeftLeg", Some(2), [0.0, -0.40, 0.0]),       // 4
        bone("RightLeg", Some(3), [0.0, -0.40, 0.0]),      // 5
        bone("LeftToeBase", Some(4), [0.0, -0.40, 0.08]),  // 6
        bone("RightToeBase", Some(5), [0.0, -0.40, 0.08]), // 7
        bone("Spine", Some(0), [0.0, 0.08, 0.0]),          // 8: sibling of the pelvis
        bone("Spine1", Some(8), [0.0, 0.09, 0.0]),         // 9
        bone("Spine2", Some(9), [0.0, 0.09, 0.0]),         // 10
        bone("Neck", Some(10), [0.0, 0.09, 0.0]),          // 11
        bone("LeftHand", Some(10), [-0.25, 0.02, 0.0]),    // 12
        bone("RightHand", Some(10), [0.25, 0.02, 0.0]),    // 13
    ]
}

fn yaw(theta_radians: f64) -> Vec<f64> {
    let rotation = DQuat::from_rotation_y(theta_radians);
    rotation.to_array().to_vec()
}

/// Root carriage: a 5 mm bob and a ±2° bank on `COM`, plus the same bank on
/// the pelvis. Written identically on both artifacts.
fn carriage(samples: usize) -> Vec<Channel> {
    // Document node indices: bones start at 1, below the armature.
    let com_node = 1;
    let hips_node = 2;
    let mut translation = Vec::with_capacity(samples);
    let mut com_rotation = Vec::with_capacity(samples);
    let mut hips_rotation = Vec::with_capacity(samples);
    for index in 0..samples {
        let phase = index as f64 / (samples - 1).max(1) as f64 * std::f64::consts::TAU;
        translation.push(vec![0.0, 0.005 * phase.sin(), 0.0]);
        com_rotation.push(yaw(0.035 * phase.sin()));
        hips_rotation.push(yaw(0.035 * phase.sin()));
    }
    vec![
        Channel {
            node: com_node,
            path: "translation",
            values: translation,
        },
        Channel {
            node: com_node,
            path: "rotation",
            values: com_rotation,
        },
        Channel {
            node: hips_node,
            path: "rotation",
            values: hips_rotation,
        },
    ]
}

fn write_artifact(name: &str, bones: &[Bone], clip: &[Channel]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("sculpto-rig-regression-test-{name}.glb"));
    std::fs::write(&path, skeleton(bones, clip)).expect("a test artifact writes");
    path
}

// ---------------------------------------------------------------------------
// pure cores
// ---------------------------------------------------------------------------

/// A vertical five-joint chain, parent-relative.
fn straight_chain() -> Vec<DVec3> {
    (0..5)
        .map(|index| DVec3::new(0.0, 0.1 * index as f64, 0.0))
        .collect()
}

#[test]
fn a_chain_rigid_to_its_frame_measures_zero_under_any_carriage() {
    let rest = straight_chain();
    let mut frames = Vec::new();
    for index in 0..10 {
        let phase = index as f64 / 9.0;
        // The frame joint itself bobs, banks and pitches; the chain rides with
        // it, so the folded positions must not move.
        let carriage = DMat4::from_rotation_translation(
            DQuat::from_euler(
                glam::EulerRot::XYZ,
                0.2 * phase.sin(),
                0.5 * phase.sin(),
                0.1 * phase.sin(),
            ),
            DVec3::new(1.2 * phase, 0.9 + 0.05 * phase.sin(), -0.7 * phase),
        );
        let world: Vec<DVec3> = rest
            .iter()
            .map(|point| carriage.transform_point3(*point))
            .collect();
        // The product folds before it measures; the round trip through the
        // same function is part of what is under test.
        let folded = world
            .iter()
            .map(|point| chain_local(carriage, *point))
            .collect::<Vec<DVec3>>();
        frames.push((phase, folded));
    }
    let metrics = spine_metrics(&rest, &frames);
    assert!(metrics.peak_lateral_deviation_mm < 1e-6, "{metrics:?}");
    assert!(metrics.peak_lateral_tilt_deg < 1e-6, "{metrics:?}");
    assert!(metrics.peak_sagittal_tilt_deg < 1e-6, "{metrics:?}");
}

#[test]
fn a_lateral_bend_is_measured_wherever_the_carriage_points() {
    let rest = straight_chain();
    // Bend the spine segment 30 degrees sideways about the sagittal axis,
    // first with the body facing forward, then facing the other way: the
    // measured bend must be the bend, not the facing.
    let bend = DQuat::from_rotation_z(30_f64.to_radians());
    for facing in [0.0_f64, std::f64::consts::PI] {
        let frame = DMat4::from_rotation_y(facing);
        let posed: Vec<DVec3> = rest
            .iter()
            .enumerate()
            .map(|(index, point)| match index {
                // The spine root stays; everything above it swings 30°
                // sideways around it.
                0 | 1 => *point,
                _ => rest[1] + bend * (*point - rest[1]),
            })
            .collect();
        let posed: Vec<DVec3> = posed.iter().map(|p| frame.transform_point3(*p)).collect();
        let metrics = spine_metrics(&rest, &[(0.5, posed)]);
        assert!(
            (metrics.peak_lateral_tilt_deg - 30.0).abs() < 0.5,
            "facing {facing}: {metrics:?}"
        );
        let expected = 0.3_f64 * 30_f64.to_radians().sin() * 1000.0;
        assert!(
            (metrics.peak_lateral_deviation_mm - expected).abs() < 5.0,
            "facing {facing}: {metrics:?}"
        );
    }
}

#[test]
fn the_connector_into_the_spine_never_peaks_or_gates() {
    let rest = straight_chain();
    // The pelvis drifts sideways under a static spine: segment 0 moves, the
    // spine segments stay rigid, and the reported peaks stay zero.
    let posed: Vec<DVec3> = rest
        .iter()
        .enumerate()
        .map(|(index, point)| match index {
            0 => *point + DVec3::new(0.08, 0.0, 0.0),
            _ => *point,
        })
        .collect();
    let metrics = spine_metrics(&rest, &[(0.5, posed)]);
    assert!(
        metrics.peak_lateral_tilt_deg < 1e-9 && metrics.peak_sagittal_tilt_deg < 1e-9,
        "{metrics:?}"
    );
    assert!(
        metrics.segments[0].peak_lateral_deg > 30.0,
        "the connector is still reported: {metrics:?}"
    );
}

#[test]
fn clearance_names_the_closest_joint_and_target() {
    let frames = vec![ClearanceFrame {
        time: 0.25,
        joints: vec![
            ("L hand".to_owned(), DVec3::new(-0.30, 1.0, 0.0)),
            ("R finger".to_owned(), DVec3::new(0.14, 0.70, 0.0)),
        ],
        targets: vec![
            ("pelvis".to_owned(), DVec3::ZERO, DVec3::ZERO),
            (
                "R thigh".to_owned(),
                DVec3::new(0.09, 0.9, 0.0),
                DVec3::new(0.09, 0.5, 0.0),
            ),
        ],
    }];
    let metrics = clearance_metrics(&frames);
    // The R finger sits 50 mm from the R thigh segment in x, inside its span.
    assert!((metrics.min_mm - 50.0).abs() < 1e-6, "{metrics:?}");
    assert_eq!(metrics.joint, "R finger");
    assert_eq!(metrics.target, "R thigh");
    assert!((metrics.time - 0.25).abs() < 1e-9);
}

#[test]
fn the_spine_gate_refuses_on_the_floor_when_the_own_walk_is_straight() {
    let straight = spine_metrics(&straight_chain(), &[(0.5, straight_chain())]);
    let mut bent = straight.clone();
    bent.peak_lateral_deviation_mm = 138.0;
    bent.peak_lateral_tilt_deg = 12.0;
    let verdict = spine_gate(&straight, &bent, "actor/adapted");
    assert!(!verdict.pass, "{verdict:?}");
    assert_eq!(verdict.gate, "spine_lateral");
    // The floors exist so a straight default cannot absorb the gate; a small
    // sway under them passes.
    let mut sway = straight.clone();
    sway.peak_lateral_deviation_mm = 4.0;
    sway.peak_lateral_tilt_deg = 1.0;
    let verdict = spine_gate(&straight, &sway, "actor/adapted");
    assert!(verdict.pass, "{verdict:?}");
}

#[test]
fn the_clearance_gate_refuses_only_a_crossing() {
    // A hand joint inside the 25 mm flesh-envelope floor is a crossing even
    // when the own walk keeps a wide corridor.
    let mut default = clearance_metrics(&[]);
    default.min_mm = 276.0;
    let mut adapted = clearance_metrics(&[]);
    adapted.min_mm = 14.6;
    let verdict = clearance_gate(&default, &adapted, "actor/adapted");
    assert!(!verdict.pass, "{verdict:?}");
    assert!(verdict.detail.contains("CROSSING"), "{verdict:?}");
    // Outside the envelope the row passes even when the corridor halves: the
    // narrowing is the reference clip's own authored carriage (012D: the
    // reference body reaches 0.24 mm arm-to-torso), reported as advisory.
    let mut adapted = clearance_metrics(&[]);
    adapted.min_mm = 124.6;
    let verdict = clearance_gate(&default, &adapted, "actor/adapted");
    assert!(verdict.pass, "{verdict:?}");
    assert!(verdict.detail.contains("advisory"), "{verdict:?}");
}

#[test]
fn the_clearance_gate_is_vacuous_when_the_own_clip_crosses() {
    let mut default = clearance_metrics(&[]);
    default.min_mm = 0.0;
    let mut adapted = clearance_metrics(&[]);
    adapted.min_mm = 0.0;
    let verdict = clearance_gate(&default, &adapted, "actor/adapted");
    assert!(verdict.pass, "{verdict:?}");
    assert!(verdict.detail.contains("vacuous"));
}

#[test]
fn the_stretch_gate_is_relative_with_a_hard_zero_case() {
    let verdict = stretch_gate(8.67, 11.98, "actor/adapted");
    assert!(!verdict.pass, "{verdict:?}");
    let verdict = stretch_gate(8.67, 8.0, "actor/adapted");
    assert!(verdict.pass, "{verdict:?}");
    let verdict = stretch_gate(0.0, 2.0, "actor/adapted");
    assert!(!verdict.pass, "{verdict:?}");
}

#[test]
fn the_clothing_gate_takes_the_absolute_rule_when_the_anchor_passes() {
    let verdict = clothing_gate(2.1, 1.962, 7.841, "Gait", "emily/adapted");
    assert!(!verdict.pass, "{verdict:?}");
    assert!(verdict.detail.contains("passing"), "{verdict:?}");
}

#[test]
fn the_clothing_gate_takes_the_delta_when_the_anchor_fails() {
    let verdict = clothing_gate(2.1, 26.698, 36.370, "WalkForward", "mike/adapted");
    assert!(!verdict.pass, "{verdict:?}");
    assert!(verdict.detail.contains("delta"), "{verdict:?}");
    // And a delta within the bar passes even though both rows fail absolutely.
    let verdict = clothing_gate(2.1, 26.698, 25.0, "WalkForward", "mike/adapted");
    assert!(verdict.pass, "{verdict:?}");
}

// ---------------------------------------------------------------------------
// end to end on synthetic actors
// ---------------------------------------------------------------------------

#[test]
fn a_rotated_rest_frame_does_not_turn_a_sway_into_a_flip() {
    // The measured bug, pinned: on these palettes the chain runs along the
    // frame joint's local +Z (COM's rest convention), so the world-lateral
    // projection of a 122 mm segment is a 5 mm sliver. The unaligned metric
    // turned an 11 mm sway into a ~118 degree tilt; the alignment must keep
    // the projection the length of the segment and read the real angle.
    let frame_world_rest = DMat4::from_rotation_x(-std::f64::consts::FRAC_PI_2);
    let axes = rest_frame_axes(frame_world_rest);
    let rest_local = DVec3::new(0.0, 5.34, 122.37);
    let posed_local = DVec3::new(11.14, -0.52, 121.83);
    let rest_aligned = chain_aligned(axes, rest_local);
    let posed_aligned = chain_aligned(axes, posed_local);
    assert!(
        rest_aligned.y.abs() > 100.0,
        "the rest segment stands along world up after alignment: {rest_aligned}"
    );
    let dot = rest_aligned.x * posed_aligned.x + rest_aligned.y * posed_aligned.y;
    let length = (rest_aligned.x * rest_aligned.x + rest_aligned.y * rest_aligned.y).sqrt()
        * (posed_aligned.x * posed_aligned.x + posed_aligned.y * posed_aligned.y).sqrt();
    let degrees = (dot / length).clamp(-1.0, 1.0).acos().to_degrees();
    assert!(
        degrees < 15.0,
        "an 11 mm sway on a 122 mm segment is ~5 degrees, measured {degrees:.2}"
    );
}

#[test]
fn the_harness_names_the_rows_the_broken_transfer_breaks() {
    let bones = body();
    let default_path = write_artifact("e2e-default", &bones, &carriage(31));
    let mut adapted_clip = carriage(31);
    // The broken transfer: a 90-degree sideways flip written into the spine
    // root, on top of the same carriage.
    let bend = DQuat::from_rotation_z(90_f64.to_radians())
        .to_array()
        .to_vec();
    adapted_clip.push(Channel {
        node: 9, // the Spine bone's document node
        path: "rotation",
        values: vec![bend.clone(); 31],
    });
    let adapted_path = write_artifact("e2e-adapted", &bones, &adapted_clip);

    let params = RegressionParams {
        actors: vec![RegressionActor {
            name: "synthetic".to_owned(),
            default: default_path,
            adapted: adapted_path,
            default_clip: "walk".to_owned(),
            adapted_clip: "walk".to_owned(),
            stress_anchor_clip: None,
            stress_clips: Vec::new(),
            body: "Body".to_owned(),
            garments: Vec::new(),
        }],
        frames: 8,
        tolerance: 0.0021,
        report: None,
    };
    let report = run_params(&params).expect("the synthetic pair measures");
    let actor = &report["actors"][0];
    let default_spine = &actor["default"]["spine"];
    assert_eq!(
        default_spine["peak_lateral_tilt_deg"].as_f64().unwrap(),
        0.0,
        "the own walk folds its carriage out: {default_spine}"
    );
    assert_eq!(
        default_spine["peak_lateral_deviation_mm"].as_f64().unwrap(),
        0.0,
        "{default_spine}"
    );
    let adapted_spine = &actor["adapted"]["spine"];
    assert!(
        adapted_spine["peak_lateral_deviation_mm"].as_f64().unwrap() > 100.0,
        "the broken spine transfer shows: {adapted_spine}"
    );
    assert_eq!(
        actor["default"]["spine_frame_joint"], "COM",
        "the chain is measured in the spine's own parent"
    );
    let failed = report["failed_rows"]
        .as_array()
        .expect("a verdict")
        .iter()
        .filter_map(|row| row.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        // The hands hang off the spine chain, so the 90-degree flip narrows
        // the hand-to-hip corridor too — but 110 mm is outside the 25 mm
        // crossing floor, so the narrowing reports as advisory and only the
        // spine row fails the run.
        failed,
        vec!["synthetic/adapted:spine_lateral"],
        "the whole verdict: {report}"
    );
}
