//! The multi-part rigged writer: one skin, one primitive and one review-palette
//! material per partition, re-read through the `gltf` crate. @req DECL0008

use glam::DMat4;
use gltf::Gltf;
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::review::{BODY_COLOR, hex, palette};
use sculpto_gltf::{GltfError, PartMaterial, write_partitioned_rigged_glb};

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
            joint("Spine", Some(0), [0.0, 0.0, 2.0]),
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

/// Two triangles in two partitions, each bound at its own rest.
fn asset(rig: &Rig) -> MeshAsset {
    let bind = |name: &str| {
        world(rig, name)
            .inverse()
            .to_cols_array_2d()
            .map(|column| column.map(|value| value as f32))
    };
    MeshAsset {
        name: "Body".into(),
        positions: vec![
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 2.0],
            [1.0, 0.0, 2.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        triangle_partitions: vec![PartitionId(0), PartitionId(1)],
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: vec![bind("Root"), bind("Spine")],
            influences: SkinWeights::from_quads(
                &[[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0], [1, 1, 0, 0]],
                &[
                    [1.0, 0.0, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                ],
            )
            .expect("four-lane rows"),
        }),
        ..Default::default()
    }
}

fn identity() -> [[f32; 4]; 4] {
    DMat4::IDENTITY
        .to_cols_array_2d()
        .map(|column| column.map(|value| value as f32))
}

fn binding<'a>(rig: &'a Rig, asset: &'a MeshAsset) -> RestBinding<'a> {
    RestBinding::prove(
        rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape: 0,
            root: JointId::new(0),
            positions: &asset.positions,
            skin: asset.skin.as_ref().expect("the fixture is skinned"),
            global_to_skin: identity(),
        }],
    )
    .expect("the fixture reconstructs its rest")
}

#[test]
fn each_partition_carries_its_own_palette_material() {
    let rig = rig();
    let asset = asset(&rig);
    let binding = binding(&rig, &asset);
    let parts = [
        PartMaterial {
            partition: 0,
            name: "body".into(),
            color: BODY_COLOR,
        },
        PartMaterial {
            partition: 1,
            name: "bra".into(),
            color: palette(1).expect("rank 1 has a colour"),
        },
    ];
    let bytes = write_partitioned_rigged_glb(&asset, &binding, 0, &[], &parts)
        .expect("a checked part-coloured actor writes");
    let parsed = Gltf::from_slice(&bytes).expect("the writer output parses");

    assert_eq!(
        parsed.document.materials().count(),
        2,
        "one material per part"
    );
    let names: Vec<&str> = parsed
        .document
        .materials()
        .map(|material| material.name().expect("a named part material"))
        .collect();
    assert_eq!(names, vec!["body", "bra"]);
    let colours: Vec<String> = parsed
        .document
        .materials()
        .map(|material| {
            let colour = material
                .pbr_metallic_roughness()
                .base_color_factor()
                .map(|lane| (lane * 255.0).round() as u8);
            format!("#{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2])
        })
        .collect();
    assert_eq!(colours, vec![hex(BODY_COLOR), "#2ECC71".to_string()]);
    assert!(
        parsed
            .document
            .materials()
            .all(|material| material.double_sided()),
        "a review material is double sided"
    );

    assert_eq!(
        parsed.document.skins().count(),
        1,
        "one palette for the actor"
    );
    let mesh = parsed
        .document
        .meshes()
        .next()
        .expect("the actor carries one mesh");
    let primitives: Vec<u16> = mesh
        .primitives()
        .map(|primitive| {
            primitive
                .material()
                .index()
                .expect("every part primitive is coloured") as u16
        })
        .collect();
    assert_eq!(
        primitives,
        vec![0, 1],
        "one primitive per partition, in order"
    );
}

#[test]
fn a_partition_without_a_material_is_refused() {
    let rig = rig();
    let asset = asset(&rig);
    let binding = binding(&rig, &asset);
    let parts = [PartMaterial {
        partition: 0,
        name: "body".into(),
        color: BODY_COLOR,
    }];
    let refused = write_partitioned_rigged_glb(&asset, &binding, 0, &[], &parts)
        .expect_err("an uncoloured partition refuses");
    assert_eq!(
        refused,
        GltfError::PartMaterialMissing { partition: 1 },
        "{refused}"
    );

    let duplicated = [
        PartMaterial {
            partition: 0,
            name: "body".into(),
            color: BODY_COLOR,
        },
        PartMaterial {
            partition: 0,
            name: "again".into(),
            color: BODY_COLOR,
        },
    ];
    let refused = write_partitioned_rigged_glb(&asset, &binding, 0, &[], &duplicated)
        .expect_err("a repeated partition refuses");
    assert_eq!(refused, GltfError::PartMaterialDuplicate { partition: 0 });
}

#[test]
fn a_mesh_with_material_slots_cannot_also_declare_parts() {
    let rig = rig();
    let mut asset = asset(&rig);
    asset.materials = vec![sculpto_core::mesh::MaterialSlot {
        name: "authored".into(),
        ..Default::default()
    }];
    let binding = binding(&rig, &asset);
    let parts = [
        PartMaterial {
            partition: 0,
            name: "body".into(),
            color: BODY_COLOR,
        },
        PartMaterial {
            partition: 1,
            name: "bra".into(),
            color: BODY_COLOR,
        },
    ];
    let refused = write_partitioned_rigged_glb(&asset, &binding, 0, &[], &parts)
        .expect_err("two colour routes refuse");
    assert_eq!(
        refused,
        GltfError::PartMaterialConflict { count: 1 },
        "{refused}"
    );
}
