//! `actor torso-fit` fits the axis the mesh decides and refuses the other. @req DECL0013
//!
//! The fixtures are closed boxes: a rectangular section whose area centre is a
//! real landmark on the lateral axis and, on the anterior axis, whatever the soft
//! tissue happens to be. The fit must move a laterally offset spine onto the box's
//! symmetry plane, keep every rest world it does not own, and leave the anterior
//! axis alone.

use serde_json::{Value, json};

#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use common::Fixture;
use glam::{DMat4, DQuat, DVec3};
use sculpto_mesh::actor::{Actor, Decision, PartSelector, Pose, torso_fit, write_frames};

/// The spine chain a box fixture carries, with its rest heights.
const CHAIN: [(&str, f32); 6] = [
    ("NPC Pelvis [Pelv]", 0.40),
    ("NPC Spine [Spn0]", 0.52),
    ("NPC Spine1 [Spn1]", 0.66),
    ("NPC Spine2 [Spn2]", 0.80),
    ("NPC Neck [Neck]", 0.92),
    ("NPC Head [Head]", 1.02),
];

/// Node index of the head in the fixture: Root, then the chain.
const HEAD_NODE: usize = 6;
/// Node index of the neck in the fixture.
const NECK_NODE: usize = 5;

/// A closed box with the six-joint spine inside it and a foot pair that fixes the
/// actor's forward direction.
///
/// `offset` is the lateral (`x`) world position of every torso joint, so zero puts
/// the chain on the box's symmetry plane and any other value is a placement defect
/// the fit must remove.
fn torso_fixture(offset: f32) -> Fixture {
    let mut fixture = Fixture::triangle();
    let spine: Vec<DVec3> = CHAIN
        .iter()
        .map(|(_, height)| DVec3::new(f64::from(offset), f64::from(*height), 0.0))
        .collect();
    let ankle = DVec3::new(0.10, 0.05, 0.05);
    let ball = ankle + DVec3::new(0.0, 0.0, -0.10);

    // Root 0, chain 1..=6, ankle 7, ball 8, body 9.
    let mut nodes = vec![json!({"name": "Root", "children": []})];
    let mut parent = DVec3::ZERO;
    for (index, (name, _)) in CHAIN.iter().enumerate() {
        let mut node = json!({"name": name, "translation": (spine[index] - parent).to_array()});
        if index + 1 < CHAIN.len() {
            node["children"] = json!([index + 2]);
        }
        nodes.push(node);
        parent = spine[index];
    }
    let ankle_node = nodes.len();
    nodes.push(json!({"name": "NPC L Foot [Lft ]", "translation": ankle.to_array()}));
    let ball_node = nodes.len();
    nodes.push(json!({"name": "NPC L Toe0 [LToe]", "translation": (ball - ankle).to_array()}));
    nodes[ankle_node]["children"] = json!([ball_node]);
    let body_node = nodes.len();
    nodes.push(json!({"name": "Body", "mesh": 0, "skin": 0}));
    nodes[0]["children"] = json!([1, ankle_node]);
    fixture.json["nodes"] = json!(nodes);

    // A closed box, outward-wound on every face: 8 corners, 12 triangles,
    // lateral ±0.15, depth ±0.10 (anterior is -Z in this actor).
    let corners: [[f32; 3]; 8] = [
        [-0.15, 0.30, -0.10],
        [0.15, 0.30, -0.10],
        [0.15, 0.30, 0.10],
        [-0.15, 0.30, 0.10],
        [-0.15, 1.10, -0.10],
        [0.15, 1.10, -0.10],
        [0.15, 1.10, 0.10],
        [-0.15, 1.10, 0.10],
    ];
    let faces: [[u32; 3]; 12] = [
        [0, 1, 2],
        [0, 2, 3],
        [4, 6, 5],
        [4, 7, 6],
        [0, 4, 5],
        [0, 5, 1],
        [3, 2, 6],
        [3, 6, 7],
        [1, 5, 6],
        [1, 6, 2],
        [0, 3, 7],
        [0, 7, 4],
    ];
    let points: Vec<f32> = corners.iter().flatten().copied().collect();
    let positions = fixture.floats(&points, "VEC3", corners.len());
    fixture.json["accessors"][positions]["min"] = json!([-0.15, 0.30, -0.10]);
    fixture.json["accessors"][positions]["max"] = json!([0.15, 1.10, 0.10]);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
    let indices: Vec<u8> = faces
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let index_accessor = fixture.accessor(&indices, 5125, "SCALAR", 36);
    fixture.json["meshes"][0]["primitives"][0]["indices"] = json!(index_accessor);

    // Every joint of the skin, with the inverse binds that make the rest pose
    // reproduce the authored box: `world * inverse_bind` is the identity.
    let joints: Vec<usize> = (1..=CHAIN.len()).chain([ankle_node, ball_node]).collect();
    let mut worlds: Vec<DMat4> = spine
        .iter()
        .map(|point| DMat4::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, *point))
        .collect();
    for point in [ankle, ball] {
        worlds.push(DMat4::from_scale_rotation_translation(
            DVec3::ONE,
            DQuat::IDENTITY,
            point,
        ));
    }
    let binds: Vec<f32> = worlds
        .iter()
        .flat_map(|world| world.inverse().to_cols_array().map(|value| value as f32))
        .collect();
    let bind_accessor = fixture.floats(&binds, "MAT4", joints.len());
    fixture.json["skins"] = json!([{"joints": joints, "inverseBindMatrices": bind_accessor}]);

    // Every vertex blends the pelvis and the first spine joint: any weight row
    // will do, the fit reads pivots and not influences.
    let rows: Vec<u8> = (0..corners.len())
        .flat_map(|_| vec![0_u8, 1, 0, 0])
        .collect();
    let joint_accessor = fixture.accessor(&rows, 5121, "VEC4", corners.len());
    let weights: Vec<f32> = (0..corners.len())
        .flat_map(|_| vec![0.5, 0.5, 0.0, 0.0])
        .collect();
    let weight_accessor = fixture.floats(&weights, "VEC4", corners.len());
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joint_accessor);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weight_accessor);
    let _ = body_node;
    fixture
}

fn body() -> PartSelector {
    PartSelector::parse("Body").unwrap()
}

/// The written document's node translations, indexed by node.
fn node_translations(written: &[u8]) -> Vec<[f64; 3]> {
    let framing = gltf::binary::Glb::from_slice(written).expect("GLB framing");
    let json: Value = serde_json::from_slice(&framing.json).expect("GLB JSON");
    json["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|node| {
            let values = node["translation"].as_array().cloned().unwrap_or_default();
            [0, 1, 2].map(|axis| values.get(axis).and_then(Value::as_f64).unwrap_or_default())
        })
        .collect()
}

#[test]
fn a_lateral_offset_is_fitted_onto_the_symmetry_plane() {
    let bytes = torso_fixture(0.02).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = torso_fit(&actor, &body(), &[]).expect("fit");

    let spine = outcome
        .joints
        .iter()
        .find(|joint| joint.level == "spine1")
        .expect("spine1 measured");
    assert_eq!(spine.lateral_decision, Decision::Fitted);
    assert!(
        (spine.lateral_before - 0.02).abs() < 5e-4,
        "lateral offset {} m",
        spine.lateral_before
    );
    assert!(spine.lateral_after.abs() < 5e-4);
    assert!(
        spine.pivot_after[0].abs() < 5e-4,
        "the pivot must land on the box's centre, found {}",
        spine.pivot_after[0]
    );
    // The anterior axis is measured and never moved.
    assert_eq!(spine.anterior_decision, Decision::Unresolved);
    assert!((spine.pivot_after[2] - spine.pivot_before[2]).abs() < 1e-9);
    // A box 0.20 m deep with the pivot in its middle: 0.10 m from the back face.
    assert!(
        (spine.posterior_standoff.unwrap() - 0.10).abs() < 1e-3,
        "standoff {:?}",
        spine.posterior_standoff
    );
    assert!((spine.fraction_from_anterior.unwrap() - 0.5).abs() < 0.01);
}

#[test]
fn the_fit_keeps_every_rest_world_and_the_rest_geometry() {
    let bytes = torso_fixture(0.02).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let before = actor.sample(&["Body"], Pose::Rest).expect("rest");
    let outcome = torso_fit(&actor, &body(), &[]).expect("fit");
    let written = write_frames(&bytes, &outcome.frames).expect("write");
    let fitted = Actor::from_glb(&written).expect("re-read");

    let after = fitted.sample(&["Body"], Pose::Rest).expect("rest");
    let shift = before
        .vertices
        .iter()
        .zip(&after.vertices)
        .map(|(left, right)| DVec3::from_array(*left).distance(DVec3::from_array(*right)))
        .fold(0.0_f64, f64::max);
    assert!(shift < 1e-6, "rest geometry moved by {shift:e} m");

    // The neck is fitted, so it lands on the symmetry plane; the head is not this
    // chain's to move, so its rest world survives through a compensating local.
    let translations = node_translations(&written);
    assert!(
        translations[NECK_NODE][0].abs() < 5e-4,
        "the neck must sit on the plane, found {}",
        translations[NECK_NODE][0]
    );
    let head_local = translations[HEAD_NODE][0];
    assert!(
        (head_local - 0.02).abs() < 5e-4,
        "the head must keep its rest world through a local of 0.02, found {head_local}"
    );
}

#[test]
fn a_chain_on_the_symmetry_plane_is_agreed_and_not_written() {
    let bytes = torso_fixture(0.0).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = torso_fit(&actor, &body(), &[]).expect("fit");
    assert_eq!(outcome.moved_joints, 0);
    for joint in &outcome.joints {
        assert_eq!(
            joint.lateral_decision,
            Decision::Agreed,
            "{} reported {:?}",
            joint.level,
            joint.lateral_decision
        );
    }
}

#[test]
fn a_palette_without_a_spine_chain_is_refused() {
    let mut fixture = torso_fixture(0.0);
    let nodes = fixture.json["nodes"].as_array_mut().unwrap();
    for node in nodes.iter_mut() {
        if node["name"].as_str().is_some_and(|name| {
            name.contains("Spine") || name.contains("Pelvis") || name.contains("Neck")
        }) {
            node["name"] = json!("Limb");
        }
    }
    let actor = Actor::from_glb(&fixture.bytes()).expect("fixture");
    let error = torso_fit(&actor, &body(), &[]).expect_err("a spine-less palette must refuse");
    assert!(
        error.to_string().contains("spine chain"),
        "refusal named: {error}"
    );
}

#[test]
fn the_reported_frame_is_the_actor_frame() {
    let bytes = torso_fixture(0.0).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = torso_fit(&actor, &body(), &[]).expect("fit");
    assert_eq!(outcome.frame.anterior_source, "feet");
    assert!(
        outcome.frame.anterior[2] < -0.99,
        "{:?}",
        outcome.frame.anterior
    );
    assert!(
        outcome.frame.lateral[0] > 0.99,
        "{:?}",
        outcome.frame.lateral
    );
    assert!(outcome.frame.up[1] > 0.99, "{:?}", outcome.frame.up);
}
