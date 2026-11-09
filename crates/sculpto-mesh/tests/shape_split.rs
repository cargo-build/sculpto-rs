//! The produced garment must fit the NIF SE per-shape vertex ceiling.
//!
//! Private-host tier: the 154 629-vertex garment comes from the operator's
//! combined GLB. The test is ignored by default; run it with
//! `SCULPTO_GARMENT_GLB=<path> cargo test -p sculpto-mesh --test shape_split -- --ignored`.

use std::path::PathBuf;

use sculpto_core::mesh::{MeshAsset, MorphDelta, MorphTarget, PartitionId};

const SE_VERTEX_LIMIT: usize = 65_535;

fn fixture() -> PathBuf {
    std::env::var("SCULPTO_GARMENT_GLB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.tmp/robe-combined.glb")
        })
}

#[test]
#[ignore = "private host fixture: the produced garment GLB"]
fn produced_garment_splits_into_se_legal_shapes() {
    let path = fixture();
    assert!(path.exists(), "missing fixture: {}", path.display());
    let bytes = std::fs::read(&path).expect("read GLB bytes");
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
            };
            if best
                .as_ref()
                .is_none_or(|current| asset.positions.len() > current.positions.len())
            {
                best = Some(asset);
            }
        }
    }
    let garment = best.expect("a mesh in the GLB");
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
