#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use common::Fixture;
use sculpto_mesh::actor::{Actor, GaitParameters, gait, write_gait_clip};
use serde_json::json;

/// A minimal standard-palette skeleton (no mesh): the gait drives joint local
/// transforms and measures world positions, so it needs only the named rig.
fn gait_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Scene Root", "children": [1]},
        {"name": "NPC COM [COM ]", "translation": [0., 0.96, 0.], "children": [2]},
        {
            "name": "NPC Pelvis [Pelv]",
            "translation": [0., 0., 0.],
            "children": [3, 7, 11, 13, 15],
        },
        {"name": "NPC L Thigh [LThg]", "translation": [-0.1, 0., 0.], "children": [4]},
        {"name": "NPC L Calf [LClf]", "translation": [0., -0.45, 0.], "children": [5]},
        {"name": "NPC L Foot [Lft ]", "translation": [0., -0.4, 0.], "children": [6]},
        {"name": "NPC L Toe0 [LToe]", "translation": [0., -0.05, 0.12]},
        {"name": "NPC R Thigh [RThg]", "translation": [0.1, 0., 0.], "children": [8]},
        {"name": "NPC R Calf [RClf]", "translation": [0., -0.45, 0.], "children": [9]},
        {"name": "NPC R Foot [Rft ]", "translation": [0., -0.4, 0.], "children": [10]},
        {"name": "NPC R Toe0 [RToe]", "translation": [0., -0.05, 0.12]},
        {"name": "NPC L UpperArm [LUar]", "translation": [-0.2, 0.5, 0.], "children": [12]},
        {"name": "NPC L Forearm [LLar]", "translation": [0., -0.3, 0.]},
        {"name": "NPC R UpperArm [RUar]", "translation": [0.2, 0.5, 0.], "children": [14]},
        {"name": "NPC R Forearm [RLar]", "translation": [0., -0.3, 0.]},
        // The torso hangs off the hips, so the head inherits the pelvis yaw and
        // the counter track has something to cancel.
        {"name": "NPC Spine [Spn0]", "translation": [0., 0.2, 0.], "children": [16]},
        {"name": "NPC Spine2 [Spn2]", "translation": [0., 0.2, 0.], "children": [17]},
        {"name": "NPC Head [Head]", "translation": [0., 0.1, 0.]},
    ]);
    // The gait never samples geometry; drop the triangle mesh the base fixture
    // carries so this is a pure skeleton.
    fixture.json["meshes"] = json!([]);
    fixture
}

#[test]
fn gait_is_symmetric_and_bobs_the_pelvis_by_the_requested_amount() {
    let actor = Actor::from_glb(&gait_fixture().bytes()).unwrap();
    // 29 keyframes: the phase grid lands on the bob extrema and is invariant
    // under the half-cycle L/R shift, so the measures are tight.
    let params = GaitParameters {
        pelvis_bob: 0.03,
        samples: 29,
        ..GaitParameters::default()
    };
    let outcome = gait(&actor, &params).unwrap();
    let measures = &outcome.measures;
    // Left and right run the same waveforms half a cycle apart on a mirrored
    // skeleton; the shared pelvis bob leaves only a small discretization split,
    // far inside the 10% acceptance gate.
    let split = (measures.foot_lift_left - measures.foot_lift_right).abs()
        / measures.foot_lift_left.max(measures.foot_lift_right);
    assert!(split < 0.02, "foot-lift split {split}");
    assert!((measures.arm_swing_left_deg - measures.arm_swing_right_deg).abs() < 0.5);
    assert!(measures.foot_lift_left > 0.0);
    assert!(measures.arm_swing_left_deg > 10.0);
    // The bob is applied along world-up and its extrema are sampled, so the
    // range equals the parameter.
    assert!((measures.pelvis_vertical_range - 0.03).abs() < 1e-9);
    assert_eq!(outcome.times.len(), params.samples);
    assert_eq!(outcome.times[0], 0.0);
    assert!((outcome.times[params.samples - 1] - params.cycle_seconds).abs() < 1e-9);
    // 12 limb rotation tracks + pelvis yaw + head counter, and one bob.
    assert_eq!(outcome.rotations.len(), 14);
    assert_eq!(outcome.translations.len(), 1);
}

#[test]
fn the_head_counter_holds_the_gaze_while_the_chest_follows_the_pelvis() {
    let actor = Actor::from_glb(&gait_fixture().bytes()).unwrap();
    let params = GaitParameters {
        samples: 29,
        ..GaitParameters::default()
    };
    let outcome = gait(&actor, &params).unwrap();
    let measures = &outcome.measures;
    assert!(
        (measures.chest_yaw_deg - 2.0 * params.pelvis_rotation_deg).abs() < 0.5,
        "the chest follows the pelvis: {}",
        measures.chest_yaw_deg
    );
    assert!(
        measures.head_yaw_deg < 0.5,
        "the head counter leaves the face forward: {}",
        measures.head_yaw_deg
    );
    // Without the counter the pelvis yaw reaches the face unchanged.
    let uncountered = gait(
        &actor,
        &GaitParameters {
            head_counter: 0.0,
            ..params
        },
    )
    .unwrap();
    assert!(
        (uncountered.measures.head_yaw_deg - measures.chest_yaw_deg).abs() < 0.5,
        "no counter means the head turns with the chest: {}",
        uncountered.measures.head_yaw_deg
    );
}

#[test]
fn a_head_counter_outside_the_unit_interval_is_a_named_refusal() {
    let actor = Actor::from_glb(&gait_fixture().bytes()).unwrap();
    let refused = gait(
        &actor,
        &GaitParameters {
            head_counter: 1.5,
            ..GaitParameters::default()
        },
    )
    .expect_err("a counter above one over-rotates the head");
    assert!(format!("{refused}").contains("head counter in 0..=1"));
}

#[test]
fn written_gait_clip_is_readable_and_keeps_the_idle_slot() {
    let mut fixture = gait_fixture();
    // Give the skeleton an idle clip so the gait must land at the walk slot.
    fixture.channel(2, "rotation", &[0., 1.], &[0., 0., 0., 1., 0., 0., 0., 1.]);
    fixture.json["animations"][0]["name"] = json!("Idle");
    let bytes = fixture.bytes();
    let actor = Actor::from_glb(&bytes).unwrap();
    let outcome = gait(&actor, &GaitParameters::default()).unwrap();
    let (glb, index) = write_gait_clip(&bytes, &outcome, "Gait").unwrap();
    assert_eq!(index, 1); // idle stays at 0, gait takes the walk slot
    let written = Actor::from_glb(&glb).unwrap();
    let names = written.clip_names();
    assert_eq!(names[0], Some("Idle"));
    assert_eq!(names[1], Some("Gait"));
    let (start, end) = written.clip_span("Gait").unwrap();
    assert_eq!(start, 0.0);
    assert!((end - GaitParameters::default().cycle_seconds).abs() < 1e-6);
}

#[test]
fn gait_refuses_a_missing_role_joint_and_bad_parameters() {
    let mut fixture = gait_fixture();
    fixture.json["nodes"][4]["name"] = json!("Not a calf");
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    assert!(gait(&actor, &GaitParameters::default()).is_err());

    let actor = Actor::from_glb(&gait_fixture().bytes()).unwrap();
    let bad = GaitParameters {
        samples: 1,
        ..GaitParameters::default()
    };
    assert!(gait(&actor, &bad).is_err());
    let bad = GaitParameters {
        cycle_seconds: 0.0,
        ..GaitParameters::default()
    };
    assert!(gait(&actor, &bad).is_err());
}
