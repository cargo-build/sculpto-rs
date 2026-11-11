//! Checked neutral rigs reach the existing Actor evaluator. @req DECL0008

use glam::{DMat4, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{Clip, ClipChannel, GltfError, write_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose};

const SHAPE: usize = 7;

fn fixture(frame: RigFrame) -> (Rig, MeshAsset, [[f32; 4]; 4]) {
    let joint = |name: &str, parent: Option<u32>, translation: [f32; 3]| Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    };
    let rig = Rig::new(
        vec![
            joint("limb", Some(1), [1.0, 0.0, 0.0]),
            joint("connector", Some(2), [0.0, 1.0, 0.0]),
            joint("root", None, [2.0, 3.0, 4.0]),
            joint("anchor", Some(2), [5.0, 0.0, 0.0]),
        ],
        frame,
    )
    .expect("non-palette ancestry and out-of-order graph");
    let basis = DMat4::from_translation(DVec3::new(0.7, -1.3, 0.2)) * DMat4::from_rotation_z(0.25);
    let placement =
        DMat4::from_translation(DVec3::new(4.0, -2.0, 3.0)) * DMat4::from_rotation_x(0.3);
    let inverse = [0, 3].map(|index| {
        let world =
            DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(index)).expect("joint world"));
        ((basis * world).inverse() * placement)
            .to_cols_array_2d()
            .map(|column| column.map(|x| x as f32))
    });
    let asset = MeshAsset {
        name: "Body".into(),
        positions: vec![
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        triangles: vec![[0, 1, 3], [1, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        skin: Some(Skin {
            bones: vec!["limb".into(), "anchor".into()],
            bind_inverse: inverse.to_vec(),
            influences: SkinWeights::new(vec![0, 1, 2, 3, 4], vec![0, 0, 1, 1], vec![1.0; 4], 2)
                .expect("source influences"),
        }),
        ..Default::default()
    };
    (
        rig,
        asset,
        placement
            .inverse()
            .to_cols_array_2d()
            .map(|column| column.map(|x| x as f32)),
    )
}

fn source<'a>(
    rig: &Rig,
    asset: &'a MeshAsset,
    global_to_skin: [[f32; 4]; 4],
) -> SkinRestSource<'a> {
    SkinRestSource {
        shape: SHAPE,
        root: rig.roots()[0],
        positions: &asset.positions,
        skin: asset.skin.as_ref().expect("source skin"),
        global_to_skin,
    }
}

fn frame_matrix(frame: RigFrame) -> DMat4 {
    match frame {
        RigFrame::SourceZUp => DMat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]),
        RigFrame::YUp => DMat4::IDENTITY,
    }
}

#[test]
fn checked_hierarchy_rest_and_palette_clip_reach_actor() {
    for frame in [RigFrame::SourceZUp, RigFrame::YUp] {
        let (rig, asset, g) = fixture(frame);
        let original = asset.clone();
        let binding = RestBinding::prove(&rig, frame, &[source(&rig, &asset, g)])
            .expect("checked nonidentity rest basis");
        assert_ne!(binding.basis(), DMat4::IDENTITY.to_cols_array_2d());
        let clip = Clip {
            name: "move limb".into(),
            times: vec![0.0, 1.0],
            channels: vec![ClipChannel::Translation {
                bone: 0,
                values: vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            }],
        };
        let bytes = write_rigged_glb(&asset, &binding, SHAPE, &[clip]).expect("hierarchical GLB");
        let parsed = gltf::Gltf::from_slice(&bytes).expect("glTF references");
        let mesh_placement = parsed
            .nodes()
            .find(|node| node.mesh().is_some())
            .expect("mesh placement")
            .transform()
            .matrix();
        let expected_placement = DMat4::from_cols_array_2d(&g.map(|column| column.map(f64::from)))
            .inverse()
            .to_cols_array_2d();
        assert!(
            mesh_placement
                .iter()
                .flatten()
                .zip(expected_placement.iter().flatten())
                .all(|(&a, &b)| (f64::from(a) - b).abs() < 1e-6)
        );
        assert_eq!(
            parsed.nodes().len(),
            7,
            "mesh, full graph, document basis and scene basis"
        );
        let palette: Vec<_> = parsed
            .skins()
            .next()
            .expect("skin")
            .joints()
            .map(|joint| joint.name().expect("joint name"))
            .collect();
        assert_eq!(palette, ["limb", "anchor"]);
        let connector = parsed
            .nodes()
            .find(|node| node.name() == Some("connector"))
            .expect("non-palette parent");
        assert_eq!(
            connector.children().next().expect("limb child").name(),
            Some("limb")
        );
        let actor = Actor::from_glb(&bytes).expect("existing Actor admission");
        let rest = actor.sample(&["Body"], Pose::Rest).expect("rest sample");
        let posed = actor
            .sample(
                &["Body"],
                Pose::Clip {
                    name: "move limb",
                    time: 1.0,
                },
            )
            .expect("existing pose evaluation");
        let c = frame_matrix(frame);
        let placement = DMat4::from_cols_array_2d(&g.map(|column| column.map(f64::from))).inverse();
        let delta = (c * DMat4::from_cols_array_2d(&binding.basis())).transform_vector3(DVec3::X);
        assert!(delta.length() > 0.1, "the clip must be nontrivial");
        for (index, ((point, rest), posed)) in asset
            .positions
            .iter()
            .zip(&rest.vertices)
            .zip(&posed.vertices)
            .enumerate()
        {
            let expected =
                (c * placement).transform_point3(DVec3::from_array(point.map(f64::from)));
            assert!(
                expected.distance(DVec3::from_array(*rest)) < 1e-4,
                "frame {frame:?}, rest vertex{index}"
            );
            let expected_delta = if index < 2 { delta } else { DVec3::ZERO };
            let actual_delta = DVec3::from_array(*posed) - DVec3::from_array(*rest);
            assert!(
                actual_delta.distance(expected_delta) < 1e-5,
                "frame {frame:?}, posed vertex{index}: {actual_delta:?}"
            );
        }
        assert_eq!(
            asset, original,
            "export must preserve the source skin and geometry"
        );
    }
}

#[test]
fn mismatched_or_ambiguous_checked_sources_are_refused() {
    let (rig, asset, g) = fixture(RigFrame::SourceZUp);
    let source = source(&rig, &asset, g);
    let binding = RestBinding::prove(&rig, RigFrame::SourceZUp, &[source]).expect("rest proof");
    assert!(matches!(
        write_rigged_glb(&asset, &binding, SHAPE + 1, &[]),
        Err(GltfError::RigBinding {
            field: "shape identity",
            ..
        })
    ));
    let mut changed = asset.clone();
    changed.positions[0][0] += 0.1;
    assert!(matches!(
        write_rigged_glb(&changed, &binding, SHAPE, &[]),
        Err(GltfError::RigBinding {
            field: "positions",
            ..
        })
    ));
    changed = asset.clone();
    changed.skin.as_mut().expect("skin").bones.swap(0, 1);
    assert!(matches!(
        write_rigged_glb(&changed, &binding, SHAPE, &[]),
        Err(GltfError::RigBinding { field: "skin", .. })
    ));
    let ambiguous = RestBinding::prove(&rig, RigFrame::SourceZUp, &[source, source])
        .expect("individual rest proofs");
    assert!(matches!(
        write_rigged_glb(&asset, &ambiguous, SHAPE, &[]),
        Err(GltfError::RigBinding {
            field: "ambiguous shape identity",
            ..
        })
    ));
}

#[test]
fn configured_default_body_rigs_reconstruct_and_pose_through_glb() {
    let Some(root) = std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES") else {
        eprintln!(
            "SKIP configured_default_body_rigs_reconstruct_and_pose_through_glb: SCULPTO_RIG_TEMPLATE_FIXTURES is unset"
        );
        return;
    };
    let root = std::path::PathBuf::from(root);
    for (body_file, template_file) in [
        ("himbo.nif", "skeleton_male_sse.nif"),
        ("bijin.nif", "skeleton_female_sse.nif"),
    ] {
        let read = |name: &str| {
            sculpto_nif::read(&std::fs::read(root.join(name)).expect("configured NIF must exist"))
                .expect("configured NIF must decode")
        };
        let body = read(body_file);
        let template = read(template_file);
        let authored = body.rig.as_ref().expect("authored graph");
        let template = template.rig.as_ref().expect("family graph");
        let rig = sculpto_core::rig::template::complete_family_rig(
            authored,
            template,
            sculpto_core::rig::template::TemplateRoots {
                authored: authored.roots()[0],
                template: template.roots()[0],
            },
        )
        .expect("completed family graph");
        let binding = body.bind_rest_on(&rig).expect("completed rest proof");
        let (shape_index, shape) = body
            .shapes
            .iter()
            .enumerate()
            .max_by_key(|(_, shape)| shape.positions.len())
            .expect("body shape");
        let asset = MeshAsset::from(shape);
        let skin = asset.skin.as_ref().expect("body skin");
        let arm = rig
            .joint_by_name("NPC L UpperArm [LUar]")
            .expect("supported arm");
        let bone = skin
            .bones
            .iter()
            .position(|name| name == "NPC L UpperArm [LUar]")
            .expect("weighted arm palette");
        let arm_joint = rig.joint(arm).expect("arm joint");
        let rest_translation = arm_joint.rest.translation;
        let mut moved_translation = rest_translation;
        moved_translation[0] += 1.0;
        let clip = Clip {
            name: "arm translation probe".into(),
            times: vec![0.0, 1.0],
            channels: vec![ClipChannel::Translation {
                bone,
                values: vec![rest_translation, moved_translation],
            }],
        };
        let bytes = write_rigged_glb(&asset, &binding, shape_index, &[clip])
            .expect("checked full body GLB");
        let actor = Actor::from_glb(&bytes).expect("real Actor admission");
        let rest = actor.sample(&[&asset.name], Pose::Rest).expect("real rest");
        let posed = actor
            .sample(
                &[&asset.name],
                Pose::Clip {
                    name: "arm translation probe",
                    time: 1.0,
                },
            )
            .expect("real posed sample");
        assert_eq!(rest.vertices.len(), asset.positions.len());
        assert_eq!(posed.vertices.len(), asset.positions.len());
        let c = frame_matrix(RigFrame::SourceZUp);
        let placement = body.shape_placement(shape_index).expect("source placement");
        let parent = DMat4::from_cols_array_2d(
            &rig.rest_world(arm_joint.parent.expect("arm parent"))
                .expect("parent world"),
        );
        let expected_motion =
            (c * DMat4::from_cols_array_2d(&binding.basis()) * parent).transform_vector3(DVec3::X);
        let descendants: Vec<_> = skin
            .bones
            .iter()
            .map(|name| {
                let joint = rig.joint_by_name(name).expect("palette joint");
                joint == arm || rig.is_ancestor(arm, joint).expect("same rig")
            })
            .collect();
        let mut max_rest = 0.0_f64;
        let mut max_error = 0.0_f64;
        let mut max_motion = 0.0_f64;
        let mut non_descendant_residual = 0.0_f64;
        let mut detached = 0;
        let mut moving = 0;
        for (vertex, ((&point, rest), posed)) in asset
            .positions
            .iter()
            .zip(&rest.vertices)
            .zip(&posed.vertices)
            .enumerate()
        {
            let expected = c.transform_point3(DVec3::from_array(
                placement.position_in_source(point).map(f64::from),
            ));
            max_rest = max_rest.max(expected.distance(DVec3::from_array(*rest)));
            let weight: f64 = skin
                .influences
                .normalized_row(vertex)
                .expect("body vertex must be weighted")
                .filter(|(bone, _)| descendants[usize::from(*bone)])
                .map(|(_, weight)| weight)
                .sum();
            let actual = DVec3::from_array(*posed) - DVec3::from_array(*rest);
            let expected = expected_motion * weight;
            max_error = max_error.max(actual.distance(expected));
            max_motion = max_motion.max(actual.length());
            if weight == 0.0 {
                non_descendant_residual = non_descendant_residual.max(actual.length());
            }
            if expected.length() > 1e-4 {
                moving += 1;
                if actual.length() <= 1e-5 {
                    detached += 1;
                }
            }
        }
        eprintln!(
            "{body_file}: graph={} vertices={} rest={max_rest:.9e} pose_error={max_error:.9e} non_descendant={non_descendant_residual:.9e} moving={moving} detached={detached} max_motion={max_motion:.9e}",
            rig.joints().len(),
            asset.positions.len()
        );
        assert!(max_rest <= 1e-4, "{body_file} rest {max_rest}");
        assert!(
            max_error <= 1e-5,
            "{body_file} posed displacement {max_error}"
        );
        assert!(
            non_descendant_residual <= 1e-5,
            "{body_file} stationary region moved"
        );
        assert!(
            max_motion > 0.1 && moving > 0,
            "the real pose must move the limb"
        );
        assert_eq!(detached, 0, "{body_file} weighted vertices must follow");
    }
}
