//! Transferred garment weights reach the existing Actor evaluator. @req DECL0008
//!
//! FIT-08 foundation evidence: real garment parts take the target body's
//! authored weights through `garment::transfer`, keep that body's palette,
//! inverse binds and source `G`, pass the checked rest proof at the body's exact
//! basis, export through `write_rigged_glb` and move in the existing `Actor`.
//!
//! This is a weight and pose foundation, not a fit result, a cloth simulation or
//! full RIG-04 animation acceptance: the garment keeps its authored source rest
//! geometry, so a fitted or conformed garment is not what these tests measure.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{DMat4, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{Clip, ClipChannel, GltfError, write_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose};
use sculpto_mesh::garment::{
    DEFAULT_TRANSFER_RADIUS, Influences, TransferError, TransferOptions, transfer,
};
use sculpto_mesh::weight_norm::{MAX_INFLUENCES, SUM_TOLERANCE, WeightError};
use sculpto_nif::{NifDocument, NifShape};

/// Largest admitted rest, weight or displacement error, source units.
const TOLERANCE: f64 = 1e-4;
/// Smallest admitted vertex movement, source units.
const MOVEMENT: f64 = 0.1;
/// Probe displacement along the selected joint's local `x`, source units.
const PROBE_TRANSLATION: f32 = 1.0;
/// Meaningful limb or pelvis joints, most specific first.
const PROBE_CANDIDATES: [&str; 3] = [
    "NPC L UpperArm [LUar]",
    "NPC L Thigh [LThg]",
    "NPC Pelvis [Pelv]",
];

fn to_f64(point: [f32; 3]) -> DVec3 {
    DVec3::from_array(point.map(f64::from))
}

fn to_f32(point: DVec3) -> [f32; 3] {
    point.to_array().map(|value| value as f32)
}

/// A stored `f32` affine matrix as the exact `f64` transform it denotes.
fn affine(matrix: [[f32; 4]; 4]) -> DMat4 {
    DMat4::from_cols_array_2d(&matrix.map(|column| column.map(f64::from)))
}

fn frame_matrix(frame: RigFrame) -> DMat4 {
    match frame {
        RigFrame::SourceZUp => DMat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]),
        RigFrame::YUp => DMat4::IDENTITY,
    }
}

fn read_document(path: &Path, label: &str) -> NifDocument {
    let bytes = std::fs::read(path).unwrap_or_else(|error| {
        panic!(
            "{label}: configured fixture {} must be readable: {error}",
            path.display()
        )
    });
    sculpto_nif::read(&bytes).unwrap_or_else(|error| {
        panic!(
            "{label}: configured fixture {} must decode completely: {error}",
            path.display()
        )
    })
}

fn shape_by_name<'a>(document: &'a NifDocument, name: &str, label: &str) -> (usize, &'a NifShape) {
    document
        .shapes
        .iter()
        .enumerate()
        .find(|(_, shape)| shape.name == name)
        .unwrap_or_else(|| {
            let found: Vec<&str> = document
                .shapes
                .iter()
                .map(|shape| shape.name.as_str())
                .collect();
            panic!("{label}: configured document must contain shape {name:?}, found {found:?}")
        })
}

/// The body palette's authored rows as name-keyed influences.
fn name_rows(skin: &Skin) -> Vec<Influences> {
    (0..skin.influences.rows())
        .map(|vertex| {
            let row = skin
                .influences
                .row(vertex)
                .expect("a body vertex must carry a weight row");
            row.joints
                .iter()
                .zip(row.weights)
                .map(|(joint, weight)| (skin.bones[usize::from(*joint)].clone(), *weight))
                .collect()
        })
        .collect()
}

/// Refuse an empty, over-wide or off-unit copied row, and report the bounds.
fn copied_weight_contract(rows: &[Influences], label: &str) -> (usize, f32) {
    let mut widest = 0;
    let mut worst = 0.0_f32;
    for (vertex, row) in rows.iter().enumerate() {
        let positive = row.values().filter(|weight| **weight > 0.0).count();
        assert!(
            positive > 0,
            "{label}: garment vertex {vertex} has no copied weight"
        );
        assert!(
            positive <= MAX_INFLUENCES,
            "{label}: garment vertex {vertex} has {positive} influences, above {}",
            MAX_INFLUENCES
        );
        widest = widest.max(positive);
        let sum: f32 = row.values().sum();
        let error = (sum - 1.0).abs();
        assert!(
            error <= SUM_TOLERANCE,
            "{label}: garment vertex {vertex} weight sum {sum} is off by {error}"
        );
        worst = worst.max(error);
    }
    (widest, worst)
}

/// Encode the copied rows against the target body's own palette and binds.
///
/// A copied bone the body palette does not name, a missing bind and a row set
/// that is not a valid skin are refusals with their vertex, not repairs.
fn copied_skin(
    bones: &[String],
    bind_inverse: &[[[f32; 4]; 4]],
    rows: &[Influences],
    label: &str,
) -> Skin {
    assert_eq!(
        bind_inverse.len(),
        bones.len(),
        "{label}: the target body palette needs one authored bind per bone"
    );
    let palette: BTreeMap<&str, u16> = bones
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let slot = u16::try_from(index).unwrap_or_else(|_| {
                panic!("{label}: body palette entry {index} is above the u16 influence range")
            });
            (name.as_str(), slot)
        })
        .collect();
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut joints = Vec::new();
    let mut values = Vec::new();
    offsets.push(0_u32);
    for (vertex, row) in rows.iter().enumerate() {
        for (bone, weight) in row {
            let slot = palette.get(bone.as_str()).unwrap_or_else(|| {
                panic!("{label}: garment vertex {vertex} copied bone {bone:?}, which the target body palette does not name")
            });
            joints.push(*slot);
            values.push(*weight);
        }
        offsets.push(u32::try_from(joints.len()).unwrap_or_else(|_| {
            panic!("{label}: garment influence rows exceed the u32 offset range")
        }));
    }
    let influences =
        SkinWeights::new(offsets, joints, values, bones.len()).unwrap_or_else(|error| {
            panic!("{label}: copied garment rows must form a valid skin: {error}")
        });
    Skin {
        bones: bones.to_vec(),
        bind_inverse: bind_inverse.to_vec(),
        influences,
    }
}

/// One movable joint with its palette slot and descendant palette entries.
struct Probe {
    name: &'static str,
    joint: JointId,
    parent: JointId,
    bone: usize,
    descendants: Vec<bool>,
}

/// Whether each palette bone is `joint` or one of its descendants.
fn descendant_mask(rig: &Rig, joint: JointId, bones: &[String], label: &str) -> Vec<bool> {
    bones
        .iter()
        .map(|palette| {
            let palette_joint = rig.joint_by_name(palette).unwrap_or_else(|error| {
                panic!(
                    "{label}: palette bone {palette:?} is not in the completed family rig: {error}"
                )
            });
            palette_joint == joint
                || rig
                    .is_ancestor(joint, palette_joint)
                    .unwrap_or_else(|error| panic!("{label}: {error}"))
        })
        .collect()
}

/// The first meaningful limb or pelvis joint the garment actually weighs.
fn select_probe(rig: &Rig, skin: &Skin, label: &str) -> Probe {
    let mut readings = Vec::new();
    for name in PROBE_CANDIDATES {
        let Ok(joint) = rig.joint_by_name(name) else {
            readings.push(format!("{name}: no such joint"));
            continue;
        };
        let Some(parent) = rig.joint(joint).and_then(|found| found.parent) else {
            readings.push(format!("{name}: is a root joint"));
            continue;
        };
        let Some(bone) = skin
            .bones
            .iter()
            .position(|candidate| candidate.as_str() == name)
        else {
            readings.push(format!("{name}: not in the target body palette"));
            continue;
        };
        let descendants = descendant_mask(rig, joint, &skin.bones, label);
        let heaviest = (0..skin.influences.rows())
            .map(|vertex| {
                skin.influences
                    .normalized_row(vertex)
                    .expect("a garment vertex row")
                    .filter(|(entry, _)| descendants[usize::from(*entry)])
                    .map(|(_, weight)| weight)
                    .sum::<f64>()
            })
            .fold(0.0, f64::max);
        readings.push(format!("{name}: heaviest descendant weight {heaviest:.6}"));
        if heaviest > MOVEMENT {
            return Probe {
                name,
                joint,
                parent,
                bone,
                descendants,
            };
        }
    }
    panic!("{label}: no meaningful limb or pelvis joint carries garment weight: {readings:?}");
}

/// One configured body and garment pair.
struct Case {
    label: &'static str,
    body: PathBuf,
    body_shape: &'static str,
    body_counts: (usize, usize),
    template: PathBuf,
    garment: PathBuf,
    garment_shape: &'static str,
    garment_counts: (usize, usize),
    copy_radius: f32,
    default_radius_refusal: Option<usize>,
}

fn transferred_garment(case: &Case) {
    let body_document = read_document(&case.body, case.label);
    let template_document = read_document(&case.template, case.label);
    let garment_document = read_document(&case.garment, case.label);

    let (body_index, body_shape) = shape_by_name(&body_document, case.body_shape, case.label);
    let body_original = body_shape.clone();
    let counts = (body_shape.positions.len(), body_shape.triangles.len());
    assert_eq!(
        counts, case.body_counts,
        "{}: the configured body shape {:?} must stay at its pinned source counts",
        case.label, case.body_shape
    );
    let body_skin = body_shape.skin.as_ref().unwrap_or_else(|| {
        panic!(
            "{}: body shape {:?} must carry authored weights",
            case.label, body_shape.name
        )
    });
    let body_global = body_document
        .sources
        .get(body_index)
        .and_then(|source| source.global_skin)
        .unwrap_or_else(|| {
            panic!(
                "{}: body shape {:?} must carry its source global-to-skin matrix",
                case.label, body_shape.name
            )
        });

    let authored = body_document
        .rig
        .as_ref()
        .unwrap_or_else(|| panic!("{}: configured body must author a node graph", case.label));
    let family = template_document.rig.as_ref().unwrap_or_else(|| {
        panic!(
            "{}: configured family template must author a node graph",
            case.label
        )
    });
    let rig = complete_family_rig(
        authored,
        family,
        TemplateRoots {
            authored: *authored.roots().first().unwrap_or_else(|| {
                panic!("{}: configured body graph has no root joint", case.label)
            }),
            template: *family.roots().first().unwrap_or_else(|| {
                panic!(
                    "{}: configured family template has no root joint",
                    case.label
                )
            }),
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "{}: configured body must complete against its family template: {error}",
            case.label
        )
    });
    let skeleton_root = *rig
        .roots()
        .first()
        .unwrap_or_else(|| panic!("{}: completed family rig has no root joint", case.label));
    let body_binding = body_document.bind_rest_on(&rig).unwrap_or_else(|error| {
        panic!(
            "{}: configured body skin must bind at rest: {error}",
            case.label
        )
    });

    let (garment_index, garment_shape) =
        shape_by_name(&garment_document, case.garment_shape, case.label);
    let garment_original = garment_shape.clone();
    let counts = (garment_shape.positions.len(), garment_shape.triangles.len());
    assert_eq!(
        counts, case.garment_counts,
        "{}: the configured garment shape {:?} must stay at its pinned source counts",
        case.label, case.garment_shape
    );
    let placement = garment_document
        .shape_placement(garment_index)
        .unwrap_or_else(|error| {
            panic!(
                "{}: garment shape {:?} must have a source placement: {error}",
                case.label, garment_shape.name
            )
        });
    // The authored scene geometry stays the independent rest expectation; the
    // copy runs in the target body's skin frame, which one `G` reaches from it.
    let scene: Vec<[f32; 3]> = garment_shape
        .positions
        .iter()
        .map(|&point| placement.position_in_source(point))
        .collect();
    let into_body_frame = affine(body_global);
    let body_frame: Vec<[f32; 3]> = scene
        .iter()
        .map(|&point| to_f32(into_body_frame.transform_point3(to_f64(point))))
        .collect();

    let body_rows = name_rows(body_skin);
    if let Some(vertex) = case.default_radius_refusal {
        assert!(
            matches!(
                transfer(&body_frame, &body_shape.positions, &body_rows, &TransferOptions::default()),
                Err(TransferError::Vertex {
                    vertex: failed,
                    error: WeightError::MissingUnlockedMass { remainder: 1.0 },
                }) if failed == vertex
            ),
            "{}: the uncovered default-radius copy must refuse vertex {vertex}",
            case.label
        );
    }
    let copied = transfer(
        &body_frame,
        &body_shape.positions,
        &body_rows,
        &TransferOptions {
            radius: case.copy_radius,
            ..TransferOptions::default()
        },
    )
    .unwrap_or_else(|error| {
        panic!(
            "{}: the native weight copy must answer: {error}",
            case.label
        )
    });
    let (widest, sum_error) = copied_weight_contract(&copied, case.label);
    let garment_skin = copied_skin(
        &body_skin.bones,
        &body_skin.bind_inverse,
        &copied,
        case.label,
    );
    let proof = RestBinding::prove_with_basis(
        &rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: garment_index,
            root: skeleton_root,
            positions: &body_frame,
            skin: &garment_skin,
            global_to_skin: body_global,
        }],
        body_binding.basis(),
    )
    .unwrap_or_else(|error| {
        panic!("{}: copied garment weights must reconstruct the source rest at the body basis: {error}", case.label)
    });

    let probe = select_probe(&rig, &garment_skin, case.label);
    let joint = rig.joint(probe.joint).expect("the selected probe joint");
    let rest_translation = joint.rest.translation;
    let mut moved_translation = rest_translation;
    moved_translation[0] += PROBE_TRANSLATION;
    let clip_name = "garment joint translation probe";
    let clip = Clip {
        name: clip_name.into(),
        times: vec![0.0, 1.0],
        channels: vec![ClipChannel::Translation {
            bone: probe.bone,
            values: vec![rest_translation, moved_translation],
        }],
    };

    let mut asset = MeshAsset::from(garment_shape);
    asset.positions = body_frame.clone();
    asset.skin = Some(garment_skin.clone());
    let bytes = write_rigged_glb(&asset, &proof, garment_index, &[clip]).unwrap_or_else(|error| {
        panic!(
            "{}: the checked garment GLB must encode: {error}",
            case.label
        )
    });
    let actor = Actor::from_glb(&bytes).unwrap_or_else(|error| {
        panic!("{}: Actor must admit the checked GLB: {error}", case.label)
    });
    let rest = actor
        .sample(&[&asset.name], Pose::Rest)
        .unwrap_or_else(|error| panic!("{}: Actor rest sample: {error}", case.label));
    let posed = actor
        .sample(
            &[&asset.name],
            Pose::Clip {
                name: clip_name,
                time: 1.0,
            },
        )
        .unwrap_or_else(|error| panic!("{}: Actor posed sample: {error}", case.label));
    assert_eq!(
        rest.vertices.len(),
        asset.positions.len(),
        "{}: rest vertices",
        case.label
    );
    assert_eq!(
        posed.vertices.len(),
        asset.positions.len(),
        "{}: posed vertices",
        case.label
    );

    // The moved joint's local translation shifts every descendant vertex by the
    // joint's rest parent applied to the displacement; only the descendant
    // weight a vertex carries scales it.
    let frame = frame_matrix(RigFrame::SourceZUp);
    let parent_world = DMat4::from_cols_array_2d(
        &rig.rest_world(probe.parent)
            .unwrap_or_else(|error| panic!("{}: probe parent rest world: {error}", case.label)),
    );
    let motion = (frame * DMat4::from_cols_array_2d(&proof.basis()) * parent_world)
        .transform_vector3(DVec3::X * f64::from(PROBE_TRANSLATION));
    assert!(
        motion.length() > MOVEMENT,
        "{}: the probe translation must be nontrivial",
        case.label
    );

    let mut max_rest = 0.0_f64;
    let mut max_error = 0.0_f64;
    let mut max_stationary = 0.0_f64;
    let mut max_motion = 0.0_f64;
    let mut moving = 0;
    let mut detached = 0;
    for (vertex, ((&point, rest_point), posed_point)) in scene
        .iter()
        .zip(&rest.vertices)
        .zip(&posed.vertices)
        .enumerate()
    {
        let expected_rest = frame.transform_point3(to_f64(point));
        max_rest = max_rest.max(expected_rest.distance(DVec3::from_array(*rest_point)));
        let weight: f64 = garment_skin
            .influences
            .normalized_row(vertex)
            .expect("a copied garment vertex row")
            .filter(|(entry, _)| probe.descendants[usize::from(*entry)])
            .map(|(_, weight)| weight)
            .sum();
        let actual = DVec3::from_array(*posed_point) - DVec3::from_array(*rest_point);
        max_error = max_error.max(actual.distance(motion * weight));
        max_motion = max_motion.max(actual.length());
        if weight == 0.0 {
            max_stationary = max_stationary.max(actual.length());
        }
        if (motion * weight).length() > 1e-4 {
            moving += 1;
            if actual.length() <= 1e-5 {
                detached += 1;
            }
        }
    }

    eprintln!(
        "{}: weight and pose foundation only, not a fit or RIG-04 acceptance; copy_radius={} body={} body_vertices={} garment={} garment_vertices={} triangles={} palette={} influences<={} sum_error={:.9e} joint={} moving={} detached={} rest={:.9e} pose_error={:.9e} stationary={:.9e} max_motion={:.9e}",
        case.label,
        case.copy_radius,
        body_shape.name,
        body_shape.positions.len(),
        garment_shape.name,
        garment_shape.positions.len(),
        garment_shape.triangles.len(),
        garment_skin.bones.len(),
        widest,
        sum_error,
        probe.name,
        moving,
        detached,
        max_rest,
        max_error,
        max_stationary,
        max_motion,
    );
    assert!(
        max_rest <= TOLERANCE,
        "{}: rest reconstruction {max_rest:.9e} exceeds {:.1e}",
        case.label,
        TOLERANCE
    );
    assert!(
        max_error <= TOLERANCE,
        "{}: posed displacement {max_error:.9e} exceeds {:.1e}",
        case.label,
        TOLERANCE
    );
    assert!(
        max_stationary <= TOLERANCE,
        "{}: a vertex without {} weight moved {max_stationary:.9e}",
        case.label,
        probe.name
    );
    assert!(
        max_motion > MOVEMENT && moving > 0,
        "{}: the probe must move garment vertices, {moving} moved by at most {max_motion:.9e}",
        case.label
    );
    assert_eq!(
        detached, 0,
        "{}: weighted garment vertices must follow {}",
        case.label, probe.name
    );
    assert_eq!(
        garment_document.shapes[garment_index], garment_original,
        "{}: the source garment must be preserved",
        case.label
    );
    assert_eq!(
        body_document.shapes[body_index], body_original,
        "{}: the source body must be preserved",
        case.label
    );
}

#[test]
fn configured_garment_weight_copy_reaches_actor() {
    let required = [
        "SCULPTO_FIT_FIXTURE",
        "SCULPTO_MOD_FIXTURE",
        "SCULPTO_RIG_TEMPLATE_FIXTURES",
    ];
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| std::env::var_os(name).is_none())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "SKIP configured_garment_weight_copy_reaches_actor: unset fixture roots {missing:?}; configure SCULPTO_FIT_FIXTURE, SCULPTO_MOD_FIXTURE and SCULPTO_RIG_TEMPLATE_FIXTURES together"
        );
        return;
    }
    let root = |name: &str| PathBuf::from(std::env::var_os(name).expect("a checked fixture root"));
    let fit = root("SCULPTO_FIT_FIXTURE");
    let mod_root = root("SCULPTO_MOD_FIXTURE");
    let rig_root = root("SCULPTO_RIG_TEMPLATE_FIXTURES");
    let nightshade = mod_root.join(
        "apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif",
    );
    let cases = [
        Case {
            label: "sentinel-armor001-on-himbo",
            body: fit.join("body.nif"),
            body_shape: "HIMBO - Body",
            body_counts: (9346, 18074),
            template: rig_root.join("skeleton_male_sse.nif"),
            garment: fit.join("tunic.nif"),
            garment_shape: "Armor001",
            garment_counts: (2737, 4040),
            copy_radius: 11.0,
            default_radius_refusal: Some(1146),
        },
        Case {
            label: "sentinel-armor001-1-on-himbo",
            body: fit.join("body.nif"),
            body_shape: "HIMBO - Body",
            body_counts: (9346, 18074),
            template: rig_root.join("skeleton_male_sse.nif"),
            garment: fit.join("tunic.nif"),
            garment_shape: "Armor001_1",
            garment_counts: (702, 1200),
            copy_radius: DEFAULT_TRANSFER_RADIUS,
            default_radius_refusal: None,
        },
        Case {
            label: "nightshade-pants-on-bijin",
            body: rig_root.join("bijin.nif"),
            body_shape: "BaseShape",
            body_counts: (3471, 6516),
            template: rig_root.join("skeleton_female_sse.nif"),
            garment: nightshade.clone(),
            garment_shape: "pants",
            garment_counts: (392, 632),
            copy_radius: DEFAULT_TRANSFER_RADIUS,
            default_radius_refusal: None,
        },
        Case {
            label: "nightshade-sleeves-on-bijin",
            body: rig_root.join("bijin.nif"),
            body_shape: "BaseShape",
            body_counts: (3471, 6516),
            template: rig_root.join("skeleton_female_sse.nif"),
            garment: nightshade,
            garment_shape: "sleeves",
            garment_counts: (556, 886),
            copy_radius: DEFAULT_TRANSFER_RADIUS,
            default_radius_refusal: None,
        },
    ];
    for case in &cases {
        transferred_garment(case);
    }
}

#[test]
fn foundation_copied_body_frame_weights_and_a_wrong_frame_refusal() {
    const SHAPE: usize = 3;
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
        RigFrame::SourceZUp,
    )
    .expect("non-palette ancestry and an out-of-order graph");
    let basis = DMat4::from_translation(DVec3::new(0.7, -1.3, 0.2)) * DMat4::from_rotation_z(0.25);
    let body_global =
        DMat4::from_translation(DVec3::new(-4.0, 2.5, 0.75)) * DMat4::from_rotation_x(0.3);
    let garment_global =
        DMat4::from_translation(DVec3::new(3.0, -1.0, 2.0)) * DMat4::from_rotation_y(-0.4);
    assert_ne!(
        basis,
        DMat4::IDENTITY,
        "the control needs a nonidentity basis"
    );
    assert_ne!(
        body_global,
        DMat4::IDENTITY,
        "the control needs a nonidentity body G"
    );
    let bind = |entry: u32, global: DMat4| {
        let world = DMat4::from_cols_array_2d(
            &rig.rest_world(JointId::new(entry))
                .expect("a joint of this control rig"),
        );
        ((basis * world).inverse() * global.inverse())
            .to_cols_array_2d()
            .map(|column| column.map(|value| value as f32))
    };
    let body_skin = Skin {
        bones: vec!["limb".into(), "anchor".into()],
        bind_inverse: vec![bind(0, body_global), bind(3, body_global)],
        influences: SkinWeights::new(
            vec![0, 1, 3, 4],
            vec![0, 0, 1, 1],
            vec![1.0, 0.5, 0.5, 1.0],
            2,
        )
        .expect("a limb row and a blended limb/anchor row"),
    };
    let body_positions = [[-1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let root = rig.roots()[0];
    let body_binding = RestBinding::prove(
        &rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: SHAPE,
            root,
            positions: &body_positions,
            skin: &body_skin,
            global_to_skin: to_f32_matrix(body_global),
        }],
    )
    .expect("the authored body binds at its own nonidentity basis");
    assert_ne!(
        body_binding.basis(),
        DMat4::IDENTITY.to_cols_array_2d(),
        "the automatic proof must keep the nonidentity basis"
    );

    // The garment authored its points in the garment scene; the copy runs in the
    // target body's skin frame, so each authored point takes one `G`.
    let scene = [
        DVec3::new(-0.9, 0.1, 0.0),
        DVec3::new(0.05, 0.2, 0.1),
        DVec3::new(0.95, -0.1, 0.05),
    ];
    let authored: Vec<[f32; 3]> = scene
        .iter()
        .map(|point| to_f32(garment_global.transform_point3(*point)))
        .collect();
    let conversion = body_global * garment_global.inverse();
    assert_ne!(conversion, DMat4::IDENTITY, "the frames must differ");
    let copied_frame: Vec<[f32; 3]> = authored
        .iter()
        .map(|&point| to_f32(conversion.transform_point3(to_f64(point))))
        .collect();
    for (index, point) in scene.iter().enumerate() {
        let expected = body_global.transform_point3(*point);
        assert!(
            to_f64(copied_frame[index]).distance(expected) <= TOLERANCE,
            "vertex {index}: one G per frame, error {:.9e}",
            to_f64(copied_frame[index]).distance(expected)
        );
    }
    assert!(
        (0..scene.len())
            .any(|index| to_f64(copied_frame[index]).distance(to_f64(authored[index])) > 1e-3),
        "the control must exercise a real frame change"
    );

    let body_rows = name_rows(&body_skin);
    let copied = transfer(
        &copied_frame,
        &body_positions,
        &body_rows,
        &TransferOptions::default(),
    )
    .expect("the native copy answers for the control body");
    let (widest, sum_error) = copied_weight_contract(&copied, "portable control");
    let garment_skin = copied_skin(
        &body_skin.bones,
        &body_skin.bind_inverse,
        &copied,
        "portable control",
    );
    let proof = RestBinding::prove_with_basis(
        &rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: SHAPE,
            root,
            positions: &copied_frame,
            skin: &garment_skin,
            global_to_skin: to_f32_matrix(body_global),
        }],
        body_binding.basis(),
    )
    .expect("the copied skin reconstructs the source rest at the body basis");

    let probe = Probe {
        name: "limb",
        joint: rig.joint_by_name("limb").expect("the control limb"),
        parent: rig
            .joint_by_name("connector")
            .expect("the control limb parent"),
        bone: garment_skin
            .bones
            .iter()
            .position(|name| name.as_str() == "limb")
            .expect("the control palette names the limb"),
        descendants: descendant_mask(
            &rig,
            rig.joint_by_name("limb").expect("the control limb"),
            &garment_skin.bones,
            "portable control",
        ),
    };
    let rest_translation = rig
        .joint(probe.joint)
        .expect("the selected probe joint")
        .rest
        .translation;
    let mut moved_translation = rest_translation;
    moved_translation[0] += PROBE_TRANSLATION;
    let clip_name = "control limb translation probe";
    let clip = Clip {
        name: clip_name.into(),
        times: vec![0.0, 1.0],
        channels: vec![ClipChannel::Translation {
            bone: probe.bone,
            values: vec![rest_translation, moved_translation],
        }],
    };
    let asset = MeshAsset {
        name: "Garment".into(),
        positions: copied_frame.clone(),
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0); 1],
        skin: Some(garment_skin.clone()),
        ..Default::default()
    };
    let bytes = write_rigged_glb(&asset, &proof, SHAPE, &[clip]).expect("checked control GLB");
    let actor = Actor::from_glb(&bytes).expect("Actor admits the control GLB");
    let rest = actor
        .sample(&[&asset.name], Pose::Rest)
        .expect("control rest sample");
    let posed = actor
        .sample(
            &[&asset.name],
            Pose::Clip {
                name: clip_name,
                time: 1.0,
            },
        )
        .expect("control posed sample");

    let parent_world = DMat4::from_cols_array_2d(
        &rig.rest_world(probe.parent)
            .expect("the probe parent world"),
    );
    let motion = (frame_matrix(RigFrame::SourceZUp)
        * DMat4::from_cols_array_2d(&proof.basis())
        * parent_world)
        .transform_vector3(DVec3::X * f64::from(PROBE_TRANSLATION));
    let mut max_rest = 0.0_f64;
    let mut max_error = 0.0_f64;
    let mut max_motion = 0.0_f64;
    let mut moving = 0;
    let mut detached = 0;
    for (vertex, ((point, rest_point), posed_point)) in scene
        .iter()
        .zip(&rest.vertices)
        .zip(&posed.vertices)
        .enumerate()
    {
        let expected_rest = frame_matrix(RigFrame::SourceZUp).transform_point3(*point);
        max_rest = max_rest.max(expected_rest.distance(DVec3::from_array(*rest_point)));
        let weight: f64 = garment_skin
            .influences
            .normalized_row(vertex)
            .expect("a copied control row")
            .filter(|(entry, _)| probe.descendants[usize::from(*entry)])
            .map(|(_, weight)| weight)
            .sum();
        let actual = DVec3::from_array(*posed_point) - DVec3::from_array(*rest_point);
        max_error = max_error.max(actual.distance(motion * weight));
        max_motion = max_motion.max(actual.length());
        if (motion * weight).length() > 1e-4 {
            moving += 1;
            if actual.length() <= 1e-5 {
                detached += 1;
            }
        }
    }
    eprintln!(
        "portable control: weight and pose foundation only; palette={} influences<={} sum_error={sum_error:.9e} joint={} moving={moving} detached={detached} rest={max_rest:.9e} pose_error={max_error:.9e} max_motion={max_motion:.9e} basis_identity={} body_G_identity={}",
        garment_skin.bones.len(),
        widest,
        probe.name,
        proof.basis() == DMat4::IDENTITY.to_cols_array_2d(),
        body_global == DMat4::IDENTITY,
    );
    assert!(max_rest <= TOLERANCE, "control rest {max_rest:.9e}");
    assert!(
        max_error <= TOLERANCE,
        "control posed displacement {max_error:.9e}"
    );
    assert!(
        max_motion > MOVEMENT && moving > 0,
        "the control probe must move the garment"
    );
    assert_eq!(
        detached, 0,
        "weighted control vertices must follow the probe"
    );

    // A garment-frame asset against a body-frame binding is refused, and so is a
    // skin that declares the garment frame as the body skin frame.
    let mut wrong_asset = asset.clone();
    wrong_asset.positions = authored.clone();
    assert!(
        matches!(
            write_rigged_glb(&wrong_asset, &proof, SHAPE, &[]),
            Err(GltfError::RigBinding {
                field: "positions",
                ..
            })
        ),
        "an unconverted garment-frame asset must not pass the body-frame binding"
    );
    let declared = RestBinding::prove_with_basis(
        &rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: SHAPE,
            root,
            positions: &copied_frame,
            skin: &garment_skin,
            global_to_skin: to_f32_matrix(garment_global),
        }],
        body_binding.basis(),
    );
    assert!(
        declared.is_err(),
        "declaring the garment frame as the body skin frame must be refused"
    );
}

/// A control matrix in the `f32` shape a source `G` is stored in.
fn to_f32_matrix(matrix: DMat4) -> [[f32; 4]; 4] {
    matrix
        .to_cols_array_2d()
        .map(|column| column.map(|value| value as f32))
}
