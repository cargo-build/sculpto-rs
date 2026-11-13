//! Explicit seam targets with finite mesh-edge support; render indices stay unchanged.
//! The caller supplies targets in the source frame and retains all other channels.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fmt;

use crate::topology::key;

/// Required lengths, in the same units as positions and targets. @req DECL0007
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamOptions {
    /// Mesh-edge distance at which displacement reaches zero.
    pub collar_radius: f32,
    /// Largest permitted anchor displacement; no silent clipping.
    pub max_shift: f32,
}

impl SeamOptions {
    /// Reject invalid explicit settings before building a distance graph. @req DECL0007
    pub fn validate(&self) -> Result<(), SeamError> {
        [self.collar_radius, self.max_shift]
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
            .then_some(())
            .ok_or(SeamError::InvalidOptions)
    }
}

/// An original render-vertex target; coincident aliases share it. @req DECL0007
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamAnchor {
    /// Original source vertex, never a welded-mesh ordinal.
    pub vertex: usize,
    /// Desired point in the source frame.
    pub target: [f32; 3],
}

/// Derived positions for adapters retaining all other channels. @req DECL0007
#[derive(Debug, Clone, PartialEq)]
pub struct SeamFit {
    /// Original order; outside-support values are copied exactly.
    pub positions: Vec<[f32; 3]>,
    /// Changed original IDs in increasing order.
    pub changed_vertices: Vec<usize>,
    /// Largest applied displacement in caller units.
    pub max_shift: f64,
}

/// Invalid geometry or contradictory constraints; inputs are never mutated. @req DECL0007
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamError {
    /// Required lengths are not finite and positive.
    InvalidOptions,
    /// Source positions or anchors are empty.
    EmptyInput,
    /// A source point or target contains a nonfinite coordinate.
    NonFinite { vertex: usize },
    /// A triangle or anchor refers outside the source positions.
    InvalidIndex { vertex: usize },
    /// Targets disagree at one exactly coincident position.
    ConflictingTargets { vertex: usize },
    /// An anchor exceeds the explicit displacement guard.
    ShiftLimit { vertex: usize },
    /// An output coordinate cannot be represented as finite f32.
    NonFiniteOutput { vertex: usize },
}

impl fmt::Display for SeamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOptions => {
                f.write_str("collar_radius and max_shift must be finite and positive")
            }
            Self::EmptyInput => f.write_str("source positions and seam anchors must be nonempty"),
            Self::NonFinite { vertex } => {
                write!(f, "nonfinite source or target at vertex {vertex}")
            }
            Self::InvalidIndex { vertex } => write!(f, "vertex {vertex} is outside the source"),
            Self::ConflictingTargets { vertex } => write!(
                f,
                "conflicting seam targets at vertex {vertex} or its aliases"
            ),
            Self::ShiftLimit { vertex } => {
                write!(f, "seam target at vertex {vertex} exceeds max_shift")
            }
            Self::NonFiniteOutput { vertex } => {
                write!(f, "nonfinite fitted position at vertex {vertex}")
            }
        }
    }
}

impl std::error::Error for SeamError {}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Visit {
    distance: f64,
    vertex: usize,
    owner: usize,
}

impl Eq for Visit {}

impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.owner.cmp(&self.owner))
            .then_with(|| other.vertex.cmp(&self.vertex))
    }
}

impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(b)).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Move anchors exactly and fade displacement over shortest mesh-edge distance.
/// Exact-position aliases share graph nodes without changing render topology;
/// equal-distance ties select the lowest source representative. @req DECL0007
///
/// # Errors
/// Refuses invalid settings/data/indices, conflicting aliases, excessive anchor
/// shifts and unrepresentable output. No length or frame default is inferred.
pub fn fit_seam(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    anchors: &[SeamAnchor],
    options: &SeamOptions,
) -> Result<SeamFit, SeamError> {
    options.validate()?;
    if positions.is_empty() || anchors.is_empty() {
        return Err(SeamError::EmptyInput);
    }
    positions
        .iter()
        .enumerate()
        .try_for_each(
            |(vertex, point)| match point.iter().all(|value| value.is_finite()) {
                true => Ok(()),
                false => Err(SeamError::NonFinite { vertex }),
            },
        )?;
    triangles.iter().flatten().try_for_each(|&vertex| {
        match (vertex as usize) < positions.len() {
            true => Ok(()),
            false => Err(SeamError::InvalidIndex {
                vertex: vertex as usize,
            }),
        }
    })?;
    let mut identities = HashMap::new();
    let nodes: Vec<usize> = positions
        .iter()
        .enumerate()
        .map(|(index, point)| *identities.entry(key(*point)).or_insert(index))
        .collect();
    let mut adjacency = vec![Vec::new(); positions.len()];
    triangles.iter().for_each(|&[a, b, c]| {
        [(a, b), (b, c), (c, a)].iter().for_each(|&(a, b)| {
            let (a, b) = (nodes[a as usize], nodes[b as usize]);
            let length = distance(positions[a], positions[b]);
            adjacency[a].push((b, length));
            adjacency[b].push((a, length));
        });
    });
    let mut targets = vec![None; positions.len()];
    let mut heap = BinaryHeap::new();
    anchors.iter().try_for_each(|anchor| {
        let vertex = anchor.vertex;
        let original = positions
            .get(vertex)
            .ok_or(SeamError::InvalidIndex { vertex })?;
        if !anchor.target.iter().all(|value| value.is_finite()) {
            return Err(SeamError::NonFinite { vertex });
        }
        if distance(*original, anchor.target) > f64::from(options.max_shift) {
            return Err(SeamError::ShiftLimit { vertex });
        }
        let node = nodes[vertex];
        match targets[node] {
            Some(target) if key(target) != key(anchor.target) => {
                return Err(SeamError::ConflictingTargets { vertex });
            }
            Some(_) => (),
            None => {
                targets[node] = Some(anchor.target);
                heap.push(Visit {
                    distance: 0.0,
                    vertex: node,
                    owner: node,
                });
            }
        }
        Ok(())
    })?;
    let mut nearest = vec![None; positions.len()];
    while let Some(visit) = heap.pop() {
        if nearest[visit.vertex].is_some() || visit.distance >= f64::from(options.collar_radius) {
            continue;
        }
        nearest[visit.vertex] = Some(visit);
        adjacency[visit.vertex]
            .iter()
            .for_each(|&(vertex, length)| {
                if nearest[vertex].is_none() {
                    heap.push(Visit {
                        distance: visit.distance + length,
                        vertex,
                        owner: visit.owner,
                    });
                }
            });
    }
    let fitted = positions
        .iter()
        .enumerate()
        .map(|(vertex, &original)| {
            let Some(visit) = nearest[nodes[vertex]] else {
                return Ok(original);
            };
            let target = targets[visit.owner].ok_or(SeamError::ConflictingTargets { vertex })?;
            let point = match visit.distance == 0.0 {
                true => target,
                false => {
                    let t = visit.distance / f64::from(options.collar_radius);
                    let influence = 1.0 - t * t * (3.0 - 2.0 * t);
                    std::array::from_fn(|axis| {
                        (f64::from(original[axis])
                            + influence
                                * (f64::from(target[axis])
                                    - f64::from(positions[visit.owner][axis])))
                            as f32
                    })
                }
            };
            match point.iter().all(|value| value.is_finite()) {
                true => Ok(point),
                false => Err(SeamError::NonFiniteOutput { vertex }),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let changed_vertices = positions
        .iter()
        .zip(&fitted)
        .enumerate()
        .filter_map(|(index, (old, new))| (old != new).then_some(index))
        .collect();
    let max_shift = positions
        .iter()
        .zip(&fitted)
        .map(|(a, b)| distance(*a, *b))
        .fold(0.0, f64::max);
    Ok(SeamFit {
        positions: fitted,
        changed_vertices,
        max_shift,
    })
}
