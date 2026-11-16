//! The explicit-resolution entry into the bone map checks what the remap kernel
//! relies on, without the body-family gate. @req DECL0008

use sculpto_core::mesh::Skin;
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_mesh::rig::map::{BoneMap, BoneMapError, Destination};

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn chain(names: &[&str], frame: RigFrame) -> Rig {
    let joints = names
        .iter()
        .enumerate()
        .map(|(index, name)| Joint {
            name: Some((*name).into()),
            parent: (index > 0).then_some(JointId::new(0)),
            rest: LocalTransform::default(),
        })
        .collect();
    Rig::new(joints, frame).expect("a valid joint graph")
}

fn binding<'a>(rig: &'a Rig, positions: &'a [[f32; 3]], skin: &'a Skin) -> RestBinding<'a> {
    RestBinding::prove(
        rig,
        rig.frame(),
        &[SkinRestSource {
            shape: 0,
            root: rig.roots()[0],
            positions,
            skin,
            global_to_skin: IDENTITY,
        }],
    )
    .expect("a checked source binding")
}

fn source_skin() -> Skin {
    Skin {
        bones: vec!["root".into(), "spine".into(), "breast".into()],
        bind_inverse: vec![IDENTITY; 3],
        influences: SkinWeights::new(vec![0, 2, 3], vec![0, 1, 2], vec![0.6, 0.4, 1.0], 3)
            .expect("source rows"),
    }
}

#[test]
fn a_checked_table_resolves_without_the_family_gate() {
    let source = chain(&["root", "spine", "breast"], RigFrame::YUp);
    let target = chain(&["hips", "spine"], RigFrame::YUp);
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let skin = source_skin();
    let bound = binding(&source, &positions, &skin);
    let table = [
        Destination::Direct(JointId::new(0)),
        Destination::Direct(JointId::new(1)),
        Destination::Collapsed(JointId::new(1)),
    ];
    let map = BoneMap::resolved(&bound, &target, &table).expect("a checked resolution");
    assert_eq!(map.entries().len(), 3);
    assert_eq!(
        map.entries()[0].destination,
        Destination::Direct(JointId::new(0))
    );
    assert_eq!(
        map.entries()[2].destination,
        Destination::Collapsed(JointId::new(1))
    );
    // The breast carries weight; the root and spine are weighted too.
    assert!(map.entries()[2].weighted);
    assert_eq!(
        map.resolve(JointId::new(2)).expect("folded bone resolves"),
        JointId::new(1)
    );
}

#[test]
fn a_table_of_the_wrong_length_refuses() {
    let source = chain(&["root", "spine", "breast"], RigFrame::YUp);
    let target = chain(&["hips", "spine"], RigFrame::YUp);
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let skin = source_skin();
    let bound = binding(&source, &positions, &skin);
    let error = BoneMap::resolved(&bound, &target, &[Destination::Unsupported; 2])
        .expect_err("two entries for three joints");
    assert_eq!(
        error,
        BoneMapError::DestinationCount {
            expected: 3,
            got: 2
        }
    );
}

#[test]
fn a_destination_outside_the_target_refuses() {
    let source = chain(&["root", "spine", "breast"], RigFrame::YUp);
    let target = chain(&["hips", "spine"], RigFrame::YUp);
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let skin = source_skin();
    let bound = binding(&source, &positions, &skin);
    let table = [
        Destination::Direct(JointId::new(0)),
        Destination::Direct(JointId::new(9)),
        Destination::Unsupported,
    ];
    let error = BoneMap::resolved(&bound, &target, &table).expect_err("destination 9 is outside");
    assert!(matches!(error, BoneMapError::DestinationOutOfRange { .. }));
}

#[test]
fn mixed_frames_refuse() {
    let source = chain(&["root", "spine", "breast"], RigFrame::YUp);
    let target = chain(&["hips", "spine"], RigFrame::SourceZUp);
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let skin = source_skin();
    let bound = binding(&source, &positions, &skin);
    let error = BoneMap::resolved(&bound, &target, &[Destination::Unsupported; 3])
        .expect_err("mixed frames");
    assert!(matches!(error, BoneMapError::Frame { .. }));
}
