mod common;

use common::Fixture;
use sculpto_mesh::actor::{Actor, ActorError, Pose, RigRole};
use serde_json::json;

fn near(actual: [f64; 3], expected: [f64; 3]) {
    actual
        .into_iter()
        .zip(expected)
        .for_each(|(a, b)| assert!((a - b).abs() < 1e-12, "{a} != {b}"));
}

/// Root 0 connects spine/hand/fingers and, through an unweighted connector
/// node that no skin lists, a distant toe; the body part is skinned onto the
/// fingers only, so every influence must collapse onto retained ancestors.
fn branching_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [10., 0., 0.], "children": [1, 6, 5]},
        {"name": "Spine", "translation": [0., 1., 0.], "children": [2]},
        {"name": "Hand", "translation": [0., 2., 0.], "children": [3, 4]},
        {"name": "FingerL", "translation": [0., 0., 3.]},
        {"name": "FingerR", "translation": [0., 0., 4.]},
        {"name": "Body", "mesh": 0, "skin": 0, "translation": [99., 0., 0.]},
        {"name": "Bridge", "translation": [0., 5., 0.], "children": [7]},
        {"name": "Toe", "translation": [0., 0., 7.]},
    ]);
    fixture.json["skins"] = json!([{"joints": [3, 4]}]);
    let joints = fixture.accessor(&[0, 1, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[0.25, 0.75, 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    fixture
}

#[test]
fn connectors_retain_paths_roles_and_order() {
    let actor = Actor::from_glb(&branching_fixture().bytes()).unwrap();
    let rig = actor
        .rig_sample(&["Body"], Pose::Rest, &["Spine", "Hand", "Toe"])
        .unwrap();
    assert_eq!(
        rig.rig.iter().map(|node| node.node).collect::<Vec<_>>(),
        [0, 1, 6, 2, 7],
        "parent-before-child by the validated topological order"
    );
    assert_eq!(rig.rig[0].role, RigRole::Connector);
    assert_eq!(rig.rig[1].role, RigRole::Requested);
    assert_eq!(rig.rig[2].role, RigRole::Connector, "unweighted connector");
    assert_eq!(rig.rig[3].role, RigRole::Requested);
    assert_eq!(rig.rig[4].role, RigRole::Requested);
    assert_eq!(rig.rig[0].parent, None, "the LCA root ends the rig");
    assert_eq!(rig.rig[1].parent, Some(0));
    assert_eq!(rig.rig[2].parent, Some(0));
    assert_eq!(rig.rig[3].parent, Some(1));
    assert_eq!(rig.rig[4].parent, Some(2));
    assert_eq!(rig.rig[2].name.as_deref(), Some("Bridge"));
    assert_eq!(
        rig.requested
            .iter()
            .map(|joint| joint.rig)
            .collect::<Vec<_>>(),
        [1, 3, 4],
        "caller order maps into the retained rig"
    );
    assert_eq!(rig.requested[1].name, "Hand");
    assert_eq!(rig.requested[1].node, 2);
}

#[test]
fn finger_influences_collapse_onto_the_nearest_retained_ancestor() {
    let actor = Actor::from_glb(&branching_fixture().bytes()).unwrap();
    let rig = actor
        .rig_sample(&["Body"], Pose::Rest, &["Hand", "FingerL"])
        .unwrap();
    let hand = rig.requested[0].rig;
    let finger = rig.requested[1].rig;
    assert_eq!(
        rig.rig.iter().map(|node| node.node).collect::<Vec<_>>(),
        [2, 3],
        "paths stop at the LCA Hand; Root, Spine and FingerR stay out"
    );
    let part = rig.sample.parts.first().expect("one skinned part");
    (part.vertices.start..part.vertices.end).for_each(|vertex| {
        assert_eq!(
            rig.influences[vertex]
                .iter()
                .map(|binding| (binding.rig, binding.weight))
                .collect::<Vec<_>>(),
            [(hand, 0.75), (finger, 0.25)],
            "weights accumulate on the real nearest retained ancestors"
        );
    });
}

#[test]
fn skin_local_joint_indices_map_through_each_skins_table() {
    let mut fixture = branching_fixture();
    // A second skinned part repeats the same JOINTS_0/WEIGHTS_0 bytes, but its
    // skin lists the shared finger nodes in the opposite order.
    fixture.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "Other", "mesh": 1, "skin": 1, "translation": [0., 0., 0.]}));
    fixture.json["nodes"][0]["children"]
        .as_array_mut()
        .unwrap()
        .push(json!(8));
    let body_attributes = fixture.json["meshes"][0]["primitives"][0]["attributes"].clone();
    fixture.json["meshes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"primitives": [{"attributes": {
            "POSITION": 0,
            "JOINTS_0": body_attributes["JOINTS_0"],
            "WEIGHTS_0": body_attributes["WEIGHTS_0"],
        }}]}));
    fixture.json["skins"]
        .as_array_mut()
        .unwrap()
        .push(json!({"joints": [4, 3]}));
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    let rig = actor
        .rig_sample(&["Body", "Other"], Pose::Rest, &["Hand", "FingerL"])
        .unwrap();
    let hand = rig.requested[0].rig;
    let finger = rig.requested[1].rig;
    let [body, other] = &rig.sample.parts[..] else {
        panic!("two single-primitive parts expected");
    };
    let body_vertex = body.vertices.start;
    let other_vertex = other.vertices.start;
    let weights = |vertex: usize| {
        rig.influences[vertex]
            .iter()
            .map(|binding| (binding.rig, binding.weight))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        weights(body_vertex),
        [(hand, 0.75), (finger, 0.25)],
        "skin 0: local 0 is FingerL, local 1 is FingerR -> Hand"
    );
    assert_eq!(
        weights(other_vertex),
        [(hand, 0.25), (finger, 0.75)],
        "skin 1 swaps the same nodes, so identical bytes bind differently"
    );
}

#[test]
fn requested_joints_resolve_by_name_in_a_renamed_reordered_graph() {
    let mut fixture = branching_fixture();
    // Rebuild the same hierarchy with reordered ids and one renamed joint.
    fixture.json["nodes"] = json!([
        {"name": "Toe", "translation": [0., 0., 7.]},
        {"name": "Bridge", "translation": [0., 5., 0.], "children": [0]},
        {"name": "Root", "translation": [10., 0., 0.], "children": [1, 3, 7]},
        {"name": "Chest", "translation": [0., 1., 0.], "children": [4]},
        {"name": "Hand", "translation": [0., 2., 0.], "children": [5, 6]},
        {"name": "FingerR", "translation": [0., 0., 4.]},
        {"name": "FingerL", "translation": [0., 0., 3.]},
        {"name": "Body", "mesh": 0, "skin": 0, "translation": [99., 0., 0.]},
    ]);
    fixture.json["skins"] = json!([{"joints": [6, 5]}]);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    let rig = actor
        .rig_sample(&["Body"], Pose::Rest, &["Hand", "Chest", "Toe"])
        .unwrap();
    assert_eq!(
        rig.requested
            .iter()
            .map(|joint| joint.node)
            .collect::<Vec<_>>(),
        [4, 3, 0],
        "names resolve in the reordered graph, not by old row ids"
    );
    let hand = &rig.rig[rig.requested[0].rig];
    near(
        [hand.world[3][0], hand.world[3][1], hand.world[3][2]],
        [10., 3., 0.],
    );
    let toe = &rig.rig[rig.requested[2].rig];
    near(
        [toe.world[3][0], toe.world[3][1], toe.world[3][2]],
        [10., 5., 7.],
    );
}

#[test]
fn rig_worlds_share_the_pose_evaluation_and_geometry_stays_unchanged() {
    let mut fixture = branching_fixture();
    fixture.channel(2, "translation", &[0., 1.], &[0., 2., 0., 1., 2., 0.]);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    let pose = Pose::Clip {
        name: "Run",
        time: 0.5,
    };
    let rig = actor.rig_sample(&["Body"], pose, &["Hand"]).unwrap();
    assert_eq!(
        rig.sample,
        actor.sample(&["Body"], pose).unwrap(),
        "rig metadata must not change sampled geometry"
    );
    assert_ne!(
        rig.sample,
        actor.sample(&["Body"], Pose::Rest).unwrap(),
        "REST keeps asset defaults, not clip time zero"
    );
    let hand = &rig.rig[rig.requested[0].rig];
    near(
        [hand.world[3][0], hand.world[3][1], hand.world[3][2]],
        [10.5, 3., 0.],
    );
    let rest = actor.rig_sample(&["Body"], Pose::Rest, &["Hand"]).unwrap();
    let rest_hand = &rest.rig[rest.requested[0].rig];
    near(
        [
            rest_hand.world[3][0],
            rest_hand.world[3][1],
            rest_hand.world[3][2],
        ],
        [10., 3., 0.],
    );
}

#[test]
fn unskinned_donors_and_joint_selection_fail_contextually() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [1., 0., 0.], "children": [1]},
        {"name": "Body", "mesh": 0},
    ]);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    match actor.rig_sample(&["Body"], Pose::Rest, &["Root"]) {
        Err(ActorError::Unsupported { context, feature }) => {
            assert!(context.contains("Body"), "{context}");
            assert_eq!(feature, "unskinned mesh");
        }
        other => panic!("unskinned donor must fail, got {other:?}"),
    }
    let mut ambiguous = branching_fixture();
    ambiguous.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "Hand"}));
    let actor = Actor::from_glb(&ambiguous.bytes()).unwrap();
    assert!(matches!(
        actor.rig_sample(&["Body"], Pose::Rest, &["Hand"]),
        Err(ActorError::JointSelection { matches: 2, .. })
    ));
    assert!(matches!(
        actor.rig_sample(&["Body"], Pose::Rest, &["missing"]),
        Err(ActorError::JointSelection { matches: 0, .. })
    ));
    let actor = Actor::from_glb(&branching_fixture().bytes()).unwrap();
    assert!(matches!(
        actor.rig_sample(&["Body"], Pose::Rest, &["Hand", "Hand"]),
        Err(ActorError::DuplicateJoint { .. })
    ));
    assert!(matches!(
        actor.rig_sample(&["Body"], Pose::Rest, &[]),
        Err(ActorError::Invalid { .. })
    ));
}

#[test]
fn disconnected_roots_and_unmatched_influences_are_typed_failures() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "RootA", "translation": [10., 0., 0.], "children": [1, 2]},
        {"name": "HandA", "translation": [0., 2., 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
        {"name": "RootB", "translation": [-10., 0., 0.], "children": [4, 5]},
        {"name": "LimbB", "translation": [0., 1., 0.]},
        {"name": "ToeB", "translation": [0., 0., 5.]},
    ]);
    fixture.json["skins"] = json!([{"joints": [4]}]);
    let joints = fixture.accessor(&[0, 0, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[1., 0., 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    assert!(matches!(
        actor.rig_sample(&["Body"], Pose::Rest, &["HandA", "ToeB"]),
        Err(ActorError::DisconnectedRig)
    ));
    match actor.rig_sample(&["Body"], Pose::Rest, &["HandA"]) {
        Err(ActorError::Invalid { reason, .. }) => {
            assert!(reason.contains("no retained ancestor"), "{reason}");
        }
        other => panic!("influence under another root must fail, got {other:?}"),
    }
}

#[test]
fn shared_skin_fixture_maps_its_two_reduced_influences() {
    let mut fixture = Fixture::triangle();
    fixture.skin();
    fixture.json["nodes"][1]["name"] = json!("Joint");
    fixture.json["nodes"][3]["name"] = json!("Tip");
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    let rig = actor.rig_sample(&["Body"], Pose::Rest, &["Joint"]).unwrap();
    assert_eq!(rig.rig.len(), 1, "Tip collapses onto the retained Joint");
    near(rig.sample.vertices[0], [11., 2., 2.25]);
    let joint = rig.requested[0].rig;
    let part = rig.sample.parts.first().expect("one skinned part");
    (part.vertices.start..part.vertices.end).for_each(|vertex| {
        assert_eq!(
            rig.influences[vertex]
                .iter()
                .map(|binding| (binding.rig, binding.weight))
                .collect::<Vec<_>>(),
            [(joint, 1.0)],
            "both source influences share the retained ancestor"
        );
    });
}

#[test]
fn existing_skin_validation_unchanged_through_the_rig_path() {
    (0..3).for_each(|case| {
        let mut fixture = branching_fixture();
        let attributes = fixture.json["meshes"][0]["primitives"][0]["attributes"].clone();
        match case {
            0 => {
                fixture.json["meshes"][0]["primitives"][0]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("WEIGHTS_0");
            }
            1 => {
                let index = attributes["WEIGHTS_0"].as_u64().unwrap() as usize;
                let view = fixture.json["accessors"][index]["bufferView"]
                    .as_u64()
                    .unwrap() as usize;
                let offset = fixture.json["bufferViews"][view]["byteOffset"]
                    .as_u64()
                    .unwrap() as usize;
                fixture.bin[offset..offset + 4].copy_from_slice(&0.5_f32.to_le_bytes());
            }
            _ => {
                let index = attributes["JOINTS_0"].as_u64().unwrap() as usize;
                let view = fixture.json["accessors"][index]["bufferView"]
                    .as_u64()
                    .unwrap() as usize;
                let offset = fixture.json["bufferViews"][view]["byteOffset"]
                    .as_u64()
                    .unwrap() as usize;
                fixture.bin[offset] = 9;
            }
        }
        let result = Actor::from_glb(&fixture.bytes())
            .and_then(|actor| actor.rig_sample(&["Body"], Pose::Rest, &["Hand"]));
        assert!(result.is_err(), "case {case}");
    });
}
