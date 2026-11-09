//! Integration tests for glTF materials, textures, and morph targets.
//!
//! Assertions read the parsed document back via the `gltf` crate, comparing
//! parsed values directly against authored model values.

use gltf::Gltf;
use sculpto_core::mesh::{
    MaterialSlot, MeshAsset, MeshError, MorphDelta, MorphTarget, PartitionId, ShaderKind,
};
use sculpto_gltf::{GltfError, write_glb};

/// A three-vertex triangle base for testing.
fn triangle() -> MeshAsset {
    MeshAsset {
        name: "test_mesh".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
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

#[test]
fn material_uris_and_primitive_index_round_trip() {
    let mut asset = triangle();
    asset.materials = vec![MaterialSlot {
        name: "robe".into(),
        shader: ShaderKind::Cloth,
        diffuse: Some("textures/robe_d.png".into()),
        normal: Some("textures/robe_n.png".into()),
        specular: None,
        double_sided: true,
    }];

    let bytes = write_glb(&asset).expect("valid model with material writes");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");

    assert_eq!(parsed.document.materials().count(), 1);
    let material = parsed.document.materials().next().expect("one material");
    assert_eq!(material.name(), Some("robe"));
    assert!(material.double_sided(), "double_sided flag preserved");

    // baseColorTexture (diffuse)
    let pbr = material.pbr_metallic_roughness();
    let base_color = pbr.base_color_texture().expect("baseColorTexture present");
    let diffuse_source = base_color.texture().source().source();
    let diffuse_uri = match diffuse_source {
        gltf::image::Source::Uri { uri, .. } => uri,
        _ => panic!("expected URI source for diffuse texture"),
    };
    assert_eq!(
        diffuse_uri,
        asset.materials[0].diffuse.as_deref().unwrap(),
        "diffuse URI matches model value"
    );

    // normalTexture (normal)
    let normal_info = material.normal_texture().expect("normalTexture present");
    let normal_source = normal_info.texture().source().source();
    let normal_uri = match normal_source {
        gltf::image::Source::Uri { uri, .. } => uri,
        _ => panic!("expected URI source for normal texture"),
    };
    assert_eq!(
        normal_uri,
        asset.materials[0].normal.as_deref().unwrap(),
        "normal URI matches model value"
    );

    // Primitive references material index 0
    let mesh = parsed.document.meshes().next().expect("one mesh");
    let primitive = mesh.primitives().next().expect("one primitive");
    let prim_mat = primitive.material();
    assert_eq!(prim_mat.index(), Some(0));
    assert_eq!(prim_mat.name(), Some("robe"));
}

#[test]
fn morph_targets_weights_names_and_deltas_round_trip() {
    let mut asset = triangle();
    asset.morphs = vec![
        MorphTarget {
            name: "Smile".into(),
            deltas: vec![MorphDelta {
                vertex: 1,
                position: [0.25, -0.5, 1.5],
            }],
        },
        MorphTarget {
            name: "Widen".into(),
            deltas: vec![
                MorphDelta {
                    vertex: 0,
                    position: [1.0, 0.0, -0.5],
                },
                MorphDelta {
                    vertex: 2,
                    position: [-0.75, 0.5, 0.25],
                },
            ],
        },
    ];

    let bytes = write_glb(&asset).expect("model with morphs writes");
    let parsed = Gltf::from_slice(&bytes).expect("writer output parses");

    let mesh = parsed.document.meshes().next().expect("one mesh");

    // mesh.weights: all zero
    let weights = mesh.weights().expect("mesh.weights present");
    assert_eq!(weights.len(), 2, "one weight per morph target");
    assert_eq!(weights, &[0.0, 0.0], "default weights are all zero");

    // Target names via extras
    let extras_raw = mesh
        .extras()
        .as_ref()
        .expect("extras present on mesh")
        .get();
    let extras_json: serde_json::Value =
        serde_json::from_str(extras_raw).expect("valid extras JSON");
    let target_names: Vec<&str> = extras_json["targetNames"]
        .as_array()
        .expect("targetNames array in extras")
        .iter()
        .map(|v| v.as_str().expect("string target name"))
        .collect();
    assert_eq!(
        target_names,
        ["Smile", "Widen"],
        "target names match authored morph names"
    );

    // Primitive morph targets count and deltas
    let primitive = mesh.primitives().next().expect("one primitive");
    let targets: Vec<_> = primitive.morph_targets().collect();
    assert_eq!(targets.len(), 2, "two morph targets on primitive");

    // Target 0 ("Smile"): vertex 1 moved, 0 and 2 at origin
    let pos_accessor_0 = targets[0]
        .positions()
        .expect("target 0 has POSITION accessor");
    assert_eq!(
        pos_accessor_0.count(),
        asset.positions.len(),
        "target accessor count matches vertex count"
    );
    let deltas_0 = floats::<3>(accessor_bytes(&parsed, &pos_accessor_0));
    let model_delta = asset.morphs[0].deltas[0].position;

    // Nonzero delta value compared against model value
    assert_eq!(
        deltas_0[1], model_delta,
        "parsed delta at vertex 1 matches model value"
    );
    assert_eq!(deltas_0[1], [0.25, -0.5, 1.5]);
    assert_eq!(
        deltas_0[0],
        [0.0, 0.0, 0.0],
        "unshifted vertex 0 delta is 0"
    );
    assert_eq!(
        deltas_0[2],
        [0.0, 0.0, 0.0],
        "unshifted vertex 2 delta is 0"
    );

    // Negative control: parsed delta vs corrupted model value
    let corrupted_delta = [model_delta[0] + 0.1, model_delta[1], model_delta[2]];
    assert_ne!(
        deltas_0[1], corrupted_delta,
        "negative control: parsed delta differs from perturbed model delta"
    );

    // Target 1 ("Widen"): vertices 0 and 2 moved, 1 at origin
    let pos_accessor_1 = targets[1]
        .positions()
        .expect("target 1 has POSITION accessor");
    let deltas_1 = floats::<3>(accessor_bytes(&parsed, &pos_accessor_1));
    assert_eq!(
        deltas_1[0], asset.morphs[1].deltas[0].position,
        "vertex 0 delta matches model"
    );
    assert_eq!(
        deltas_1[2], asset.morphs[1].deltas[1].position,
        "vertex 2 delta matches model"
    );
    assert_eq!(
        deltas_1[1],
        [0.0, 0.0, 0.0],
        "unshifted vertex 1 delta is 0"
    );
}

#[test]
fn multi_partition_mesh_shares_material_and_morph_targets() {
    let asset = MeshAsset {
        name: "two_partitions".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        triangle_partitions: vec![PartitionId(0), PartitionId(1)],
        materials: vec![MaterialSlot {
            name: "unified".into(),
            diffuse: Some("textures/cloth.png".into()),
            ..MaterialSlot::default()
        }],
        morphs: vec![MorphTarget {
            name: "Shift".into(),
            deltas: vec![MorphDelta {
                vertex: 3,
                position: [0.0, 0.0, 1.0],
            }],
        }],
        ..MeshAsset::default()
    };

    let bytes = write_glb(&asset).expect("multi-partition mesh writes");
    let parsed = Gltf::from_slice(&bytes).expect("parses");

    let primitives: Vec<_> = parsed
        .document
        .meshes()
        .next()
        .expect("one mesh")
        .primitives()
        .collect();
    assert_eq!(primitives.len(), 2, "one primitive per partition");

    for (index, primitive) in primitives.iter().enumerate() {
        assert_eq!(
            primitive.material().index(),
            Some(0),
            "primitive {index} maps to material 0"
        );
        let targets: Vec<_> = primitive.morph_targets().collect();
        assert_eq!(
            targets.len(),
            1,
            "primitive {index} carries the morph target"
        );
        let pos = targets[0].positions().expect("POSITION delta");
        let deltas = floats::<3>(accessor_bytes(&parsed, &pos));
        assert_eq!(deltas[3], [0.0, 0.0, 1.0]);
    }
}

#[test]
fn specular_textures_are_refused() {
    let mut asset = triangle();
    asset.materials = vec![MaterialSlot {
        name: "shiny".into(),
        diffuse: Some("textures/robe_d.png".into()),
        specular: Some("textures/robe_s.png".into()),
        ..MaterialSlot::default()
    }];

    assert_eq!(
        write_glb(&asset),
        Err(GltfError::TextureSlotUnsupported { slot: "specular" }),
        "specular texture map has no glTF core slot and must be refused"
    );
}

#[test]
fn multiple_material_slots_are_refused() {
    let mut asset = triangle();
    asset.materials = vec![
        MaterialSlot {
            name: "slot_0".into(),
            ..MaterialSlot::default()
        },
        MaterialSlot {
            name: "slot_1".into(),
            ..MaterialSlot::default()
        },
    ];

    assert_eq!(
        write_glb(&asset),
        Err(GltfError::MultipleMaterialSlots { count: 2 }),
        "model with multiple material slots is refused today"
    );
}

#[test]
fn morph_with_vertex_out_of_range_is_refused() {
    let mut asset = triangle();
    asset.morphs = vec![MorphTarget {
        name: "out_of_bounds".into(),
        deltas: vec![MorphDelta {
            vertex: 3, // Valid indices are 0..=2
            position: [1.0, 2.0, 3.0],
        }],
    }];

    assert_eq!(
        write_glb(&asset),
        Err(GltfError::InvalidMesh(MeshError::MorphVertexOutOfRange {
            target: 0,
            vertex: 3,
        })),
        "a morph referencing non-existent vertex must be refused, not truncated"
    );
}

#[test]
fn mesh_without_materials_or_morphs_omits_optional_fields() {
    let asset = triangle();
    let bytes = write_glb(&asset).expect("plain triangle writes");
    let parsed = Gltf::from_slice(&bytes).expect("parses");

    assert_eq!(parsed.document.materials().count(), 0);
    let mesh = parsed.document.meshes().next().expect("one mesh");
    assert!(mesh.weights().is_none());
    let primitive = mesh.primitives().next().expect("one primitive");
    assert!(primitive.material().index().is_none());
    assert_eq!(primitive.morph_targets().count(), 0);
}
