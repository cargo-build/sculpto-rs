//! The palette step that roots the torso under the hips: a graph edit that
//! preserves every rest world, expressed per naming family. @req DECL0008

use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_engine::export_glb::follow_hips;

fn joint(name: &str, parent: Option<JointId>, rest: LocalTransform) -> Joint {
    Joint {
        name: Some(name.into()),
        parent,
        rest,
    }
}

fn place(z: f32, y: f32) -> LocalTransform {
    LocalTransform {
        translation: [0.0, y, z],
        ..LocalTransform::default()
    }
}

/// Every joint's rest world keyed by name: a re-parent changes the topological
/// order, so the comparison must not depend on it.
fn worlds(rig: &Rig) -> std::collections::BTreeMap<String, [[f64; 4]; 4]> {
    rig.joints()
        .iter()
        .enumerate()
        .filter_map(|(index, joint)| {
            joint.name.as_ref().map(|name| {
                (
                    name.clone(),
                    rig.rest_world(JointId::new(index as u32))
                        .expect("rest world"),
                )
            })
        })
        .collect()
}

/// Emily's shape: one static root offset, the hips and the torso as siblings.
fn authored() -> Rig {
    Rig::new(
        vec![
            joint("NPC Root [Root]", None, place(0.0, 0.0)),
            joint("NPC COM [COM ]", Some(JointId::new(0)), place(0.95, 0.0)),
            joint("NPC Pelvis [Pelv]", Some(JointId::new(1)), place(0.0, 0.0)),
            joint("NPC Spine [Spn0]", Some(JointId::new(1)), place(0.01, 0.0)),
            joint("NPC Spine1 [Spn1]", Some(JointId::new(3)), place(0.05, 0.0)),
        ],
        RigFrame::SourceZUp,
    )
    .expect("authored rig")
}

#[test]
fn the_torso_root_moves_under_the_hips_without_moving_anything() {
    let source = authored();
    let before = worlds(&source);
    let (moved, report) = follow_hips(&source).expect("the anchors are present");
    assert_eq!(report.moved.as_deref(), Some("NPC Spine [Spn0]"));
    assert_eq!(report.parent.as_deref(), Some("NPC Pelvis [Pelv]"));
    assert_eq!(report.previous.as_deref(), Some("NPC COM [COM ]"));
    assert_eq!(report.note, "torso now rooted under the hips");
    let spine = moved.joint_by_name("NPC Spine [Spn0]").expect("torso root");
    assert_eq!(
        moved.joint(spine).expect("torso root").parent,
        moved.joint_by_name("NPC Pelvis [Pelv]").ok()
    );
    assert_eq!(worlds(&moved), before, "every rest world is unchanged");
    // The remaining children of the root offset stay siblings of the hips.
    let pelvis = moved
        .joint_by_name("NPC Pelvis [Pelv]")
        .expect("hips joint");
    assert_eq!(
        moved.joint(pelvis).expect("hips").parent,
        moved.joint_by_name("NPC COM [COM ]").ok()
    );
}

#[test]
fn a_mirrored_palette_moves_its_own_upper_body_mirror() {
    // Mike's shape: the `CME` mirror family roots the upper body, and the hips
    // carry a `CME` mirror of their own one level down.
    let source = Rig::new(
        vec![
            joint("Scene Root", None, place(0.0, 0.0)),
            joint("NPC COM [COM ]", Some(JointId::new(0)), place(0.0, 0.0)),
            joint("CME Body [Body]", Some(JointId::new(1)), place(0.0, 0.0)),
            joint("CME LBody [LBody]", Some(JointId::new(2)), place(0.0, 0.0)),
            joint("CME UBody [UBody]", Some(JointId::new(2)), place(0.02, 0.0)),
            joint("NPC Pelvis [Pelv]", Some(JointId::new(3)), place(0.0, 0.0)),
            joint("CME Pelvis [Pelv]", Some(JointId::new(5)), place(0.0, 0.0)),
            joint("NPC Spine [Spn0]", Some(JointId::new(4)), place(0.0, 0.0)),
        ],
        RigFrame::SourceZUp,
    )
    .expect("mirrored rig");
    let before = worlds(&source);
    let (moved, report) = follow_hips(&source).expect("the anchors are present");
    assert_eq!(report.moved.as_deref(), Some("CME UBody [UBody]"));
    assert_eq!(
        report.parent.as_deref(),
        Some("CME Pelvis [Pelv]"),
        "the mirror family keeps its own hips, so the two families stay mirrors"
    );
    assert_eq!(worlds(&moved), before);
}

#[test]
fn a_palette_without_the_anchors_keeps_its_authored_tree() {
    let source = Rig::new(
        vec![
            joint("mixamorig:Hips", None, place(0.0, 0.0)),
            joint("mixamorig:Spine", Some(JointId::new(0)), place(0.1, 0.0)),
        ],
        RigFrame::YUp,
    )
    .expect("mixamo rig");
    let (moved, report) = follow_hips(&source).expect("a foreign palette is not an error");
    assert_eq!(report.moved, None);
    assert_eq!(report.note, "palette carries no hips/torso anchors");
    assert_eq!(worlds(&moved), worlds(&source));
}

#[test]
fn an_already_following_palette_is_left_alone() {
    let source = authored();
    let (once, _) = follow_hips(&source).expect("the first move succeeds");
    let (twice, report) = follow_hips(&once).expect("a second run is a no-op");
    assert_eq!(report.moved, None);
    assert_eq!(report.note, "already follows the hips");
    assert_eq!(worlds(&twice), worlds(&once));
}
