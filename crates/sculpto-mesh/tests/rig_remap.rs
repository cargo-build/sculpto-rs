//! The neutral skin remap keeps geometry, locks and rest authority. @req DECL0008

use glam::{DMat4, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{Clip, ClipChannel, write_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose};
use sculpto_mesh::rig::map::{BodyFamily, BoneMap, BoneMapError, Collapse};
use sculpto_mesh::rig::remap::{GraphSide, RemapError, RemapSettings, remap_skin};
use sculpto_mesh::weight_norm::WeightError;

const BODY_SHAPE: usize = 3;
const GARMENT_SHAPE: usize = 9;
const TARGET_SHAPE: usize = 5;

// Source rig ids: root 0, spine 1, arm 2, skirt 3, cuff 4, hem 5, extra 6.
// Target rig ids: root 0, spine 1, arm 2, anchor 3, hip 4.
const SKIRT: JointId = JointId::new(3);
const HEM: JointId = JointId::new(5);
const ANCHOR: JointId = JointId::new(3);
const HIP: JointId = JointId::new(4);
const SPINE: JointId = JointId::new(1);

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

fn rotated(name: &str, parent: Option<u32>, translation: [f32; 3], angle: f32) -> Joint {
    Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            rotation: [0.0, 0.0, (angle / 2.0).sin(), (angle / 2.0).cos()],
            ..Default::default()
        },
    }
}

fn source_rig(frame: RigFrame) -> Rig {
    Rig::new(
        vec![
            joint("root", None, [1.0, 0.0, 2.0]),
            joint("spine", Some(0), [0.0, 2.0, 0.0]),
            rotated("arm", Some(1), [3.0, 0.0, 0.0], 0.6),
            joint("skirt", Some(1), [0.0, -1.0, 0.0]),
            joint("cuff", Some(2), [1.0, 0.0, 0.0]),
            joint("hem", Some(1), [0.0, -2.0, 0.0]),
            joint("extra", Some(0), [7.0, 0.0, 0.0]),
        ],
        frame,
    )
    .expect("valid source graph")
}

fn target_rig(frame: RigFrame) -> Rig {
    Rig::new(
        vec![
            joint("root", None, [-2.0, 0.0, 1.0]),
            joint("spine", Some(0), [0.0, 1.5, 0.0]),
            rotated("arm", Some(1), [2.5, 0.0, 0.0], 0.9),
            joint("anchor", Some(0), [0.0, 4.0, 0.0]),
            joint("hip", Some(0), [1.0, -3.0, 0.0]),
        ],
        frame,
    )
    .expect("valid target graph")
}

fn collapse_declarations() -> [Collapse<'static>; 3] {
    [
        Collapse {
            source: "skirt",
            target: "anchor",
        },
        Collapse {
            source: "hem",
            target: "anchor",
        },
        Collapse {
            source: "cuff",
            target: "hip",
        },
    ]
}

fn source_basis() -> DMat4 {
    DMat4::from_rotation_z(0.4) * DMat4::from_translation(DVec3::new(1.5, -0.5, 0.25))
}

fn source_global() -> DMat4 {
    DMat4::from_translation(DVec3::new(4.0, -2.0, 3.0)) * DMat4::from_rotation_x(0.3)
}

fn target_basis() -> DMat4 {
    DMat4::from_translation(DVec3::new(0.7, -1.3, 0.2)) * DMat4::from_rotation_z(0.25)
}

fn target_global() -> DMat4 {
    DMat4::from_translation(DVec3::new(-1.0, 2.0, 0.5))
}

fn f32_of(value: DMat4) -> [[f32; 4]; 4] {
    value
        .to_cols_array_2d()
        .map(|column| column.map(|x| x as f32))
}

fn f64_of(value: [[f32; 4]; 4]) -> DMat4 {
    DMat4::from_cols_array_2d(&value.map(|column| column.map(f64::from)))
}

fn assert_close(left: DMat4, right: DMat4, tolerance: f64, context: &str) {
    assert!(
        (left - right).to_cols_array().into_iter().all(|value| value.abs() < tolerance),
        "{context}: {left} vs {right}"
    );
}

fn binds(rig: &Rig, basis: DMat4, global: DMat4, bones: &[&str]) -> Vec<[[f32; 4]; 4]> {
    bones
        .iter()
        .map(|name| {
            let joint = rig.joint_by_name(name).expect("fixture joint");
            let world = DMat4::from_cols_array_2d(&rig.rest_world(joint).expect("fixture rest world"));
            f32_of((basis * world).inverse() * global.inverse())
        })
        .collect()
}

fn skin(rig: &Rig, basis: DMat4, global: DMat4, bones: &[&str], rows: &[(&[u16], &[f32])]) -> Skin {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut joints = Vec::new();
    let mut values = Vec::new();
    offsets.push(0);
    for (slots, weights) in rows {
        joints.extend_from_slice(slots);
        values.extend_from_slice(weights);
        offsets.push(joints.len() as u32);
    }
    Skin {
        bones: bones.iter().map(|name| (*name).into()).collect(),
        bind_inverse: binds(rig, basis, global, bones),
        influences: SkinWeights::new(offsets, joints, values, bones.len())
            .expect("fixture influence rows"),
    }
}

fn bound<'a>(
    rig: &'a Rig,
    shape: usize,
    positions: &'a [[f32; 3]],
    skin: &'a Skin,
    global: DMat4,
) -> SkinRestSource<'a> {
    SkinRestSource {
        shape,
        root: rig.roots()[0],
        positions,
        skin,
        global_to_skin: f32_of(global),
    }
}

fn row_entries(skin: &Skin, vertex: usize) -> Vec<(u16, f32)> {
    let row = skin.influences.row(vertex).expect("output row");
    row.joints
        .iter()
        .copied()
        .zip(row.weights.iter().copied())
        .collect()
}

fn slot(skin: &Skin, name: &str) -> u16 {
    skin.bones
        .iter()
        .position(|bone| bone == name)
        .expect("palette bone") as u16
}

fn body_positions() -> Vec<[f32; 3]> {
    vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.5, 0.0],
        [2.0, 1.0, 1.0],
        [-1.0, 2.0, 0.5],
    ]
}

fn garment_positions() -> Vec<[f32; 3]> {
    vec![[0.5, 1.0, 0.0], [1.5, 0.0, 2.0]]
}

fn target_positions() -> Vec<[f32; 3]> {
    vec![
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 2.0],
        [2.0, 2.0, 1.0],
        [0.5, 0.5, 0.5],
        [1.0, 1.0, 1.0],
    ]
}

fn body_skin(source: &Rig, basis: DMat4, global: DMat4) -> Skin {
    skin(
        source,
        basis,
        global,
        &["root", "spine", "arm", "skirt", "cuff", "hem"],
        &[
            (&[2], &[1.0]),
            (&[1, 2], &[0.5, 0.5]),
            (&[3, 5, 1], &[0.3, 0.2, 0.5]),
            (&[0, 1, 2, 3, 4], &[0.30, 0.24, 0.20, 0.16, 0.10]),
        ],
    )
}

fn garment_skin(source: &Rig, basis: DMat4, global: DMat4) -> Skin {
    skin(
        source,
        basis,
        global,
        &["skirt", "spine"],
        &[(&[0], &[1.0]), (&[0, 1], &[0.4, 0.6])],
    )
}

#[test]
fn a_checked_document_remaps_across_nonidentity_rest_graphs() {
    for frame in [RigFrame::SourceZUp, RigFrame::YUp] {
        let source_rig = source_rig(frame);
        let target_rig = target_rig(frame);
        let target_positions = target_positions();
        let bs = source_basis();
        let gs = source_global();
        let bt = target_basis();
        let gt = target_global();
        let body_positions = body_positions();
        let garment_positions = garment_positions();
        let body_skin = body_skin(&source_rig, bs, gs);
        let garment_skin = garment_skin(&source_rig, bs, gs);
        let target_skin = skin(
            &target_rig,
            bt,
            gt,
            &["root", "spine", "arm", "anchor", "hip"],
            &[
                (&[0], &[1.0]),
                (&[1], &[1.0]),
                (&[2], &[1.0]),
                (&[3], &[1.0]),
                (&[4], &[1.0]),
            ],
        );

        let source_binding = RestBinding::prove(
            &source_rig,
            frame,
            &[
                bound(
                    &source_rig,
                    BODY_SHAPE,
                    &body_positions,
                    &body_skin,
                    gs,
                ),
                bound(
                    &source_rig,
                    GARMENT_SHAPE,
                    &garment_positions,
                    &garment_skin,
                    gs,
                ),
            ],
        )
        .expect("checked source document");
        let target_binding = RestBinding::prove(
            &target_rig,
            frame,
            &[bound(
                &target_rig,
                TARGET_SHAPE,
                &target_positions,
                &target_skin,
                gt,
            )],
        )
        .expect("checked target document");
        assert_close(
            DMat4::from_cols_array_2d(&source_binding.basis()),
            bs,
            1.0e-5,
            "automatic source candidate",
        );
        assert_close(
            DMat4::from_cols_array_2d(&target_binding.basis()),
            bt,
            1.0e-5,
            "automatic target candidate",
        );

        let map = BoneMap::build(
            BodyFamily::Himbo,
            &source_binding,
            BodyFamily::Cbbe,
            &target_rig,
            &collapse_declarations(),
        )
        .expect("family map");
        let settings = RemapSettings {
            source_shape: BODY_SHAPE,
            target_shape: TARGET_SHAPE,
            locked_source_joints: Vec::new(),
        };
        let remapped =
            remap_skin(&source_binding, &map, &target_binding, &settings).expect("neutral body");

        // Graph identity, target reference root and unchanged source inputs.
        assert!(std::ptr::eq(remapped.rig, &target_rig));
        assert_eq!(remapped.root, target_rig.roots()[0]);
        assert_eq!(remapped.global_to_skin, f32_of(gs));
        assert_eq!(source_binding.skins()[0].source().skin, &body_skin);
        assert_eq!(
            source_binding.skins()[0].source().positions,
            body_positions.as_slice()
        );

        // Deterministic palette: used destinations in target joint order.
        assert_eq!(
            remapped.skin.bones,
            ["root", "spine", "arm", "anchor", "hip"]
        );

        // Direct and collapsed weights, packed to unit sums per row.
        let spine_slot = slot(&remapped.skin, "spine");
        let (arm_slot, anchor_slot) = (slot(&remapped.skin, "arm"), slot(&remapped.skin, "anchor"));
        assert_eq!(row_entries(&remapped.skin, 0), [(arm_slot, 1.0)]);
        assert_eq!(
            row_entries(&remapped.skin, 1),
            [(spine_slot, 0.5), (arm_slot, 0.5)]
        );
        assert_eq!(
            row_entries(&remapped.skin, 2),
            [(spine_slot, 0.5), (anchor_slot, 0.5)]
        );
        let packed_row = row_entries(&remapped.skin, 3);
        assert_eq!(
            packed_row.len(),
            4,
            "the five-to-four cap keeps four entries"
        );
        assert!(
            !packed_row
                .iter()
                .any(|(bone, _)| *bone == slot(&remapped.skin, "hip")),
            "the smallest aggregated weight loses its slot"
        );
        for vertex in 0..4 {
            let sum: f32 = row_entries(&remapped.skin, vertex)
                .iter()
                .map(|(_, weight)| *weight)
                .sum();
            assert!((sum - 1.0).abs() < 1.0e-4, "vertex {vertex} packs to one");
        }

        // Declared loss with vertex/identity evidence, in raw source units.
        assert_eq!(remapped.report.discarded.len(), 1);
        let discarded = &remapped.report.discarded[0];
        assert_eq!(discarded.vertex, 3);
        assert_eq!(discarded.destination, HIP);
        assert_eq!(discarded.weight, 0.10);

        // Report merges and singleton collapses, including a later dropped target.
        assert_eq!(remapped.report.collapses.len(), 3);
        let collapse = &remapped.report.collapses[0];
        assert_eq!(collapse.vertex, 2);
        assert_eq!(collapse.destination, ANCHOR);
        assert_eq!(collapse.sources, [(SKIRT, 0.3), (HEM, 0.2)]);
        assert!((collapse.source_sum - 0.5).abs() < 1.0e-6);
        let singleton = &remapped.report.collapses[1];
        assert_eq!(singleton.vertex, 3);
        assert_eq!(singleton.destination, ANCHOR);
        assert_eq!(singleton.sources, [(SKIRT, 0.16)]);
        let discarded_collapse = &remapped.report.collapses[2];
        assert_eq!(discarded_collapse.vertex, 3);
        assert_eq!(discarded_collapse.destination, HIP);
        assert_eq!(discarded_collapse.sources, [(source_rig.joint_by_name("cuff").expect("source cuff"), 0.1)]);
        assert!(remapped.report.measurement.max_vertex < 1.0e-4);

        // The garment shape transfers with its own smaller palette.
        let garment_settings = RemapSettings {
            source_shape: GARMENT_SHAPE,
            target_shape: TARGET_SHAPE,
            locked_source_joints: Vec::new(),
        };
        let garment = remap_skin(&source_binding, &map, &target_binding, &garment_settings)
            .expect("neutral garment");
        assert_eq!(garment.skin.bones, ["spine", "anchor"]);
        assert_eq!(garment.skin.influences.rows(), 2);
        assert_eq!(
            row_entries(&garment.skin, 0),
            [(slot(&garment.skin, "anchor"), 1.0)]
        );
        assert_eq!(
            row_entries(&garment.skin, 1),
            [
                (slot(&garment.skin, "spine"), 0.6),
                (slot(&garment.skin, "anchor"), 0.4)
            ]
        );
        assert_eq!(garment.report.collapses.len(), 2);
        assert_eq!(garment.report.collapses[0].sources, [(SKIRT, 1.0)]);
        assert_eq!(garment.report.collapses[1].sources, [(SKIRT, 0.4)]);
        assert!(garment.report.discarded.is_empty());
        assert!(garment.report.measurement.max_vertex < 1.0e-4);
    }
}

#[test]
fn locked_weights_transfer_bit_exactly_and_impossible_locks_refuse() {
    let frame = RigFrame::SourceZUp;
    let source_rig = source_rig(frame);
    let target_rig = target_rig(frame);
    let target_positions = target_positions();
    let bs = source_basis();
    let gs = source_global();
    let bt = target_basis();
    let gt = target_global();
    let positions = body_positions();
    let target_skin = skin(
        &target_rig,
        bt,
        gt,
        &["root", "spine", "arm", "anchor", "hip"],
        &[
            (&[0], &[1.0]),
            (&[1], &[1.0]),
            (&[2], &[1.0]),
            (&[3], &[1.0]),
            (&[4], &[1.0]),
        ],
    );
    let target_binding = RestBinding::prove(
        &target_rig,
        frame,
        &[bound(
            &target_rig,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        )],
    )
    .expect("checked target document");

    // A locked source weight keeps its exact f32 value; the row renormalizes
    // around it without touching it.
    let locked_row: (&[u16], &[f32]) = (&[0, 1], &[0.8, 0.1]);
    let locked_skin = skin(&source_rig, bs, gs, &["spine", "arm"], &[locked_row; 4]);
    let source_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &locked_skin,
            gs,
        )],
    )
    .expect("checked source document");
    let map = BoneMap::build(
        BodyFamily::Himbo,
        &source_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let settings = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: vec![SPINE],
    };
    let remapped = remap_skin(&source_binding, &map, &target_binding, &settings)
        .expect("a lone locked weight transfers exactly");
    let locked_value = row_entries(&remapped.skin, 0)
        .iter()
        .find(|(bone, _)| *bone == slot(&remapped.skin, "spine"))
        .expect("locked destination stays in the row")
        .1;
    assert_eq!(locked_value.to_bits(), 0.8_f32.to_bits());
    let sum: f32 = row_entries(&remapped.skin, 0).iter().map(|(_, w)| *w).sum();
    assert!((sum - 1.0).abs() < 1.0e-4);

    // A locked mass above one is refused through the pack's own error.
    let overweight_row: (&[u16], &[f32]) = (&[0, 1], &[1.5, 0.2]);
    let overweight = skin(&source_rig, bs, gs, &["spine", "arm"], &[overweight_row; 4]);
    let overweight_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &overweight,
            gs,
        )],
    )
    .expect("checked source document");
    let overweight_map = BoneMap::build(
        BodyFamily::Himbo,
        &overweight_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    assert_eq!(
        remap_skin(
            &overweight_binding,
            &overweight_map,
            &target_binding,
            &settings
        ),
        Err(RemapError::Pack {
            vertex: 0,
            error: WeightError::LockedMassOverOne { locked: 1.5 }
        })
    );

    // Five locked positives cannot fit the four hardware slots.
    let five_row: (&[u16], &[f32]) = (&[0, 1, 2, 3, 4], &[0.2; 5]);
    let five_locked = skin(
        &source_rig,
        bs,
        gs,
        &["root", "spine", "arm", "skirt", "cuff"],
        &[five_row; 4],
    );
    let five_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &five_locked,
            gs,
        )],
    )
    .expect("checked source document");
    let five_map = BoneMap::build(
        BodyFamily::Himbo,
        &five_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let five_settings = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: vec![
            JointId::new(0),
            JointId::new(1),
            JointId::new(2),
            JointId::new(3),
            JointId::new(4),
        ],
    };
    assert_eq!(
        remap_skin(&five_binding, &five_map, &target_binding, &five_settings),
        Err(RemapError::Pack {
            vertex: 0,
            error: WeightError::TooManyLocked {
                locked: 5,
                slots: 4
            }
        })
    );

    // A locked collapse source sharing its destination cannot stay
    // independent, so the collision is named instead of silently summed.
    let collision_row: (&[u16], &[f32]) = (&[0, 1, 2], &[0.5, 0.3, 0.2]);
    let collision = skin(
        &source_rig,
        bs,
        gs,
        &["spine", "skirt", "hem"],
        &[collision_row; 4],
    );
    let collision_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &collision,
            gs,
        )],
    )
    .expect("checked source document");
    let collision_map = BoneMap::build(
        BodyFamily::Himbo,
        &collision_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let collision_settings = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: vec![SKIRT],
    };
    assert_eq!(
        remap_skin(
            &collision_binding,
            &collision_map,
            &target_binding,
            &collision_settings
        ),
        Err(RemapError::LockCollapse {
            vertex: 0,
            locked: SKIRT,
            destination: ANCHOR
        })
    );

    let zero_row: (&[u16], &[f32]) = (&[0, 1, 2], &[0.5, 0.0, 0.5]);
    let zero_collision = skin(
        &source_rig,
        bs,
        gs,
        &["spine", "skirt", "hem"],
        &[zero_row; 4],
    );
    let zero_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(&source_rig, BODY_SHAPE, &positions, &zero_collision, gs)],
    )
    .expect("source with a locked zero");
    let zero_map = BoneMap::build(
        BodyFamily::Himbo, &zero_binding, BodyFamily::Cbbe,
        &target_rig, &collapse_declarations(),
    )
    .expect("declared collapse into the locked destination");
    assert_eq!(
        remap_skin(&zero_binding, &zero_map, &target_binding, &collision_settings),
        Err(RemapError::LockCollapse {
            vertex: 0, locked: SKIRT, destination: ANCHOR,
        }),
        "a zero lock cannot acquire another source's weight"
    );
}

#[test]
fn an_unsupported_positive_joint_is_a_named_refusal() {
    let frame = RigFrame::SourceZUp;
    let source_rig = source_rig(frame);
    let target_rig = target_rig(frame);
    let target_positions = target_positions();
    let bs = source_basis();
    let gs = source_global();
    let bt = target_basis();
    let gt = target_global();
    let positions = body_positions();
    let target_skin = skin(
        &target_rig,
        bt,
        gt,
        &["root", "spine", "arm", "anchor", "hip"],
        &[
            (&[0], &[1.0]),
            (&[1], &[1.0]),
            (&[2], &[1.0]),
            (&[3], &[1.0]),
            (&[4], &[1.0]),
        ],
    );
    let target_binding = RestBinding::prove(
        &target_rig,
        frame,
        &[bound(
            &target_rig,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        )],
    )
    .expect("checked target document");

    let extra_row: (&[u16], &[f32]) = (&[0, 1], &[0.5, 0.5]);
    let weighted_extra = skin(&source_rig, bs, gs, &["spine", "extra"], &[extra_row; 4]);
    let source_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &weighted_extra,
            gs,
        )],
    )
    .expect("checked source document");
    let map = BoneMap::build(
        BodyFamily::Himbo,
        &source_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let settings = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: Vec::new(),
    };
    assert_eq!(
        remap_skin(&source_binding, &map, &target_binding, &settings),
        Err(RemapError::Map(BoneMapError::Unsupported {
            joint: JointId::new(6),
            name: Some("extra".into())
        }))
    );

    // A zero-weight entry of the same unsupported bone may be omitted.
    let unweighted_row: (&[u16], &[f32]) = (&[0, 1], &[1.0, 0.0]);
    let unweighted_extra = skin(
        &source_rig,
        bs,
        gs,
        &["spine", "extra"],
        &[unweighted_row; 4],
    );
    let unweighted_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &unweighted_extra,
            gs,
        )],
    )
    .expect("checked source document");
    let unweighted_map = BoneMap::build(
        BodyFamily::Himbo,
        &unweighted_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let remapped = remap_skin(
        &unweighted_binding,
        &unweighted_map,
        &target_binding,
        &settings,
    )
    .expect("an unweighted unsupported entry omits itself");
    assert_eq!(remapped.skin.bones, ["spine"]);
}

#[test]
fn mismatched_owners_and_ambiguous_shape_selections_are_refused() {
    let frame = RigFrame::SourceZUp;
    let source_rig = source_rig(frame);
    let target_rig = target_rig(frame);
    let target_positions = target_positions();
    let bs = source_basis();
    let gs = source_global();
    let bt = target_basis();
    let gt = target_global();
    let positions = body_positions();
    let body_skin = body_skin(&source_rig, bs, gs);
    let target_skin = skin(
        &target_rig,
        bt,
        gt,
        &["root", "spine", "arm", "anchor", "hip"],
        &[
            (&[0], &[1.0]),
            (&[1], &[1.0]),
            (&[2], &[1.0]),
            (&[3], &[1.0]),
            (&[4], &[1.0]),
        ],
    );
    let source_binding = RestBinding::prove(
        &source_rig,
        frame,
        &[bound(
            &source_rig,
            BODY_SHAPE,
            &positions,
            &body_skin,
            gs,
        )],
    )
    .expect("checked source document");
    let target_binding = RestBinding::prove(
        &target_rig,
        frame,
        &[bound(
            &target_rig,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        )],
    )
    .expect("checked target document");
    let map = BoneMap::build(
        BodyFamily::Himbo,
        &source_binding,
        BodyFamily::Cbbe,
        &target_rig,
        &collapse_declarations(),
    )
    .expect("family map");
    let settings = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: Vec::new(),
    };

    // A cloned-but-equal rig is still another graph: its joint ids do not
    // belong to the map.
    let source_clone = source_rig.clone();
    let clone_binding = RestBinding::prove(
        &source_clone,
        frame,
        &[bound(
            &source_clone,
            BODY_SHAPE,
            &positions,
            &body_skin,
            gs,
        )],
    )
    .expect("checked clone document");
    assert_eq!(
        remap_skin(&clone_binding, &map, &target_binding, &settings),
        Err(RemapError::GraphOwner {
            side: GraphSide::Source
        })
    );
    let target_clone = target_rig.clone();
    let target_clone_binding = RestBinding::prove(
        &target_clone,
        frame,
        &[bound(
            &target_clone,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        )],
    )
    .expect("checked clone document");
    assert_eq!(
        remap_skin(&source_binding, &map, &target_clone_binding, &settings),
        Err(RemapError::GraphOwner {
            side: GraphSide::Target
        })
    );

    // Shape selections must be exact: no default and no ambiguity.
    let missing = RemapSettings {
        source_shape: 42,
        target_shape: TARGET_SHAPE,
        locked_source_joints: Vec::new(),
    };
    assert_eq!(
        remap_skin(&source_binding, &map, &target_binding, &missing),
        Err(RemapError::MissingShape {
            side: GraphSide::Source,
            shape: 42
        })
    );
    let missing_target = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: 7,
        locked_source_joints: Vec::new(),
    };
    assert_eq!(
        remap_skin(&source_binding, &map, &target_binding, &missing_target),
        Err(RemapError::MissingShape {
            side: GraphSide::Target,
            shape: 7
        })
    );
    let duplicate_source = [BODY_SHAPE, BODY_SHAPE]
        .map(|shape| bound(&source_rig, shape, &positions, &body_skin, gs));
    let duplicate_binding =
        RestBinding::prove(&source_rig, frame, &duplicate_source).expect("equal shapes still bind");
    assert_eq!(
        remap_skin(&duplicate_binding, &map, &target_binding, &settings),
        Err(RemapError::DuplicateShape {
            side: GraphSide::Source,
            shape: BODY_SHAPE
        })
    );
    let duplicate_target = [
        bound(
            &target_rig,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        ),
        bound(
            &target_rig,
            TARGET_SHAPE,
            &target_positions,
            &target_skin,
            gt,
        ),
    ];
    let duplicate_target_binding =
        RestBinding::prove(&target_rig, frame, &duplicate_target).expect("equal shapes still bind");
    assert_eq!(
        remap_skin(&source_binding, &map, &duplicate_target_binding, &settings),
        Err(RemapError::DuplicateShape {
            side: GraphSide::Target,
            shape: TARGET_SHAPE
        })
    );

    // Lock declarations are validated against the source graph.
    let unknown_lock = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: vec![JointId::new(99)],
    };
    assert_eq!(
        remap_skin(&source_binding, &map, &target_binding, &unknown_lock),
        Err(RemapError::UnknownLock {
            joint: JointId::new(99),
            joints: 7
        })
    );
    let duplicate_lock = RemapSettings {
        source_shape: BODY_SHAPE,
        target_shape: TARGET_SHAPE,
        locked_source_joints: vec![SPINE, SPINE],
    };
    assert_eq!(
        remap_skin(&source_binding, &map, &target_binding, &duplicate_lock),
        Err(RemapError::DuplicateLock { joint: SPINE })
    );
}

#[test]
fn a_remapped_skin_exports_and_poses_through_the_existing_actor() {
    for frame in [RigFrame::SourceZUp, RigFrame::YUp] {
        let source_rig = source_rig(frame);
        let target_rig = target_rig(frame);
        let target_positions = target_positions();
        let bs = source_basis();
        let gs = source_global();
        let bt = target_basis();
        let gt = target_global();
        let positions = body_positions();
        // A nontrivial moving limb and fixed non-descendant weights.
        let pose_skin = skin(
            &source_rig,
            bs,
            gs,
            &["arm", "spine"],
            &[
                (&[0], &[1.0]),
                (&[1], &[1.0]),
                (&[0, 1], &[0.5, 0.5]),
                (&[0], &[1.0]),
            ],
        );
        let target_skin = skin(
            &target_rig,
            bt,
            gt,
            &["root", "spine", "arm", "anchor", "hip"],
            &[
                (&[0], &[1.0]),
                (&[1], &[1.0]),
                (&[2], &[1.0]),
                (&[3], &[1.0]),
                (&[4], &[1.0]),
            ],
        );
        let source_binding = RestBinding::prove(
            &source_rig,
            frame,
            &[bound(
                &source_rig,
                BODY_SHAPE,
                &positions,
                &pose_skin,
                gs,
            )],
        )
        .expect("checked source document");
        let target_binding = RestBinding::prove(
            &target_rig,
            frame,
            &[bound(
                &target_rig,
                TARGET_SHAPE,
                &target_positions,
                &target_skin,
                gt,
            )],
        )
        .expect("checked target document");
        let map = BoneMap::build(
            BodyFamily::Himbo,
            &source_binding,
            BodyFamily::Cbbe,
            &target_rig,
            &collapse_declarations(),
        )
        .expect("family map");
        let settings = RemapSettings {
            source_shape: BODY_SHAPE,
            target_shape: TARGET_SHAPE,
            locked_source_joints: Vec::new(),
        };
        let remapped = remap_skin(&source_binding, &map, &target_binding, &settings)
            .expect("neutral transfer");

        // The caller proves the encoded output at the target basis explicitly.
        let proof_source = SkinRestSource {
            shape: BODY_SHAPE,
            root: remapped.root,
            positions: &positions,
            skin: &remapped.skin,
            global_to_skin: remapped.global_to_skin,
        };
        let output_binding = RestBinding::prove_with_basis(
            remapped.rig,
            frame,
            &[proof_source],
            target_binding.basis(),
        )
        .expect("encoded output proof at the target basis");
        assert!(
            RestBinding::prove_with_basis(
                remapped.rig,
                frame,
                &[proof_source],
                DMat4::IDENTITY.to_cols_array_2d(),
            )
            .is_err(),
            "the encoded binds answer only the target basis"
        );

        let asset = MeshAsset {
            name: "Body".into(),
            positions: positions.clone(),
            triangles: vec![[0, 1, 3], [1, 2, 3]],
            triangle_partitions: vec![PartitionId(0); 2],
            skin: Some(remapped.skin.clone()),
            ..Default::default()
        };
        asset.validate().expect("the remapped asset validates");
        let arm_slot = remapped
            .skin
            .bones
            .iter()
            .position(|name| name == "arm")
            .expect("arm palette entry");
        let arm_joint = target_rig.joint_by_name("arm").expect("target arm");
        let rest_translation = target_rig
            .joint(arm_joint)
            .expect("target joint")
            .rest
            .translation;
        let mut moved_translation = rest_translation;
        moved_translation[0] += 1.0;
        let clip = Clip {
            name: "swing arm".into(),
            times: vec![0.0, 1.0],
            channels: vec![ClipChannel::Translation {
                bone: arm_slot,
                values: vec![rest_translation, moved_translation],
            }],
        };
        let bytes =
            write_rigged_glb(&asset, &output_binding, BODY_SHAPE, &[clip]).expect("remapped GLB");
        let actor = Actor::from_glb(&bytes).expect("remapped Actor admission");
        let rest = actor.sample(&["Body"], Pose::Rest).expect("rest sample");
        let posed = actor
            .sample(
                &["Body"],
                Pose::Clip {
                    name: "swing arm",
                    time: 1.0,
                },
            )
            .expect("posed sample");

        let c = frame_matrix(frame);
        let placement = f64_of(remapped.global_to_skin).inverse();
        let delta =
            (c * DMat4::from_cols_array_2d(&output_binding.basis())).transform_vector3(DVec3::X);
        assert!(delta.length() > 0.1, "the clip must be nontrivial");
        let mut non_descendant_residual = 0.0_f64;
        for (index, ((point, rest_vertex), posed_vertex)) in positions
            .iter()
            .zip(&rest.vertices)
            .zip(&posed.vertices)
            .enumerate()
        {
            let expected_rest =
                (c * placement).transform_point3(DVec3::from_array(point.map(f64::from)));
            assert!(
                expected_rest.distance(DVec3::from_array(*rest_vertex)) < 1.0e-4,
                "frame {frame:?}, rest vertex {index}"
            );
            let arm_weight: f64 = remapped
                .skin
                .influences
                .normalized_row(index)
                .expect("output row")
                .filter(|(bone, _)| usize::from(*bone) == arm_slot)
                .map(|(_, weight)| weight)
                .sum();
            let actual = DVec3::from_array(*posed_vertex) - DVec3::from_array(*rest_vertex);
            if arm_weight == 0.0 {
                non_descendant_residual = non_descendant_residual.max(actual.length());
            }
            let expected_delta = delta * arm_weight;
            assert!(
                actual.distance(expected_delta) < 1.0e-5,
                "frame {frame:?}, posed vertex {index}: {actual:?}"
            );
        }
        assert!(
            non_descendant_residual <= 1.0e-5,
            "fixed non-descendant vertices moved"
        );
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
