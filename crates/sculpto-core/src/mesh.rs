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

use std::collections::{BTreeMap, BTreeSet};

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

/// Which part of the mesh a morph target moves.
///
/// The `.tri` writer projects one shape per partition, so a [`MorphScope::Part`]
/// target travels only with the shape owning its partition, while a
/// [`MorphScope::Whole`] target travels with every projected shape. glTF morph
/// targets stay whole-mesh: [`MorphScope`] narrows the `.tri` shape projection,
/// not the glTF target list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MorphScope {
    /// The target moves the whole mesh and projects into every shape.
    #[default]
    Whole,
    /// The target moves only the named partition's shape.
    Part(PartitionId),
}

/// One named morph target with sparse, vertex-sorted deltas.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphTarget {
    /// Target name.
    pub name: String,
    /// Sparse deltas, strictly increasing by vertex.
    pub deltas: Vec<MorphDelta>,
    /// Which part of the mesh the target moves.
    pub scope: MorphScope,
}

/// One ordered skin: bone names and four influences per vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skin {
    /// Bone names, indexed by [`Self::indices`].
    pub bones: Vec<String>,
    /// Per-bone bind matrices in mesh space, column-major (mesh -> bone at
    /// bind). Empty means the identity fallback for every bone; otherwise one
    /// matrix per bone.
    pub bind_inverse: Vec<[[f32; 4]; 4]>,
    /// Four bone indices per vertex.
    pub indices: Vec<[u16; 4]>,
    /// Four weights per vertex, parallel to [`Self::indices`].
    pub weights: Vec<[f32; 4]>,
}

/// Neutral shader family a slot belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShaderKind {
    /// Ordinary opaque surface.
    #[default]
    Default,
    /// Skinned body or garment surface.
    Skin,
    /// Hair or fur surface.
    Hair,
    /// Eye surface.
    Eyes,
    /// Cloth with alpha rules.
    Cloth,
}

/// One texture channel the model knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSlot {
    /// Base colour.
    Diffuse,
    /// Normal map.
    Normal,
    /// Specular or roughness map.
    Specular,
}

impl TextureSlot {
    /// Stable label for errors and reference output.
    pub fn as_str(self) -> &'static str {
        match self {
            TextureSlot::Diffuse => "diffuse",
            TextureSlot::Normal => "normal",
            TextureSlot::Specular => "specular",
        }
    }
}

/// One material slot. Format-specific fields stay in the adapters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialSlot {
    /// Slot name, as authored.
    pub name: String,
    /// Neutral shader family.
    pub shader: ShaderKind,
    /// Base-colour texture path.
    pub diffuse: Option<String>,
    /// Normal-map path.
    pub normal: Option<String>,
    /// Specular/roughness map path.
    pub specular: Option<String>,
    /// Render both faces.
    pub double_sided: bool,
}

impl MaterialSlot {
    /// Read one texture channel, so Skyrim and glTF projections share a source.
    pub fn texture(&self, slot: TextureSlot) -> Option<&str> {
        match slot {
            TextureSlot::Diffuse => self.diffuse.as_deref(),
            TextureSlot::Normal => self.normal.as_deref(),
            TextureSlot::Specular => self.specular.as_deref(),
        }
    }
}

/// A named material-slot defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialDefect {
    /// The slot name is empty.
    EmptyName,
    /// Another slot already uses this name.
    DuplicateName,
    /// A declared texture path is empty.
    EmptyTexturePath(TextureSlot),
}

/// A typed reference to the physics sidecar. The sidecar document itself is
/// owned by the physics adapter; the model carries no solver types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhysicsRef {
    /// Path to the sidecar file, relative to the asset.
    Path(String),
    /// Inline sidecar document, for adapters that cannot ship a second file.
    Inline(String),
}

impl PhysicsRef {
    /// The sidecar document or path this reference names.
    pub fn sidecar(&self) -> &str {
        match self {
            PhysicsRef::Path(path) => path,
            PhysicsRef::Inline(document) => document,
        }
    }
}

/// A named physics-reference defect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicsDefect {
    /// The path is empty.
    EmptyPath,
    /// The inline document is empty.
    EmptyInline,
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
    /// Physics sidecar reference, when the asset carries one.
    pub physics: Option<PhysicsRef>,
}

/// One sparse offset in the `.tri` projection: a `u16` vertex index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriOffset {
    /// Vertex index, narrowed to the `.tri` field width.
    pub vertex: u16,
    /// Position displacement.
    pub position: [f32; 3],
}

/// One named `.tri` morph: sparse offsets sorted by vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct TriMorph {
    /// Morph name.
    pub name: String,
    /// Sparse offsets.
    pub offsets: Vec<TriOffset>,
}

/// One `.tri` shape: the sparse morph model the byte writer consumes.
#[derive(Debug, Clone, PartialEq)]
pub struct TriShape {
    /// Shape name.
    pub name: String,
    /// Vertex count the offsets address.
    pub vertex_count: u32,
    /// Morphs, one per model target.
    pub morphs: Vec<TriMorph>,
}

/// Why a mesh cannot be projected into the `.tri` model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriError {
    /// The arena has more vertices than the `u32` count can hold.
    TooManyVertices,
    /// A morph offset does not fit the `u16` vertex field; split the shape first.
    IndexOutOfRange { target: usize, vertex: u32 },
}

impl MeshAsset {
    /// Project the model into the sparse `.tri` model.
    ///
    /// The `.tri` layout stores one `u16` index per offset and has no target
    /// cap, so every model target survives — including [`MorphScope::Part`]
    /// targets scoped to other partitions; use [`Self::tri_shape_for_part`]
    /// to drop them. A shape above 65 535 vertices must be split first
    /// ([`Self::split_for_limit`]); this projection refuses an index it
    /// cannot narrow.
    pub fn tri_shape(&self) -> Result<TriShape, TriError> {
        self.tri_shape_filtered(|_| true)
    }

    /// [`Self::tri_shape`] narrowed to one partition's shape: it carries the
    /// [`MorphScope::Whole`] targets and the [`MorphScope::Part`] targets owned
    /// by `part`, and drops targets scoped to other partitions. The vertex
    /// arena is unchanged; only the morph list narrows.
    pub fn tri_shape_for_part(&self, part: PartitionId) -> Result<TriShape, TriError> {
        self.tri_shape_filtered(|scope| match scope {
            MorphScope::Whole => true,
            MorphScope::Part(target) => *target == part,
        })
    }

    /// The shared `.tri` projection; `keep` selects the targets that survive.
    fn tri_shape_filtered(&self, keep: impl Fn(&MorphScope) -> bool) -> Result<TriShape, TriError> {
        let vertex_count =
            u32::try_from(self.positions.len()).map_err(|_| TriError::TooManyVertices)?;
        let morphs = self
            .morphs
            .iter()
            .enumerate()
            .filter(|(_, morph)| keep(&morph.scope))
            .map(|(target, morph)| {
                let offsets = morph
                    .deltas
                    .iter()
                    .map(|delta| {
                        let vertex =
                            u16::try_from(delta.vertex).map_err(|_| TriError::IndexOutOfRange {
                                target,
                                vertex: delta.vertex,
                            })?;
                        Ok(TriOffset {
                            vertex,
                            position: delta.position,
                        })
                    })
                    .collect::<Result<Vec<_>, TriError>>()?;
                Ok(TriMorph {
                    name: morph.name.clone(),
                    offsets,
                })
            })
            .collect::<Result<Vec<_>, TriError>>()?;
        Ok(TriShape {
            name: self.name.clone(),
            vertex_count,
            morphs,
        })
    }
}

/// One partition's bone palette: the bones its vertices reference, sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionPalette {
    /// Partition this palette serves.
    pub partition: PartitionId,
    /// Bone indices referenced by the partition's vertices, ascending.
    pub bones: Vec<u16>,
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
    /// A morph target scoped to a partition the model has no triangles for.
    MorphUnknownPart {
        /// The morph target's position in the morph slice.
        target: usize,
        /// The partition id the target names.
        part: PartitionId,
    },
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
    /// A vertex's skin weights sum to zero, so they cannot be normalized.
    ZeroWeightSum { vertex: usize },
    /// The bind-matrix array is neither empty nor one per bone.
    BindLengthMismatch { bind: usize, bones: usize },
    /// A bind matrix cell is not finite.
    NonFiniteBind { bone: usize },
    /// A triangle partition has no supplied palette.
    MissingPartitionPalette { partition: PartitionId },
    /// A supplied palette omits a bone its vertices reference.
    PaletteMissingBone { partition: PartitionId, bone: u16 },
    /// The same partition id appears in two palettes.
    DuplicatePartitionPalette { partition: PartitionId },
    /// A triangle carries the reader's unmatched-partition sentinel.
    UnmatchedPartition { triangle: usize },
    /// A material slot violates a named rule.
    Material { slot: usize, defect: MaterialDefect },
    /// The physics reference is malformed.
    Physics { defect: PhysicsDefect },
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
        if let Some(skin) = &self.skin {
            if !skin.bind_inverse.is_empty() && skin.bind_inverse.len() != skin.bones.len() {
                return Err(MeshError::BindLengthMismatch {
                    bind: skin.bind_inverse.len(),
                    bones: skin.bones.len(),
                });
            }
            for (bone, matrix) in skin.bind_inverse.iter().enumerate() {
                if !matrix.iter().flatten().all(|value| value.is_finite()) {
                    return Err(MeshError::NonFiniteBind { bone });
                }
            }
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
            if let MorphScope::Part(part) = morph.scope
                && !self.triangle_partitions.contains(&part)
            {
                return Err(MeshError::MorphUnknownPart { target, part });
            }
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

        let mut names = std::collections::BTreeSet::new();
        for (slot, material) in self.materials.iter().enumerate() {
            if material.name.trim().is_empty() {
                return Err(MeshError::Material {
                    slot,
                    defect: MaterialDefect::EmptyName,
                });
            }
            if !names.insert(material.name.as_str()) {
                return Err(MeshError::Material {
                    slot,
                    defect: MaterialDefect::DuplicateName,
                });
            }
            for texture in [
                TextureSlot::Diffuse,
                TextureSlot::Normal,
                TextureSlot::Specular,
            ] {
                if material
                    .texture(texture)
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err(MeshError::Material {
                        slot,
                        defect: MaterialDefect::EmptyTexturePath(texture),
                    });
                }
            }
        }

        match &self.physics {
            Some(PhysicsRef::Path(path)) if path.trim().is_empty() => {
                return Err(MeshError::Physics {
                    defect: PhysicsDefect::EmptyPath,
                });
            }
            Some(PhysicsRef::Inline(document)) if document.trim().is_empty() => {
                return Err(MeshError::Physics {
                    defect: PhysicsDefect::EmptyInline,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Normalize every vertex's skin weights to sum to one.
    ///
    /// Idempotent: a second call divides by a sum already within float noise of
    /// one. A zero (or non-finite) sum is refused instead of hidden.
    pub fn normalize_weights(&mut self) -> Result<(), MeshError> {
        let Some(skin) = &mut self.skin else {
            return Ok(());
        };
        for (vertex, weights) in skin.weights.iter_mut().enumerate() {
            let sum: f32 = weights.iter().sum();
            if sum == 0.0 || !sum.is_finite() {
                return Err(MeshError::ZeroWeightSum { vertex });
            }
            for weight in weights.iter_mut() {
                *weight /= sum;
            }
        }
        Ok(())
    }

    /// Derive one exact palette per triangle partition.
    ///
    /// A bone is in a palette only when some vertex of that partition carries a
    /// non-zero weight for it, so the result is the smallest legal palette. The
    /// writers project it; a caller may also compare it against an authored set.
    pub fn partition_palettes(&self) -> Vec<PartitionPalette> {
        let Some(skin) = &self.skin else {
            return Vec::new();
        };
        let mut by_partition: BTreeMap<PartitionId, BTreeSet<u16>> = BTreeMap::new();
        for (index, triangle) in self.triangles.iter().enumerate() {
            let entry = by_partition
                .entry(self.triangle_partitions[index])
                .or_default();
            for vertex in triangle {
                let vertex = *vertex as usize;
                for (slot, bone) in skin.indices[vertex].iter().enumerate() {
                    if skin.weights[vertex][slot] > 0.0 {
                        entry.insert(*bone);
                    }
                }
            }
        }
        by_partition
            .into_iter()
            .map(|(partition, bones)| PartitionPalette {
                partition,
                bones: bones.into_iter().collect(),
            })
            .collect()
    }

    /// Refuse a palette set that misses a partition or a referenced bone.
    ///
    /// This is the gate a writer or an authoring surface uses before emitting
    /// `NiSkinPartition`: a partition whose palette omits one of its bones
    /// produces a mesh that deforms wrongly at runtime, so it is a typed error.
    pub fn validate_partition_palettes(
        &self,
        palettes: &[PartitionPalette],
    ) -> Result<(), MeshError> {
        let mut supplied: BTreeMap<PartitionId, &PartitionPalette> = BTreeMap::new();
        for palette in palettes {
            if supplied.insert(palette.partition, palette).is_some() {
                return Err(MeshError::DuplicatePartitionPalette {
                    partition: palette.partition,
                });
            }
        }
        let Some(skin) = &self.skin else {
            return Ok(());
        };
        for (index, triangle) in self.triangles.iter().enumerate() {
            let partition = self.triangle_partitions[index];
            let Some(palette) = supplied.get(&partition) else {
                return Err(MeshError::MissingPartitionPalette { partition });
            };
            for vertex in triangle {
                let vertex = *vertex as usize;
                for (slot, bone) in skin.indices[vertex].iter().enumerate() {
                    if skin.weights[vertex][slot] > 0.0 && !palette.bones.contains(bone) {
                        return Err(MeshError::PaletteMissingBone {
                            partition,
                            bone: *bone,
                        });
                    }
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
                for (slot, vertex) in triangle.iter().enumerate() {
                    if !chunk.map.contains_key(vertex) && !triangle[..slot].contains(vertex) {
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
            bind_inverse: skin.bind_inverse.clone(),
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
                    scope: target.scope,
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
            physics: self.physics.clone(),
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
            scope: MorphScope::Whole,
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
                    scope: MorphScope::Whole,
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
                    scope: MorphScope::Whole,
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
                bind_inverse: Vec::new(),
                indices: vec![[0, 0, 0, 0]; 6],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 6],
            }),
            positions,
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            triangle_partitions: vec![PartitionId(1), PartitionId(1)],
            morphs: Vec::new(),
            materials: vec![MaterialSlot {
                name: "robe".into(),
                ..MaterialSlot::default()
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
            bind_inverse: Vec::new(),
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
            scope: MorphScope::Whole,
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
            scope: MorphScope::Whole,
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
            scope: MorphScope::Whole,
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
    fn physics_reference_is_typed_and_split_copies_it() {
        let mut mesh = one_triangle();
        mesh.physics = Some(PhysicsRef::Path("robe.sculpto-phys.json".into()));
        mesh.validate().expect("a path reference is valid");
        assert_eq!(
            mesh.physics.as_ref().expect("reference").sidecar(),
            "robe.sculpto-phys.json"
        );

        let shapes = mesh.split_for_limit(3).expect("splits");
        assert!(shapes.iter().all(|shape| shape.physics == mesh.physics));

        mesh.physics = Some(PhysicsRef::Inline("{\"constraints\":[]}".into()));
        mesh.validate().expect("an inline reference is valid");
        assert_eq!(
            mesh.physics.as_ref().expect("reference").sidecar(),
            "{\"constraints\":[]}"
        );
    }

    #[test]
    fn physics_reference_negative_controls_are_named() {
        let mut mesh = one_triangle();
        mesh.physics = Some(PhysicsRef::Path(String::new()));
        assert_eq!(
            mesh.validate(),
            Err(MeshError::Physics {
                defect: PhysicsDefect::EmptyPath
            })
        );

        mesh.physics = Some(PhysicsRef::Inline("  ".into()));
        assert_eq!(
            mesh.validate(),
            Err(MeshError::Physics {
                defect: PhysicsDefect::EmptyInline
            })
        );
    }

    fn two_partitions() -> MeshAsset {
        MeshAsset {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [1, 2, 3]],
            triangle_partitions: vec![PartitionId(0), PartitionId(1)],
            skin: Some(Skin {
                bones: vec!["a".into(), "b".into(), "c".into()],
                bind_inverse: Vec::new(),
                indices: vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0], [2, 0, 0, 0]],
                weights: vec![
                    [1.0, 0.0, 0.0, 0.0],
                    [0.5, 0.5, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0, 0.0],
                ],
            }),
            ..MeshAsset::default()
        }
    }

    #[test]
    fn partition_palettes_are_exact_and_sorted() {
        let mesh = two_partitions();
        mesh.validate().expect("valid");
        assert_eq!(
            mesh.partition_palettes(),
            vec![
                PartitionPalette {
                    partition: PartitionId(0),
                    bones: vec![0, 1],
                },
                PartitionPalette {
                    partition: PartitionId(1),
                    bones: vec![0, 1, 2],
                },
            ]
        );
        mesh.validate_partition_palettes(&mesh.partition_palettes())
            .expect("derived palettes cover every reference");

        let mut skinless = mesh.clone();
        skinless.skin = None;
        assert!(skinless.partition_palettes().is_empty());
        skinless
            .validate_partition_palettes(&[])
            .expect("a skinless mesh needs no palette");
    }

    #[test]
    fn partition_palette_defects_are_named() {
        let mesh = two_partitions();
        let mut palettes = mesh.partition_palettes();

        palettes[0].bones.retain(|bone| *bone != 1);
        assert_eq!(
            mesh.validate_partition_palettes(&palettes),
            Err(MeshError::PaletteMissingBone {
                partition: PartitionId(0),
                bone: 1
            })
        );

        let mut missing = mesh.partition_palettes();
        missing.pop();
        assert_eq!(
            mesh.validate_partition_palettes(&missing),
            Err(MeshError::MissingPartitionPalette {
                partition: PartitionId(1)
            })
        );

        let mut duplicate = mesh.partition_palettes();
        duplicate.push(duplicate[0].clone());
        assert_eq!(
            mesh.validate_partition_palettes(&duplicate),
            Err(MeshError::DuplicatePartitionPalette {
                partition: PartitionId(0)
            })
        );
    }

    #[test]
    fn material_slots_validate_and_share_one_projection() {
        let mut mesh = one_triangle();
        mesh.materials = vec![
            MaterialSlot {
                name: "body".into(),
                shader: ShaderKind::Skin,
                diffuse: Some("body_d.dds".into()),
                normal: Some("body_n.dds".into()),
                specular: None,
                double_sided: false,
            },
            MaterialSlot {
                name: "robe".into(),
                shader: ShaderKind::Cloth,
                diffuse: Some("robe_d.dds".into()),
                ..MaterialSlot::default()
            },
        ];
        mesh.validate().expect("valid slots");

        // A Skyrim projection and a glTF projection read the same value.
        let body = &mesh.materials[0];
        assert_eq!(body.texture(TextureSlot::Diffuse), Some("body_d.dds"));
        assert_eq!(body.texture(TextureSlot::Normal), Some("body_n.dds"));
        assert_eq!(body.texture(TextureSlot::Specular), None);
        assert_eq!(body.shader, ShaderKind::Skin);
        assert!(!body.double_sided);

        let mut empty_name = one_triangle();
        empty_name.materials = vec![MaterialSlot::default()];
        assert_eq!(
            empty_name.validate(),
            Err(MeshError::Material {
                slot: 0,
                defect: MaterialDefect::EmptyName
            })
        );

        let mut duplicate = one_triangle();
        duplicate.materials = vec![
            MaterialSlot {
                name: "same".into(),
                ..MaterialSlot::default()
            },
            MaterialSlot {
                name: "same".into(),
                ..MaterialSlot::default()
            },
        ];
        assert_eq!(
            duplicate.validate(),
            Err(MeshError::Material {
                slot: 1,
                defect: MaterialDefect::DuplicateName
            })
        );

        let mut empty_path = one_triangle();
        empty_path.materials = vec![MaterialSlot {
            name: "robe".into(),
            diffuse: Some(String::new()),
            ..MaterialSlot::default()
        }];
        assert_eq!(
            empty_path.validate(),
            Err(MeshError::Material {
                slot: 0,
                defect: MaterialDefect::EmptyTexturePath(TextureSlot::Diffuse)
            })
        );

        empty_path.materials[0].diffuse = Some("   ".into());
        assert_eq!(
            empty_path.validate(),
            Err(MeshError::Material {
                slot: 0,
                defect: MaterialDefect::EmptyTexturePath(TextureSlot::Diffuse)
            })
        );
    }

    #[test]
    fn tri_model_accepts_a_65th_target_and_keeps_it_sparse() {
        let mut mesh = one_triangle();
        mesh.name = "body".into();
        mesh.morphs = (0..65)
            .map(|target| MorphTarget {
                name: format!("m{target}"),
                deltas: vec![MorphDelta {
                    vertex: target % 3,
                    position: [target as f32, 0.0, 0.0],
                }],
                scope: MorphScope::Whole,
            })
            .collect();
        mesh.validate().expect("65 targets are valid");
        let tri = mesh.tri_shape().expect("fits the .tri model");
        assert_eq!(tri.name, "body");
        assert_eq!(tri.vertex_count, 3);
        assert_eq!(tri.morphs.len(), 65);
        assert_eq!(tri.morphs[64].name, "m64");
        assert_eq!(tri.morphs[64].offsets.len(), 1);
        assert_eq!(tri.morphs[64].offsets[0].vertex, 1);
        assert_eq!(tri.morphs[64].offsets[0].position, [64.0, 0.0, 0.0]);

        let shapes = mesh.split_for_limit(65_535).expect("splits");
        assert_eq!(shapes[0].morphs.len(), 65);
        assert_eq!(shapes[0].tri_shape().expect("tri").morphs.len(), 65);
    }

    #[test]
    fn tri_model_refuses_an_index_it_cannot_narrow() {
        let mut mesh = MeshAsset {
            positions: vec![[0.0; 3]; 70_000],
            ..MeshAsset::default()
        };
        mesh.triangles = vec![[0, 1, 2]];
        mesh.triangle_partitions = vec![PartitionId(0)];
        mesh.morphs = vec![MorphTarget {
            name: "far".into(),
            deltas: vec![MorphDelta {
                vertex: 69_999,
                position: [1.0, 0.0, 0.0],
            }],
            scope: MorphScope::Whole,
        }];
        mesh.validate().expect("the model itself is valid");
        assert_eq!(
            mesh.tri_shape(),
            Err(TriError::IndexOutOfRange {
                target: 0,
                vertex: 69_999
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
                scope: MorphScope::Whole,
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
    fn degenerate_triangles_do_not_oversplit() {
        let mesh = MeshAsset {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 0, 1], [2, 2, 2]],
            triangle_partitions: vec![PartitionId(0), PartitionId(0)],
            ..MeshAsset::default()
        };
        mesh.validate().expect("in-range indices");
        let shapes = mesh.split_for_limit(3).expect("splits");
        assert_eq!(shapes.len(), 1, "repeated indices must count once");
        assert_eq!(shapes[0].positions.len(), 3);
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
            scope: MorphScope::Whole,
        }];
        assert_eq!(
            unsorted.split_for_limit(4),
            Err(SplitError::Invalid(MeshError::MorphDeltasNotSorted {
                target: 0
            }))
        );
    }
}
