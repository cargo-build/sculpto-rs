//! Multi-shape SE bundles: round trip, block layout and one shared skeleton.
//!
//! The synthetic tier is procedural: three skinned shapes in one body palette,
//! a different influence pair per shape, distinct vertex counts and only some of
//! them textured. It pins the two things a bundle can get wrong without a
//! reader error — that every shape's geometry, weights and material survive, and
//! that the file has one root, one bone node per bone name, and one partition
//! per shape.
//!
//! The private-host tier writes a real body from `SCULPTO_BIJIN_NIF` next to two
//! synthetic shapes in its own palette and compares every emitted shape's
//! vertex count, triangle count and bone names with the input. It skips without
//! the fixture, so the suite stays portable.

use std::collections::BTreeSet;

use byroredux_nif::blocks::node::NiNode;
use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet};
use byroredux_nif::blocks::skin::{NiSkinInstance, NiSkinPartition};
use byroredux_nif::blocks::tri_shape::BsTriShape;
use byroredux_nif::parse_nif;
use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, ShaderKind, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::read;
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::skin::identity;

/// The palette every shape in the synthetic bundle shares.
const BONES: [&str; 3] = ["Root", "Spine", "Neck"];

/// One skinned shape with `vertices` vertices and a chosen influence row.
///
/// The row picks a different bone pair per shape, so each partition's bone list
/// has to come from that shape's own lanes rather than from the shared palette.
fn shape(
    name: &str,
    base: f32,
    vertices: usize,
    influence: ([u16; 4], [f32; 4]),
    material: Option<MaterialSlot>,
) -> MeshAsset {
    let positions = (0..vertices)
        .map(|vertex| [base + vertex as f32, f32::from(vertex as u8 % 2), 0.0])
        .collect::<Vec<_>>();
    let mut triangles = vec![[0, 1, 2]];
    if vertices > 3 {
        triangles.push([1, 2, 3]);
    }
    let bind_inverse = vec![identity(); BONES.len()];
    let influence_rows = vec![influence.0; vertices];
    let influence_weights = vec![influence.1; vertices];
    MeshAsset {
        name: name.to_string(),
        normals: vec![[0.0, 0.0, 1.0]; vertices],
        uvs: (0..vertices)
            .map(|vertex| [vertex as f32, f32::from(vertex as u8 % 2)])
            .collect(),
        positions,
        triangles,
        triangle_partitions: vec![PartitionId(0); 1 + usize::from(vertices > 3)],
        skin: Some(Skin {
            bones: BONES.iter().map(|name| name.to_string()).collect(),
            bind_inverse,
            influences: SkinWeights::from_quads(&influence_rows, &influence_weights)
                .expect("four-lane rows"),
        }),
        materials: material.into_iter().collect(),
        ..MeshAsset::default()
    }
}

/// Three shapes: a textured triangle, a textured quad and an untextured
/// triangle, each with its own bone pair.
fn bundle() -> Vec<MeshAsset> {
    vec![
        shape(
            "Body",
            0.0,
            3,
            ([0, 1, 0, 0], [0.75, 0.25, 0.0, 0.0]),
            Some(MaterialSlot {
                name: "body".into(),
                shader: ShaderKind::Cloth,
                diffuse: Some("textures/body_d.dds".into()),
                normal: Some("textures/body_n.dds".into()),
                specular: None,
                double_sided: true,
            }),
        ),
        shape(
            "Hands",
            4.0,
            4,
            ([1, 2, 0, 0], [0.75, 0.25, 0.0, 0.0]),
            Some(MaterialSlot {
                name: "hands".into(),
                shader: ShaderKind::Skin,
                diffuse: Some("textures/hands_d.dds".into()),
                normal: None,
                specular: None,
                double_sided: false,
            }),
        ),
        shape("Feet", 9.0, 3, ([0, 2, 0, 0], [0.75, 0.25, 0.0, 0.0]), None),
    ]
}

#[test]
fn se_multi_shape() {
    let shapes = bundle();
    for asset in &shapes {
        asset.validate().expect("the fixture is a valid model");
    }
    let bytes = write_se_bundle(&shapes).expect("a three-shape bundle writes");
    let document = read(&bytes).expect("our SE reader opens the bundle");
    assert_eq!(document.bsver, 100, "the bundle is a Skyrim SE stream");
    assert_eq!(document.header.user_version, 12);
    assert_eq!(document.shapes.len(), shapes.len(), "one block per shape");

    for (index, source) in shapes.iter().enumerate() {
        let shape = &document.shapes[index];
        assert_eq!(shape.name, source.name, "shape {index} keeps its name");
        assert_eq!(
            shape.positions.len(),
            source.positions.len(),
            "shape {index} keeps its vertex count"
        );
        assert_eq!(
            shape.triangles.len(),
            source.triangles.len(),
            "shape {index} keeps its triangle count"
        );
        assert_eq!(shape.triangles, source.triangles);
        for (vertex, (actual, expected)) in
            shape.positions.iter().zip(&source.positions).enumerate()
        {
            for (left, right) in actual.iter().zip(expected) {
                assert!(
                    (left - right).abs() <= 1e-6,
                    "shape {index} vertex {vertex} drifted"
                );
            }
        }
        let authored = source.skin.as_ref().expect("authored skin");
        let read_skin = shape.skin.as_ref().expect("each shape keeps its skin");
        assert_eq!(
            read_skin.bones, authored.bones,
            "shape {index} keeps the shared palette order"
        );
        assert_eq!(read_skin.influences.rows(), authored.influences.rows());
        for vertex in 0..authored.influences.rows() {
            let read_row = read_skin.influences.row(vertex).expect("read row");
            let authored_row = authored.influences.row(vertex).expect("authored row");
            assert_eq!(
                read_row.joints, authored_row.joints,
                "shape {index} vertex {vertex} bone lanes"
            );
            for (left, right) in read_row.weights.iter().zip(authored_row.weights) {
                assert!(
                    (left - right).abs() <= 1e-3,
                    "shape {index} vertex {vertex} weight drifted (f16)"
                );
            }
        }
    }

    // Distinct counts prove no shape consumed another shape's buffer.
    assert_eq!(
        document
            .shapes
            .iter()
            .map(|shape| (shape.positions.len(), shape.triangles.len()))
            .collect::<Vec<_>>(),
        vec![(3, 1), (4, 2), (3, 1)]
    );
}

#[test]
fn se_shape_count() {
    let bytes = write_se_bundle(&bundle()).expect("writes");
    let scene = parse_nif(&bytes).expect("parses");
    let (header, _) = byroredux_nif::header::NifHeader::parse(&bytes).expect("header");
    assert_eq!(
        header.block_sizes.len(),
        scene.len(),
        "one recorded size per block"
    );

    // Block order: shape groups, then the root, then the bone union.
    let types: Vec<&str> = (0..scene.len())
        .map(|index| scene.get(index).expect("block").block_type_name())
        .collect();
    assert_eq!(
        types,
        vec![
            "BSTriShape",
            "NiSkinInstance",
            "NiSkinData",
            "NiSkinPartition",
            "BSLightingShaderProperty",
            "BSShaderTextureSet",
            "BSTriShape",
            "NiSkinInstance",
            "NiSkinData",
            "NiSkinPartition",
            "BSLightingShaderProperty",
            "BSShaderTextureSet",
            "BSTriShape",
            "NiSkinInstance",
            "NiSkinData",
            "NiSkinPartition",
            "NiNode",
            "NiNode",
            "NiNode",
            "NiNode",
        ]
    );

    let root = scene.root_index.expect("one scene root");
    assert_eq!(root, 16, "the root follows the shape groups");
    let root_node = scene.get_as::<NiNode>(root).expect("root NiNode");
    let children: Vec<usize> = root_node
        .children
        .iter()
        .filter_map(|child| child.index())
        .collect();
    assert_eq!(children, vec![0, 6, 12], "one root references every shape");
    assert!(
        root_node.av.net.extra_data_refs.is_empty(),
        "no physics link without a sidecar"
    );

    let bone_nodes: Vec<usize> = (root + 1..scene.len())
        .filter(|index| scene.get_as::<NiNode>(*index).is_some())
        .collect();
    assert_eq!(bone_nodes.len(), BONES.len(), "one NiNode per bone name");
    let names: Vec<&str> = bone_nodes
        .iter()
        .map(|index| {
            scene
                .get_as::<NiNode>(*index)
                .and_then(|node| node.av.net.name.as_deref())
                .expect("every bone node is named")
        })
        .collect();
    assert_eq!(names, BONES);

    let mut referenced = BTreeSet::new();
    for instance_index in [1usize, 7, 13] {
        let instance = scene
            .get_as::<NiSkinInstance>(instance_index)
            .expect("instance");
        let bones: Vec<usize> = instance
            .bone_refs
            .iter()
            .filter_map(|bone| bone.index())
            .collect();
        assert_eq!(
            bones, bone_nodes,
            "shape at {instance_index} points at the shared bone nodes"
        );
        assert_eq!(
            instance.skeleton_root_ref.index(),
            Some(root),
            "every instance shares the one root"
        );
        referenced.extend(bones);
    }
    assert_eq!(
        referenced.len(),
        BONES.len(),
        "the union has no duplicated bone node"
    );

    // Per shape: its own partition, its own bone pair, its own vertex buffer.
    for (partition_index, bones, vertices) in [
        (3usize, vec![0u16, 1], 3usize),
        (9, vec![1, 2], 4),
        (15, vec![0, 2], 3),
    ] {
        let partition = scene
            .get_as::<NiSkinPartition>(partition_index)
            .expect("partition");
        assert_eq!(partition.partitions.len(), 1);
        assert_eq!(
            partition.partitions[0].bones, bones,
            "partition {partition_index} follows its own shape's lanes"
        );
        let global = partition
            .global_vertex_data
            .as_ref()
            .expect("skinned SE shapes keep the packed buffer");
        let stride = global.vertex_size as usize;
        assert_eq!(
            global.raw_bytes.len(),
            stride * vertices,
            "partition {partition_index} packs only its own shape"
        );
    }

    // One shader per textured shape, in shape order.
    for (shader_index, textures_index, expected) in [
        (
            4usize,
            5usize,
            vec!["textures/body_d.dds", "textures/body_n.dds"],
        ),
        (10, 11, vec!["textures/hands_d.dds"]),
    ] {
        let shader = scene
            .get_as::<BSLightingShaderProperty>(shader_index)
            .expect("shader");
        let textures = scene
            .get_as::<BSShaderTextureSet>(textures_index)
            .expect("texture set");
        assert_eq!(
            shader.texture_set_ref.index(),
            Some(textures_index),
            "shader {shader_index} names its own texture set"
        );
        assert_eq!(textures.textures, expected);
    }
}

/// Copy a skin with every row reduced to the four strongest influences.
///
/// The real Bijin body authors five influences on five vertices, which the
/// four-lane native view refuses. The bundle test compares counts and bone
/// names, so reducing those rows in the test keeps the source palette and every
/// other row exactly as authored.
fn four_lane_skin(skin: &Skin) -> Skin {
    let mut joints = Vec::with_capacity(skin.influences.rows());
    let mut weights = Vec::with_capacity(skin.influences.rows());
    for row in 0..skin.influences.rows() {
        let source = skin.influences.row(row).expect("authored row");
        let mut positive: Vec<(u16, f32)> = source
            .joints
            .iter()
            .copied()
            .zip(source.weights.iter().copied())
            .filter(|(_, weight)| *weight > 0.0)
            .collect();
        positive.sort_unstable_by(|(left_joint, left), (right_joint, right)| {
            right.total_cmp(left).then(left_joint.cmp(right_joint))
        });
        let kept = &positive[..positive.len().min(4)];
        let sum: f32 = kept.iter().map(|(_, weight)| *weight).sum();
        let mut lanes = ([0u16; 4], [0.0f32; 4]);
        for (lane, (joint, weight)) in kept.iter().enumerate() {
            lanes.0[lane] = *joint;
            lanes.1[lane] = weight / sum;
        }
        joints.push(lanes.0);
        weights.push(lanes.1);
    }
    Skin {
        bones: skin.bones.clone(),
        bind_inverse: skin.bind_inverse.clone(),
        influences: SkinWeights::from_quads(&joints, &weights).expect("four-lane rows"),
    }
}

/// A synthetic shape that joins the real body's palette.
fn companion(name: &str, bones: &[String], base: f32) -> MeshAsset {
    MeshAsset {
        name: name.to_string(),
        positions: vec![[base, 0.0, 0.0], [base + 1.0, 0.0, 0.0], [base, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: bones.to_vec(),
            bind_inverse: Vec::new(),
            influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 3], &[[0.75, 0.25, 0.0, 0.0]; 3])
                .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

#[test]
fn se_bundle_with_the_real_bijin_body() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        eprintln!("SCULPTO_BIJIN_NIF not set; skipping the real-body bundle control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the fixture NIF is readable");
    let source = read(&bytes).expect("the Bijin body parses");
    assert!(!source.shapes.is_empty(), "the fixture carries a body");

    let body = &source.shapes[0];
    let palette = body
        .skin
        .as_ref()
        .expect("the body is skinned")
        .bones
        .clone();
    let mut real = MeshAsset::from(body);
    real.name = "Body".into();
    real.skin = real.skin.as_ref().map(four_lane_skin);
    let expected = [(real.positions.len(), real.triangles.len()), (3, 1), (3, 1)];
    let shapes = vec![
        real,
        companion("Hands", &palette, 5000.0),
        companion("Feet", &palette, 5003.0),
    ];

    let written = write_se_bundle(&shapes).expect("the real body joins the bundle");
    let document = read(&written).expect("the engine reader opens the bundle");
    assert_eq!(document.shapes.len(), shapes.len());
    eprintln!(
        "real bundle: {} shapes, body {} vertices / {} triangles, {} bone names, {} bytes",
        shapes.len(),
        expected[0].0,
        expected[0].1,
        palette.len(),
        written.len()
    );
    for (index, source) in shapes.iter().enumerate() {
        let shape = &document.shapes[index];
        assert_eq!(
            (shape.positions.len(), shape.triangles.len()),
            expected[index],
            "shape {index} keeps its emitted counts"
        );
        assert_eq!(
            shape.skin.as_ref().expect("skin").bones,
            source.skin.as_ref().expect("authored skin").bones,
            "shape {index} keeps its bone names"
        );
    }
}

/// Guards the parser assumption the block-order test depends on.
#[test]
fn scene_root_is_the_first_ni_node_block() {
    let bytes = write_se_bundle(&bundle()).expect("writes");
    let scene = parse_nif(&bytes).expect("parses");
    let first_node = (0..scene.len())
        .find(|index| scene.get_as::<NiNode>(*index).is_some())
        .expect("the bundle has a node");
    assert_eq!(scene.root_index, Some(first_node));
    let shape = scene.get_as::<BsTriShape>(0).expect("shape");
    assert_eq!(
        shape.num_vertices, 0,
        "skinned SE shapes carry no inline count"
    );
    assert!(shape.vertices.is_empty());
}
