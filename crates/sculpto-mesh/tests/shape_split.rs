//! The produced garment must fit the NIF SE per-shape vertex ceiling and match
//! its golden descriptor.
//!
//! Private-host tier: the 154 629-vertex garment comes from the operator's
//! combined GLB. The split test is ignored by default; run it with
//! `SCULPTO_GARMENT_GLB=<path> cargo test -p sculpto-mesh --test shape_split -- --ignored`.
//! The golden test runs whenever `SCULPTO_GARMENT_GLB` is set.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, MorphDelta, MorphTarget, PartitionId};

const SE_VERTEX_LIMIT: usize = 65_535;

fn fixture() -> PathBuf {
    std::env::var("SCULPTO_GARMENT_GLB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.tmp/robe-combined.glb")
        })
}

fn garment_asset(path: &Path) -> MeshAsset {
    let bytes = std::fs::read(path).expect("read GLB bytes");
    let gltf = gltf::Gltf::from_slice(&bytes).expect("parse GLB");
    let document = &gltf.document;
    let blob = gltf.blob.as_deref();
    let mut best: Option<MeshAsset> = None;
    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| match buffer.source() {
                gltf::buffer::Source::Bin => blob,
                _ => None,
            });
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();
            let Some(indices) = reader.read_indices() else {
                continue;
            };
            let indices: Vec<u32> = indices.into_u32().collect();
            let (chunks, _rest) = indices.as_chunks::<3>();
            let triangles: Vec<[u32; 3]> = chunks.to_vec();
            let mut morphs = Vec::new();
            for (target, (delta_positions, _, _)) in reader.read_morph_targets().enumerate() {
                let deltas = delta_positions
                    .map(|iter| {
                        iter.enumerate()
                            .filter_map(|(vertex, delta)| {
                                (delta != [0.0; 3]).then_some(MorphDelta {
                                    vertex: vertex as u32,
                                    position: delta,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                morphs.push(MorphTarget {
                    name: format!("target{target}"),
                    deltas,
                });
            }
            let triangle_count = triangles.len();
            let asset = MeshAsset {
                positions,
                triangles,
                triangle_partitions: vec![PartitionId(0); triangle_count],
                morphs,
                ..MeshAsset::default()
            };
            if best
                .as_ref()
                .is_none_or(|current| asset.positions.len() > current.positions.len())
            {
                best = Some(asset);
            }
        }
    }
    best.expect("a mesh in the GLB")
}

fn fnv1a64(positions: &[[f32; 3]]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for position in positions {
        for value in position {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    format!("{hash:016x}")
}

#[test]
#[ignore = "private host fixture: the produced garment GLB"]
fn produced_garment_splits_into_se_legal_shapes() {
    let path = fixture();
    assert!(path.exists(), "missing fixture: {}", path.display());
    let garment = garment_asset(&path);
    assert_eq!(garment.positions.len(), 154_629);
    assert_eq!(garment.morphs.len(), 37);
    let triangles = garment.triangles.len();
    let nonzero_deltas: usize = garment
        .morphs
        .iter()
        .map(|target| target.deltas.len())
        .sum();

    let shapes = garment.split_for_limit(SE_VERTEX_LIMIT).expect("split");
    assert_eq!(shapes.len(), 3);
    for shape in &shapes {
        assert!(shape.positions.len() <= SE_VERTEX_LIMIT);
        assert_eq!(shape.morphs.len(), 37);
        shape.validate().expect("valid shape");
    }
    assert_eq!(
        shapes
            .iter()
            .map(|shape| shape.triangles.len())
            .sum::<usize>(),
        triangles
    );
    assert!(
        shapes
            .iter()
            .flat_map(|shape| &shape.morphs)
            .map(|target| target.deltas.len())
            .sum::<usize>()
            >= nonzero_deltas
    );
}

#[test]
fn produced_garment_matches_the_golden_descriptor() {
    let Ok(path) = std::env::var("SCULPTO_GARMENT_GLB") else {
        return;
    };
    let path = PathBuf::from(path);
    let garment = garment_asset(&path);
    garment.validate().expect("golden garment validates");
    let actual = format!(
        "vertices={}\ntriangles={}\nmorphs={}\npositions_fnv1a64={}\n",
        garment.positions.len(),
        garment.triangles.len(),
        garment.morphs.len(),
        fnv1a64(&garment.positions),
    );
    let descriptor =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/garment-golden.txt");
    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        std::fs::create_dir_all(descriptor.parent().expect("parent")).expect("fixture directory");
        std::fs::write(descriptor, &actual).expect("write descriptor");
        return;
    }
    let expected = std::fs::read_to_string(descriptor).expect("golden descriptor is committed");
    assert_eq!(
        actual, expected,
        "produced garment drifted from the golden fixture"
    );
}
