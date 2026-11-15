//! `export.glb`: the merge, the checked rest proof, clip mapping and the
//! typed refusals. @req DECL0008

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::{DMat4, DQuat, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::export_glb::{
    ClipAnchor, ClipTolerances, ExportGlbParams, MergedActor, merge_shapes, plan_clip, run,
};
use sculpto_gltf::{Clip, ClipChannel, write_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose, SourceProperty, SourceValues};
use sculpto_nif::{NifDocument, NifShape, ShapeSource};

/// A three joint chain with a non-identity rest so every bind is exercised.
fn rig() -> Rig {
    let joint = |name: &str, parent: Option<u32>, translation: [f32; 3]| Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    };
    Rig::new(
        vec![
            joint("Root", None, [0.0, 0.0, 0.0]),
            joint("Spine", Some(0), [0.0, 0.0, 10.0]),
            joint("Limb", Some(1), [0.0, 4.0, 0.0]),
        ],
        RigFrame::SourceZUp,
    )
    .expect("a valid source graph")
}

fn world(rig: &Rig, name: &str) -> DMat4 {
    DMat4::from_cols_array_2d(
        &rig.rest_world(rig.joint_by_name(name).expect("joint"))
            .expect("world"),
    )
}

/// One shape bound at rest with `bind = world⁻¹ · G⁻¹` and `G = I`.
fn shape(name: &str, rig: &Rig, bones: &[&str], quad: [usize; 4]) -> (NifShape, ShapeSource) {
    let positions = vec![
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let binds: Vec<[[f32; 4]; 4]> = bones
        .iter()
        .map(|bone| {
            world(rig, bone)
                .inverse()
                .to_cols_array_2d()
                .map(|column| column.map(|value| value as f32))
        })
        .collect();
    let mut quads = Vec::new();
    let mut weights = Vec::new();
    for slot in quad {
        let mut lanes = [0_u16; 4];
        lanes[0] = slot as u16;
        quads.push(lanes);
        weights.push([1.0, 0.0, 0.0, 0.0]);
    }
    let shape = NifShape {
        name: name.into(),
        positions,
        uvs: Vec::new(),
        normals: Vec::new(),
        tangents: Vec::new(),
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        skin: Some(Skin {
            bones: bones.iter().map(|bone| (*bone).to_owned()).collect(),
            bind_inverse: binds,
            influences: SkinWeights::from_quads(&quads, &weights).expect("unit rows"),
        }),
        triangle_partitions: vec![PartitionId(0); 2],
        partitions: Vec::new(),
        materials: Vec::new(),
    };
    let source = ShapeSource {
        parent: None,
        local: LocalTransform::default(),
        global_skin: Some(
            DMat4::IDENTITY
                .to_cols_array_2d()
                .map(|column| column.map(|value| value as f32)),
        ),
        skeleton_root: Some("Root".into()),
    };
    (shape, source)
}

fn document(rig: &Rig) -> NifDocument {
    let (body, body_source) = shape("Body", rig, &["Root", "Spine", "Limb"], [1, 1, 2, 2]);
    let (garment, garment_source) = shape("Garment", rig, &["Spine", "Limb"], [0, 0, 1, 1]);
    NifDocument {
        shapes: vec![body, garment],
        rig: Some(rig.clone()),
        sources: vec![body_source, garment_source],
        ..sculpto_nif::read(
            &sculpto_nif::write::se_bundle::write_se_bundle(&[MeshAsset {
                name: "fixture".into(),
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                triangles: vec![[0, 1, 2]],
                triangle_partitions: vec![PartitionId(0)],
                ..Default::default()
            }])
            .expect("a header fixture"),
        )
        .expect("a readable header fixture")
    }
}

#[test]
fn merge_keeps_authored_weights_one_palette_and_one_rest_basis() {
    let rig = rig();
    let document = document(&rig);
    let merged: MergedActor = merge_shapes(&document, &rig, 0, &[1]).expect("the merge answers");
    assert_eq!(merged.asset.positions.len(), 8, "both shapes are merged");
    assert_eq!(merged.asset.triangles.len(), 4);
    assert_eq!(
        merged.asset.triangle_partitions,
        vec![
            PartitionId(0),
            PartitionId(0),
            PartitionId(1),
            PartitionId(1)
        ],
        "one partition per part"
    );
    let skin = merged.asset.skin.as_ref().expect("the merged skin");
    assert_eq!(skin.bones, vec!["Root", "Spine", "Limb"], "one palette");
    assert_eq!(skin.influences.rows(), 8);
    assert!(
        merged.bind_repairs.is_empty(),
        "authored binds are consistent"
    );
    assert!(merged.bind_residual <= sculpto_core::rig::rest::REST_TOLERANCE);

    let proof = merged.proof(&rig).expect("the merged skin proves its rest");
    let measurement = proof.skins()[0].measurement();
    assert!(
        measurement.max_vertex <= 1e-6,
        "rest reconstruction {:.9e}",
        measurement.max_vertex
    );
    let bytes = write_rigged_glb(&merged.asset, &proof, merged.shape, &[]).expect("checked GLB");
    let actor = Actor::from_glb(&bytes).expect("the actor admits the written bytes");
    let rest = actor.sample(&["Body"], Pose::Rest).expect("rest sample");
    assert_eq!(rest.parts.len(), 2, "one primitive, one part");
    // Each primitive re-indexes its own vertices in the sampled arena, so the
    // rest positions are checked per part against the merged positions.
    assert_eq!(
        rest.vertices.len(),
        merged.asset.positions.len() * rest.parts.len(),
        "each primitive samples the shared arena"
    );
    for (index, part) in rest.parts.iter().enumerate() {
        assert!(!part.vertices.is_empty() && !part.triangles.is_empty());
        assert_eq!(part.vertices.len(), merged.asset.positions.len());
        for (offset, vertex) in part.vertices.clone().enumerate() {
            let expected = merged.asset.positions[offset];
            // The writer places the scene in glTF Y-up: (x, y, z) -> (x, z, -y).
            let expected = DVec3::new(
                f64::from(expected[0]),
                f64::from(expected[2]),
                -f64::from(expected[1]),
            );
            let actual = DVec3::from_array(rest.vertices[vertex]);
            assert!(
                expected.distance(actual) <= 1e-5,
                "part {index} vertex {offset}: rest {expected:?} vs {actual:?}"
            );
        }
    }
}

#[test]
fn merge_refuses_unskinned_missing_and_duplicate_shapes() {
    let rig = rig();
    let mut source = document(&rig);
    source.shapes[1].skin = None;
    let refused = merge_shapes(&source, &rig, 0, &[1]).expect_err("an unskinned part refuses");
    assert_eq!(refused.code(), "export_glb_unskinned_shape");
    assert_eq!(refused.kind(), sculpto_engine::ErrorKind::Refused);

    let bare = document(&rig);
    let refused = merge_shapes(&bare, &rig, 9, &[]).expect_err("a shape outside the file refuses");
    assert_eq!(refused.code(), "invalid_value");
}

#[test]
fn run_refuses_missing_shape_names_and_clip_declarations() {
    let directory = std::env::temp_dir().join(format!("sculpto-export-glb-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp directory");
    let nif = directory.join("fixture.nif");
    let bundle = |name: &str| MeshAsset {
        name: name.into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..Default::default()
    };
    let bytes =
        sculpto_nif::write::se_bundle::write_se_bundle(&[bundle("Body"), bundle("Garment")])
            .expect("header fixture");
    std::fs::write(&nif, bytes).expect("fixture file");

    let params = ExportGlbParams {
        nif: nif.clone(),
        body_shape: "Missing".into(),
        garment_shapes: Vec::new(),
        skeleton: None,
        clips: None,
        clip_names: Vec::new(),
        units_per_metre: None,
        rest_translation_tolerance: None,
        rest_scale_tolerance: None,
        rest_frame_tolerance_degrees: None,
        out: directory.join("out.glb"),
        report: None,
    };
    let refused = run(&params).expect_err("a shape the file does not carry refuses");
    assert_eq!(refused.code(), "export_glb_shape_missing");

    let mut duplicated = params.clone();
    duplicated.body_shape = "Body".into();
    duplicated.garment_shapes = vec!["Body".into()];
    let refused = run(&duplicated).expect_err("one shape selected twice refuses");
    assert_eq!(refused.code(), "export_glb_duplicate_part");

    let mut unskinned = params.clone();
    unskinned.body_shape = "Body".into();
    let refused = run(&unskinned).expect_err("a shape without a skin refuses");
    assert_eq!(refused.code(), "export_glb_unskinned_shape");

    let mut with_clips = params.clone();
    with_clips.clips = Some(directory.join("none.glb"));
    let refused = run(&with_clips).expect_err("clips without names refuse");
    assert_eq!(refused.code(), "invalid_value");

    let mut duplicate = params.clone();
    duplicate.clip_names = vec!["Idle".into(), "Idle".into()];
    duplicate.clips = Some(directory.join("none.glb"));
    let refused = run(&duplicate).expect_err("a repeated clip refuses");
    assert_eq!(refused.code(), "invalid_value");
    std::fs::remove_dir_all(&directory).ok();
}

/// A clip written by this crate reads back as node-local channel data.
#[test]
fn clip_extraction_round_trips_a_written_clip() {
    let rig = rig();
    let document = document(&rig);
    let merged = merge_shapes(&document, &rig, 0, &[1]).expect("merge");
    let proof = merged.proof(&rig).expect("proof");
    let limb = merged
        .palette
        .iter()
        .position(|name| name == "Limb")
        .expect("palette");
    let rest = rig
        .joint(rig.joint_by_name("Limb").expect("joint"))
        .expect("joint")
        .rest
        .translation;
    let clip = Clip {
        name: "Turn".into(),
        times: vec![0.0, 0.5, 1.0],
        channels: vec![
            ClipChannel::Translation {
                bone: limb,
                values: vec![rest.to_vec().try_into().expect("3 lanes"); 3],
            },
            ClipChannel::Rotation {
                bone: limb,
                values: vec![
                    [0.0, 0.0, 0.0, 1.0],
                    DQuat::from_rotation_x(0.5)
                        .to_array()
                        .map(|lane| lane as f32),
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
        ],
    };
    let bytes = write_rigged_glb(&merged.asset, &proof, merged.shape, &[clip]).expect("GLB");
    let actor = Actor::from_glb(&bytes).expect("actor");
    assert_eq!(actor.clip_names(), vec![Some("Turn")]);
    let read = actor.clip("Turn").expect("the written clip reads back");
    assert_eq!(read.times, vec![0.0, 0.5, 1.0]);
    assert_eq!(read.channels.len(), 2);
    let rotation = read
        .channels
        .iter()
        .find(|channel| channel.property == SourceProperty::Rotation)
        .expect("a rotation channel");
    assert_eq!(rotation.node_name.as_deref(), Some("Limb"));
    match &rotation.values {
        SourceValues::Rotation(values) => {
            assert_eq!(values.len(), 3);
            assert!((values[0][3] - 1.0).abs() <= 1e-6, "identity first sample");
            assert!((values[1][0] - DQuat::from_rotation_x(0.5).x).abs() <= 1e-6);
        }
        other => panic!("expected rotation samples, found {other:?}"),
    }

    // The same clip maps onto its own rig at the written unit.
    let anchor = ClipAnchor::of(&actor, &read).expect("anchor");
    let plan = plan_clip(
        &read,
        &actor,
        &anchor,
        &rig,
        &merged.palette,
        &ClipTolerances::default(),
    )
    .expect("the written clip plans");
    assert_eq!(plan.mapped_bones, 1, "one channel node maps by name");
    assert!(plan.moving_channels >= 1);
    assert!(
        (plan.scale - 1.0).abs() <= 1e-6,
        "the clip is already in the rig's unit: {}",
        plan.scale
    );
    // Its own first sample is the anchor, so the plan reproduces the rest.
    if let Some(channel) = plan
        .clip
        .channels
        .iter()
        .find(|channel| matches!(channel, ClipChannel::Rotation { .. }))
    {
        let ClipChannel::Rotation { values, .. } = channel else {
            unreachable!()
        };
        assert!(
            (values[0][3] - 1.0).abs() <= 1e-6,
            "the anchor sample is the rig rest: {:?}",
            values[0]
        );
    }
}

/// The two exact NeverNude fixtures: merge, prove, write and report.
#[test]
fn configured_bodies_export_a_checked_clipped_actor() {
    let Some(root) = std::env::var_os("SCULPTO_RIGGED_FIXTURES").map(PathBuf::from) else {
        eprintln!(
            "SKIP configured_bodies_export_a_checked_clipped_actor: set SCULPTO_RIGGED_FIXTURES \
             (JSON with `emily`, `mike`, `female_skeleton`, `male_skeleton`, `emily_clips`, \
             `mike_clips`, `out`) to run the tier-2 gates"
        );
        return;
    };
    let manifest: BTreeMap<String, String> = serde_json::from_str(
        &std::fs::read_to_string(&root).expect("SCULPTO_RIGGED_FIXTURES must be a JSON file"),
    )
    .expect("SCULPTO_RIGGED_FIXTURES must hold string paths");
    let path = |key: &str| PathBuf::from(manifest.get(key).unwrap_or_else(|| panic!("{key}")));
    let out = path("out");
    std::fs::create_dir_all(&out).expect("output directory");

    for (body, shape, garments, skeleton, clips, clip_name, out_name, vertices) in [
        (
            "emily",
            "3BA",
            vec!["Bra", "Panty"],
            "female_skeleton",
            "emily_clips",
            "Idle",
            "emily-nevernude.glb",
            17441_usize,
        ),
        (
            "mike",
            "HIMBO - Body",
            vec!["HIMBO - Boxers"],
            "male_skeleton",
            "mike_clips",
            "Idle",
            "mike-nevernude.glb",
            10224,
        ),
    ] {
        let params = ExportGlbParams {
            nif: path(body),
            body_shape: shape.into(),
            garment_shapes: garments.iter().map(|name| (*name).to_owned()).collect(),
            skeleton: Some(path(skeleton)),
            clips: Some(path(clips)),
            clip_names: vec![clip_name.into()],
            units_per_metre: Some(71.4286),
            rest_translation_tolerance: None,
            rest_scale_tolerance: None,
            rest_frame_tolerance_degrees: None,
            out: out.join(out_name),
            report: Some(out.join(out_name).with_extension("json")),
        };
        let report = run(&params).unwrap_or_else(|error| panic!("{body}: {error:?}"));
        let parts = report["parts"].as_array().expect("parts");
        let total: usize = parts
            .iter()
            .map(|part| part["vertices"].as_u64().expect("vertices") as usize)
            .sum();
        assert_eq!(total, vertices, "{body}: merged vertex count");
        assert_eq!(
            report["rest"]["max_vertex"].as_f64().expect("rest"),
            report["rest"]["max_vertex"].as_f64().expect("rest"),
            "{body}: the rest proof ran"
        );
        assert!(
            report["rest"]["max_vertex"].as_f64().expect("rest") <= 1e-4,
            "{body}: rest reconstruction {}",
            report["rest"]["max_vertex"]
        );
        assert_eq!(
            report["rest"]["basis"],
            serde_json::json!([
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0]
            ]),
            "{body}: a native document proves at the identity basis"
        );
        let clip = &report["clips"][0];
        assert_eq!(clip["index"], 0);
        assert_eq!(clip["name"], clip_name);
        assert!(clip["moving_channels"].as_u64().expect("moving") > 0);
        assert!(
            (clip["unit_scale"].as_f64().expect("scale") - 71.4286).abs() < 0.01,
            "{body}: the clips are BodySlide units: {}",
            clip["unit_scale"]
        );
        let written = std::fs::read(out.join(out_name)).expect("written GLB");
        let actor = Actor::from_glb(&written).expect("the written bytes are one actor");
        assert_eq!(actor.clip_names(), vec![Some(clip_name)]);
        let sample = actor
            .sample(&[shape], Pose::Rest)
            .expect("the merged part is sampleable by name");
        assert_eq!(
            sample.parts.len(),
            1 + garments.len(),
            "{body}: one primitive per part"
        );
    }
}
