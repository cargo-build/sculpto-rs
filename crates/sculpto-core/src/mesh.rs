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
    /// Shape name, as authored.
    pub name: String,
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

/// A validation defect over the aligned model arrays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshError {
    /// The arena has no vertices, so there is no part to export.
    NoVertices,
    /// An optional per-vertex attribute has a length other than zero or the arena.
    AttributeLengthMismatch { attribute: &'static str },
    /// Triangle and partition arrays differ in length.
    PartitionLengthMismatch,
    /// A triangle references a vertex outside the arena.
    TriangleVertexOutOfRange { triangle: usize, vertex: u32 },
    /// A morph delta references a vertex outside the arena.
    MorphVertexOutOfRange { target: usize, vertex: u32 },
    /// A morph target is not strictly sorted by vertex.
    MorphDeltasNotSorted { target: usize },
    /// A position, normal, UV, tangent, morph delta or weight is not finite.
    NonFinite {
        attribute: &'static str,
        index: usize,
    },
    /// Skin influences or weights do not match the arena.
    SkinLengthMismatch,
    /// A skin influence names a bone outside the skin's bone list.
    BoneIndexOutOfRange {
        vertex: usize,
        influence: usize,
        bone: u16,
        bones: usize,
    },
    /// A vertex's skin weights do not sum to one within `1e-4`.
    WeightSumOutOfRange { vertex: usize, sum: f32 },
    /// A triangle carries the reader's unmatched-partition sentinel.
    UnmatchedPartition { triangle: usize },
}

/// Why a mesh cannot be split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitError {
    /// The model itself is invalid; see [`MeshError`].
    Invalid(MeshError),
    /// The mesh has no triangles.
    Empty,
    /// The limit is zero.
    ZeroLimit,
    /// A triangle needs three vertices, so the limit must be at least three.
    LimitTooSmall,
    /// A morph delta points at a vertex no triangle references; the split
    /// cannot attach it to any output shape.
    UnreferencedMorphVertex { target: usize, vertex: u32 },
}

impl MeshAsset {
    /// Validate the model against every named invariant the writers rely on.
    ///
    /// Structural limits are type-level: a skin has exactly four influences per
    /// vertex because the field is `[u16; 4]` / `[f32; 4]`.
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.positions.is_empty() {
            return Err(MeshError::NoVertices);
        }
        for (attribute, len) in [
            ("normals", self.normals.len()),
            ("uvs", self.uvs.len()),
            ("tangents", self.tangents.len()),
        ] {
            if len != 0 && len != self.positions.len() {
                return Err(MeshError::AttributeLengthMismatch { attribute });
            }
        }
        if self.triangles.len() != self.triangle_partitions.len() {
            return Err(MeshError::PartitionLengthMismatch);
        }
        if let Some(skin) = &self.skin
            && (skin.indices.len() != self.positions.len()
                || skin.weights.len() != self.positions.len())
        {
            return Err(MeshError::SkinLengthMismatch);
        }

        for (index, position) in self.positions.iter().enumerate() {
            if !position.iter().all(|value| value.is_finite()) {
                return Err(MeshError::NonFinite {
                    attribute: "positions",
                    index,
                });
            }
        }
        for (index, normal) in self.normals.iter().enumerate() {
            if !normal.iter().all(|value| value.is_finite()) {
                return Err(MeshError::NonFinite {
                    attribute: "normals",
                    index,
                });
            }
        }
        for (index, uv) in self.uvs.iter().enumerate() {
            if !uv.iter().all(|value| value.is_finite()) {
                return Err(MeshError::NonFinite {
                    attribute: "uvs",
                    index,
                });
            }
        }
        for (index, tangent) in self.tangents.iter().enumerate() {
            if !tangent.iter().all(|value| value.is_finite()) {
                return Err(MeshError::NonFinite {
                    attribute: "tangents",
                    index,
                });
            }
        }

        for (triangle, vertices) in self.triangles.iter().enumerate() {
            for vertex in vertices {
                if *vertex as usize >= self.positions.len() {
                    return Err(MeshError::TriangleVertexOutOfRange {
                        triangle,
                        vertex: *vertex,
                    });
                }
            }
        }
        for (triangle, partition) in self.triangle_partitions.iter().enumerate() {
            if partition.0 == u16::MAX {
                return Err(MeshError::UnmatchedPartition { triangle });
            }
        }

        for (target, morph) in self.morphs.iter().enumerate() {
            let mut previous = None;
            for delta in &morph.deltas {
                if delta.vertex as usize >= self.positions.len() {
                    return Err(MeshError::MorphVertexOutOfRange {
                        target,
                        vertex: delta.vertex,
                    });
                }
                if previous.is_some_and(|value| value >= delta.vertex) {
                    return Err(MeshError::MorphDeltasNotSorted { target });
                }
                if !delta.position.iter().all(|value| value.is_finite()) {
                    return Err(MeshError::NonFinite {
                        attribute: "morphs",
                        index: delta.vertex as usize,
                    });
                }
                previous = Some(delta.vertex);
            }
        }

        if let Some(skin) = &self.skin {
            for (vertex, indices) in skin.indices.iter().enumerate() {
                for (influence, bone) in indices.iter().enumerate() {
                    if *bone as usize >= skin.bones.len() {
                        return Err(MeshError::BoneIndexOutOfRange {
                            vertex,
                            influence,
                            bone: *bone,
                            bones: skin.bones.len(),
                        });
                    }
                }
                let weights = skin.weights[vertex];
                if !weights.iter().all(|value| value.is_finite()) {
                    return Err(MeshError::NonFinite {
                        attribute: "weights",
                        index: vertex,
                    });
                }
                let sum: f32 = weights.iter().sum();
                if (sum - 1.0).abs() > 1e-4 {
                    return Err(MeshError::WeightSumOutOfRange { vertex, sum });
                }
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
        self.validate().map_err(SplitError::Invalid)?;
        if self.triangles.is_empty() {
            return Err(SplitError::Empty);
        }
        if max_vertices == 0 {
            return Err(SplitError::ZeroLimit);
        }
        if max_vertices < 3 {
            return Err(SplitError::LimitTooSmall);
        }

        let mut referenced = vec![false; self.positions.len()];
        for triangle in &self.triangles {
            for vertex in triangle {
                referenced[*vertex as usize] = true;
            }
        }
        for (target, morph) in self.morphs.iter().enumerate() {
            if let Some(delta) = morph
                .deltas
                .iter()
                .find(|delta| !referenced[delta.vertex as usize])
            {
                return Err(SplitError::UnreferencedMorphVertex {
                    target,
                    vertex: delta.vertex,
                });
            }
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
                    let name = self.chunk_name(shapes.len());
                    shapes.push(self.flush(&mut chunk, partition, name));
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
                let name = self.chunk_name(shapes.len());
                shapes.push(self.flush(&mut chunk, partition, name));
            }
        }
        Ok(shapes)
    }

    fn chunk_name(&self, index: usize) -> String {
        match index {
            0 => self.name.clone(),
            _ => format!("{}.{index}", self.name),
        }
    }

    fn flush(&self, chunk: &mut Chunk, partition: PartitionId, name: String) -> Self {
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
            name,
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
            ..MeshAsset::default()
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
            Err(SplitError::Invalid(MeshError::AttributeLengthMismatch {
                attribute: "normals"
            }))
        );
        let mut bad_skin = mesh;
        bad_skin.skin.as_mut().expect("skin").weights.pop();
        assert_eq!(
            bad_skin.split_for_limit(3),
            Err(SplitError::Invalid(MeshError::SkinLengthMismatch))
        );
    }

    fn one_triangle() -> MeshAsset {
        MeshAsset {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            ..MeshAsset::default()
        }
    }

    fn skinned() -> MeshAsset {
        let mut mesh = one_triangle();
        mesh.skin = Some(Skin {
            bones: vec!["root".into()],
            indices: vec![[0, 0, 0, 0]; 3],
            weights: vec![[0.25, 0.25, 0.25, 0.25]; 3],
        });
        mesh
    }

    #[test]
    fn validation_names_every_defect() {
        one_triangle().validate().expect("valid triangle");
        skinned().validate().expect("valid skin");

        assert_eq!(MeshAsset::default().validate(), Err(MeshError::NoVertices));

        let mut mesh = one_triangle();
        mesh.normals = vec![[0.0; 3]; 2];
        assert_eq!(
            mesh.validate(),
            Err(MeshError::AttributeLengthMismatch {
                attribute: "normals"
            })
        );

        let mut mesh = one_triangle();
        mesh.triangle_partitions.clear();
        assert_eq!(mesh.validate(), Err(MeshError::PartitionLengthMismatch));

        let mut mesh = one_triangle();
        mesh.triangles[0] = [0, 1, 3];
        assert_eq!(
            mesh.validate(),
            Err(MeshError::TriangleVertexOutOfRange {
                triangle: 0,
                vertex: 3
            })
        );

        let mut mesh = one_triangle();
        mesh.triangle_partitions[0] = PartitionId(u16::MAX);
        assert_eq!(
            mesh.validate(),
            Err(MeshError::UnmatchedPartition { triangle: 0 })
        );

        let mut mesh = one_triangle();
        mesh.morphs = vec![MorphTarget {
            name: "bad".into(),
            deltas: vec![MorphDelta {
                vertex: 3,
                position: [0.0; 3],
            }],
        }];
        assert_eq!(
            mesh.validate(),
            Err(MeshError::MorphVertexOutOfRange {
                target: 0,
                vertex: 3
            })
        );

        let mut mesh = one_triangle();
        mesh.morphs = vec![MorphTarget {
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
            mesh.validate(),
            Err(MeshError::MorphDeltasNotSorted { target: 0 })
        );

        for attribute in ["positions", "normals", "uvs", "tangents"] {
            let mut mesh = one_triangle();
            match attribute {
                "positions" => mesh.positions[1][0] = f32::NAN,
                "normals" => {
                    mesh.normals = vec![[0.0; 3]; 3];
                    mesh.normals[1][0] = f32::INFINITY;
                }
                "uvs" => {
                    mesh.uvs = vec![[0.0; 2]; 3];
                    mesh.uvs[1][0] = f32::NAN;
                }
                "tangents" => {
                    mesh.tangents = vec![[0.0; 4]; 3];
                    mesh.tangents[1][0] = f32::NAN;
                }
                _ => unreachable!(),
            }
            assert_eq!(
                mesh.validate(),
                Err(MeshError::NonFinite {
                    attribute,
                    index: 1
                })
            );
        }

        let mut mesh = one_triangle();
        mesh.morphs = vec![MorphTarget {
            name: "nan".into(),
            deltas: vec![MorphDelta {
                vertex: 0,
                position: [f32::NAN, 0.0, 0.0],
            }],
        }];
        assert_eq!(
            mesh.validate(),
            Err(MeshError::NonFinite {
                attribute: "morphs",
                index: 0
            })
        );

        let mut mesh = skinned();
        mesh.skin.as_mut().expect("skin").weights.pop();
        assert_eq!(mesh.validate(), Err(MeshError::SkinLengthMismatch));

        let mut mesh = skinned();
        mesh.skin.as_mut().expect("skin").indices[1][2] = 4;
        assert_eq!(
            mesh.validate(),
            Err(MeshError::BoneIndexOutOfRange {
                vertex: 1,
                influence: 2,
                bone: 4,
                bones: 1
            })
        );

        let mut mesh = skinned();
        mesh.skin.as_mut().expect("skin").weights[2] = [0.5, 0.0, 0.0, 0.0];
        assert_eq!(
            mesh.validate(),
            Err(MeshError::WeightSumOutOfRange {
                vertex: 2,
                sum: 0.5
            })
        );

        let mut mesh = skinned();
        mesh.skin.as_mut().expect("skin").weights[0][0] = f32::NAN;
        assert_eq!(
            mesh.validate(),
            Err(MeshError::NonFinite {
                attribute: "weights",
                index: 0
            })
        );
    }

    #[test]
    fn an_unreferenced_morph_delta_is_refused_not_dropped() {
        let mesh = MeshAsset {
            positions: vec![[0.0; 3]; 4],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            morphs: vec![MorphTarget {
                name: "loose".into(),
                deltas: vec![MorphDelta {
                    vertex: 3,
                    position: [1.0, 0.0, 0.0],
                }],
            }],
            ..MeshAsset::default()
        };
        assert_eq!(
            mesh.split_for_limit(65_535),
            Err(SplitError::UnreferencedMorphVertex {
                target: 0,
                vertex: 3
            })
        );
    }

    #[test]
    fn negative_controls_are_named() {
        assert_eq!(
            MeshAsset::default().split_for_limit(4),
            Err(SplitError::Invalid(MeshError::NoVertices))
        );
        let mut empty = one_triangle();
        empty.triangles.clear();
        empty.triangle_partitions.clear();
        assert_eq!(empty.split_for_limit(4), Err(SplitError::Empty));

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
            Err(SplitError::Invalid(MeshError::PartitionLengthMismatch))
        );

        let mut out_of_range = one.clone();
        out_of_range.triangles[0] = [0, 1, 3];
        assert_eq!(
            out_of_range.split_for_limit(4),
            Err(SplitError::Invalid(MeshError::TriangleVertexOutOfRange {
                triangle: 0,
                vertex: 3
            }))
        );

        let mut bad_morph = one.clone();
        bad_morph.morphs = vec![target("bad", 3)];
        assert_eq!(
            bad_morph.split_for_limit(4),
            Err(SplitError::Invalid(MeshError::MorphVertexOutOfRange {
                target: 0,
                vertex: 3
            }))
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
            Err(SplitError::Invalid(MeshError::MorphDeltasNotSorted {
                target: 0
            }))
        );
    }
}
