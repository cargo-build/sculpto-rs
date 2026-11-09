use sculpto_core::mesh::{MeshAsset, MorphDelta, MorphTarget, PartitionId, Skin};
use sculpto_gltf::write_glb;

fn tier1_fixture_asset() -> MeshAsset {
    MeshAsset {
        name: "Tier1 Body".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0), PartitionId(0)],
        ..MeshAsset::default()
    }
}

fn synthetic_skinned_asset() -> MeshAsset {
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
            indices: vec![[0, 1, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]],
            weights: vec![
                [0.7, 0.3, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
            ],
        }),
        ..MeshAsset::default()
    }
}

fn synthetic_morph_asset() -> MeshAsset {
    MeshAsset {
        name: "morph_body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        morphs: vec![
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
        ],
        ..MeshAsset::default()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (kind, out_path) = match args.as_slice() {
        [k, p] if k == "--tier1" => ("tier1", p.as_str()),
        [k, p] if k == "--morph" => ("morph", p.as_str()),
        [k, p] if k == "--synthetic" => ("synthetic", p.as_str()),
        [p] => ("synthetic", p.as_str()),
        _ => ("synthetic", "/tmp/synthetic_skinned.glb"),
    };

    let asset = match kind {
        "tier1" => tier1_fixture_asset(),
        "morph" => synthetic_morph_asset(),
        _ => synthetic_skinned_asset(),
    };

    let bytes = write_glb(&asset).expect("write_glb succeeds");
    std::fs::write(out_path, bytes).expect("write file succeeds");
    println!("Generated {kind} GLB at: {out_path}");
}
