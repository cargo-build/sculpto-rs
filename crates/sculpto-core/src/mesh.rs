//! Export mesh model and the shape-split projection.
//!
//! The model is one flat vertex arena with per-triangle partition ids and sparse
//! morph deltas. It carries no format types: NIF, `.tri` and glTF are
//! projections over this value.
//!
//! A NIF shape addresses vertices with `u16`, so a part whose triangle set
//! references more than 65 535 vertices cannot be one shape. [`MeshAsset::split_for_limit`]
//! is the pure projection that splits such a part into deterministic sub-shapes
//! while preserving partition ids and morph deltas.

use std::collections::BTreeMap;

/// Partition id inside one NIF shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionId(pub u16);

/// One sparse morph displacement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphDelta {
    /// Index into [`MeshAsset::positions`].
    pub vertex: u32,
    /// Position displacement.
    pub position: [f32; 3],
}

/// One named morph target with sparse, vertex-sorted deltas.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphTarget {
    /// Target name.
    pub name: String,
    /// Sparse deltas, strictly increasing by vertex.
    pub deltas: Vec<MorphDelta>,
}

/// One ordered skin: bone names and four influences per vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skin {
    /// Bone names, indexed by [`Self::indices`].
    pub bones: Vec<String>,
    /// Four bone indices per vertex.
    pub indices: Vec<[u16; 4]>,
    /// Four weights per vertex, parallel to [`Self::indices`].
    pub weights: Vec<[f32; 4]>,
}

/// One material slot. Format-specific fields stay in the adapters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialSlot {
    /// Slot name, as authored.
    pub name: String,
}

/// One export mesh: flat arena, per-triangle partitions, sparse morphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshAsset {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals; empty or one per position.
    pub normals: Vec<[f32; 3]>,
    /// Vertex UVs; empty or one per position.
    pub uvs: Vec<[f32; 2]>,
    /// Vertex tangents; empty or one per position.
    pub tangents: Vec<[f32; 4]>,
    /// Triangle indices into [`Self::positions`].
    pub triangles: Vec<[u32; 3]>,
    /// One partition id per triangle.
    pub triangle_partitions: Vec<PartitionId>,
    /// One ordered skin, when the mesh is skinned.
    pub skin: Option<Skin>,
    /// Morph targets, sparse and vertex-sorted.
    pub morphs: Vec<MorphTarget>,
    /// Material slots.
    pub materials: Vec<MaterialSlot>,
}

/// Why a mesh cannot be split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// The mesh has no triangles.
    Empty,
    /// The limit is zero.
    ZeroLimit,
    /// A triangle needs three vertices, so the limit must be at least three.
    LimitTooSmall,
    /// Triangle and partition arrays differ in length.
    PartitionLengthMismatch,
    /// A triangle references a vertex outside the arena.
    TriangleVertexOutOfRange,
    /// A morph delta references a vertex outside the arena.
    MorphVertexOutOfRange,
    /// A morph target is not strictly sorted by vertex.
    MorphDeltasNotSorted,
    /// An optional per-vertex attribute has a length other than zero or the arena.
    AttributeLengthMismatch { attribute: &'static str },
    /// Skin influences or weights do not match the arena.
    SkinLengthMismatch,
}

impl MeshAsset {
    /// Validate the cross-array invariants the split depends on.
    pub fn validate(&self) -> Result<(), SplitError> {
        if self.triangles.len() != self.triangle_partitions.len() {
            return Err(SplitError::PartitionLengthMismatch);
        }
        for (name, len) in [
            ("normals", self.normals.len()),
            ("uvs", self.uvs.len()),
            ("tangents", self.tangents.len()),
        ] {
            if len != 0 && len != self.positions.len() {
                return Err(SplitError::AttributeLengthMismatch { attribute: name });
            }
        }
        if let Some(skin) = &self.skin
            && (skin.indices.len() != self.positions.len()
                || skin.weights.len() != self.positions.len())
        {
            return Err(SplitError::SkinLengthMismatch);
        }
        for triangle in &self.triangles {
            for vertex in triangle {
                if *vertex as usize >= self.positions.len() {
                    return Err(SplitError::TriangleVertexOutOfRange);
                }
            }
        }
        for target in &self.morphs {
            let mut previous = None;
            for delta in &target.deltas {
                if delta.vertex as usize >= self.positions.len() {
                    return Err(SplitError::MorphVertexOutOfRange);
                }
                if previous.is_some_and(|value| value >= delta.vertex) {
                    return Err(SplitError::MorphDeltasNotSorted);
                }
                previous = Some(delta.vertex);
            }
        }
        Ok(())
    }

    /// Split every partition into sub-shapes of at most `max_vertices`.
    ///
    /// Partitions are processed in id order; triangles keep their relative
    /// order. A sub-shape copies only the vertices its triangles reference, and
    /// every morph target is projected through the same remap. The union of the
    /// outputs reconstructs the input without duplicating a triangle.
    pub fn split_for_limit(&self, max_vertices: usize) -> Result<Vec<Self>, SplitError> {
        self.validate()?;
        if self.triangles.is_empty() {
            return Err(SplitError::Empty);
        }
        if max_vertices == 0 {
            return Err(SplitError::ZeroLimit);
        }
        if max_vertices < 3 {
            return Err(SplitError::LimitTooSmall);
        }

        let mut by_partition: BTreeMap<PartitionId, Vec<usize>> = BTreeMap::new();
        for (index, partition) in self.triangle_partitions.iter().enumerate() {
            by_partition.entry(*partition).or_default().push(index);
        }

        let mut shapes = Vec::new();
        for (partition, triangle_indices) in by_partition {
            let mut chunk = Chunk::default();
            for triangle_index in triangle_indices {
                let triangle = self.triangles[triangle_index];
                let mut new_vertices = 0;
                for vertex in triangle {
                    if !chunk.map.contains_key(&vertex) {
                        new_vertices += 1;
                    }
                }
                if chunk.vertices.len() + new_vertices > max_vertices {
                    shapes.push(self.flush(&mut chunk, partition));
                }
                let mut local = [0_u32; 3];
                for (slot, vertex) in triangle.iter().enumerate() {
                    let index = match chunk.map.get(vertex) {
                        Some(index) => *index,
                        None => {
                            let index = chunk.vertices.len() as u32;
                            chunk.vertices.push(*vertex);
                            chunk.map.insert(*vertex, index);
                            index
                        }
                    };
                    local[slot] = index;
                }
                chunk.triangles.push(local);
            }
            if !chunk.triangles.is_empty() {
                shapes.push(self.flush(&mut chunk, partition));
            }
        }
        Ok(shapes)
    }

    fn flush(&self, chunk: &mut Chunk, partition: PartitionId) -> Self {
        let positions = chunk
            .vertices
            .iter()
            .map(|vertex| self.positions[*vertex as usize])
            .collect();
        let normals = self.gather(&chunk.vertices, &self.normals);
        let uvs = self.gather(&chunk.vertices, &self.uvs);
        let tangents = self.gather(&chunk.vertices, &self.tangents);
        let skin = self.skin.as_ref().map(|skin| Skin {
            bones: skin.bones.clone(),
            indices: self.gather(&chunk.vertices, &skin.indices),
            weights: self.gather(&chunk.vertices, &skin.weights),
        });
        let morphs = self
            .morphs
            .iter()
            .map(|target| {
                let mut deltas: Vec<MorphDelta> = target
                    .deltas
                    .iter()
                    .filter_map(|delta| {
                        chunk.map.get(&delta.vertex).map(|local| MorphDelta {
                            vertex: *local,
                            position: delta.position,
                        })
                    })
                    .collect();
                deltas.sort_unstable_by_key(|delta| delta.vertex);
                MorphTarget {
                    name: target.name.clone(),
                    deltas,
                }
            })
            .collect();
        let triangles = std::mem::take(&mut chunk.triangles);
        let triangle_partitions = vec![partition; triangles.len()];
        chunk.map.clear();
        chunk.vertices.clear();
        Self {
            positions,
            normals,
            uvs,
            tangents,
            triangles,
            triangle_partitions,
            skin,
            morphs,
            materials: self.materials.clone(),
        }
    }

    fn gather<T: Copy>(&self, vertices: &[u32], values: &[T]) -> Vec<T> {
        if values.is_empty() {
            return Vec::new();
        }
        vertices
            .iter()
            .map(|vertex| values[*vertex as usize])
            .collect()
    }
}

#[derive(Default)]
struct Chunk {
    map: BTreeMap<u32, u32>,
    vertices: Vec<u32>,
    triangles: Vec<[u32; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, vertex: u32) -> MorphTarget {
        MorphTarget {
            name: name.into(),
            deltas: vec![MorphDelta {
                vertex,
                position: [0.1, 0.2, 0.3],
            }],
        }
    }

    #[test]
    fn a_garment_over_the_limit_splits_into_shapes_under_it() {
        let vertex_count = 70_000_u32;
        let mesh = MeshAsset {
            positions: (0..vertex_count)
                .map(|index| [index as f32, 0.0, 0.0])
                .collect(),
            triangles: (0..vertex_count - 2)
                .map(|index| [index, index + 1, index + 2])
                .collect(),
            triangle_partitions: vec![PartitionId(0); (vertex_count - 2) as usize],
            morphs: vec![
                MorphTarget {
                    name: "all".into(),
                    deltas: (0..vertex_count)
                        .map(|vertex| MorphDelta {
                            vertex,
                            position: [1.0, 0.0, 0.0],
                        })
                        .collect(),
                },
                MorphTarget {
                    name: "even".into(),
                    deltas: (0..vertex_count)
                        .step_by(2)
                        .map(|vertex| MorphDelta {
                            vertex,
                            position: [0.0, 1.0, 0.0],
                        })
                        .collect(),
                },
            ],
            ..MeshAsset::default()
        };
        mesh.validate().expect("valid");
        let before_positions: Vec<_> = mesh.positions.clone();

        let shapes = mesh.split_for_limit(65_535).expect("splits");
        assert_eq!(shapes.len(), 2);
        for shape in &shapes {
            assert!(shape.positions.len() <= 65_535);
            assert_eq!(shape.triangles.len(), shape.triangle_partitions.len());
            shape.validate().expect("valid split");
        }
        assert!(
            shapes
                .iter()
                .map(|shape| shape.positions.len())
                .sum::<usize>()
                >= vertex_count as usize
        );
        assert_eq!(
            shapes
                .iter()
                .map(|shape| shape.triangles.len())
                .sum::<usize>(),
            (vertex_count - 2) as usize
        );
        let original: std::collections::HashSet<[u32; 3]> = before_positions
            .iter()
            .map(|position| position.map(f32::to_bits))
            .collect();
        let mut seen = std::collections::HashSet::new();
        for shape in &shapes {
            for position in &shape.positions {
                let bits = position.map(f32::to_bits);
                assert!(original.contains(&bits), "split invented a position");
                seen.insert(bits);
            }
            assert_eq!(shape.morphs[0].deltas.len(), shape.positions.len());
            assert!(shape.morphs[1].deltas.len() <= shape.positions.len());
            for delta in &shape.morphs[0].deltas {
                assert_eq!(delta.position, [1.0, 0.0, 0.0]);
            }
            for delta in &shape.morphs[1].deltas {
                assert_eq!(delta.position, [0.0, 1.0, 0.0]);
            }
        }
        assert_eq!(seen.len(), vertex_count as usize);
        for shape in &shapes {
            for target in &shape.morphs {
                let mut sorted = target.deltas.clone();
                sorted.sort_unstable_by_key(|delta| delta.vertex);
                assert_eq!(sorted, target.deltas);
            }
        }
    }

    #[test]
    fn partitions_stay_separate_and_keep_their_ids() {
        let mesh = MeshAsset {
            positions: (0..10).map(|index| [index as f32, 0.0, 0.0]).collect(),
            triangles: vec![[0, 1, 2], [2, 3, 4], [5, 6, 7], [7, 8, 9]],
            triangle_partitions: vec![
                PartitionId(3),
                PartitionId(3),
                PartitionId(7),
                PartitionId(7),
            ],
            morphs: vec![target("m", 0)],
            ..MeshAsset::default()
        };
        let shapes = mesh.split_for_limit(4).expect("splits");
        assert_eq!(shapes.len(), 4);
        let ids: Vec<_> = shapes
            .iter()
            .map(|shape| shape.triangle_partitions[0])
            .collect();
        assert_eq!(
            ids,
            vec![
                PartitionId(3),
                PartitionId(3),
                PartitionId(7),
                PartitionId(7)
            ]
        );
        for shape in &shapes {
            assert!(shape.positions.len() <= 4);
        }
    }

    #[test]
    fn split_preserves_attributes_skin_and_materials() {
        let positions: Vec<[f32; 3]> = (0..6).map(|i| [i as f32, 0.0, 0.0]).collect();
        let mesh = MeshAsset {
            normals: positions.clone(),
            uvs: (0..6).map(|i| [i as f32 / 10.0, 0.5]).collect(),
            tangents: (0..6).map(|i| [1.0, 0.0, 0.0, i as f32]).collect(),
            skin: Some(Skin {
                bones: vec!["root".into()],
                indices: vec![[0, 0, 0, 0]; 6],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 6],
            }),
            positions,
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            triangle_partitions: vec![PartitionId(1), PartitionId(1)],
            morphs: Vec::new(),
            materials: vec![MaterialSlot {
                name: "robe".into(),
            }],
        };
        let shapes = mesh.split_for_limit(3).expect("splits");
        assert_eq!(shapes.len(), 2);
        for shape in &shapes {
            assert_eq!(shape.normals, shape.positions);
            assert_eq!(shape.uvs.len(), 3);
            assert_eq!(shape.tangents.len(), 3);
            let skin = shape.skin.as_ref().expect("skin");
            assert_eq!(skin.indices.len(), 3);
            assert_eq!(skin.weights.len(), 3);
            assert_eq!(shape.materials, mesh.materials);
            shape.validate().expect("valid");
        }

        let mut bad_normals = mesh.clone();
        bad_normals.normals.pop();
        assert_eq!(
            bad_normals.split_for_limit(3),
            Err(SplitError::AttributeLengthMismatch {
                attribute: "normals"
            })
        );
        let mut bad_skin = mesh;
        bad_skin.skin.as_mut().expect("skin").weights.pop();
        assert_eq!(
            bad_skin.split_for_limit(3),
            Err(SplitError::SkinLengthMismatch)
        );
    }

    #[test]
    fn negative_controls_are_named() {
        assert_eq!(
            MeshAsset::default().split_for_limit(4),
            Err(SplitError::Empty)
        );

        let one = MeshAsset {
            positions: vec![[0.0; 3]; 3],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            morphs: Vec::new(),
            ..MeshAsset::default()
        };
        assert_eq!(one.split_for_limit(0), Err(SplitError::ZeroLimit));
        assert_eq!(one.split_for_limit(2), Err(SplitError::LimitTooSmall));

        let mut mismatch = one.clone();
        mismatch.triangle_partitions.clear();
        assert_eq!(
            mismatch.split_for_limit(4),
            Err(SplitError::PartitionLengthMismatch)
        );

        let mut out_of_range = one.clone();
        out_of_range.triangles[0] = [0, 1, 3];
        assert_eq!(
            out_of_range.split_for_limit(4),
            Err(SplitError::TriangleVertexOutOfRange)
        );

        let mut bad_morph = one.clone();
        bad_morph.morphs = vec![target("bad", 3)];
        assert_eq!(
            bad_morph.split_for_limit(4),
            Err(SplitError::MorphVertexOutOfRange)
        );

        let mut unsorted = one;
        unsorted.morphs = vec![MorphTarget {
            name: "unsorted".into(),
            deltas: vec![
                MorphDelta {
                    vertex: 1,
                    position: [0.0; 3],
                },
                MorphDelta {
                    vertex: 1,
                    position: [1.0; 3],
                },
            ],
        }];
        assert_eq!(
            unsorted.split_for_limit(4),
            Err(SplitError::MorphDeltasNotSorted)
        );
    }
}
