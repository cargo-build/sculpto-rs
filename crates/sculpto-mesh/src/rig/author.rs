//! Author skin weights for a deform skeleton directly from the flesh.
//!
//! The method is **geodesic bone envelopes**: every deform bone owns the flesh
//! around the segments it spans, the envelope is measured in that bone's own flesh
//! section, and a vertex's weight for a bone falls off with the **surface-geodesic**
//! distance from it. Geodesic distance is the occlusion term: a torso-wall vertex
//! beside the armpit is two centimetres from the arm axis but a long way round the
//! crease, so the arm cannot reach it the way a Euclidean envelope would.
//!
//! ## The ownership boundary
//!
//! An envelope is bounded, and the bound is what keeps an arm from owning a chest.
//! [`section_radii`] measures each bone's **own flesh section**: the inscribed
//! radius of the surface along the bone's own segments, read off the geometry
//! rather than off a vertex ownership that a fat trunk would inflate — the older
//! rule reported the ribcage's thickness as the arm's own radius (121 mm) and the
//! arm owned 190 mm of back with it. Three layers then bound the envelope:
//!
//! 1. **Ownership.** Flesh belongs to the bone whose *section* is nearest to it,
//!    not whose axis is: the spine's 90 mm section reaches the chest wall that its
//!    0 mm axis cannot, and the arm's 29 mm section stops where its axis would not.
//! 2. **Decay.** A bone's ownership falls off with `ownership_band` sections of
//!    excess distance past its own flesh, so the handover between two bones is a
//!    continuous field rather than a step at a partition boundary.
//! 3. **Reach.** A bone's influence is cut to zero at `reach_band` sections of
//!    **surface path** from the flesh it seeds. Beyond that the weight is zero, not
//!    small: three percent of arm weight on a sternum tears it three times over.
//!
//! That is the armpit crease, measured instead of named. The crease is the narrowest
//! flesh between the deltoid and the ribcage: the surface path from the arm's own
//! skin to the chest wall crosses it, and the chest wall stands a whole trunk section
//! away from the arm section. Measured on the M2 body at seat `eb318020`, the chest
//! wall is 74 mm from the arm axis and 113 mm of surface path from the arm's own
//! skin; the arm's section is 29 mm. Before this boundary existed chest flesh carried
//! up to `0.797` arm weight and an arm rotation dragged the ribcage 100 mm; with it,
//! arm weight on chest flesh is exactly zero and the chest moves 0.0 mm.
//!
//! The residual limit is the axilla fold itself: flesh between the deltoid and the
//! ribcage is 90-130 mm from the shoulder joint, and any handover there moves a
//! vertex that far under arm rotation. The decay and the smoothing widen the
//! handover band until the field would stop being one owner per limb; a shoulder
//! joint seated closer to the flesh is what shortens the lever, and that is a seat
//! problem, not a weight one.
//!
//! Concentration comes from the falloff exponent, not from a smoothing hack: a
//! vertex inside one section carries one owner at weight one, and only the overlap
//! of two sections (a joint) produces a blend. A shell that no bone seeds (a garment
//! primitive is its own surface) inherits the flesh it lies on, which is declared in
//! the report, never silent.
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

pub mod knee;
pub mod parted;
mod seams;

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
    /// Falloff distances are measured in units of the bone's own flesh section,
    /// so one rule serves a nine-centimetre hip and an eight-millimetre finger.
    pub blend_in_flesh_radii: bool,
    /// Sections of surface path a bone's influence may travel beyond the flesh it
    /// seeds before it is cut to zero: the crease an envelope may not cross.
    pub reach_band: f64,
    /// Sections over which ownership decays past a bone's own flesh. Two neighbouring
    /// fingers are one section apart; the crease between an arm and a ribcage is one
    /// section of the thinner bone.
    pub ownership_band: f64,
    /// Sections of a bone's own flesh over which its geodesic falloff decays: the
    /// width of a joint's blend band, in the units of the bone that reaches.
    pub falloff_band: f64,
    /// Weights under this fraction of a row's maximum are dropped before packing.
    pub prune_ratio: f64,
    /// Surface smoothing passes over the weight field.
    pub smoothing: usize,
    /// Half-width in metres over which opposite-side influence fades to zero.
    pub midline_dead_zone: f64,
}

impl Default for AuthorSettings {
    fn default() -> Self {
        Self {
            exponent: 4.0,
            blend_in_flesh_radii: true,
            reach_band: 3.0,
            ownership_band: 0.6,
            falloff_band: 2.0,
            prune_ratio: 0.02,
            smoothing: 3,
            midline_dead_zone: 0.010,
        }
    }
}

/// One bone's authored footprint.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneFootprint {
    /// Bone name.
    pub name: String,
    /// Its own flesh section: the inscribed radius measured along its segments.
    pub section: f64,
    /// The surface path its influence is allowed to travel, from the section.
    pub reach: f64,
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
    /// Coincident boundary endpoint groups that share one body influence row.
    pub seam_weight_groups: usize,
    /// Vertices whose opposite-side mass the isolation mask removed.
    pub side_masked_vertices: usize,
    /// Mass the isolation mask removed, in raw pre-pack units.
    pub side_masked_mass: f64,
    /// Vertices whose only influence was the opposite side and kept it.
    pub side_mask_restored: usize,
    /// Flesh vertices the section rule owns through a bone other than the nearest
    /// one: the flesh the boundary moved, reported rather than assumed.
    pub off_nearest: usize,
    /// Vertices whose envelope mass the reach bound removed.
    pub envelope_cut_vertices: usize,
    /// Mass the reach bound removed, in raw pre-pack units.
    pub envelope_cut_mass: f64,
    /// Vertices the reach bound would have emptied and left with their heaviest
    /// influence: the bound must never starve a row, and the exception is counted.
    pub envelope_restored: usize,
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
    let seams = seams::SeamWeights::find(positions, triangles, flesh);
    let surface = Surface::build(&points, triangles, seams.links());
    let euclidean = euclidean_fields(&points, bones);
    // The bone's own flesh section, measured from the surface. Ownership uses it
    // as a capsule; the falloff and the reach bound use it as their unit.
    let section = section_radii(&points, bones);
    // The flesh is the caller's body; a garment primitive or any other layer is
    // a shell that has no anatomy of its own and inherits the flesh it lies on.
    let (seeds, off_nearest) = seed_sets(&euclidean, &section, bones.len(), count, flesh);
    let geodesic = geodesic_fields(&surface, &seeds, bones.len());

    let mut weights = falloff(
        &geodesic.squared,
        &euclidean,
        &section,
        bones.len(),
        count,
        settings,
    );
    let (axis, origin) = lateral_frame(bones);
    let mut mask = side_mask(&points, &axis, &origin, settings.midline_dead_zone);
    let mut neighbours = Neighbours::from_surface(&surface);
    for _ in 0..settings.smoothing {
        neighbours.smooth(&mut weights, bones.len(), count);
    }
    // The bound runs after smoothing and before the shell inheritance: a smoothing
    // pass would carry cut mass back across the crease, and a shell must inherit
    // the bounded field of the flesh it lies on.
    let reach = reach_bounds(&section, settings);
    let (envelope_cut_vertices, envelope_cut_mass, envelope_restored) =
        apply_envelope_bound(&mut weights, &geodesic.squared, &reach, bones.len(), count);
    seams.share(&mut weights, count, &mut mask);
    let inherited = inherit_from_flesh(&points, flesh, &mut weights, bones.len(), count);
    resolve_bilateral_overlap(&mut weights, bones, count);
    // The mask runs last: a smoothing pass would carry masked mass back across
    // the midline from the dead-zone band, and side isolation is a product gate
    // on the encoded rows, not an aspiration.
    let (masked_vertices, masked_mass, mask_restored) =
        apply_side_mask(&mut weights, &mask, bones, count);

    let packed = pack_rows(&weights, bones, count, settings)?;
    let footprints = footprints(&packed.rows, bones, &section, &reach, &seeds);
    Ok(AuthoredSkin {
        rows: packed.rows,
        report: AuthorReport {
            vertices: count,
            components: surface.components,
            bones: bones.len(),
            unreached_pairs: geodesic.unreached,
            flesh_vertices: flesh.iter().filter(|flag| **flag).count(),
            inherited_vertices: inherited,
            seam_weight_groups: seams.len(),
            side_masked_vertices: masked_vertices,
            side_masked_mass: masked_mass,
            side_mask_restored: mask_restored,
            off_nearest,
            envelope_cut_vertices,
            envelope_cut_mass,
            envelope_restored,
            pruned: packed.pruned,
            discarded: packed.discarded,
            lateral_axis: axis.to_array(),
            midline_origin: origin.to_array(),
            footprints,
        },
    })
}

/// Undirected surface adjacency with Euclidean edge lengths and components.
struct Surface {
    offsets: Vec<u32>,
    neighbours: Vec<u32>,
    lengths: Vec<f64>,
    components: usize,
}

impl Surface {
    fn build(
        points: &[DVec3],
        triangles: &[[u32; 3]],
        seams: impl Iterator<Item = (u32, u32)>,
    ) -> Self {
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
        edges.extend(seams);
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

/// Where a bone's own flesh section is sampled along its segments: the caps are
/// skipped, because a bone that ends at the skin measures its own tip there.
const SECTION_START: f64 = 0.12;
/// Upper end of the same interior span.
const SECTION_END: f64 = 0.88;
/// Sample spacing along a segment when measuring the section, metres.
const SECTION_STEP: f64 = 0.010;
/// A section never shrinks below this, so a degenerate bone still gets a unit.
const SECTION_FLOOR_M: f64 = 1e-3;

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

/// Each bone's own flesh section: the largest inscribed radius of the surface
/// along the interior of its segments, metres.
///
/// Measured from the surface, never from the vertices the bone happens to be
/// nearest to: a limb axis seated inside a fat trunk would report the trunk's
/// thickness as its own radius, which is how an arm envelope grew to 121 mm and
/// swallowed the ribcage. A segment's caps are skipped, because a bone that ends
/// at the skin (a crown, a fingertip) measures its own tip there, not its section.
fn section_radii(points: &[DVec3], bones: &[DeformBone]) -> Vec<f64> {
    let all: Vec<usize> = (0..points.len()).collect();
    let grid = Grid::build(points, &all);
    bones
        .iter()
        .map(|bone| {
            let mut best = 0.0_f64;
            for (head, tail) in &bone.segments {
                let axis = *tail - *head;
                let length = axis.length();
                if length <= f64::EPSILON {
                    continue;
                }
                let steps = ((length * (SECTION_END - SECTION_START) / SECTION_STEP).ceil()
                    as usize)
                    .max(2);
                for step in 0..=steps {
                    let t =
                        SECTION_START + (SECTION_END - SECTION_START) * step as f64 / steps as f64;
                    let sample = *head + axis * t;
                    if let Some(distance) = grid.distance_to_nearest(points, sample) {
                        best = best.max(distance);
                    }
                }
            }
            best
        })
        .collect()
}

/// Each bone's seed set: the flesh belongs to the bone whose **own section** is
/// nearest to it.
///
/// A capsule is a guess at a bone's thickness, and a torso bone's wide capsule
/// reaches into the biceps while a finger's narrow one loses the palm. Seeding by
/// the bare nearest bone has the opposite defect on a fat body: an A-pose arm
/// rests against the ribcage, so the chest wall is nearer to the arm axis than to
/// the midline spine column and one arm owned 190 mm of back. Measuring the
/// distance to the bone's *section* instead of to the bone settles both: the
/// spine's 90 mm section reaches the chest wall that its 0 mm axis cannot, and the
/// arm's 29 mm section stops at the armpit crease that its axis would cross.
///
/// Every flesh vertex gets exactly one owner, so the field starts at weight one and
/// blends only where two sections meet. The report counts the vertices this rule
/// moves off the nearest bone — [`AuthorReport::off_nearest_bone`] — so the effect of
/// the boundary is measurable, not asserted.
fn seed_sets(
    euclidean: &[f64],
    section: &[f64],
    bones: usize,
    count: usize,
    flesh: &[bool],
) -> (Vec<Vec<u32>>, usize) {
    let mut seeds = vec![Vec::new(); bones];
    let mut off_nearest = 0;
    for vertex in 0..count {
        if !flesh[vertex] {
            continue;
        }
        let mut best = None::<((f64, f64), usize)>;
        let mut nearest = None::<(f64, usize)>;
        for bone in 0..bones {
            let distance = euclidean[bone * count + vertex];
            if !distance.is_finite() {
                continue;
            }
            if nearest.is_none_or(|(known, _)| distance < known) {
                nearest = Some((distance, bone));
            }
            let surface = (distance - section[bone]).max(0.0);
            let key = (surface, distance);
            if best.is_none_or(|(known, _)| key < known) {
                best = Some((key, bone));
            }
        }
        if let Some((_, bone)) = best {
            if nearest.is_some_and(|(_, raw)| raw != bone) {
                off_nearest += 1;
            }
            seeds[bone].push(vertex as u32);
        }
    }
    (seeds, off_nearest)
}

/// How far each bone's influence may travel, metres: its own section, scaled.
fn reach_bounds(section: &[f64], settings: &AuthorSettings) -> Vec<f64> {
    section
        .iter()
        .map(|radius| settings.reach_band * radius.max(SECTION_FLOOR_M))
        .collect()
}

/// Cut every bone's weight beyond its reach, and never starve a row.
///
/// This is the ownership boundary itself: a bone whose surface path to a vertex
/// is longer than its reach owns nothing there, so the arm's envelope stops at
/// the armpit crease instead of thinning out across the chest. A row that would
/// be emptied keeps its heaviest influence, and the count of those is reported.
fn apply_envelope_bound(
    weights: &mut [f64],
    squared: &[f64],
    reach: &[f64],
    bones: usize,
    count: usize,
) -> (usize, f64, usize) {
    let mut vertices = 0;
    let mut mass = 0.0;
    let mut restored = 0;
    for vertex in 0..count {
        let mut removed = 0.0;
        let mut heaviest = None::<(f64, usize)>;
        for bone in 0..bones {
            let weight = weights[bone * count + vertex];
            if weight <= 0.0 {
                continue;
            }
            if squared[bone * count + vertex] <= reach[bone] * reach[bone] {
                continue;
            }
            removed += weight;
            if heaviest.is_none_or(|(known, _)| weight > known) {
                heaviest = Some((weight, bone));
            }
            weights[bone * count + vertex] = 0.0;
        }
        if removed <= 0.0 {
            continue;
        }
        vertices += 1;
        mass += removed;
        let remaining: f64 = (0..bones).map(|bone| weights[bone * count + vertex]).sum();
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

/// Interpolate the flesh field through a compact distance kernel.
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
    inherited.iter().for_each(|&vertex| {
        let neighbours = grid.blend(points, points[vertex]);
        (0..bones).for_each(|bone| {
            weights[bone * count + vertex] = neighbours
                .iter()
                .map(|(source, fraction)| weights[bone * count + source] * fraction)
                .sum();
        });
    });
    inherited.len()
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

    /// The support follows nearest distance continuously; its boundary has zero mass.
    /// A nearest-vertex switch across an opening must not tear a spanning garment.
    fn blend(&self, points: &[DVec3], point: DVec3) -> Vec<(usize, f64)> {
        let Some((distance, source)) = self.search(points, point) else {
            return Vec::new();
        };
        if distance < 1e-9 {
            return vec![(source, 1.0)];
        }
        let radius = distance + self.cell;
        let low = Self::key(point - DVec3::splat(radius), self.cell);
        let high = Self::key(point + DVec3::splat(radius), self.cell);
        let mut blend: Vec<(usize, f64)> = (low.0..=high.0)
            .flat_map(|x| (low.1..=high.1).map(move |y| (x, y)))
            .flat_map(|(x, y)| (low.2..=high.2).map(move |z| (x, y, z)))
            .filter_map(|key| self.buckets.get(&key))
            .flatten()
            .filter_map(|&source| {
                let squared = points[source].distance_squared(point);
                let taper = 1.0 - squared / radius.powi(2);
                match taper > 0.0 {
                    true => Some((source, taper.powi(2) / squared)),
                    false => None,
                }
            })
            .collect();
        let total: f64 = blend.iter().map(|(_, weight)| weight).sum();
        blend.iter_mut().for_each(|(_, weight)| *weight /= total);
        blend
    }

    /// Distance from an arbitrary point to the nearest source vertex.
    fn distance_to_nearest(&self, points: &[DVec3], point: DVec3) -> Option<f64> {
        self.search(points, point).map(|(distance, _)| distance)
    }

    fn search(&self, points: &[DVec3], point: DVec3) -> Option<(f64, usize)> {
        let (x, y, z) = Self::key(point, self.cell);
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
        best
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
        surface_distances(surface, bone_seeds, row, |_| true);
        for value in row.iter_mut() {
            match value.is_finite() {
                true => *value *= *value,
                false => unreached += 1,
            }
        }
    }
    GeodesicFields { squared, unreached }
}

/// Multi-source surface distances within a connected anatomical region. @req DECL0013
fn surface_distances(
    surface: &Surface,
    seeds: &[u32],
    row: &mut [f64],
    allowed: impl Fn(usize) -> bool,
) {
    let mut heap = BinaryHeap::with_capacity(seeds.len());
    for &seed in seeds {
        row[seed as usize] = 0.0;
        heap.push(Candidate {
            distance: 0.0,
            vertex: seed,
        });
    }
    while let Some(Candidate { distance, vertex }) = heap.pop() {
        let vertex = vertex as usize;
        if distance > row[vertex] {
            continue;
        }
        for slot in surface.neighbours(vertex) {
            let next = surface.neighbours[slot] as usize;
            let total = distance + surface.lengths[slot];
            if allowed(next) && total < row[next] {
                row[next] = total;
                heap.push(Candidate {
                    distance: total,
                    vertex: next as u32,
                });
            }
        }
    }
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

/// Inverse-distance falloff over each bone's geodesic field, gated by ownership.
///
/// `w = ownership * (r / (d + r))^p`, with `r` the **bone's own** flesh section and
/// `d` the surface path from the bone's seeds. A joint therefore blends over a
/// band the size of the bone's own flesh: a thin arm decays over its own
/// centimetres and cannot smear across a trunk, while a fat trunk blends wide as
/// it should. Using the *vertex's* thickness here — the older rule — let an arm
/// reach 90 mm into a chest, because the reach was the chest's fatness rather than
/// the arm's.
///
/// `ownership = exp(-(d_e - r) / (reach_band r))` is what keeps the boundary from
/// becoming a step. A hard partition (one owner at one, everyone else from the falloff) puts
/// the whole weight jump on the two rings that straddle the crease; at a 130 mm
/// lever arm that is a tear of several times the rest length. The excess distance
/// past a bone's own section is continuous in space, so an exponential in it makes
/// the ownership itself continuous, and the blend spans about one section of the
/// thinner bone — the width of the crease being crossed. An unreachable bone
/// contributes nothing.
fn falloff(
    squared: &[f64],
    euclidean: &[f64],
    section: &[f64],
    bones: usize,
    count: usize,
    settings: &AuthorSettings,
) -> Vec<f64> {
    let mut weights = vec![0.0_f64; bones * count];
    for bone in 0..bones {
        let row = &squared[bone * count..(bone + 1) * count];
        let distance = &euclidean[bone * count..(bone + 1) * count];
        let target = &mut weights[bone * count..(bone + 1) * count];
        let flesh = section[bone].max(SECTION_FLOOR_M);
        let reach = match settings.blend_in_flesh_radii {
            true => flesh * settings.falloff_band,
            false => 1.0,
        };
        let span = (flesh * settings.ownership_band).max(SECTION_FLOOR_M);
        for vertex in 0..count {
            if !row[vertex].is_finite() {
                continue;
            }
            let excess = (distance[vertex] - section[bone]).max(0.0);
            let ownership = (-excess / span).exp();
            let ratio = reach / (row[vertex].sqrt() + reach);
            target[vertex] = ownership * ratio.powf(settings.exponent);
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
            ((left - right).normalize(), (left + right) * 0.5)
        }
        _ => (DVec3::ZERO, DVec3::ZERO),
    }
}

/// Signed progress through the metre-wide isolation band, clamped at its ends.
fn side_mask(points: &[DVec3], axis: &DVec3, origin: &DVec3, dead_zone: f64) -> Vec<f64> {
    match axis.length_squared() > f64::EPSILON {
        true => points
            .iter()
            .map(|point| {
                let lateral = (*point - *origin).dot(*axis);
                match dead_zone > 0.0 {
                    true => (lateral / dead_zone).clamp(-1.0, 1.0),
                    false if lateral != 0.0 => lateral.signum(),
                    false => 0.0,
                }
            })
            .collect(),
        false => vec![0.0; points.len()],
    }
}

/// Give shared branch influence to the unsided bone that already reaches the vertex. @req DECL0013
fn resolve_bilateral_overlap(weights: &mut [f64], bones: &[DeformBone], count: usize) {
    for (hub, bone) in bones
        .iter()
        .enumerate()
        .filter(|(_, bone)| bone.side.is_none())
    {
        let mut branches = bones.iter().enumerate().filter_map(|(index, branch)| {
            let side = branch.side?;
            let head = branch.segments.first()?.0;
            bone.segments
                .iter()
                .any(|&(_, tail)| tail == head)
                .then_some((index, side))
        });
        let (Some((a, side)), Some((b, other)), None) =
            (branches.next(), branches.next(), branches.next())
        else {
            continue;
        };
        if side == other {
            continue;
        }
        for vertex in 0..count {
            let (centre, first, second) =
                (hub * count + vertex, a * count + vertex, b * count + vertex);
            let shared = weights[centre].min(weights[first]).min(weights[second]);
            weights[first] -= shared;
            weights[second] -= shared;
            weights[centre] += 2.0 * shared;
        }
    }
}

/// Fade opposite-side mass smoothly inside the band, remove it beyond the band.
///
/// A vertex whose only influence was the opposite side keeps its heaviest such
/// bone: the isolation rule must not empty a row, and the number of those
/// vertices is reported so the exception is never silent.
fn apply_side_mask(
    weights: &mut [f64],
    mask: &[f64],
    bones: &[DeformBone],
    count: usize,
) -> (usize, f64, usize) {
    let mut vertices = 0;
    let mut mass = 0.0;
    let mut restored = 0;
    for (vertex, progress) in mask.iter().copied().enumerate() {
        let side = match progress {
            value if value > 0.0 => BodySide::Left,
            value if value < 0.0 => BodySide::Right,
            _ => continue,
        };
        let opposite = side.opposite();
        let t = progress.abs();
        let removed_fraction = t * t * (3.0 - 2.0 * t);
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
            removed += weight * removed_fraction;
            if heaviest.is_none_or(|(known, _)| weight > known) {
                heaviest = Some((weight, bone));
            }
        }
        if removed <= 0.0 {
            continue;
        }
        for (bone, spec) in bones.iter().enumerate() {
            if spec.side == Some(opposite) {
                weights[bone * count + vertex] *= 1.0 - removed_fraction;
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
    section: &[f64],
    reach: &[f64],
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
            section: section[slot],
            reach: reach[slot],
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

#[cfg(test)]
mod continuity_tests {
    use super::*;

    fn sides(span: f64) -> Vec<DeformBone> {
        [(BodySide::Left, span), (BodySide::Right, -span)]
            .into_iter()
            .map(|(side, x)| DeformBone {
                name: format!("{side:?}"),
                side: Some(side),
                segments: vec![(DVec3::new(x, 0.0, 0.0), DVec3::new(x, 1.0, 0.0))],
            })
            .collect()
    }

    #[test]
    fn isolation_band_is_measured_in_metres_at_different_body_widths() {
        [0.1, 0.5, 2.0].into_iter().for_each(|span| {
            let (axis, origin) = lateral_frame(&sides(span));
            assert!((axis.length() - 1.0).abs() < 1e-12);
            let mask = side_mask(&[DVec3::X * 0.005], &axis, &origin, 0.010);
            assert!((mask[0] - 0.5).abs() < 1e-12);
        });
    }

    #[test]
    fn opposite_mass_reaches_zero_without_a_step_at_band_boundary() {
        let points = [0.009999, 0.010, 0.010001].map(|x| DVec3::X * x);
        let bones = sides(0.3);
        let (axis, origin) = lateral_frame(&bones);
        let mask = side_mask(&points, &axis, &origin, 0.010);
        let mut weights = vec![0.5; 6];
        apply_side_mask(&mut weights, &mask, &bones, 3);
        assert!(weights[3] < 2e-8, "boundary jump: {}", weights[3]);
        assert_eq!(&weights[4..], &[0.0, 0.0]);
        assert_eq!(&weights[..3], &[0.5; 3]);
    }

    #[test]
    fn opposite_branches_move_only_shared_mass_to_their_common_bone() {
        let mut bones = sides(0.3);
        bones.push(DeformBone {
            name: "Centre".to_owned(),
            side: None,
            segments: vec![
                (DVec3::Y, bones[0].segments[0].0),
                (DVec3::Y, bones[1].segments[0].0),
            ],
        });
        let mut weights = [0.5, 0.5, 0.25, 0.5, 0.25, 0.0];
        resolve_bilateral_overlap(&mut weights, &bones, 2);
        assert_eq!(weights, [0.25, 0.5, 0.0, 0.5, 0.75, 0.0]);
    }

    #[test]
    fn shell_crossing_a_nearest_source_bisector_has_a_continuous_field() {
        let points = [DVec3::X * -0.02, DVec3::X * 0.02];
        let grid = Grid::build(&points, &[0, 1]);
        let mass = |x| {
            grid.blend(&points, DVec3::new(x, 0.01, 0.0))
                .iter()
                .filter(|(source, _)| *source == 0)
                .map(|(_, weight)| weight)
                .sum::<f64>()
        };
        assert!((mass(0.0) - 0.5).abs() < 1e-12);
        assert!((mass(-1e-7) - mass(1e-7)).abs() < 0.001);
        assert_eq!(grid.blend(&points, points[0]), vec![(0, 1.0)]);
    }
}
