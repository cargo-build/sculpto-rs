//! Integration tests for `MorphScope`: the `.tri` projection narrows to the
//! shape that owns a `Part` target, while the whole-arena projection keeps
//! every target.

use sculpto_core::mesh::{MeshAsset, MeshError, MorphDelta, MorphScope, MorphTarget, PartitionId};

/// A two-partition triangle pair: partition 0 owns vertex 0, partition 1 owns
/// vertices 1 and 2.
fn two_parts() -> MeshAsset {
    MeshAsset {
        name: "two_parts".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2], [1, 2, 0]],
        triangle_partitions: vec![PartitionId(0), PartitionId(1)],
        ..MeshAsset::default()
    }
}

/// The two targets under test: one whole-mesh, one scoped to partition 0.
fn whole_and_part(part: PartitionId) -> Vec<MorphTarget> {
    vec![
        MorphTarget {
            name: "whole".into(),
            deltas: vec![MorphDelta {
                vertex: 0,
                position: [0.0, 0.0, 1.0],
            }],
            scope: MorphScope::Whole,
        },
        MorphTarget {
            name: "part".into(),
            deltas: vec![MorphDelta {
                vertex: 1,
                position: [0.0, 1.0, 0.0],
            }],
            scope: MorphScope::Part(part),
        },
    ]
}

/// The morph names a projected `.tri` shape carries.
fn names(shape: &sculpto_core::mesh::TriShape) -> Vec<&str> {
    shape
        .morphs
        .iter()
        .map(|morph| morph.name.as_str())
        .collect()
}

#[test]
fn a_part_projection_carries_whole_and_owning_part_only() {
    let mut asset = two_parts();
    asset.morphs = whole_and_part(PartitionId(0));
    asset.validate().expect("both partitions exist");

    let own = asset
        .tri_shape_for_part(PartitionId(0))
        .expect("projection");
    assert_eq!(
        names(&own),
        ["whole", "part"],
        "partition 0's shape carries the Whole target and its own Part target"
    );

    let other = asset
        .tri_shape_for_part(PartitionId(1))
        .expect("projection");
    assert_eq!(
        names(&other),
        ["whole"],
        "partition 1's shape carries only the Whole target"
    );
}

#[test]
fn tri_shape_keeps_every_target_regardless_of_scope() {
    let mut asset = two_parts();
    asset.morphs = whole_and_part(PartitionId(0));

    let shape = asset.tri_shape().expect("projection");
    assert_eq!(
        names(&shape),
        ["whole", "part"],
        "the whole-arena projection keeps targets scoped to any partition"
    );
}

#[test]
fn a_part_target_on_an_unknown_partition_is_refused() {
    let mut asset = two_parts();
    asset.morphs = whole_and_part(PartitionId(9));
    assert_eq!(
        asset.validate(),
        Err(MeshError::MorphUnknownPart {
            target: 1,
            part: PartitionId(9),
        }),
        "validation names the morph index and the part id"
    );
}
