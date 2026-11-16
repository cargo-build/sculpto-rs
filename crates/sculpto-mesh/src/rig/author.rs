//! Author skin weights for a deform skeleton directly from the flesh.
//!
//! The method is **geodesic bone envelopes**: every deform bone owns a capsule
//! around the segments it spans, the capsule radius is its own flesh radius
//! (the 75th percentile of the surface distance to the bone among the vertices
//! the bone is Euclidean-nearest to), and a vertex's weight for a bone falls off
//! with the **surface-geodesic** distance from that bone's capsule. Geodesic
//! distance is the occlusion term: a torso-wall vertex beside the armpit is two
//! centimetres from the arm axis but a long way round the crease, so the arm
//! cannot reach it the way a Euclidean envelope would.
//!
//! Concentration comes from the falloff exponent, not from a smoothing hack: a
//! vertex inside one capsule carries one owner at weight one, and only the
//! overlap of two capsules (a joint) produces a blend. A shell that no bone
//! seeds (a garment primitive is its own surface) falls back to the Euclidean
//! distance, which is declared in the report, never silent.
//!
//! Ownership, when the caller can measure it, is decided by the **solid**, not by
//! the surface: each deform bone owns a cell of the body volume, and a vertex may
//! only be carried by the cells whose closure contains it (plus the cells within
//! the narrow blend band the joint earns). [`VertexCells`] carries that verdict
//! as a budget: the owning cell takes at least half the row and the whole row a
//! band inside its own flesh, and the neighbours share the remainder by how close
//! their flesh is. A surface-geodesic field alone cannot hold the armpit boundary
//! in an A-pose,
//! because the upper arm touches the ribcage and a surface path skims the contact
//! patch; a cell cannot, since the film between two touching surfaces is not flesh.
//!
//! Normalization and the four-slot cap stay with
//! [`pack_influences`](crate::weight_norm::pack_influences); this module only
//! decides which bones a vertex belongs to and how much. @req DECL0008

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fmt;

use glam::DVec3;

use crate::garment::Influences;
use crate::weight_norm::{WeightError, pack_influences};

/// Which body side a bone drives, from its authored name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodySide {
    Left,
    Right,
}

impl BodySide {
    /// The opposite side, for the isolation mask.
    fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// One bone the authored skin may reach: its name, its side and its segments.
///
/// A segment runs from the bone's own joint to one child joint. A leaf bone has
/// no segment and therefore no influence: the authored skin never paints a
/// weight onto a tip that no flesh can follow.
#[derive(Debug, Clone, PartialEq)]
pub struct DeformBone {
    /// Bone name, unique in the palette.
    pub name: String,
    /// Side token from the name, when the bone carries one.
    pub side: Option<BodySide>,
    /// Head-to-child segments in the rest pose, metres.
    pub segments: Vec<(DVec3, DVec3)>,
}

/// Tuning of the authored skin, all of it declared before a measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorSettings {
    /// Falloff exponent on the inverse geodesic distance.
    pub exponent: f64,
    /// Falloff distances are measured in units of the vertex's own flesh
    /// thickness, so one rule serves a nine-centimetre hip and an eight
    /// millimetre finger.
    pub blend_in_flesh_radii: bool,
    /// Quantile of a bone's own-owner distances reported as its flesh radius.
    pub radius_quantile: f64,
    /// Weights under this fraction of a row's maximum are dropped before packing.
    pub prune_ratio: f64,
    /// Surface smoothing passes over the weight field.
    pub smoothing: usize,
    /// Half-width of the midline band that may carry both sides. A vertex inside
    /// the band has no side worth isolating; a vertex outside it belongs to one.
    pub midline_dead_zone: f64,
}

impl Default for AuthorSettings {
    fn default() -> Self {
        Self {
            exponent: 4.0,
            blend_in_flesh_radii: true,
            radius_quantile: 0.75,
            prune_ratio: 0.02,
            smoothing: 2,
            midline_dead_zone: 0.010,
        }
    }
}

/// The volume cells' verdict for one surface vertex: the bones that carry it and
/// the share of the row each takes.
///
/// Built by the caller from the body's solid (the engine's voxel occupancy, EDT
/// and watershed — never a second distance structure here). The shares are a
/// smooth budget: the owning cell holds at least half, two cells meeting at the
/// interface share 50/50, and a bone outside the blend band takes nothing. An
/// empty row is unconstrained and means the caller measured no cells.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexCells {
    /// `(bone index, share)` per vertex, owner first; shares sum to 1.
    pub allowed: Vec<Vec<(u32, f32)>>,
}

/// What the cell mask did to the authored field.
#[derive(Debug, Clone, PartialEq)]
pub struct CellReport {
    /// Vertices whose permitted set removed weight.
    pub cut_vertices: usize,
    /// Mass the mask removed, in raw pre-pack units.
    pub cut_mass: f64,
    /// Vertices the mask would have emptied and left with their heaviest
    /// influence: the boundary must never starve a row, and the exception is counted.
    pub restored: usize,
    /// Mean number of permitted bones over the constrained vertices.
    pub allowed_mean: f64,
    /// Widest permitted set.
    pub allowed_widest: usize,
    /// Vertices the caller left unconstrained.
    pub unconstrained: usize,
}

/// One bone's authored footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneFootprint {
    /// Bone name.
    pub name: String,
    /// Capsule radius used for its seeds, metres.
    pub radius: f64,
    /// Vertices inside the capsule that seeded the geodesic field.
    pub seeds: usize,
    /// Vertices the bone dominates, and their mean dominant weight.
    pub dominated: usize,
    /// Mean dominant weight over the dominated vertices; 0.0 when none.
    pub dominance: f64,
    /// Total authored weight mass.
    pub mass: f64,
}

/// Evidence of one authored skin.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorReport {
    /// Vertices authored.
    pub vertices: usize,
    /// Surface components the mesh splits into.
    pub components: usize,
    /// Bones offered to the authoring step.
    pub bones: usize,
    /// Bone-vertex pairs no surface path resolved; such a bone carries no
    /// weight at that vertex.
    pub unreached_pairs: usize,
    /// Vertices of the flesh component the field was authored on.
    pub flesh_vertices: usize,
    /// Shell vertices that inherited the flesh they lie on.
    pub inherited_vertices: usize,
    /// Vertices whose opposite-side mass the isolation mask removed.
    pub side_masked_vertices: usize,
    /// Mass the isolation mask removed, in raw pre-pack units.
    pub side_masked_mass: f64,
    /// Vertices whose only influence was the opposite side and kept it.
    pub side_mask_restored: usize,
    /// Weights dropped by the relative prune, before the strict pack.
    pub pruned: usize,
    /// Positive weights the strict four-slot pack still dropped.
    pub discarded: Vec<(String, f64)>,
    /// The lateral axis and midline point the isolation mask used.
    pub lateral_axis: [f64; 3],
    /// The midline point the isolation mask used.
    pub midline_origin: [f64; 3],
    /// Per-bone footprint in the order the caller declared the bones.
    pub footprints: Vec<BoneFootprint>,
    /// What the volume-cell mask removed; `None` when the caller supplied no cells.
    pub cells: Option<CellReport>,
}

/// One authored skin, ready for the caller's palette encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthoredSkin {
    /// Normalized, capped influence rows in vertex order.
    pub rows: Vec<Influences>,
    /// Measurement of the authoring step.
    pub report: AuthorReport,
}

/// Why the authoring step could not produce a skin.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthorError {
    /// No deform bone was offered.
    NoBones,
    /// A bone carries no segment, so it cannot own flesh.
    Boneless { bone: String },
    /// A bone name is declared twice.
    DuplicateBone { bone: String },
    /// The mesh has no triangles to build a surface from.
    NoSurface,
    /// A vertex position is not finite.
    NonFinitePosition { vertex: usize },
    /// A bone segment point is not finite.
    NonFiniteBone { bone: String },
    /// A vertex stayed unreachable and outside every fallback.
    Unreachable { vertex: usize },
    /// The flesh mask does not describe this surface.
    NoFlesh { vertices: usize },
    /// The strict pack refused a row.
    Pack { vertex: usize, error: WeightError },
    /// A bone's name could not be packed.
    DuplicateInfluence { vertex: usize, bone: String },
}

impl fmt::Display for AuthorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBones => write!(formatter, "no deform bone was offered"),
            Self::Boneless { bone } => {
                write!(formatter, "bone {bone:?} has no segment to own flesh")
            }
            Self::DuplicateBone { bone } => write!(formatter, "bone {bone:?} is declared twice"),
            Self::NoSurface => write!(formatter, "the mesh has no triangles"),
            Self::NonFinitePosition { vertex } => {
                write!(formatter, "vertex {vertex} has a non-finite position")
            }
            Self::NonFiniteBone { bone } => {
                write!(formatter, "bone {bone:?} has a non-finite segment point")
            }
            Self::Unreachable { vertex } => {
                write!(formatter, "vertex {vertex} has no reachable bone")
            }
            Self::NoFlesh { vertices } => {
                write!(formatter, "the flesh mask covers no vertex of {vertices}")
            }
            Self::Pack { vertex, error } => write!(formatter, "vertex {vertex}: {error}"),
            Self::DuplicateInfluence { vertex, bone } => {
                write!(formatter, "vertex {vertex}: bone {bone:?} arrives twice")
            }
        }
    }
}

impl std::error::Error for AuthorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pack { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Author one skin for `bones` over the surface of `positions`/`triangles`.
///
/// # Errors
/// See [`AuthorError`]; a refusal never returns a partial skin.
pub fn author_weights(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    bones: &[DeformBone],
    flesh: &[bool],
    settings: &AuthorSettings,
) -> Result<AuthoredSkin, AuthorError> {
    author_weights_within(positions, triangles, bones, flesh, settings, None)
}

/// Author one skin, bounded by the solid's own cells when the caller measured them.
///
/// `cells` is the ownership verdict of [`VertexCells`]: a vertex may carry only
/// the bones whose cell contains it or whose cell is inside its blend band. The
/// mask runs after smoothing — a smoothing pass would carry cut mass back across
/// the crease — and before the shell inheritance, so a garment layer inherits the
/// bounded field of the flesh it lies on rather than a second, unbounded one.
///
/// # Errors
/// See [`AuthorError`]; a refusal never returns a partial skin.
pub fn author_weights_within(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    bones: &[DeformBone],
    flesh: &[bool],
    settings: &AuthorSettings,
    cells: Option<&VertexCells>,
) -> Result<AuthoredSkin, AuthorError> {
    if bones.is_empty() {
        return Err(AuthorError::NoBones);
    }
    let mut seen = BTreeSet::new();
    for bone in bones {
        if !seen.insert(bone.name.as_str()) {
            return Err(AuthorError::DuplicateBone {
                bone: bone.name.clone(),
            });
        }
        if bone.segments.is_empty() {
            return Err(AuthorError::Boneless {
                bone: bone.name.clone(),
            });
        }
        for (head, tail) in &bone.segments {
            if !head.is_finite() || !tail.is_finite() {
                return Err(AuthorError::NonFiniteBone {
                    bone: bone.name.clone(),
                });
            }
        }
    }
    if triangles.is_empty() || positions.is_empty() {
        return Err(AuthorError::NoSurface);
    }
    if flesh.len() != positions.len() || !flesh.iter().any(|flag| *flag) {
        return Err(AuthorError::NoFlesh {
            vertices: positions.len(),
        });
    }
    let points: Vec<DVec3> = positions
        .iter()
        .enumerate()
        .map(|(vertex, point)| {
            let point = DVec3::from_array(*point);
            match point.is_finite() {
                true => Ok(point),
                false => Err(AuthorError::NonFinitePosition { vertex }),
            }
        })
        .collect::<Result<_, _>>()?;

    let count = points.len();
    let surface = Surface::build(&points, triangles);
    let euclidean = euclidean_fields(&points, bones);
    let radius = flesh_radii(&points, bones, &euclidean, count, settings);
    // The flesh is the caller's body; a garment primitive or any other layer is
    // a shell that has no anatomy of its own and inherits the flesh it lies on.
    let seeds = seed_sets(&euclidean, bones.len(), count, flesh);
    let geodesic = geodesic_fields(&surface, &seeds, bones.len());

    // The vertex's own flesh thickness: its Euclidean distance to the bone that
    // owns it, which every vertex has by construction.
    let mut local = vec![0.0_f64; count];
    for vertex in 0..count {
        local[vertex] = (0..bones.len())
            .map(|bone| euclidean[bone * count + vertex])
            .fold(f64::INFINITY, f64::min);
    }
    let mut weights = falloff(&geodesic.squared, &local, bones.len(), count, settings);
    let (axis, origin) = lateral_frame(bones);
    let mask = side_mask(&points, &axis, &origin, settings.midline_dead_zone);
    let mut neighbours = Neighbours::from_surface(&surface);
    for _ in 0..settings.smoothing {
        neighbours.smooth(&mut weights, bones.len(), count);
    }
    // The cells cut after smoothing and before every later stage: the boundary is
    // the solid's, and a shell must inherit the field that survived it.
    let cell_report =
        cells.map(|allowed| apply_cell_mask(&mut weights, allowed, count, bones.len()));
    // The mask runs last: a smoothing pass would carry masked mass back across
    // the midline from the dead-zone band, and side isolation is a product gate
    // on the encoded rows, not an aspiration.
    let inherited = inherit_from_flesh(&points, flesh, &mut weights, bones.len(), count);
    let (masked_vertices, masked_mass, mask_restored) =
        apply_side_mask(&mut weights, &mask, bones, count);

    let packed = pack_rows(&weights, bones, count, settings)?;
    let footprints = footprints(&packed.rows, bones, &radius, &seeds);
    Ok(AuthoredSkin {
        rows: packed.rows,
        report: AuthorReport {
            vertices: count,
            components: surface.components,
            bones: bones.len(),
            unreached_pairs: geodesic.unreached,
            flesh_vertices: flesh.iter().filter(|flag| **flag).count(),
            inherited_vertices: inherited,
            side_masked_vertices: masked_vertices,
            side_masked_mass: masked_mass,
            side_mask_restored: mask_restored,
            pruned: packed.pruned,
            discarded: packed.discarded,
            lateral_axis: axis.to_array(),
            midline_origin: origin.to_array(),
            footprints,
            cells: cell_report,
        },
    })
}

/// Replace each vertex's row with the budget its cells permit.
///
/// This is the hard cut and the narrow blend band in one step: a bone's cell does
/// not contain the vertex and is not inside its band, so the bone carries nothing
/// there whatever the geodesic field says, and the arm stops at the armpit crease
/// instead of thinning out across the chest wall — the failure the geodesic
/// envelope alone could not hold in an A-pose, where the upper arm rests against
/// the ribcage. Inside the band the shares ramp instead of stepping, so the seam
/// stays continuous. A row the cells do not reach keeps its heaviest influence,
/// and the exception is counted so the boundary is never silent.
fn apply_cell_mask(
    weights: &mut [f64],
    allowed: &VertexCells,
    count: usize,
    bones: usize,
) -> CellReport {
    let mut cut_vertices = 0;
    let mut cut_mass = 0.0;
    let mut permitted_total = 0_usize;
    let mut permitted_widest = 0;
    let mut unconstrained = 0;
    for vertex in 0..count {
        let permitted = allowed.allowed.get(vertex).filter(|set| !set.is_empty());
        let Some(permitted) = permitted else {
            unconstrained += 1;
            continue;
        };
        permitted_total += permitted.len();
        permitted_widest = permitted_widest.max(permitted.len());
        let mut moved = 0.0;
        for bone in 0..bones {
            let share = permitted
                .iter()
                .find(|(slot, _)| *slot == bone as u32)
                .map_or(0.0, |(_, share)| f64::from(*share));
            let before = weights[bone * count + vertex];
            moved += (before - share).abs();
            weights[bone * count + vertex] = share;
        }
        if moved <= 1e-9 {
            continue;
        }
        cut_vertices += 1;
        cut_mass += moved / 2.0;
    }
    let constrained = count.saturating_sub(unconstrained).max(1);
    CellReport {
        cut_vertices,
        cut_mass,
        restored: 0,
        allowed_mean: permitted_total as f64 / constrained as f64,
        allowed_widest: permitted_widest,
        unconstrained,
    }
}

/// Undirected surface adjacency with Euclidean edge lengths and components.
struct Surface {
    offsets: Vec<u32>,
    neighbours: Vec<u32>,
    lengths: Vec<f64>,
    components: usize,
}

impl Surface {
    fn build(points: &[DVec3], triangles: &[[u32; 3]]) -> Self {
        let mut edges = BTreeSet::new();
        for triangle in triangles {
            for (left, right) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                if left == right {
                    continue;
                }
                let key = match left < right {
                    true => (left, right),
                    false => (right, left),
                };
                edges.insert(key);
            }
        }
        let mut degree = vec![0_u32; points.len()];
        for (left, right) in &edges {
            degree[*left as usize] += 1;
            degree[*right as usize] += 1;
        }
        let mut offsets = vec![0_u32; points.len() + 1];
        for (index, count) in degree.iter().enumerate() {
            offsets[index + 1] = offsets[index] + count;
        }
        let mut cursor = offsets.clone();
        let mut neighbours = vec![0_u32; edges.len() * 2];
        let mut lengths = vec![0.0_f64; edges.len() * 2];
        for (left, right) in &edges {
            let length = points[*left as usize].distance(points[*right as usize]);
            for (from, to) in [(*left, *right), (*right, *left)] {
                let slot = cursor[from as usize] as usize;
                neighbours[slot] = to;
                lengths[slot] = length;
                cursor[from as usize] += 1;
            }
        }
        let components = label_components(&offsets, &neighbours, points.len());
        Self {
            offsets,
            neighbours,
            lengths,
            components,
        }
    }

    fn neighbours(&self, vertex: usize) -> std::ops::Range<usize> {
        let start = self.offsets[vertex] as usize;
        let end = self.offsets[vertex + 1] as usize;
        start..end
    }
}

/// BFS component labels; the count is what the report names.
fn label_components(offsets: &[u32], neighbours: &[u32], vertices: usize) -> usize {
    let mut label = vec![usize::MAX; vertices];
    let mut components = 0;
    let mut queue = Vec::new();
    for start in 0..vertices {
        if label[start] != usize::MAX {
            continue;
        }
        label[start] = components;
        queue.clear();
        queue.push(start);
        while let Some(vertex) = queue.pop() {
            let start = offsets[vertex] as usize;
            let end = offsets[vertex + 1] as usize;
            for &next in &neighbours[start..end] {
                let next = next as usize;
                if label[next] == usize::MAX {
                    label[next] = components;
                    queue.push(next);
                }
            }
        }
        components += 1;
    }
    components
}

/// Euclidean distance from every vertex to every bone's segment set.
fn euclidean_fields(points: &[DVec3], bones: &[DeformBone]) -> Vec<f64> {
    let count = points.len();
    let mut field = vec![f64::INFINITY; bones.len() * count];
    for (bone_index, bone) in bones.iter().enumerate() {
        let row = &mut field[bone_index * count..(bone_index + 1) * count];
        for (vertex, point) in points.iter().enumerate() {
            let mut best = f64::INFINITY;
            for (head, tail) in &bone.segments {
                best = best.min(segment_distance(*point, *head, *tail).0);
            }
            row[vertex] = best;
        }
    }
    field
}

/// Distance from a point to a segment, with the projection parameter `t`.
fn segment_distance(point: DVec3, head: DVec3, tail: DVec3) -> (f64, f64) {
    let axis = tail - head;
    let length_squared = axis.length_squared();
    if length_squared <= f64::EPSILON {
        return (point.distance(head), 0.0);
    }
    let t = ((point - head).dot(axis) / length_squared).clamp(0.0, 1.0);
    (point.distance(head + axis * t), t)
}

/// Each bone's flesh radius: a quantile of the surface distance among the
/// vertices the bone owns. Diagnostic only — seeding is by ownership, so a bone
/// cannot gain flesh by having a fat measured radius.
fn flesh_radii(
    points: &[DVec3],
    bones: &[DeformBone],
    euclidean: &[f64],
    count: usize,
    settings: &AuthorSettings,
) -> Vec<f64> {
    let mut owned: Vec<Vec<f64>> = vec![Vec::new(); bones.len()];
    for vertex in 0..count {
        let mut best = 0;
        let mut best_distance = f64::INFINITY;
        for bone in 0..bones.len() {
            let distance = euclidean[bone * count + vertex];
            if distance < best_distance {
                best_distance = distance;
                best = bone;
            }
        }
        if best_distance.is_finite() {
            owned[best].push(best_distance);
        }
    }
    let _ = points;
    owned
        .iter_mut()
        .map(|distances| {
            if distances.is_empty() {
                return 0.0;
            }
            distances.sort_by(f64::total_cmp);
            let index = ((distances.len() - 1) as f64 * settings.radius_quantile).round() as usize;
            distances[index]
        })
        .collect()
}

/// Each bone's seed set: the surface it is Euclidean-nearest to.
///
/// Ownership, not a capsule, is what makes the field truthful. A capsule is a
/// guess at a bone's thickness, and a torso bone's wide capsule reaches into the
/// biceps while a finger's narrow one loses the palm. The nearest-owner region
/// needs no such guess: a bone seeds exactly the flesh it is closest to, and the
/// geodesic field then carries its influence across the joint band and no
/// further. A bone with no owned vertices seeds nothing and carries no weight.
fn seed_sets(euclidean: &[f64], bones: usize, count: usize, flesh: &[bool]) -> Vec<Vec<u32>> {
    let mut seeds = vec![Vec::new(); bones];
    for vertex in 0..count {
        if !flesh[vertex] {
            continue;
        }
        let mut best = 0;
        let mut best_distance = f64::INFINITY;
        for bone in 0..bones {
            let distance = euclidean[bone * count + vertex];
            if distance < best_distance {
                best_distance = distance;
                best = bone;
            }
        }
        if best_distance.is_finite() {
            seeds[best].push(vertex as u32);
        }
    }
    seeds
}

/// Give every shell vertex the weights of the nearest flesh vertex.
///
/// A garment primitive, a detached toe or a nail is its own surface: its bones
/// are the ones it lies on, never the ones its isolated geodesic graph would
/// invent for it. Without this the underwear and the chest under it are weighted
/// twice, independently, and the layer slides through the body.
fn inherit_from_flesh(
    points: &[DVec3],
    flesh: &[bool],
    weights: &mut [f64],
    bones: usize,
    count: usize,
) -> usize {
    let sources: Vec<usize> = (0..count).filter(|vertex| flesh[*vertex]).collect();
    if sources.is_empty() {
        return 0;
    }
    let inherited: Vec<usize> = (0..count).filter(|vertex| !flesh[*vertex]).collect();
    if inherited.is_empty() {
        return 0;
    }
    let grid = Grid::build(points, &sources);
    let mut borrowed = 0;
    for vertex in inherited {
        let Some(source) = grid.nearest(points, vertex) else {
            continue;
        };
        for bone in 0..bones {
            weights[bone * count + vertex] = weights[bone * count + source];
        }
        borrowed += 1;
    }
    borrowed
}

/// A uniform grid over the flesh vertices, for nearest-source queries.
struct Grid {
    cell: f64,
    buckets: BTreeMap<(i64, i64, i64), Vec<usize>>,
}

impl Grid {
    fn build(points: &[DVec3], sources: &[usize]) -> Self {
        let mut low = DVec3::splat(f64::INFINITY);
        let mut high = DVec3::splat(f64::NEG_INFINITY);
        for source in sources {
            low = low.min(points[*source]);
            high = high.max(points[*source]);
        }
        let extent = (high - low).max_element();
        let cell = (extent / 64.0).max(1e-3);
        let mut buckets: BTreeMap<(i64, i64, i64), Vec<usize>> = BTreeMap::new();
        for source in sources {
            buckets
                .entry(Self::key(points[*source], cell))
                .or_default()
                .push(*source);
        }
        Self { cell, buckets }
    }

    fn key(point: DVec3, cell: f64) -> (i64, i64, i64) {
        (
            (point.x / cell).floor() as i64,
            (point.y / cell).floor() as i64,
            (point.z / cell).floor() as i64,
        )
    }

    /// The nearest source vertex, searching outward one grid shell at a time.
    ///
    /// The search stops as soon as the best candidate found is closer than the
    /// inner boundary of the next shell, so a loose layer standing off the body
    /// (a boxer short at 110 mm) is still resolved exactly.
    fn nearest(&self, points: &[DVec3], vertex: usize) -> Option<usize> {
        let (x, y, z) = Self::key(points[vertex], self.cell);
        let point = points[vertex];
        let mut best: Option<(f64, usize)> = None;
        for ring in 0..=64_i64 {
            if let Some((distance, _)) = best
                && (ring as f64 - 1.0) * self.cell > distance
            {
                break;
            }
            for dx in -ring..=ring {
                for dy in -ring..=ring {
                    for dz in -ring..=ring {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                            continue;
                        }
                        let Some(bucket) = self.buckets.get(&(x + dx, y + dy, z + dz)) else {
                            continue;
                        };
                        for source in bucket {
                            let distance = points[*source].distance(point);
                            if best.is_none_or(|(known, _)| distance < known) {
                                best = Some((distance, *source));
                            }
                        }
                    }
                }
            }
        }
        best.map(|(_, source)| source)
    }
}

/// Squared geodesic distance per bone; infinite where the bone does not reach.
struct GeodesicFields {
    squared: Vec<f64>,
    /// Bone-vertex pairs no surface path resolved.
    unreached: usize,
}

/// Multi-source Dijkstra from a bone's owned region, per component that it seeds.
fn geodesic_fields(surface: &Surface, seeds: &[Vec<u32>], bones: usize) -> GeodesicFields {
    let count = surface.offsets.len() - 1;
    let mut squared = vec![f64::INFINITY; bones * count];
    let mut unreached = 0;
    for (bone, bone_seeds) in seeds.iter().enumerate().take(bones) {
        if bone_seeds.is_empty() {
            unreached += count;
            continue;
        }
        let base = bone * count;
        let row = &mut squared[base..base + count];
        let mut heap = BinaryHeap::with_capacity(bone_seeds.len() * 4);
        for seed in bone_seeds {
            row[*seed as usize] = 0.0;
            heap.push(Candidate {
                distance: 0.0,
                vertex: *seed,
            });
        }
        while let Some(Candidate { distance, vertex }) = heap.pop() {
            let vertex = vertex as usize;
            let known = row[vertex];
            if distance > known {
                continue;
            }
            for slot in surface.neighbours(vertex) {
                let next = surface.neighbours[slot] as usize;
                let total = known + surface.lengths[slot];
                if total < row[next] {
                    row[next] = total;
                    heap.push(Candidate {
                        distance: total,
                        vertex: next as u32,
                    });
                }
            }
        }
        for value in row.iter_mut() {
            match value.is_finite() {
                true => *value *= *value,
                false => unreached += 1,
            }
        }
    }
    GeodesicFields { squared, unreached }
}

/// One Dijkstra frontier entry, ordered by distance then vertex for determinism.
struct Candidate {
    distance: f64,
    vertex: u32,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.vertex == other.vertex
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        // The heap is a max-heap; invert so the nearest vertex surfaces first.
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.vertex.cmp(&self.vertex))
    }
}

/// Inverse-distance falloff over each bone's geodesic field.
///
/// `w = (r / (d + r))^p` with `r` the vertex's own flesh thickness — its
/// Euclidean distance to the bone that owns it. A joint therefore blends over a
/// band proportional to the flesh it joins: wide at the hip, narrow at a finger,
/// with no per-region number anywhere. An unreachable bone contributes nothing.
fn falloff(
    squared: &[f64],
    local: &[f64],
    bones: usize,
    count: usize,
    settings: &AuthorSettings,
) -> Vec<f64> {
    let mut weights = vec![0.0_f64; bones * count];
    for bone in 0..bones {
        let row = &squared[bone * count..(bone + 1) * count];
        let target = &mut weights[bone * count..(bone + 1) * count];
        for vertex in 0..count {
            if !row[vertex].is_finite() {
                continue;
            }
            let reach = match settings.blend_in_flesh_radii {
                true => local[vertex].max(1e-4),
                false => 1.0,
            };
            let ratio = reach / (row[vertex].sqrt() + reach);
            target[vertex] = ratio.powf(settings.exponent);
        }
    }
    weights
}

/// The lateral axis and midline point the isolation mask splits the body on.
///
/// The axis is the mean left bone head minus the mean right bone head: mirrored
/// limbs cancel their vertical and depth components, so the difference is the
/// body's lateral direction even in an A-pose.
fn lateral_frame(bones: &[DeformBone]) -> (DVec3, DVec3) {
    let mean = |side: BodySide| -> Option<DVec3> {
        let mut sum = DVec3::ZERO;
        let mut count = 0;
        for bone in bones.iter().filter(|bone| bone.side == Some(side)) {
            if let Some((head, _)) = bone.segments.first() {
                sum += *head;
                count += 1;
            }
        }
        match count {
            0 => None,
            _ => Some(sum / count as f64),
        }
    };
    match (mean(BodySide::Left), mean(BodySide::Right)) {
        (Some(left), Some(right)) if left.distance(right) > f64::EPSILON => {
            (left - right, (left + right) * 0.5)
        }
        _ => (DVec3::ZERO, DVec3::ZERO),
    }
}

/// Vertices that must not carry the opposite side: clear of the midline band.
fn side_mask(
    points: &[DVec3],
    axis: &DVec3,
    origin: &DVec3,
    dead_zone: f64,
) -> Vec<Option<BodySide>> {
    match axis.length_squared() > f64::EPSILON {
        true => points
            .iter()
            .map(|point| {
                let lateral = (*point - *origin).dot(*axis);
                match lateral {
                    value if value > dead_zone => Some(BodySide::Left),
                    value if value < -dead_zone => Some(BodySide::Right),
                    _ => None,
                }
            })
            .collect(),
        false => vec![None; points.len()],
    }
}

/// Remove every opposite-side influence from a clearly one-sided vertex.
///
/// A vertex whose only influence was the opposite side keeps its heaviest such
/// bone: the isolation rule must not empty a row, and the number of those
/// vertices is reported so the exception is never silent.
fn apply_side_mask(
    weights: &mut [f64],
    mask: &[Option<BodySide>],
    bones: &[DeformBone],
    count: usize,
) -> (usize, f64, usize) {
    let mut vertices = 0;
    let mut mass = 0.0;
    let mut restored = 0;
    for (vertex, side) in mask.iter().enumerate() {
        let Some(side) = side else { continue };
        let opposite = side.opposite();
        let mut removed = 0.0;
        let mut heaviest = None::<(f64, usize)>;
        for (bone, spec) in bones.iter().enumerate() {
            if spec.side != Some(opposite) {
                continue;
            }
            let weight = weights[bone * count + vertex];
            if weight <= 0.0 {
                continue;
            }
            removed += weight;
            if heaviest.is_none_or(|(known, _)| weight > known) {
                heaviest = Some((weight, bone));
            }
        }
        if removed <= 0.0 {
            continue;
        }
        for (bone, spec) in bones.iter().enumerate() {
            if spec.side == Some(opposite) {
                weights[bone * count + vertex] = 0.0;
            }
        }
        vertices += 1;
        mass += removed;
        let remaining: f64 = (0..bones.len())
            .map(|bone| weights[bone * count + vertex])
            .sum();
        if remaining > 0.0 {
            continue;
        }
        if let Some((weight, bone)) = heaviest {
            weights[bone * count + vertex] = weight;
            restored += 1;
        }
    }
    (vertices, mass, restored)
}

/// Surface neighbour lists for the smoothing passes.
struct Neighbours {
    offsets: Vec<u32>,
    neighbours: Vec<u32>,
}

impl Neighbours {
    fn from_surface(surface: &Surface) -> Self {
        Self {
            offsets: surface.offsets.clone(),
            neighbours: surface.neighbours.clone(),
        }
    }

    /// Two-pass-friendly smoothing: one ring average, half the vertex's own mass.
    fn smooth(&mut self, weights: &mut [f64], bones: usize, count: usize) {
        let mut next = vec![0.0_f64; weights.len()];
        for bone in 0..bones {
            let base = bone * count;
            for vertex in 0..count {
                let range = self.offsets[vertex] as usize..self.offsets[vertex + 1] as usize;
                let degree = range.len();
                if degree == 0 {
                    next[base + vertex] = weights[base + vertex];
                    continue;
                }
                let mut sum = 0.0;
                for slot in range {
                    sum += weights[base + self.neighbours[slot] as usize];
                }
                next[base + vertex] = 0.5 * weights[base + vertex] + 0.5 * sum / degree as f64;
            }
        }
        weights.copy_from_slice(&next);
    }
}

/// One authored skin's packed rows and the mass the pack moved.
struct Packed {
    rows: Vec<Influences>,
    pruned: usize,
    discarded: Vec<(String, f64)>,
}

/// Prune, cap and normalize every row through the one pack owner.
fn pack_rows(
    weights: &[f64],
    bones: &[DeformBone],
    count: usize,
    settings: &AuthorSettings,
) -> Result<Packed, AuthorError> {
    let mut rows = Vec::with_capacity(count);
    let mut pruned = 0;
    let mut discarded: Vec<(String, f64)> = Vec::new();
    for vertex in 0..count {
        let mut row: Vec<(usize, f64)> = (0..bones.len())
            .map(|bone| (bone, weights[bone * count + vertex]))
            .filter(|(_, weight)| *weight > 0.0)
            .collect();
        if row.is_empty() {
            return Err(AuthorError::Unreachable { vertex });
        }
        let maximum = row
            .iter()
            .map(|(_, weight)| *weight)
            .fold(0.0_f64, f64::max);
        let threshold = maximum * settings.prune_ratio;
        let before = row.len();
        row.retain(|(_, weight)| *weight >= threshold);
        pruned += before - row.len();
        row.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        row.truncate(crate::weight_norm::MAX_INFLUENCES);
        let mut influences = Influences::new();
        for (bone, weight) in &row {
            if influences
                .insert(bones[*bone].name.clone(), *weight as f32)
                .is_some()
            {
                return Err(AuthorError::DuplicateInfluence {
                    vertex,
                    bone: bones[*bone].name.clone(),
                });
            }
        }
        let packed = pack_influences(&influences, &BTreeSet::new())
            .map_err(|error| AuthorError::Pack { vertex, error })?;
        for (bone, weight) in &influences {
            if !packed.contains_key(bone) {
                discarded.push((bone.clone(), f64::from(*weight)));
            }
        }
        rows.push(packed);
    }
    Ok(Packed {
        rows,
        pruned,
        discarded,
    })
}

/// Per-bone dominance and mass over the packed rows.
fn footprints(
    rows: &[Influences],
    bones: &[DeformBone],
    radius: &[f64],
    seeds: &[Vec<u32>],
) -> Vec<BoneFootprint> {
    let mut mass = vec![0.0_f64; bones.len()];
    let mut dominated = vec![0_usize; bones.len()];
    let mut dominance = vec![0.0_f64; bones.len()];
    let index: BTreeMap<&str, usize> = bones
        .iter()
        .enumerate()
        .map(|(slot, bone)| (bone.name.as_str(), slot))
        .collect();
    for row in rows {
        let mut best: Option<(&str, f32)> = None;
        for (bone, weight) in row {
            if let Some(slot) = index.get(bone.as_str()) {
                mass[*slot] += f64::from(*weight);
            }
            if best.is_none_or(|(_, current)| *weight > current) {
                best = Some((bone.as_str(), *weight));
            }
        }
        if let Some((bone, weight)) = best
            && let Some(slot) = index.get(bone)
        {
            dominated[*slot] += 1;
            dominance[*slot] += f64::from(weight);
        }
    }
    bones
        .iter()
        .enumerate()
        .map(|(slot, bone)| BoneFootprint {
            name: bone.name.clone(),
            radius: radius[slot],
            seeds: seeds[slot].len(),
            dominated: dominated[slot],
            dominance: match dominated[slot] {
                0 => 0.0,
                count => dominance[slot] / count as f64,
            },
            mass: mass[slot],
        })
        .collect()
}
