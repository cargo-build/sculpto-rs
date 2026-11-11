use std::f32::consts::FRAC_1_SQRT_2;

use sculpto_core::rig::template::{TemplateError, TemplateRoots, complete_family_rig};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};

const ROOT: JointId = JointId::new(0);
const HAND: JointId = JointId::new(1);
const ROOTS: TemplateRoots = TemplateRoots { authored: ROOT, template: ROOT };

fn joint(name: &str, parent: Option<JointId>, rest: LocalTransform) -> Joint {
    Joint { name: Some(name.into()), parent, rest }
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
    let completed = complete_family_rig(&authored, &template, ROOTS).expect("flattened rig completes");
    let forearm_id = completed.joint_by_name("forearm").expect("template ancestor");
    assert_eq!(completed.joint_by_name("hand"), Ok(HAND));
    let completed_hand = completed.joint(HAND).expect("authored ID");
    assert_eq!(completed_hand.parent, Some(forearm_id));
    assert_eq!(completed_hand.rest.scale, 0.5);
    assert!((completed_hand.rest.translation[1] + 1.0).abs() < 1e-6);
    assert_eq!(completed.joint(forearm_id).expect("added ancestor").rest, forearm);
    assert_eq!(completed.joint(ROOT), authored.joint(ROOT));
    assert_eq!(completed.joint(JointId::new(2)), authored.joint(JointId::new(2)));
    for &id in authored.topological_order() {
        let before = authored.rest_world(id).expect("source world");
        let after = completed.rest_world(id).expect("completed world");
        assert!(before.iter().flatten().zip(after.iter().flatten())
            .all(|(before, after)| (before - after).abs() < 1e-6));
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
