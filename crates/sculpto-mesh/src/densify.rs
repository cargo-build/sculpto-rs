//! Neutral densify: midpoint subdivision plus optional boundary-locked
//! smoothing, measured and returned as a diagnostic surface.
//!
//! [`crate::dress::midpoint_subdivide`] gives the contact solve its
//! resolution, but the solve owns the result: a dressed mesh is moved into its
//! declared band and its registration, so "densified" never exists on its own.
//! This module is the neutral half of that pair: the authored geometry with
//! more triangles, nothing else. It is for diagnosing a coarse mesh — the
//! tear, the scallop and the poke that a sparse vertex pass hides become
//! resolvable at the densified resolution — and it is not a fit result: no
//! body, no band, no clearance, and the authored vertex positions are kept
//! exactly. @req DECL0013
//!
//! Optional uniform Laplacian smoothing (the [`crate::joint_seal`] band
//! smoother's mechanics) softens shading noise on the densified surface. The
//! invariant that makes it usable for review: **boundary vertices never
//! move**, in any mode. A hem is authored; smoothing may soften the interior
//! but cannot creep a rim. The report measures the boundary displacement it
//! caused, which is zero by the freeze, and the coincident-position groups
//! move by one shared position so authored slits and seam twins stay closed.
//!
//! The report carries no fit vocabulary on purpose. A densify+smooth result
//! looks like a clean garment; an agent reading it must find
//! `diagnostic: true` on the wire and no band or clearance field to mistake
//! for a verdict. @req DECL0013

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::check::unit_triangle_normal;
use crate::classify::component_groups;
use crate::dress::{MAX_CUTS, midpoint_subdivide};
use crate::intake::{Opening, boundary_loops};
use crate::joint_seal::SMOOTH_LAMBDA;
use crate::surface_util::first_non_finite;
use crate::topology::{key as position_key, position_topology};

/// Smoothing passes one densify may run; `0` is pure densify. The seal's own
/// iteration ceiling, reused so the two smoothers refuse at the same bound.
pub const MAX_SMOOTH_ITERATIONS: u32 = 20;

/// What to run: subdivision rounds plus optional boundary-locked smoothing.
///
/// `cuts` follows the dress solve's own ceiling ([`MAX_CUTS`]); the third
/// round costs 16× the queries for a triangle already finer than any review
/// needs. `smooth_max_shift` is the hard cumulative displacement bound of the
/// smoothing stage: a vertex that would move beyond it refuses the command
/// instead of being clipped, so the diagnostic never silently corrupts the
/// surface it exists to show. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct DensifyParams {
    /// Midpoint subdivision rounds, `0..=2`.
    pub cuts: u32,
    /// Laplacian passes over the interior, `0..=20`; `0` is pure densify.
    pub smooth_iterations: u32,
    /// Hard cumulative per-vertex displacement bound of smoothing, in input
    /// units; exceeded means refused, never clipped. `None` measures without
    /// bounding.
    pub smooth_max_shift: Option<f32>,
}

impl Default for DensifyParams {
    fn default() -> Self {
        Self {
            cuts: 1,
            smooth_iterations: 0,
            smooth_max_shift: None,
        }
    }
}

impl DensifyParams {
    /// Refuse every value the module would otherwise silently replace or clip.
    ///
    /// # Errors
    /// [`DensifyError::Cuts`], [`DensifyError::SmoothIterations`] and
    /// [`DensifyError::SmoothMaxShift`] name the parameter and the bound.
    pub fn validate(&self) -> Result<(), DensifyError> {
        if self.cuts > MAX_CUTS {
            return Err(DensifyError::Cuts(self.cuts));
        }
        if self.smooth_iterations > MAX_SMOOTH_ITERATIONS {
            return Err(DensifyError::SmoothIterations(self.smooth_iterations));
        }
        if let Some(bound) = self.smooth_max_shift
            && (!bound.is_finite() || bound < 0.0)
        {
            return Err(DensifyError::SmoothMaxShift(bound));
        }
        Ok(())
    }
}

/// Why a densify was refused. Every variant names its input, so a caller
/// reads the refusal and not a combined message. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DensifyError {
    /// The mesh has no vertices.
    EmptyMesh,
    /// The mesh has vertices but no triangles.
    EmptyFaces,
    /// A vertex position is not finite.
    NonFinitePosition { vertex: usize },
    /// A triangle references a vertex the mesh does not have.
    FaceIndex { face: usize, vertex: u32 },
    /// A triangle repeats a corner index. Subdividing it would collapse its
    /// sub-triangles onto the apex and change the mesh's topology, so the
    /// command refuses instead of producing counts that mean nothing.
    DegenerateFace { face: usize },
    /// An edge is used by more than two triangles. Its interior/boundary
    /// reading is ambiguous, so the boundary inventory and the frozen-hem
    /// invariant are not defined on it.
    NonManifoldEdge { edge: (u32, u32), uses: usize },
    /// Subdivision rounds above [`MAX_CUTS`].
    Cuts(u32),
    /// Smoothing passes above [`MAX_SMOOTH_ITERATIONS`].
    SmoothIterations(u32),
    /// A negative or non-finite displacement bound.
    SmoothMaxShift(f32),
    /// Smoothing would move a vertex beyond the declared bound; the mesh is
    /// left unwritten rather than clipped.
    ShiftLimit {
        vertex: usize,
        required: f64,
        bound: f64,
    },
    /// Smoothing produced a non-finite position.
    NonFiniteResult { vertex: usize },
}

impl std::fmt::Display for DensifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMesh => write!(formatter, "the mesh has no vertices"),
            Self::EmptyFaces => write!(formatter, "the mesh has no triangles"),
            Self::NonFinitePosition { vertex } => {
                write!(formatter, "vertex {vertex} is not finite")
            }
            Self::FaceIndex { face, vertex } => write!(
                formatter,
                "triangle {face} references vertex {vertex}, which the mesh does not have"
            ),
            Self::DegenerateFace { face } => write!(
                formatter,
                "triangle {face} repeats a corner index; subdivision would collapse it and change the topology"
            ),
            Self::NonManifoldEdge { edge, uses } => write!(
                formatter,
                "edge {}-{} is used by {uses} triangles; the boundary reading is ambiguous",
                edge.0, edge.1
            ),
            Self::Cuts(cuts) => write!(formatter, "cuts must be 0..={MAX_CUTS}, got {cuts}"),
            Self::SmoothIterations(iterations) => write!(
                formatter,
                "smooth_iterations must be 0..={MAX_SMOOTH_ITERATIONS}, got {iterations}"
            ),
            Self::SmoothMaxShift(bound) => {
                write!(
                    formatter,
                    "smooth_max_shift must be a finite value >= 0, got {bound}"
                )
            }
            Self::ShiftLimit {
                vertex,
                required,
                bound,
            } => write!(
                formatter,
                "smoothing needs to move vertex {vertex} by {required:.4}, beyond the declared bound {bound:.4}"
            ),
            Self::NonFiniteResult { vertex } => {
                write!(
                    formatter,
                    "smoothing moved vertex {vertex} to a non-finite position"
                )
            }
        }
    }
}

impl std::error::Error for DensifyError {}

/// What the optional smoothing stage measured. `boundary_max_displacement` is
/// the frozen-boundary invariant read back from the output: it is zero because
/// boundary vertices are never written, and a non-zero reading would be a
/// defect, not a tolerance. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmoothReport {
    /// Did the stage run (`smooth_iterations > 0`)?
    pub enabled: bool,
    /// Passes executed.
    pub iterations: u32,
    /// The bound in effect, as declared.
    pub max_shift: Option<f32>,
    /// Largest cumulative move of any smoothed interior vertex.
    pub max_displacement: f64,
    /// Largest cumulative move of any boundary vertex; the frozen-hem
    /// invariant, measured.
    pub boundary_max_displacement: f64,
    /// Vertices the stage moved.
    pub changed_vertices: usize,
    /// Triangles whose facing turned past 90 degrees under smoothing.
    pub flipped_triangles: usize,
}

/// The measured result of one densify: counts, per-part preservation and the
/// boundary/smoothing invariants. Counts are deterministic —
/// `V' = V + E` per cut — so equal before/after component and opening
/// inventories are the per-part preservation proof. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct DensifyReport {
    /// Vertices given.
    pub source_vertices: usize,
    /// Triangles given.
    pub source_triangles: usize,
    /// Subdivision rounds executed.
    pub cuts: u32,
    /// Vertices written.
    pub vertices: usize,
    /// Triangles written.
    pub triangles: usize,
    /// Components of the input index graph, before and after.
    pub components_indexed_before: usize,
    pub components_indexed_after: usize,
    /// Components after exact-position weld, before and after; seam twins
    /// that share a position stay one part.
    pub components_exact_position_before: usize,
    pub components_exact_position_after: usize,
    /// Boundary-loop openings, largest first, before and after.
    pub openings_before: Vec<Opening>,
    pub openings_after: Vec<Opening>,
    /// Boundary edges, before and after (each boundary edge subdivides into
    /// two).
    pub boundary_edges_before: usize,
    pub boundary_edges_after: usize,
    /// Boundary vertices, before and after (each boundary edge adds one
    /// midpoint).
    pub boundary_vertices_before: usize,
    pub boundary_vertices_after: usize,
    /// Exactly-coincident position groups with two or more members, before
    /// and after; authored slit and seam twins survive unchanged.
    pub coincident_groups_before: usize,
    pub coincident_groups_after: usize,
    /// Largest move of any boundary-loop vertex across the whole operation.
    /// Subdivision moves nothing and smoothing is boundary-locked, so this is
    /// the frozen-hem reading.
    pub boundary_max_drift: f64,
    /// The optional smoothing stage's report.
    pub smoothing: SmoothReport,
}

/// The densified mesh and its report: positions, triangles and measurements.
pub type Densified = (Vec<[f32; 3]>, Vec<[u32; 3]>, DensifyReport);

/// Densify a mesh: midpoint subdivision, optional boundary-locked smoothing,
/// and the measured report. The first `positions.len()` vertices of the
/// result are the input's own vertices in their input order at their input
/// positions, so a caller keeps its identity map; every new vertex is a
/// midpoint and nothing else moves unless smoothing ran.
///
/// # Errors
///
/// Refuses [`DensifyError`] before anything is produced: malformed or empty
/// geometry, a topology subdivision would change (degenerate faces,
/// non-manifold edges), every out-of-range parameter, and a smoothing stage
/// that would exceed its declared displacement bound.
pub fn densify(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    params: &DensifyParams,
) -> Result<Densified, DensifyError> {
    params.validate()?;
    validate_geometry(positions, triangles)?;
    let uses_before = edge_uses(triangles);
    if let Some((&edge, &uses)) = uses_before.iter().find(|&(_, &uses)| uses > 2) {
        return Err(DensifyError::NonManifoldEdge {
            edge,
            uses: uses as usize,
        });
    }

    let boundary_before = boundary_vertices(&uses_before);
    let report = DensifyReport {
        source_vertices: positions.len(),
        source_triangles: triangles.len(),
        cuts: params.cuts,
        vertices: 0,
        triangles: 0,
        components_indexed_before: component_groups(positions, triangles).len(),
        components_exact_position_before: exact_position_components(positions, triangles),
        openings_before: boundary_loops(positions, triangles),
        boundary_edges_before: boundary_edges(&uses_before),
        boundary_vertices_before: boundary_before.len(),
        coincident_groups_before: coincident_groups(positions),
        coincident_groups_after: 0,
        components_indexed_after: 0,
        components_exact_position_after: 0,
        openings_after: Vec::new(),
        boundary_edges_after: 0,
        boundary_vertices_after: 0,
        boundary_max_drift: 0.0,
        smoothing: SmoothReport {
            enabled: false,
            iterations: 0,
            max_shift: params.smooth_max_shift,
            max_displacement: 0.0,
            boundary_max_displacement: 0.0,
            changed_vertices: 0,
            flipped_triangles: 0,
        },
    };

    let (mut out_positions, out_triangles) = midpoint_subdivide(positions, triangles, params.cuts);
    let subdivided = out_positions.clone();
    let after_edges = edge_uses(&out_triangles);
    let boundary_after = boundary_vertices(&after_edges);

    let smoothing = if params.smooth_iterations > 0 {
        smooth_locked_boundary(
            &mut out_positions,
            &out_triangles,
            &boundary_after,
            &subdivided,
            params,
        )?
    } else {
        report.smoothing
    };

    // The frozen-hem reading is measured over the output's boundary vertices:
    // originals against their input positions, midpoint vertices against the
    // subdivided positions smoothing started from (which equal the input for
    // originals, because subdivision only appends midpoints).
    let drift = boundary_after
        .iter()
        .map(|&vertex| {
            let at = vertex as usize;
            let reference = if at < positions.len() {
                positions[at]
            } else {
                subdivided[at]
            };
            distance(reference, out_positions[at])
        })
        .fold(0.0f64, f64::max);

    let report = DensifyReport {
        vertices: out_positions.len(),
        triangles: out_triangles.len(),
        components_indexed_after: component_groups(&out_positions, &out_triangles).len(),
        components_exact_position_after: exact_position_components(&out_positions, &out_triangles),
        openings_after: boundary_loops(&out_positions, &out_triangles),
        boundary_edges_after: boundary_edges(&after_edges),
        boundary_vertices_after: boundary_after.len(),
        coincident_groups_after: coincident_groups(&out_positions),
        boundary_max_drift: drift,
        smoothing,
        ..report
    };
    Ok((out_positions, out_triangles, report))
}

/// The shared geometry refusals, in this module's own names.
fn validate_geometry(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<(), DensifyError> {
    if positions.is_empty() {
        return Err(DensifyError::EmptyMesh);
    }
    if triangles.is_empty() {
        return Err(DensifyError::EmptyFaces);
    }
    if let Some(vertex) = first_non_finite(positions) {
        return Err(DensifyError::NonFinitePosition { vertex });
    }
    for (face, &[a, b, c]) in triangles.iter().enumerate() {
        for vertex in [a, b, c] {
            if vertex as usize >= positions.len() {
                return Err(DensifyError::FaceIndex { face, vertex });
            }
        }
        if a == b || b == c || c == a {
            return Err(DensifyError::DegenerateFace { face });
        }
    }
    Ok(())
}

/// Undirected edge → face-use count.
fn edge_uses(triangles: &[[u32; 3]]) -> BTreeMap<(u32, u32), u32> {
    let mut uses = BTreeMap::new();
    for &[a, b, c] in triangles {
        for (left, right) in [(a, b), (b, c), (c, a)] {
            *uses.entry((left.min(right), left.max(right))).or_default() += 1;
        }
    }
    uses
}

/// Vertices on an edge used by exactly one face.
fn boundary_vertices(edge_uses: &BTreeMap<(u32, u32), u32>) -> BTreeSet<u32> {
    let mut vertices = BTreeSet::new();
    for (&(a, b), &uses) in edge_uses {
        if uses == 1 {
            vertices.insert(a);
            vertices.insert(b);
        }
    }
    vertices
}

/// Edges used by exactly one face.
fn boundary_edges(edge_uses: &BTreeMap<(u32, u32), u32>) -> usize {
    edge_uses.values().filter(|&&uses| uses == 1).count()
}

/// Components of the exact-position (welded) graph: seam twins that share a
/// position are one part.
fn exact_position_components(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> usize {
    let topology = position_topology(positions, triangles);
    component_groups(&topology.positions, &topology.triangles).len()
}

/// Exactly-coincident position groups with two or more members.
fn coincident_groups(positions: &[[f32; 3]]) -> usize {
    let mut by_key: HashMap<[u32; 3], usize> = HashMap::new();
    for position in positions {
        *by_key.entry(position_key(*position)).or_default() += 1;
    }
    by_key.values().filter(|&&size| size > 1).count()
}

/// Uniform Laplacian passes over the interior only: boundary vertices are
/// frozen, and a coincident group touching a frozen vertex is frozen whole so
/// an authored slit cannot open. The cumulative displacement bound refuses
/// instead of clipping. @req DECL0013
fn smooth_locked_boundary(
    positions: &mut Vec<[f32; 3]>,
    triangles: &[[u32; 3]],
    frozen: &BTreeSet<u32>,
    before_smoothing: &[[f32; 3]],
    params: &DensifyParams,
) -> Result<SmoothReport, DensifyError> {
    let mut adjacency: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for &[a, b, c] in triangles {
        for (left, right) in [(a, b), (b, c), (c, a)] {
            adjacency.entry(left).or_default().insert(right);
            adjacency.entry(right).or_default().insert(left);
        }
    }
    // Authored coincident copies are one surface point; smoothing them
    // independently would open slits. A group with a frozen member stays
    // frozen whole, so the boundary lock wins over the coincidence lock.
    let groups: Vec<Vec<usize>> = {
        let mut by_key: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
        for (vertex, position) in positions.iter().enumerate() {
            by_key
                .entry(position_key(*position))
                .or_default()
                .push(vertex);
        }
        by_key.into_values().collect()
    };

    let mut current = positions.clone();
    for _ in 0..params.smooth_iterations {
        let mut next = current.clone();
        for (&vertex, neighbours) in &adjacency {
            if frozen.contains(&vertex) {
                continue;
            }
            let count = neighbours.len().max(1);
            let mut mean = [0.0f32; 3];
            for neighbour in neighbours {
                for axis in 0..3 {
                    mean[axis] += current[*neighbour as usize][axis];
                }
            }
            let at = vertex as usize;
            for axis in 0..3 {
                next[at][axis] = current[at][axis]
                    + SMOOTH_LAMBDA * (mean[axis] / count as f32 - current[at][axis]);
            }
        }
        for group in &groups {
            if group
                .iter()
                .any(|&vertex| frozen.contains(&(vertex as u32)))
            {
                continue;
            }
            let mean = group
                .iter()
                .fold([0.0f32; 3], |sum, &vertex| {
                    let position = next[vertex];
                    [
                        sum[0] + position[0],
                        sum[1] + position[1],
                        sum[2] + position[2],
                    ]
                })
                .map(|lane| lane / group.len() as f32);
            for &vertex in group {
                next[vertex] = mean;
            }
        }
        current = next;
    }

    let mut report = SmoothReport {
        enabled: true,
        iterations: params.smooth_iterations,
        max_shift: params.smooth_max_shift,
        max_displacement: 0.0,
        boundary_max_displacement: 0.0,
        changed_vertices: 0,
        flipped_triangles: 0,
    };
    for (vertex, (before, after)) in positions.iter().zip(&current).enumerate() {
        let shift = distance(*before, *after);
        if shift > 0.0 {
            report.changed_vertices += 1;
        }
        if frozen.contains(&(vertex as u32)) {
            report.boundary_max_displacement = report.boundary_max_displacement.max(shift);
        } else {
            report.max_displacement = report.max_displacement.max(shift);
        }
        if let Some(bound) = params.smooth_max_shift
            && shift > f64::from(bound)
        {
            return Err(DensifyError::ShiftLimit {
                vertex,
                required: shift,
                bound: f64::from(bound),
            });
        }
        if !after.iter().all(|lane| lane.is_finite()) {
            return Err(DensifyError::NonFiniteResult { vertex });
        }
    }
    report.flipped_triangles = flipped_triangles(before_smoothing, &current, triangles);
    *positions = current;
    Ok(report)
}

/// Triangles whose facing turned past 90 degrees, the fit's own flip reading.
fn flipped_triangles(before: &[[f32; 3]], after: &[[f32; 3]], triangles: &[[u32; 3]]) -> usize {
    triangles
        .iter()
        .filter(|face| {
            let corners = |positions: &[[f32; 3]]| {
                [
                    positions[face[0] as usize],
                    positions[face[1] as usize],
                    positions[face[2] as usize],
                ]
            };
            let was = unit_triangle_normal(corners(before));
            let now = unit_triangle_normal(corners(after));
            was.zip(now).is_some_and(|(was, now)| {
                let turn = was.iter().zip(now).map(|(a, b)| a * b).sum::<f64>();
                turn < 0.0
            })
        })
        .count()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(b)).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3×3 unit grid: eight triangles, one square boundary loop of eight
    /// edges, one interior vertex.
    fn grid() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let positions: Vec<[f32; 3]> = (0..3)
            .flat_map(|row| (0..3).map(move |column| [column as f32, row as f32, 0.0]))
            .collect();
        let quad = |row: u32, column: u32| {
            let a = row * 3 + column;
            [[a, a + 1, a + 3], [a + 1, a + 4, a + 3]]
        };
        let triangles: Vec<[u32; 3]> = (0..2u32)
            .flat_map(|row| (0..2u32).flat_map(move |column| quad(row, column)))
            .collect();
        (positions, triangles)
    }

    #[test]
    fn pure_densify_is_neutral_and_preserves_parts() {
        let (positions, triangles) = grid();
        // A second, separate triangle: the per-part counter must follow it.
        let mut all_positions = positions.clone();
        let base = all_positions.len() as u32;
        all_positions.push([10.0, 0.0, 0.0]);
        all_positions.push([11.0, 0.0, 0.0]);
        all_positions.push([10.0, 1.0, 0.0]);
        let mut all_triangles = triangles.clone();
        all_triangles.push([base, base + 1, base + 2]);

        let (out_positions, out_triangles, report) =
            densify(&all_positions, &all_triangles, &DensifyParams::default()).expect("densify");
        // Counts are deterministic: V' = V + E, F' = 4F.
        let edge_count = edge_uses(&all_triangles).len();
        assert_eq!(out_positions.len(), all_positions.len() + edge_count);
        assert_eq!(out_triangles.len(), all_triangles.len() * 4);
        assert_eq!(report.vertices, out_positions.len());
        assert_eq!(report.triangles, out_triangles.len());
        // The first source vertices keep their input positions exactly.
        assert_eq!(&out_positions[..all_positions.len()], &all_positions[..]);
        // Per-part preservation on both graphs.
        assert_eq!(report.components_indexed_before, 2);
        assert_eq!(report.components_indexed_after, 2);
        assert_eq!(report.components_exact_position_before, 2);
        assert_eq!(report.components_exact_position_after, 2);
        // Boundaries double, nothing drifts, no smoothing ran.
        assert_eq!(
            report.boundary_edges_after,
            report.boundary_edges_before * 2
        );
        assert_eq!(report.boundary_max_drift, 0.0);
        assert!(!report.smoothing.enabled);
    }

    #[test]
    fn openings_keep_their_loop_inventory() {
        let (positions, triangles) = grid();
        let (_, _, report) =
            densify(&positions, &triangles, &DensifyParams::default()).expect("densify");
        assert_eq!(report.openings_before.len(), 1);
        assert_eq!(report.openings_after.len(), 1);
        assert_eq!(report.openings_before[0].edges, 8);
        assert_eq!(report.openings_after[0].edges, 16);
        assert_eq!(report.boundary_vertices_before, 8);
        assert_eq!(report.boundary_vertices_after, 16);
        // The loop's midpoint vertices land on the loop: the centers agree to
        // the input's own float resolution.
        let was = report.openings_before[0].center;
        let now = report.openings_after[0].center;
        assert!(distance(was, now) < 1e-5, "{was:?} vs {now:?}");
    }

    #[test]
    fn smoothing_softens_the_interior_and_never_moves_a_boundary() {
        let (mut positions, triangles) = grid();
        // The symmetric grid center already sits at its neighbours' mean, so
        // the Laplacian would not move it; pull it off the centroid first.
        positions[4] = [0.9, 1.2, 0.3];
        let params = DensifyParams {
            cuts: 0,
            smooth_iterations: 5,
            smooth_max_shift: None,
        };
        let (out_positions, _, report) = densify(&positions, &triangles, &params).expect("densify");
        assert!(report.smoothing.enabled);
        assert_eq!(report.smoothing.iterations, 5);
        assert!(report.smoothing.max_displacement > 0.0);
        assert_eq!(report.smoothing.boundary_max_displacement, 0.0);
        assert_eq!(report.boundary_max_drift, 0.0);
        // The named interior vertex moved; a named boundary vertex did not.
        assert!(distance(positions[4], out_positions[4]) > 0.0);
        assert_eq!(positions[0], out_positions[0]);
        assert_eq!(positions[1], out_positions[1]);
    }

    #[test]
    fn coincident_twins_survive_as_one_part_and_stay_coincident() {
        // Two triangles at the same place, authored as separate index ranges:
        // two index components, one exact-position component. Every vertex is
        // a twin pair, and each pair's subdivided midpoints coincide again,
        // so 3 twin groups become 6.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let triangles = vec![[0, 1, 2], [3, 4, 5]];
        let (out_positions, _, report) =
            densify(&positions, &triangles, &DensifyParams::default()).expect("densify");
        assert_eq!(report.components_indexed_before, 2);
        assert_eq!(report.components_indexed_after, 2);
        assert_eq!(report.components_exact_position_before, 1);
        assert_eq!(report.components_exact_position_after, 1);
        assert_eq!(report.coincident_groups_before, 3);
        assert_eq!(report.coincident_groups_after, 6);
        // The twins subdivided into coincident copies again.
        for (twin, copy) in [(0, 3), (1, 4), (2, 5)] {
            assert_eq!(out_positions[twin], out_positions[copy]);
        }
    }

    #[test]
    fn two_cuts_follow_the_deterministic_counts() {
        let (positions, triangles) = grid();
        let params = DensifyParams {
            cuts: 2,
            smooth_iterations: 0,
            smooth_max_shift: None,
        };
        let (_, _, report) = densify(&positions, &triangles, &params).expect("densify");
        let edges0 = edge_uses(&triangles).len();
        // One cut: V1 = V + E, F1 = 4F, E1 = 2E + 3F.
        let vertices1 = positions.len() + edges0;
        let triangles1 = triangles.len() * 4;
        let edges1 = 2 * edges0 + 3 * triangles.len();
        // Two cuts repeat the same rule on the cut mesh.
        assert_eq!(report.vertices, vertices1 + edges1);
        assert_eq!(report.triangles, triangles1 * 4);
    }

    #[test]
    fn every_out_of_range_parameter_is_typed() {
        let (positions, triangles) = grid();
        let cases = [
            (
                DensifyError::Cuts(3),
                DensifyParams {
                    cuts: 3,
                    smooth_iterations: 0,
                    smooth_max_shift: None,
                },
            ),
            (
                DensifyError::SmoothIterations(21),
                DensifyParams {
                    cuts: 1,
                    smooth_iterations: 21,
                    smooth_max_shift: None,
                },
            ),
            (
                DensifyError::SmoothMaxShift(-1.0),
                DensifyParams {
                    cuts: 1,
                    smooth_iterations: 1,
                    smooth_max_shift: Some(-1.0),
                },
            ),
        ];
        for (expected, params) in cases {
            assert_eq!(densify(&positions, &triangles, &params), Err(expected));
        }
    }

    #[test]
    fn topology_that_subdivision_would_change_is_refused() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        // A repeated corner collapses onto its apex under midpoint subdivision.
        let degenerate = vec![[0, 0, 1]];
        assert_eq!(
            densify(&positions, &degenerate, &DensifyParams::default()),
            Err(DensifyError::DegenerateFace { face: 0 })
        );
        // Three faces sharing one edge: the boundary reading is ambiguous.
        let mut non_manifold_positions = positions.clone();
        non_manifold_positions.push([0.0, 0.5, 0.0]);
        let non_manifold = vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]];
        assert_eq!(
            densify(
                &non_manifold_positions,
                &non_manifold,
                &DensifyParams::default()
            ),
            Err(DensifyError::NonManifoldEdge {
                edge: (0, 1),
                uses: 3
            })
        );
        // A face corner the mesh does not have.
        let out_of_range = vec![[0, 1, 9]];
        assert_eq!(
            densify(&positions, &out_of_range, &DensifyParams::default()),
            Err(DensifyError::FaceIndex { face: 0, vertex: 9 })
        );
        assert_eq!(
            densify(&[], &[], &DensifyParams::default()),
            Err(DensifyError::EmptyMesh)
        );
        let vertices_only = vec![[0.0, 0.0, 0.0]];
        assert_eq!(
            densify(&vertices_only, &[], &DensifyParams::default()),
            Err(DensifyError::EmptyFaces)
        );
    }

    #[test]
    fn a_shift_bound_refuses_instead_of_clipping() {
        let (mut positions, triangles) = grid();
        // The symmetric center would not move; pull it off the centroid so
        // smoothing has work the bound can refuse.
        positions[4] = [0.9, 1.2, 0.3];
        let params = DensifyParams {
            cuts: 0,
            smooth_iterations: 1,
            smooth_max_shift: Some(1e-6),
        };
        assert!(matches!(
            densify(&positions, &triangles, &params),
            Err(DensifyError::ShiftLimit { .. })
        ));
        // A generous bound measures and passes.
        let params = DensifyParams {
            cuts: 0,
            smooth_iterations: 1,
            smooth_max_shift: Some(1.0),
        };
        let (_, _, report) = densify(&positions, &triangles, &params).expect("densify");
        assert!(report.smoothing.max_displacement <= 1.0);
        assert!(report.smoothing.changed_vertices > 0);
    }
}
