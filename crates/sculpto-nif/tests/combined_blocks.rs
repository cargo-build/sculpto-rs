//! Combined-block layout: skin, material and the physics link in one file.
//!
//! Every block after the geometry moves once a skin exists, and the
//! `next_block` advance past the texture block under a skin
//! (`write/se_shape.rs:102-125`, `write/le_shape.rs:68-91`) had only been
//! traced, never asserted end-to-end. One fixture — two bones, two triangle
//! partitions, one diffuse+normal material and a `PhysicsRef::Path` sidecar —
//! is written per target and parsed with byroredux: every block type sits at
//! its expected index (SE root 4, LE root 5, then bones, shader, texture set,
//! extra data), the shape's skin/shader references resolve to the
//! `NiSkinInstance` and the `BSLightingShaderProperty`, the instance points at
//! the `NiSkinData` and `NiSkinPartition`, the root `NiNode` carries exactly
//! one `NiStringExtraData` naming the physics path, and the texture paths
//! match the model.

use byroredux_nif::blocks::extra_data::NiExtraData;
use byroredux_nif::blocks::node::NiNode;
use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet};
use byroredux_nif::blocks::skin::{NiSkinData, NiSkinInstance, NiSkinPartition};
use byroredux_nif::blocks::traits::HasObjectNET;
use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape};
use byroredux_nif::parse_nif;
use byroredux_nif::scene::NifScene;
use byroredux_nif::types::BlockRef;
use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, PhysicsRef, ShaderKind, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::write::extra::{BLOCK_TYPE, HDT_PHYSICS_NAME};
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::write::skin::identity;

const PHYSICS_PATH: &str = "SKSE\\Plugins\\hdtSkinnedMeshConfigs\\robe.xml";

/// Skinned, textured and physics-linked: every writer branch at once.
fn robe() -> MeshAsset {
    let mut asset = MeshAsset {
        name: "Robe".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [1.5, 2.0, 0.0],
        ],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        triangle_partitions: vec![PartitionId(0), PartitionId(1)],
        materials: vec![MaterialSlot {
            name: "robe".into(),
            shader: ShaderKind::Cloth,
            diffuse: Some("textures/robe_d.dds".into()),
            normal: Some("textures/robe_n.dds".into()),
            specular: None,
            double_sided: true,
        }],
        physics: Some(PhysicsRef::Path(PHYSICS_PATH.into())),
        ..MeshAsset::default()
    };
    asset.skin = Some(Skin {
        bones: vec!["Root".into(), "Spine".into()],
        bind_inverse: vec![identity(), identity()],
        influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 4], &[[0.75, 0.25, 0.0, 0.0]; 4])
            .expect("four-lane rows"),
    });
    asset
}

/// Expected block indices for one target's combined file.
struct Layout {
    /// Block type at every index, file root and bone `NiNode`s included; the
    /// link block downcasts as `NiExtraData`, so its entry is that registry
    /// name and the wire spelling is asserted separately.
    types: &'static [&'static str],
    /// File root `NiNode`, added when the shape is skinned.
    root: usize,
    skin_instance: usize,
    skin_data: usize,
    skin_partition: usize,
    /// Bone `NiNode`s in skin order.
    bones: [usize; 2],
    shader: usize,
    textures: usize,
    extra: usize,
}

/// SE: shape, instance, data, partition, root, bones 5-6, shader, textures, link.
const SE: Layout = Layout {
    types: &[
        "BSTriShape",
        "NiSkinInstance",
        "NiSkinData",
        "NiSkinPartition",
        "NiNode",
        "NiNode",
        "NiNode",
        "BSLightingShaderProperty",
        "BSShaderTextureSet",
        "NiExtraData",
    ],
    root: 4,
    skin_instance: 1,
    skin_data: 2,
    skin_partition: 3,
    bones: [5, 6],
    shader: 7,
    textures: 8,
    extra: 9,
};

/// LE inserts the shape's `NiTriShapeData` at index 1 and shifts the rest.
const LE: Layout = Layout {
    types: &[
        "NiTriShape",
        "NiTriShapeData",
        "NiSkinInstance",
        "NiSkinData",
        "NiSkinPartition",
        "NiNode",
        "NiNode",
        "NiNode",
        "BSLightingShaderProperty",
        "BSShaderTextureSet",
        "NiExtraData",
    ],
    root: 5,
    skin_instance: 2,
    skin_data: 3,
    skin_partition: 4,
    bones: [6, 7],
    shader: 8,
    textures: 9,
    extra: 10,
};

/// Assert the combined file's wiring on the parsed scene.
fn assert_combined(scene: &NifScene, layout: &Layout) {
    assert_eq!(scene.len(), layout.types.len(), "block count");
    for (index, expected) in layout.types.iter().enumerate() {
        let block = scene.get(index).expect("block present");
        assert_eq!(block.block_type_name(), *expected, "block type at {index}");
    }
    assert_eq!(
        scene.root_index,
        Some(layout.root),
        "skinned file root is the added node"
    );

    let instance = scene
        .get_as::<NiSkinInstance>(layout.skin_instance)
        .expect("skin instance block");
    assert_eq!(
        instance.data_ref.index(),
        Some(layout.skin_data),
        "instance → skin data"
    );
    assert_eq!(
        instance.skin_partition_ref.index(),
        Some(layout.skin_partition),
        "instance → skin partition"
    );
    assert_eq!(
        instance.skeleton_root_ref.index(),
        Some(layout.root),
        "the skin names the file's root node as its skeleton root"
    );
    let bone_names: Vec<Option<&str>> = instance
        .bone_refs
        .iter()
        .map(|bone| {
            bone.index()
                .and_then(|index| scene.get_as::<NiNode>(index))
                .and_then(|node| node.name())
        })
        .collect();
    assert_eq!(bone_names, vec![Some("Root"), Some("Spine")]);
    assert_eq!(
        instance
            .bone_refs
            .iter()
            .filter_map(|bone| bone.index())
            .collect::<Vec<_>>(),
        layout.bones.to_vec(),
        "the instance references the bone nodes in palette order"
    );

    let data = scene
        .get_as::<NiSkinData>(layout.skin_data)
        .expect("skin data block");
    assert_eq!(data.bones.len(), 2, "one bind entry per bone");

    let partition = scene
        .get_as::<NiSkinPartition>(layout.skin_partition)
        .expect("skin partition block");
    assert_eq!(partition.partitions.len(), 2, "two triangle partitions");
    for entry in &partition.partitions {
        assert_eq!(entry.bones, vec![0, 1], "each palette covers both bones");
    }

    let shader = scene
        .get_as::<BSLightingShaderProperty>(layout.shader)
        .expect("shader block");
    assert_eq!(
        shader.texture_set_ref.index(),
        Some(layout.textures),
        "shader → texture set"
    );
    let textures = scene
        .get_as::<BSShaderTextureSet>(layout.textures)
        .expect("texture set block");
    assert_eq!(
        textures.textures,
        vec!["textures/robe_d.dds", "textures/robe_n.dds"]
    );

    let root = scene
        .get_as::<NiNode>(layout.root)
        .expect("root node block");
    assert_eq!(
        root.av.net.extra_data_refs.len(),
        1,
        "exactly one link on the root"
    );
    assert_eq!(
        root.av.net.extra_data_refs[0].index(),
        Some(layout.extra),
        "root → extra block"
    );
    let link = scene
        .get_as::<NiExtraData>(layout.extra)
        .expect("extra data block");
    assert_eq!(link.type_name, BLOCK_TYPE, "the link block spelling");
    assert_eq!(link.name.as_deref(), Some(HDT_PHYSICS_NAME));
    assert_eq!(link.string_value.as_deref(), Some(PHYSICS_PATH));
    let extras = (0..scene.len())
        .filter(|index| scene.get_as::<NiExtraData>(*index).is_some())
        .count();
    assert_eq!(extras, 1, "the whole file carries one extra block");
}

/// The shape references shared by both targets: skin and shader property.
fn assert_shape_refs(shape_skin: BlockRef, shape_shader: BlockRef, layout: &Layout) {
    assert_eq!(
        shape_skin.index(),
        Some(layout.skin_instance),
        "shape → skin instance"
    );
    assert_eq!(
        shape_shader.index(),
        Some(layout.shader),
        "shape → shader property"
    );
}

#[test]
fn se_skin_material_and_physics_keep_their_block_indices() {
    let bytes = write_se_shape(&robe()).expect("SE writes");
    let scene = parse_nif(&bytes).expect("parses");
    let shape = scene.get_as::<BsTriShape>(0).expect("SE shape block");
    assert_shape_refs(shape.skin_ref, shape.shader_property_ref, &SE);
    assert!(
        shape.av.net.extra_data_refs.is_empty(),
        "with a skin the link moves off the shape"
    );
    assert_combined(&scene, &SE);
}

#[test]
fn le_skin_material_and_physics_keep_their_block_indices() {
    let bytes = write_le_shape(&robe()).expect("LE writes");
    let scene = parse_nif(&bytes).expect("parses");
    let shape = scene.get_as::<NiTriShape>(0).expect("LE shape block");
    assert_eq!(shape.data_ref.index(), Some(1), "shape → geometry data");
    assert_shape_refs(shape.skin_instance_ref, shape.shader_property_ref, &LE);
    assert!(
        shape.av.net.extra_data_refs.is_empty(),
        "with a skin the link moves off the shape"
    );
    assert_combined(&scene, &LE);
}
