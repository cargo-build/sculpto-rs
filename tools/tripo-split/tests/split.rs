//! Split a synthetic three-figure GLB and verify the manifest contract.

use std::fs;
use std::path::PathBuf;

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId};

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "tripo-split-{}-{label}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp dir");
    path
}

fn source_glb(path: &PathBuf) {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [0.2, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.1, 0.0, 0.0],
        [10.2, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.1, 0.0, 0.0],
        [20.2, 0.0, 0.0],
    ];
    let triangles = vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]];
    let asset = MeshAsset {
        name: "three figures".to_string(),
        positions,
        triangles: triangles.clone(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        materials: vec![MaterialSlot {
            name: "default".to_string(),
            ..MaterialSlot::default()
        }],
        ..MeshAsset::default()
    };
    fs::write(path, sculpto_gltf::write_glb(&asset).expect("write glb")).expect("source");
}

#[test]
fn writes_one_glb_and_manifest_row_per_figure() {
    let root = temp_dir("three-figures");
    let source = root.join("three figures.glb");
    source_glb(&source);
    let out = root.join("out");
    let manifest = root.join("parts.jsonl");
    let report = tripo_split::split_glb(&source, &out).expect("split");
    assert_eq!(report.parts.len(), 3);
    for (index, part) in report.parts.iter().enumerate() {
        assert_eq!(part.part, format!("figure-{:02}", index + 1));
        assert_eq!(part.vertices, 3);
        assert_eq!(part.triangles, 1);
        assert!(part.path.is_file(), "{}", part.path.display());
        let raw = sculpto_mesh::intake_gltf::read_raw(&part.path).expect("read part");
        assert_eq!(raw.positions.len(), 3);
        assert_eq!(raw.triangles.len(), 1);
    }
    tripo_split::write_manifest(&report, &manifest, "tripo3d-parts").expect("manifest");
    let text = fs::read_to_string(&manifest).expect("manifest text");
    assert_eq!(text.lines().count(), 3);
    for line in text.lines() {
        let row: serde_json::Value = serde_json::from_str(line).expect("row");
        assert_eq!(row["category"], "mesh");
        assert_eq!(row["source"], "tripo3d-parts");
        assert_eq!(row["vertices"], 3);
        assert_eq!(row["triangles"], 1);
        assert!(
            row["source_sha256"]
                .as_str()
                .is_some_and(|value| value.len() == 64)
        );
        assert!(
            row["tags"]
                .as_array()
                .expect("tags")
                .iter()
                .any(|tag| tag == "raw:part")
        );
    }
    let _ = fs::remove_dir_all(root);
}
