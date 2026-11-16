use std::f32::consts::FRAC_1_SQRT_2;

use sculpto_core::rig::template::{TemplateError, TemplateRoots, complete_family_rig, reparent};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};

const ROOT: JointId = JointId::new(0);
const HAND: JointId = JointId::new(1);
const ROOTS: TemplateRoots = TemplateRoots {
    authored: ROOT,
    template: ROOT,
};

fn joint(name: &str, parent: Option<JointId>, rest: LocalTransform) -> Joint {
    Joint {
        name: Some(name.into()),
        parent,
        rest,
    }
}

fn rig(joints: Vec<Joint>) -> Rig {
    Rig::new(joints, RigFrame::SourceZUp).expect("valid source rig")
}

#[test]
fn recovered_hierarchy_preserves_authored_worlds_under_a_rotated_parent() {
    let root = LocalTransform {
        translation: [2.0, 3.0, 0.0],
        ..LocalTransform::default()
    };
    let hand = LocalTransform {
        translation: [3.0, 0.0, 0.0],
        ..LocalTransform::default()
    };
    let charm = LocalTransform {
        translation: [0.0, 0.0, 1.0],
        ..LocalTransform::default()
    };
    let authored = rig(vec![
        joint("body root", None, root),
        joint("hand", Some(ROOT), hand),
        joint("charm", Some(HAND), charm),
    ]);
    let forearm = LocalTransform {
        translation: [1.0, 0.0, 0.0],
        rotation: [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
        scale: 2.0,
    };
    let template = rig(vec![
        joint("family root", None, LocalTransform::default()),
        joint("forearm", Some(ROOT), forearm),
        joint("hand", Some(JointId::new(1)), LocalTransform::default()),
    ]);
    let completed =
        complete_family_rig(&authored, &template, ROOTS).expect("flattened rig completes");
    let forearm_id = completed
        .joint_by_name("forearm")
        .expect("template ancestor");
    assert_eq!(completed.joint_by_name("hand"), Ok(HAND));
    let completed_hand = completed.joint(HAND).expect("authored ID");
    assert_eq!(completed_hand.parent, Some(forearm_id));
    assert_eq!(completed_hand.rest.scale, 0.5);
    assert!((completed_hand.rest.translation[1] + 1.0).abs() < 1e-6);
    assert_eq!(
        completed.joint(forearm_id).expect("added ancestor").rest,
        forearm
    );
    assert_eq!(completed.joint(ROOT), authored.joint(ROOT));
    assert_eq!(
        completed.joint(JointId::new(2)),
        authored.joint(JointId::new(2))
    );
    for &id in authored.topological_order() {
        let before = authored.rest_world(id).expect("source world");
        let after = completed.rest_world(id).expect("completed world");
        assert!(
            before
                .iter()
                .flatten()
                .zip(after.iter().flatten())
                .all(|(before, after)| (before - after).abs() < 1e-6)
        );
    }
}

#[test]
fn the_template_cannot_replace_an_authored_non_root_parent() {
    let local = LocalTransform::default();
    let source = rig(vec![
        joint("root", None, local),
        joint("old parent", Some(ROOT), local),
        joint("hand", Some(JointId::new(1)), local),
    ]);
    let template = rig(vec![
        joint("root", None, local),
        joint("other parent", Some(ROOT), local),
        joint("hand", Some(JointId::new(1)), local),
    ]);
    assert_eq!(
        complete_family_rig(&source, &template, ROOTS),
        Err(TemplateError::ParentConflict {
            joint: JointId::new(2),
            authored: Some(JointId::new(1)),
            template: JointId::new(3),
        }),
    );
}

/// The palette's shape around the hips: one static root offset, the pelvis and
/// the torso as its two children, and the torso chain above the spine.
fn hips_and_torso() -> Rig {
    let spine = LocalTransform {
        translation: [0.0, 0.0, 0.96],
        ..LocalTransform::default()
    };
    let pelvis = LocalTransform {
        translation: [0.0, 0.0, 0.95],
        ..LocalTransform::default()
    };
    let above = LocalTransform {
        translation: [0.0, 0.05, 0.0],
        rotation: [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
        scale: 1.0,
    };
    rig(vec![
        joint("NPC COM", None, LocalTransform::default()),
        joint("NPC Spine", Some(ROOT), spine),
        joint("NPC Pelvis", Some(ROOT), pelvis),
        joint("NPC Spine1", Some(JointId::new(1)), above),
    ])
}

#[test]
fn reparent_preserves_every_rest_world_and_only_moves_the_link() {
    let source = hips_and_torso();
    let spine = source.joint_by_name("NPC Spine").expect("torso root");
    let pelvis = source.joint_by_name("NPC Pelvis").expect("hips");
    let moved = reparent(&source, spine, pelvis).expect("torso follows the hips");
    assert_eq!(moved.joints().len(), source.joints().len());
    assert_eq!(
        moved.joint(spine).expect("moved joint").parent,
        Some(pelvis)
    );
    for (before, after) in source.joints().iter().zip(moved.joints()) {
        assert_eq!(before.name, after.name, "joint identity is unchanged");
    }
    for &id in source.topological_order() {
        let before = source.rest_world(id).expect("source world");
        let after = moved.rest_world(id).expect("re-parented world");
        let drift = before
            .iter()
            .flatten()
            .zip(after.iter().flatten())
            .map(|(before, after)| (before - after).abs())
            .fold(0.0_f64, f64::max);
        assert!(drift < 1e-9, "joint {id} rest world drift {drift:e}");
    }
    // The moved local is the parent-relative one, not the old offset.
    let local = moved.joint(spine).expect("moved joint").rest;
    assert!((local.translation[2] - 0.01).abs() < 1e-6);
}

#[test]
fn reparent_refuses_a_parent_inside_the_moved_subtree() {
    let source = hips_and_torso();
    let spine = source.joint_by_name("NPC Spine").expect("torso root");
    let above = source.joint_by_name("NPC Spine1").expect("spine child");
    assert_eq!(
        reparent(&source, spine, above),
        Err(TemplateError::ParentCycle {
            joint: spine,
            parent: above,
        })
    );
}
