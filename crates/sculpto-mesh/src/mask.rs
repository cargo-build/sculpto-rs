//! Vertex masks share 1=locked storage across conform, surface fitting and weight copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::check;

const CHANGE_EPSILON: f32 = 1e-6;
const WELD_EPSILON: f32 = 1e-4;
const WELD_EPSILON_RATIO: f32 = 0.01;

/// Mask data is validated once so every consumer gets finite values in [0, 1]. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct VertexMask(Vec<f32>);

/// Invalid authored input remains distinct from an unmatched transfer. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub enum MaskError {
    Value { vertex: usize, value: f32 },
    Index { vertex: usize, vertices: usize },
    Length { expected: usize, actual: usize },
    Position { vertex: usize },
    Triangle { triangle: usize, vertex: u32 },
    Strength(f32),
    Distance(f32),
}

impl fmt::Display for MaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value { vertex, value } => {
                write!(f, "mask vertex {vertex} has invalid value {value}")
            }
            Self::Index { vertex, vertices } => {
                write!(f, "mask vertex {vertex} exceeds {vertices} vertices")
            }
            Self::Length { expected, actual } => {
                write!(f, "mask needs {expected} values, got {actual}")
            }
            Self::Position { vertex } => write!(f, "mask geometry vertex {vertex} is not finite"),
            Self::Triangle { triangle, vertex } => write!(
                f,
                "mask triangle {triangle} references missing vertex {vertex}"
            ),
            Self::Strength(value) => write!(f, "mask smoothing strength {value} is outside [0, 1]"),
            Self::Distance(value) => write!(
                f,
                "mask transfer distance {value} is not finite and nonnegative"
            ),
        }
    }
}

impl std::error::Error for MaskError {}

impl VertexMask {
    /// An explicitly unmasked mesh; empty storage is valid for an empty mesh. @req DECL0013
    pub fn zeros(vertices: usize) -> Self {
        Self(vec![0.0; vertices])
    }

    /// Reject invalid values rather than silently clamping authored state. @req DECL0013
    pub fn from_values(values: Vec<f32>) -> Result<Self, MaskError> {
        values
            .iter()
            .enumerate()
            .try_for_each(|(index, &value)| validate_value(index, value))?;
        Ok(Self(values))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }
    pub fn get(&self, vertex: usize) -> Option<f32> {
        self.0.get(vertex).copied()
    }

    /// Validate edits before mutating the existing mask. @req DECL0013
    pub fn set(&mut self, vertex: usize, value: f32) -> Result<(), MaskError> {
        validate_value(vertex, value)?;
        let vertices = self.len();
        let slot = self
            .0
            .get_mut(vertex)
            .ok_or(MaskError::Index { vertex, vertices })?;
        *slot = value;
        Ok(())
    }

    pub fn invert(&mut self) {
        self.0.iter_mut().for_each(|value| *value = 1.0 - *value);
    }
    pub fn clear(&mut self) {
        self.0.fill(0.0);
    }

    /// Preserve the native sparse representation, including partial mask values. @req DECL0013
    pub fn masked(&self) -> BTreeMap<usize, f32> {
        self.0
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, value)| *value != 0.0)
            .collect()
    }

    /// An empty freedom vector means no locks, as in native FixWeights. @req DECL0013
    pub fn freedom(&self) -> Vec<f32> {
        match self.0.iter().any(|&value| value > 0.0) {
            true => self.0.iter().map(|value| 1.0 - value).collect(),
            false => Vec::new(),
        }
    }

    pub fn grow(&mut self, positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Result<usize, MaskError> {
        self.grow_shrink(positions, faces, true)
    }

    pub fn shrink(
        &mut self,
        positions: &[[f32; 3]],
        faces: &[[u32; 3]],
    ) -> Result<usize, MaskError> {
        self.grow_shrink(positions, faces, false)
    }

    fn grow_shrink(
        &mut self,
        positions: &[[f32; 3]],
        faces: &[[u32; 3]],
        grow: bool,
    ) -> Result<usize, MaskError> {
        let topology = MaskTopology::build(positions, faces, self.len())?;
        let to = match grow {
            true => 1.0,
            false => 0.0,
        };
        let seeds: Vec<usize> = self
            .0
            .iter()
            .enumerate()
            .filter_map(
                |(index, &value)| match (grow && value > 0.0) || (!grow && value == 0.0) {
                    true => Some(index),
                    false => None,
                },
            )
            .collect();
        let welded: BTreeSet<usize> = seeds
            .iter()
            .flat_map(|&index| topology.welded[index].iter().copied())
            .filter(|&index| self.0[index] != to)
            .collect();
        let changes = match welded.is_empty() {
            true => seeds
                .iter()
                .flat_map(|&index| topology.adjacent[index].iter().copied())
                .filter(|&index| self.0[index] != to)
                .collect(),
            false => welded,
        };
        let count = changes.len();
        changes.into_iter().for_each(|index| self.0[index] = to);
        Ok(count)
    }

    /// Smooth from one snapshot; weld averaging follows native peer-only traversal. @req DECL0013
    pub fn smooth(
        &mut self,
        positions: &[[f32; 3]],
        faces: &[[u32; 3]],
        strength: f32,
    ) -> Result<usize, MaskError> {
        match strength.is_finite() && (0.0..=1.0).contains(&strength) {
            true => {}
            false => return Err(MaskError::Strength(strength)),
        }
        let topology = MaskTopology::build(positions, faces, self.len())?;
        let smoothed: Vec<f32> = self
            .0
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let neighbors: Vec<usize> = topology.adjacent[index]
                    .iter()
                    .copied()
                    .filter(|&other| other != index)
                    .collect();
                match neighbors.is_empty() {
                    true => value,
                    false => {
                        value
                            + strength
                                * (neighbors.iter().map(|&other| self.0[other]).sum::<f32>()
                                    / neighbors.len() as f32
                                    - value)
                    }
                }
            })
            .collect();
        let next: Vec<f32> = smoothed
            .iter()
            .enumerate()
            .map(|(index, &value)| match topology.welded[index].is_empty() {
                true => value,
                false => {
                    topology.welded[index]
                        .iter()
                        .map(|&other| smoothed[other])
                        .sum::<f32>()
                        / topology.welded[index].len() as f32
                }
            })
            .collect();
        let mut changed = 0;
        self.0.iter_mut().zip(next).for_each(|(old, value)| {
            if (value - *old).abs() > CHANGE_EPSILON {
                *old = value;
                changed += 1;
            }
        });
        Ok(changed)
    }
}

fn validate_value(vertex: usize, value: f32) -> Result<(), MaskError> {
    match value.is_finite() && (0.0..=1.0).contains(&value) {
        true => Ok(()),
        false => Err(MaskError::Value { vertex, value }),
    }
}

fn validate_positions(positions: &[[f32; 3]], expected: usize) -> Result<(), MaskError> {
    match positions.len() == expected {
        true => {}
        false => {
            return Err(MaskError::Length {
                expected,
                actual: positions.len(),
            });
        }
    }
    positions
        .iter()
        .enumerate()
        .try_for_each(
            |(vertex, point)| match point.iter().all(|value| value.is_finite()) {
                true => Ok(()),
                false => Err(MaskError::Position { vertex }),
            },
        )
}

struct MaskTopology {
    welded: Vec<Vec<usize>>,
    adjacent: Vec<BTreeSet<usize>>,
}

impl MaskTopology {
    fn build(
        positions: &[[f32; 3]],
        faces: &[[u32; 3]],
        expected: usize,
    ) -> Result<Self, MaskError> {
        validate_positions(positions, expected)?;
        faces.iter().enumerate().try_for_each(|(triangle, face)| {
            face.iter()
                .try_for_each(|&vertex| match (vertex as usize) < positions.len() {
                    true => Ok(()),
                    false => Err(MaskError::Triangle { triangle, vertex }),
                })
        })?;
        let scale = positions
            .iter()
            .flatten()
            .map(|value| value.abs())
            .fold(0.0f32, f32::max);
        let epsilon = WELD_EPSILON * WELD_EPSILON_RATIO * scale;
        let mut sorted: Vec<usize> = (0..positions.len()).collect();
        sorted.sort_by(|&left, &right| {
            positions[left][0]
                .total_cmp(&positions[right][0])
                .then(left.cmp(&right))
        });
        let mut used = vec![false; sorted.len()];
        let mut welded = vec![Vec::new(); positions.len()];
        for (rank, &index) in sorted.iter().enumerate() {
            if used[rank] {
                continue;
            }
            let matches: Vec<(usize, usize)> = sorted
                .iter()
                .enumerate()
                .skip(rank + 1)
                .take_while(|(_, other)| positions[**other][0] - positions[index][0] < epsilon)
                .filter(|(other_rank, other)| {
                    !used[*other_rank]
                        && (positions[index][1] - positions[**other][1]).abs() < epsilon
                        && (positions[index][2] - positions[**other][2]).abs() < epsilon
                })
                .map(|(rank, &vertex)| (rank, vertex))
                .collect();
            if matches.is_empty() {
                continue;
            }
            let group: Vec<usize> = std::iter::once(index)
                .chain(matches.iter().map(|&(_, vertex)| vertex))
                .collect();
            matches.iter().for_each(|&(rank, _)| used[rank] = true);
            group.iter().for_each(|&vertex| {
                welded[vertex] = group
                    .iter()
                    .copied()
                    .filter(|&other| other != vertex)
                    .collect()
            });
        }
        let mut adjacent = vec![BTreeSet::new(); positions.len()];
        faces.iter().for_each(|face| {
            [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])]
                .into_iter()
                .for_each(|(left, right)| {
                    let left = left as usize;
                    let right = right as usize;
                    std::iter::once(left)
                        .chain(welded[left].iter().copied())
                        .for_each(|left| {
                            std::iter::once(right)
                                .chain(welded[right].iter().copied())
                                .for_each(|right| {
                                    adjacent[left].insert(right);
                                    adjacent[right].insert(left);
                                });
                        });
                });
        });
        Ok(Self { welded, adjacent })
    }
}

/// Unmatched targets and discarded invalid facets are observable, never inferred. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct MaskTransfer {
    pub mask: VertexMask,
    pub matched: Vec<bool>,
    pub skipped_triangles: usize,
}

/// Copy from the closest facet corner, not a barycentric blend or global vertex. @req DECL0013
pub fn transfer_mask(
    source_positions: &[[f32; 3]],
    source_faces: &[[u32; 3]],
    source_mask: &VertexMask,
    target_positions: &[[f32; 3]],
    max_distance: f32,
) -> Result<MaskTransfer, MaskError> {
    validate_positions(source_positions, source_mask.len())?;
    validate_positions(target_positions, target_positions.len())?;
    match max_distance.is_finite() && max_distance >= 0.0 {
        true => {}
        false => return Err(MaskError::Distance(max_distance)),
    }
    let faces: Vec<[u32; 3]> = source_faces
        .iter()
        .copied()
        .filter(|face| {
            face.iter()
                .all(|&index| (index as usize) < source_positions.len())
        })
        .collect();
    let skipped_triangles = source_faces.len() - faces.len();
    let hits = check::closest_surface_points(target_positions, source_positions, &faces);
    let mut values = vec![0.0; target_positions.len()];
    let mut matched = vec![false; target_positions.len()];
    hits.iter().enumerate().for_each(|(index, hit)| {
        let Some(hit) = hit.filter(|hit| hit.distance <= max_distance) else {
            return;
        };
        let face = faces[hit.triangle];
        let corner = face
            .iter()
            .copied()
            .enumerate()
            .min_by(|(left_order, left), (right_order, right)| {
                squared_distance(hit.point, source_positions[*left as usize])
                    .total_cmp(&squared_distance(
                        hit.point,
                        source_positions[*right as usize],
                    ))
                    .then(left_order.cmp(right_order))
            })
            .map(|(_, corner)| corner as usize)
            .expect("a triangle has three corners");
        values[index] = source_mask.0[corner];
        matched[index] = true;
    });
    Ok(MaskTransfer {
        mask: VertexMask::from_values(values)?,
        matched,
        skipped_triangles,
    })
}

fn squared_distance(left: [f32; 3], right: [f32; 3]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| (f64::from(*left) - f64::from(right)).powi(2))
        .sum()
}

/// Measure invariance only at fully locked vertices; fractional masks may move. @req DECL0013
pub fn masked_vertex_max_delta(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    mask: &VertexMask,
) -> Result<f32, MaskError> {
    validate_positions(before, mask.len())?;
    validate_positions(after, mask.len())?;
    Ok(mask
        .0
        .iter()
        .enumerate()
        .filter(|(_, value)| **value == 1.0)
        .map(|(index, _)| squared_distance(before[index], after[index]).sqrt() as f32)
        .fold(0.0f32, f32::max))
}
