//! The skinned GLB writer, re-read through the `gltf` crate.
//!
//! Every assertion reads the parsed document back, never the input struct, so
//! the tests see what a consumer would see.

use gltf::{Gltf, Semantic};
use sculpto_core::mesh::{MeshAsset, MeshError, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{GltfError, write_glb};

/// A two-bone skinned triangle with asymmetric bind matrices, so a transpose
/// bug cannot pass the per-element comparison.
fn two_bone_triangle() -> MeshAsset {
    MeshAsset {
        name: "body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: vec!["root".into(), "spine".into()],
            bind_inverse: vec![
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [1.0, 2.0, 3.0, 1.0],
                ],
                [
                    [2.0, 0.0, 0.0, 0.0],
                    [0.0, 0.5, 0.0, 0.0],
                    [0.0, 0.0, 4.0, 0.0],
                    [5.0, 0.0, -1.0, 1.0],
                ],
            ],
            influences: SkinWeights::from_quads(
                &[[0, 1, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]],
                &[
                    [0.7, 0.3, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                ],
            )
            .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

/// Raw accessor payload from the embedded buffer, at accessor coordinates.
fn accessor_bytes<'doc>(parsed: &'doc Gltf, accessor: &gltf::Accessor<'doc>) -> &'doc [u8] {
    let view = accessor.view().expect("accessor has a buffer view");
    let blob = parsed.blob.as_deref().expect("GLB carries the buffer");
    let start = view.offset() + accessor.offset();
    let end = start + accessor.size() * accessor.count();
    &blob[start..end]
}

/// Fixed-width `f32` rows, little-endian.
fn floats<const WIDTH: usize>(bytes: &[u8]) -> Vec<[f32; WIDTH]> {
    bytes
        .chunks_exact(4 * WIDTH)
        .map(|row| {
            let mut values = [0.0_f32; WIDTH];
            for (axis, cell) in values.iter_mut().enumerate() {
                let start = axis * 4;
                *cell = f32::from_le_bytes(row[start..start + 4].try_into().expect("f32 cell"));
            }
            values
        })
        .collect()
}

/// Fixed-width `u16` rows, little-endian.
fn shorts<const WIDTH: usize>(bytes: &[u8]) -> Vec<[u16; WIDTH]> {
    bytes
        .chunks_exact(2 * WIDTH)
        .map(|row| {
            let mut values = [0_u16; WIDTH];
            for (axis, cell) in values.iter_mut().enumerate() {
                let start = axis * 2;
                *cell = u16::from_le_bytes(row[start..start + 2].try_into().expect("u16 cell"));
            }
            values
        })
        .collect()
}

/// Scalar `u32` values, little-endian.
fn words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|row| u32::from_le_bytes(*row))
        .collect()
}

/// `MAT4` rows: four column-major f32 columns per matrix.
fn matrices(bytes: &[u8]) -> Vec<[[f32; 4]; 4]> {
    bytes
        .as_chunks::<64>()
        .0
        .iter()
        .map(|row| floats::<4>(row).try_into().expect("MAT4 row"))
        .collect()
}

#[test]
fn skinned_triangle_reports_joints_weights_and_bind_matrices() {
    let asset = two_bone_triangle();
    let bytes = write_glb(&asset).expect("a valid skinned triangle writes");
    let parsed = Gltf::from_slice(&bytes).expect("the writer output parses");

    let skin = parsed.document.skins().next().expect("one skin");
    let joints: Vec<_> = skin.joints().collect();
    let skin_data = asset.skin.as_ref().expect("skin");
    assert_eq!(joints.len(), skin_data.bones.len(), "one joint per bone");
    let names: Vec<_> = joints
        .iter()
        .map(|node| node.name().expect("joint nodes are named"))
        .collect();
    assert_eq!(names, ["root", "spine"], "joint order is model bone order");
    let skeleton = skin.skeleton().expect("the skin names a skeleton root");
    assert_eq!(skeleton.index(), joints[0].index(), "the first joint roots");

    let ibm = skin
        .inverse_bind_matrices()
        .expect("bind matrices are written");
    assert_eq!(
        ibm.data_type(),
        gltf::accessor::DataType::F32,
        "bind matrices are f32"
    );
    let read_back = matrices(accessor_bytes(&parsed, &ibm));
    assert_eq!(read_back.len(), skin_data.bind_inverse.len());
    for (written, bound) in read_back.iter().zip(skin_data.bind_inverse.iter()) {
        for (column, bound_column) in written.iter().zip(bound.iter()) {
            for (value, bound_value) in column.iter().zip(bound_column.iter()) {
                assert!(
                    (value - bound_value).abs() <= 1e-6,
                    "IBM cell {value} vs bound {bound_value}"
                );
            }
        }
    }

    let mesh = parsed.document.meshes().next().expect("one mesh");
    let primitive = mesh.primitives().next().expect("one primitive");
    let joints_accessor = primitive.get(&Semantic::Joints(0)).expect("JOINTS_0");
    assert_eq!(
        joints_accessor.data_type(),
        gltf::accessor::DataType::U16,
        "JOINTS_0 is u16"
    );
    let joints_read = shorts::<4>(accessor_bytes(&parsed, &joints_accessor));
    assert_eq!(joints_read, [[0, 1, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]]);

    let weights_accessor = primitive.get(&Semantic::Weights(0)).expect("WEIGHTS_0");
    assert_eq!(
        weights_accessor.data_type(),
        gltf::accessor::DataType::F32,
        "WEIGHTS_0 is f32"
    );
    let weights_read = floats::<4>(accessor_bytes(&parsed, &weights_accessor));
    assert_eq!(weights_read.len(), asset.positions.len());
    for row in &weights_read {
        let sum: f32 = row.iter().sum();
        assert!((sum - 1.0).abs() <= 1e-6, "weights sum to {sum}");
    }

    let mesh_node = parsed
        .document
        .nodes()
        .find(|node| node.mesh().is_some())
        .expect("one mesh node");
    assert_eq!(mesh_node.name(), Some("body"), "mesh node carries the name");
    assert_eq!(mesh.name(), Some("body"), "mesh carries the name");
}

#[test]
fn skinless_mesh_writes_and_parses_without_a_skin() {
    let asset = MeshAsset {
        name: "plain".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    };
    let bytes = write_glb(&asset).expect("a skinless mesh writes");
    let parsed = Gltf::from_slice(&bytes).expect("the writer output parses");

    assert_eq!(parsed.document.skins().count(), 0, "no skin is written");
    assert_eq!(
        parsed.document.nodes().count(),
        1,
        "no bone nodes are added"
    );
    let mesh = parsed.document.meshes().next().expect("one mesh");
    let primitive = mesh.primitives().next().expect("one primitive");
    assert!(primitive.get(&Semantic::Joints(0)).is_none());
    assert!(primitive.get(&Semantic::Weights(0)).is_none());
    let mesh_node = parsed
        .document
        .nodes()
        .find(|node| node.mesh().is_some())
        .expect("one mesh node");
    assert!(
        parsed.document.as_json().nodes[mesh_node.index()]
            .skin
            .is_none()
    );
    assert_eq!(mesh_node.name(), Some("plain"));
    assert_eq!(mesh.name(), Some("plain"));
}

#[test]
fn positions_and_indices_round_trip_exactly() {
    let asset = MeshAsset {
        name: "exact".into(),
        positions: vec![
            [0.1, -0.25, 3.75],
            [1.0 / 3.0, 1e-7, 1.0e7],
            [-12345.678, 0.5, 2.0],
            [9.0, 8.0, 7.0],
        ],
        triangles: vec![[3, 2, 1], [0, 1, 2]],
        // Partition 7 appears first in the model; primitives still come in
        // ascending partition order.
        triangle_partitions: vec![PartitionId(7), PartitionId(0)],
        ..MeshAsset::default()
    };
    asset.validate().expect("the exactness model is valid");
    let parsed = Gltf::from_slice(&write_glb(&asset).expect("writes")).expect("parses");

    let primitives: Vec<_> = parsed
        .document
        .meshes()
        .next()
        .expect("one mesh")
        .primitives()
        .collect();
    assert_eq!(primitives.len(), 2, "one primitive per partition");

    let positions_accessor = primitives[0]
        .get(&Semantic::Positions)
        .expect("POSITION is written");
    let positions = floats::<3>(accessor_bytes(&parsed, &positions_accessor));
    assert_eq!(positions, asset.positions.as_slice(), "f32 is bit-exact");

    let first = primitives[0].indices().expect("indices are written");
    assert_eq!(
        first.data_type(),
        gltf::accessor::DataType::U32,
        "indices are u32"
    );
    assert_eq!(words(accessor_bytes(&parsed, &first)), [0, 1, 2]);
    let second = primitives[1].indices().expect("indices are written");
    assert_eq!(words(accessor_bytes(&parsed, &second)), [3, 2, 1]);
}

#[test]
fn corrupting_a_weight_changes_the_parsed_accessor() {
    let read_first_weights = |asset: &MeshAsset| {
        let parsed = Gltf::from_slice(&write_glb(asset).expect("writes")).expect("parses");
        let primitive = parsed
            .document
            .meshes()
            .next()
            .expect("one mesh")
            .primitives()
            .next()
            .expect("one primitive");
        let accessor = primitive.get(&Semantic::Weights(0)).expect("WEIGHTS_0");
        floats::<4>(accessor_bytes(&parsed, &accessor))
    };

    let plain = read_first_weights(&two_bone_triangle());
    let mut corrupted = two_bone_triangle();
    corrupted.skin.as_mut().expect("skin").influences = SkinWeights::from_quads(
        &[[1, 0, 0, 0], [0, 1, 0, 0], [0, 1, 0, 0]],
        &[
            [0.6, 0.4, 0.0, 0.0],
            [0.7, 0.3, 0.0, 0.0],
            [0.7, 0.3, 0.0, 0.0],
        ],
    )
    .expect("four-lane rows");
    let corrupted = read_first_weights(&corrupted);

    assert_ne!(
        plain[0], corrupted[0],
        "the corruption reaches the parsed accessor"
    );
    assert_eq!(plain[0], [0.7, 0.3, 0.0, 0.0]);
    // The wire row is the normalized projection, so compare within f32 noise.
    for (actual, expected) in corrupted[0].iter().zip([0.6, 0.4, 0.0, 0.0]) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "the corrupted row is written normalized: {actual} vs {expected}"
        );
    }
}

#[test]
fn an_empty_bind_inverse_writes_identity_matrices() {
    let mut asset = two_bone_triangle();
    asset.skin.as_mut().expect("skin").bind_inverse.clear();
    let parsed = Gltf::from_slice(&write_glb(&asset).expect("writes")).expect("parses");

    let skin = parsed.document.skins().next().expect("one skin");
    let read_back = matrices(accessor_bytes(
        &parsed,
        &skin
            .inverse_bind_matrices()
            .expect("bind matrices are written"),
    ));
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    assert_eq!(read_back, [identity, identity]);
}

#[test]
fn an_invalid_model_is_refused_without_bytes() {
    let mut short_skin = two_bone_triangle();
    short_skin.skin.as_mut().expect("skin").influences =
        SkinWeights::from_quads(&[[0, 1, 0, 0]; 2], &[[0.7, 0.3, 0.0, 0.0]; 2])
            .expect("two four-lane rows");
    assert_eq!(
        write_glb(&short_skin),
        Err(GltfError::InvalidMesh(MeshError::SkinLengthMismatch)),
        "a skin with fewer rows than vertices is refused before any byte is produced"
    );

    let no_triangles = MeshAsset {
        positions: vec![[0.0; 3]; 3],
        ..MeshAsset::default()
    };
    assert_eq!(write_glb(&no_triangles), Err(GltfError::NoTriangles));
}

#[test]
fn all_five_influences_survive_paired_sets_with_one_unit_sum() {
    let mut asset = two_bone_triangle();
    let source = SkinWeights::new(
        vec![0, 5, 6, 7],
        vec![0, 1, 2, 3, 4, 1, 2],
        vec![0.25, 0.25, 0.25, 0.25, 1.0, 1.5, 0.5],
        5,
    )
    .expect("authored rows");
    asset.skin = Some(Skin {
        bones: (0..5).map(|joint| format!("joint{joint}")).collect(),
        bind_inverse: Vec::new(),
        influences: source.clone(),
    });
    let parsed = Gltf::from_slice(&write_glb(&asset).expect("multi-set GLB"))
        .expect("consumer decodes the output");
    let mesh = parsed.document.meshes().next().expect("mesh");
    let primitive = mesh.primitives().next().expect("primitive");
    let access = |semantic| primitive.get(&semantic).expect("paired accessor");
    assert_eq!(
        shorts::<4>(accessor_bytes(&parsed, &access(Semantic::Joints(1)))),
        [[4, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]
    );
    assert_eq!(
        floats::<4>(accessor_bytes(&parsed, &access(Semantic::Weights(0)))),
        [[0.125; 4], [1., 0., 0., 0.], [1., 0., 0., 0.]]
    );
    assert_eq!(
        floats::<4>(accessor_bytes(&parsed, &access(Semantic::Weights(1)))),
        [[0.5, 0., 0., 0.], [0.; 4], [0.; 4]]
    );
    assert!(primitive.get(&Semantic::Joints(2)).is_none());
    assert_eq!(asset.skin.as_ref().expect("source skin").influences, source);
}

#[test]
fn unrepresentable_positive_weight_is_refused_before_glb_output() {
    let mut asset = two_bone_triangle();
    let skin = asset.skin.as_mut().expect("skin");
    skin.influences = SkinWeights::new(
        vec![0, 2, 3, 4],
        vec![0, 1, 0, 1],
        vec![f32::from_bits(1), 8., 1., 1.],
        2,
    )
    .expect("positive finite authored values");
    assert!(matches!(
        write_glb(&asset),
        Err(GltfError::WeightUnrepresentable {
            vertex: 0,
            joint: 0
        })
    ));
}
