//! Deterministic property tests over the export model.
//!
//! The generator is a small LCG so every failure is reproducible from its seed;
//! no external property crate is needed. Each property also has a deliberately
//! corrupted fixture, so a vacuous pass is visible.

use std::collections::BTreeSet;

use sculpto_core::mesh::{
    MaterialSlot, MeshAsset, MeshError, MorphDelta, MorphTarget, PartitionId, PhysicsRef, Skin,
};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(0x9e37_79b9_7f4a_7c15 ^ seed)
    }

    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn range(&mut self, low: u32, high: u32) -> u32 {
        low + self.next() % (high - low)
    }

    fn signed(&mut self) -> f32 {
        self.next() as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

fn generated(seed: u64) -> MeshAsset {
    let mut rng = Rng::new(seed);
    let vertices = 3 + rng.range(0, 120) as usize;
    let positions = (0..vertices)
        .map(|_| [rng.signed(), rng.signed(), rng.signed()])
        .collect();
    let triangles: Vec<[u32; 3]> = (0..1 + rng.range(0, 200))
        .map(|_| {
            [
                rng.range(0, vertices as u32),
                rng.range(0, vertices as u32),
                rng.range(0, vertices as u32),
            ]
        })
        .collect();
    let triangle_partitions = triangles
        .iter()
        .map(|_| PartitionId(rng.range(0, 3) as u16))
        .collect();
    let referenced: Vec<u32> = triangles
        .iter()
        .flat_map(|triangle| triangle.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let morphs = (0..1 + rng.range(0, 4))
        .map(|target| {
            let mut chosen: Vec<u32> = referenced
                .iter()
                .copied()
                .filter(|_| rng.next().is_multiple_of(3))
                .collect();
            if chosen.is_empty() {
                chosen.push(referenced[0]);
            }
            let deltas = chosen
                .into_iter()
                .map(|vertex| MorphDelta {
                    vertex,
                    position: [rng.signed(), rng.signed(), rng.signed()],
                })
                .collect();
            MorphTarget {
                name: format!("target{target}"),
                deltas,
            }
        })
        .collect();
    let skin = Some(Skin {
        bones: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        bind_inverse: Vec::new(),
        indices: (0..vertices)
            .map(|_| {
                [
                    rng.range(0, 4) as u16,
                    rng.range(0, 4) as u16,
                    rng.range(0, 4) as u16,
                    rng.range(0, 4) as u16,
                ]
            })
            .collect(),
        weights: (0..vertices)
            .map(|_| {
                [
                    rng.range(0, 100) as f32,
                    rng.range(0, 100) as f32,
                    rng.range(0, 100) as f32,
                    rng.range(0, 100) as f32,
                ]
            })
            .collect(),
    });
    let mut mesh = MeshAsset {
        name: format!("seed{seed}"),
        positions,
        triangles,
        triangle_partitions,
        skin,
        morphs,
        materials: vec![MaterialSlot {
            name: "slot".into(),
            ..MaterialSlot::default()
        }],
        physics: Some(PhysicsRef::Path("sidecar.json".into())),
        ..MeshAsset::default()
    };
    mesh.normalize_weights()
        .expect("generated weights are positive");
    mesh
}

fn partition_counts(mesh: &MeshAsset) -> Vec<(PartitionId, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for partition in &mesh.triangle_partitions {
        *counts.entry(*partition).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

#[test]
fn split_preserves_triangles_and_partition_counts() {
    for seed in 0..64 {
        let mesh = generated(seed);
        mesh.validate().expect("generated mesh validates");
        let limit = 3 + (seed as usize % 40);
        let shapes = mesh.split_for_limit(limit).expect("splits");
        assert_eq!(
            shapes
                .iter()
                .map(|shape| shape.triangles.len())
                .sum::<usize>(),
            mesh.triangles.len(),
            "seed {seed}: triangle count"
        );
        let expected = partition_counts(&mesh);
        let mut actual = std::collections::BTreeMap::new();
        for shape in &shapes {
            assert!(shape.positions.len() <= limit, "seed {seed}: limit");
            shape.validate().expect("split shape validates");
            for partition in &shape.triangle_partitions {
                *actual.entry(*partition).or_insert(0) += 1;
            }
        }
        assert_eq!(
            actual.into_iter().collect::<Vec<_>>(),
            expected,
            "seed {seed}: partition counts"
        );
    }
}

#[test]
fn split_index_mapping_is_stable() {
    for seed in 0..32 {
        let mesh = generated(seed);
        let original: BTreeSet<[u32; 3]> = mesh
            .positions
            .iter()
            .map(|position| position.map(f32::to_bits))
            .collect();
        for shape in mesh
            .split_for_limit(4 + (seed as usize % 20))
            .expect("splits")
        {
            for position in &shape.positions {
                assert!(
                    original.contains(&position.map(f32::to_bits)),
                    "seed {seed}: split invented a position"
                );
            }
            for target in &shape.morphs {
                for delta in &target.deltas {
                    assert!((delta.vertex as usize) < shape.positions.len());
                }
            }
        }
    }
}

#[test]
fn weight_normalisation_is_idempotent() {
    for seed in 0..64 {
        let mut mesh = generated(seed);
        mesh.normalize_weights().expect("normalizes");
        mesh.validate().expect("normalized weights validate");
        let before = mesh.skin.as_ref().expect("skin").weights.clone();
        mesh.normalize_weights().expect("normalizes again");
        let after = mesh.skin.as_ref().expect("skin").weights.clone();
        for (vertex, (before, after)) in before.iter().zip(&after).enumerate() {
            for (left, right) in before.iter().zip(after) {
                assert!(
                    (left - right).abs() <= 1e-5,
                    "seed {seed}, vertex {vertex}: normalization is not idempotent"
                );
            }
        }
    }
}

#[test]
fn corrupted_fixtures_fail() {
    let mut mesh = generated(7);
    let vertex = mesh.positions.len() as u32;
    mesh.triangles[0][0] = vertex;
    assert_eq!(
        mesh.validate(),
        Err(MeshError::TriangleVertexOutOfRange {
            triangle: 0,
            vertex
        })
    );

    let mut mesh = generated(8);
    mesh.skin.as_mut().expect("skin").weights[0] = [0.0; 4];
    assert!(matches!(
        mesh.validate(),
        Err(MeshError::WeightSumOutOfRange { vertex: 0, .. })
    ));
    assert_eq!(
        mesh.normalize_weights(),
        Err(MeshError::ZeroWeightSum { vertex: 0 })
    );

    let mut mesh = generated(9);
    mesh.morphs[0].deltas[0].vertex = mesh.positions.len() as u32;
    assert!(matches!(
        mesh.validate(),
        Err(MeshError::MorphVertexOutOfRange { target: 0, .. })
    ));
}
