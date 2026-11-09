//! `read_raw` intake tests: a GLB written by `sculpto_gltf::write_glb`
//! round-trips its geometry, an undersized export normalises back to metres,
//! `.gltf` and `.obj` capture the material, and refusals are typed.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId};
use sculpto_gltf::write_glb;
use sculpto_mesh::intake_gltf::{IntakeError, ScaleReport, ScaleRule, read_raw};

/// A two-triangle garment plane, 1.2 × 1.5, optionally shrunk.
fn synthetic_asset(scale: f32) -> MeshAsset {
    MeshAsset {
        name: "robe".into(),
        positions: [
            [0.0, 0.0, 0.0],
            [1.2, 0.0, 0.0],
            [1.2, 1.5, 0.0],
            [0.0, 1.5, 0.0],
        ]
        .into_iter()
        .map(|position| position.map(|value| value * scale))
        .collect(),
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].to_vec(),
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("sculpto-intake-gltf-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp home");
    root
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::write(path, bytes).expect("write fixture");
    path.to_owned()
}

fn close(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() <= 1e-6
}

fn assert_positions(actual: &[[f32; 3]], expected: &[[f32; 3]]) {
    assert_eq!(actual.len(), expected.len(), "vertex count");
    for (vertex, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (axis, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                close(*actual, *expected),
                "vertex {vertex} axis {axis}: {actual} vs {expected}"
            );
        }
    }
}

fn assert_uvs(actual: &[[f32; 2]], expected: &[[f32; 2]]) {
    assert_eq!(actual.len(), expected.len(), "uv count");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            close(actual[0], expected[0]) && close(actual[1], expected[1]),
            "uv {index}: {actual:?} vs {expected:?}"
        );
    }
}

#[test]
fn glb_roundtrip_preserves_geometry_uvs_and_defaults_the_material() {
    let root = temp_root("glb");
    let asset = synthetic_asset(1.0);
    let bytes = write_glb(&asset).expect("writes");
    let path = write(&root.join("raw.glb"), &bytes);

    let garment = read_raw(&path).expect("reads");

    assert_eq!(garment.name, "robe");
    assert_eq!(garment.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    assert_positions(&garment.positions, &asset.positions);
    assert_uvs(&garment.uvs, &asset.uvs);
    assert_eq!(
        garment.material,
        MaterialSlot {
            name: "default".into(),
            ..MaterialSlot::default()
        },
        "the writer emits no material at this revision, so intake defaults"
    );
    assert_eq!(
        garment.scale,
        ScaleReport {
            rule: ScaleRule::Metres,
            source_extent: 1.5,
            factor: 1.0,
        }
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn undersized_glb_reports_the_factor_and_normalises_back_to_metres() {
    let root = temp_root("scaled");
    let metres = synthetic_asset(1.0);
    let shrunk = synthetic_asset(0.01);
    let bytes = write_glb(&shrunk).expect("writes");
    let path = write(&root.join("small.glb"), &bytes);

    let garment = read_raw(&path).expect("reads");
    let rerun = read_raw(&path).expect("reads again");

    assert_eq!(garment.scale.rule, ScaleRule::UndersizedMetres);
    assert_eq!(garment.scale.source_extent, 0.015);
    assert_eq!(garment.scale.factor, 100.0);
    assert_eq!(rerun.scale, garment.scale, "identical factor on re-run");
    assert_positions(&garment.positions, &metres.positions);
    assert_uvs(&garment.uvs, &metres.uvs);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gltf_json_captures_the_material_and_its_texture_paths() {
    let root = temp_root("gltf");
    let mut bin = Vec::new();
    for [x, y, z] in [[0.0f32, 0.0, 0.0], [1.2, 0.0, 0.0], [0.0, 1.5, 0.0]] {
        bin.extend_from_slice(&x.to_le_bytes());
        bin.extend_from_slice(&y.to_le_bytes());
        bin.extend_from_slice(&z.to_le_bytes());
    }
    for [u, v] in [[0.0f32, 0.0], [1.0, 0.0], [0.0, 1.0]] {
        bin.extend_from_slice(&u.to_le_bytes());
        bin.extend_from_slice(&v.to_le_bytes());
    }
    for index in [0u32, 1, 2] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    write(&root.join("raw.bin"), &bin);
    let json = serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "velvet dress"}],
        "meshes": [{"name": "velvet dress", "primitives": [{
            "attributes": {"POSITION": 0, "TEXCOORD_0": 1},
            "indices": 2,
            "material": 0,
            "mode": 4,
        }]}],
        "materials": [{
            "name": "velvet",
            "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}},
            "normalTexture": {"index": 1},
        }],
        "textures": [{"source": 0}, {"source": 1}],
        "images": [{"uri": "velvet_base.png"}, {"uri": "velvet_n.png"}],
        "buffers": [{"uri": "raw.bin", "byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 24},
            {"buffer": 0, "byteOffset": 60, "byteLength": 12},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3,
             "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.2, 1.5, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5125, "count": 3, "type": "SCALAR"},
        ],
    });
    let path = write(
        &root.join("dress.gltf"),
        serde_json::to_string_pretty(&json).unwrap().as_bytes(),
    );

    let garment = read_raw(&path).expect("reads");

    assert_eq!(garment.name, "velvet dress");
    assert_eq!(garment.triangles, vec![[0, 1, 2]]);
    assert_eq!(garment.positions.len(), 3);
    assert_positions(
        &garment.positions,
        &[[0.0, 0.0, 0.0], [1.2, 0.0, 0.0], [0.0, 1.5, 0.0]],
    );
    assert_uvs(&garment.uvs, &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    assert!(garment.normals.is_empty(), "no NORMAL accessor, no normals");
    assert_eq!(
        garment.material,
        MaterialSlot {
            name: "velvet".into(),
            diffuse: Some("velvet_base.png".into()),
            normal: Some("velvet_n.png".into()),
            ..MaterialSlot::default()
        }
    );
    assert_eq!(
        garment.scale,
        ScaleReport {
            rule: ScaleRule::Metres,
            source_extent: 1.5,
            factor: 1.0,
        }
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn obj_captures_the_material_and_fan_triangulates_faces() {
    let root = temp_root("obj");
    let obj = "# sculpto intake fixture\n\
               mtllib robe.mtl\n\
               o robe\n\
               usemtl velvet\n\
               v 0 0 0\n\
               v 1.2 0 0\n\
               v 1.2 1.5 0\n\
               v 0 1.5 0\n\
               vt 0 0\n\
               vt 1 0\n\
               vt 1 1\n\
               vt 0 1\n\
               vn 0 0 1\n\
               vn 0 0 1\n\
               vn 0 0 1\n\
               vn 0 0 1\n\
               f 1/1/1 2/2/2 3/3/3\n\
               f 1/1/1 3/3/3 4/4/4 2/2/2\n";
    let mtl = "newmtl velvet\n\
               map_Kd velvet_base.png\n\
               map_Bump velvet_n.png\n";
    write(&root.join("raw.obj"), obj.as_bytes());
    write(&root.join("robe.mtl"), mtl.as_bytes());

    let garment = read_raw(&root.join("raw.obj")).expect("reads");

    assert_eq!(garment.positions.len(), 4);
    assert_eq!(
        garment.triangles,
        vec![[0, 1, 2], [0, 2, 3], [0, 3, 1]],
        "the quad fans around its first corner"
    );
    assert_positions(
        &garment.positions,
        &[
            [0.0, 0.0, 0.0],
            [1.2, 0.0, 0.0],
            [1.2, 1.5, 0.0],
            [0.0, 1.5, 0.0],
        ],
    );
    assert_uvs(
        &garment.uvs,
        &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    );
    assert_eq!(garment.normals, vec![[0.0, 0.0, 1.0]; 4]);
    assert_eq!(
        garment.material,
        MaterialSlot {
            name: "velvet".into(),
            diffuse: Some("velvet_base.png".into()),
            normal: Some("velvet_n.png".into()),
            ..MaterialSlot::default()
        }
    );
    assert_eq!(garment.scale.factor, 1.0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn empty_and_unsupported_files_are_refused() {
    let root = temp_root("refusals");

    let empty_obj = write(&root.join("blank.obj"), b"");
    let empty_glb = write(&root.join("blank.glb"), b"");
    let unsupported = write(&root.join("model.fbx"), b"not a mesh we read");

    assert!(matches!(
        read_raw(&empty_obj),
        Err(IntakeError::Empty { .. })
    ));
    assert!(matches!(
        read_raw(&empty_glb),
        Err(IntakeError::Empty { .. })
    ));
    match read_raw(&unsupported) {
        Err(IntakeError::UnsupportedExtension { extension }) => {
            assert_eq!(extension, "fbx");
        }
        other => panic!("expected UnsupportedExtension, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}
