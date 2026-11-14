//! The dismember partition table and the authored texture-set slots survive a
//! write and a read, on both Skyrim targets.
//!
//! The host-gated BLD-09 measurement (`nif_projection.rs`) runs the same
//! contract on a real garment; this test pins it on a synthetic model, so the
//! portable gate catches a writer or reader that loses either property.

use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet};
use byroredux_nif::blocks::skin::{BsDismemberSkinInstance, NiSkinInstance, NiSkinPartition};
use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape};
use byroredux_nif::parse_nif;
use byroredux_nif::scene::NifScene;
use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, PartitionSlot, ShaderKind, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::read;
use sculpto_nif::write::header::NifTarget;
use sculpto_nif::write::le_bundle::write_le_bundle;
use sculpto_nif::write::se_bundle::write_se_bundle;

/// The authored texture-set slot list: diffuse, normal, the source's tint slot
/// (which the neutral roles do not name), an unassigned slot, and an
/// environment cube.
fn texture_set() -> Vec<String> {
    vec![
        "textures/clothes/imperialmage/robe2.dds".into(),
        "textures/clothes/magejourneyman/m/robe_n.dds".into(),
        "textures/actors/character/male/MaleBody_1_sk.dds".into(),
        String::new(),
        "textures/cubemaps/chitin_e_ebony.dds".into(),
    ]
}

/// A two-partition skinned shape with one authored material.
fn authored() -> MeshAsset {
    MeshAsset {
        name: "robes".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 2.0, 0.0],
        ],
        triangles: vec![[0, 1, 2], [1, 3, 2], [2, 3, 4]],
        triangle_partitions: vec![PartitionId(0), PartitionId(0), PartitionId(1)],
        partitions: vec![
            PartitionSlot {
                partition: PartitionId(0),
                part_flag: 257,
                body_part: 32,
            },
            PartitionSlot {
                partition: PartitionId(1),
                part_flag: 257,
                body_part: 38,
            },
        ],
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into(), "Pelvis".into()],
            bind_inverse: Vec::new(),
            influences: SkinWeights::from_quads(
                &[
                    [0, 0, 0, 0],
                    [1, 0, 0, 0],
                    [2, 0, 0, 0],
                    [1, 2, 0, 0],
                    [0, 1, 0, 0],
                ],
                &[
                    [1.0, 0.0, 0.0, 0.0],
                    [0.6, 0.4, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                ],
            )
            .expect("four-lane rows"),
        }),
        materials: vec![MaterialSlot {
            name: "robes".into(),
            shader: ShaderKind::Cloth,
            double_sided: true,
            texture_set: texture_set(),
            ..MaterialSlot::default()
        }],
        ..MeshAsset::default()
    }
}

fn write(shapes: &[MeshAsset], target: NifTarget) -> Vec<u8> {
    match target {
        NifTarget::Se => write_se_bundle(shapes),
        NifTarget::Le => write_le_bundle(shapes),
    }
    .expect("the authored model writes")
}

/// The parsed written file.
fn parsed(bytes: &[u8]) -> NifScene {
    let scene = parse_nif(bytes).expect("our writer's bytes parse");
    assert!(!scene.truncated, "the written file parses completely");
    scene
}

/// SE writes `BSTriShape`: its block index.
fn se_shape(scene: &NifScene) -> usize {
    (0..scene.len())
        .find(|index| scene.get_as::<BsTriShape>(*index).is_some())
        .expect("one BSTriShape block")
}

/// LE writes `NiTriShape`: its block index.
fn le_shape(scene: &NifScene) -> usize {
    (0..scene.len())
        .find(|index| scene.get_as::<NiTriShape>(*index).is_some())
        .expect("one NiTriShape block")
}

#[test]
fn the_dismember_table_and_texture_slots_roundtrip_on_se() {
    let bytes = write(&[authored()], NifTarget::Se);
    let scene = parsed(&bytes);
    let shape = scene.get_as::<BsTriShape>(se_shape(&scene)).expect("shape");
    let instance = scene
        .get_as::<BsDismemberSkinInstance>(shape.skin_ref.index().expect("skinned"))
        .expect("the shape writes the dismember subclass");
    assert_eq!(
        instance
            .partitions
            .iter()
            .map(|slot| (slot.part_flag, slot.body_part))
            .collect::<Vec<_>>(),
        vec![(257, 32), (257, 38)],
    );
    let partition = scene
        .get_as::<NiSkinPartition>(instance.base.skin_partition_ref.index().expect("partition"));
    let partition = partition.expect("partition block");
    assert_eq!(
        partition
            .partitions
            .iter()
            .map(|entry| (entry.triangles.len(), entry.vertex_map.len()))
            .collect::<Vec<_>>(),
        vec![(2, 4), (1, 3)],
        "one entry per authored slot, in slot order",
    );

    let shader = scene
        .get_as::<BSLightingShaderProperty>(shape.shader_property_ref.index().expect("shader"))
        .expect("shader block");
    assert_eq!(shader.net.name, None, "the shader block stays unnamed");
    assert_ne!(
        shader.shader_flags_2 & 0x10,
        0,
        "the authored double-sided bit is written"
    );
    let textures = scene
        .get_as::<BSShaderTextureSet>(shader.texture_set_ref.index().expect("texture set"))
        .expect("texture set block");
    assert_eq!(textures.textures, texture_set());

    // The same facts through the reader's projection.
    let document = read(&bytes).expect("the re-encode reads");
    let shape = &document.shapes[0];
    assert_eq!(
        shape.triangle_partitions,
        vec![PartitionId(0), PartitionId(0), PartitionId(1)]
    );
    assert_eq!(
        shape
            .partitions
            .iter()
            .map(|slot| (slot.partition, slot.part_flag, slot.body_part))
            .collect::<Vec<_>>(),
        vec![(PartitionId(0), 257, 32), (PartitionId(1), 257, 38)]
    );
    assert_eq!(shape.materials[0].texture_set, texture_set());
    let mesh = MeshAsset::from(shape);
    mesh.validate().expect("the round-tripped model validates");
    assert_eq!(mesh.skin.as_ref().expect("skin").bones.len(), 3);
}

#[test]
fn the_dismember_table_and_texture_slots_roundtrip_on_le() {
    let bytes = write(&[authored()], NifTarget::Le);
    let scene = parsed(&bytes);
    let shape = &scene.get_as::<NiTriShape>(le_shape(&scene)).expect("shape");
    let instance = scene
        .get_as::<BsDismemberSkinInstance>(shape.skin_instance_ref.index().expect("skinned"))
        .expect("the shape writes the dismember subclass");
    assert_eq!(
        instance
            .partitions
            .iter()
            .map(|slot| (slot.part_flag, slot.body_part))
            .collect::<Vec<_>>(),
        vec![(257, 32), (257, 38)],
    );
    let textures = scene
        .get_as::<BSLightingShaderProperty>(shape.shader_property_ref.index().expect("shader"))
        .and_then(|shader| shader.texture_set_ref.index())
        .and_then(|index| scene.get_as::<BSShaderTextureSet>(index))
        .expect("texture set block");
    assert_eq!(textures.textures, texture_set());
}

#[test]
fn a_shape_without_authored_slots_stays_a_plain_instance() {
    let mut plain = authored();
    plain.partitions.clear();
    plain.triangle_partitions = vec![PartitionId(0); plain.triangles.len()];
    plain.name = "plain".into();
    let bytes = write(&[plain], NifTarget::Se);
    let scene = parsed(&bytes);
    let index = se_shape(&scene);
    let skin_ref = scene.get_as::<BsTriShape>(index).expect("shape").skin_ref;
    let index = skin_ref.index().expect("skinned");
    assert!(
        scene.get_as::<BsDismemberSkinInstance>(index).is_none(),
        "no dismember table is invented"
    );
    assert!(scene.get_as::<NiSkinInstance>(index).is_some());
    assert_eq!(
        read(&bytes).expect("reads").shapes[0].partitions,
        Vec::new()
    );
}
