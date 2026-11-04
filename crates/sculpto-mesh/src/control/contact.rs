//! Bounded local separation of reduction-created control contacts. @req DECL0013
//!
//! Decimation can fold a boundary ear through a neighbouring triangle. The
//! profile may authorize a single, measured separation instead of rejecting the
//! control: one geometry-derived minimum separating axis, the smaller face moving
//! in full with its immediate neighbours at half strength, the opposing face
//! fixed. Every moved vertex, the chosen axis and the residual state are reported,
//! and the result is rejected when it exceeds the declared bound, inverts or
//! degenerates an incident face, or leaves any contact.
//! Shared physical anchors cannot obey that rigid translation and are refused.
//!
//! Witness detection is not implemented here: it reuses
//! [`crate::check::self_intersection_witnesses`], the same predicate the final
//! qualification counts.
//!
//! No asset name, source path or vertex number is involved; the choice of moving
//! face is by measured area and index only.

use std::fmt;

use crate::check::{Witness, WitnessKind, self_intersection_witnesses};

type Point = [f64; 3];
type Vec3 = [f64; 3];

/// What the profile permits when a finished control still contacts itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactMode {
    /// Any contact terminates qualification.
    Reject,
    /// One bounded local separation may be proposed.
    Separate,
}

impl ContactMode {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Separate => "separate",
        }
    }
}

/// Declared contact policy, in raw source units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactPolicy {
    pub mode: ContactMode,
    /// Separation margin added to the measured overlap.
    pub clearance: f32,
    /// Largest displacement any moved vertex may take.
    pub max_displacement: f32,
    /// Declared pair cap; this implementation additionally restricts repair to one pair.
    pub max_pairs: usize,
}

impl ContactPolicy {
    pub(crate) fn validate(&self) -> Result<(), ContactError> {
        if !self.clearance.is_finite() || self.clearance <= 0.0 {
            return Err(ContactError::InvalidPolicy {
                field: "contact.clearance",
                reason: "expected a positive finite length",
            });
        }
        if !self.max_displacement.is_finite() || self.max_displacement <= 0.0 {
            return Err(ContactError::InvalidPolicy {
                field: "contact.max_displacement",
                reason: "expected a positive finite length",
            });
        }
        if self.clearance > self.max_displacement {
            return Err(ContactError::InvalidPolicy {
                field: "contact.clearance",
                reason: "must not exceed contact.max_displacement",
            });
        }
        if self.max_pairs == 0 {
            return Err(ContactError::InvalidPolicy {
                field: "contact.max_pairs",
                reason: "expected at least one pair",
            });
        }
        Ok(())
    }
}

/// Every way a local separation is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum ContactError {
    /// A policy value is missing, nonfinite or not positive.
    InvalidPolicy {
        field: &'static str,
        reason: &'static str,
    },
    /// A coincident duplicate face cannot be separated by moving geometry.
    DuplicateFace { left: usize, right: usize },
    /// Translating one face would also move a shared anchor of the fixed face.
    SharedFeature { left: usize, right: usize },
    /// More witnessed pairs than the policy allows: coupled or excessive.
    CoupledContacts { pairs: usize, limit: usize },
    /// The smallest analytic separation is larger than the declared bound.
    DisplacementBeyondBound { required: f32, limit: f32 },
    /// Moving the pair would invert or collapse one of its incident faces.
    InvertedIncidentFace { triangle: usize },
    /// Contacts remain after the single proposed correction.
    ResidualContact { pairs: usize },
    /// No numerically usable separating axis exists for the witnessed pair.
    NoSeparatingAxis,
    /// The corrected surface is not the same ordered surface plus displacements.
    InvalidInput,
}

impl fmt::Display for ContactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy { field, reason } => {
                write!(formatter, "contact policy `{field}`: {reason}")
            }
            Self::DuplicateFace { left, right } => write!(
                formatter,
                "triangles {left} and {right} are coincident duplicate faces; geometry separation cannot repair them"
            ),
            Self::SharedFeature { left, right } => write!(
                formatter,
                "triangles {left} and {right} share a physical anchor; rigid separation cannot hold the opposing face fixed"
            ),
            Self::CoupledContacts { pairs, limit } => write!(
                formatter,
                "{pairs} witnessed contact pairs exceed the local repair limit of {limit}"
            ),
            Self::DisplacementBeyondBound { required, limit } => write!(
                formatter,
                "the minimum separating translation {required:.9} exceeds the declared bound {limit:.9}"
            ),
            Self::InvertedIncidentFace { triangle } => write!(
                formatter,
                "separating the pair would invert or collapse incident triangle {triangle}"
            ),
            Self::ResidualContact { pairs } => write!(
                formatter,
                "{pairs} contact pair(s) remain after the single proposed separation"
            ),
            Self::NoSeparatingAxis => {
                write!(formatter, "the contact has no usable separating axis")
            }
            Self::InvalidInput => write!(
                formatter,
                "the control is empty, nonfinite, degenerate or has an invalid triangle index"
            ),
        }
    }
}

impl std::error::Error for ContactError {}

/// One moved vertex, with its displacement share of the applied translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexMove {
    pub vertex: u32,
    pub share: f32,
    pub from: [f32; 3],
    pub to: [f32; 3],
}

/// The measured separation that was applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Separation {
    /// Witnessed pairs on the input surface.
    pub pairs: usize,
    /// The witness addressed by this correction.
    pub witness: Witness,
    /// Triangle that moved in full (the smaller of the pair).
    pub moving_face: usize,
    /// Triangle held fixed.
    pub fixed_face: usize,
    /// Which analytic axis produced the minimum.
    pub axis: &'static str,
    /// Unit translation direction.
    pub normal: [f32; 3],
    /// Measured overlap along `axis`.
    pub overlap: f32,
    /// Declared margin added to the overlap.
    pub clearance: f32,
    /// Full translation applied to the moving face.
    pub translation: f32,
    /// Largest displacement of any vertex.
    pub max_displacement: f32,
    pub moved: Vec<VertexMove>,
    /// Smallest area ratio of an incident face after the correction.
    pub min_incident_area_ratio: f32,
}

/// A separated surface plus the record of what was done.
#[derive(Debug, Clone, PartialEq)]
pub struct SeparationOutcome {
    pub positions: Vec<[f32; 3]>,
    pub separation: Separation,
}

/// Separate the single witnessed pair under the declared policy.
///
/// Returns `Ok(None)` when the surface already has no contact. Any refusal is a
/// definite `Err`: the caller must not retry with other axes or margins.
pub fn separate(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    policy: &ContactPolicy,
) -> Result<Option<SeparationOutcome>, ContactError> {
    policy.validate()?;
    if positions.is_empty()
        || triangles.is_empty()
        || positions.iter().flatten().any(|value| !value.is_finite())
    {
        return Err(ContactError::InvalidInput);
    }
    if triangles
        .iter()
        .flatten()
        .any(|vertex| *vertex as usize >= positions.len())
    {
        return Err(ContactError::InvalidInput);
    }
    if triangles
        .iter()
        .any(|triangle| area(corners(positions, *triangle)) <= 0.0)
    {
        return Err(ContactError::InvalidInput);
    }
    let witnesses = self_intersection_witnesses(positions, triangles);
    let Some(first) = witnesses.first().copied() else {
        return Ok(None);
    };
    if let Some(duplicate) = witnesses
        .iter()
        .find(|witness| witness.kind == WitnessKind::Duplicate)
    {
        return Err(ContactError::DuplicateFace {
            left: duplicate.left,
            right: duplicate.right,
        });
    }
    let pair_limit = policy.max_pairs.min(1);
    if witnesses.len() > pair_limit {
        return Err(ContactError::CoupledContacts {
            pairs: witnesses.len(),
            limit: pair_limit,
        });
    }
    if policy.mode == ContactMode::Reject {
        return Err(ContactError::CoupledContacts {
            pairs: witnesses.len(),
            limit: 0,
        });
    }

    let a = corners(positions, triangles[first.left]);
    let b = corners(positions, triangles[first.right]);
    if a.iter().any(|point| b.contains(point)) {
        return Err(ContactError::SharedFeature {
            left: first.left,
            right: first.right,
        });
    }
    let moving_face = match area(a) <= area(b) {
        true => first.left,
        false => first.right,
    };
    let fixed_face = match moving_face == first.left {
        true => first.right,
        false => first.left,
    };
    let (moving, fixed) = (
        corners(positions, triangles[moving_face]),
        corners(positions, triangles[fixed_face]),
    );
    let (axis, normal, overlap) =
        minimum_axis(moving, fixed).ok_or(ContactError::NoSeparatingAxis)?;
    let required = overlap + f64::from(policy.clearance);
    if required > f64::from(policy.max_displacement) {
        return Err(ContactError::DisplacementBeyondBound {
            required: required as f32,
            limit: policy.max_displacement,
        });
    }

    // The moving face moves in full; vertices of faces sharing a vertex with it
    // move at half strength; the opposing face stays fixed.
    let moving_vertices: Vec<u32> = triangles[moving_face].to_vec();
    let fixed_vertices: Vec<u32> = triangles[fixed_face].to_vec();
    let neighbours = neighbour_vertices(triangles, &moving_vertices, &fixed_vertices);
    let translation: Vec3 = normal.map(|component| component * required);
    let mut updated = positions.to_vec();
    let mut moved = Vec::new();
    for (vertex, share) in moving_vertices
        .iter()
        .map(|vertex| (*vertex, 1.0f32))
        .chain(neighbours.iter().map(|vertex| (*vertex, 0.5f32)))
    {
        let share = f64::from(share);
        let before = positions[vertex as usize];
        let after = [
            (f64::from(before[0]) + translation[0] * share) as f32,
            (f64::from(before[1]) + translation[1] * share) as f32,
            (f64::from(before[2]) + translation[2] * share) as f32,
        ];
        updated[vertex as usize] = after;
        moved.push(VertexMove {
            vertex,
            share: share as f32,
            from: before,
            to: after,
        });
    }
    let max_displacement = moved
        .iter()
        .map(|movement| distance(movement.from, movement.to))
        .fold(0.0f32, f32::max);
    if f64::from(max_displacement) > f64::from(policy.max_displacement) + 1e-9 {
        return Err(ContactError::DisplacementBeyondBound {
            required: max_displacement,
            limit: policy.max_displacement,
        });
    }

    // Incident faces may not invert or collapse.
    let touched: Vec<u32> = moved.iter().map(|movement| movement.vertex).collect();
    let mut min_incident_area_ratio = 1.0f32;
    for (index, triangle) in triangles.iter().enumerate() {
        if !triangle.iter().any(|vertex| touched.contains(vertex)) {
            continue;
        }
        let before = corners(positions, *triangle);
        let after = corners(&updated, *triangle);
        let (before_area, after_area) = (area(before), area(after));
        if after_area <= 0.0 {
            return Err(ContactError::InvertedIncidentFace { triangle: index });
        }
        let ratio = (after_area / before_area) as f32;
        min_incident_area_ratio = min_incident_area_ratio.min(ratio);
        let agreement = dot(unit(normal_of(before)), unit(normal_of(after)));
        if agreement <= 0.0 {
            return Err(ContactError::InvertedIncidentFace { triangle: index });
        }
    }

    let residual = self_intersection_witnesses(&updated, triangles);
    if !residual.is_empty() {
        return Err(ContactError::ResidualContact {
            pairs: residual.len(),
        });
    }
    Ok(Some(SeparationOutcome {
        positions: updated,
        separation: Separation {
            pairs: witnesses.len(),
            witness: first,
            moving_face,
            fixed_face,
            axis,
            normal: normal.map(|component| component as f32),
            overlap: overlap as f32,
            clearance: policy.clearance,
            translation: required as f32,
            max_displacement,
            moved,
            min_incident_area_ratio,
        },
    }))
}

fn corners(positions: &[[f32; 3]], triangle: [u32; 3]) -> [Point; 3] {
    triangle.map(|vertex| positions[vertex as usize].map(f64::from))
}

fn neighbour_vertices(triangles: &[[u32; 3]], moving: &[u32], fixed: &[u32]) -> Vec<u32> {
    let mut neighbours = Vec::new();
    for triangle in triangles {
        if !triangle.iter().any(|vertex| moving.contains(vertex)) {
            continue;
        }
        for vertex in triangle {
            if moving.contains(vertex) || fixed.contains(vertex) || neighbours.contains(vertex) {
                continue;
            }
            neighbours.push(*vertex);
        }
    }
    neighbours.sort_unstable();
    neighbours
}

/// The minimum separating translation over the 13 triangle-pair axes.
fn minimum_axis(a: [Point; 3], b: [Point; 3]) -> Option<(&'static str, Vec3, f64)> {
    let edges_a = [sub(a[1], a[0]), sub(a[2], a[1]), sub(a[0], a[2])];
    let edges_b = [sub(b[1], b[0]), sub(b[2], b[1]), sub(b[0], b[2])];
    let mut axes: Vec<(&'static str, Vec3)> = vec![
        ("face-normal-a", cross(edges_a[0], edges_a[1])),
        ("face-normal-b", cross(edges_b[0], edges_b[1])),
    ];
    for (index, edge_a) in edges_a.iter().enumerate() {
        for (other, edge_b) in edges_b.iter().enumerate() {
            let label: &'static str = match (index, other) {
                (0, 0) => "edge-a0-x-b0",
                (0, 1) => "edge-a0-x-b1",
                (0, 2) => "edge-a0-x-b2",
                (1, 0) => "edge-a1-x-b0",
                (1, 1) => "edge-a1-x-b1",
                (1, 2) => "edge-a1-x-b2",
                (2, 0) => "edge-a2-x-b0",
                (2, 1) => "edge-a2-x-b1",
                _ => "edge-a2-x-b2",
            };
            axes.push((label, cross(*edge_a, *edge_b)));
        }
    }
    let mut best: Option<(&'static str, Vec3, f64)> = None;
    for (label, axis) in axes {
        let size = length(axis);
        if size < 1e-18 {
            continue;
        }
        let unit = axis.map(|component| component / size);
        let (low_a, high_a) = interval(a, unit);
        let (low_b, high_b) = interval(b, unit);
        for (candidate, signed) in [(high_b - low_a, 1.0), (high_a - low_b, -1.0)] {
            if candidate < 0.0 {
                continue;
            }
            let magnitude = candidate;
            match &best {
                Some((_, _, current)) if *current <= magnitude => {}
                _ => {
                    best = Some((label, unit.map(|component| component * signed), magnitude));
                }
            }
        }
    }
    best
}

fn interval(points: [Point; 3], axis: Vec3) -> (f64, f64) {
    let values = points.map(|point| dot(point, axis));
    (
        values.iter().copied().fold(f64::INFINITY, f64::min),
        values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    )
}

fn area(points: [Point; 3]) -> f64 {
    0.5 * length(cross(sub(points[1], points[0]), sub(points[2], points[0])))
}

fn normal_of(points: [Point; 3]) -> Vec3 {
    cross(sub(points[1], points[0]), sub(points[2], points[0]))
}

fn sub(left: Point, right: Point) -> Vec3 {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn cross(left: Vec3, right: Vec3) -> Vec3 {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: Vec3, right: Vec3) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn length(vector: Vec3) -> f64 {
    dot(vector, vector).sqrt()
}

fn unit(vector: Vec3) -> Vec3 {
    let size = length(vector);
    match size > 0.0 {
        true => vector.map(|component| component / size),
        false => [0.0; 3],
    }
}

fn distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    let delta = [
        f64::from(left[0]) - f64::from(right[0]),
        f64::from(left[1]) - f64::from(right[1]),
        f64::from(left[2]) - f64::from(right[2]),
    ];
    length(delta) as f32
}
