//! `actor shoulder-seat` reads a seat off the surface and refuses to invent a depth. @req DECL0013
//!
//! The fixtures are closed boxes with an SSE-shaped palette inside them. Placing
//! the clavicle root in front of the box makes the flesh prove it is outside;
//! placing it in the middle makes it agreed. Neither case may move a pivot: a
//! surface decides the sign of a seat and nothing about an interior target.

use serde_json::{Value, json};

#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use common::Fixture;
use glam::{DMat4, DQuat, DVec3};
use sculpto_mesh::actor::{Actor, Decision, PartSelector, SeatJoint, shoulder_seat};

/// The joints the seat reads, in node order: chain 1..=5, clavicles, arms, feet.
const CHAIN: [(&str, f32); 5] = [
    ("NPC Pelvis [Pelv]", 0.40),
    ("NPC Spine [Spn0]", 0.52),
    ("NPC Spine1 [Spn1]", 0.66),
    ("NPC Spine2 [Spn2]", 0.80),
    ("NPC Neck [Neck]", 0.98),
];

/// A closed box with an SSE-shaped shoulder girdle inside it.
///
/// `clavicle_anterior` is the `z` of both clavicle roots; the box spans
/// `z ∈ [-0.10, 0.10]` and its anterior face is `-z`, so a value below `-0.10`
/// puts the roots in front of the skin.
fn seat_fixture(clavicle_anterior: f32) -> Fixture {
    let mut fixture = Fixture::triangle();
    let mut nodes = vec![json!({"name": "Root", "children": []})];
    let mut worlds: Vec<DVec3> = Vec::new();
    let mut parent = DVec3::ZERO;
    let mut attach = Vec::new();
    for (name, height) in CHAIN {
        let point = DVec3::new(0.0, f64::from(height), 0.0);
        let index = nodes.len();
        nodes.push(json!({"name": name, "translation": (point - parent).to_array()}));
        if index > 1 {
            nodes[index - 1]["children"] = json!([index]);
        }
        worlds.push(point);
        attach.push(index);
        parent = point;
    }
    // The girdle hangs off Spine2, as the SSE skeleton's does.
    let spine2 = attach[3];
    let spine = worlds[3];
    let mut girdle = Vec::new();
    for (side, sign) in [('L', -1.0_f64), ('R', 1.0_f64)] {
        let clavicle = DVec3::new(sign * 0.05, 0.95, f64::from(clavicle_anterior));
        let shoulder = DVec3::new(sign * 0.12, 0.92, 0.0);
        let elbow = DVec3::new(sign * 0.12, 0.80, 0.0);
        let wrist = DVec3::new(sign * 0.12, 0.70, 0.0);
        let clavicle_node = nodes.len();
        nodes.push(json!({
            "name": format!("NPC {side} Clavicle [{}Clv]", if side == 'L' { "L" } else { "R" }),
            "translation": (clavicle - spine).to_array(),
        }));
        worlds.push(clavicle);
        let shoulder_node = nodes.len();
        nodes.push(json!({
            "name": format!("NPC {side} UpperArm [{}Uar]", if side == 'L' { "L" } else { "R" }),
            "translation": (shoulder - clavicle).to_array(),
        }));
        worlds.push(shoulder);
        let elbow_node = nodes.len();
        nodes.push(json!({
            "name": format!("NPC {side} Forearm [{}Lar]", if side == 'L' { "L" } else { "R" }),
            "translation": (elbow - shoulder).to_array(),
        }));
        worlds.push(elbow);
        let wrist_node = nodes.len();
        nodes.push(json!({
            "name": format!("NPC {side} Hand [{}Hnd]", if side == 'L' { "L" } else { "R" }),
            "translation": (wrist - elbow).to_array(),
        }));
        worlds.push(wrist);
        nodes[clavicle_node]["children"] = json!([shoulder_node]);
        nodes[shoulder_node]["children"] = json!([elbow_node]);
        nodes[elbow_node]["children"] = json!([wrist_node]);
        girdle.push(clavicle_node);
    }
    let spine2_girdle: Vec<usize> = girdle.clone();
    nodes[spine2]["children"] = json!(spine2_girdle);

    // A foot pair fixes the actor's forward direction: anterior is -Z.
    let ankle = DVec3::new(0.10, 0.05, 0.05);
    let ball = ankle + DVec3::new(0.0, 0.0, -0.10);
    let ankle_node = nodes.len();
    nodes.push(json!({"name": "NPC L Foot [Lft ]", "translation": ankle.to_array()}));
    worlds.push(ankle);
    let ball_node = nodes.len();
    nodes.push(json!({"name": "NPC L Toe0 [LToe]", "translation": (ball - ankle).to_array()}));
    worlds.push(ball);
    nodes[ankle_node]["children"] = json!([ball_node]);
    nodes[attach[0]]["children"] = json!([]);
    let body_node = nodes.len();
    nodes.push(json!({"name": "Body", "mesh": 0, "skin": 0}));
    nodes[0]["children"] = json!([attach[0], ankle_node]);
    fixture.json["nodes"] = json!(nodes);

    // A closed box, outward-wound on every face: lateral ±0.15, depth ±0.10.
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

    // Every node of the skin, with inverse binds that reproduce the authored box.
    let joints: Vec<usize> = (1..=CHAIN.len())
        .chain(girdle.iter().copied())
        .chain([ankle_node, ball_node])
        .collect();
    let binds: Vec<f32> = joints
        .iter()
        .flat_map(|node| {
            let point = match *node {
                1..=5 => worlds[*node - 1],
                _ => worlds[*node - 1],
            };
            let world = DMat4::from_scale_rotation_translation(DVec3::ONE, DQuat::IDENTITY, point);
            world.inverse().to_cols_array().map(|value| value as f32)
        })
        .collect();
    let bind_accessor = fixture.floats(&binds, "MAT4", joints.len());
    fixture.json["skins"] = json!([{"joints": joints, "inverseBindMatrices": bind_accessor}]);

    // Every vertex blends the pelvis and Spine0; the seat reads pivots, not weights.
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

fn clavicle(outcome: &sculpto_mesh::actor::SeatOutcome, side: char) -> &SeatJoint {
    outcome
        .joints
        .iter()
        .find(|joint| joint.level == "clavicle" && joint.side == side)
        .expect("clavicle measured")
}

fn wall(joint: &SeatJoint, direction: &str) -> Option<f64> {
    joint
        .walls
        .iter()
        .find(|wall| wall.direction == direction)
        .and_then(|wall| wall.distance)
}

#[test]
fn a_clavicle_root_in_front_of_the_skin_is_reported_outside_with_its_excursion() {
    let bytes = seat_fixture(-0.20).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = shoulder_seat(&actor, &body(), &[]).expect("seat");

    for side in ['L', 'R'] {
        let joint = clavicle(&outcome, side);
        assert!(
            (joint.signed - 0.10).abs() < 1e-3,
            "{side} clavicle excursion {} m, wanted 0.10",
            joint.signed
        );
        assert!(joint.nearest > 0.09 && joint.nearest < 0.11);
        assert_eq!(
            joint.decision,
            Decision::Unresolved,
            "an outside pivot is a finding, not a fit"
        );
        // Nothing is in front of the root, so the anterior ray cannot cross.
        assert_eq!(
            wall(joint, "anterior"),
            None,
            "the anterior side must read as open air"
        );
        // The box's front face is the nearest surface, 0.10 m posterior of the root.
        let posterior = wall(joint, "posterior").expect("the box behind the root");
        assert!(
            (posterior - 0.10).abs() < 1e-3,
            "{side} posterior wall {posterior}"
        );
    }
    assert_eq!(
        outcome.outside(),
        2,
        "only the two clavicle roots are outside"
    );
}

#[test]
fn a_clavicle_root_inside_the_flesh_is_agreed() {
    let bytes = seat_fixture(0.0).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = shoulder_seat(&actor, &body(), &[]).expect("seat");
    for side in ['L', 'R'] {
        let joint = clavicle(&outcome, side);
        assert!(joint.signed < 0.0, "{side} signed {}", joint.signed);
        assert_eq!(joint.decision, Decision::Agreed);
        assert!(
            (joint.nearest - 0.10).abs() < 1e-3,
            "{side} nearest {} m",
            joint.nearest
        );
        assert!(wall(joint, "anterior").is_some() && wall(joint, "posterior").is_some());
    }
    assert_eq!(outcome.outside(), 0);
}

#[test]
fn the_shoulder_joint_and_the_arm_context_are_measured_on_every_side() {
    let bytes = seat_fixture(0.0).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = shoulder_seat(&actor, &body(), &[]).expect("seat");
    let levels: Vec<(char, &str, bool)> = outcome
        .joints
        .iter()
        .map(|joint| (joint.side, joint.level, joint.owned))
        .collect();
    for side in ['L', 'R'] {
        assert!(levels.contains(&(side, "clavicle", true)));
        assert!(levels.contains(&(side, "shoulder", true)));
        assert!(levels.contains(&(side, "elbow", false)));
        assert!(levels.contains(&(side, "wrist", false)));
    }
    // The girdle hangs off Spine2 and the link is reported, not inferred.
    let joint = clavicle(&outcome, 'L');
    assert_eq!(
        joint.parent.as_deref().map(|name| &name[..9]),
        Some("NPC Spine")
    );
    let link = joint.parent_link.expect("link");
    let length = (link[0] * link[0] + link[1] * link[1] + link[2] * link[2]).sqrt();
    assert!(length > 0.10 && length < 0.20, "link {length} m");
    // A box has no neck taper, so the trapezius line is honestly absent and the
    // rows carry no "below the line" claim instead of a made-up one.
    assert!(
        outcome.neck_base.is_none(),
        "a box body has no trapezius line to find"
    );
    assert!(joint.below_neck_base.is_none());
}

#[test]
fn a_palette_without_a_girdle_is_refused() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "children": [1]},
        {"name": "NPC Pelvis [Pelv]", "translation": [0.0, 0.4, 0.0], "children": [2]},
        {"name": "NPC Spine [Spn0]", "translation": [0.0, 0.1, 0.0], "children": [3]},
        {"name": "NPC Spine1 [Spn1]", "translation": [0.0, 0.1, 0.0], "children": [4]},
        {"name": "NPC Spine2 [Spn2]", "translation": [0.0, 0.1, 0.0]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1, 2, 3, 4]}]);
    let rows: Vec<u8> = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0];
    let joint_accessor = fixture.accessor(&rows, 5121, "VEC4", 3);
    let weights: Vec<f32> = [0.5, 0.5, 0.0, 0.0].repeat(3);
    let weight_accessor = fixture.floats(&weights, "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joint_accessor);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weight_accessor);
    let bytes = fixture.bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let error = shoulder_seat(&actor, &body(), &[]).expect_err("no girdle");
    assert!(error.to_string().contains("Clavicle"), "error: {error}");
}

#[test]
fn the_reported_frame_is_the_actor_frame() {
    let bytes = seat_fixture(0.0).bytes();
    let actor = Actor::from_glb(&bytes).expect("fixture");
    let outcome = shoulder_seat(&actor, &body(), &[]).expect("seat");
    // The fixture's foot points to -Z, so that is the anterior axis and Y is up.
    assert!(
        (outcome.frame.anterior[2] + 1.0).abs() < 1e-6,
        "{:?}",
        outcome.frame
    );
    assert!((outcome.frame.up[1] - 1.0).abs() < 1e-6);
    assert_eq!(outcome.frame.anterior_source, "feet");
    assert!(outcome.frame.lateral[0] > 0.99);
    let value: Value = json!(outcome.frame.lateral);
    assert_eq!(value.as_array().map(Vec::len), Some(3));
}
