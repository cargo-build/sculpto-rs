use std::path::Path;

use sculpto_core::mesh::Skin;
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigError, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_mesh::rig::map::{BodyFamily, BoneMap, BoneMapError, Collapse, Destination};
use sculpto_nif::{NifDocument, read};

const ROOT: JointId = JointId::new(0);
const ARM: JointId = JointId::new(1);
const AUX: JointId = JointId::new(2);
const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn rig(names: &[&str], frame: RigFrame) -> Rig {
    let joints = names
        .iter()
        .enumerate()
        .map(|(index, name)| Joint {
            name: Some((*name).into()),
            parent: (index > 0).then_some(ROOT),
            rest: LocalTransform::default(),
        })
        .collect();
    Rig::new(joints, frame).expect("a valid joint graph")
}

fn with_binding(test: impl FnOnce(&RestBinding<'_>)) {
    let source = rig(&["Root", "Arm", "Aux", "Unused"], RigFrame::SourceZUp);
    let positions = [[1.0, 2.0, 3.0]];
    let skin = Skin {
        bones: vec!["Arm".into(), "Aux".into(), "Unused".into()],
        bind_inverse: vec![IDENTITY; 3],
        influences: SkinWeights::new(vec![0, 3], vec![0, 1, 2], vec![0.7, 0.3, 0.0], 3)
            .expect("one source row"),
    };
    let binding = RestBinding::prove(
        &source,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: 0,
            root: ROOT,
            positions: &positions,
            skin: &skin,
            global_to_skin: IDENTITY,
        }],
    )
    .expect("checked source");
    test(&binding);
}

#[test]
fn maps_exact_ids_and_reports_positive_source_coverage() {
    with_binding(|source| {
        let target = rig(&["Root", "Torso", "Arm"], RigFrame::SourceZUp);
        let map = BoneMap::build(BodyFamily::Himbo, source, BodyFamily::Cbbe, &target, &[])
            .expect("the supported pair");
        assert!(std::ptr::eq(map.source(), source.rig()));
        assert!(std::ptr::eq(map.target(), &target));
        assert_eq!(map.entries().len(), 4);
        assert_eq!(
            map.entries()[ARM.index()].destination,
            Destination::Direct(AUX)
        );
        assert_eq!(map.resolve(ARM), Ok(AUX));
        let weighted: Vec<_> = map
            .entries()
            .iter()
            .filter(|entry| entry.weighted)
            .map(|entry| entry.source)
            .collect();
        assert_eq!(weighted, [ARM, AUX]);
        assert_eq!(
            map.resolve(AUX),
            Err(BoneMapError::Unsupported {
                joint: AUX,
                name: Some("Aux".into()),
            })
        );
        let outside = JointId::new(80);
        assert_eq!(
            map.resolve(outside),
            Err(BoneMapError::UnknownSource(outside))
        );
    });
}

#[test]
fn only_an_explicit_collapse_resolves_a_missing_name() {
    with_binding(|source| {
        let target = rig(&["Root", "Arm"], RigFrame::SourceZUp);
        let declaration = Collapse {
            source: "Aux",
            target: "Arm",
        };
        let make = |collapses: &[Collapse<'_>]| {
            BoneMap::build(
                BodyFamily::ThreeBa,
                source,
                BodyFamily::Himbo,
                &target,
                collapses,
            )
        };
        let map = make(&[declaration]).expect("explicit collapse");
        assert_eq!(
            map.entries()[AUX.index()].destination,
            Destination::Collapsed(ARM)
        );
        assert_eq!(map.resolve(AUX), Ok(ARM));
        assert!(matches!(
            make(&[declaration, declaration]),
            Err(BoneMapError::DuplicateCollapse { joint: AUX, .. })
        ));
        assert!(matches!(
            make(&[Collapse {
                source: "Arm",
                target: "Root"
            }]),
            Err(BoneMapError::DirectCollapse { joint: ARM, .. })
        ));
        assert!(
            matches!(make(&[Collapse { source: "Aux", target: "Unknown" }]),
            Err(BoneMapError::NamedJoint { side: "target", error: RigError::UnknownJointName { name } }) if name == "Unknown")
        );
    });
}

#[test]
fn refuses_ambiguous_joints_other_pairs_and_mixed_frames() {
    with_binding(|source| {
        let target = rig(&["Root", "Arm", "Arm"], RigFrame::SourceZUp);
        assert!(matches!(
            BoneMap::build(BodyFamily::Himbo, source, BodyFamily::Cbbe, &target, &[]),
            Err(BoneMapError::NamedJoint { side: "target", error: RigError::AmbiguousJointName { name, .. } }) if name == "Arm"
        ));
        assert!(matches!(
            BoneMap::build(BodyFamily::Cbbe, source, BodyFamily::ThreeBa, &target, &[]),
            Err(BoneMapError::Families { .. })
        ));
        let target = rig(&["Root", "Arm"], RigFrame::YUp);
        assert!(matches!(
            BoneMap::build(BodyFamily::Himbo, source, BodyFamily::Cbbe, &target, &[]),
            Err(BoneMapError::Frame { .. })
        ));
    });
}

fn complete(root: &Path, body: &str, template: &str) -> (NifDocument, Rig) {
    let body = read(&std::fs::read(root.join(body)).expect("configured body")).expect("body NIF");
    let template = read(&std::fs::read(root.join(template)).expect("configured template"))
        .expect("template NIF");
    let authored = body.rig.as_ref().expect("source graph");
    let template = template.rig.as_ref().expect("template graph");
    let completed = complete_family_rig(
        authored,
        template,
        TemplateRoots {
            authored: authored.roots()[0],
            template: template.roots()[0],
        },
    )
    .expect("completed family hierarchy");
    (body, completed)
}

#[test]
fn configured_body_family_maps_measure_every_weighted_joint() {
    let Some(root) = std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES") else {
        eprintln!("SCULPTO_RIG_TEMPLATE_FIXTURES not set; skipping real body-family map");
        return;
    };
    let root = Path::new(&root);
    let (himbo, male) = complete(root, "himbo.nif", "skeleton_male_sse.nif");
    let (bijin, female) = complete(root, "bijin.nif", "skeleton_female_sse.nif");
    let (three_ba, extended) = complete(root, "3ba.nif", "skeleton_female_sse.nif");
    let himbo = himbo.bind_rest_on(&male).expect("HIMBO rest");
    let bijin = bijin.bind_rest_on(&female).expect("Bijin rest");
    let three_ba = three_ba.bind_rest_on(&extended).expect("3BA rest");
    use BodyFamily::{Cbbe, Himbo, ThreeBa};
    for (origin, binding, family, target, expected) in [
        (Himbo, &himbo, Cbbe, &female, (24, 0)),
        (Himbo, &himbo, ThreeBa, &extended, (24, 0)),
        (Cbbe, &bijin, Himbo, &male, (22, 0)),
        (ThreeBa, &three_ba, Himbo, &male, (35, 16)),
    ] {
        let map = BoneMap::build(origin, binding, family, target, &[]).expect("body map");
        let mut coverage = (0, 0);
        for entry in map.entries().iter().filter(|entry| entry.weighted) {
            let name = map
                .source()
                .joint(entry.source)
                .expect("source joint")
                .name
                .as_deref()
                .expect("weighted joint name");
            match target.joint_by_name(name) {
                Ok(id) => {
                    assert_eq!(entry.destination, Destination::Direct(id));
                    assert_eq!(map.resolve(entry.source), Ok(id));
                    coverage.0 += 1;
                }
                Err(RigError::UnknownJointName { .. }) => {
                    assert_eq!(entry.destination, Destination::Unsupported);
                    assert_eq!(
                        map.resolve(entry.source),
                        Err(BoneMapError::Unsupported {
                            joint: entry.source,
                            name: Some(name.into()),
                        })
                    );
                    coverage.1 += 1;
                }
                Err(error) => panic!("{error}"),
            }
        }
        assert_eq!(coverage, expected);
        eprintln!(
            "{origin:?} -> {family:?}: {} direct, {} unsupported",
            coverage.0, coverage.1
        );
    }
}
