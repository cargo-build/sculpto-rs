//! Body compression under a pressing cover, and follower motion over it.
//!
//! [`make_contact_morph`] ports `MeshDeform::MakeContactMorph`
//! (`MeshDeform.cpp:250-466`): the body under the cover sinks in, smoothly from
//! the edge of the covered area, and swells out just past it. Distances are
//! measured along the body's surface, so the shape follows the cover's outline
//! instead of the creases of a box mask. [`follow_deltas`] ports
//! `MeshDeform::FollowDeltas` (`468-549`): a follower mesh takes the body motion
//! at its closest surface point, fades out at the query radius, smooths over its
//! own mesh and goes back out to its old gap where the morphed body would close
//! in on it.
//!
//! Both produce pure deltas in the caller's frame; writing a slider or moving an
//! asset belongs to the consumer. Locks reuse the shared [`VertexMask`] storage
//! (1 = locked) and apply as `freedom = 1 - mask`, so a fully locked vertex keeps
//! an exact zero delta. The closest-point and ray queries use the diagnostic
//! [`SurfaceIndex`] (`DECL0013`) rather than a second index or predicate.
//!
//! Control-surface self-contact repair is a different contract
//! ([`crate::control::contact`]); this module neither reads nor replaces it.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fmt;

use crate::check::SurfaceIndex;
use crate::mask::VertexMask;
use crate::point_query::PointIndex;
use crate::surface_util::{
    OpenEdges, add, barycentric_clamped, coincident_groups, dot, edge_pairs, face_normal_of,
    first_non_finite, length, past_open_edge, scale, sub, unit,
};

/// Native floor of the contact distance (`MeshDeform.cpp:290`).
const MIN_CONTACT_DISTANCE: f32 = 1e-4;

/// Native floor of the profile widths (`MeshDeform.cpp:358-359`).
const MIN_PROFILE_WIDTH: f32 = 1e-3;

/// Native passes that smooth the body's outward normals (`MeshDeform.cpp:274`).
const NORMAL_SMOOTHING_PASSES: usize = 10;

/// Native share of the neighbour average per offset smoothing pass
/// (`MeshDeform.cpp:439`).
const OFFSET_SMOOTHING_STRENGTH: f32 = 0.5;

/// Native share of the neighbour average per follower smoothing pass
/// (`MeshDeform.cpp:510`).
const FOLLOWER_SMOOTHING_STRENGTH: f32 = 0.5;

/// Native follower smoothing passes (`MeshDeform.cpp:501`).
const FOLLOWER_SMOOTHING_PASSES: usize = 2;

/// Native follower weld distance (`MeshDeform.cpp:496`).
const FOLLOWER_WELD_DISTANCE: f32 = 1e-4;

/// Share of the query radius that the follower follows in full; the smoothstep
/// fade runs over the rest (`MeshDeform.cpp:488-489`).
const FOLLOWER_FULL_MOTION_SHARE: f32 = 0.75;

/// Native gap-restoring passes and the gap change worth correcting
/// (`MeshDeform.cpp:524,537`).
const FOLLOWER_GAP_PASSES: usize = 3;
const FOLLOWER_GAP_TOLERANCE: f32 = 1e-3;

/// Faint smoothed tails below this magnitude are not worth keeping as slider
/// data (`MeshDeform.cpp:458`).
const TAIL_CUTOFF: f32 = 0.002;

/// `nifly::Vector3::IsZero(true)` compares against nifly's `EPSILON`
/// (`Object3d.hpp:16,111-122`).
const ZERO_EPSILON: f32 = 1e-4;

/// How far the body is pressed, and how it eases back out.
///
/// The defaults are the native values (`MeshDeform.h:41-50`); every length is in
/// the caller's coordinate units. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactMorphOptions {
    /// Body vertices this close to the cover are under it.
    pub contact_distance: f32,
    /// And those with the cover in front of them this close, where it tents over
    /// the body.
    pub tent_distance: f32,
    /// How far the covered body sinks in.
    pub depth: f32,
    /// How far in from the edge of the covered area the full depth is reached.
    pub depth_width: f32,
    /// How far the body swells out past the edge, where the body next to it sank
    /// the full depth.
    pub bulge: f32,
    /// Distance from the edge to the top of the swell, which fades out after it.
    pub bulge_width: f32,
    /// Passes that smooth the result.
    pub smoothing: u32,
    /// Vertex copies along split seams closer than this move together.
    pub weld_distance: f32,
}

impl Default for ContactMorphOptions {
    fn default() -> Self {
        Self {
            contact_distance: 0.45,
            tent_distance: 2.0,
            depth: 1.3,
            depth_width: 1.8,
            bulge: 0.85,
            bulge_width: 1.2,
            smoothing: 5,
            weld_distance: 1e-4,
        }
    }
}

impl ContactMorphOptions {
    /// Reject values the native formulas cannot take before any mesh query runs.
    pub fn validate(&self) -> Result<(), ContactError> {
        finite_positive(self.contact_distance, ContactError::ContactDistance)?;
        finite_nonnegative(self.tent_distance, ContactError::TentDistance)?;
        finite_nonnegative(self.depth, ContactError::Depth)?;
        finite_positive(self.depth_width, ContactError::DepthWidth)?;
        finite_nonnegative(self.bulge, ContactError::Bulge)?;
        finite_positive(self.bulge_width, ContactError::BulgeWidth)?;
        finite_nonnegative(self.weld_distance, ContactError::WeldDistance)
    }
}

fn finite_positive(value: f32, error: fn(f32) -> ContactError) -> Result<(), ContactError> {
    match value.is_finite() && value > 0.0 {
        true => Ok(()),
        false => Err(error(value)),
    }
}

fn finite_nonnegative(value: f32, error: fn(f32) -> ContactError) -> Result<(), ContactError> {
    match value.is_finite() && value >= 0.0 {
        true => Ok(()),
        false => Err(error(value)),
    }
}

/// Why a contact morph or a follower motion was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContactError {
    /// `contact_distance` is not finite and positive.
    ContactDistance(f32),
    /// `tent_distance` is not finite and nonnegative.
    TentDistance(f32),
    /// `depth` is not finite and nonnegative.
    Depth(f32),
    /// `depth_width` is not finite and positive.
    DepthWidth(f32),
    /// `bulge` is not finite and nonnegative.
    Bulge(f32),
    /// `bulge_width` is not finite and positive.
    BulgeWidth(f32),
    /// `weld_distance` is not finite and nonnegative.
    WeldDistance(f32),
    /// The body has a nonfinite position.
    NonFiniteBody { vertex: usize },
    /// The cover has a nonfinite position.
    NonFiniteCover { vertex: usize },
    /// The follower mesh has a nonfinite position.
    NonFiniteFollower { vertex: usize },
    /// A body triangle references a vertex that does not exist.
    BodyFaceIndex { triangle: usize, vertex: u32 },
    /// A cover triangle references a vertex that does not exist.
    CoverFaceIndex { triangle: usize, vertex: u32 },
    /// A follower triangle references a vertex that does not exist.
    FollowerFaceIndex { triangle: usize, vertex: u32 },
    /// The mask does not have one value per body vertex.
    MaskLength { expected: usize, actual: usize },
    /// The body deltas do not have one value per body vertex.
    DeltaCount { expected: usize, actual: usize },
    /// A body delta is not finite.
    NonFiniteDelta { vertex: usize },
    /// The follower radius is not finite and positive.
    Radius(f32),
}

impl fmt::Display for ContactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContactDistance(value) => write!(
                formatter,
                "contact distance {value} is not finite and positive"
            ),
            Self::TentDistance(value) => {
                write!(
                    formatter,
                    "tent distance {value} is not finite and nonnegative"
                )
            }
            Self::Depth(value) => write!(
                formatter,
                "morph depth {value} is not finite and nonnegative"
            ),
            Self::DepthWidth(value) => {
                write!(formatter, "depth width {value} is not finite and positive")
            }
            Self::Bulge(value) => write!(formatter, "bulge {value} is not finite and nonnegative"),
            Self::BulgeWidth(value) => {
                write!(formatter, "bulge width {value} is not finite and positive")
            }
            Self::WeldDistance(value) => write!(
                formatter,
                "weld distance {value} is not finite and nonnegative"
            ),
            Self::NonFiniteBody { vertex } => {
                write!(formatter, "body vertex {vertex} is not finite")
            }
            Self::NonFiniteCover { vertex } => {
                write!(formatter, "cover vertex {vertex} is not finite")
            }
            Self::NonFiniteFollower { vertex } => {
                write!(formatter, "follower vertex {vertex} is not finite")
            }
            Self::BodyFaceIndex { triangle, vertex } => write!(
                formatter,
                "body triangle {triangle} references missing vertex {vertex}"
            ),
            Self::CoverFaceIndex { triangle, vertex } => write!(
                formatter,
                "cover triangle {triangle} references missing vertex {vertex}"
            ),
            Self::FollowerFaceIndex { triangle, vertex } => write!(
                formatter,
                "follower triangle {triangle} references missing vertex {vertex}"
            ),
            Self::MaskLength { expected, actual } => write!(
                formatter,
                "the mask needs {expected} body values, got {actual}"
            ),
            Self::DeltaCount { expected, actual } => write!(
                formatter,
                "the body has {expected} vertices, but {actual} deltas were given"
            ),
            Self::NonFiniteDelta { vertex } => {
                write!(formatter, "body delta {vertex} is not finite")
            }
            Self::Radius(value) => {
                write!(
                    formatter,
                    "follower radius {value} is not finite and positive"
                )
            }
        }
    }
}

impl std::error::Error for ContactError {}

/// A body morph under a pressing cover, one value per body vertex.
///
/// `deltas` are `offsets` along the body's smoothed outward normals, so the
/// caller can add them to the body positions or write them as slider data. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct ContactMorph {
    /// Signed displacement along the outward normal: negative sinks in,
    /// positive swells out.
    pub offsets: Vec<f32>,
    /// `offsets` applied along the outward normal of each body vertex.
    pub deltas: Vec<[f32; 3]>,
    /// Distance along the body's surface to the edge of the covered area:
    /// positive under the cover, negative outside it.
    pub edge_distance: Vec<f32>,
    /// How many body vertices the cover was found over.
    pub covered: usize,
    /// Largest sink, as a positive value.
    pub max_depth: f32,
    /// Largest swell, as a positive value.
    pub max_bulge: f32,
}

/// A body compressed by a cover, from plain positions and triangles in one frame.
///
/// `lock` is the shared 1 = locked mask; a locked vertex keeps an exact zero
/// delta, and a partly locked one scales its sink or swell by its freedom. @req DECL0013
pub fn make_contact_morph(
    body_positions: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    cover_positions: &[[f32; 3]],
    cover_faces: &[[u32; 3]],
    lock: Option<&VertexMask>,
    options: &ContactMorphOptions,
) -> Result<ContactMorph, ContactError> {
    options.validate()?;
    validate_positions(body_positions, |vertex| ContactError::NonFiniteBody {
        vertex,
    })?;
    validate_faces(body_positions, body_faces, |triangle, vertex| {
        ContactError::BodyFaceIndex { triangle, vertex }
    })?;
    validate_positions(cover_positions, |vertex| ContactError::NonFiniteCover {
        vertex,
    })?;
    validate_faces(cover_positions, cover_faces, |triangle, vertex| {
        ContactError::CoverFaceIndex { triangle, vertex }
    })?;
    if let Some(mask) = lock
        && mask.len() != body_positions.len()
    {
        return Err(ContactError::MaskLength {
            expected: body_positions.len(),
            actual: mask.len(),
        });
    }

    let count = body_positions.len();
    let mut morph = ContactMorph {
        offsets: vec![0.0; count],
        deltas: vec![[0.0; 3]; count],
        edge_distance: vec![-f32::MAX; count],
        covered: 0,
        max_depth: 0.0,
        max_bulge: 0.0,
    };
    // Native returns the empty morph: no body, or nothing to press it.
    if count == 0 || cover_positions.is_empty() || cover_faces.is_empty() {
        return Ok(morph);
    }

    let topology = Topology::build(body_positions, body_faces, options.weld_distance);
    let all: Vec<usize> = (0..count).collect();

    // The direction the body moves in: its outward normals, averaged over seam
    // copies and smoothed so a small bump moves back as a whole instead of
    // spreading out along its own normals (`MeshDeform.cpp:269-285`).
    let (mut normals, _) = outward_vertex_normals(body_positions, body_faces);
    average_welds(&topology, &all, &mut normals);
    let mut smoothed = normals.clone();
    for _ in 0..NORMAL_SMOOTHING_PASSES {
        for vertex in 0..count {
            let neighbours = &topology.neighbours[vertex];
            let summed = neighbours.iter().fold(normals[vertex], |sum, &neighbour| {
                add(sum, normals[neighbour])
            });
            smoothed[vertex] = scale(summed, 1.0 / (neighbours.len() + 1) as f32);
        }
        average_welds(&topology, &all, &mut smoothed);
        std::mem::swap(&mut normals, &mut smoothed);
    }
    let normals: Vec<[f32; 3]> = normals.into_iter().map(unit).collect();

    // Under the cover: close to its surface, which includes a margin of
    // contact_distance around its edge, or with the cover right in front of it
    // where it tents over the body (`MeshDeform.cpp:287-345`).
    let contact = options.contact_distance.max(MIN_CONTACT_DISTANCE);
    let tent_reach = options.tent_distance.max(contact);
    let cover_index = SurfaceIndex::build(cover_positions, cover_faces);
    let mut covered = vec![false; count];
    let mut tent_candidates = Vec::new();
    for vertex in 0..count {
        let Some(hit) = closest_hit(
            &cover_index,
            cover_faces,
            cover_positions,
            body_positions[vertex],
            tent_reach,
        ) else {
            continue;
        };
        match hit.distance <= contact {
            true => covered[vertex] = true,
            false => tent_candidates.push(vertex),
        }
    }
    tent_candidates
        .into_iter()
        .filter(|&vertex| {
            cover_index
                .first_hit(
                    body_positions[vertex],
                    normals[vertex],
                    0.0,
                    tent_reach,
                    &[],
                )
                .is_some()
        })
        .for_each(|vertex| covered[vertex] = true);

    // Copies of a seam vertex are covered together.
    let mut covered_copies: Vec<f32> = covered
        .iter()
        .map(|&under| match under {
            true => 1.0,
            false => 0.0,
        })
        .collect();
    average_welds(&topology, &all, &mut covered_copies);
    covered = covered_copies.iter().map(|&value| value > 0.0).collect();
    morph.covered = covered.iter().filter(|&&under| under).count();
    if morph.covered == 0 {
        return Ok(morph);
    }

    let depth_width = options.depth_width.max(MIN_PROFILE_WIDTH);
    let bulge_width = options.bulge_width.max(MIN_PROFILE_WIDTH);
    let distance = edge_distances(
        body_positions,
        &topology,
        &covered,
        (depth_width * 3.0).max(bulge_width * 5.0),
    );

    // Sinks in under the cover, reaching the full depth inside (`MeshDeform.cpp:394-403`).
    for vertex in 0..count {
        let under = covered[vertex];
        let reached = distance[vertex];
        morph.edge_distance[vertex] = match (reached == f32::MAX, under) {
            (true, true) => f32::MAX,
            (true, false) => -f32::MAX,
            (false, true) => reached,
            (false, false) => -reached,
        };
        if under {
            morph.offsets[vertex] = match reached == f32::MAX {
                true => -options.depth,
                false => {
                    let u = reached / depth_width;
                    -options.depth * (1.0 - (-(u * u)).exp())
                }
            };
        }
    }

    // Swells out past the edge, as much as the covered body next to it sank: a
    // thin strap that barely presses in pushes little out (`MeshDeform.cpp:405-426`).
    if options.depth > 0.0 && options.bulge > 0.0 {
        let covered_list: Vec<usize> = (0..count).filter(|&vertex| covered[vertex]).collect();
        let covered_positions: Vec<[f32; 3]> = covered_list
            .iter()
            .map(|&vertex| body_positions[vertex])
            .collect();
        let sink_index = PointIndex::build(&covered_positions);
        // Far enough in to reach the depth the covered area has, not only its
        // shallow edge.
        let near_radius = (depth_width + bulge_width) * 2.0;
        for vertex in 0..count {
            if covered[vertex] || distance[vertex] == f32::MAX {
                continue;
            }
            let sank = sink_index
                .within_radius(body_positions[vertex], near_radius)
                .into_iter()
                .map(|(index, _)| -morph.offsets[covered_list[index]])
                .fold(0.0f32, f32::max);
            let x = distance[vertex] / bulge_width;
            morph.offsets[vertex] =
                options.bulge * (sank / options.depth).min(1.0) * x * x * (2.0 * (1.0 - x)).exp();
        }
    }

    // Smoothed over the surface, which rounds the turn at the edge (`MeshDeform.cpp:428-443`).
    let mut next = morph.offsets.clone();
    for _ in 0..options.smoothing {
        for (vertex, value) in next.iter_mut().enumerate() {
            let neighbours = &topology.neighbours[vertex];
            if neighbours.is_empty() {
                continue;
            }
            let average = neighbours
                .iter()
                .map(|&neighbour| morph.offsets[neighbour])
                .sum::<f32>()
                / neighbours.len() as f32;
            *value = morph.offsets[vertex]
                + (average - morph.offsets[vertex]) * OFFSET_SMOOTHING_STRENGTH;
        }
        average_welds(&topology, &all, &mut next);
        morph.offsets.copy_from_slice(&next);
    }

    // The swell only rises as far as the cover leaves room: none right next to
    // it, where its edge flares away from the body and the body would push
    // through between its vertices, all of it where the cover is far (`MeshDeform.cpp:445-452`).
    if options.bulge > 0.0 {
        let room_reach = contact + options.bulge;
        for (vertex, &position) in body_positions.iter().enumerate() {
            if morph.offsets[vertex] <= 0.0 {
                continue;
            }
            let Some(hit) = closest_hit(
                &cover_index,
                cover_faces,
                cover_positions,
                position,
                room_reach,
            ) else {
                continue;
            };
            morph.offsets[vertex] = morph.offsets[vertex].min((hit.distance - contact).max(0.0));
        }
    }

    // Freedom scales what is left, and the faint tails go (`MeshDeform.cpp:454-464`).
    let freedom: Vec<f32> = match lock {
        Some(mask) => mask.as_slice().iter().map(|value| 1.0 - value).collect(),
        None => vec![1.0; count],
    };
    for vertex in 0..count {
        let scaled = morph.offsets[vertex] * freedom[vertex];
        morph.offsets[vertex] = match scaled.abs() < TAIL_CUTOFF {
            true => 0.0,
            false => scaled,
        };
        morph.deltas[vertex] = scale(normals[vertex], morph.offsets[vertex]);
        morph.max_depth = morph.max_depth.max(-morph.offsets[vertex]);
        morph.max_bulge = morph.max_bulge.max(morph.offsets[vertex]);
    }
    Ok(morph)
}

/// Deltas for a follower mesh sitting on or near a morphed body.
///
/// Each follower vertex moves like the closest point of the body's surface,
/// fully up to three quarters of `radius` and fading out to nothing at `radius`.
/// With follower triangles the motion is smoothed over that mesh so pieces move
/// as a whole, and a vertex that would end up closer to the body than it was goes
/// back out to its old gap (`MeshDeform.cpp:468-549`). @req DECL0013
pub fn follow_deltas(
    body_positions: &[[f32; 3]],
    body_faces: &[[u32; 3]],
    body_deltas: &[[f32; 3]],
    follower_positions: &[[f32; 3]],
    follower_faces: &[[u32; 3]],
    radius: f32,
) -> Result<Vec<[f32; 3]>, ContactError> {
    finite_positive(radius, ContactError::Radius)?;
    validate_positions(body_positions, |vertex| ContactError::NonFiniteBody {
        vertex,
    })?;
    validate_faces(body_positions, body_faces, |triangle, vertex| {
        ContactError::BodyFaceIndex { triangle, vertex }
    })?;
    validate_positions(follower_positions, |vertex| {
        ContactError::NonFiniteFollower { vertex }
    })?;
    validate_faces(follower_positions, follower_faces, |triangle, vertex| {
        ContactError::FollowerFaceIndex { triangle, vertex }
    })?;
    if body_deltas.len() != body_positions.len() {
        return Err(ContactError::DeltaCount {
            expected: body_positions.len(),
            actual: body_deltas.len(),
        });
    }
    if let Some(vertex) = body_deltas
        .iter()
        .position(|delta| !delta.iter().all(|value| value.is_finite()))
    {
        return Err(ContactError::NonFiniteDelta { vertex });
    }

    let mut deltas = vec![[0.0; 3]; follower_positions.len()];
    if body_positions.is_empty() {
        return Ok(deltas);
    }

    let body_index = SurfaceIndex::build(body_positions, body_faces);
    for (point, &position) in follower_positions.iter().enumerate() {
        let Some(hit) = closest_hit(&body_index, body_faces, body_positions, position, radius)
        else {
            continue;
        };
        let face = body_faces[hit.triangle].map(|vertex| vertex as usize);
        let blended = add(
            add(
                scale(body_deltas[face[0]], hit.bary[0]),
                scale(body_deltas[face[1]], hit.bary[1]),
            ),
            scale(body_deltas[face[2]], hit.bary[2]),
        );
        deltas[point] = scale(blended, follower_fade(hit.distance, radius));
    }

    // Smoothed over the follower's own mesh, so a piece that sits partly on the
    // body and partly away from it moves as a whole instead of crumpling
    // (`MeshDeform.cpp:495-515`).
    if !follower_faces.is_empty() {
        let topology = Topology::build(follower_positions, follower_faces, FOLLOWER_WELD_DISTANCE);
        let all: Vec<usize> = (0..follower_positions.len()).collect();
        let mut next = deltas.clone();
        for _ in 0..FOLLOWER_SMOOTHING_PASSES {
            for (vertex, neighbours) in topology.neighbours.iter().enumerate() {
                if neighbours.is_empty() {
                    continue;
                }
                let average = neighbours
                    .iter()
                    .map(|&neighbour| deltas[neighbour])
                    .fold([0.0f32; 3], add);
                let average = scale(average, 1.0 / neighbours.len() as f32);
                next[vertex] = add(
                    deltas[vertex],
                    scale(sub(average, deltas[vertex]), FOLLOWER_SMOOTHING_STRENGTH),
                );
            }
            average_welds(&topology, &all, &mut next);
            deltas.copy_from_slice(&next);
        }
    }

    // Where the body swells next to a follower that moved with a part that sank,
    // the follower would end up closer to the body than it was, or inside it: it
    // goes back out to its old gap, along the morphed body's normal there
    // (`MeshDeform.cpp:517-547`).
    let morphed: Vec<[f32; 3]> = body_positions
        .iter()
        .zip(body_deltas)
        .map(|(position, delta)| add(*position, *delta))
        .collect();
    let before = GapSurface::build(body_positions, body_faces);
    let after = GapSurface::build(&morphed, body_faces);
    let gaps_before: Vec<Option<f32>> = follower_positions
        .iter()
        .map(|&position| before.gap(position, radius))
        .collect();
    for _ in 0..FOLLOWER_GAP_PASSES {
        let moved: Vec<[f32; 3]> = follower_positions
            .iter()
            .zip(&deltas)
            .map(|(position, delta)| add(*position, *delta))
            .collect();
        let mut pushed = false;
        for point in 0..follower_positions.len() {
            let (Some(gap_before), Some(gap_after)) =
                (gaps_before[point], after.gap(moved[point], radius))
            else {
                continue;
            };
            if is_near_zero(deltas[point]) {
                continue;
            }
            let lost = gap_before - gap_after;
            if lost <= FOLLOWER_GAP_TOLERANCE {
                continue;
            }
            let Some(hit) = after.closest(moved[point], radius) else {
                continue;
            };
            let face = body_faces[hit.triangle].map(|vertex| vertex as usize);
            let normal = unit(add(
                add(
                    scale(after.normals[face[0]], hit.bary[0]),
                    scale(after.normals[face[1]], hit.bary[1]),
                ),
                scale(after.normals[face[2]], hit.bary[2]),
            ));
            deltas[point] = add(deltas[point], scale(normal, lost));
            pushed = true;
        }
        if !pushed {
            break;
        }
    }
    Ok(deltas)
}

/// Native fade: full motion up to three quarters of the radius, smoothstep to
/// none at the radius (`MeshDeform.cpp:487-490`).
fn follower_fade(distance: f32, radius: f32) -> f32 {
    let inner = radius * FOLLOWER_FULL_MOTION_SHARE;
    let t = ((distance - inner) / (radius - inner)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn is_near_zero(vector: [f32; 3]) -> bool {
    vector.iter().all(|value| value.abs() < ZERO_EPSILON)
}

/// One closest point of a triangle surface, from the shared diagnostic index.
struct Hit {
    triangle: usize,
    position: [f32; 3],
    bary: [f32; 3],
    distance: f32,
}

/// The closest point within `radius`, which the index itself does not bound.
fn closest_hit(
    index: &SurfaceIndex,
    faces: &[[u32; 3]],
    positions: &[[f32; 3]],
    point: [f32; 3],
    radius: f32,
) -> Option<Hit> {
    let nearest = index.closest(point)?;
    if nearest.distance > radius {
        return None;
    }
    let face = faces[nearest.triangle].map(|vertex| vertex as usize);
    let corners = [positions[face[0]], positions[face[1]], positions[face[2]]];
    Some(Hit {
        triangle: nearest.triangle,
        position: nearest.point,
        bary: barycentric_clamped(nearest.point, corners),
        distance: nearest.distance,
    })
}

/// A body surface as the follower gap passes see it: one shared index, its
/// outward vertex normals and the lookup for the body's openings, so the signed
/// gap and the push share a single set of queries. @req DECL0013
struct GapSurface<'mesh> {
    faces: &'mesh [[u32; 3]],
    positions: &'mesh [[f32; 3]],
    index: SurfaceIndex,
    normals: Vec<[f32; 3]>,
    sign: f32,
    open: OpenEdges,
}

impl<'mesh> GapSurface<'mesh> {
    fn build(positions: &'mesh [[f32; 3]], faces: &'mesh [[u32; 3]]) -> Self {
        let (normals, sign) = outward_vertex_normals(positions, faces);
        Self {
            faces,
            positions,
            index: SurfaceIndex::build(positions, faces),
            normals,
            sign,
            open: OpenEdges::build(faces, positions.len()),
        }
    }

    fn closest(&self, point: [f32; 3], radius: f32) -> Option<Hit> {
        closest_hit(&self.index, self.faces, self.positions, point, radius)
    }

    /// `ClippingFixer::SignedDistancesToBody` (`ClippingFixer.cpp:235-320`): the
    /// distance along the outward normal, or the plain distance past an opening
    /// where the normals there point sideways and a point over the hole would
    /// read as inside.
    fn gap(&self, point: [f32; 3], radius: f32) -> Option<f32> {
        let hit = self.closest(point, radius)?;
        let face = self.faces[hit.triangle];
        let bary = hit.bary;
        let corners = face.map(|vertex| vertex as usize);
        let interpolated = add(
            add(
                scale(self.normals[corners[0]], bary[0]),
                scale(self.normals[corners[1]], bary[1]),
            ),
            scale(self.normals[corners[2]], bary[2]),
        );
        let outward = match is_near_zero(interpolated) {
            true => scale(
                face_normal_of([
                    self.positions[corners[0]],
                    self.positions[corners[1]],
                    self.positions[corners[2]],
                ]),
                self.sign,
            ),
            false => interpolated,
        };
        let offset = sub(point, hit.position);
        Some(match past_open_edge(&self.open, face, bary) {
            true => length(offset),
            false => dot(offset, unit(outward)),
        })
    }
}

/// Vertex neighbours along the triangle edges, and the copies of a vertex along
/// split seams (`Topology`, `MeshDeform.cpp:27-60`). @req DECL0013
struct Topology {
    neighbours: Vec<Vec<usize>>,
    welds: Vec<Vec<usize>>,
    weld_of: Vec<Option<usize>>,
}

impl Topology {
    fn build(positions: &[[f32; 3]], faces: &[[u32; 3]], weld_distance: f32) -> Self {
        let mut neighbour_sets = vec![Vec::new(); positions.len()];
        for face in faces {
            let corners = face.map(|vertex| vertex as usize);
            for (a, b) in edge_pairs(&corners) {
                // Native skips the degenerate corner pair of a collapsed triangle.
                if a == b {
                    continue;
                }
                neighbour_sets[a].push(b);
                neighbour_sets[b].push(a);
            }
        }
        // Sorted and deduplicated, so the smoothing sums do not depend on the
        // order of the triangles.
        neighbour_sets.iter_mut().for_each(|list| {
            list.sort_unstable();
            list.dedup();
        });
        let welds = coincident_groups(positions, weld_distance);
        let mut weld_of = vec![None; positions.len()];
        for (group, copies) in welds.iter().enumerate() {
            copies
                .iter()
                .for_each(|&vertex| weld_of[vertex] = Some(group));
        }
        Self {
            neighbours: neighbour_sets,
            welds,
            weld_of,
        }
    }
}

/// Values that a seam's copies average over the native template
/// (`AverageWelds`, `MeshDeform.cpp:62-79`).
trait SeamValue: Copy {
    fn zero() -> Self;
    fn plus(self, other: Self) -> Self;
    fn times(self, factor: f32) -> Self;
}

impl SeamValue for f32 {
    fn zero() -> Self {
        0.0
    }
    fn plus(self, other: Self) -> Self {
        self + other
    }
    fn times(self, factor: f32) -> Self {
        self * factor
    }
}

impl SeamValue for [f32; 3] {
    fn zero() -> Self {
        [0.0; 3]
    }
    fn plus(self, other: Self) -> Self {
        add(self, other)
    }
    fn times(self, factor: f32) -> Self {
        scale(self, factor)
    }
}

/// Gives the copies of each seam vertex in `vertices` their average, so seams do
/// not open. @req DECL0013
fn average_welds<T: SeamValue>(topology: &Topology, vertices: &[usize], values: &mut [T]) {
    let mut done = vec![false; topology.welds.len()];
    for &vertex in vertices {
        let Some(group) = topology.weld_of[vertex] else {
            continue;
        };
        if done[group] {
            continue;
        }
        done[group] = true;
        let copies = &topology.welds[group];
        let summed = copies
            .iter()
            .fold(T::zero(), |sum: T, &copy| sum.plus(values[copy]));
        let average = summed.times(1.0 / copies.len() as f32);
        copies.iter().for_each(|&copy| values[copy] = average);
    }
}

/// Area-weighted vertex normals turned to point away from the body's inside,
/// whatever the winding of its triangles, with the sign that was applied
/// (`ClippingFixer.cpp:139-162,188-206,322-326`). @req DECL0013
fn outward_vertex_normals(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> (Vec<[f32; 3]>, f32) {
    if positions.is_empty() {
        return (Vec::new(), 1.0);
    }
    let sign = outward_sign(positions, faces);
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for face in faces {
        let corners = face.map(|vertex| vertex as usize);
        let normal = face_normal_of([
            positions[corners[0]],
            positions[corners[1]],
            positions[corners[2]],
        ]);
        corners
            .iter()
            .for_each(|&corner| normals[corner] = add(normals[corner], normal));
    }
    (
        normals
            .into_iter()
            .map(|normal| scale(unit(normal), sign))
            .collect(),
        sign,
    )
}

/// `1` when the body's triangles mostly face away from its inside, `-1` when they
/// face into it. The winding order of a NIF body varies, so the centroid decides.
fn outward_sign(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> f32 {
    let centroid = scale(
        positions
            .iter()
            .fold([0.0f32; 3], |sum, &position| add(sum, position)),
        1.0 / positions.len() as f32,
    );
    let (outward, inward) = faces
        .iter()
        .map(|face| {
            let corners = face.map(|vertex| vertex as usize);
            let corners = [
                positions[corners[0]],
                positions[corners[1]],
                positions[corners[2]],
            ];
            let center = scale(
                corners
                    .iter()
                    .fold([0.0f32; 3], |sum, &corner| add(sum, corner)),
                1.0 / 3.0,
            );
            match dot(sub(center, centroid), face_normal_of(corners)) > 0.0 {
                true => (1u32, 0u32),
                false => (0, 1),
            }
        })
        .fold((0u32, 0u32), |(outward, inward), (one, other)| {
            (outward + one, inward + other)
        });
    match outward >= inward {
        true => 1.0,
        false => -1.0,
    }
}

/// Distance along the body's surface to the edge of the covered area: the
/// Dijkstra relaxation of `MeshDeform.cpp:356-392`, crossing the edges between
/// covered and free vertices halfway and stepping between a seam's copies at no
/// cost. Without that zero-cost step the two sides of a split seam would not
/// share the profile.
fn edge_distances(
    positions: &[[f32; 3]],
    topology: &Topology,
    covered: &[bool],
    reach: f32,
) -> Vec<f32> {
    let count = positions.len();
    let mut distance = vec![f32::MAX; count];
    let mut queue = BinaryHeap::new();
    for vertex in 0..count {
        for &neighbour in &topology.neighbours[vertex] {
            if covered[vertex] == covered[neighbour] {
                continue;
            }
            let half = length(sub(positions[vertex], positions[neighbour])) * 0.5;
            if half < distance[vertex] {
                distance[vertex] = half;
                queue.push(QueueEntry {
                    distance: half,
                    vertex,
                });
            }
        }
    }
    while let Some(QueueEntry {
        distance: reached,
        vertex,
    }) = queue.pop()
    {
        if reached > distance[vertex] || reached > reach {
            continue;
        }
        let mut relax = |target: usize, step: f32| {
            let next = reached + step;
            if next < distance[target] {
                distance[target] = next;
                queue.push(QueueEntry {
                    distance: next,
                    vertex: target,
                });
            }
        };
        for &neighbour in &topology.neighbours[vertex] {
            relax(
                neighbour,
                length(sub(positions[vertex], positions[neighbour])),
            );
        }
        if let Some(group) = topology.weld_of[vertex] {
            for &copy in &topology.welds[group] {
                relax(copy, 0.0);
            }
        }
    }
    distance
}

/// Min-heap entry of the geodesic search, ordered like the native
/// `greater<Entry>` comparator (`MeshDeform.cpp:361-363`).
#[derive(Debug, Clone, Copy)]
struct QueueEntry {
    distance: f32,
    vertex: usize,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.distance.total_cmp(&other.distance) == Ordering::Equal && self.vertex == other.vertex
    }
}

impl Eq for QueueEntry {}

impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.vertex.cmp(&self.vertex))
    }
}

impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn validate_positions(
    positions: &[[f32; 3]],
    error: fn(usize) -> ContactError,
) -> Result<(), ContactError> {
    match first_non_finite(positions) {
        Some(vertex) => Err(error(vertex)),
        None => Ok(()),
    }
}

fn validate_faces(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
    error: fn(usize, u32) -> ContactError,
) -> Result<(), ContactError> {
    faces.iter().enumerate().try_for_each(|(triangle, face)| {
        match face
            .iter()
            .find(|&&vertex| vertex as usize >= positions.len())
        {
            Some(&vertex) => Err(error(triangle, vertex)),
            None => Ok(()),
        }
    })
}
