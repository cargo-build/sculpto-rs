//! Unsupported hanging strands in an authored garment layer.
//!
//! An authored garment can carry a cord as its own piece — a drawstring end, a
//! loose strap tail. It abuts the shell at one point, and nothing under it is
//! body, so it swings free and in motion it crosses the opposite limb. Deciding
//! that a piece is such a strand is a **shape** question, so the rule is stated
//! with measured quantities rather than with a fixture:
//!
//! 1. a piece carries no more than a configured share of the layer's area;
//! 2. its cross-section perpendicular to its own long axis is thin;
//! 3. it abuts the retained pieces only over a cluster confined to one end —
//!    a piece that abuts along its whole rim closes a seam and is never
//!    touched, which is what keeps the straps of a welded bra;
//! 4. most of it is farther from the body surface than the configured support
//!    distance, including the free end that hangs past the shell;
//! 5. a piece hanging off a piece already named a strand is the same strand —
//!    the authored tip caps of a drawstring are six-vertex pieces of their own.
//!
//! Removal is whole pieces and whole triangles: a triangle never loses a
//! vertex to this rule, so the retained shell's boundary edges are unchanged
//! and no hole can open where the strand met it. The report carries every
//! number the decision used. @req DECL0013

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::check::surface_distances;
use crate::point_query::PointIndex;

/// Authored contact, in mesh units, at which two pieces count as attached. The
/// authored underwear panels abut at exactly `0.0`; five hundredths of a unit
/// is far below the spacing of any authored seam.
pub const DEFAULT_ATTACH_EPSILON: f32 = 0.05;
/// Body-surface distance, in mesh units, above which a strand vertex is
/// unsupported. At the standard 71.4286 units per metre this is 2.8 cm, and the
/// authored underwear of both default bodies sits inside it everywhere except
/// under Mike's hanging drawstring.
pub const DEFAULT_SUPPORT_DISTANCE: f32 = 2.0;
/// Share of a strand's own vertices that must be unsupported before the piece
/// is called hanging; a strap that lifts at one corner keeps passing.
pub const DEFAULT_MIN_UNSUPPORTED_SHARE: f32 = 0.5;
/// Largest share of the layer's surface area one strand may carry. The measured
/// worst case (the boxers drawstring) is 3.4 %; the guard is the reason a
/// garment panel can never be a candidate however far it floats.
pub const DEFAULT_MAX_AREA_SHARE: f32 = 0.05;
/// Largest median cross-section width, in mesh units, of a strand. A drawstring
/// measures 1.60, the right boxers end 1.68; the thinnest measured garment
/// panel is Emily's panty waist strip at 3.99 and every other panel is wider.
pub const DEFAULT_MAX_THICKNESS: f32 = 3.0;
/// Largest share of its own extent over which a piece may touch the retained
/// pieces. The drawstring attaches over 3.1 % and 6.3 % of its length; a seam
/// copy abuts over 100 %.
pub const DEFAULT_MAX_ATTACHMENT_SPAN_SHARE: f32 = 0.30;
/// Cross-section bands measured along the strand's own axis.
pub const DEFAULT_SLICES: usize = 8;

/// Configured bounds of the strand rule, all in caller units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrandOptions {
    /// Two pieces closer than this along an authored seam are attached.
    pub attach_epsilon: f32,
    /// Body distance above which a strand vertex hangs unsupported.
    pub support_distance: f32,
    /// Share of the strand's vertices that must be unsupported.
    pub min_unsupported_share: f32,
    /// Largest share of the layer area one strand may carry.
    pub max_area_share: f32,
    /// Largest median cross-section width of a strand.
    pub max_thickness: f32,
    /// Largest share of its extent over which a strand may touch the rest.
    pub max_attachment_span_share: f32,
    /// Cross-section bands measured along the piece's axis.
    pub slices: usize,
}

impl Default for StrandOptions {
    fn default() -> Self {
        Self {
            attach_epsilon: DEFAULT_ATTACH_EPSILON,
            support_distance: DEFAULT_SUPPORT_DISTANCE,
            min_unsupported_share: DEFAULT_MIN_UNSUPPORTED_SHARE,
            max_area_share: DEFAULT_MAX_AREA_SHARE,
            max_thickness: DEFAULT_MAX_THICKNESS,
            max_attachment_span_share: DEFAULT_MAX_ATTACHMENT_SPAN_SHARE,
            slices: DEFAULT_SLICES,
        }
    }
}

impl StrandOptions {
    /// Reject settings that would silently change the rule's meaning.
    ///
    /// # Errors
    /// [`StrandError::InvalidOptions`] for a non-positive distance, a share
    /// outside `(0, 1]` or fewer than two cross-section bands.
    pub fn validate(&self) -> Result<(), StrandError> {
        let distances_ok = [
            self.attach_epsilon,
            self.support_distance,
            self.max_thickness,
        ]
        .iter()
        .all(|value| value.is_finite() && *value > 0.0);
        let shares_ok = [
            self.min_unsupported_share,
            self.max_area_share,
            self.max_attachment_span_share,
        ]
        .iter()
        .all(|share| share.is_finite() && *share > 0.0 && *share <= 1.0);
        (distances_ok && shares_ok && self.slices >= 2)
            .then_some(())
            .ok_or(StrandError::InvalidOptions)
    }
}

/// Invalid input or settings; no input is ever mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrandError {
    /// A length is not finite and positive, a share is outside `(0, 1]`, or
    /// fewer than two cross-section bands were declared.
    InvalidOptions,
    /// The garment has no vertices.
    EmptyGarment,
    /// The garment has no triangles.
    NoGarmentTriangles,
    /// The body carries no surface to measure support against.
    EmptyBody,
    /// A garment position is not finite.
    NonFinite { vertex: usize },
    /// A triangle refers outside the garment's positions.
    InvalidIndex { triangle: usize },
}

impl fmt::Display for StrandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOptions => f.write_str(
                "strand lengths must be finite and positive, shares within (0, 1], cuts >= 2",
            ),
            Self::EmptyGarment => f.write_str("the garment has no vertices"),
            Self::NoGarmentTriangles => f.write_str("the garment has no triangles"),
            Self::EmptyBody => f.write_str("the body has no surface to measure support against"),
            Self::NonFinite { vertex } => {
                write!(f, "nonfinite garment position at vertex {vertex}")
            }
            Self::InvalidIndex { triangle } => {
                write!(f, "triangle {triangle} refers outside the garment")
            }
        }
    }
}

impl std::error::Error for StrandError {}

/// Why a thin piece was kept, with the measurement that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrandRefusal {
    /// The piece or its chain carries more than the declared area share.
    AreaShare,
    /// The piece touches no other piece, so it does not hang *from* the layer.
    Unattached,
    /// The piece abuts the rest along its whole rim: it closes a seam.
    AttachedAlongTheRim,
    /// Too little of the piece is off the body surface.
    Supported,
    /// Enough of the piece is off the body, but its free end is not.
    FreeEndSupported,
}

impl StrandRefusal {
    /// Stable wire name for the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AreaShare => "area_share",
            Self::Unattached => "unattached",
            Self::AttachedAlongTheRim => "attached_along_the_rim",
            Self::Supported => "supported",
            Self::FreeEndSupported => "free_end_supported",
        }
    }
}

/// One removed strand: the pieces it spans and the numbers that named it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedStrand {
    /// Piece ordinals, in discovery order, all of them removed together.
    pub pieces: Vec<usize>,
    /// Triangles removed with this strand.
    pub triangles: usize,
    /// Vertices belonging to those triangles.
    pub vertices: usize,
    /// Surface area removed, in squared mesh units.
    pub area: f64,
    /// Removed area as a share of the layer's area.
    pub area_share: f32,
    /// Largest median cross-section width across the strand's pieces.
    pub thickness: f32,
    /// Longest axial extent across the strand's pieces.
    pub extent: f32,
    /// Share of the strand's vertices farther from the body than the support
    /// distance.
    pub unsupported_share: f32,
    /// Distance from the body surface to the hanging free end.
    pub tip_distance: f32,
    /// Largest share of one touching piece's own extent its contact with the
    /// retained pieces covers.
    pub attachment_span_share: f32,
}

/// One thin piece the rule examined and kept.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptStrand {
    /// Piece ordinals in this chain.
    pub pieces: Vec<usize>,
    /// Area share of the chain.
    pub area_share: f32,
    /// Median cross-section width, the widest across the chain.
    pub thickness: f32,
    /// Share of the chain's vertices farther from the body than the support
    /// distance.
    pub unsupported_share: f32,
    /// Which test refused it.
    pub refusal: StrandRefusal,
}

/// The cleaned layer and everything the rule measured.
#[derive(Debug, Clone, PartialEq)]
pub struct DropStrands {
    /// Retained positions, compacted so every one is referenced.
    pub positions: Vec<[f32; 3]>,
    /// Retained triangles, remapped onto `positions`.
    pub triangles: Vec<[u32; 3]>,
    /// Removed strands, discovery order.
    pub removed: Vec<RemovedStrand>,
    /// Thin pieces the rule examined and kept, with the reason.
    pub kept: Vec<KeptStrand>,
    /// Vertices of the input layer.
    pub source_vertices: usize,
    /// Vertices the input's triangles reference.
    pub source_referenced: usize,
    /// Triangles of the input layer.
    pub source_triangles: usize,
    /// Connected pieces of the input layer that carry triangles.
    pub source_pieces: usize,
    /// Connected pieces that remain.
    pub kept_pieces: usize,
    /// Surface area of the input layer.
    pub source_area: f64,
    /// Surface area that remains.
    pub kept_area: f64,
    /// Boundary edges of the input layer.
    pub boundary_edges_before: usize,
    /// Boundary edges that remain; never larger, because whole pieces are
    /// removed and no retained triangle loses a vertex.
    pub boundary_edges_after: usize,
}

impl DropStrands {
    /// Vertices the rule dropped with the removed pieces.
    pub fn removed_vertices(&self) -> usize {
        self.source_referenced - self.positions.len()
    }

    /// Triangles the rule dropped.
    pub fn removed_triangles(&self) -> usize {
        self.source_triangles - self.triangles.len()
    }

    /// Surface area the rule dropped.
    pub fn removed_area(&self) -> f64 {
        self.source_area - self.kept_area
    }
}

/// Remove the pieces of a garment layer that hang off the body unsupported.
///
/// A piece is removed only when every measured condition holds: it carries at
/// most [`StrandOptions::max_area_share`] of the layer's area, its median
/// cross-section is at most [`StrandOptions::max_thickness`], it touches the
/// retained pieces over at most
/// [`StrandOptions::max_attachment_span_share`] of its own extent, and at least
/// [`StrandOptions::min_unsupported_share`] of its vertices — the free end among
/// them — lie farther from the body surface than
/// [`StrandOptions::support_distance`]. A piece touching a removed piece is the
/// same strand and goes with it.
///
/// # Errors
/// Refuses invalid settings, an empty garment or body, a nonfinite position and
/// an out-of-range triangle index. The inputs are never mutated.
pub fn drop_unsupported_strands(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    body_positions: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
    options: &StrandOptions,
) -> Result<DropStrands, StrandError> {
    options.validate()?;
    if positions.is_empty() {
        return Err(StrandError::EmptyGarment);
    }
    if triangles.is_empty() {
        return Err(StrandError::NoGarmentTriangles);
    }
    if body_positions.is_empty() || body_triangles.is_empty() {
        return Err(StrandError::EmptyBody);
    }
    for (vertex, point) in positions.iter().enumerate() {
        if !point.iter().all(|value| value.is_finite()) {
            return Err(StrandError::NonFinite { vertex });
        }
    }
    for (index, triangle) in triangles.iter().enumerate() {
        if triangle
            .iter()
            .any(|vertex| *vertex as usize >= positions.len())
        {
            return Err(StrandError::InvalidIndex { triangle: index });
        }
    }

    let (labels, pieces) = piece_labels(positions.len(), triangles);
    let areas = triangle_areas(positions, triangles);
    let source_area = areas.iter().sum::<f64>();
    let distances = surface_distances(positions, body_positions, body_triangles);
    let mut referenced = vec![false; positions.len()];
    let mut triangle_pieces = Vec::with_capacity(triangles.len());
    for triangle in triangles {
        for vertex in triangle {
            referenced[*vertex as usize] = true;
        }
        triangle_pieces.push(labels[triangle[0] as usize] as usize);
    }
    let source_referenced = referenced.iter().filter(|used| **used).count();
    // A piece without triangles carries no surface and cannot be a strand.
    let used_pieces: Vec<usize> = triangle_pieces
        .iter()
        .copied()
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .collect();

    let shapes: Vec<PieceShape> = used_pieces
        .iter()
        .map(|piece| {
            PieceShape::measure(
                *piece,
                positions,
                &pieces[*piece],
                &areas,
                &triangle_pieces,
                options.slices,
            )
        })
        .collect();
    let shape_index: HashMap<usize, usize> = used_pieces
        .iter()
        .enumerate()
        .map(|(slot, piece)| (*piece, slot))
        .collect();
    let shape_of = |piece: usize| &shapes[shape_index[&piece]];

    // Authored contact: which other pieces each vertex sits on.
    let index = PointIndex::build(positions);
    let mut contact: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for (vertex, point) in positions.iter().enumerate() {
        let mut touched: Vec<usize> = index
            .within_radius(*point, options.attach_epsilon)
            .into_iter()
            .map(|(other, _)| labels[other] as usize)
            .filter(|piece| *piece != labels[vertex] as usize)
            .collect();
        touched.sort_unstable();
        touched.dedup();
        contact[vertex] = touched;
    }

    // Thin pieces in authored-contact chains; a chain is one candidate strand.
    let is_narrow = |piece: usize| shape_of(piece).is_narrow(options.max_thickness);
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for piece in &used_pieces {
        if !is_narrow(*piece) || group_of.contains_key(piece) {
            continue;
        }
        let group = groups.len();
        let mut members: Vec<usize> = Vec::new();
        let mut stack = vec![*piece];
        while let Some(current) = stack.pop() {
            if group_of.contains_key(&current) {
                continue;
            }
            group_of.insert(current, group);
            members.push(current);
            touching_pieces(&pieces[current], &labels, &contact)
                .into_iter()
                .filter(|other| is_narrow(*other) && !group_of.contains_key(other))
                .for_each(|other| stack.push(other));
        }
        members.sort_unstable();
        groups.push(members);
    }

    let mut removed_pieces: BTreeSet<usize> = BTreeSet::new();
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for members in &groups {
        let member_set: BTreeSet<usize> = members.iter().copied().collect();
        let area = members
            .iter()
            .map(|piece| shape_of(*piece).area)
            .sum::<f64>();
        let area_share = (area / source_area.max(f64::MIN_POSITIVE)) as f32;
        let thickness = members
            .iter()
            .map(|piece| shape_of(*piece).thickness)
            .fold(0.0_f32, f32::max);
        let extent = members
            .iter()
            .map(|piece| shape_of(*piece).extent)
            .fold(0.0_f32, f32::max);
        let vertices: Vec<u32> = members
            .iter()
            .flat_map(|piece| pieces[*piece].iter().copied())
            .collect();
        let unsupported_share = unsupported(&vertices, &distances, options.support_distance);
        match judge(
            positions,
            &vertices,
            members,
            &pieces,
            &shapes,
            &shape_index,
            &contact,
            &member_set,
            &distances,
            options,
            area_share,
            unsupported_share,
        ) {
            Ok(junction) => {
                let triangle_count = members
                    .iter()
                    .map(|piece| shape_of(*piece).triangles)
                    .sum::<usize>();
                removed_pieces.extend(members.iter().copied());
                removed.push(RemovedStrand {
                    pieces: members.clone(),
                    triangles: triangle_count,
                    vertices: vertices.len(),
                    area,
                    area_share,
                    thickness,
                    extent,
                    unsupported_share,
                    tip_distance: junction.tip_distance,
                    attachment_span_share: junction.attachment_span_share,
                });
            }
            Err(refusal) => kept.push(KeptStrand {
                pieces: members.clone(),
                area_share,
                thickness,
                unsupported_share,
                refusal,
            }),
        }
    }

    // Whole pieces, whole triangles: a retained triangle never loses a vertex.
    let mut removed_triangles = vec![false; triangles.len()];
    for (index, piece) in triangle_pieces.iter().enumerate() {
        removed_triangles[index] = removed_pieces.contains(piece);
    }
    let boundary_edges_before = boundary_edges(triangles.iter().copied());
    let kept_triangles: Vec<[u32; 3]> = triangles
        .iter()
        .zip(&removed_triangles)
        .filter_map(|(triangle, removed)| (!removed).then_some(*triangle))
        .collect();
    let boundary_edges_after = boundary_edges(kept_triangles.iter().copied());
    let kept_area = areas
        .iter()
        .zip(&removed_triangles)
        .filter_map(|(area, removed)| (!removed).then_some(*area))
        .sum::<f64>();

    let mut remap = vec![u32::MAX; positions.len()];
    let mut compacted = Vec::with_capacity(source_referenced);
    let mut compact_triangles = Vec::with_capacity(kept_triangles.len());
    for triangle in &kept_triangles {
        let mut mapped = [0_u32; 3];
        for (lane, vertex) in triangle.iter().enumerate() {
            if remap[*vertex as usize] == u32::MAX {
                remap[*vertex as usize] = u32::try_from(compacted.len())
                    .map_err(|_| StrandError::InvalidIndex { triangle: 0 })?;
                compacted.push(positions[*vertex as usize]);
            }
            mapped[lane] = remap[*vertex as usize];
        }
        compact_triangles.push(mapped);
    }

    Ok(DropStrands {
        positions: compacted,
        triangles: compact_triangles,
        removed,
        kept,
        source_vertices: positions.len(),
        source_referenced,
        source_triangles: triangles.len(),
        source_pieces: used_pieces.len(),
        kept_pieces: used_pieces.len() - removed_pieces.len(),
        source_area,
        kept_area,
        boundary_edges_before,
        boundary_edges_after,
    })
}

/// The junction a removable strand hangs by.
#[derive(Debug, Clone, Copy)]
struct Junction {
    tip_distance: f32,
    attachment_span_share: f32,
}

/// Decide one thin chain: a [`Junction`] when it is an unsupported strand.
#[allow(clippy::too_many_arguments)]
fn judge(
    positions: &[[f32; 3]],
    vertices: &[u32],
    members: &[usize],
    pieces: &[Vec<u32>],
    shapes: &[PieceShape],
    shape_index: &HashMap<usize, usize>,
    contact: &[Vec<usize>],
    member_set: &BTreeSet<usize>,
    distances: &[Option<f32>],
    options: &StrandOptions,
    area_share: f32,
    unsupported_share: f32,
) -> Result<Junction, StrandRefusal> {
    if area_share > options.max_area_share {
        return Err(StrandRefusal::AreaShare);
    }
    // The contact cluster with the retained pieces, per touching piece.
    let mut spanning = 0.0_f32;
    let mut touches = false;
    let mut junction_sum = [0.0_f64; 3];
    let mut junction_count = 0_usize;
    for piece in members {
        let shape = &shapes[shape_index[piece]];
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for (local, vertex) in pieces[*piece].iter().enumerate() {
            let retained = contact[*vertex as usize]
                .iter()
                .any(|other| !member_set.contains(other));
            if !retained {
                continue;
            }
            touches = true;
            let axial = shape.axial[local];
            low = low.min(axial);
            high = high.max(axial);
            let point = positions[*vertex as usize];
            for (lane, value) in point.iter().enumerate() {
                junction_sum[lane] += f64::from(*value);
            }
            junction_count += 1;
        }
        if low <= high {
            spanning = spanning.max((high - low) / shape.extent.max(f32::MIN_POSITIVE));
        }
    }
    if !touches {
        return Err(StrandRefusal::Unattached);
    }
    if spanning > options.max_attachment_span_share {
        return Err(StrandRefusal::AttachedAlongTheRim);
    }
    if unsupported_share < options.min_unsupported_share {
        return Err(StrandRefusal::Supported);
    }
    // The free end: the vertex farthest from the attachment cluster, measured
    // against the body surface. A strand whose whole body hangs has this too.
    let count = junction_count.max(1) as f64;
    let centroid = junction_sum.map(|value| value / count);
    let tip_distance = vertices
        .iter()
        .map(|vertex| {
            let point = positions[*vertex as usize];
            let reach = point
                .iter()
                .zip(centroid)
                .map(|(value, center)| (f64::from(*value) - center).powi(2))
                .sum::<f64>()
                .sqrt();
            (reach, distances[*vertex as usize].unwrap_or(f32::INFINITY))
        })
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, distance)| distance)
        .unwrap_or(0.0);
    if tip_distance <= options.support_distance {
        return Err(StrandRefusal::FreeEndSupported);
    }
    Ok(Junction {
        tip_distance,
        attachment_span_share: spanning,
    })
}

/// The pieces a set of vertices touches through authored contact.
fn touching_pieces(piece_vertices: &[u32], labels: &[u32], contact: &[Vec<usize>]) -> Vec<usize> {
    let own = labels[piece_vertices[0] as usize] as usize;
    let mut found: Vec<usize> = piece_vertices
        .iter()
        .flat_map(|vertex| contact[*vertex as usize].iter().copied())
        .filter(|piece| *piece != own)
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// Share of `vertices` farther from the body than the support distance. An
/// unreached vertex has no comparable body surface and counts as unsupported.
fn unsupported(vertices: &[u32], distances: &[Option<f32>], support_distance: f32) -> f32 {
    let beyond = vertices
        .iter()
        .filter(|vertex| {
            distances[**vertex as usize].is_none_or(|distance| distance > support_distance)
        })
        .count();
    beyond as f32 / vertices.len().max(1) as f32
}

/// Measured geometry of one connected piece.
#[derive(Debug, Clone, PartialEq)]
struct PieceShape {
    area: f64,
    triangles: usize,
    extent: f32,
    thickness: f32,
    /// Axial coordinate of each piece vertex, in the piece's own vertex order.
    axial: Vec<f32>,
}

impl PieceShape {
    fn measure(
        piece: usize,
        positions: &[[f32; 3]],
        vertices: &[u32],
        areas: &[f64],
        triangle_pieces: &[usize],
        slices: usize,
    ) -> Self {
        let points: Vec<[f32; 3]> = vertices
            .iter()
            .map(|vertex| positions[*vertex as usize])
            .collect();
        let axis = principal_axis(&points);
        let center = centroid(&points);
        let axial: Vec<f32> = points
            .iter()
            .map(|point| {
                point
                    .iter()
                    .zip(center)
                    .zip(axis)
                    .map(|((value, middle), lane)| (value - middle) * lane)
                    .sum()
            })
            .collect();
        let (low, high) = extent_of(&axial);
        let mut area = 0.0_f64;
        let mut triangles = 0_usize;
        for (index, owner) in triangle_pieces.iter().enumerate() {
            if *owner == piece {
                area += areas[index];
                triangles += 1;
            }
        }
        Self {
            area,
            triangles,
            extent: high - low,
            thickness: slice_thickness(&points, &axial, axis, slices),
            axial,
        }
    }

    fn is_narrow(&self, max_thickness: f32) -> bool {
        self.thickness <= max_thickness
    }
}

fn extent_of(axial: &[f32]) -> (f32, f32) {
    axial
        .iter()
        .fold((f32::MAX, f32::MIN), |(low, high), value| {
            (low.min(*value), high.max(*value))
        })
}

/// Median width of the piece's cross-section bands along its own axis.
fn slice_thickness(points: &[[f32; 3]], axial: &[f32], axis: [f32; 3], slices: usize) -> f32 {
    let reference = match axis[0].abs() < 0.9 {
        true => [1.0, 0.0, 0.0],
        false => [0.0, 1.0, 0.0],
    };
    let across = cross(axis, reference);
    let across = scale(
        across,
        1.0 / dot(across, across).sqrt().max(f32::MIN_POSITIVE),
    );
    let along = cross(axis, across);
    let (low, high) = extent_of(axial);
    let span = (high - low).max(f32::MIN_POSITIVE);
    let mut widths = Vec::new();
    for band in 0..slices {
        let start = low + span * band as f32 / slices as f32;
        let end = low + span * (band + 1) as f32 / slices as f32;
        let mut bounds = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut count = 0_usize;
        for (point, value) in points.iter().zip(axial) {
            if *value < start || (band + 1 != slices && *value > end) {
                continue;
            }
            let left = dot(*point, across);
            let right = dot(*point, along);
            bounds = (
                bounds.0.min(left),
                bounds.1.max(left),
                bounds.2.min(right),
                bounds.3.max(right),
            );
            count += 1;
        }
        if count >= 2 {
            widths.push(((bounds.1 - bounds.0).powi(2) + (bounds.3 - bounds.2).powi(2)).sqrt());
        }
    }
    if widths.is_empty() {
        return 0.0;
    }
    widths.sort_by(f32::total_cmp);
    widths[widths.len() / 2]
}

/// Direction of the largest spread of a point cloud, by Jacobi rotations on its
/// covariance. Deterministic: twelve sweeps from the identity, no random start.
fn principal_axis(points: &[[f32; 3]]) -> [f32; 3] {
    let center = centroid(points);
    let count = points.len().max(1) as f64;
    let mut matrix = [[0.0_f64; 3]; 3];
    for point in points {
        let delta = [
            f64::from(point[0] - center[0]),
            f64::from(point[1] - center[1]),
            f64::from(point[2] - center[2]),
        ];
        for row in 0..3 {
            for column in 0..3 {
                matrix[row][column] += delta[row] * delta[column];
            }
        }
    }
    for row in matrix.iter_mut() {
        for value in row.iter_mut() {
            *value /= count;
        }
    }
    let mut vectors = [[1.0_f64, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..12 {
        let mut pair = (0, 1);
        let mut largest = matrix[0][1].abs();
        for (row, column) in [(0, 2), (1, 2)] {
            if matrix[row][column].abs() > largest {
                largest = matrix[row][column].abs();
                pair = (row, column);
            }
        }
        let (p, q) = pair;
        if largest <= 1e-12 {
            break;
        }
        let theta = (matrix[q][q] - matrix[p][p]) / (2.0 * matrix[p][q]);
        let t = theta.signum() / (theta.abs() + (1.0 + theta * theta).sqrt());
        let c = 1.0 / (1.0 + t * t).sqrt();
        let s = t * c;
        let (app, aqq, apq) = (matrix[p][p], matrix[q][q], matrix[p][q]);
        for k in 0..3 {
            if k == p || k == q {
                continue;
            }
            let akp = matrix[k][p];
            let akq = matrix[k][q];
            matrix[k][p] = c * akp - s * akq;
            matrix[p][k] = matrix[k][p];
            matrix[k][q] = s * akp + c * akq;
            matrix[q][k] = matrix[k][q];
        }
        matrix[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        matrix[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        matrix[p][q] = 0.0;
        matrix[q][p] = 0.0;
        for row in vectors.iter_mut() {
            let vkp = row[p];
            let vkq = row[q];
            row[p] = c * vkp - s * vkq;
            row[q] = s * vkp + c * vkq;
        }
    }
    let mut best = 0;
    for index in 1..3 {
        if matrix[index][index] > matrix[best][best] {
            best = index;
        }
    }
    let axis = [
        vectors[0][best] as f32,
        vectors[1][best] as f32,
        vectors[2][best] as f32,
    ];
    let length = dot(axis, axis).sqrt();
    match length > 0.0 {
        true => scale(axis, 1.0 / length),
        false => [0.0, 0.0, 1.0],
    }
}

fn centroid(points: &[[f32; 3]]) -> [f32; 3] {
    let count = points.len().max(1) as f32;
    let sum = points.iter().fold([0.0_f32; 3], |sum, point| {
        [sum[0] + point[0], sum[1] + point[1], sum[2] + point[2]]
    });
    scale(sum, 1.0 / count)
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn scale(value: [f32; 3], factor: f32) -> [f32; 3] {
    [value[0] * factor, value[1] * factor, value[2] * factor]
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

/// Surface area of every triangle, in squared caller units.
fn triangle_areas(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<f64> {
    triangles
        .iter()
        .map(|[a, b, c]| {
            let [a, b, c] = [*a as usize, *b as usize, *c as usize];
            let normal = cross(
                subtract(positions[b], positions[a]),
                subtract(positions[c], positions[a]),
            );
            0.5 * f64::from(dot(normal, normal)).sqrt()
        })
        .collect()
}

/// Connected pieces by shared render vertex, and the vertex list of each.
fn piece_labels(vertex_count: usize, triangles: &[[u32; 3]]) -> (Vec<u32>, Vec<Vec<u32>>) {
    fn find(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let mut parent: Vec<usize> = (0..vertex_count).collect();
    for triangle in triangles {
        for (left, right) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[0], triangle[2]),
        ] {
            let (left, right) = (
                find(&mut parent, left as usize),
                find(&mut parent, right as usize),
            );
            if left != right {
                parent[right] = left;
            }
        }
    }
    let mut root_label: Vec<Option<u32>> = vec![None; vertex_count];
    let mut pieces: Vec<Vec<u32>> = Vec::new();
    let labels: Vec<u32> = (0..vertex_count)
        .map(|vertex| {
            let root = find(&mut parent, vertex);
            let piece = match root_label[root] {
                Some(piece) => piece,
                None => {
                    let piece = u32::try_from(pieces.len()).unwrap_or(u32::MAX);
                    root_label[root] = Some(piece);
                    pieces.push(Vec::new());
                    piece
                }
            };
            pieces[piece as usize].push(vertex as u32);
            piece
        })
        .collect();
    (labels, pieces)
}

/// Edges used by exactly one triangle.
fn boundary_edges<I>(triangles: I) -> usize
where
    I: IntoIterator<Item = [u32; 3]>,
{
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for triangle in triangles {
        for (left, right) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[0], triangle[2]),
        ] {
            let key = match left < right {
                true => (left, right),
                false => (right, left),
            };
            *edges.entry(key).or_default() += 1;
        }
    }
    edges.values().filter(|count| **count == 1).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square plate of `cells` × `cells` quads in the `z = 0` plane, centred.
    fn plate(cells: usize, half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let step = 2.0 * half / cells as f32;
        let mut positions = Vec::new();
        for row in 0..=cells {
            for column in 0..=cells {
                positions.push([-half + column as f32 * step, -half + row as f32 * step, 0.0]);
            }
        }
        let stride = (cells + 1) as u32;
        let mut triangles = Vec::new();
        for row in 0..cells as u32 {
            for column in 0..cells as u32 {
                let corner = row * stride + column;
                triangles.push([corner, corner + 1, corner + stride + 1]);
                triangles.push([corner, corner + stride + 1, corner + stride]);
            }
        }
        (positions, triangles)
    }

    /// A four-sided cord hanging from `z = 0` down to `z = -length`, centred on
    /// (`x`, `y`) with square cross-section `width`.
    fn cord(
        x: f32,
        y: f32,
        width: f32,
        length: f32,
        rings: usize,
    ) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let half = width / 2.0;
        let mut positions = Vec::new();
        for ring in 0..=rings {
            let z = -(length * ring as f32 / rings as f32);
            positions.push([x - half, y - half, z]);
            positions.push([x + half, y - half, z]);
            positions.push([x + half, y + half, z]);
            positions.push([x - half, y + half, z]);
        }
        let mut triangles = Vec::new();
        for ring in 0..rings as u32 {
            let base = ring * 4;
            let next = base + 4;
            for side in 0..4_u32 {
                let a = base + side;
                let b = base + (side + 1) % 4;
                triangles.push([a, b, next + (side + 1) % 4]);
                triangles.push([a, next + (side + 1) % 4, next + side]);
            }
        }
        (positions, triangles)
    }

    /// Append one mesh to another, shifting the second's indices.
    fn join(
        left: (Vec<[f32; 3]>, Vec<[u32; 3]>),
        right: (Vec<[f32; 3]>, Vec<[u32; 3]>),
    ) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let (mut positions, mut triangles) = left;
        let base = positions.len() as u32;
        positions.extend(right.0);
        triangles.extend(
            right
                .1
                .iter()
                .map(|[a, b, c]| [a + base, b + base, c + base]),
        );
        (positions, triangles)
    }

    #[test]
    fn a_hanging_cord_is_removed_and_opens_no_boundary() {
        let shell = plate(16, 4.0);
        let body = shell.clone();
        let garment = join(shell, cord(4.0, 4.0, 0.1, 6.0, 6));
        let before = boundary_edges(garment.1.iter().copied());
        let options = StrandOptions {
            attach_epsilon: 0.3,
            ..StrandOptions::default()
        };
        let dropped = drop_unsupported_strands(&garment.0, &garment.1, &body.0, &body.1, &options)
            .expect("ok");
        assert_eq!(dropped.source_pieces, 2);
        assert_eq!(dropped.removed.len(), 1, "{:?}", dropped.kept);
        let strand = &dropped.removed[0];
        assert!(strand.area_share < 0.05, "{strand:?}");
        assert!(strand.thickness < 0.5, "{strand:?}");
        assert!(strand.tip_distance > 5.0, "{strand:?}");
        assert!(strand.attachment_span_share < 0.1, "{strand:?}");
        assert_eq!(dropped.removed_triangles(), 48);
        // The cord's own two open rims go with it; the plate's boundary is
        // untouched, so no hole can open where the strand met the shell.
        assert!(dropped.boundary_edges_after < before);
        assert_eq!(dropped.boundary_edges_before, before);
        assert_eq!(
            dropped.boundary_edges_after,
            boundary_edges(plate(16, 4.0).1)
        );
    }

    #[test]
    fn a_short_cord_resting_on_the_body_is_kept() {
        let shell = plate(16, 4.0);
        let body = shell.clone();
        let garment = join(shell, cord(4.0, 4.0, 0.2, 1.0, 2));
        let options = StrandOptions {
            attach_epsilon: 0.3,
            ..StrandOptions::default()
        };
        let dropped = drop_unsupported_strands(&garment.0, &garment.1, &body.0, &body.1, &options)
            .expect("ok");
        assert!(dropped.removed.is_empty(), "{:?}", dropped.removed);
        assert_eq!(dropped.kept.len(), 1, "{:?}", dropped.kept);
        assert_eq!(dropped.kept[0].refusal, StrandRefusal::Supported);
        assert_eq!(dropped.triangles.len(), garment.1.len());
    }

    #[test]
    fn a_piece_abutting_along_its_whole_rim_is_a_seam_and_is_kept() {
        // The body stops short of the shell's edge, so a coplanar strip beyond
        // that edge hangs in the air while still abutting the shell along its
        // whole length — the seam-copy shape, never a strand.
        let (shell_positions, shell_triangles) = plate(16, 4.0);
        let body = plate(12, 3.0);
        let mut strip_positions = Vec::new();
        let mut strip_triangles = Vec::new();
        for row in 0..=8_u32 {
            let y = -4.0 + row as f32;
            strip_positions.push([4.0, y, 0.0]);
            strip_positions.push([4.2, y, 0.0]);
        }
        for row in 0..8_u32 {
            let base = row * 2;
            strip_triangles.push([base, base + 1, base + 3]);
            strip_triangles.push([base, base + 3, base + 2]);
        }
        let garment = join(
            (shell_positions, shell_triangles),
            (strip_positions, strip_triangles),
        );
        let options = StrandOptions {
            attach_epsilon: 0.3,
            support_distance: 0.9,
            ..StrandOptions::default()
        };
        let dropped = drop_unsupported_strands(&garment.0, &garment.1, &body.0, &body.1, &options)
            .expect("ok");
        assert!(dropped.removed.is_empty(), "{:?}", dropped.removed);
        assert_eq!(
            dropped.kept[0].refusal,
            StrandRefusal::AttachedAlongTheRim,
            "{:?}",
            dropped.kept
        );
    }

    #[test]
    fn a_cord_carrying_too_much_area_is_kept() {
        let shell = plate(8, 2.0);
        let body = shell.clone();
        let garment = join(shell, cord(2.0, 2.0, 1.0, 6.0, 6));
        let options = StrandOptions {
            attach_epsilon: 0.8,
            ..StrandOptions::default()
        };
        let dropped = drop_unsupported_strands(&garment.0, &garment.1, &body.0, &body.1, &options)
            .expect("ok");
        assert!(dropped.removed.is_empty(), "{:?}", dropped.removed);
        assert_eq!(
            dropped.kept[0].refusal,
            StrandRefusal::AreaShare,
            "{:?}",
            dropped.kept
        );
    }

    #[test]
    fn a_tip_cap_goes_with_the_cord_it_caps() {
        let shell = plate(16, 4.0);
        let body = shell.clone();
        let garment = join(shell, join(cord(4.0, 4.0, 0.1, 6.0, 6), plate(1, 0.06)));
        // Move the cap to the cord's tip, six units below the shell.
        let (mut positions, triangles) = garment;
        let cap = positions.len() - 4;
        positions[cap..].iter_mut().for_each(|point| {
            point[0] += 4.0;
            point[1] += 4.0;
            point[2] -= 6.0;
        });
        let options = StrandOptions {
            attach_epsilon: 0.3,
            ..StrandOptions::default()
        };
        let dropped = drop_unsupported_strands(&positions, &triangles, &body.0, &body.1, &options)
            .expect("ok");
        assert_eq!(dropped.source_pieces, 3);
        assert_eq!(dropped.removed.len(), 1, "{:?}", dropped.kept);
        assert_eq!(dropped.removed[0].pieces.len(), 2, "{:?}", dropped.removed);
        assert_eq!(dropped.removed_triangles(), 48 + 2);
    }

    #[test]
    fn a_detached_piece_is_not_a_strand() {
        let shell = plate(16, 4.0);
        let body = shell.clone();
        let garment = join(shell, cord(-9.0, 0.0, 0.1, 6.0, 6));
        let dropped = drop_unsupported_strands(
            &garment.0,
            &garment.1,
            &body.0,
            &body.1,
            &StrandOptions::default(),
        )
        .expect("ok");
        assert!(dropped.removed.is_empty(), "{:?}", dropped.removed);
        assert_eq!(dropped.kept[0].refusal, StrandRefusal::Unattached);
    }

    #[test]
    fn inputs_and_settings_are_refused_before_any_work() {
        let shell = plate(4, 1.0);
        let options = StrandOptions::default();
        assert_eq!(
            drop_unsupported_strands(&[], &shell.1, &shell.0, &shell.1, &options),
            Err(StrandError::EmptyGarment)
        );
        assert_eq!(
            drop_unsupported_strands(&shell.0, &[], &shell.0, &shell.1, &options),
            Err(StrandError::NoGarmentTriangles)
        );
        assert_eq!(
            drop_unsupported_strands(&shell.0, &shell.1, &[], &[], &options),
            Err(StrandError::EmptyBody)
        );
        assert_eq!(
            drop_unsupported_strands(
                &[[f32::NAN, 0.0, 0.0]],
                &[[0, 0, 0]],
                &shell.0,
                &shell.1,
                &options
            ),
            Err(StrandError::NonFinite { vertex: 0 })
        );
        assert_eq!(
            drop_unsupported_strands(&shell.0, &[[0, 1, 900]], &shell.0, &shell.1, &options),
            Err(StrandError::InvalidIndex { triangle: 0 })
        );
        let bad = StrandOptions {
            max_area_share: 0.0,
            ..StrandOptions::default()
        };
        assert_eq!(
            drop_unsupported_strands(&shell.0, &shell.1, &shell.0, &shell.1, &bad),
            Err(StrandError::InvalidOptions)
        );
    }

    #[test]
    fn the_principal_axis_follows_the_longest_spread() {
        let points = [
            [0.0, 0.0, 0.0],
            [0.0, 9.0, 0.0],
            [0.1, 4.0, 0.05],
            [0.0, 5.0, 0.0],
        ];
        let axis = principal_axis(&points);
        assert!(axis[1].abs() > 0.99, "{axis:?}");
    }

    #[test]
    fn a_cord_slice_measures_its_own_width() {
        let (points, _) = cord(0.0, 0.0, 0.4, 6.0, 12);
        let axis = principal_axis(&points);
        let axial: Vec<f32> = points.iter().map(|point| dot(*point, axis)).collect();
        let thickness = slice_thickness(&points, &axial, axis, 8);
        let expected = 0.4 * 2.0_f32.sqrt();
        assert!((thickness - expected).abs() < 0.05, "{thickness}");
    }
}
