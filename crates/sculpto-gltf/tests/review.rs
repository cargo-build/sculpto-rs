//! Review GLB byte layout, round-tripped through the `gltf` crate reader.
//!
//! The container is parsed by the same reader a consumer uses, so the
//! assertions cover the framing (magic, alignment, lengths), the one
//! mesh/material/node-per-part layout, the palette colours and the typed
//! refusals — not a hand-rolled byte walk.

use gltf::Gltf;
use sculpto_gltf::GltfError;
use sculpto_gltf::review::{BODY_COLOR, ReviewPart, palette, write_review_glb};

/// One unit triangle at a given offset, as a stand-in part.
fn part(name: &str, offset: f32, color: [f32; 4]) -> ReviewPart {
    ReviewPart {
        name: name.to_string(),
        positions: vec![
            [offset, 0.0, 0.0],
            [offset + 1.0, 0.0, 0.0],
            [offset, 1.0, 0.0],
        ],
        triangles: vec![[0, 1, 2]],
        color,
    }
}

/// Raw `f32` rows from the embedded buffer, at accessor coordinates.
fn floats(parsed: &Gltf, accessor: &gltf::Accessor<'_>, width: usize) -> Vec<Vec<f32>> {
    let view = accessor.view().expect("accessor has a buffer view");
    let blob = parsed.blob.as_deref().expect("GLB carries the buffer");
    let start = view.offset() + accessor.offset();
    let end = start + accessor.size() * accessor.count();
    blob[start..end]
        .chunks_exact(4 * width)
        .map(|row| {
            (0..width)
                .map(|axis| {
                    f32::from_le_bytes(row[axis * 4..axis * 4 + 4].try_into().expect("f32 cell"))
                })
                .collect()
        })
        .collect()
}

#[test]
fn the_container_is_framed_and_chunk_aligned() {
    let bytes = write_review_glb(&[part("body", 0.0, BODY_COLOR)]).expect("a GLB");
    // The header names the whole file; the chunks sit on 4-byte boundaries.
    assert_eq!(&bytes[0..4], b"glTF", "magic");
    let total = u32::from_le_bytes(bytes[8..12].try_into().expect("length"));
    assert_eq!(total as usize, bytes.len(), "header length covers the file");
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().expect("json length")) as usize;
    assert_eq!(json_len % 4, 0, "JSON chunk is padded to 4 bytes");
    assert_eq!(
        bytes[16], 0x4A,
        "JSON chunk type starts with 'J' of \"JSON\""
    );
    let bin_offset = 20 + json_len;
    let bin_len = u32::from_le_bytes(
        bytes[bin_offset..bin_offset + 4]
            .try_into()
            .expect("bin length"),
    ) as usize;
    assert_eq!(bin_len % 4, 0, "binary chunk is padded to 4 bytes");
    assert_eq!(
        bytes[bin_offset + 4],
        0x42,
        "binary chunk type starts with 'B' of \"BIN\\0\""
    );
    assert_eq!(bin_offset + 8 + bin_len, bytes.len(), "no trailing bytes");
}

#[test]
fn each_part_is_one_mesh_one_material_one_node() {
    let layer = palette(2).expect("rank 2 has a palette slot");
    let bytes = write_review_glb(&[part("body", 0.0, BODY_COLOR), part("panty", 5.0, layer)])
        .expect("a GLB");
    let parsed = Gltf::from_slice(&bytes).expect("the GLB parses");

    assert_eq!(parsed.meshes().len(), 2, "one mesh per part");
    assert_eq!(parsed.materials().len(), 2, "one material per part");
    assert_eq!(parsed.nodes().len(), 2, "one root node per part");
    let scene = parsed.default_scene().expect("a default scene");
    let roots: Vec<gltf::Node<'_>> = scene.nodes().collect();
    assert_eq!(roots.len(), 2, "every part is a scene root");

    for (index, node) in roots.iter().enumerate() {
        let mesh = node.mesh().expect("each node carries its mesh");
        assert_eq!(node.index(), index, "nodes follow the parts order");
        let name = mesh.name().expect("the part name travels");
        let expected = if index == 0 { "body" } else { "panty" };
        assert_eq!(name, expected);

        let primitives: Vec<_> = mesh.primitives().collect();
        assert_eq!(primitives.len(), 1, "one primitive per part");
        let primitive = &primitives[0];
        assert_eq!(primitive.mode(), gltf::mesh::Mode::Triangles);
        let material = primitive.material();
        assert_eq!(
            material.name().expect("the material is named"),
            expected,
            "the material keeps the part name"
        );
        let [r, g, b, a] = material.pbr_metallic_roughness().base_color_factor();
        let expected_color = if index == 0 { BODY_COLOR } else { layer };
        for (lane, expected) in [r, g, b, a].iter().zip(expected_color) {
            assert!(
                (lane - expected).abs() < 1e-6,
                "baseColorFactor lane {lane} != {expected}"
            );
        }
    }
}

#[test]
fn positions_carry_bounds_and_indexed_geometry_round_trips() {
    let bytes = write_review_glb(&[part("body", 0.0, BODY_COLOR)]).expect("a GLB");
    let parsed = Gltf::from_slice(&bytes).expect("the GLB parses");
    let primitive = parsed
        .meshes()
        .next()
        .expect("a mesh")
        .primitives()
        .next()
        .expect("a primitive");

    let positions = primitive
        .get(&gltf::Semantic::Positions)
        .expect("POSITION accessor");
    assert_eq!(positions.count(), 3);
    assert_eq!(positions.dimensions(), gltf::accessor::Dimensions::Vec3);
    assert_eq!(
        positions.min().expect("POSITION min"),
        serde_json::json!([0.0, 0.0, 0.0])
    );
    assert_eq!(
        positions.max().expect("POSITION max"),
        serde_json::json!([1.0, 1.0, 0.0])
    );
    let rows = floats(&parsed, &positions, 3);
    assert_eq!(rows[0], vec![0.0, 0.0, 0.0]);

    let normals = primitive
        .get(&gltf::Semantic::Normals)
        .expect("the writer computes normals");
    assert_eq!(normals.count(), 3, "one normal per vertex");

    let indices = primitive.indices().expect("indexed geometry");
    assert_eq!(
        indices.data_type(),
        gltf::accessor::DataType::U32,
        "34,682-vertex bodies exceed u16 indices"
    );
    assert_eq!(indices.count(), 3);
    let index_view = indices.view().expect("the index view");
    let blob = parsed.blob.as_deref().expect("GLB carries the buffer");
    let start = index_view.offset() + indices.offset();
    let read: Vec<u32> = blob[start..start + 4 * indices.count()]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|cell| u32::from_le_bytes(*cell))
        .collect();
    assert_eq!(read, vec![0, 1, 2]);

    let buffers: Vec<_> = parsed.buffers().collect();
    assert_eq!(buffers.len(), 1, "one embedded buffer");
    assert!(buffers[0].length() > 0);
}

#[test]
fn an_empty_parts_list_is_refused() {
    let error = write_review_glb(&[]).expect_err("no parts is a refusal");
    assert_eq!(error, GltfError::ReviewNoParts);
    assert!(error.to_string().contains("no parts"));
}

#[test]
fn a_part_without_geometry_is_refused_before_any_byte() {
    let mut empty = part("panty", 0.0, palette(1).expect("rank 1"));
    empty.triangles.clear();
    let error = write_review_glb(&[part("body", 0.0, BODY_COLOR), empty])
        .expect_err("no triangles is a refusal");
    let GltfError::ReviewPartEmpty { part, what } = error else {
        panic!("expected ReviewPartEmpty, got {error:?}");
    };
    assert_eq!(part, "panty");
    assert_eq!(what, "triangles");

    let vertexless = ReviewPart {
        name: "bra".to_string(),
        positions: Vec::new(),
        triangles: vec![[0, 1, 2]],
        color: palette(1).expect("rank 1"),
    };
    let error = write_review_glb(&[vertexless]).expect_err("no vertices is a refusal");
    let GltfError::ReviewPartEmpty { part, what } = error else {
        panic!("expected ReviewPartEmpty, got {error:?}");
    };
    assert_eq!(part, "bra");
    assert_eq!(what, "vertices");
}
