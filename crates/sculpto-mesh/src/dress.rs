//! Dress a garment onto a body: densify it, then resolve every vertex against
//! its own nearest body point, interleaved with the topology-free
//! [`crate::wrap::fit_to_body`] registration.
//!
//! Midpoint subdivision (#[`DressParams::cuts`]) gives the contact solve the
//! resolution it needs: an authored 926-triangle bra spans body curvature with
//! triangles far larger than the clearance, so a vertex-sampled repair cannot
//! see the body poking through between them.
//!
//! The contact solve moves each vertex into a declared band — inward while it
//! floats beyond [`DressParams::clearance`], outward while it penetrates below
//! [`DressParams::contact`]. A penetrating vertex leaves along the garment's
//! own outward side when that side still agrees with the body's, because the
//! shortest way out of a bust runs down the under-bust crease: a cup fitted
//! that way never covers the breast. Body vertices that still poke through a
//! garment triangle between its corners push that triangle out, the repair
//! `clipping_fix` performs, one push per triangle per round.
//!
//! A cup with more area than the body it now covers crumples where the
//! projection compresses it, so between projections the solve slides each
//! vertex tangentially, and only where the projection strained its edges. The
//! fit is interleaved rather than run first: a garment authored on a much
//! smaller body starts with a third of its surface behind the skin, and the
//! fit's nearest-point targets then land inside the crease under a bust.
//!
//! This is a placement rule: a garment authored with its own ease over a body
//! is brought to the declared clearance, so that ease is not retained. Keep an
//! authored standoff when that is the requirement
//! ([`crate::wrap::GapMode::Keep`]); dress answers the opposite acceptance.
//!
//! [`dress_against`] dresses over a stack that is already partly clothed: the
//! obstacle becomes the [`AccumulatedSurface`] of the body plus the fitted
//! layers, graded by the outermost-member rule, so a raw outer garment resolves
//! its covered regions against the cloth it must clear and its bare regions
//! against the body. Plain [`dress`] keeps the single-body behaviour exactly.
//! @req DECL0013 DECL0015

use std::fmt;
use std::ops::Range;

use crate::accumulated_surface::AccumulatedSurface;
use crate::check::{ClosestSurfacePoint, SurfaceIndex, unit_triangle_normal};
use crate::surface_util::{
    OpenEdges, SideField, add, coincident_groups, component_labels, contact_offset, dot,
    face_normal_of, first_non_finite, length, outward_vertex_normals, remap_faces, scale, sub,
    unit, vertex_normals,
};
use crate::wrap::repair::final_push::{
    FinalPushOptions, FinalPushReport, final_push, final_push_layered,
};
use crate::wrap::{BodySurface, FitError, FitOptions, GapMode, Mesh, fit_to_body};

/// Midpoint subdivision rounds the solve may run. Two rounds take a 926-face
/// bra to 14,816 faces, which is the useful ceiling: the third round costs
/// 16× the queries and only sharpens a triangle already finer than the body's.
pub const MAX_CUTS: u32 = 2;
/// Contact iterations this port allows. Each iteration moves at most one
/// [`DressParams::max_step`], so the count only has to cover the largest
/// registration residue divided by the step.
pub const MAX_ITERATIONS: u32 = 32;
/// Body-poke repair rounds per contact pass this port allows.
pub const MAX_POKE_PASSES: u32 = 32;
/// Registration rounds this port allows. Each one costs a full fit, and two
/// already leave the fitted surface smooth on the supplied underwear.
pub const MAX_ROUNDS: u32 = 3;
/// How far inward from the declared clearance a pulled vertex is targeted, so a
/// later measurement cannot report the boundary value itself as a violation.
const PULL_MARGIN: f32 = 1e-3;
/// Ray length that counts as a body poke; the fit's own `poke_depth` default.
const POKE_SEARCH: f32 = 1.5;
/// Least cosine between a garment vertex's outward normal and the body normal
/// at its nearest point for the vertex to leave along its own normal. Below it
/// the two sides disagree too much for that direction to remove the
/// penetration, and the body normal is the only measured way out.
const MIN_PUSH_AGREEMENT: f32 = 0.2;
/// Residual at or below this ends the contact loop, in input units (1e-4 is
/// the crate's own resolution limit, `check::OCCLUSION_EPSILON`).
const CONVERGENCE: f32 = 1e-4;

/// Parameters of one dress.
///
/// `clearance` is the declared standoff: the largest floating gap a dressed
/// garment may keep, and the target of every inward move. `contact` is the
/// smallest standoff kept, the target of every outward move; it must stay
/// below `clearance`, which is what makes the two sides of the band distinct.
/// Both are in the input frame's units. @req DECL0013 DECL0015
#[derive(Debug, Clone, PartialEq)]
pub struct DressParams {
    /// Declared standoff in input units; the largest accepted floating gap.
    pub clearance: f32,
    /// Smallest standoff kept, below `clearance`.
    pub contact: f32,
    /// Midpoint subdivision rounds before the contact solve, `0..=2`.
    pub cuts: u32,
    /// Contact iterations, `1..=32`.
    pub iterations: u32,
    /// Hard per-vertex displacement bound of one iteration, in input units.
    pub max_step: f32,
    /// Weight of one tangential relaxation per iteration, `0..=1`; `0` leaves
    /// the projection unregularized.
    pub relax: f32,
    /// Registration rounds interleaved with the contact solve, `0..=3`; `0`
    /// runs the contact solve only.
    pub rounds: u32,
    /// Body-poke repair rounds per contact pass; `0` leaves pokes alone.
    ///
    /// A round ends with a contact pass, and that pull reopens part of the
    /// crossings a push just closed, so the repair converges over rounds rather
    /// than in one: the supplied 926-face bra needs about a dozen rounds on the
    /// Bijin and 3BA bodies. The loop stops as soon as a round ends with no
    /// hits, so the budget costs nothing on a garment that does not poke.
    pub poke_passes: u32,
    /// Opt-in bounded final push-out over graded visible depth. `None` runs no
    /// pass and measures nothing; accepting a visible depth is the caller's
    /// fit decision, so the library states no default.
    pub final_push: Option<FinalPushOptions>,
}

impl Default for DressParams {
    fn default() -> Self {
        Self {
            clearance: 0.2,
            contact: 0.05,
            cuts: 1,
            iterations: 16,
            max_step: 1.0,
            relax: 0.2,
            rounds: 2,
            poke_passes: 16,
            final_push: None,
        }
    }
}

/// Why a dress was refused. Every variant names its input, so a caller reads
/// the refusal and not a combined message. `@req DECL0013`
#[derive(Debug, Clone, PartialEq)]
pub enum DressError {
    /// The garment has no vertices.
    EmptyGarment,
    /// The body has no vertices.
    EmptyBody,
    /// The body has no triangles, so no clearance can be measured.
    EmptyBodyFaces,
    /// A garment vertex position is not finite.
    NonFiniteGarmentPosition { vertex: usize },
    /// A body vertex position is not finite.
    NonFiniteBodyPosition { vertex: usize },
    /// A garment triangle references a missing vertex.
    GarmentFaceIndex { triangle: usize, vertex: u32 },
    /// A body triangle references a missing vertex.
    BodyFaceIndex { triangle: usize, vertex: u32 },
    /// A position the solve produced is not finite.
    NonFiniteResult { vertex: usize },
    /// `clearance` is not positive and finite.
    Clearance(f32),
    /// `contact` is negative, not below `clearance`, or not finite.
    Contact { contact: f32, clearance: f32 },
    /// `cuts` is above [`MAX_CUTS`].
    Cuts(u32),
    /// `iterations` is zero or above [`MAX_ITERATIONS`].
    Iterations(u32),
    /// `max_step` is not positive and finite.
    MaxStep(f32),
    /// `relax` is outside `0..=1` or not finite.
    Relax(f32),
    /// `rounds` is above [`MAX_ROUNDS`].
    Rounds(u32),
    /// `poke_passes` is above [`MAX_POKE_PASSES`].
    PokePasses(u32),
    /// The registration stage refused the same geometry.
    Registration(FitError),
    /// The optional final push-out stage refused its options or geometry.
    FinalPush(FitError),
}

impl fmt::Display for DressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(formatter, "the garment has no vertices"),
            Self::EmptyBody => write!(formatter, "the body has no vertices"),
            Self::EmptyBodyFaces => write!(formatter, "the body has no triangles to dress against"),
            Self::NonFiniteGarmentPosition { vertex } => {
                write!(formatter, "garment vertex {vertex} is not finite")
            }
            Self::NonFiniteBodyPosition { vertex } => {
                write!(formatter, "body vertex {vertex} is not finite")
            }
            Self::GarmentFaceIndex { triangle, vertex } => write!(
                formatter,
                "garment triangle {triangle} references vertex {vertex}, which the garment does not have"
            ),
            Self::BodyFaceIndex { triangle, vertex } => write!(
                formatter,
                "body triangle {triangle} references vertex {vertex}, which the body does not have"
            ),
            Self::NonFiniteResult { vertex } => write!(
                formatter,
                "the dressed position of vertex {vertex} is not finite"
            ),
            Self::Clearance(value) => {
                write!(
                    formatter,
                    "the declared clearance {value} must be positive and finite"
                )
            }
            Self::Contact { contact, clearance } => write!(
                formatter,
                "the contact standoff {contact} must be non-negative, finite and below the clearance {clearance}"
            ),
            Self::Cuts(value) => {
                write!(
                    formatter,
                    "the subdivision cut count {value} is above {MAX_CUTS}"
                )
            }
            Self::Iterations(value) => write!(
                formatter,
                "the contact iteration count {value} is outside 1..={MAX_ITERATIONS}"
            ),
            Self::MaxStep(value) => {
                write!(
                    formatter,
                    "the per-vertex step bound {value} must be positive and finite"
                )
            }
            Self::Relax(value) => {
                write!(
                    formatter,
                    "the relaxation weight {value} is outside 0..=1 or not finite"
                )
            }
            Self::Rounds(value) => {
                write!(
                    formatter,
                    "the registration round count {value} is above {MAX_ROUNDS}"
                )
            }
            Self::PokePasses(value) => {
                write!(
                    formatter,
                    "the body-poke pass count {value} is above {MAX_POKE_PASSES}"
                )
            }
            Self::Registration(error) => write!(formatter, "registration refused: {error}"),
            Self::FinalPush(error) => {
                write!(formatter, "the final push-out stage refused: {error}")
            }
        }
    }
}

impl std::error::Error for DressError {}

/// What one dress changed and what it left. `@req DECL0013 DECL0015`
#[derive(Debug, Clone, PartialEq)]
pub struct DressReport {
    /// Garment vertices given.
    pub source_vertices: usize,
    /// Garment triangles given.
    pub source_triangles: usize,
    /// Midpoint subdivision rounds executed.
    pub cuts: u32,
    /// Vertices of the returned mesh (after the seam merge and subdivision).
    pub vertices: usize,
    /// Triangles of the returned mesh (after the seam merge and subdivision).
    pub triangles: usize,
    /// What the seam merge did to the authored garment.
    pub seam_weld: SeamWeldReport,
    /// Declared clearance the solve used.
    pub clearance: f32,
    /// Contact standoff the solve used.
    pub contact: f32,
    /// Contact iterations executed (a converged solve stops early).
    pub iterations: u32,
    /// The residual band was reached before the iteration budget ended.
    pub converged: bool,
    /// Vertices the contact solve moved at least once.
    pub moved: usize,
    /// Largest single displacement over the whole solve.
    pub max_move: f32,
    /// Vertices floating beyond the declared clearance before the solve.
    pub floating_before: usize,
    /// Largest floating gap before the solve, `0` when nothing floats.
    pub max_floating_gap_before: f32,
    /// Largest floating gap of the returned mesh.
    pub max_floating_gap: f32,
    /// Vertices penetrating the body before the solve.
    pub penetrating_before: usize,
    /// Deepest penetration before the solve, `0` when nothing penetrates.
    pub max_penetration_before: f32,
    /// Deepest penetration of the returned mesh.
    pub max_penetration: f32,
    /// Body vertices poking through the garment before the solve.
    pub pokes_before: usize,
    /// Body vertices poking through the returned garment.
    pub pokes_after: usize,
    /// Triangles whose normal turned by more than 90 degrees against the mesh
    /// the contact solve started from.
    pub flipped: usize,
    /// Registration report of the last registration round, absent when
    /// `rounds` is `0`.
    pub registration: Option<crate::wrap::FitReport>,
    /// Final push-out report, absent when `final_push` was off.
    pub final_push: Option<FinalPushReport>,
    /// Which accumulated member the returned vertices projected against,
    /// present only for a layered dress ([`dress_against`]).
    pub projections: Option<DressProjections>,
}

/// What a layered dress resolved its vertices against. @req DECL0013 DECL0015
#[derive(Debug, Clone, PartialEq)]
pub struct DressProjections {
    /// Provenance label, for example `accumulated[body, panty]`.
    pub surface: String,
    /// Members in dressing order with the vertices each one owns.
    pub members: Vec<DressMemberProjection>,
    /// Owning member index of every returned vertex, in vertex order.
    pub vertex_members: Vec<usize>,
}

/// One accumulated member and the returned vertices it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DressMemberProjection {
    /// `body`, or the fitted layer's declared id.
    pub id: String,
    /// The body's `0`, or the layer's declared rank.
    pub order: u32,
    /// Returned vertices whose owning member this is.
    pub vertices: usize,
}

/// One garment vertex measured against its nearest obstacle triangle.
#[derive(Debug, Clone, Copy)]
struct Sample {
    /// Signed distance along the owning member's nearest triangle normal;
    /// negative is behind that triangle, the same sign `check` grades.
    signed: f32,
    /// Unit outward normal of that triangle; `None` when it is degenerate and
    /// names no side, so the vertex has no direction to move in.
    normal: Option<[f32; 3]>,
    /// The member the vertex resolved against, in dressing order (`0` is the
    /// body): the highest-order member the vertex is behind, or the nearest
    /// one when it is behind none.
    member: usize,
}

use std::collections::{BTreeMap, BTreeSet};

/// The obstacle one dress queries across iterations: nearest point and side for
/// the contact solve, plus the poke ray the band cannot see.
///
/// The obstacle may be one surface (the body [`dress`] uses) or the members of
/// an [`AccumulatedSurface`] ([`dress_against`]); every member carries its own
/// nearest-point index over its slice of `faces`, so sampling can apply the
/// outermost-member rule instead of a bare nearest-triangle query over the
/// concatenation.
struct Obstacle<'a> {
    /// Nearest-point index per member, over that member's face slice.
    members: Vec<SurfaceIndex>,
    /// Each member's face slice in `faces`, in dressing order.
    member_faces: Vec<Range<usize>>,
    /// Each member's side field, present only when ownership is a choice: a
    /// single member cannot own or lose a vertex, and `dress` without
    /// `--against` keeps the nearest triangle's own face normal.
    sides: Option<Vec<SideField>>,
    positions: &'a [[f32; 3]],
    faces: &'a [[u32; 3]],
    /// Obstacle vertex normals and the majority orientation sign
    /// (`surface_util::outward_vertex_normals`); the poke ray runs inward.
    normals: Vec<[f32; 3]>,
    surface: BodySurface<'a>,
}

impl<'a> Obstacle<'a> {
    fn build(positions: &'a [[f32; 3]], faces: &'a [[u32; 3]]) -> Self {
        let (normals, _) = outward_vertex_normals(positions, faces);
        Self {
            members: vec![SurfaceIndex::build(positions, faces)],
            member_faces: std::iter::once(0..faces.len()).collect(),
            sides: None,
            positions,
            faces,
            normals,
            surface: BodySurface::build(positions, faces),
        }
    }

    /// The accumulated surface as one obstacle: one member index per declared
    /// layer, over that layer's slice of the composite triangle list.
    fn build_layered(accumulated: &'a AccumulatedSurface) -> Self {
        let (positions, faces) = accumulated.obstacle();
        let member_faces: Vec<Range<usize>> = accumulated
            .layers()
            .iter()
            .map(|layer| layer.triangle_range.clone())
            .collect();
        let members = member_faces
            .iter()
            .map(|range| SurfaceIndex::build(positions, &faces[range.clone()]))
            .collect();
        let sides = member_faces
            .iter()
            .map(|range| SideField::build(positions, &faces[range.clone()]))
            .collect();
        let (normals, _) = outward_vertex_normals(positions, faces);
        Self {
            members,
            member_faces,
            sides: Some(sides),
            positions,
            faces,
            normals,
            surface: BodySurface::build(positions, faces),
        }
    }

    /// The nearest point of one member and the sample it names for `point`.
    ///
    /// A layered obstacle resolves the member's *side* through its
    /// [`SideField`] and applies the coverage rule: a tie at a hem corner names
    /// one deterministic side, and a member is the vertex's contact surface only
    /// within [`crate::surface_util::COVERAGE_MIN_COSINE`] of its normal axis, so
    /// a vertex that only grazes a distant surface is pushed by the member below
    /// instead of along a far horizon. `None` is that refusal. A single-member
    /// obstacle keeps the nearest triangle's own face normal, the measurement
    /// `dress` has always taken.
    fn member_sample(
        &self,
        member: usize,
        closest: &ClosestSurfacePoint,
        point: [f32; 3],
    ) -> Option<Sample> {
        let Some(sides) = self.sides.as_ref() else {
            return Some(self.plain_sample(member, closest, point));
        };
        let faces = &self.faces[self.member_faces[member].clone()];
        let side = sides[member].side(self.positions, faces, closest.triangle, closest.point)?;
        let signed = contact_offset(side, sub(point, closest.point))?;
        Some(Sample {
            signed: signed as f32,
            normal: Some(side.normal),
            member,
        })
    }

    /// The nearest triangle's own face-normal sample, with no coverage rule.
    fn plain_sample(
        &self,
        member: usize,
        closest: &ClosestSurfacePoint,
        point: [f32; 3],
    ) -> Sample {
        let face = self.faces[self.member_faces[member].start + closest.triangle];
        let corners = [
            self.positions[face[0] as usize],
            self.positions[face[1] as usize],
            self.positions[face[2] as usize],
        ];
        let normal = unit_triangle_normal(corners)
            .map(|normal| [normal[0] as f32, normal[1] as f32, normal[2] as f32]);
        let signed = match normal {
            Some(normal) => dot(sub(point, closest.point), normal),
            None => 0.0,
        };
        Sample {
            signed,
            normal,
            member,
        }
    }

    fn sample(&self, point: [f32; 3]) -> Option<Sample> {
        // Outermost member first: the first member the vertex is behind and
        // covered by owns it, exactly the rule
        // `AccumulatedSurface::layered_visible_depths` grades by. A vertex
        // behind no member floats; the nearest covering member is its pull
        // target, and on a tie the outermost member wins. One member reduces
        // both branches to the plain nearest-triangle sample `dress` has always
        // taken.
        let mut nearest: Option<(usize, ClosestSurfacePoint)> = None;
        let mut last_resort: Option<(usize, ClosestSurfacePoint)> = None;
        for member in (0..self.members.len()).rev() {
            let closest: ClosestSurfacePoint = self.members[member].closest(point)?;
            if last_resort.is_none_or(|(_, best)| closest.distance < best.distance) {
                last_resort = Some((member, closest));
            }
            let Some(sample) = self.member_sample(member, &closest, point) else {
                continue;
            };
            if sample.normal.is_some() && sample.signed < 0.0 {
                return Some(sample);
            }
            if nearest.is_none_or(|(_, best)| closest.distance < best.distance) {
                nearest = Some((member, closest));
            }
        }
        match nearest {
            Some((member, closest)) => self.member_sample(member, &closest, point),
            // Every member is one the vertex only grazes: measure it against
            // the nearest one anyway, so no vertex leaves the band unmeasured.
            None => last_resort.map(|(member, closest)| self.plain_sample(member, &closest, point)),
        }
    }

    /// Obstacle vertices poking through the garment: the shard a render shows
    /// and a per-vertex clearance cannot see. @req DECL0013
    fn poke_hits<'g>(&self, garment: Mesh<'g>) -> Vec<(usize, crate::check::RayHit, f32)> {
        self.surface.poke_hits(garment, &self.normals, POKE_SEARCH)
    }
}

/// Midpoint-subdivide `triangles` `cuts` times, sharing every new edge vertex.
///
/// Vertex order is stable: the input vertices keep their indices and the new
/// midpoint vertices follow in first-edge order, so a caller's vertex map stays
/// valid after dressing — [`seam_weld`] supplies the authored-to-merged half.
/// Sharing the edge vertices is not an optimisation — a per-triangle fan would
/// open every edge into a crack the contact solve cannot close.
pub fn midpoint_subdivide(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    cuts: u32,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut result_positions = positions.to_vec();
    let mut result_triangles = triangles.to_vec();
    for _ in 0..cuts {
        let mut midpoints = std::collections::HashMap::new();
        let mut next = Vec::new();
        let mut mid = |a: u32, b: u32, result_positions: &mut Vec<[f32; 3]>| -> u32 {
            let key = (a.min(b), a.max(b));
            *midpoints.entry(key).or_insert_with(|| {
                let index = result_positions.len() as u32;
                let midpoint = scale(
                    add(result_positions[a as usize], result_positions[b as usize]),
                    0.5,
                );
                result_positions.push(midpoint);
                index
            })
        };
        for &[a, b, c] in &result_triangles {
            let ab = mid(a, b, &mut result_positions);
            let bc = mid(b, c, &mut result_positions);
            let ca = mid(c, a, &mut result_positions);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        result_triangles = next;
    }
    (result_positions, result_triangles)
}

/// The dressed mesh and its report: positions, triangles and measurements.
pub type Dressed = (Vec<[f32; 3]>, Vec<[u32; 3]>, DressReport);

/// Tolerance of the seam merge, in input units: the crate's own resolution
/// (`FitOptions::weld_distance`, `CONVERGENCE`).
///
/// The supplied garments carry their authored seam copies at bit-identical
/// positions, so any positive tolerance merges them; 1e-4 is chosen for
/// consistency with the two other users of that resolution, not because the
/// twins need the room. It is absolute in input units: a caller that rescales a
/// garment rescales the meaning of the seam tolerance with it.
pub const SEAM_WELD_EPSILON: f32 = 1e-4;

/// What the seam merge did to the authored garment.
///
/// A garment is authored as separate pieces — the supplied panty is five, the
/// bra six — and their seam copies are exactly coincident. Subdividing and
/// solving each copy separately tears them apart, so the merge runs first. The
/// record is the audit trail: `groups` names the merged raw ids and
/// `vertex_map` is the fast path from a source vertex to its dressed one.
/// @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct SeamWeldReport {
    /// Tolerance the grouping used, in input units.
    pub tolerance: f32,
    /// Vertices of the merged garment.
    pub welded_vertices: usize,
    /// Source vertices that became a copy of a representative.
    pub merged_vertices: usize,
    /// Coincident source pairs the merge joined.
    pub merged_pairs: usize,
    /// Merged groups: raw ids, the representative first. A group joins
    /// coincident copies that belonged to *different* pieces; a coincidence
    /// inside one piece is an authored dart and stays open.
    pub groups: Vec<Vec<u32>>,
    /// Merged vertex of every source vertex, in source order.
    pub vertex_map: Vec<u32>,
    /// Connected pieces of the authored garment.
    pub components_before: usize,
    /// Connected pieces of the merged garment.
    pub components_after: usize,
    /// Edges one triangle used before the merge: the authored openings.
    pub boundary_edges_before: usize,
    /// Edges one triangle uses after the merge: the openings that remain.
    pub boundary_edges_after: usize,
    /// Faces dropped because the merge collapsed two of their corners.
    pub dropped_faces: usize,
}

/// Merge the coincident seam copies of separate garment pieces.
///
/// Faces a piece authored as its own dart — two coincident lips of one
/// connected piece — are not merged: they are a zero-width feature of that
/// piece, and closing them would delete it. A merged group takes the lowest raw
/// id as its representative and the merged mesh lists representatives in
/// ascending id order, so the merge is deterministic and `vertex_map` is the
/// only source-to-dressed correspondence a caller needs.
///
/// @req DECL0013
pub fn seam_weld(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    tolerance: f32,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>, SeamWeldReport) {
    let piece_of = component_labels(positions.len(), triangles);
    let groups: Vec<Vec<u32>> = coincident_groups(positions, tolerance)
        .into_iter()
        .filter(|group| {
            let piece = piece_of[group[0]];
            group.iter().any(|&vertex| piece_of[vertex] != piece)
        })
        .map(|group| {
            let mut ids: Vec<u32> = group.into_iter().map(|vertex| vertex as u32).collect();
            ids.sort_unstable();
            ids
        })
        .collect();

    let mut representative = (0..positions.len() as u32).collect::<Vec<u32>>();
    for group in &groups {
        let Some(lowest) = group.iter().copied().min() else {
            continue;
        };
        group
            .iter()
            .for_each(|&vertex| representative[vertex as usize] = lowest);
    }
    let mut merged_of = vec![u32::MAX; positions.len()];
    let mut welded_positions = Vec::with_capacity(positions.len());
    for vertex in 0..positions.len() as u32 {
        if representative[vertex as usize] == vertex {
            merged_of[vertex as usize] = welded_positions.len() as u32;
            welded_positions.push(positions[vertex as usize]);
        }
    }
    for vertex in 0..positions.len() as u32 {
        if merged_of[vertex as usize] == u32::MAX {
            merged_of[vertex as usize] = merged_of[representative[vertex as usize] as usize];
        }
    }
    let (welded_triangles, dropped_faces) = remap_faces(triangles, &merged_of);
    let merged_components =
        distinct_labels(&component_labels(welded_positions.len(), &welded_triangles));
    let report = SeamWeldReport {
        tolerance,
        welded_vertices: welded_positions.len(),
        merged_vertices: positions.len() - welded_positions.len(),
        merged_pairs: groups
            .iter()
            .map(|group| group.len() * (group.len() - 1) / 2)
            .sum(),
        groups,
        vertex_map: merged_of,
        components_before: distinct_labels(&piece_of),
        components_after: merged_components,
        boundary_edges_before: OpenEdges::build(triangles, positions.len()).boundary_edges(),
        boundary_edges_after: OpenEdges::build(&welded_triangles, welded_positions.len())
            .boundary_edges(),
        dropped_faces,
    };
    (welded_positions, welded_triangles, report)
}

/// How many distinct values `labels` holds.
fn distinct_labels(labels: &[u32]) -> usize {
    labels.iter().copied().collect::<BTreeSet<u32>>().len()
}

/// Dress `garment` onto `body` and return the fitted mesh plus a report.
///
/// Both meshes must be in the same frame. The returned mesh is the subdivided
/// one: its first `report.seam_weld.welded_vertices` vertices are the merged
/// garment's vertices in ascending representative order and the rest are
/// subdivision midpoints. [`SeamWeldReport::vertex_map`] locates source vertex
/// `i`; a merged copy shares its representative's position and motion.
///
/// # Errors
///
/// Refuses [`DressError`] before any query: malformed or empty geometry, a
/// body without triangles, and every out-of-range parameter. A registration or
/// final-push refusal keeps its own [`FitError`] under [`DressError`].
pub fn dress(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    params: &DressParams,
) -> Result<Dressed, DressError> {
    params.validate()?;
    validate_geometry(garment, body)?;
    let obstacle = Obstacle::build(body.0, body.1);
    solve(garment, body, &obstacle, None, params)
}

/// Dress `garment` over an already-fitted stack: the accumulated surface of the
/// body plus the layers below the new garment.
///
/// The obstacle is the composite graded by the outermost-member rule, so a
/// vertex the underwear covers resolves against the underwear and a vertex over
/// bare skin resolves against the body — never against a covered body triangle
/// the cloth between them hides. The registration stage runs against the same
/// composite, an optional final push-out grades through
/// [`final_push_layered`], and the report names the member every returned
/// vertex projected against.
///
/// # Errors
///
/// The same refusals as [`dress`]; the composite itself is validated by
/// [`AccumulatedSurface`] before this runs.
pub fn dress_against(
    garment: Mesh<'_>,
    accumulated: &AccumulatedSurface,
    params: &DressParams,
) -> Result<Dressed, DressError> {
    params.validate()?;
    validate_geometry(garment, accumulated.obstacle())?;
    let obstacle = Obstacle::build_layered(accumulated);
    let mut dressed = solve(
        garment,
        accumulated.obstacle(),
        &obstacle,
        Some(accumulated),
        params,
    )?;
    dressed.2.projections = Some(project(&obstacle, accumulated, &dressed.0));
    Ok(dressed)
}

/// The shared solve both entry points run: seam merge, subdivision, contact
/// band, interleaved registration, poke repair and the optional final
/// push-out. The callers validated the parameters and the geometry before
/// building the obstacle.
///
/// `register` is the surface the registration stage fits against — the body
/// for [`dress`], the accumulated composite for [`dress_against`] — and
/// `accumulated` switches the final push-out to its layered grading.
fn solve(
    garment: Mesh<'_>,
    register: Mesh<'_>,
    obstacle: &Obstacle<'_>,
    accumulated: Option<&AccumulatedSurface>,
    params: &DressParams,
) -> Result<Dressed, DressError> {
    let (welded_positions, welded_triangles, seam_weld) =
        seam_weld(garment.0, garment.1, SEAM_WELD_EPSILON);
    let (mut positions, triangles) =
        midpoint_subdivide(&welded_positions, &welded_triangles, params.cuts);

    let before = measure(obstacle, &positions, &triangles, params);
    let pokes_before = obstacle.poke_hits((&positions, &triangles)).len();
    let authored = positions.clone();
    let mut moved_of = vec![false; positions.len()];

    // The contact solve runs first and the registration stage is interleaved
    // with it, not run before it: a garment authored on a much smaller body
    // starts with a third of its surface behind the skin, and the topology-free
    // fit's nearest-point targets then sit in the crease under a bust — the cup
    // lands in the crease and the breast stays uncovered. Contact first moves
    // the cloth onto the body it has to cover; the fit then carries that
    // deformation over the garment's own edges. Each round ends with a contact
    // pass, because the fit alone leaves a floating shell behind. @req DECL0013
    let first = resolve(obstacle, &mut positions, &triangles, &mut moved_of, params);
    let mut iterations = first.iterations;
    let mut max_move = first.max_move;
    let mut registration = None;
    let mut solved = first;
    for _ in 0..params.rounds {
        let options = register_options(params);
        let (registered, report) = fit_to_body((&positions, &triangles), register, None, &options)
            .map_err(DressError::Registration)?;
        positions = registered;
        registration = Some(report);
        solved = resolve(obstacle, &mut positions, &triangles, &mut moved_of, params);
        iterations += solved.iterations;
        max_move = max_move.max(solved.max_move);
    }
    let mut report = DressReport {
        source_vertices: garment.0.len(),
        source_triangles: garment.1.len(),
        cuts: params.cuts,
        vertices: positions.len(),
        triangles: triangles.len(),
        seam_weld,
        clearance: params.clearance,
        contact: params.contact,
        iterations,
        converged: solved.converged,
        moved: moved_of.iter().filter(|&&moved| moved).count(),
        max_move,
        floating_before: before.floating,
        max_floating_gap_before: before.max_floating_gap,
        max_floating_gap: solved.max_floating_gap,
        penetrating_before: before.penetrating,
        max_penetration_before: before.max_penetration,
        max_penetration: solved.max_penetration,
        pokes_before,
        pokes_after: solved.pokes,
        flipped: flipped_triangles(&authored, &positions, &triangles),
        registration,
        final_push: None,
        projections: None,
    };
    if let Some(options) = params.final_push {
        let (pushed, push_report) = match accumulated {
            None => final_push(
                (&positions, &triangles),
                register,
                None,
                &FitOptions::default(),
                &options,
            ),
            Some(accumulated) => final_push_layered(
                (&positions, &triangles),
                accumulated,
                None,
                &FitOptions::default(),
                &options,
            ),
        }
        .map_err(DressError::FinalPush)?;
        positions = pushed;
        report.final_push = Some(push_report);
    }
    if let Some(vertex) = first_non_finite(&positions) {
        return Err(DressError::NonFiniteResult { vertex });
    }
    Ok((positions, triangles, report))
}

/// Which accumulated member every solved vertex resolved against: the same
/// outermost-member sample the contact solve took, read once more off the
/// returned mesh. @req DECL0013
fn project(
    obstacle: &Obstacle<'_>,
    accumulated: &AccumulatedSurface,
    positions: &[[f32; 3]],
) -> DressProjections {
    let mut members: Vec<DressMemberProjection> = accumulated
        .layers()
        .iter()
        .map(|layer| DressMemberProjection {
            id: layer.id.clone(),
            order: layer.order,
            vertices: 0,
        })
        .collect();
    let mut vertex_members = Vec::with_capacity(positions.len());
    for &position in positions {
        let member = obstacle
            .sample(position)
            .map(|sample| sample.member)
            .unwrap_or(0);
        members[member].vertices += 1;
        vertex_members.push(member);
    }
    DressProjections {
        surface: accumulated.surface_label(),
        members,
        vertex_members,
    }
}

/// The registration the contact solve starts from: a constant declared gap
/// under the fit's own strain cap and flip repair, so a large authored oversize
/// comes in smoothly instead of in one per-vertex jump. @req DECL0013
fn register_options(params: &DressParams) -> FitOptions {
    FitOptions {
        gap_mode: GapMode::Constant,
        gap: params.clearance,
        min_gap: params.contact,
        relief_smoothing: 0,
        clipping_passes: 2,
        poke_depth: 0.0,
        ..FitOptions::default()
    }
}

impl DressParams {
    /// Reject a parameter that has no accepted reading, so no value is
    /// silently clamped into something the caller did not ask for.
    fn validate(&self) -> Result<(), DressError> {
        if !self.clearance.is_finite() || self.clearance <= 0.0 {
            return Err(DressError::Clearance(self.clearance));
        }
        if !self.contact.is_finite() || self.contact < 0.0 || self.contact >= self.clearance {
            return Err(DressError::Contact {
                contact: self.contact,
                clearance: self.clearance,
            });
        }
        if self.cuts > MAX_CUTS {
            return Err(DressError::Cuts(self.cuts));
        }
        if self.iterations == 0 || self.iterations > MAX_ITERATIONS {
            return Err(DressError::Iterations(self.iterations));
        }
        if !self.max_step.is_finite() || self.max_step <= 0.0 {
            return Err(DressError::MaxStep(self.max_step));
        }
        if !self.relax.is_finite() || !(0.0..=1.0).contains(&self.relax) {
            return Err(DressError::Relax(self.relax));
        }
        if self.rounds > MAX_ROUNDS {
            return Err(DressError::Rounds(self.rounds));
        }
        if self.poke_passes > MAX_POKE_PASSES {
            return Err(DressError::PokePasses(self.poke_passes));
        }
        Ok(())
    }
}

/// Band measurement of one mesh against the body, without moving anything.
struct Band {
    floating: usize,
    max_floating_gap: f32,
    penetrating: usize,
    max_penetration: f32,
}

fn measure(
    obstacle: &Obstacle<'_>,
    positions: &[[f32; 3]],
    _triangles: &[[u32; 3]],
    params: &DressParams,
) -> Band {
    let mut band = Band {
        floating: 0,
        max_floating_gap: 0.0,
        penetrating: 0,
        max_penetration: 0.0,
    };
    for &position in positions {
        let Some(sample) = obstacle.sample(position) else {
            continue;
        };
        if sample.signed > params.clearance {
            band.floating += 1;
            band.max_floating_gap = band.max_floating_gap.max(sample.signed);
        }
        if sample.signed < 0.0 {
            band.penetrating += 1;
            band.max_penetration = band.max_penetration.max(-sample.signed);
        }
    }
    band
}

/// What the contact loop measured and left.
struct Solved {
    iterations: u32,
    converged: bool,
    max_move: f32,
    max_floating_gap: f32,
    max_penetration: f32,
    pokes: usize,
}

/// Push or pull every vertex into the `[contact, clearance]` band, repair the
/// body pokes the band cannot see between garment vertices, and repeat.
///
/// The two stages alternate instead of running once each: a poke push moves a
/// triangle out, and the next contact pass would pull it straight back unless
/// the push is re-measured. Each round is bounded, the contact loop stops at
/// [`CONVERGENCE`], and every single move is clamped to [`DressParams::max_step`]
/// along a direction that ends on the target, never past it.
fn resolve(
    obstacle: &Obstacle<'_>,
    positions: &mut [[f32; 3]],
    triangles: &[[u32; 3]],
    moved_of: &mut [bool],
    params: &DressParams,
) -> Solved {
    let mut neighbours = vec![Vec::new(); positions.len()];
    for face in triangles {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            let (a, b) = (a as usize, b as usize);
            if !neighbours[a].contains(&b) {
                neighbours[a].push(b);
            }
            if !neighbours[b].contains(&a) {
                neighbours[b].push(a);
            }
        }
    }
    // A garment vertex deep inside a bust cannot follow the nearest body
    // triangle: at the under-bust crease that triangle faces down, so the
    // shortest way out runs along the crease and the cup never covers the
    // breast. The garment's own outward normal names the side its cloth
    // belongs on, so a penetrating vertex pushes out along it when it still
    // agrees with the body normal; a shallow clip keeps the exact body normal.
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for face in triangles {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            let key = (a.min(b), a.max(b));
            if !edges.contains(&key) {
                edges.push(key);
            }
        }
    }
    // An open hem is never slid: its one-ring mean sits inboard, so a
    // tangential pass curls the border instead of easing it.
    let mut uses: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for face in triangles {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let mut boundary = vec![false; positions.len()];
    for (&(a, b), &count) in &uses {
        if count == 1 {
            boundary[a as usize] = true;
            boundary[b as usize] = true;
        }
    }
    let authored_edges: Vec<f32> = edges
        .iter()
        .map(|&(a, b)| length(sub(positions[b as usize], positions[a as usize])))
        .collect();
    let garment_normals = vertex_normals(positions, triangles);
    let mut max_move = 0.0f32;
    let mut iterations = 0;
    let mut converged = false;
    for round in 0..=params.poke_passes {
        for iteration in 0..params.iterations {
            iterations += 1;
            // Smooth first, project second, and cool the smoothing to zero on
            // the last iteration: the projection is the last word of a pass, so
            // the band it establishes is what the report and the next
            // measurement read. A relaxation that stayed at full weight leaves
            // the smoothed surface a little outside the pull target, and the
            // band is then missed by exactly that amount.
            let total = params.iterations.max(1) as f32;
            let remaining = (params.iterations - iteration).max(1) as f32;
            let weight = params.relax * (remaining - 1.0) / total;
            if weight > 0.0 {
                let strain = edge_strain(positions, &edges, &authored_edges);
                relax(
                    positions,
                    triangles,
                    &neighbours,
                    &strain,
                    &boundary,
                    weight,
                );
            }
            let mut largest = 0.0f32;
            for vertex in 0..positions.len() {
                let Some(sample) = obstacle.sample(positions[vertex]) else {
                    continue;
                };
                let Some(body_normal) = sample.normal else {
                    continue;
                };
                let displacement = if sample.signed > params.clearance {
                    // Floating: come in along the body normal, stopping on the
                    // declared clearance measured the way the depth report
                    // measures it. Moving the vertex onto the nearest point's
                    // shell instead would drag the cup sideways over the bust
                    // and fold it.
                    let step = (params.clearance - PULL_MARGIN - sample.signed)
                        .clamp(-params.max_step, params.max_step);
                    scale(body_normal, step)
                } else if sample.signed < params.contact {
                    // Penetrating: leave the body along the garment's own
                    // outward normal while that side still agrees with the
                    // body's. A cup embedded in a bust has to expand over the
                    // bump; the shortest way out (the body normal at the
                    // nearest triangle) runs down the under-bust crease and
                    // never covers the breast. The depth along the body normal
                    // is divided by the agreement, the distance along the
                    // garment normal that removes it, so the step shrinks with
                    // the penetration instead of running past the surface.
                    let own = garment_normals[vertex];
                    let agreement = dot(own, body_normal);
                    match agreement > MIN_PUSH_AGREEMENT {
                        true => {
                            let step = (params.contact - sample.signed) / agreement;
                            scale(own, step.min(params.max_step))
                        }
                        false => {
                            let step = (params.contact - sample.signed)
                                .clamp(-params.max_step, params.max_step);
                            scale(body_normal, step)
                        }
                    }
                } else {
                    continue;
                };
                let step = length(displacement);
                positions[vertex] = add(positions[vertex], displacement);
                moved_of[vertex] = true;
                max_move = max_move.max(step.abs());
                largest = largest.max(step.abs());
            }
            if largest <= CONVERGENCE {
                converged = true;
                break;
            }
        }
        if round == params.poke_passes {
            break;
        }
        // A poke is a body triangle crossing the garment between its corners;
        // the crossed triangle moves out as a whole, so the cover stays closed.
        let hits = obstacle.poke_hits((positions, triangles));
        if hits.is_empty() {
            break;
        }
        let pushed = push_pokes(obstacle, positions, triangles, moved_of, params, &hits);
        max_move = max_move.max(pushed);
        converged = false;
    }
    // The loop above ends every round with a contact pass, so leaving it with
    // no hits is the repair's own convergence test: a facet that a later pull
    // reopens shows up again here, and the pass budget is what bounds it.
    let band = measure(obstacle, positions, triangles, params);
    Solved {
        iterations,
        converged,
        max_move,
        max_floating_gap: band.max_floating_gap,
        max_penetration: band.max_penetration,
        pokes: obstacle.poke_hits((positions, triangles)).len(),
    }
}

/// Push every crossed garment triangle out once by its deepest poke, and
/// return the largest displacement applied.
///
/// One garment triangle can be crossed by many body vertices — a breast meets
/// the same cup facet through a whole ring of them. Each triangle moves once by
/// its deepest poke, not once per hit: summing the pushes launches the facet
/// out of the garment.
fn push_pokes(
    obstacle: &Obstacle<'_>,
    positions: &mut [[f32; 3]],
    triangles: &[[u32; 3]],
    moved_of: &mut [bool],
    params: &DressParams,
    hits: &[(usize, crate::check::RayHit, f32)],
) -> f32 {
    let mut deepest: BTreeMap<usize, (usize, f32)> = BTreeMap::new();
    for &(vertex, hit, depth) in hits {
        let entry = deepest.entry(hit.triangle).or_insert((vertex, depth));
        if depth > entry.1 {
            *entry = (vertex, depth);
        }
    }
    let mut largest = 0.0f32;
    for (triangle, (vertex, depth)) in deepest {
        // Clear the crossing by the contact standoff, not by the declared
        // clearance: a lift of `depth + clearance` ends above the band, the next
        // pull then returns the facet to the clearance, and the poke opens
        // again — the repair and the pull cancel each other round after round.
        // A lift of `depth + contact` lands inside the band, so the pull leaves
        // it alone and the crossing stays closed.
        let margin = params.contact.max(PULL_MARGIN);
        let step = (depth + margin).min(params.max_step);
        let displacement = scale(obstacle.normals[vertex], step);
        for corner in triangles[triangle] {
            let corner = corner as usize;
            positions[corner] = add(positions[corner], displacement);
            moved_of[corner] = true;
        }
        largest = largest.max(step);
    }
    largest
}

/// Even out the garment's own surface without leaving its standoff: one
/// Laplacian pass per vertex, with the component along the vertex normal
/// removed, and only where the projection has actually stretched the cloth.
///
/// A cup with more area than the body it now covers crumples where the
/// projection compresses it, so the extra area has to slide. A plain Laplacian
/// pass would redistribute it but also shrink the garment toward its centroid
/// and lift it off the band; keeping only the tangential part slides the
/// vertices within the cloth and leaves every standoff to the contact
/// projection. `strain` restricts the pass to vertices the projection moved
/// apart or together: a garment that already fits — the supplied panty — must
/// not have its waistband slid into folds.
fn relax(
    positions: &mut [[f32; 3]],
    triangles: &[[u32; 3]],
    neighbours: &[Vec<usize>],
    strain: &[f32],
    boundary: &[bool],
    weight: f32,
) {
    let current = positions.to_vec();
    let normals = vertex_normals(&current, triangles);
    for vertex in 0..positions.len() {
        if neighbours[vertex].is_empty() {
            continue;
        }
        let share = (strain[vertex] - RELAX_MIN_STRAIN) / (RELAX_FULL_STRAIN - RELAX_MIN_STRAIN);
        if share <= 0.0 {
            continue;
        }
        let ring: Vec<usize> = match boundary[vertex] {
            // A hem slides along itself only: its full one-ring mean sits
            // inboard and would drag the border off the body.
            true => neighbours[vertex]
                .iter()
                .copied()
                .filter(|&neighbour| boundary[neighbour])
                .collect(),
            false => neighbours[vertex].clone(),
        };
        if ring.is_empty() {
            continue;
        }
        let mut mean = [0.0f32; 3];
        for &neighbour in &ring {
            mean = add(mean, current[neighbour]);
        }
        let mean = scale(mean, 1.0 / ring.len() as f32);
        let delta = sub(mean, current[vertex]);
        let step = match boundary[vertex] {
            true => delta,
            false => {
                let normal = normals[vertex];
                sub(delta, scale(normal, dot(delta, normal)))
            }
        };
        positions[vertex] = add(current[vertex], scale(step, weight * share.clamp(0.0, 1.0)));
    }
}

/// Edge strain at which the relaxation starts, and the strain at which it runs
/// at full weight: a garment the projection did not stretch keeps its form.
const RELAX_MIN_STRAIN: f32 = 0.2;
const RELAX_FULL_STRAIN: f32 = 0.8;

/// Largest relative length change of each vertex's incident edges against the
/// authored mesh: where the projection stretched or compressed the cloth.
fn edge_strain(positions: &[[f32; 3]], edges: &[(u32, u32)], authored: &[f32]) -> Vec<f32> {
    let mut strain = vec![0.0f32; positions.len()];
    for (index, &(a, b)) in edges.iter().enumerate() {
        let length = length(sub(positions[b as usize], positions[a as usize]));
        let change = match authored[index] > 1e-6 {
            true => (length / authored[index] - 1.0).abs(),
            false => 0.0,
        };
        strain[a as usize] = strain[a as usize].max(change);
        strain[b as usize] = strain[b as usize].max(change);
    }
    strain
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
            let was = unit(face_normal_of(corners(before)));
            let now = unit(face_normal_of(corners(after)));
            length(was) > 0.0 && length(now) > 0.0 && dot(was, now) < 0.0
        })
        .count()
}

/// The shared geometry refusals, in this module's own names.
fn validate_geometry(garment: Mesh<'_>, body: Mesh<'_>) -> Result<(), DressError> {
    if garment.0.is_empty() {
        return Err(DressError::EmptyGarment);
    }
    if body.0.is_empty() {
        return Err(DressError::EmptyBody);
    }
    if body.1.is_empty() {
        return Err(DressError::EmptyBodyFaces);
    }
    if let Some(vertex) = first_non_finite(garment.0) {
        return Err(DressError::NonFiniteGarmentPosition { vertex });
    }
    if let Some(vertex) = first_non_finite(body.0) {
        return Err(DressError::NonFiniteBodyPosition { vertex });
    }
    let garment_count = garment.0.len() as u32;
    for (triangle, face) in garment.1.iter().enumerate() {
        if let Some(&vertex) = face.iter().find(|&&vertex| vertex >= garment_count) {
            return Err(DressError::GarmentFaceIndex { triangle, vertex });
        }
    }
    let body_count = body.0.len() as u32;
    for (triangle, face) in body.1.iter().enumerate() {
        if let Some(&vertex) = face.iter().find(|&&vertex| vertex >= body_count) {
            return Err(DressError::BodyFaceIndex { triangle, vertex });
        }
    }
    Ok(())
}
