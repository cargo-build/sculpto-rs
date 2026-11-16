//! `actor rebind` end-to-end on a synthetic actor: the mesh keeps its geometry
//! and colours, the skeleton becomes the target's, and a soft-tissue bone folds
//! to its nearest mapped ancestor with its mass named. @req DECL0008 @req DECL0013

use std::path::PathBuf;

use glam::{DMat4, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::rebind::{RebindParams, run};
use sculpto_gltf::{PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose};

const IDENTITY_F32: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn joint(name: &str, parent: Option<u32>, translation: [f32; 3]) -> Joint {
    Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    }
}

fn f32_of(value: DMat4) -> [[f32; 4]; 4] {
    value
        .to_cols_array_2d()
        .map(|column| column.map(|x| x as f32))
}

/// A rigged GLB in world Y-up with an identity basis, one shared pool, one part.
fn actor_glb(
    name: &str,
    rig: &Rig,
    palette: &[&str],
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    rows: &[(&[u16], &[f32])],
    color: [f32; 4],
) -> Vec<u8> {
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for (slots, weights) in rows {
        joints.extend_from_slice(slots);
        values.extend_from_slice(weights);
        offsets.push(joints.len() as u32);
    }
    let skin = Skin {
        bones: palette.iter().map(|bone| (*bone).into()).collect(),
        bind_inverse: palette
            .iter()
            .map(|bone| {
                let joint = rig.joint_by_name(bone).expect("palette bone");
                f32_of(
                    DMat4::from_cols_array_2d(&rig.rest_world(joint).expect("rest world"))
                        .inverse(),
                )
            })
            .collect(),
        influences: SkinWeights::new(offsets, joints, values, palette.len()).expect("rows"),
    };
    let binding = RestBinding::prove_with_basis(
        rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: rig.roots()[0],
            positions,
            skin: &skin,
            global_to_skin: IDENTITY_F32,
        }],
        DMat4::IDENTITY.to_cols_array_2d(),
    )
    .expect("a checked rest");
    let asset = MeshAsset {
        name: name.into(),
        positions: positions.to_vec(),
        triangles: triangles.to_vec(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let materials = [PartMaterial {
        partition: 0,
        name: "body".into(),
        color,
    }];
    write_partitioned_rigged_glb(&asset, &binding, 0, &[], &materials).expect("a written actor")
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sculpto-rebind-{}-{label}.glb", std::process::id()))
}

#[test]
fn a_soft_bone_folds_onto_the_seated_target_and_geometry_survives() {
    // Source: pelvis -> spine -> breast (soft). Target: hips -> spine.
    let source = Rig::new(
        vec![
            joint("NPC Pelvis [Pelv]", None, [0.0, 0.9, 0.0]),
            joint("NPC Spine [Spn0]", Some(0), [0.0, 0.2, 0.0]),
            joint("L Breast01", Some(1), [0.05, 0.1, 0.1]),
        ],
        RigFrame::YUp,
    )
    .expect("source rig");
    let target = Rig::new(
        vec![
            joint("mixamorig:Hips", None, [0.0, 1.0, 0.0]),
            joint("mixamorig:Spine", Some(0), [0.0, 0.15, 0.0]),
        ],
        RigFrame::YUp,
    )
    .expect("target rig");

    let positions = [
        [0.0_f32, 0.9, 0.0],
        [0.0, 1.15, 0.05],
        [0.05, 1.2, 0.1],
        [0.1, 1.0, 0.0],
    ];
    let triangles = [[0_u32, 1, 2], [0, 2, 3]];
    let source_rows: &[(&[u16], &[f32])] = &[
        (&[0_u16], &[1.0_f32]),
        (&[1, 2], &[0.5, 0.5]),
        (&[2], &[1.0]),
        (&[0, 1], &[0.7, 0.3]),
    ];
    let source_palette = ["NPC Pelvis [Pelv]", "NPC Spine [Spn0]", "L Breast01"];
    let color = [0.9, 0.8, 0.7, 1.0];
    let source_bytes = actor_glb(
        "TestBody",
        &source,
        &source_palette,
        &positions,
        &triangles,
        source_rows,
        color,
    );
    // The skeleton carries its own trivial mesh; only its palette is used.
    let skeleton_bytes = actor_glb(
        "TargetBody",
        &target,
        &["mixamorig:Hips", "mixamorig:Spine"],
        &positions,
        &triangles,
        &[
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
        ],
        [0.5, 0.5, 0.5, 1.0],
    );

    let actor_path = temp_path("actor");
    let skeleton_path = temp_path("skeleton");
    let out_path = temp_path("out");
    std::fs::write(&actor_path, &source_bytes).expect("write actor");
    std::fs::write(&skeleton_path, &skeleton_bytes).expect("write skeleton");

    let report = run(&RebindParams {
        actor: actor_path.clone(),
        skeleton: skeleton_path.clone(),
        out: Some(out_path.clone()),
        report: None,
    })
    .expect("rebind succeeds");

    assert_eq!(report["palette"]["source_bones"], 3);
    assert_eq!(report["palette"]["target_bones"], 2);
    assert_eq!(report["palette"]["mapped_source_bones"], 2);
    // Breast mass 1.5 of total 4.0 folds to the spine.
    let folded = report["weight_census"]["folded_share"].as_f64().unwrap();
    assert!((folded - 0.375).abs() < 1e-6, "folded share {folded}");
    let groups = report["weight_census"]["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["group"], "breast");
    assert_eq!(groups[0]["targets"][0], "mixamorig:Spine");
    assert_eq!(report["validation"]["empty_rows"], 0);
    assert!(report["validation"]["max_sum_error"].as_f64().unwrap() < 1e-6);
    assert!(report["validation"]["rest"]["max_vertex"].as_f64().unwrap() < 1e-5);
    // Per-bone output mass: hips 1.7, spine 2.3.
    let per_bone = report["per_bone_mass"].as_array().unwrap();
    assert!((per_bone[0]["mass"].as_f64().unwrap() - 1.7).abs() < 1e-5);
    assert!((per_bone[1]["mass"].as_f64().unwrap() - 2.3).abs() < 1e-5);

    // The written actor carries the target palette and the same world geometry.
    let out_bytes = std::fs::read(&out_path).expect("read output");
    let out = Actor::from_glb(&out_bytes).expect("output actor");
    let joints = out.skin_joints().expect("output palette");
    let names: Vec<&str> = joints
        .iter()
        .map(|&node| out.node_name(node).unwrap())
        .collect();
    assert_eq!(names, ["mixamorig:Hips", "mixamorig:Spine"]);
    let sample = out
        .sample(&["TestBody"], Pose::Rest)
        .expect("output sample");
    for (vertex, expected) in sample.vertices.iter().zip(positions.iter()) {
        let distance =
            DVec3::from_array(*vertex).distance(DVec3::from_array(expected.map(f64::from)));
        assert!(distance < 1e-5, "vertex moved {distance}");
    }
    // The part colour survives.
    let material = out.document().materials().next().expect("a part material");
    assert_eq!(material.name(), Some("body"));
    assert_eq!(material.pbr_metallic_roughness().base_color_factor(), color);

    for path in [&actor_path, &skeleton_path, &out_path] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_scaled_skeleton_seats_per_chain_without_throwing_tips_out_of_range() {
    // The target skeleton carries a Blender-style 0.01 armature scale and a leaf
    // marker (HeadTop_End) the source cannot name. The per-chain seat proportions
    // the spine to the flesh and must not amplify the tip by the inverse scale.
    let source = Rig::new(
        vec![
            joint("NPC Pelvis [Pelv]", None, [0.0, 0.9, 0.0]),
            joint("NPC Spine [Spn0]", Some(0), [0.0, 0.2, 0.0]),
        ],
        RigFrame::YUp,
    )
    .expect("source rig");
    let scaled = |name: &str, parent: Option<u32>, translation: [f32; 3], scale: f32| Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            scale,
            ..Default::default()
        },
    };
    let target = Rig::new(
        vec![
            scaled("mixamorig:Hips", None, [0.0, 1.0, 0.0], 0.01),
            scaled("mixamorig:Spine", Some(0), [0.0, 15.0, 0.0], 1.0),
            scaled("mixamorig:HeadTop_End", Some(1), [0.0, 30.0, 0.0], 1.0),
        ],
        RigFrame::YUp,
    )
    .expect("target rig");
    let positions = [[0.0_f32, 0.9, 0.0], [0.0, 1.1, 0.0], [0.1, 1.0, 0.0]];
    let triangles = [[0_u32, 1, 2]];
    let source_bytes = actor_glb(
        "Body",
        &source,
        &["NPC Pelvis [Pelv]", "NPC Spine [Spn0]"],
        &positions,
        &triangles,
        &[(&[0_u16], &[1.0_f32]), (&[1], &[1.0]), (&[0], &[1.0])],
        [1.0, 1.0, 1.0, 1.0],
    );
    let skeleton_bytes = actor_glb(
        "Target",
        &target,
        &["mixamorig:Hips", "mixamorig:Spine", "mixamorig:HeadTop_End"],
        &positions,
        &triangles,
        &[(&[0_u16], &[1.0_f32]), (&[0], &[1.0]), (&[0], &[1.0])],
        [1.0, 1.0, 1.0, 1.0],
    );
    let actor_path = temp_path("scaled-actor");
    let skeleton_path = temp_path("scaled-skeleton");
    let out_path = temp_path("scaled-out");
    std::fs::write(&actor_path, &source_bytes).expect("write actor");
    std::fs::write(&skeleton_path, &skeleton_bytes).expect("write skeleton");
    let report = run(&RebindParams {
        actor: actor_path.clone(),
        skeleton: skeleton_path.clone(),
        out: Some(out_path.clone()),
        report: None,
    })
    .expect("rebind succeeds");

    // The seat is per-chain (Mixamo directions, flesh-proportioned lengths), and a
    // 0.01-scaled skeleton must not throw the leaf metres away: the tip lands at a
    // flesh-proportioned height, not amplified by the inverse armature scale.
    assert!(
        report["seat"]["method"]
            .as_str()
            .unwrap()
            .contains("per-chain")
    );
    let spine_scale = report["seat"]["chain_scale"]["spine"].as_f64().unwrap();
    assert!(
        spine_scale.is_finite() && spine_scale > 0.0,
        "spine scale {spine_scale}"
    );
    let out_bytes = std::fs::read(&out_path).expect("read output");
    let out = Actor::from_glb(&out_bytes).expect("output actor");
    let (out_rig, _) = sculpto_engine::adapt::actor_rig(&out).expect("output rig");
    let tip = out_rig.joint_by_name("mixamorig:HeadTop_End").unwrap();
    let tip_y = DMat4::from_cols_array_2d(&out_rig.rest_world(tip).unwrap())
        .w_axis
        .y;
    assert!(
        tip_y > 1.0 && tip_y < 2.0,
        "tip thrown out of range: y={tip_y}"
    );
    for path in [&actor_path, &skeleton_path, &out_path] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_weighted_bone_with_no_mapped_ancestor_refuses() {
    // Source root is a soft bone with no anatomical name; the target maps nothing
    // above it, so its weight cannot fold.
    let source = Rig::new(
        vec![joint("MysteryRoot", None, [0.0, 0.9, 0.0])],
        RigFrame::YUp,
    )
    .expect("source rig");
    let target = Rig::new(
        vec![joint("mixamorig:Hips", None, [0.0, 1.0, 0.0])],
        RigFrame::YUp,
    )
    .expect("target rig");
    let positions = [[0.0_f32, 0.9, 0.0], [0.1, 0.9, 0.0], [0.0, 0.9, 0.1]];
    let triangles = [[0_u32, 1, 2]];
    let source_bytes = actor_glb(
        "Mystery",
        &source,
        &["MysteryRoot"],
        &positions,
        &triangles,
        &[
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let skeleton_bytes = actor_glb(
        "Target",
        &target,
        &["mixamorig:Hips"],
        &positions,
        &triangles,
        &[
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
            (&[0_u16], &[1.0_f32]),
        ],
        [1.0, 1.0, 1.0, 1.0],
    );
    let actor_path = temp_path("mystery-actor");
    let skeleton_path = temp_path("mystery-skeleton");
    std::fs::write(&actor_path, &source_bytes).expect("write actor");
    std::fs::write(&skeleton_path, &skeleton_bytes).expect("write skeleton");
    let error = run(&RebindParams {
        actor: actor_path.clone(),
        skeleton: skeleton_path.clone(),
        out: None,
        report: None,
    })
    .expect_err("a weighted bone with no mapped ancestor must refuse");
    let message = error.to_string();
    assert!(
        message.contains("no mapped ancestor") || message.contains("resolves to a palette"),
        "unexpected refusal: {message}"
    );
    for path in [&actor_path, &skeleton_path] {
        let _ = std::fs::remove_file(path);
    }
}
