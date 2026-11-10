//! Post-fit clipping repair against a body surface (`fix_clipping`).
//!
//! Ported from BodySlide and Outfit Studio's `ClippingFixer::FixClipping`
//! and `SignedDistancesToBody` (GPL-3.0, `src/components/ClippingFixer.cpp`
//! at 58dfbd30). One invocation inflates the body surface outward by a
//! strength-scaled distance, pushes garment vertices, triangle interiors and
//! long-edge midpoints that sit inside the influence band out along the
//! interpolated body normal, optionally pushes garment triangles the body
//! pokes through between vertices, propagates the pushes over the garment
//! connectivity so thin structures move together, smooths them, and unifies
//! co-located vertex copies. This is the strict stage after the soft
//! [`crate::wrap`] fit; it moves vertices toward clearance and does not
//! guarantee a clear mesh by itself — bounded retries and quality guards
//! belong to the caller, like the source's `auto_fix_clipping`.
//! @req DECL0013
//!
//! Divergences from the source, kept deliberate: empty or malformed input is
//! rejected ([`ClipFixError`]) where the source silently returns; `strength`
//! outside `0..=1` is rejected instead of clamped (`0` stays the source's
//! no-op); the optional lock mask is the source caller's own per-vertex fix
//! weight (`ProjectOperations.cpp:937-966`: none where masked, all where
//! free, a blend on a soft border) applied after the mask-free primitive, so
//! fully locked vertices keep their positions bit-identically; co-located
//! copies are grouped by the shared spatial-hash welder instead of the
//! source's x-sorted per-axis scan (exact duplicates group identically); the
//! body-poke query and far-side cull go through [`crate::wrap::BodySurface`]
//! shared with the fit; and the output is checked finite where the source
//! would carry NaNs silently. Body normals are the source's area-weighted
//! set with one global majority sign; closest points, rays and barycentrics
//! come from the shared `check`/`surface_util` owners.

use std::collections::BTreeSet;
use std::fmt;

use crate::check::SurfaceIndex;
use crate::mask::VertexMask;
use crate::surface_util::{
    OpenEdges, add, barycentric_clamped, coincident_groups, dot, face_normal_of, first_non_finite,
    length, outward_vertex_normals, past_open_edge, scale, sub, unit, vertex_normals,
};
use crate::wrap::{BodySurface, FitError, Mesh, validate_geometry};

/// Native `IsZero(true)`: each component within this of zero
/// (`Object3d.hpp:16`).
const NEARLY_ZERO: f32 = 1e-4;
/// Share kept when adopting a neighbor's push (`FixClipping`, decay).
const PROPAGATION_DECAY: f32 = 0.85;
/// Inflation distance at zero/one strength (`FixClipping:437`).
const INFLATION_BASE: f32 = 0.05;
const INFLATION_SPAN: f32 = 0.25;
/// Closest-body search radius at zero/one strength (`FixClipping:456`).
const SEARCH_BASE: f32 = 2.0;
const SEARCH_SPAN: f32 = 3.0;
/// Push depth cap at zero/one strength for body pokes (`FixClipping:642`).
const POKE_PUSH_BASE: f32 = 0.1;
const POKE_PUSH_SPAN: f32 = 0.5;

/// Options of one clipping repair, mirroring the native `ClippingFixOptions`
/// (`ClippingFixer.h:13-17`). One invocation only: the source wraps retries
/// and quality guards in its caller (`auto_fix_clipping`), not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFixOptions {
    /// `0..=1`: scales the inflation distance, search radius, propagation
    /// rings and poke push cap.
    pub strength: f32,
    /// Also push garment triangles the body pokes out through between their
    /// vertices, which happens where large triangles span a body curve while
    /// every vertex stays outside it.
    pub fix_poke_through: bool,
}

impl Default for ClipFixOptions {
    fn default() -> Self {
        Self {
            strength: 0.5,
            fix_poke_through: false,
        }
    }
}

/// What one repair actually changed, measured on the applied result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipFixReport {
    /// Vertices whose final displacement is not near zero.
    pub moved: usize,
    /// Largest applied single displacement.
    pub max_push: f32,
}

/// Why a clipping repair or signed-distance query was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClipFixError {
    /// The garment or body failed the shared fit geometry validation.
    Geometry(FitError),
    /// The lock mask is not one value per garment vertex.
    LockLength { expected: usize, actual: usize },
    /// `strength` is outside `0..=1` or not finite; the source clamps it.
    Strength(f32),
    /// The search radius is not a positive finite length.
    SearchRadius(f32),
    /// A measured point is not finite.
    NonFinitePoint { point: usize },
    /// The repair produced a non-finite position; measured, not assumed.
    NonFiniteOutput { vertex: usize },
}

impl fmt::Display for ClipFixError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Geometry(error) => write!(formatter, "invalid mesh geometry: {error}"),
            Self::LockLength { expected, actual } => write!(
                formatter,
                "the lock mask has {actual} values for {expected} garment vertices"
            ),
            Self::Strength(value) => write!(
                formatter,
                "the strength {value} is outside 0..=1 or not finite; the source would clamp it"
            ),
            Self::SearchRadius(value) => write!(
                formatter,
                "the search radius {value} is not a positive finite length"
            ),
            Self::NonFinitePoint { point } => {
                write!(formatter, "point {point} is not finite")
            }
            Self::NonFiniteOutput { vertex } => {
                write!(
                    formatter,
                    "the repair moved vertex {vertex} to a non-finite position"
                )
            }
        }
    }
}

impl std::error::Error for ClipFixError {}

impl ClipFixOptions {
    fn validate(&self) -> Result<(), ClipFixError> {
        if !self.strength.is_finite() || !(0.0..=1.0).contains(&self.strength) {
            return Err(ClipFixError::Strength(self.strength));
        }
        Ok(())
    }

    /// Inflation distance, margin and total influence band (`FixClipping:437-441`).
    fn bands(&self) -> (f32, f32, f32) {
        let inflation = INFLATION_BASE + self.strength * INFLATION_SPAN;
        let margin = inflation * 0.5;
        (inflation, margin, inflation + margin)
    }
}

/// Repair one garment against a body: return the moved positions plus what
/// changed. The garment topology is untouched and the input arrays are not
/// modified. @req DECL0013
///
/// This moves clipping regions toward the body's inflated surface; whether
/// the result is clear is a measurement ([`signed_distances_to_body`]), not a
/// property this promises.
///
/// # Errors
///
/// Refuses [`ClipFixError::Geometry`] through the shared fit validator, a
/// mismatched lock mask, an out-of-range strength and any non-finite output,
/// before or after the repair.
pub fn fix_clipping(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &ClipFixOptions,
) -> Result<(Vec<[f32; 3]>, ClipFixReport), ClipFixError> {
    options.validate()?;
    validate_geometry(garment, body).map_err(ClipFixError::Geometry)?;
    if let Some(mask) = lock
        && mask.len() != garment.0.len()
    {
        return Err(ClipFixError::LockLength {
            expected: garment.0.len(),
            actual: mask.len(),
        });
    }

    // The source's zero-strength no-op (`FixClipping:432-434`); emptiness is
    // already refused above, where the source silently returned.
    if options.strength <= 0.0 {
        return Ok((
            garment.0.to_vec(),
            ClipFixReport {
                moved: 0,
                max_push: 0.0,
            },
        ));
    }

    let (inflation, margin, total_influence) = options.bands();
    let search_radius = SEARCH_BASE + options.strength * SEARCH_SPAN;
    let count = garment.0.len();

    // The body as the repair measures it: one global-sign normal set and one
    // shared closest-point index (`FixClipping:444-454`).
    let (body_normals, body_sign) = outward_vertex_normals(body.0, body.1);
    let body_index = SurfaceIndex::build(body.0, body.1);

    // Outfit normals turned away from the body's centroid
    // (`FixClipping:465-468`).
    let body_centroid = scale(
        body.0
            .iter()
            .fold([0.0f32; 3], |sum, &position| add(sum, position)),
        1.0 / body.0.len() as f32,
    );
    let outfit_sign = normal_sign_from_point(garment.0, garment.1, body_centroid);
    let outfit_normals: Vec<[f32; 3]> = vertex_normals(garment.0, garment.1)
        .into_iter()
        .map(|normal| scale(normal, outfit_sign))
        .collect();

    // Garment connectivity and open boundary (`BuildTopology`,
    // `FixClipping:462-463`).
    let adjacency = adjacency_lists(garment);
    let open_edges = OpenEdges::build(garment.1, count);
    let boundary: Vec<bool> = (0..count)
        .map(|vertex| open_edges.vertex_is_open(vertex as u32))
        .collect();

    let field = ClippingField {
        body,
        body_index: &body_index,
        body_normals: &body_normals,
        body_sign,
        inflation,
        margin,
        total_influence,
        search_radius,
    };

    let mut displacements = vec![[0.0f32; 3]; count];
    let mut eligible = vec![false; count];
    // Native `mergeDisplacement` (`FixClipping:536-543`): keep the longer
    // push; any nonzero contribution marks the vertex eligible.
    let merge = |vertex: usize,
                 displacement: [f32; 3],
                 can_propagate: bool,
                 displacements: &mut [[f32; 3]],
                 eligible: &mut [bool]| {
        if can_propagate {
            eligible[vertex] = true;
        }
        if nearly_zero(displacement) {
            return;
        }
        if dot(displacement, displacement) > dot(displacements[vertex], displacements[vertex]) {
            displacements[vertex] = displacement;
        }
        eligible[vertex] = true;
    };

    // Every garment vertex first (`FixClipping:545-558`).
    for vertex in 0..count {
        let normal = (!nearly_zero(outfit_normals[vertex])).then_some(outfit_normals[vertex]);
        let (displacement, can_propagate) = field.sample(garment.0[vertex], normal);
        eligible[vertex] = can_propagate || !nearly_zero(displacement);
        displacements[vertex] = displacement;
    }

    // Triangle interiors and long edges cover sparse low-poly faces
    // (`FixClipping:560-636`).
    let long_edge_threshold_sq = (total_influence * 1.5).powi(2);
    for face in garment.1 {
        let corners = [
            garment.0[face[0] as usize],
            garment.0[face[1] as usize],
            garment.0[face[2] as usize],
        ];
        let face_normal = unit(scale(face_normal_of(corners), outfit_sign));
        let sample_normal = (!nearly_zero(face_normal)).then_some(face_normal);
        let sample = |point: [f32; 3],
                      owners: &[u32],
                      displacements: &mut [[f32; 3]],
                      eligible: &mut [bool]| {
            let (displacement, can_propagate) = field.sample(point, sample_normal);
            for &vertex in owners {
                merge(
                    vertex as usize,
                    displacement,
                    can_propagate,
                    displacements,
                    eligible,
                );
            }
        };
        let centroid = scale(add(add(corners[0], corners[1]), corners[2]), 1.0 / 3.0);
        sample(centroid, face, &mut displacements, &mut eligible);
        let edge_lengths_sq = [
            length_sq(sub(corners[1], corners[0])),
            length_sq(sub(corners[2], corners[1])),
            length_sq(sub(corners[0], corners[2])),
        ];
        if edge_lengths_sq.iter().copied().fold(0.0f32, f32::max) > long_edge_threshold_sq {
            let midpoints = [
                ([face[0], face[1]], scale(add(corners[0], corners[1]), 0.5)),
                ([face[1], face[2]], scale(add(corners[1], corners[2]), 0.5)),
                ([face[2], face[0]], scale(add(corners[2], corners[0]), 0.5)),
            ];
            for (owners, point) in midpoints {
                sample(point, &owners, &mut displacements, &mut eligible);
            }
        }
    }

    // The body poking out between garment vertices (`FixClipping:638-652`).
    if options.fix_poke_through {
        let surface = BodySurface::build(body.0, body.1);
        let max_push = POKE_PUSH_BASE + options.strength * POKE_PUSH_SPAN;
        for (vertex, hit, depth) in surface.poke_hits(garment, &body_normals, total_influence + 1.0)
        {
            let displacement = scale(body_normals[vertex], depth.min(max_push) + inflation);
            for corner in garment.1[hit.triangle] {
                merge(
                    corner as usize,
                    displacement,
                    false,
                    &mut displacements,
                    &mut eligible,
                );
            }
        }
    }

    // Pushes carry through the connectivity so straps keep their thickness
    // (`FixClipping:654-693`).
    let rings = 4 + (options.strength * 4.0).round() as usize;
    for _ in 0..rings {
        let mut next = displacements.clone();
        let mut next_eligible = eligible.clone();
        for vertex in 0..count {
            if boundary[vertex] {
                continue;
            }
            let mut best = [0.0f32; 3];
            let mut best_length = 0.0f32;
            for &neighbour in &adjacency[vertex] {
                let neighbour_length = length(displacements[neighbour]);
                if neighbour_length > best_length {
                    best_length = neighbour_length;
                    best = displacements[neighbour];
                }
            }
            if best_length * PROPAGATION_DECAY > length(displacements[vertex]) + 0.001 {
                next[vertex] = scale(best, PROPAGATION_DECAY);
                next_eligible[vertex] = true;
            }
        }
        displacements = next;
        eligible = next_eligible;
    }

    // Two smoothing passes over eligible vertices (`FixClipping:695-730`).
    for _ in 0..2 {
        let mut next = displacements.clone();
        for vertex in 0..count {
            if boundary[vertex] || !eligible[vertex] {
                continue;
            }
            let mut sum = displacements[vertex];
            let mut weight = 1.0f32;
            for &neighbour in &adjacency[vertex] {
                if !eligible[neighbour] || nearly_zero(displacements[neighbour]) {
                    continue;
                }
                sum = add(sum, displacements[neighbour]);
                weight += 1.0;
            }
            if weight <= 1.0 {
                continue;
            }
            let average = scale(sum, 1.0 / weight);
            let mut blended = add(scale(displacements[vertex], 0.65), scale(average, 0.35));
            let current = length(displacements[vertex]);
            let blended_length = length(blended);
            if current > 0.001 && blended_length < current * 0.8 {
                blended = if blended_length > 1e-6 {
                    scale(blended, current * 0.8 / blended_length)
                } else {
                    scale(displacements[vertex], 0.8)
                };
            }
            next[vertex] = blended;
        }
        displacements = next;
    }

    unify_coincident_copies(garment.0, &mut displacements);

    // The source caller's per-vertex fix weight: none where masked, all
    // where free, a blend on a soft border (`ProjectOperations.cpp:964-966`).
    let mut moved = 0usize;
    let mut max_push = 0.0f32;
    let mut positions = Vec::with_capacity(count);
    for (vertex, (&origin, &push)) in garment.0.iter().zip(&displacements).enumerate() {
        let freedom = lock.map_or(1.0, |mask| 1.0 - mask.as_slice()[vertex]);
        let displacement = scale(push, freedom);
        let position = add(origin, displacement);
        if !nearly_zero(displacement) {
            moved += 1;
        }
        max_push = max_push.max(length(displacement));
        positions.push(position);
    }
    if let Some(vertex) = first_non_finite(&positions) {
        return Err(ClipFixError::NonFiniteOutput { vertex });
    }
    Ok((positions, ClipFixReport { moved, max_push }))
}

/// Signed distance of each point from the body surface along the body's
/// outward normal: negative inside the body, positive outside; `None` where
/// no body surface lies within `search_radius` (`SignedDistancesToBody`,
/// `ClippingFixer.cpp:235-320`). A point whose closest surface point lies
/// past an open edge of the body is past the opening, not inside, and reads
/// its unsigned distance. @req DECL0013
///
/// # Errors
///
/// Refuses invalid body geometry, a non-finite point and a non-positive
/// radius before any query.
pub fn signed_distances_to_body(
    body: Mesh<'_>,
    points: &[[f32; 3]],
    search_radius: f32,
) -> Result<Vec<Option<f32>>, ClipFixError> {
    if !search_radius.is_finite() || search_radius <= 0.0 {
        return Err(ClipFixError::SearchRadius(search_radius));
    }
    validate_geometry(body, body).map_err(|error| {
        let error = match error {
            FitError::EmptyGarment => FitError::EmptyBody,
            FitError::NonFiniteGarmentPosition { vertex } => {
                FitError::NonFiniteBodyPosition { vertex }
            }
            FitError::GarmentFaceIndex { triangle, vertex } => {
                FitError::BodyFaceIndex { triangle, vertex }
            }
            error => error,
        };
        ClipFixError::Geometry(error)
    })?;
    if let Some(point) = first_non_finite(points) {
        return Err(ClipFixError::NonFinitePoint { point });
    }

    let (normals, sign) = outward_vertex_normals(body.0, body.1);
    let index = SurfaceIndex::build(body.0, body.1);
    let open_edges = OpenEdges::build(body.1, body.0.len());
    Ok(points
        .iter()
        .map(|&point| {
            let nearest = index.closest(point)?;
            if nearest.distance > search_radius {
                return None;
            }
            let face = body.1[nearest.triangle];
            let corners = [
                body.0[face[0] as usize],
                body.0[face[1] as usize],
                body.0[face[2] as usize],
            ];
            let bary = barycentric_clamped(nearest.point, corners);
            let outward = unit(add(
                add(
                    scale(normals[face[0] as usize], bary[0]),
                    scale(normals[face[1] as usize], bary[1]),
                ),
                scale(normals[face[2] as usize], bary[2]),
            ));
            let outward = if nearly_zero(outward) {
                unit(scale(face_normal_of(corners), sign))
            } else {
                outward
            };
            let offset = sub(point, nearest.point);
            Some(if past_open_edge(&open_edges, face, bary) {
                length(offset)
            } else {
                dot(offset, outward)
            })
        })
        .collect())
}

/// The body side of one repair: cached normal set and closest-point index.
struct ClippingField<'a> {
    body: Mesh<'a>,
    body_index: &'a SurfaceIndex,
    body_normals: &'a [[f32; 3]],
    body_sign: f32,
    inflation: f32,
    margin: f32,
    total_influence: f32,
    search_radius: f32,
}

impl ClippingField<'_> {
    /// Native `calcDisplacement` (`FixClipping:470-534`): the push of one
    /// sample toward the inflated body surface, and whether pushes may
    /// propagate from it.
    fn sample(&self, origin: [f32; 3], sample_normal: Option<[f32; 3]>) -> ([f32; 3], bool) {
        let zero = [0.0f32; 3];
        let Some(nearest) = self.body_index.closest(origin) else {
            return (zero, false);
        };
        if nearest.distance > self.search_radius {
            return (zero, false);
        }
        let face = self.body.1[nearest.triangle];
        let corners = [
            self.body.0[face[0] as usize],
            self.body.0[face[1] as usize],
            self.body.0[face[2] as usize],
        ];
        let bary = barycentric_clamped(nearest.point, corners);
        let summed = add(
            add(
                scale(self.body_normals[face[0] as usize], bary[0]),
                scale(self.body_normals[face[1] as usize], bary[1]),
            ),
            scale(self.body_normals[face[2] as usize], bary[2]),
        );
        let direction = if nearly_zero(summed) {
            scale(face_normal_of(corners), self.body_sign)
        } else {
            summed
        };
        let direction = unit(direction);

        let opposite_facing = sample_normal
            .is_some_and(|normal| !nearly_zero(normal) && dot(normal, direction) < -0.25);
        let offset = sub(origin, nearest.point);
        let signed = dot(offset, direction);
        let tangent_sq = (dot(offset, offset) - signed * signed).max(0.0);
        let propagation_influence = self.total_influence + self.inflation;
        let can_propagate = signed < propagation_influence
            && tangent_sq <= propagation_influence * propagation_influence;

        // Surfaces facing into the body, like a thick garment's lining, are
        // left to their own offset.
        let push = if !opposite_facing
            && signed < self.total_influence
            && tangent_sq <= self.total_influence * self.total_influence
        {
            if signed < self.inflation {
                self.inflation - signed
            } else {
                let share = (signed - self.inflation) / self.margin;
                let falloff = (1.0 - share) * (1.0 - share);
                falloff * self.inflation * 0.3
            }
        } else {
            0.0
        };
        (scale(direction, push), can_propagate)
    }
}

/// One global orientation vote from `point`, usually the body centroid
/// (`ComputeNormalSignFromPoint`, `FixClipping:92-105`).
fn normal_sign_from_point(positions: &[[f32; 3]], faces: &[[u32; 3]], point: [f32; 3]) -> f32 {
    let orientation: f32 = faces
        .iter()
        .map(|face| {
            let corners = [
                positions[face[0] as usize],
                positions[face[1] as usize],
                positions[face[2] as usize],
            ];
            let center = scale(add(add(corners[0], corners[1]), corners[2]), 1.0 / 3.0);
            dot(sub(center, point), face_normal_of(corners))
        })
        .sum();
    if orientation >= 0.0 { 1.0 } else { -1.0 }
}

/// Sorted vertex adjacency over the garment faces (`BuildTopology`,
/// `FixClipping:50-90`).
fn adjacency_lists(mesh: Mesh<'_>) -> Vec<Vec<usize>> {
    let mut sets = vec![BTreeSet::new(); mesh.0.len()];
    for face in mesh.1 {
        for (a, b) in [
            (face[0] as usize, face[1] as usize),
            (face[1] as usize, face[2] as usize),
            (face[2] as usize, face[0] as usize),
        ] {
            sets[a].insert(b);
            sets[b].insert(a);
        }
    }
    sets.into_iter()
        .map(|set| set.into_iter().collect())
        .collect()
}

/// Co-located copies take the group's strongest displacement
/// (`FixClipping:732-792`); the tolerance is scale-relative
/// (`0.0001 · 0.01 · scale`), grouped through the shared spatial hash.
fn unify_coincident_copies(positions: &[[f32; 3]], displacements: &mut [[f32; 3]]) {
    let scale = positions
        .iter()
        .flat_map(|position| position.iter().map(|value| value.abs()))
        .fold(0.0f32, f32::max);
    let epsilon = 0.0001 * 0.01 * scale;
    for group in coincident_groups(positions, epsilon) {
        let mut best = group[0];
        let mut best_sq = dot(displacements[best], displacements[best]);
        for &member in &group[1..] {
            let squared = dot(displacements[member], displacements[member]);
            if squared > best_sq {
                best_sq = squared;
                best = member;
            }
        }
        let strongest = displacements[best];
        for &member in &group {
            displacements[member] = strongest;
        }
    }
}

fn nearly_zero(vector: [f32; 3]) -> bool {
    vector.iter().all(|component| component.abs() < NEARLY_ZERO)
}

fn length_sq(vector: [f32; 3]) -> f32 {
    dot(vector, vector)
}
