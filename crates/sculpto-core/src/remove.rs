//! Zap vertex deletion: remove vertices from every attribute and index space.
//!
//! [`MeshAsset::remove_vertices`] is the model twin of nifly's
//! `notifyVerticesDelete` cascade (NifFile.cpp:4411-4490): the arena drops the
//! named rows, triangles that reference a deleted vertex are removed rather
//! than repaired, partition slots whose triangles all died leave the table,
//! morph deltas renumber and skin influence rows follow the kept vertices.
//! The caller decides what a fully emptied shape means: hidden under
//! `KeepZappedShapes` or dropped from the file (BodySlideApp.cpp:4422-4432).

use std::collections::BTreeSet;

use crate::mesh::skin_weight_error;
use crate::mesh::{MeshAsset, MeshError, MorphDelta, MorphScope, MorphTarget, PartitionSlot, Skin};

/// One removed-vertex view over an attribute array: the kept rows, in order.
fn kept_rows<'a, T: Copy>(values: &'a [T], renumber: &'a [u32]) -> impl Iterator<Item = T> + 'a {
    values
        .iter()
        .enumerate()
        .filter_map(|(old, value)| (renumber[old] != u32::MAX).then_some(*value))
}

impl MeshAsset {
    /// Remove the vertices `sorted` names from every attribute and index
    /// space. @req BLD-01
    ///
    /// `sorted` must be strictly ascending and must name existing vertices;
    /// anything else is refused instead of repaired. Positions, UVs, normals
    /// and tangents drop the named rows; a triangle that references a deleted
    /// vertex is dropped and the survivors are renumbered, never merged;
    /// partition slots whose triangles all died leave the table, mirroring
    /// `RemoveEmptyPartitions`; morph deltas on deleted vertices disappear
    /// while the rest renumber (a target scoped to a removed partition goes
    /// with it); skin influence rows follow the kept vertices with their
    /// values unchanged.
    ///
    /// Deleting every vertex, or every triangle, returns an empty mesh rather
    /// than an error: the caller decides whether a fully zapped shape is
    /// hidden (`KeepZappedShapes`) or dropped from the file
    /// (BodySlideApp.cpp:4422-4432 — `DeleteVertsForShape` reports
    /// `allVertsDeleted` for an empty vertex or triangle set).
    ///
    /// # Errors
    /// [`MeshError::RemoveIndicesNotSorted`] when the list is not strictly
    /// ascending, [`MeshError::RemoveIndexOutOfRange`] when it names a vertex
    /// the arena does not have, and the mesh's own validation errors when the
    /// input model is unsound.
    pub fn remove_vertices(&self, sorted: &[u16]) -> Result<Self, MeshError> {
        self.validate()?;
        for pair in sorted.windows(2) {
            if pair[0] >= pair[1] {
                return Err(MeshError::RemoveIndicesNotSorted);
            }
        }
        if let Some(index) = sorted
            .iter()
            .find(|index| usize::from(**index) >= self.positions.len())
        {
            return Err(MeshError::RemoveIndexOutOfRange {
                index: *index,
                vertices: self.positions.len(),
            });
        }

        // Deleted slots keep the sentinel; kept slots get their new index,
        // which the loop makes strictly increasing with the old order.
        let mut deleted = sorted.iter().copied();
        let mut next_deleted = deleted.next();
        let mut kept_old: Vec<u32> = Vec::new();
        let mut renumber = vec![u32::MAX; self.positions.len()];
        for old in 0..self.positions.len() as u32 {
            if next_deleted == Some(old as u16) {
                next_deleted = deleted.next();
                continue;
            }
            renumber[old as usize] = kept_old.len() as u32;
            kept_old.push(old);
        }

        let positions = kept_rows(&self.positions, &renumber).collect();
        let normals = kept_rows(&self.normals, &renumber).collect();
        let uvs = kept_rows(&self.uvs, &renumber).collect();
        let tangents = kept_rows(&self.tangents, &renumber).collect();

        let mut triangles = Vec::new();
        let mut triangle_partitions = Vec::new();
        for (triangle, partition) in self.triangles.iter().zip(&self.triangle_partitions) {
            let mapped = triangle.map(|vertex| renumber[vertex as usize]);
            if mapped.contains(&u32::MAX) {
                continue;
            }
            triangles.push(mapped);
            triangle_partitions.push(*partition);
        }
        // A slot without triangles describes a partition the deletion emptied;
        // native drops it from the dismember table instead of writing it empty.
        let used: BTreeSet<_> = triangle_partitions.iter().copied().collect();
        let partitions: Vec<PartitionSlot> = self
            .partitions
            .iter()
            .filter(|slot| used.contains(&slot.partition))
            .copied()
            .collect();
        let morphs = self
            .morphs
            .iter()
            .filter(|target| match target.scope {
                MorphScope::Whole => true,
                MorphScope::Part(part) => used.contains(&part),
            })
            .map(|target| MorphTarget {
                name: target.name.clone(),
                deltas: target
                    .deltas
                    .iter()
                    .filter_map(|delta| {
                        let new = renumber[delta.vertex as usize];
                        (new != u32::MAX).then_some(MorphDelta {
                            vertex: new,
                            position: delta.position,
                        })
                    })
                    .collect(),
                scope: target.scope,
            })
            .collect();
        let skin = match &self.skin {
            Some(skin) => Some(Skin {
                bones: skin.bones.clone(),
                bind_inverse: skin.bind_inverse.clone(),
                influences: skin
                    .influences
                    .gather(&kept_old)
                    .map_err(skin_weight_error)?,
            }),
            None => None,
        };

        Ok(Self {
            name: self.name.clone(),
            positions,
            normals,
            uvs,
            tangents,
            triangles,
            triangle_partitions,
            partitions,
            skin,
            morphs,
            materials: self.materials.clone(),
            physics: self.physics.clone(),
            transform: self.transform,
            hidden: self.hidden,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::PartitionId;
    use crate::skin::SkinWeights;

    /// Two quad-strip partitions over eight vertices: 0-3 carry partition 1,
    /// 4-7 carry partition 7, with per-vertex attributes, a two-influence skin
    /// and one whole-mesh plus one partition-7 morph target.
    fn mesh() -> MeshAsset {
        MeshAsset {
            name: "Body".into(),
            positions: (0..8).map(|i| [i as f32, 0.0, 1.0]).collect(),
            normals: (0..8).map(|i| [0.0, i as f32, 1.0]).collect(),
            uvs: (0..8).map(|i| [i as f32 / 8.0, 0.5]).collect(),
            tangents: (0..8).map(|i| [1.0, 0.0, 0.0, i as f32]).collect(),
            triangles: vec![[0, 1, 2], [1, 3, 2], [4, 5, 6], [5, 7, 6]],
            triangle_partitions: vec![
                PartitionId(1),
                PartitionId(1),
                PartitionId(7),
                PartitionId(7),
            ],
            partitions: vec![
                PartitionSlot {
                    partition: PartitionId(1),
                    part_flag: 0,
                    body_part: 32,
                },
                PartitionSlot {
                    partition: PartitionId(7),
                    part_flag: 0,
                    body_part: 3,
                },
            ],
            skin: Some(Skin {
                bones: vec!["root".into(), "spine".into()],
                bind_inverse: Vec::new(),
                influences: SkinWeights::from_quads(
                    &[
                        [0, 1, 0, 0],
                        [1, 0, 0, 0],
                        [0, 0, 0, 0],
                        [1, 1, 0, 0],
                        [0, 1, 0, 0],
                        [1, 0, 0, 0],
                        [0, 0, 0, 0],
                        [1, 1, 0, 0],
                    ],
                    &[
                        [0.5, 0.5, 0.0, 0.0],
                        [0.4, 0.6, 0.0, 0.0],
                        [1.0, 0.0, 0.0, 0.0],
                        [0.3, 0.7, 0.0, 0.0],
                        [0.5, 0.5, 0.0, 0.0],
                        [0.4, 0.6, 0.0, 0.0],
                        [1.0, 0.0, 0.0, 0.0],
                        [0.3, 0.7, 0.0, 0.0],
                    ],
                )
                .expect("eight four-lane rows"),
            }),
            morphs: vec![
                MorphTarget {
                    name: "whole".into(),
                    deltas: vec![
                        MorphDelta {
                            vertex: 1,
                            position: [0.1, 0.0, 0.0],
                        },
                        MorphDelta {
                            vertex: 2,
                            position: [0.0, 0.1, 0.0],
                        },
                    ],
                    scope: MorphScope::Whole,
                },
                MorphTarget {
                    name: "part7".into(),
                    deltas: vec![MorphDelta {
                        vertex: 5,
                        position: [0.0, 0.0, 0.2],
                    }],
                    scope: MorphScope::Part(PartitionId(7)),
                },
            ],
            ..MeshAsset::default()
        }
    }

    #[test]
    fn middle_indices_drop_rows_triangles_and_deltas() {
        let source = mesh();
        let kept = source.remove_vertices(&[2, 3]).expect("removes");
        // Rows 0,1,4,5,6,7 survive, aligned across every attribute.
        assert_eq!(
            kept.positions,
            vec![
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [4.0, 0.0, 1.0],
                [5.0, 0.0, 1.0],
                [6.0, 0.0, 1.0],
                [7.0, 0.0, 1.0]
            ]
        );
        assert_eq!(kept.uvs[0], [0.0, 0.5]);
        assert_eq!(kept.uvs[2], [4.0 / 8.0, 0.5]);
        assert_eq!(kept.normals[2], [0.0, 4.0, 1.0]);
        assert_eq!(kept.tangents[2], [1.0, 0.0, 0.0, 4.0]);
        // Triangles touching 2 or 3 die; the partition-7 pair renumbers.
        assert_eq!(kept.triangles, vec![[2, 3, 4], [3, 5, 4]]);
        assert_eq!(
            kept.triangle_partitions,
            vec![PartitionId(7), PartitionId(7)]
        );
        assert_eq!(kept.partitions.len(), 1);
        assert_eq!(kept.partitions[0].partition, PartitionId(7));
        // The whole-mesh morph loses vertex 2 and renumbers vertex 1's delta;
        // the partition-7 target keeps its delta on renumbered vertex 3.
        assert_eq!(kept.morphs.len(), 2);
        assert_eq!(
            kept.morphs[0].deltas,
            vec![MorphDelta {
                vertex: 1,
                position: [0.1, 0.0, 0.0]
            }]
        );
        assert_eq!(
            kept.morphs[1].deltas,
            vec![MorphDelta {
                vertex: 3,
                position: [0.0, 0.0, 0.2]
            }]
        );
        kept.validate().expect("valid deletion");
    }

    #[test]
    fn edge_indices_remove_head_and_tail() {
        let kept = mesh().remove_vertices(&[0, 7]).expect("removes");
        assert_eq!(kept.positions.len(), 6);
        assert_eq!(kept.positions.first(), Some(&[1.0, 0.0, 1.0]));
        assert_eq!(kept.positions.last(), Some(&[6.0, 0.0, 1.0]));
        // Triangles [0,1,2] and [5,7,6] die; [1,3,2] and [4,5,6] renumber.
        assert_eq!(kept.triangles, vec![[0, 2, 1], [3, 4, 5]]);
        kept.validate().expect("valid deletion");
    }

    #[test]
    fn all_indices_empty_the_mesh_without_error() {
        let kept = mesh()
            .remove_vertices(&[0, 1, 2, 3, 4, 5, 6, 7])
            .expect("a full zap is a caller decision, not an error");
        assert!(kept.positions.is_empty());
        assert!(kept.triangles.is_empty());
        assert!(kept.partitions.is_empty());
        let skin = kept.skin.as_ref().expect("skin survives empty");
        assert_eq!(skin.influences.rows(), 0);
    }

    #[test]
    fn all_triangles_dying_leaves_vertices_but_no_partitions() {
        // Deleting 2,3,6,7 leaves four vertices and no triangle at all, the
        // native `allVertsDeleted` condition for an emptied triangle set.
        let kept = mesh().remove_vertices(&[2, 3, 6, 7]).expect("removes");
        assert_eq!(kept.positions.len(), 4);
        assert!(kept.triangles.is_empty());
        assert!(kept.partitions.is_empty());
        assert!(kept.triangle_partitions.is_empty());
    }

    #[test]
    fn skin_rows_follow_kept_vertices_with_values_intact() {
        let kept = mesh().remove_vertices(&[1, 4]).expect("removes");
        let skin = kept.skin.as_ref().expect("skin survives");
        assert_eq!(skin.influences.rows(), 6);
        // Row 0 is old vertex 0, unchanged; row 1 is old vertex 2, whose lone
        // influence and weight move without renormalization.
        let row = skin.influences.row(0).expect("row 0 is old vertex 0");
        assert_eq!(row.joints, &[0, 1]);
        assert_eq!(row.weights, &[0.5, 0.5]);
        let row = skin.influences.row(1).expect("row 1 is old vertex 2");
        assert_eq!(row.joints, &[0]);
        assert_eq!(row.weights, &[1.0]);
        kept.validate().expect("valid deletion");
    }

    #[test]
    fn part_scoped_morphs_follow_their_removed_partition_out() {
        // Zapping 4..=7 kills every partition-7 triangle, so a target scoped
        // to partition 7 has no home and goes with it.
        let kept = mesh().remove_vertices(&[4, 5, 6, 7]).expect("removes");
        let names: Vec<&str> = kept.morphs.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["whole"]);
        kept.validate().expect("valid deletion");
    }

    #[test]
    fn unsorted_duplicate_and_unknown_indices_are_refused() {
        let source = mesh();
        assert_eq!(
            source.remove_vertices(&[3, 1]),
            Err(MeshError::RemoveIndicesNotSorted)
        );
        assert_eq!(
            source.remove_vertices(&[1, 1]),
            Err(MeshError::RemoveIndicesNotSorted)
        );
        assert_eq!(
            source.remove_vertices(&[8]),
            Err(MeshError::RemoveIndexOutOfRange {
                index: 8,
                vertices: 8
            })
        );
        // A single in-range index is fine, and an empty list is a no-op copy.
        assert_eq!(
            source
                .remove_vertices(&[5])
                .expect("removes")
                .positions
                .len(),
            7
        );
        assert_eq!(
            source.remove_vertices(&[]).expect("empty removes nothing"),
            source
        );
    }
}
