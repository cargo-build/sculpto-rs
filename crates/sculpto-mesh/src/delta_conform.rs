//! BodySlide's per-slider delta transfer (`Automorph::GenerateResultDiff`).
//!
//! A slider moves a subset of body vertices. The garment does not simply follow
//! its nearest body vertex: every body vertex within a proximity radius of a
//! garment vertex contributes the slider offset by inverse-distance weight, the
//! nearest of them are capped, and a garment vertex whose closest body vertex
//! did not move stays put. Axis gates, a lock mask and solid mode (one per-axis
//! extreme move for the whole garment) come from the reference.
//!
//! The proximity cache is built once per body/garment pair
//! ([`ProximityCache::build`]) and scored per slider ([`generate_delta`]), so a
//! whole slider set reuses one radius query. Positions stay frame-agnostic like
//! [`crate::conform`]; the reference composes mesh-to-model transforms, whose
//! linear part [`DeltaOptions::transform`] carries.
//!
//! Divergences from the reference: vertex indices are `usize` rather than the
//! `u16` keys of `DiffDataSets`, so there is no 65535-vertex cap; the reference
//! mask is a shape-global buffer behind an enable flag, here it is an optional
//! owned per-vertex weight list.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::point_query::PointIndex;

/// Inverse-distance weight of an exact match, so a zero distance stays finite.
const EXACT_MATCH_WEIGHT: f32 = 1000.0;

/// Nearly-zero threshold of nifly's `Vector3::IsZero(true)` (`Object3d.hpp:16`).
const NEARLY_ZERO: f32 = 0.0001;

/// Proximity radius used when the caller sets none (`Automorph.h:96`).
const DEFAULT_RADIUS: f32 = 10.0;

/// Neighbour cap used when the caller sets none (`Automorph.h:99`).
const DEFAULT_MAX_RESULTS: usize = 10;

/// How far a garment vertex looks for body vertices to follow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProximityOptions {
    /// Inclusive radius; zero uses the native insertion-tree nearest-search mode.
    pub radius: f32,
}

impl Default for ProximityOptions {
    fn default() -> Self {
        Self {
            radius: DEFAULT_RADIUS,
        }
    }
}

/// One body vertex a garment vertex follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Neighbor {
    /// Index into the body vertices the cache was built from.
    pub vertex: usize,
    /// Euclidean distance between the garment and this body vertex.
    pub distance: f32,
    /// The reference body vertex `x`, which `no_squeeze` compares against.
    pub reference_x: f32,
}

/// Every body neighbour of every garment vertex, nearest first.
///
/// Build once per body/garment pair: the radius query is the expensive half of
/// a slider transfer and one cache serves every slider of a set.
#[derive(Debug, Clone, PartialEq)]
pub struct ProximityCache {
    body_vertices: usize,
    neighbors: Vec<Vec<Neighbor>>,
}

impl ProximityCache {
    /// Index every body vertex within the radius of every garment vertex.
    ///
    /// `excluded` body vertices never enter the cache, so an exclusion also
    /// survives the neighbour cap of [`generate_delta`].
    ///
    /// # Errors
    ///
    /// Fails on a negative or non-finite radius, an empty body or garment, an
    /// excluded index outside the body, and a non-finite position: a broken
    /// neighbour would silently drop the garment vertex.
    pub fn build(
        body: &[[f32; 3]],
        garment: &[[f32; 3]],
        options: &ProximityOptions,
        excluded: Option<&BTreeSet<usize>>,
    ) -> Result<Self, DeltaConformError> {
        if !options.radius.is_finite() || options.radius < 0.0 {
            return Err(DeltaConformError::InvalidRadius {
                radius: options.radius,
            });
        }
        if body.is_empty() {
            return Err(DeltaConformError::EmptyBody);
        }
        if garment.is_empty() {
            return Err(DeltaConformError::EmptyGarment);
        }
        let stray_exclusion = excluded
            .and_then(|excluded| excluded.iter().copied().find(|index| *index >= body.len()));
        if let Some(index) = stray_exclusion {
            return Err(DeltaConformError::ExcludedOutOfRange {
                index,
                body_vertices: body.len(),
            });
        }
        let broken_body = body
            .iter()
            .position(|position| !position.iter().all(|value| value.is_finite()));
        if let Some(vertex) = broken_body {
            return Err(DeltaConformError::NonFiniteBodyPosition { vertex });
        }
        let broken_garment = garment
            .iter()
            .position(|position| !position.iter().all(|value| value.is_finite()));
        if let Some(vertex) = broken_garment {
            return Err(DeltaConformError::NonFiniteGarmentPosition { vertex });
        }

        let index = match options.radius == 0.0 {
            true => PointIndex::build_native(body),
            false => PointIndex::build(body),
        };
        let mut neighbors = Vec::with_capacity(garment.len());
        for position in garment {
            let mut row = Vec::new();
            let candidates = match options.radius == 0.0 {
                true => index.zero_radius_neighbors(*position),
                false => index.within_radius(*position, options.radius),
            };
            for (vertex, distance) in candidates {
                if excluded.is_some_and(|excluded| excluded.contains(&vertex)) {
                    continue;
                }
                let Some(point) = body.get(vertex) else {
                    return Err(DeltaConformError::NeighborOutOfRange {
                        vertex,
                        body_vertices: body.len(),
                    });
                };
                row.push(Neighbor {
                    vertex,
                    distance,
                    reference_x: point[0],
                });
            }
            neighbors.push(row);
        }
        Ok(Self {
            body_vertices: body.len(),
            neighbors,
        })
    }

    /// Body vertex count this cache indexes.
    pub fn body_vertices(&self) -> usize {
        self.body_vertices
    }

    /// Garment vertex count this cache covers.
    pub fn garment_vertices(&self) -> usize {
        self.neighbors.len()
    }

    /// Retained neighbours of one garment vertex, nearest first.
    pub fn neighbors(&self, garment_vertex: usize) -> Option<&[Neighbor]> {
        self.neighbors.get(garment_vertex).map(Vec::as_slice)
    }
}

/// Knobs of one slider transfer (`Automorph::GenerateResultDiff` arguments).
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaOptions {
    /// Per-garment-vertex lock weight, `1.0` locked and `0.0` free.
    ///
    /// The reference scales a generated move by `1 - mask[i]`.
    pub mask: Option<Vec<f32>>,
    /// Neighbours blended per garment vertex; the cap applies after exclusion.
    pub max_results: usize,
    /// Drop X moves that push against the reference vertex side.
    pub no_squeeze: bool,
    /// Apply one per-axis extreme move to every unmasked garment vertex.
    pub solid_mode: bool,
    /// Transfer the body offset on the X axis.
    pub axis_x: bool,
    /// Transfer the body offset on the Y axis.
    pub axis_y: bool,
    /// Transfer the body offset on the Z axis.
    pub axis_z: bool,
    /// Column-major linear part of the mesh-to-model transform.
    pub transform: Option<[[f32; 3]; 3]>,
}

impl Default for DeltaOptions {
    fn default() -> Self {
        Self {
            mask: None,
            max_results: DEFAULT_MAX_RESULTS,
            no_squeeze: false,
            solid_mode: false,
            axis_x: true,
            axis_y: true,
            axis_z: true,
            transform: None,
        }
    }
}

impl DeltaOptions {
    /// `1 - mask[i]`, or `1.0` without a mask.
    ///
    /// [`generate_delta`] validates the mask length before scoring any vertex,
    /// so indexing stays inside the garment.
    fn unmasked_share(&self, garment_vertex: usize) -> f32 {
        match &self.mask {
            Some(mask) => 1.0 - mask[garment_vertex],
            None => 1.0,
        }
    }
}

/// Why a proximity cache or a slider transfer was refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeltaConformError {
    /// The radius is negative or not finite.
    InvalidRadius { radius: f32 },
    /// The body has no vertices to follow.
    EmptyBody,
    /// The garment has no vertices to move.
    EmptyGarment,
    /// An excluded body vertex index is outside the body.
    ExcludedOutOfRange { index: usize, body_vertices: usize },
    /// A body vertex position is not finite.
    NonFiniteBodyPosition { vertex: usize },
    /// A garment vertex position is not finite.
    NonFiniteGarmentPosition { vertex: usize },
    /// The radius query returned an index outside the body.
    NeighborOutOfRange { vertex: usize, body_vertices: usize },
    /// The neighbour cap is zero, so no offset could be blended.
    ZeroNeighborCap,
    /// The mask length differs from the garment vertex count.
    MaskLength { expected: usize, actual: usize },
    /// A mask weight is outside `0.0..=1.0` or not finite.
    MaskWeight { vertex: usize, weight: f32 },
    /// A transform entry is not finite.
    NonFiniteTransform { column: usize, row: usize },
    /// A body offset is keyed outside the body.
    DiffOutOfRange { index: usize, body_vertices: usize },
    /// A body offset component is not finite.
    NonFiniteDiff { vertex: usize },
}

impl fmt::Display for DeltaConformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRadius { radius } => write!(
                formatter,
                "the proximity radius {radius} is not a finite non-negative length"
            ),
            Self::EmptyBody => write!(formatter, "the body has no vertices to follow"),
            Self::EmptyGarment => write!(formatter, "the garment has no vertices to move"),
            Self::ExcludedOutOfRange {
                index,
                body_vertices,
            } => write!(
                formatter,
                "excluded body vertex {index} is outside the {body_vertices} body vertices"
            ),
            Self::NonFiniteBodyPosition { vertex } => {
                write!(formatter, "body vertex {vertex} has a non-finite position")
            }
            Self::NonFiniteGarmentPosition { vertex } => write!(
                formatter,
                "garment vertex {vertex} has a non-finite position"
            ),
            Self::NeighborOutOfRange {
                vertex,
                body_vertices,
            } => write!(
                formatter,
                "the radius query returned body vertex {vertex}, outside the {body_vertices} body vertices"
            ),
            Self::ZeroNeighborCap => write!(formatter, "the neighbour cap must be at least one"),
            Self::MaskLength { expected, actual } => write!(
                formatter,
                "the mask has {actual} weights for {expected} garment vertices"
            ),
            Self::MaskWeight { vertex, weight } => write!(
                formatter,
                "garment vertex {vertex} has mask weight {weight}, outside 0.0..=1.0"
            ),
            Self::NonFiniteTransform { column, row } => write!(
                formatter,
                "the transform entry at column {column}, row {row} is not finite"
            ),
            Self::DiffOutOfRange {
                index,
                body_vertices,
            } => write!(
                formatter,
                "body offset key {index} is outside the {body_vertices} body vertices"
            ),
            Self::NonFiniteDiff { vertex } => {
                write!(formatter, "body vertex {vertex} has a non-finite offset")
            }
        }
    }
}

impl std::error::Error for DeltaConformError {}

/// Transfer one slider's body offsets onto the garment.
///
/// Only garment vertices that actually move appear in the map, mirroring the
/// reference diff set: a cancelled, fully masked or nearly-zero move is an
/// absent entry, never a zero offset.
///
/// # Errors
///
/// Fails on a zero neighbour cap, a mask whose length or weights do not fit the
/// garment, a non-finite transform entry, and a body offset keyed outside the
/// body or carrying a non-finite component.
pub fn generate_delta(
    cache: &ProximityCache,
    body_diffs: &BTreeMap<usize, [f32; 3]>,
    options: &DeltaOptions,
) -> Result<BTreeMap<usize, [f32; 3]>, DeltaConformError> {
    if options.max_results == 0 {
        return Err(DeltaConformError::ZeroNeighborCap);
    }
    validate_mask(cache, options)?;
    validate_transform(options)?;
    validate_diffs(cache, body_diffs)?;

    if options.solid_mode {
        return Ok(solid_delta(cache, body_diffs, options));
    }

    let mut delta = BTreeMap::new();
    for (garment_vertex, neighbors) in cache.neighbors.iter().enumerate() {
        let Some(movement) = blended_move(neighbors, body_diffs, options) else {
            continue;
        };
        let movement = scale(movement, options.unmasked_share(garment_vertex));
        if is_nearly_zero(movement) {
            continue;
        }
        delta.insert(garment_vertex, apply_transform(movement, options.transform));
    }
    Ok(delta)
}

/// Solid mode: the per-axis extremes of every generated move, applied to all.
fn solid_delta(
    cache: &ProximityCache,
    body_diffs: &BTreeMap<usize, [f32; 3]>,
    options: &DeltaOptions,
) -> BTreeMap<usize, [f32; 3]> {
    let generated: Vec<[f32; 3]> = cache
        .neighbors
        .iter()
        .enumerate()
        .filter_map(|(garment_vertex, neighbors)| {
            let movement = blended_move(neighbors, body_diffs, options)?;
            let movement = scale(movement, options.unmasked_share(garment_vertex));
            (!is_nearly_zero(movement)).then_some(movement)
        })
        .collect();
    if generated.is_empty() {
        return BTreeMap::new();
    }

    // The reference keeps the largest positive and the smallest negative value
    // per axis, then adds the negative side, so one move spans the slider.
    let solid: [f32; 3] = std::array::from_fn(|axis| {
        let high = generated
            .iter()
            .map(|movement| movement[axis])
            .filter(|value| *value >= 0.0)
            .fold(0.0f32, f32::max);
        let low = generated
            .iter()
            .map(|movement| movement[axis])
            .filter(|value| *value < 0.0)
            .fold(0.0f32, f32::min);
        high + low
    });

    cache
        .neighbors
        .iter()
        .enumerate()
        .filter_map(|(garment_vertex, _)| {
            let movement = scale(solid, options.unmasked_share(garment_vertex));
            (!is_nearly_zero(movement))
                .then(|| (garment_vertex, apply_transform(movement, options.transform)))
        })
        .collect()
}

/// Inverse-distance blend of one garment vertex, `None` when it must not move.
///
/// The reference cancels the vertex when its closest neighbour has no offset for
/// this slider (`Automorph.cpp:605-609`); farther unmoved neighbours drop out.
fn blended_move(
    neighbors: &[Neighbor],
    body_diffs: &BTreeMap<usize, [f32; 3]>,
    options: &DeltaOptions,
) -> Option<[f32; 3]> {
    let capped = &neighbors[..neighbors.len().min(options.max_results)];
    let mut inverse_distances = Vec::with_capacity(capped.len());
    let mut effects = Vec::with_capacity(capped.len());
    for (position, neighbor) in capped.iter().enumerate() {
        let Some(diff) = body_diffs.get(&neighbor.vertex) else {
            if position == 0 {
                return None;
            }
            continue;
        };
        let inverse = if neighbor.distance == 0.0 {
            EXACT_MATCH_WEIGHT
        } else {
            1.0 / neighbor.distance
        };
        inverse_distances.push(inverse);
        effects.push(axis_effect(*diff, neighbor.reference_x, options));
    }
    let total: f32 = inverse_distances.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let movement = inverse_distances.iter().zip(&effects).fold(
        [0.0f32; 3],
        |accumulated, (inverse, effect)| {
            let weight = inverse / total;
            [
                accumulated[0] + effect[0] * weight,
                accumulated[1] + effect[1] * weight,
                accumulated[2] + effect[2] * weight,
            ]
        },
    );
    Some(movement)
}

/// Body offset one neighbour contributes after the axis and squeeze gates.
fn axis_effect(diff: [f32; 3], reference_x: f32, options: &DeltaOptions) -> [f32; 3] {
    let x = if options.axis_x && passes_squeeze(diff[0], reference_x, options.no_squeeze) {
        diff[0]
    } else {
        0.0
    };
    let y = if options.axis_y { diff[1] } else { 0.0 };
    let z = if options.axis_z { diff[2] } else { 0.0 };
    [x, y, z]
}

/// `no_squeeze` keeps an X move only when it points away from the body axis.
fn passes_squeeze(diff_x: f32, reference_x: f32, no_squeeze: bool) -> bool {
    if !no_squeeze {
        return true;
    }
    (diff_x > 0.0 && reference_x > 0.0) || (diff_x < 0.0 && reference_x < 0.0)
}

/// Multiply every component of a move, the reference mask application.
fn scale(movement: [f32; 3], factor: f32) -> [f32; 3] {
    [
        movement[0] * factor,
        movement[1] * factor,
        movement[2] * factor,
    ]
}

/// `Vector3::IsZero(true)`: every component below the nifly epsilon.
fn is_nearly_zero(movement: [f32; 3]) -> bool {
    movement
        .iter()
        .all(|component| component.abs() < NEARLY_ZERO)
}

/// Apply the column-major linear part of the mesh-to-model transform.
fn apply_transform(movement: [f32; 3], transform: Option<[[f32; 3]; 3]>) -> [f32; 3] {
    let Some(transform) = transform else {
        return movement;
    };
    std::array::from_fn(|row| {
        (0..3)
            .map(|column| transform[column][row] * movement[column])
            .sum::<f32>()
    })
}

/// The mask must match the garment and stay a weight in `0.0..=1.0`.
fn validate_mask(cache: &ProximityCache, options: &DeltaOptions) -> Result<(), DeltaConformError> {
    let Some(mask) = options.mask.as_deref() else {
        return Ok(());
    };
    if mask.len() != cache.garment_vertices() {
        return Err(DeltaConformError::MaskLength {
            expected: cache.garment_vertices(),
            actual: mask.len(),
        });
    }
    let invalid = mask
        .iter()
        .copied()
        .enumerate()
        .find(|(_, weight)| !(0.0..=1.0).contains(weight));
    if let Some((vertex, weight)) = invalid {
        return Err(DeltaConformError::MaskWeight { vertex, weight });
    }
    Ok(())
}

/// Every transform entry has to survive the arithmetic it joins.
fn validate_transform(options: &DeltaOptions) -> Result<(), DeltaConformError> {
    let Some(transform) = &options.transform else {
        return Ok(());
    };
    let invalid = transform.iter().enumerate().find_map(|(column, entries)| {
        entries
            .iter()
            .position(|value| !value.is_finite())
            .map(|row| (column, row))
    });
    if let Some((column, row)) = invalid {
        return Err(DeltaConformError::NonFiniteTransform { column, row });
    }
    Ok(())
}

/// Body offsets address body vertices of this cache with finite components.
fn validate_diffs(
    cache: &ProximityCache,
    body_diffs: &BTreeMap<usize, [f32; 3]>,
) -> Result<(), DeltaConformError> {
    let stray_key = body_diffs
        .keys()
        .copied()
        .find(|index| *index >= cache.body_vertices());
    if let Some(index) = stray_key {
        return Err(DeltaConformError::DiffOutOfRange {
            index,
            body_vertices: cache.body_vertices(),
        });
    }
    let broken_offset = body_diffs
        .iter()
        .find(|entry| !entry.1.iter().all(|value| value.is_finite()))
        .map(|entry| *entry.0);
    if let Some(vertex) = broken_offset {
        return Err(DeltaConformError::NonFiniteDiff { vertex });
    }
    Ok(())
}
