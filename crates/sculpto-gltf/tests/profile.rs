//! Integration tests for the consumer profile morph cap (DA10/E04).
//!
//! A profile refusal must name the profile and its measurement before any
//! byte is written; a count within the measured cap writes the same GLB the
//! no-profile writer produces.

use gltf::Gltf;
use sculpto_core::mesh::{
    MaterialSlot, MeshAsset, MorphDelta, MorphScope, MorphTarget, PartitionId,
};
use sculpto_gltf::profile::{ConsumerProfile, Measured};
use sculpto_gltf::{GltfError, write_glb, write_glb_with_profile};

/// A plain three-vertex triangle base for testing.
fn triangle() -> MeshAsset {
    MeshAsset {
        name: "morphed".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

/// [`triangle`] carrying three named morph targets.
fn morphed() -> MeshAsset {
    let mut asset = triangle();
    asset.morphs = ["Bend", "Fold", "Sag"]
        .into_iter()
        .enumerate()
        .map(|(index, name)| MorphTarget {
            name: name.into(),
            deltas: vec![MorphDelta {
                vertex: index as u32,
                position: [0.0, 0.0, 1.0],
            }],
            scope: MorphScope::Whole,
        })
        .collect();
    asset
}

/// The profile documented in [`ConsumerProfile`]: the A00B measurement of
/// the Bevy `bevy_mesh` morph-weight constant on resolved 0.20.1.
fn bevy(cap: usize) -> ConsumerProfile {
    ConsumerProfile {
        name: "bevy".into(),
        requirement: "0.20".into(),
        resolved: "0.20.1".into(),
        max_morph_targets: Measured {
            value: cap,
            command: "rg MAX_MORPH_WEIGHTS bevy_mesh-0.20.1/src/morph.rs".into(),
            date: "2026-11-10".into(),
        },
    }
}

#[test]
fn a_count_above_the_measured_cap_is_refused_naming_the_profile() {
    let asset = morphed();
    assert_eq!(
        write_glb_with_profile(&asset, &bevy(2)),
        Err(GltfError::ConsumerCapExceeded {
            profile: "bevy".into(),
            requirement: "0.20".into(),
            resolved: "0.20.1".into(),
            cap: 2,
            count: 3,
        }),
        "the typed refusal names name, requirement, resolved, cap and count"
    );
}

#[test]
fn a_count_within_the_cap_writes_and_parses_back_with_target_names() {
    let asset = morphed();
    let bytes = write_glb_with_profile(&asset, &bevy(3)).expect("three morphs within cap write");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");

    let mesh = parsed.document.meshes().next().expect("one mesh");
    let extras: serde_json::Value =
        serde_json::from_str(mesh.extras().as_ref().expect("mesh extras").get())
            .expect("extras JSON");
    let names: Vec<&str> = extras["targetNames"]
        .as_array()
        .expect("targetNames array")
        .iter()
        .map(|name| name.as_str().expect("string target name"))
        .collect();
    assert_eq!(
        names,
        ["Bend", "Fold", "Sag"],
        "three target names parse back"
    );

    let primitive = mesh.primitives().next().expect("one primitive");
    assert_eq!(
        primitive.morph_targets().count(),
        3,
        "every morph target survived the profiled write"
    );
}

#[test]
fn write_glb_without_a_profile_writes_the_same_morphs() {
    let asset = morphed();
    let bytes = write_glb(&asset).expect("no profile writes every morph");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");

    let mesh = parsed.document.meshes().next().expect("one mesh");
    assert_eq!(
        mesh.weights().expect("one weight per morph").len(),
        3,
        "the no-cap spelling writes all three morphs"
    );
    let primitive = mesh.primitives().next().expect("one primitive");
    assert_eq!(primitive.morph_targets().count(), 3);
}

#[test]
fn measured_carries_the_command_and_date_that_measured_it() {
    let cap = bevy(256).max_morph_targets;
    assert_eq!(cap.value, 256);
    assert_eq!(
        cap.command, "rg MAX_MORPH_WEIGHTS bevy_mesh-0.20.1/src/morph.rs",
        "the cap travels with the command that measured it"
    );
    assert_eq!(
        cap.date, "2026-11-10",
        "the cap travels with the date of its measurement"
    );
}

#[test]
fn the_documented_bevy_example_is_the_a00b_measurement() {
    let bevy = bevy(256);
    assert_eq!(bevy.name, "bevy");
    assert_eq!(bevy.requirement, "0.20");
    assert_eq!(bevy.resolved, "0.20.1");
    assert_eq!(bevy.max_morph_targets.value, 256, "the A00B measurement");
}

#[test]
fn a_texture_without_uvs_is_refused_naming_the_slot() {
    let mut textured = triangle();
    textured.materials = vec![MaterialSlot {
        name: "textured".into(),
        diffuse: Some("textures/cloth_d.png".into()),
        normal: Some("textures/cloth_n.png".into()),
        ..MaterialSlot::default()
    }];
    assert_eq!(
        write_glb(&textured),
        Err(GltfError::TextureWithoutUv { slot: "diffuse" }),
        "the diffuse texture is refused first and names its slot"
    );

    let mut normal_only = triangle();
    normal_only.materials = vec![MaterialSlot {
        name: "normal_only".into(),
        normal: Some("textures/cloth_n.png".into()),
        ..MaterialSlot::default()
    }];
    assert_eq!(
        write_glb(&normal_only),
        Err(GltfError::TextureWithoutUv { slot: "normal" }),
        "a normal texture without UVs is refused and names its slot"
    );
}

#[test]
fn an_untextured_model_without_uvs_writes() {
    let asset = triangle();
    let bytes = write_glb(&asset).expect("untextured model without UVs writes");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");
    assert_eq!(parsed.document.materials().count(), 0);
}

#[test]
fn a_textured_model_with_uvs_writes() {
    let mut asset = triangle();
    asset.uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
    asset.materials = vec![MaterialSlot {
        name: "textured".into(),
        diffuse: Some("textures/cloth_d.png".into()),
        ..MaterialSlot::default()
    }];
    let bytes = write_glb(&asset).expect("textured model with UVs writes");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");
    assert_eq!(parsed.document.materials().count(), 1);
}
