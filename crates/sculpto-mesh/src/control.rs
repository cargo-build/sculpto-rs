//! Source-guided exterior control construction from a thick source shell.
//! @req DECL0013
//!
//! A wardrobe cell exports a closed garment sculpt: an outward cloth surface, an
//! inner lining and sealed rims between them. A simulation control keeps only the
//! outward surface plus the real collar, cuff and hem openings. Every step here
//! is pure — identify exact-position topology, classify faces against an explicit
//! source-space guide, keep the largest shell, prune non-manifold corner flares,
//! name the apertures the profile expects, and plan reduction and repair. Blender
//! or any other host performs the native operators on the returned decisions.
//!
//! The rule is model-independent: no asset name, path or vertex number appears
//! here, and every threshold is either a module constant or profile data.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::topology::position_topology;

pub mod contact;
pub mod measure;
pub mod profile;
pub mod recovery;

pub use recovery::{RecoveredDisk, RecoveryRefusal, RecoveryReport};

pub use contact::{
    ContactError, ContactMode, ContactPolicy, Separation, SeparationOutcome, VertexMove,
};
pub use measure::{ControlDiagnostics, CoverageLimits, Deviation, Deviations, Qualification};
pub use profile::{ControlFrame, ControlProfile};

/// A face centroid closer than this to the guide has no outward direction.
pub const AMBIGUOUS_RADIAL_M: f32 = 1e-6;

/// Every way source-guided construction refuses to continue.
///
/// A variant carries the measured values, so a caller can report the concrete
/// defect instead of retrying with different parameters. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub enum ControlError {
    /// The source mesh has no vertices.
    EmptyMesh,
    /// The source mesh has no triangles.
    NoTriangles,
    /// A triangle references a vertex the mesh does not have.
    BadIndex {
        triangle: usize,
        vertex: u32,
        vertices: usize,
    },
    /// A vertex position is infinite or NaN.
    NonFiniteVertex { vertex: usize },
    /// A source triangle has no area.
    CollapsedTriangle { triangle: usize },
    /// The guide carries no segment.
    EmptyGuide,
    /// A guide segment endpoint is infinite or NaN.
    NonFiniteSegment { segment: usize },
    /// A guide segment has zero (or sub-epsilon) length.
    DegenerateSegment { segment: usize },
    /// Face centroids sit on the guide itself, so the outward side is undefined.
    AmbiguousOrientation { triangles: usize, first: u32 },
    /// No triangle faces away from the guide; the guide or the frame is wrong.
    NothingExterior,
    /// The profile declares no expected aperture.
    EmptyProfile,
    /// Two aperture entries share one role name.
    DuplicateRole { role: String },
    /// An aperture anchor is infinite or NaN.
    NonFiniteAnchor { role: String },
    /// An aperture radius is not a positive finite length.
    NonPositiveRadius { role: String },
    /// No boundary loop lies within an aperture's declared radius.
    UnknownAperture { role: String, radius: f32 },
    /// More than one boundary loop lies within an aperture's declared radius.
    AmbiguousAperture { role: String, candidates: usize },
    /// A boundary loop does not close on itself, so it is not an opening.
    UnclosedBoundary { loop_index: usize },
    /// More repair loops than the profile allows.
    IncidentalLimit { loops: usize, limit: usize },
    /// A finished control carries loops the profile never declared as apertures.
    UnexpectedLoops { loops: usize, apertures: usize },
    /// A repair loop is larger than the profile allows filling.
    FillLimit {
        loop_index: usize,
        edges: usize,
        limit: usize,
    },
    /// Corner-flare pruning would remove more than the profile allows.
    TipRemovalLimit {
        removed: usize,
        triangles: usize,
        limit: f32,
    },
    /// Reduction limits are not ordered `minimum <= target <= maximum`.
    InvalidReductionLimits {
        minimum: usize,
        target: usize,
        maximum: usize,
    },
    /// The reduced surface fell below the declared triangle minimum.
    ReductionBelowMinimum { triangles: usize, minimum: usize },
    /// Reduction did not reach the declared triangle maximum.
    ReductionAboveMaximum { triangles: usize, maximum: usize },
    /// The control surface contacts itself at rest.
    SelfContacts { pairs: usize },
    /// Vertices whose face fan is not a single disk.
    NotManifold { vertices: usize },
    /// Edges used by no face or by more than two.
    NonManifoldEdges { edges: usize },
    /// Two faces traverse one edge in the same direction: inconsistent winding.
    InconsistentWinding { edges: usize },
    /// The control surface is not one connected shell.
    MultipleComponents { components: usize },
    /// Coincident render vertices survived; the layers are still ambiguous.
    CoincidentVertices { vertices: usize },
    /// Euler characteristic does not match the profile's declared topology.
    UnexpectedTopology {
        euler: i64,
        loops: usize,
        genus: i64,
        expected_genus: u32,
    },
    /// Coverage tolerance or minimum share is out of range.
    InvalidCoverage { tolerance: f32, minimum: f32 },
    /// The profile file is missing a field, mistyped or carries an unknown one.
    InvalidProfile { field: String, reason: String },
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMesh => write!(formatter, "the source mesh has no vertices"),
            Self::NoTriangles => write!(formatter, "the source mesh has no triangles"),
            Self::BadIndex {
                triangle,
                vertex,
                vertices,
            } => write!(
                formatter,
                "source triangle {triangle} references vertex {vertex}, but only {vertices} vertices exist"
            ),
            Self::NonFiniteVertex { vertex } => {
                write!(formatter, "source vertex {vertex} is not finite")
            }
            Self::CollapsedTriangle { triangle } => {
                write!(formatter, "source triangle {triangle} has no area")
            }
            Self::EmptyGuide => write!(formatter, "the guide declares no segment"),
            Self::NonFiniteSegment { segment } => {
                write!(formatter, "guide segment {segment} is not finite")
            }
            Self::DegenerateSegment { segment } => {
                write!(formatter, "guide segment {segment} has zero length")
            }
            Self::AmbiguousOrientation { triangles, first } => write!(
                formatter,
                "{triangles} face centroid(s) sit on the guide, first triangle {}, so no outward side exists",
                first + 1
            ),
            Self::NothingExterior => write!(
                formatter,
                "no triangle faces away from the guide: the guide or the declared frame is wrong"
            ),
            Self::EmptyProfile => write!(
                formatter,
                "the profile declares no expected aperture, so a closure defect could not be detected"
            ),
            Self::DuplicateRole { role } => {
                write!(formatter, "aperture role `{role}` is declared twice")
            }
            Self::NonFiniteAnchor { role } => {
                write!(formatter, "aperture `{role}` anchor is not finite")
            }
            Self::NonPositiveRadius { role } => write!(
                formatter,
                "aperture `{role}` radius is not a positive finite length"
            ),
            Self::UnknownAperture { role, radius } => write!(
                formatter,
                "no boundary loop lies within {radius} of the `{role}` anchor: that opening was lost or the anchor is wrong"
            ),
            Self::AmbiguousAperture { role, candidates } => write!(
                formatter,
                "{candidates} boundary loops lie within the `{role}` radius: an ambiguous opening is never filled"
            ),
            Self::UnclosedBoundary { loop_index } => write!(
                formatter,
                "boundary loop {loop_index} does not close on itself; it is not an opening"
            ),
            Self::IncidentalLimit { loops, limit } => write!(
                formatter,
                "{loops} non-aperture boundary loops exceed the declared limit of {limit}"
            ),
            Self::UnexpectedLoops { loops, apertures } => write!(
                formatter,
                "the control has {loops} boundary loops, but the profile declares {apertures} apertures and no incidental closure remains"
            ),
            Self::FillLimit {
                loop_index,
                edges,
                limit,
            } => write!(
                formatter,
                "repair loop {loop_index} has {edges} edges, above the declared fill limit of {limit}"
            ),
            Self::TipRemovalLimit {
                removed,
                triangles,
                limit,
            } => write!(
                formatter,
                "corner-flare pruning would remove {removed} of {triangles} triangles, above the declared {:.1}% limit",
                f64::from(*limit) * 100.0
            ),
            Self::InvalidReductionLimits {
                minimum,
                target,
                maximum,
            } => write!(
                formatter,
                "reduction limits must satisfy minimum <= target <= maximum, got {minimum} <= {target} <= {maximum}"
            ),
            Self::ReductionBelowMinimum { triangles, minimum } => write!(
                formatter,
                "the reduced surface has {triangles} triangles, below the declared minimum of {minimum}"
            ),
            Self::ReductionAboveMaximum { triangles, maximum } => write!(
                formatter,
                "the reduced surface has {triangles} triangles, above the declared maximum of {maximum}"
            ),
            Self::SelfContacts { pairs } => {
                write!(
                    formatter,
                    "{pairs} nonadjacent triangle pairs intersect at rest"
                )
            }
            Self::NotManifold { vertices } => write!(
                formatter,
                "{vertices} vertices do not have a single disk face fan"
            ),
            Self::NonManifoldEdges { edges } => write!(
                formatter,
                "{edges} edges are used by no face or by more than two"
            ),
            Self::InconsistentWinding { edges } => write!(
                formatter,
                "{edges} edges are traversed in the same direction by both incident faces: the surface winding is inconsistent"
            ),
            Self::MultipleComponents { components } => write!(
                formatter,
                "the control surface is {components} disconnected shells, not one garment"
            ),
            Self::CoincidentVertices { vertices } => write!(
                formatter,
                "{vertices} coincident render vertices remain; the layers are still ambiguous"
            ),
            Self::UnexpectedTopology {
                euler,
                loops,
                genus,
                expected_genus,
            } => write!(
                formatter,
                "Euler characteristic {euler} with {loops} openings is genus {genus}, but the profile declares genus {expected_genus}"
            ),
            Self::InvalidCoverage { tolerance, minimum } => write!(
                formatter,
                "coverage needs a positive tolerance and a share in 0..=1, got {tolerance} and {minimum}"
            ),
            Self::InvalidProfile { field, reason } => {
                write!(formatter, "profile field `{field}`: {reason}")
            }
        }
    }
}

impl std::error::Error for ControlError {}

/// A finite, nonzero guide segment in the same frame as the mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideSegment {
    pub start: [f32; 3],
    pub end: [f32; 3],
}

impl GuideSegment {
    fn validate(&self, segment: usize) -> Result<(), ControlError> {
        if !self
            .start
            .iter()
            .chain(self.end.iter())
            .all(|value| value.is_finite())
        {
            return Err(ControlError::NonFiniteSegment { segment });
        }
        if length(subtract(self.end, self.start)) <= f32::EPSILON {
            return Err(ControlError::DegenerateSegment { segment });
        }
        Ok(())
    }

    fn closest(&self, point: [f32; 3], segment: usize) -> NearestOnGuide {
        let axis = subtract(self.end, self.start).map(f64::from);
        let offset = subtract(point, self.start).map(f64::from);
        let squared = dot(axis, axis);
        let along = if squared <= f64::EPSILON {
            0.0
        } else {
            dot(offset, axis) / squared
        };
        let along = along.clamp(0.0, 1.0);
        let point_on_segment = [
            f64::from(self.start[0]) + along * axis[0],
            f64::from(self.start[1]) + along * axis[1],
            f64::from(self.start[2]) + along * axis[2],
        ];
        let point_on_segment = point_on_segment.map(|value| value as f32);
        NearestOnGuide {
            point: point_on_segment,
            distance: length(subtract(point, point_on_segment)),
            segment,
        }
    }
}

/// The nearest guide point to a query point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearestOnGuide {
    pub point: [f32; 3],
    pub distance: f32,
    pub segment: usize,
}

/// An ordered guide polyline: the explicit source-space skeleton of one garment.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    segments: Vec<GuideSegment>,
}

impl Guide {
    /// Validate and keep the guide; an empty, nonfinite or zero-length segment
    /// cannot define an outward side. @req DECL0013
    pub fn new(segments: impl IntoIterator<Item = GuideSegment>) -> Result<Self, ControlError> {
        let segments = segments.into_iter().collect::<Vec<_>>();
        if segments.is_empty() {
            return Err(ControlError::EmptyGuide);
        }
        for (index, segment) in segments.iter().enumerate() {
            segment.validate(index)?;
        }
        Ok(Self { segments })
    }

    pub fn segments(&self) -> &[GuideSegment] {
        &self.segments
    }

    /// Nearest guide point; the first segment wins a tie, so the result is stable.
    pub fn nearest(&self, point: [f32; 3]) -> NearestOnGuide {
        self.segments
            .iter()
            .enumerate()
            .map(|(index, segment)| segment.closest(point, index))
            .min_by(|left, right| left.distance.total_cmp(&right.distance))
            // `new` rejects an empty guide, so at least one segment was evaluated.
            .expect("a validated guide has at least one segment")
    }
}

/// Exact-position topology of the source shell with source vertex lineage.
///
/// Render vertices split for UV seams or normals collapse only for geometry
/// decisions; the source line of every kept triangle stays recoverable through
/// [`Self::vertex_sources`]. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSurface {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    vertex_sources: Vec<u32>,
    source_vertices: usize,
    source_triangles: usize,
}

impl SourceSurface {
    /// Reject invalid input before any decision and identify exact positions.
    pub fn identify(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Self, ControlError> {
        if positions.is_empty() {
            return Err(ControlError::EmptyMesh);
        }
        if triangles.is_empty() {
            return Err(ControlError::NoTriangles);
        }
        if let Some(vertex) = positions
            .iter()
            .position(|point| !point.iter().all(|value| value.is_finite()))
        {
            return Err(ControlError::NonFiniteVertex { vertex });
        }
        for (index, [a, b, c]) in triangles.iter().enumerate() {
            if let Some(vertex) = [*a, *b, *c]
                .into_iter()
                .find(|vertex| *vertex as usize >= positions.len())
            {
                return Err(ControlError::BadIndex {
                    triangle: index,
                    vertex,
                    vertices: positions.len(),
                });
            }
            if doubled_area(
                positions[*a as usize],
                positions[*b as usize],
                positions[*c as usize],
            ) <= f64::MIN_POSITIVE
            {
                return Err(ControlError::CollapsedTriangle { triangle: index });
            }
        }
        let identical = position_topology(positions, triangles);
        Ok(Self {
            positions: identical.positions,
            triangles: identical.triangles,
            vertex_sources: identical.vertex_sources,
            source_vertices: positions.len(),
            source_triangles: triangles.len(),
        })
    }

    pub fn positions(&self) -> &[[f32; 3]] {
        &self.positions
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// Source vertex index behind each exact-position vertex.
    pub fn vertex_sources(&self) -> &[u32] {
        &self.vertex_sources
    }

    pub fn source_vertices(&self) -> usize {
        self.source_vertices
    }

    pub fn source_triangles(&self) -> usize {
        self.source_triangles
    }

    /// Render vertices that identified with another one.
    pub fn coincident_vertices(&self) -> usize {
        self.source_vertices - self.positions.len()
    }

    /// Triangles that became degenerate once coincident corners identified.
    /// They carry no surface, so every stage drops them and reports the count.
    fn welded_degenerate(&self) -> Vec<u32> {
        self.triangles
            .iter()
            .enumerate()
            .filter(|(_, [a, b, c])| a == b || b == c || c == a)
            .map(|(index, _)| index as u32)
            .collect()
    }
}

/// Which source triangles form the outward surface of the thick shell.
#[derive(Debug, Clone, PartialEq)]
pub struct ExteriorSelection {
    /// Topology triangle indices whose normal points away from the guide.
    pub kept: Vec<u32>,
    /// Topology triangle indices that face inward or along the guide.
    pub removed: usize,
}

/// Keep faces whose normal agrees with the direction away from the guide.
///
/// A centroid on the guide has no outward side: that is reported, never guessed.
/// A face exactly perpendicular to the radial direction is dropped, matching the
/// qualified construction this ports. @req DECL0013
pub fn exterior_triangles(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    guide: &Guide,
) -> Result<ExteriorSelection, ControlError> {
    let mut kept = Vec::new();
    let mut ambiguous = Vec::new();
    for (index, [a, b, c]) in triangles.iter().enumerate() {
        let corners = [
            positions[*a as usize],
            positions[*b as usize],
            positions[*c as usize],
        ];
        let centroid = [
            (corners[0][0] + corners[1][0] + corners[2][0]) / 3.0,
            (corners[0][1] + corners[1][1] + corners[2][1]) / 3.0,
            (corners[0][2] + corners[1][2] + corners[2][2]) / 3.0,
        ];
        let radial = subtract(centroid, guide.nearest(centroid).point);
        if length(radial) <= AMBIGUOUS_RADIAL_M {
            ambiguous.push(index as u32);
            continue;
        }
        let normal = cross(corners[1], corners[0], corners[2]);
        if dot(normal, radial.map(f64::from)) > 0.0 {
            kept.push(index as u32);
        }
    }
    if let Some(first) = ambiguous.first().copied() {
        return Err(ControlError::AmbiguousOrientation {
            triangles: ambiguous.len(),
            first,
        });
    }
    if kept.is_empty() {
        return Err(ControlError::NothingExterior);
    }
    let removed = triangles.len() - kept.len();
    Ok(ExteriorSelection { kept, removed })
}

/// The shell kept by shared-edge adjacency.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentSelection {
    /// Triangle indices (into the input list) of the largest shell, ascending.
    pub kept: Vec<u32>,
    pub removed_components: usize,
    pub removed_triangles: usize,
    pub largest_triangles: usize,
}

/// Keep the largest connected shell; every island is reported, not silently kept.
pub fn largest_component(triangles: &[[u32; 3]]) -> ComponentSelection {
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    let mut edges: HashMap<(u32, u32), usize> = HashMap::new();
    for (index, [a, b, c]) in triangles.iter().enumerate() {
        for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
            let key = (left.min(right), left.max(right));
            match edges.get(&key).copied() {
                Some(other) => union(&mut parent, index, other),
                None => {
                    edges.insert(key, index);
                }
            }
        }
    }
    let mut components: HashMap<usize, Vec<u32>> = HashMap::new();
    for index in 0..triangles.len() {
        let root = find(&mut parent, index);
        components.entry(root).or_default().push(index as u32);
    }
    let mut ranked = components.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .len()
            .cmp(&left.len())
            .then_with(|| left.first().cmp(&right.first()))
    });
    let Some(largest) = ranked.first().cloned() else {
        return ComponentSelection {
            kept: Vec::new(),
            removed_components: 0,
            removed_triangles: 0,
            largest_triangles: 0,
        };
    };
    let removed_triangles = triangles.len() - largest.len();
    ComponentSelection {
        kept: largest.clone(),
        removed_components: ranked.len() - 1,
        removed_triangles,
        largest_triangles: largest.len(),
    }
}

/// Corner flares left at a non-manifold vertex: extra face fans to remove.
#[derive(Debug, Clone, PartialEq)]
pub struct TipPrune {
    /// Triangle indices to remove, ascending.
    pub removed: Vec<u32>,
    /// Vertices whose fan was not a single disk.
    pub vertices: Vec<u32>,
}

/// Number of face fans around one vertex; more than one is a pinched fan.
///
/// Qualification and outside diagnostics share this rule. @req DECL0013
pub fn vertex_fan_count(triangles: &[[u32; 3]], vertex: u32, faces: &[u32]) -> usize {
    fans(triangles, vertex, faces).len()
}

/// At a pinch vertex the incident faces form several fans; the smaller fans are
/// corner flares, not cloth. Keeping the largest fan is the qualified repair.
pub fn non_manifold_tips(triangles: &[[u32; 3]]) -> TipPrune {
    let mut incident: HashMap<u32, Vec<u32>> = HashMap::new();
    for (index, corners) in triangles.iter().enumerate() {
        for vertex in corners {
            incident.entry(*vertex).or_default().push(index as u32);
        }
    }
    let mut vertices = incident.keys().copied().collect::<Vec<_>>();
    vertices.sort_unstable();
    let mut removed: HashSet<u32> = HashSet::new();
    let mut pruned = Vec::new();
    for vertex in vertices {
        let faces = &incident[&vertex];
        if faces.len() < 2 {
            continue;
        }
        let groups = fans(triangles, vertex, faces);
        if groups.len() < 2 {
            continue;
        }
        let keep = groups
            .iter()
            .enumerate()
            .max_by(|(left_index, left), (right_index, right)| {
                left.len()
                    .cmp(&right.len())
                    .then_with(|| right_index.cmp(left_index))
            })
            .map(|(index, _)| index)
            .unwrap_or_default();
        groups
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != keep)
            .flat_map(|(_, group)| group.iter().copied())
            .for_each(|triangle| {
                removed.insert(triangle);
            });
        pruned.push(vertex);
    }
    let mut removed = removed.into_iter().collect::<Vec<_>>();
    removed.sort_unstable();
    TipPrune {
        removed,
        vertices: pruned,
    }
}

/// Group the faces around one vertex into fans joined by their shared edges.
pub(crate) fn fans(triangles: &[[u32; 3]], vertex: u32, faces: &[u32]) -> Vec<Vec<u32>> {
    let mut by_edge: HashMap<u64, Vec<u32>> = HashMap::new();
    for face in faces {
        let corners = triangles[*face as usize];
        for other in corners {
            if other == vertex {
                continue;
            }
            by_edge
                .entry(edge_key(vertex, other))
                .or_default()
                .push(*face);
        }
    }
    let mut remaining: HashSet<u32> = faces.iter().copied().collect();
    let mut groups = Vec::new();
    while let Some(start) = remaining.iter().copied().min() {
        let mut group = Vec::new();
        let mut stack = vec![start];
        remaining.remove(&start);
        while let Some(face) = stack.pop() {
            group.push(face);
            let corners = triangles[face as usize];
            for other in corners {
                if other == vertex {
                    continue;
                }
                for neighbor in by_edge.get(&edge_key(vertex, other)).into_iter().flatten() {
                    if remaining.remove(neighbor) {
                        stack.push(*neighbor);
                    }
                }
            }
        }
        group.sort_unstable();
        groups.push(group);
    }
    groups
}

fn edge_key(left: u32, right: u32) -> u64 {
    (u64::from(left.min(right)) << 32) | u64::from(left.max(right))
}

/// One boundary loop of a triangle mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryLoop {
    /// Boundary edges walked.
    pub edges: usize,
    /// Vertices in walk order.
    pub vertices: Vec<u32>,
    /// Edge-length-weighted midpoint of the boundary curve.
    ///
    /// This is the centroid of the drawn boundary, so splitting an edge into collinear
    /// pieces cannot move it: two tessellations of the same opening agree, which is what
    /// aperture matching compares against a declared anchor. A loop whose every edge has
    /// zero length is a single point, and that position is its center. @req DECL0013
    pub center: [f32; 3],
    /// Whether the walk returned to its first vertex.
    pub closed: bool,
    /// Summed boundary edge length, accumulated in f64.
    pub perimeter: f32,
}

impl BoundaryLoop {
    /// Boundary edges as vertex pairs, closed loops included.
    pub fn edge_pairs(&self) -> Vec<[u32; 2]> {
        let mut pairs = self
            .vertices
            .windows(2)
            .map(|window| [window[0], window[1]])
            .collect::<Vec<_>>();
        if self.closed
            && let (Some(first), Some(last)) = (self.vertices.first(), self.vertices.last())
        {
            pairs.push([*last, *first]);
        }
        pairs
    }
}

/// Boundary loops, largest first; ties break on the lowest vertex index.
pub fn boundary_loops(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<BoundaryLoop> {
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for [a, b, c] in triangles {
        for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
            *counts
                .entry((left.min(right), left.max(right)))
                .or_default() += 1;
        }
    }
    let mut boundary = counts
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(edge, _)| edge)
        .collect::<Vec<_>>();
    boundary.sort_unstable();
    let mut adjacency: HashMap<u32, Vec<u32>> = HashMap::new();
    for (left, right) in &boundary {
        adjacency.entry(*left).or_default().push(*right);
        adjacency.entry(*right).or_default().push(*left);
    }
    walk_loops(positions, boundary, adjacency)
}

/// Walk sorted boundary edges into loops, deterministic order.
///
/// Shared by the whole-surface walk and the kept-face walk, so a recovered disk is
/// matched against exactly the loop the construction already reported.
fn walk_loops(
    positions: &[[f32; 3]],
    boundary: Vec<(u32, u32)>,
    adjacency: HashMap<u32, Vec<u32>>,
) -> Vec<BoundaryLoop> {
    let mut visited: HashMap<(u32, u32), bool> = HashMap::new();
    let mut loops = Vec::new();
    for (start_left, start_right) in boundary {
        if visited
            .get(&(start_left, start_right))
            .copied()
            .unwrap_or(false)
        {
            continue;
        }
        visited.insert((start_left, start_right), true);
        let mut vertices = vec![start_left];
        let mut current = start_right;
        let mut previous = start_left;
        let mut closed = false;
        loop {
            vertices.push(current);
            let next = adjacency
                .get(&current)
                .into_iter()
                .flatten()
                .copied()
                .find(|neighbor| *neighbor != previous);
            let Some(next) = next else { break };
            if next == start_left {
                closed = true;
                visited.insert((current.min(next), current.max(next)), true);
                break;
            }
            let key = (current.min(next), current.max(next));
            if visited.get(&key).copied().unwrap_or(false) {
                break;
            }
            visited.insert(key, true);
            previous = current;
            current = next;
        }
        loops.push(finish_loop(positions, vertices, closed));
    }
    loops.sort_by(|left, right| {
        right
            .edges
            .cmp(&left.edges)
            .then_with(|| left.vertices.first().cmp(&right.vertices.first()))
    });
    loops
}

/// Boundary loops of a set of kept surface faces, in surface vertex space.
fn kept_boundary_loops(surface: &SourceSurface, kept: &HashSet<u32>) -> Vec<BoundaryLoop> {
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for face in kept {
        let triangle = surface.triangles()[*face as usize];
        for corner in 0..3 {
            let left = triangle[corner];
            let right = triangle[(corner + 1) % 3];
            *counts
                .entry((left.min(right), left.max(right)))
                .or_default() += 1;
        }
    }
    let mut boundary = counts
        .into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(edge, _)| edge)
        .collect::<Vec<_>>();
    boundary.sort_unstable();
    let mut adjacency: HashMap<u32, Vec<u32>> = HashMap::new();
    for (left, right) in &boundary {
        adjacency.entry(*left).or_default().push(*right);
        adjacency.entry(*right).or_default().push(*left);
    }
    walk_loops(surface.positions(), boundary, adjacency)
}

/// Length-weighted midpoint of one walked boundary curve plus its length.
///
/// The midpoint rule for the curve centroid: the result depends on the drawn boundary,
/// not on how it was tessellated, so two tessellations of the same opening agree. Every
/// edge is visited exactly once through the walk order; zero-length edges carry no curve
/// and are skipped exactly. Accumulated in f64 and converted once, because an f32 sum of
/// thousands of edges loses the differences an aperture radius is asked to resolve.
/// A connected walk whose every edge has zero length is one point, and that point is the
/// center. @req DECL0013
pub(crate) fn curve_center(
    positions: &[[f32; 3]],
    vertices: &[u32],
    closed: bool,
) -> ([f32; 3], f32) {
    let mut pairs = vertices
        .windows(2)
        .map(|window| [window[0], window[1]])
        .collect::<Vec<_>>();
    if closed && let (Some(first), Some(last)) = (vertices.first(), vertices.last()) {
        pairs.push([*last, *first]);
    }
    let mut weight = 0.0f64;
    let mut sum = [0.0f64; 3];
    for [first, second] in pairs {
        let left = positions[first as usize];
        let right = positions[second as usize];
        let edge_length = f64::from(length(subtract(right, left)));
        if edge_length == 0.0 {
            continue;
        }
        weight += edge_length;
        for (axis, slot) in sum.iter_mut().enumerate() {
            *slot += 0.5 * (f64::from(left[axis]) + f64::from(right[axis])) * edge_length;
        }
    }
    let center = match weight > 0.0 {
        true => sum.map(|value| (value / weight) as f32),
        false => {
            let first = *vertices
                .first()
                .expect("a walked boundary loop holds at least one vertex");
            positions[first as usize]
        }
    };
    (center, weight as f32)
}

fn finish_loop(positions: &[[f32; 3]], vertices: Vec<u32>, closed: bool) -> BoundaryLoop {
    let edges = match closed {
        true => vertices.len(),
        false => vertices.len().saturating_sub(1),
    };
    let (center, perimeter) = curve_center(positions, &vertices, closed);
    BoundaryLoop {
        edges,
        center,
        closed,
        perimeter,
        vertices,
    }
}

/// One expected physical opening and the geometry that identifies it.
#[derive(Debug, Clone, PartialEq)]
pub struct ApertureSpec {
    /// Profile role: `collar`, `hem`, `left cuff`, or any garment-local name.
    pub role: String,
    /// Declared landmark in the same frame as the mesh.
    pub anchor: [f32; 3],
    /// Largest distance from the anchor that still identifies this opening.
    pub radius: f32,
}

/// A profile aperture matched to one measured boundary loop.
#[derive(Debug, Clone, PartialEq)]
pub struct ApertureMatch {
    pub role: String,
    pub loop_index: usize,
    pub distance: f32,
}

/// Which loops are the declared apertures and which are incidental.
#[derive(Debug, Clone, PartialEq)]
pub struct ApertureResolution {
    /// One match per declared aperture, in profile order.
    pub matched: Vec<ApertureMatch>,
    /// Loop indices that are not apertures, ascending.
    pub incidental: Vec<usize>,
}

impl ApertureResolution {
    pub fn incidental_loops(&self) -> usize {
        self.incidental.len()
    }
}

/// Match declared apertures to measured loops and refuse ambiguity.
///
/// An ambiguous or unmatched opening is reported; filling it silently is the
/// defect this rule exists to prevent. @req DECL0013
pub fn resolve_apertures(
    loops: &[BoundaryLoop],
    specs: &[ApertureSpec],
) -> Result<ApertureResolution, ControlError> {
    validate_apertures(specs)?;
    let mut assigned = vec![false; loops.len()];
    let mut matched = Vec::with_capacity(specs.len());
    for spec in specs {
        let candidates = loops
            .iter()
            .enumerate()
            .filter(|(index, _)| !assigned[*index])
            .map(|(index, boundary)| (index, length(subtract(boundary.center, spec.anchor))))
            .filter(|(_, distance)| *distance <= spec.radius)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {
                return Err(ControlError::UnknownAperture {
                    role: spec.role.clone(),
                    radius: spec.radius,
                });
            }
            [(index, distance)] => {
                if !loops[*index].closed {
                    return Err(ControlError::UnclosedBoundary { loop_index: *index });
                }
                assigned[*index] = true;
                matched.push(ApertureMatch {
                    role: spec.role.clone(),
                    loop_index: *index,
                    distance: *distance,
                });
            }
            many => {
                return Err(ControlError::AmbiguousAperture {
                    role: spec.role.clone(),
                    candidates: many.len(),
                });
            }
        }
    }
    let incidental = (0..loops.len()).filter(|index| !assigned[*index]).collect();
    Ok(ApertureResolution {
        matched,
        incidental,
    })
}

/// Validate declared apertures: at least one, unique roles, finite anchors, a
/// positive radius. Shared by profile parsing and loop matching.
pub(crate) fn validate_apertures(specs: &[ApertureSpec]) -> Result<(), ControlError> {
    if specs.is_empty() {
        return Err(ControlError::EmptyProfile);
    }
    let mut roles = HashSet::new();
    for spec in specs {
        if spec.role.trim().is_empty() {
            return Err(ControlError::InvalidProfile {
                field: "apertures".to_string(),
                reason: "a role name is blank".to_string(),
            });
        }
        if !roles.insert(spec.role.as_str()) {
            return Err(ControlError::DuplicateRole {
                role: spec.role.clone(),
            });
        }
        if !spec.anchor.iter().all(|value| value.is_finite()) {
            return Err(ControlError::NonFiniteAnchor {
                role: spec.role.clone(),
            });
        }
        if !spec.radius.is_finite() || spec.radius <= 0.0 {
            return Err(ControlError::NonPositiveRadius {
                role: spec.role.clone(),
            });
        }
    }
    Ok(())
}

/// Declared limits on how much reconstruction may remove or close.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepairLimits {
    /// Non-aperture loops a construction may leave for closure.
    pub max_incidental_loops: usize,
    /// Loops at or below this size are collapsed instead of filled.
    pub collapse_below_edges: usize,
    /// Largest non-aperture loop that may be filled.
    pub max_fill_edges: usize,
    /// Share of the surface corner-flare pruning may remove.
    pub max_tip_removal_share: f32,
}

impl Default for RepairLimits {
    fn default() -> Self {
        Self {
            max_incidental_loops: 4,
            collapse_below_edges: 10,
            max_fill_edges: 64,
            max_tip_removal_share: 0.05,
        }
    }
}

/// How a closure loop may be patched without reusing existing surface.
///
/// A guide-facing slit that reaches a crease has two loop vertices that already
/// share an interior edge. A free-chord triangulation reuses that edge and makes it
/// non-manifold, so such a loop is closed with a single boundary patch instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureKind {
    /// Free-chord triangle fill: the loop interior contains no existing edge.
    Triangulated,
    /// One boundary n-gon over the loop edges, then triangulate the patch.
    Boundary,
}

impl ClosureKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Triangulated => "triangulated",
            Self::Boundary => "boundary",
        }
    }
}

/// What the host must do to one incidental loop.
#[derive(Debug, Clone, PartialEq)]
pub enum RepairAction {
    /// Collapse the loop's shortest edge.
    Collapse {
        loop_index: usize,
        edges: usize,
        edge: [u32; 2],
        length: f32,
    },
    /// Fill the loop with a triangulated patch.
    Fill {
        loop_index: usize,
        edges: usize,
        closure: ClosureKind,
    },
}

/// Decide how each incidental loop closes; nothing is closed without a report.
pub fn repair_actions(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    loops: &[BoundaryLoop],
    resolution: &ApertureResolution,
    limits: &RepairLimits,
) -> Result<Vec<RepairAction>, ControlError> {
    if resolution.incidental.len() > limits.max_incidental_loops {
        return Err(ControlError::IncidentalLimit {
            loops: resolution.incidental.len(),
            limit: limits.max_incidental_loops,
        });
    }
    resolution
        .incidental
        .iter()
        .map(|loop_index| {
            let boundary = &loops[*loop_index];
            if !boundary.closed {
                return Err(ControlError::UnclosedBoundary {
                    loop_index: *loop_index,
                });
            }
            if boundary.edges <= limits.collapse_below_edges {
                let (edge, length) = shortest_edge(positions, boundary);
                return Ok(RepairAction::Collapse {
                    loop_index: *loop_index,
                    edges: boundary.edges,
                    edge,
                    length,
                });
            }
            if boundary.edges > limits.max_fill_edges {
                return Err(ControlError::FillLimit {
                    loop_index: *loop_index,
                    edges: boundary.edges,
                    limit: limits.max_fill_edges,
                });
            }
            Ok(RepairAction::Fill {
                loop_index: *loop_index,
                edges: boundary.edges,
                closure: match pinched_boundary(triangles, boundary) {
                    true => ClosureKind::Boundary,
                    false => ClosureKind::Triangulated,
                },
            })
        })
        .collect()
}

/// Whether two non-adjacent loop vertices already share an interior edge.
///
/// Such a loop encloses existing surface: a free-chord patch would reuse that edge
/// and make it non-manifold, so the closure must be a boundary patch.
fn pinched_boundary(triangles: &[[u32; 3]], boundary: &BoundaryLoop) -> bool {
    let mut faces: HashMap<u64, u32> = HashMap::new();
    for [a, b, c] in triangles {
        for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
            *faces.entry(edge_key(left, right)).or_default() += 1;
        }
    }
    let vertices = &boundary.vertices;
    for (index, first) in vertices.iter().enumerate() {
        for (other_index, second) in vertices.iter().enumerate() {
            let adjacent = other_index + 1 == index
                || index + 1 == other_index
                || (index == 0 && other_index + 1 == vertices.len())
                || (other_index == 0 && index + 1 == vertices.len());
            if index >= other_index || adjacent {
                continue;
            }
            if faces.get(&edge_key(*first, *second)).copied().unwrap_or(0) > 1 {
                return true;
            }
        }
    }
    false
}

/// The shortest boundary edge of a loop; ties keep the lowest vertex pair.
fn shortest_edge(positions: &[[f32; 3]], boundary: &BoundaryLoop) -> ([u32; 2], f32) {
    boundary
        .edge_pairs()
        .into_iter()
        .map(|edge| {
            (
                edge,
                length(subtract(
                    positions[edge[1] as usize],
                    positions[edge[0] as usize],
                )),
            )
        })
        .min_by(|left, right| {
            left.1.total_cmp(&right.1).then_with(|| {
                (left.0[0].min(left.0[1]), left.0[0].max(left.0[1]))
                    .cmp(&(right.0[0].min(right.0[1]), right.0[0].max(right.0[1])))
            })
        })
        // A closed boundary loop always contributes at least one edge pair.
        .expect("a closed boundary loop has at least one edge")
}

/// Declared triangle bounds for the reduced control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReductionLimits {
    pub target_triangles: usize,
    pub minimum_triangles: usize,
    pub maximum_triangles: usize,
}

impl ReductionLimits {
    fn validate(&self) -> Result<(), ControlError> {
        if self.minimum_triangles == 0
            || self.minimum_triangles > self.target_triangles
            || self.target_triangles > self.maximum_triangles
        {
            return Err(ControlError::InvalidReductionLimits {
                minimum: self.minimum_triangles,
                target: self.target_triangles,
                maximum: self.maximum_triangles,
            });
        }
        Ok(())
    }
}

/// The collapse ratio the host should apply, and the bounds to check after.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReductionPlan {
    pub current_triangles: usize,
    pub target_triangles: usize,
    /// Ratio for a collapse decimation; `1.0` when the surface is already small.
    pub ratio: f32,
    pub removed: usize,
}

impl ReductionPlan {
    /// Plan a reduction, or leave the surface alone when it is at or below target.
    pub fn plan(current_triangles: usize, limits: &ReductionLimits) -> Result<Self, ControlError> {
        limits.validate()?;
        if current_triangles == 0 {
            return Err(ControlError::EmptyMesh);
        }
        match current_triangles <= limits.target_triangles {
            true => Ok(Self {
                current_triangles,
                target_triangles: current_triangles,
                ratio: 1.0,
                removed: 0,
            }),
            false => Ok(Self {
                current_triangles,
                target_triangles: limits.target_triangles,
                ratio: limits.target_triangles as f32 / current_triangles as f32,
                removed: current_triangles - limits.target_triangles,
            }),
        }
    }
}

/// Why a source triangle did not survive reconstruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// Two corners identified, so the triangle carries no surface.
    WeldedDegenerate,
    /// The face normal disagrees with the guide radial: lining or rim.
    Interior,
    /// The triangle belongs to a disconnected shell.
    Island,
    /// The triangle is a corner flare at a pinch vertex.
    CornerFlare,
}

impl RemovalReason {
    pub fn name(&self) -> &'static str {
        match self {
            Self::WeldedDegenerate => "welded degenerate",
            Self::Interior => "interior",
            Self::Island => "island",
            Self::CornerFlare => "corner flare",
        }
    }
}

/// One removed source triangle and the stage that removed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removal {
    /// Source triangle index, in input order.
    pub triangle: u32,
    pub reason: RemovalReason,
}

/// How a profile turns the identified source shell into the control surface.
///
/// A thick sheet is reconstructed from its outward faces only. A capped solid is
/// reconstructed from its whole identified boundary, whose declared cuts are the
/// openings: its concave folds are real surface, not lining. The choice is profile
/// data and is never inferred from a name or switched after a failure. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePolicy {
    /// Keep the faces that point away from the guide.
    Guide,
    /// Keep the whole identified boundary; declared cuts carry the openings.
    Boundary,
}

impl SurfacePolicy {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Guide => "guide",
            Self::Boundary => "boundary",
        }
    }
}

/// Whether original excluded source disks may be recovered at concave folds.
///
/// Guide-facing selection can omit real fabric at a fold and leave its outline as an
/// incidental loop; a declared `IncidentalDisks` policy may retain exactly such a
/// disk instead of filling over it. Absence preserves the existing selection route.
/// @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRecovery {
    /// No recovery: the published selection and repair path.
    None,
    /// Recover original excluded disks that close one eligible incidental loop.
    IncidentalDisks,
}

impl SourceRecovery {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::IncidentalDisks => "incidental-disks",
        }
    }
}

/// Which half-space a source cut keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainSide {
    /// Keep geometry on the positive side of the plane normal.
    Positive,
    /// Keep geometry on the negative side of the plane normal.
    Negative,
}

impl RetainSide {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }
}

/// One explicit source-opening cut: a plane and the side that survives.
///
/// A capped solid sculpt has no passages; a declared half-plane cut removes the cap
/// and leaves a real opening. The plane is validated in the shared core before the
/// host mutates anything, and its normal is unit length. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct ControlCut {
    pub label: String,
    /// Plane point in the mesh frame.
    pub point: [f32; 3],
    /// Unit plane normal in the mesh frame.
    pub normal: [f32; 3],
    pub retain: RetainSide,
}

/// What reconstruction removed, by stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Composition {
    pub source_triangles: usize,
    pub source_vertices: usize,
    pub coincident_vertices: usize,
    pub exterior_triangles: usize,
    pub interior_removed: usize,
    pub welded_degenerate_removed: usize,
    pub island_components_removed: usize,
    pub island_triangles_removed: usize,
    pub tip_triangles_removed: usize,
    /// Original excluded faces that source-disk recovery retained instead of removing.
    pub recovered_triangles: usize,
}

/// Classify the source shell's outward triangles without any other decision.
///
/// Returns the identified surface and the kept triangle indices (into
/// [`SourceSurface::triangles`]) for the declared [`SurfacePolicy`], so coverage and
/// construction measure the same source surface. @req DECL0013
pub fn source_surface(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    guide: &Guide,
    policy: SurfacePolicy,
) -> Result<(SourceSurface, Vec<u32>), ControlError> {
    let surface = SourceSurface::identify(positions, triangles)?;
    let degenerate = surface
        .welded_degenerate()
        .into_iter()
        .collect::<HashSet<_>>();
    let candidates = (0..surface.triangles.len() as u32)
        .filter(|index| !degenerate.contains(index))
        .collect::<Vec<_>>();
    if policy == SurfacePolicy::Boundary {
        // The identified shell is already the surface: its folds are geometry, and
        // only the declared cuts (and the shared topology checks) may remove faces.
        return Ok((surface, candidates));
    }
    let candidate_triangles = candidates
        .iter()
        .map(|index| surface.triangles[*index as usize])
        .collect::<Vec<_>>();
    let selection = exterior_triangles(&surface.positions, &candidate_triangles, guide)?;
    let kept = selection
        .kept
        .iter()
        .map(|index| candidates[*index as usize])
        .collect();
    Ok((surface, kept))
}

/// The reconstructed high control and the decisions that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExteriorOutcome {
    /// Compacted positions, in identified-topology order.
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// Source vertex index behind each returned position, one per vertex.
    pub vertex_sources: Vec<u32>,
    /// Source triangle index behind each returned triangle, in the same order.
    pub selected_faces: Vec<u32>,
    /// Every removed source triangle with its reason.
    pub removals: Vec<Removal>,
    pub composition: Composition,
    pub loops: Vec<BoundaryLoop>,
    pub apertures: ApertureResolution,
    /// Native closures the source stage requires, already limit-checked.
    pub actions: Vec<RepairAction>,
    /// Original disks the declared policy recovered, with their evidence.
    pub recovery: RecoveryReport,
    pub reduction: ReductionPlan,
}

/// Reconstruct the outward control surface from one thick source shell.
///
/// The returned surface still carries the native topology: the host triangulates,
/// fills the incidental loops and applies the planned reduction, then reports the
/// result back through [`measure::ControlDiagnostics`]. @req DECL0013
pub fn exterior_control(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    profile: &ControlProfile,
) -> Result<ExteriorOutcome, ControlError> {
    let selection = source_selection(positions, triangles, profile)?;
    let surface = &selection.surface;
    let (positions, triangles, kept_vertices) =
        compact(&surface.positions, &selection.selected_triangles);
    let vertex_sources = kept_vertices
        .iter()
        .map(|index| surface.vertex_sources[*index as usize])
        .collect::<Vec<_>>();
    let loops = boundary_loops(&positions, &triangles);
    let apertures = resolve_apertures(&loops, &profile.apertures)?;
    let actions = repair_actions(
        &positions,
        &triangles,
        &loops,
        &apertures,
        &profile.source_repair,
    )?;
    let reduction = ReductionPlan::plan(triangles.len(), &profile.reduction)?;
    Ok(ExteriorOutcome {
        positions,
        triangles,
        vertex_sources,
        selected_faces: selection.selected_faces.clone(),
        removals: selection.removals.clone(),
        composition: Composition {
            source_triangles: surface.source_triangles,
            source_vertices: surface.source_vertices,
            coincident_vertices: surface.coincident_vertices(),
            exterior_triangles: selection.selected_triangles.len(),
            interior_removed: selection.interior_removed,
            welded_degenerate_removed: selection.welded_degenerate_removed,
            island_components_removed: selection.island_components_removed,
            island_triangles_removed: selection.island_triangles_removed,
            tip_triangles_removed: selection.tip_triangles_removed,
            recovered_triangles: selection.recovery.recovered_faces,
        },
        loops,
        apertures,
        actions,
        reduction,
        recovery: selection.recovery.clone(),
    })
}

/// Source faces a selection keeps, what it removed, and the resulting boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSelection {
    pub surface: SourceSurface,
    /// Selected surface faces, in construction order.
    pub selected_faces: Vec<u32>,
    /// The same faces as triangles, in the same order.
    pub selected_triangles: Vec<[u32; 3]>,
    pub removals: Vec<Removal>,
    pub interior_removed: usize,
    pub welded_degenerate_removed: usize,
    pub island_components_removed: usize,
    pub island_triangles_removed: usize,
    pub tip_triangles_removed: usize,
    /// Kept-boundary loops in surface vertex space, largest first.
    pub loops: Vec<BoundaryLoop>,
    pub apertures: ApertureResolution,
    pub recovery: RecoveryReport,
}

/// One pruned selection before compaction.
struct Pruned {
    faces: Vec<u32>,
    triangles: Vec<[u32; 3]>,
    removals: Vec<Removal>,
    interior_removed: usize,
    island_components_removed: usize,
    island_triangles_removed: usize,
    tip_triangles_removed: usize,
}

/// Degenerate removal, guide removal, largest component and corner flares, in order.
fn prune(
    surface: &SourceSurface,
    kept_indices: &[u32],
    degenerate: &HashSet<u32>,
    profile: &ControlProfile,
) -> Result<Pruned, ControlError> {
    let mut removals = degenerate
        .iter()
        .map(|triangle| Removal {
            triangle: *triangle,
            reason: RemovalReason::WeldedDegenerate,
        })
        .collect::<Vec<_>>();
    let kept_set = kept_indices.iter().copied().collect::<HashSet<_>>();
    let interior = (0..surface.triangles.len() as u32)
        .filter(|index| !degenerate.contains(index) && !kept_set.contains(index))
        .count();
    removals.extend(
        (0..surface.triangles.len() as u32)
            .filter(|index| !degenerate.contains(index) && !kept_set.contains(index))
            .map(|triangle| Removal {
                triangle,
                reason: RemovalReason::Interior,
            }),
    );
    let kept = kept_indices
        .iter()
        .map(|index| surface.triangles[*index as usize])
        .collect::<Vec<_>>();
    let component = largest_component(&kept);
    let component_set = component.kept.iter().copied().collect::<HashSet<_>>();
    removals.extend(
        (0..kept.len() as u32)
            .filter(|index| !component_set.contains(index))
            .map(|index| Removal {
                triangle: kept_indices[index as usize],
                reason: RemovalReason::Island,
            }),
    );
    let component_triangles = component
        .kept
        .iter()
        .map(|index| kept[*index as usize])
        .collect::<Vec<_>>();
    let component_faces = component
        .kept
        .iter()
        .map(|index| kept_indices[*index as usize])
        .collect::<Vec<_>>();
    let tips = non_manifold_tips(&component_triangles);
    let tip_set = tips.removed.iter().copied().collect::<HashSet<_>>();
    removals.extend(tips.removed.iter().map(|index| Removal {
        triangle: component_faces[*index as usize],
        reason: RemovalReason::CornerFlare,
    }));
    removals.sort_by_key(|removal| removal.triangle);
    let triangles = component_triangles
        .iter()
        .enumerate()
        .filter(|(index, _)| !tip_set.contains(&(*index as u32)))
        .map(|(_, triangle)| *triangle)
        .collect::<Vec<_>>();
    let faces = component_faces
        .iter()
        .enumerate()
        .filter(|(index, _)| !tip_set.contains(&(*index as u32)))
        .map(|(_, face)| *face)
        .collect::<Vec<_>>();
    let share = tips.removed.len() as f32 / triangles.len().max(1) as f32;
    if share > profile.source_repair.max_tip_removal_share {
        return Err(ControlError::TipRemovalLimit {
            removed: tips.removed.len(),
            triangles: component_triangles.len(),
            limit: profile.source_repair.max_tip_removal_share,
        });
    }
    Ok(Pruned {
        faces,
        triangles,
        removals,
        interior_removed: interior,
        island_components_removed: component.removed_components,
        island_triangles_removed: component.removed_triangles,
        tip_triangles_removed: tips.removed.len(),
    })
}

/// Source faces the constructor and the coverage check both treat as the surface.
///
/// This is the single owner of the selection rule: guide or retained-boundary surface,
/// component and tip pruning, then the declared source-disk recovery. `exterior_control`
/// and the CLI coverage path call it, so a recovered disk cannot be visible to one and
/// invisible to the other. @req DECL0013
pub fn source_selection(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    profile: &ControlProfile,
) -> Result<SourceSelection, ControlError> {
    let (surface, kept_indices) =
        source_surface(positions, triangles, &profile.guide, profile.surface)?;
    let degenerate = surface
        .welded_degenerate()
        .into_iter()
        .collect::<HashSet<_>>();
    let mut pruned = prune(&surface, &kept_indices, &degenerate, profile)?;
    let mut recovery = RecoveryReport::default();
    let mut loops = kept_boundary_loops(&surface, &pruned.faces.iter().copied().collect());
    let mut apertures = resolve_apertures(&loops, &profile.apertures)?;
    if profile.source_recovery == SourceRecovery::IncidentalDisks {
        let aperture_vertices = apertures
            .matched
            .iter()
            .flat_map(|matched| loops[matched.loop_index].vertices.iter().copied())
            .collect::<HashSet<_>>();
        let kept = pruned.faces.iter().copied().collect::<HashSet<_>>();
        let mut added = Vec::new();
        for index in apertures.incidental.clone() {
            let boundary = &loops[index];
            // Only a loop the published repair path would have filled is eligible; a
            // collapse or an over-limit loop keeps the existing behaviour.
            let fillable = boundary.closed
                && boundary.edges > profile.source_repair.collapse_below_edges
                && boundary.edges <= profile.source_repair.max_fill_edges;
            if !fillable {
                continue;
            }
            match recovery::recover_disk(
                &surface,
                &degenerate,
                &kept,
                index,
                &boundary.vertices,
                boundary.closed,
                &aperture_vertices,
            ) {
                Ok(Some(disk)) => {
                    added.extend(disk.faces.iter().copied());
                    recovery.disks.push(disk);
                }
                Ok(None) => {}
                Err(refusal) => recovery.refused.push((index, refusal)),
            }
        }
        if !added.is_empty() {
            added.sort_unstable();
            added.dedup();
            recovery.recovered_faces = added.len();
            let mut augmented = kept_indices.clone();
            augmented.extend(added);
            augmented.sort_unstable();
            augmented.dedup();
            pruned = prune(&surface, &augmented, &degenerate, profile)?;
            loops = kept_boundary_loops(&surface, &pruned.faces.iter().copied().collect());
            apertures = resolve_apertures(&loops, &profile.apertures)?;
        }
    }
    Ok(SourceSelection {
        surface,
        selected_faces: pruned.faces,
        selected_triangles: pruned.triangles,
        removals: pruned.removals,
        interior_removed: pruned.interior_removed,
        welded_degenerate_removed: degenerate.len(),
        island_components_removed: pruned.island_components_removed,
        island_triangles_removed: pruned.island_triangles_removed,
        tip_triangles_removed: pruned.tip_triangles_removed,
        loops,
        apertures,
        recovery,
    })
}

/// The repaired control bounding loop decisions, after native decimation.
#[derive(Debug, Clone, PartialEq)]
pub struct RepairOutcome {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub tip_vertices: Vec<u32>,
    /// Stage-local triangles removed as corner flares, ascending.
    pub tip_triangles: Vec<u32>,
    pub tip_removed: usize,
    pub loops: Vec<BoundaryLoop>,
    pub apertures: ApertureResolution,
    pub actions: Vec<RepairAction>,
}

/// Decide the post-decimation repair of one control surface.
///
/// Decimation creates pinch vertices and micro-loops; the same aperture rule names
/// the real openings and the planned closure of everything else. @req DECL0013
pub fn repair_control(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    profile: &ControlProfile,
) -> Result<RepairOutcome, ControlError> {
    let surface = SourceSurface::identify(positions, triangles)?;
    let tips = non_manifold_tips(&surface.triangles);
    let tip_set = tips.removed.iter().copied().collect::<HashSet<_>>();
    let kept = surface
        .triangles
        .iter()
        .enumerate()
        .filter(|(index, _)| !tip_set.contains(&(*index as u32)))
        .map(|(_, triangle)| *triangle)
        .collect::<Vec<_>>();
    let share = tips.removed.len() as f32 / kept.len().max(1) as f32;
    if share > profile.reduced_repair.max_tip_removal_share {
        return Err(ControlError::TipRemovalLimit {
            removed: tips.removed.len(),
            triangles: kept.len(),
            limit: profile.reduced_repair.max_tip_removal_share,
        });
    }
    let (positions, triangles, _) = compact(&surface.positions, &kept);
    let loops = boundary_loops(&positions, &triangles);
    let apertures = resolve_apertures(&loops, &profile.apertures)?;
    let actions = repair_actions(
        &positions,
        &triangles,
        &loops,
        &apertures,
        &profile.reduced_repair,
    )?;
    let tip_removed = tips.removed.len();
    Ok(RepairOutcome {
        positions,
        triangles,
        tip_vertices: tips.vertices,
        tip_triangles: tips.removed,
        tip_removed,
        loops,
        apertures,
        actions,
    })
}

/// Drop unreferenced vertices, keeping the surviving order; also return the
/// original vertex index behind each kept position.
pub fn compact(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>, Vec<u32>) {
    let mut referenced = triangles.iter().flatten().copied().collect::<Vec<_>>();
    referenced.sort_unstable();
    referenced.dedup();
    let mut remap = HashMap::new();
    let mut kept = Vec::with_capacity(referenced.len());
    for (index, source) in referenced.iter().enumerate() {
        remap.insert(*source, index as u32);
        kept.push(positions[*source as usize]);
    }
    let remapped = triangles
        .iter()
        .map(|triangle| triangle.map(|vertex| remap[&vertex]))
        .collect();
    (kept, remapped, referenced)
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn length(vector: [f32; 3]) -> f32 {
    (f64::from(vector[0]).powi(2) + f64::from(vector[1]).powi(2) + f64::from(vector[2]).powi(2))
        .sqrt() as f32
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

/// Outward face normal from the winding: `(b - a) x (c - a)`.
fn cross(b: [f32; 3], a: [f32; 3], c: [f32; 3]) -> [f64; 3] {
    let u = subtract(b, a).map(f64::from);
    let v = subtract(c, a).map(f64::from);
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

fn doubled_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f64 {
    let normal = cross(b, a, c);
    dot(normal, normal).sqrt()
}

fn find(parent: &mut [usize], index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    let mut current = index;
    while parent[current] != root {
        let next = parent[current];
        parent[current] = root;
        current = next;
    }
    root
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left = find(parent, left);
    let right = find(parent, right);
    if left != right {
        parent[left.max(right)] = left.min(right);
    }
}

#[cfg(test)]
mod curve_center_tests {
    use super::{curve_center, finish_loop};

    /// The same geometry as a closed square, optionally nonuniformly subdivided.
    fn subdivided_square(subdivision: &[(u32, f32)]) -> (Vec<[f32; 3]>, Vec<u32>) {
        let mut positions = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let mut outline = vec![0u32, 1, 2, 3];
        if !subdivision.is_empty() {
            let mut bottom = vec![0u32];
            for (index, x) in subdivision {
                let vertex = positions.len() as u32;
                positions.push([*x, 0.0, 0.0]);
                assert_eq!(vertex, *index);
                bottom.push(vertex);
            }
            bottom.push(1);
            outline = bottom.into_iter().chain([2u32, 3]).collect();
        }
        (positions, outline)
    }

    #[test]
    fn a_closed_curve_center_ignores_how_its_edges_are_split() {
        let (coarse_positions, coarse_outline) = subdivided_square(&[]);
        let (fine_positions, fine_outline) = subdivided_square(&[(4, 0.5), (5, 1.7)]);
        let coarse = finish_loop(&coarse_positions, coarse_outline, true);
        let fine = finish_loop(&fine_positions, fine_outline, true);
        assert_eq!(coarse.vertices.len(), 4);
        assert_eq!(fine.vertices.len(), 6);
        assert_eq!(coarse.perimeter, fine.perimeter);
        assert_eq!(coarse.center, fine.center);
        assert_eq!(coarse.center, [1.0, 0.5, 0.0]);
    }

    #[test]
    fn an_open_chain_walks_every_edge_once_and_centers_on_its_own_curve() {
        // An L-shaped open chain: two edges of different lengths, no closing edge.
        let coarse_positions = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
        let coarse = finish_loop(&coarse_positions, vec![0, 1, 2], false);
        assert_eq!(coarse.edges, 2);
        assert_eq!(coarse.edge_pairs().len(), 2);
        assert_eq!(coarse.perimeter, 3.0);
        // Each edge contributes its midpoint once: ((1,0)*2 + (2,0.5)*1) / 3.
        assert_eq!(coarse.center, [4.0 / 3.0, 1.0 / 6.0, 0.0]);

        // The same drawn curve with the long edge split nonuniformly.
        let fine_positions = vec![
            [0.0, 0.0, 0.0],
            [0.5, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 0.0],
        ];
        let fine = finish_loop(&fine_positions, vec![0, 1, 2, 3], false);
        assert_eq!(fine.edges, 3);
        assert_eq!(fine.edge_pairs().len(), 3);
        assert_eq!(fine.perimeter, coarse.perimeter);
        assert_eq!(fine.center, coarse.center);

        // Closing the chain is a different curve, so the centroid must move.
        let closed = finish_loop(&coarse_positions, vec![0, 1, 2], true);
        assert_eq!(closed.edges, 3);
        assert_ne!(closed.center, coarse.center);
    }

    #[test]
    fn curve_centers_remain_finite_across_coordinate_scales() {
        [1e-30f32, 1e30f32].into_iter().for_each(|scale| {
            let positions = [[0.0, 0.0, 0.0], [scale, 0.0, 0.0]];
            let (center, perimeter) = curve_center(&positions, &[0, 1], false);
            assert!(center.iter().all(|value| value.is_finite()), "{scale}");
            assert_eq!(center, [scale / 2.0, 0.0, 0.0], "{scale}");
            assert_eq!(perimeter, scale, "{scale}");
        });
    }

    #[test]
    fn a_zero_length_curve_reports_its_shared_point() {
        let positions = vec![[0.25, 0.5, 0.75]; 3];
        let center = curve_center(&positions, &[0, 1, 2], true);
        assert_eq!(center, ([0.25, 0.5, 0.75], 0.0));
        let open = curve_center(&positions, &[0, 1], false);
        assert_eq!(open, ([0.25, 0.5, 0.75], 0.0));
    }
}
