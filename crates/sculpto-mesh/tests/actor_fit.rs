//! `actor fit` keeps the rest pose and moves only pivots. @req DECL0013
//!
//! A chain the fit cannot see (a partial palette) is a refusal, and a fitted
//! skin must reproduce the input's rest geometry exactly, because the rewrite
//! changes each joint's inverse bind matrix together with its rest frame.

use serde_json::json;

#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use common::Fixture;
use sculpto_mesh::actor::{Actor, LengthMode, Pose, fit, write_fit};

/// Two hand bones and three flesh bands: a knuckle the hand and index share, an
/// index phalanx band, and a tip the last bone dominates alone.
fn hand_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [0., 0., 0.], "children": [1]},
        {"name": "NPC L Hand [LHnd]", "translation": [0., 0., 0.], "children": [2]},
        {"name": "NPC L Finger10 [LF10]", "translation": [0.04, 0., 0.], "children": [3]},
        {"name": "NPC L Finger11 [LF11]", "translation": [0.03, 0., 0.], "children": [4]},
        {"name": "NPC L Finger12 [LF12]", "translation": [0.03, 0., 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1, 2, 3, 4]}]);
    // Six triangles, two per flesh band, along +X so a band has enough vertices
    // to be a band.
    let mut points = Vec::new();
    for band in 0..3 {
        let base = 0.02 + 0.06 * band as f32;
        points.extend_from_slice(&[
            base,
            0.0,
            0.0,
            base + 0.01,
            0.0,
            0.0,
            base,
            0.01,
            0.0,
            base + 0.01,
            0.0,
            0.0,
            base + 0.01,
            0.01,
            0.0,
            base,
            0.01,
            0.0,
        ]);
    }
    let positions = fixture.floats(&points, "VEC3", 18);
    fixture.json["accessors"][positions]["min"] = json!([0.02, 0.0, 0.0]);
    fixture.json["accessors"][positions]["max"] = json!([0.14, 0.01, 0.0]);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
    // Knuckle band: the finger root dominates with the hand blending in. Middle
    // band: the root and middle bone share it. Tip: the distal bone alone.
    let rows: Vec<[u8; 4]> = vec![
        [0, 1, 0, 0],
        [0, 1, 0, 0],
        [0, 1, 0, 0],
        [0, 1, 0, 0],
        [0, 1, 0, 0],
        [0, 1, 0, 0],
        [1, 2, 0, 0],
        [1, 2, 0, 0],
        [1, 2, 0, 0],
        [1, 2, 0, 0],
        [1, 2, 0, 0],
        [1, 2, 0, 0],
        [2, 3, 0, 0],
        [2, 3, 0, 0],
        [2, 3, 0, 0],
        [2, 3, 0, 0],
        [2, 3, 0, 0],
        [2, 3, 0, 0],
    ];
    let joints = fixture.accessor(&rows.concat(), 5121, "VEC4", 18);
    let weights: Vec<f32> = [
        [0.4, 0.6, 0.0, 0.0],
        [0.4, 0.6, 0.0, 0.0],
        [0.4, 0.6, 0.0, 0.0],
        [0.4, 0.6, 0.0, 0.0],
        [0.4, 0.6, 0.0, 0.0],
        [0.4, 0.6, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.5, 0.5, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
        [0.2, 0.8, 0.0, 0.0],
    ]
    .concat();
    let weights = fixture.floats(&weights, "VEC4", 18);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    let identity: Vec<f32> = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]
    .repeat(4);
    let inverse: Vec<f32> = (0..4)
        .flat_map(|joint| {
            let mut matrix = identity[joint * 16..(joint + 1) * 16].to_vec();
            let node = 1 + joint;
            let translation = match node {
                1 | 5 => [0.0, 0.0, 0.0],
                2 => [-0.04, 0.0, 0.0],
                3 => [-0.07, 0.0, 0.0],
                _ => [-0.10, 0.0, 0.0],
            };
            matrix[12] = translation[0];
            matrix[13] = translation[1];
            matrix[14] = translation[2];
            matrix
        })
        .collect();
    let accessor = fixture.floats(&inverse, "MAT4", 4);
    fixture.json["skins"][0]["inverseBindMatrices"] = json!(accessor);
    fixture
}

#[test]
fn fit_moves_the_joint_to_the_blend_band_and_keeps_the_rest_pose() {
    let bytes = hand_fixture().bytes();
    let actor = Actor::from_glb(&bytes).unwrap();
    let parts = ["Body"];
    let before = actor.sample(&parts, Pose::Rest).unwrap();
    let outcome = fit(&actor, &parts, LengthMode::Flesh).unwrap();
    assert_eq!(
        outcome.chains.len(),
        1,
        "one finger chain; unfitted: {:?}",
        outcome.unfitted
    );
    let chain = &outcome.chains[0];
    assert_eq!(chain.kind, "finger");
    // The knuckle band is centred 0.025 along +X, so the index root lands there.
    let root = &chain.joints[0];
    assert_eq!(
        root.placed_by.as_str(),
        "blend_band",
        "band of {} vertices, span {:.4}",
        root.band_vertices,
        root.band_span
    );
    assert!(
        (root.after_head[0] - 0.025).abs() < 1e-3,
        "{:?}",
        root.after_head
    );
    assert!(
        chain.axis_after_degrees < 5.0,
        "the fitted chain follows the flesh axis, {}",
        chain.axis_after_degrees
    );
    assert!((root.perpendicular_after - 0.0).abs() < 1e-3);

    let written = write_fit(&bytes, &outcome).unwrap();
    let fitted = Actor::from_glb(&written).unwrap();
    let after = fitted.sample(&parts, Pose::Rest).unwrap();
    for (left, right) in before.vertices.iter().zip(&after.vertices) {
        let delta = [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
        let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        assert!(distance < 1e-6, "rest vertex moved {distance}");
    }
}

#[test]
fn a_palette_without_extremity_chains_is_refused() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [0., 0., 0.], "children": [1]},
        {"name": "Torso", "translation": [0., 0., 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1]}]);
    let joints = fixture.accessor(&[0, 0, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[1., 0., 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    let bytes = fixture.bytes();
    let actor = Actor::from_glb(&bytes).unwrap();
    let error = fit(&actor, &["Body"], LengthMode::Flesh).unwrap_err();
    assert!(error.to_string().contains("no extremity chain"), "{error}");
}
