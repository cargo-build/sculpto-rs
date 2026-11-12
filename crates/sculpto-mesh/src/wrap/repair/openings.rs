//! Source-bound complete-loop candidates; final geometry guards remain authoritative. @req DECL0013

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use glam::DVec3;

use crate::check::intersection_pair_list;
use crate::clip_fix::ClipFixError;
use crate::control::{ControlDiagnostics, SourceSurface};
use crate::mask::VertexMask;
use crate::topology::key;
use crate::wrap::{FitOptions, Mesh, weld};

pub(super) struct OpeningSurface {
    groups: Vec<Option<usize>>,
    representatives: Vec<usize>,
    faces: Vec<[u32; 3]>,
    neighbors: Vec<Vec<usize>>,
    loops: Vec<Vec<usize>>,
    boundary_of: Vec<Option<usize>>,
}

impl OpeningSurface {
    /// Unsupported source topology is unavailable, never an inferred opening. @req DECL0013
    pub(super) fn build(source: Mesh<'_>) -> Option<Self> {
        let physical = SourceSurface::identify(source.0, source.1).ok()?;
        let diagnostic =
            ControlDiagnostics::measure(physical.positions(), physical.triangles()).ok()?;
        if diagnostic.non_manifold_vertices != 0
            || diagnostic.non_manifold_edges != 0
            || diagnostic.orientation_conflicts != 0
            || diagnostic.loops.is_empty()
            || diagnostic
                .loops
                .iter()
                .any(|boundary| !boundary.closed || boundary.vertices.len() < 3)
        {
            return None;
        }
        let index: HashMap<_, _> = physical
            .positions()
            .iter()
            .enumerate()
            .map(|(group, &point)| (key(point), group))
            .collect();
        let groups = source
            .0
            .iter()
            .map(|&p| index.get(&key(p)).copied())
            .collect();
        let mut boundary_of = vec![None; physical.positions().len()];
        let loops: Vec<Vec<usize>> = diagnostic
            .loops
            .iter()
            .map(|boundary| boundary.vertices.iter().map(|&v| v as usize).collect())
            .collect();
        for (loop_id, vertices) in loops.iter().enumerate() {
            for &vertex in vertices {
                if boundary_of[vertex].replace(loop_id).is_some() {
                    return None;
                }
            }
        }
        let graph = weld(physical.positions(), physical.triangles(), 0.0);
        Some(Self {
            groups,
            representatives: physical
                .vertex_sources()
                .iter()
                .map(|&v| v as usize)
                .collect(),
            faces: physical.triangles().to_vec(),
            neighbors: graph.neighbours,
            loops,
            boundary_of,
        })
    }

    /// Every witnessed face must belong to one complete original boundary loop.
    /// Locks scale the field; incompatible rim anchors make it unavailable. @req DECL0013
    pub(super) fn candidate(
        &self,
        current: Mesh<'_>,
        body: Mesh<'_>,
        lock: Option<&VertexMask>,
        options: &FitOptions,
        direction: DVec3,
    ) -> Result<Option<Vec<[f32; 3]>>, ClipFixError> {
        if !positive_faces(current) || !positive_faces(body) {
            return Ok(None);
        }
        let points: Vec<_> = self
            .representatives
            .iter()
            .map(|&v| point(current.0[v]))
            .collect();
        if self
            .representatives
            .iter()
            .map(|&v| key(current.0[v]))
            .collect::<HashSet<_>>()
            .len()
            != points.len()
        {
            return Ok(None);
        }
        let mut freedom = vec![1.0_f64; points.len()];
        for (vertex, group) in self.groups.iter().enumerate() {
            let Some(group) = *group else {
                continue;
            };
            if current.0[vertex] != current.0[self.representatives[group]] {
                return Ok(None);
            }
            let weight = match options
                .rigid_parts
                .get(vertex)
                .is_some_and(|&part| part >= 0)
            {
                true => 0.0,
                false => 1.0 - f64::from(lock.map_or(0.0, |m| m.as_slice()[vertex])),
            };
            freedom[group] = freedom[group].min(weight);
        }
        let pairs = intersection_pair_list(current.0, current.1, body.0, body.1);
        if pairs.is_empty() {
            return Ok(None);
        }
        let mut targets: Vec<Option<f64>> = vec![None; self.loops.len()];
        for (garment_face, body_face) in pairs {
            let mut selected = None;
            for vertex in self.faces[garment_face] {
                if let Some(loop_id) = self.boundary_of[vertex as usize] {
                    match selected {
                        None => selected = Some(loop_id),
                        Some(previous) if previous == loop_id => {}
                        Some(_) => return Ok(None),
                    }
                }
            }
            let Some(loop_id) = selected else {
                return Ok(None);
            };
            let target = body.1[body_face]
                .iter()
                .map(|&v| point(body.0[v as usize]).dot(direction))
                .fold(f64::NEG_INFINITY, f64::max);
            targets[loop_id] = Some(targets[loop_id].map_or(target, |old| old.max(target)));
        }
        let fields = targets.iter().filter(|target| target.is_some()).count();
        let strain = f64::from(options.max_strain);
        let slope = (strain / (1.0 + strain)) / fields as f64;
        let mut shifts = vec![0.0; points.len()];
        for (loop_id, target) in targets.into_iter().enumerate() {
            let Some(target) = target else {
                continue;
            };
            let seeds = &self.loops[loop_id];
            if seeds.iter().any(|&v| freedom[v] == 0.0) {
                return Ok(None);
            }
            let lowest = seeds
                .iter()
                .map(|&v| points[v].dot(direction))
                .fold(f64::INFINITY, f64::min);
            let height = target + f64::from(options.min_gap) - lowest;
            if height <= 0.0 {
                return Ok(None);
            }
            let distances = graph_distances(&points, &self.neighbors, seeds);
            shifts
                .iter_mut()
                .zip(distances)
                .for_each(|(shift, distance)| {
                    *shift += (height - slope * distance).max(0.0);
                });
        }
        let mut output = current.0.to_vec();
        for (vertex, group) in self.groups.iter().enumerate() {
            let Some(group) = *group else {
                continue;
            };
            let shift = shifts[group] * freedom[group];
            if shift == 0.0 {
                continue;
            }
            output[vertex] = std::array::from_fn(|axis| match direction[axis] == 0.0 {
                true => current.0[vertex][axis],
                false => (f64::from(current.0[vertex][axis]) + direction[axis] * shift) as f32,
            });
            if !output[vertex].iter().all(|value| value.is_finite()) {
                return Err(ClipFixError::NonFiniteOutput { vertex });
            }
        }
        if self
            .representatives
            .iter()
            .map(|&v| key(output[v]))
            .collect::<HashSet<_>>()
            .len()
            != points.len()
        {
            return Ok(None);
        }
        Ok(Some(output))
    }
}

/// Check every retained edge, including short edges omitted by reporting thresholds. @req DECL0013
pub(super) fn within_strain(before: Mesh<'_>, after: &[[f32; 3]], limit: f32) -> bool {
    positive_faces((after, before.1))
        && before.1.iter().all(|&[a, b, c]| {
            [(a, b), (b, c), (c, a)].into_iter().all(|(a, b)| {
                let old = point(before.0[a as usize]).distance(point(before.0[b as usize]));
                let new = point(after[a as usize]).distance(point(after[b as usize]));
                old > 0.0 && new > 0.0 && (new / old).max(old / new) - 1.0 <= f64::from(limit)
            })
        })
}

fn positive_faces(mesh: Mesh<'_>) -> bool {
    mesh.1.iter().all(|&face| {
        let [a, b, c] = face.map(|v| point(mesh.0[v as usize]));
        (b - a).cross(c - a).length_squared() > 0.0
    })
}

fn point(value: [f32; 3]) -> DVec3 {
    DVec3::from(value.map(f64::from))
}

#[derive(Clone, Copy, Debug)]
struct Visit {
    distance: f64,
    vertex: usize,
}
impl PartialEq for Visit {
    fn eq(&self, other: &Self) -> bool {
        self.distance.total_cmp(&other.distance) == Ordering::Equal && self.vertex == other.vertex
    }
}
impl Eq for Visit {}
impl Ord for Visit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .distance
            .total_cmp(&self.distance)
            .then_with(|| other.vertex.cmp(&self.vertex))
    }
}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn graph_distances(points: &[DVec3], neighbors: &[Vec<usize>], seeds: &[usize]) -> Vec<f64> {
    let mut distances = vec![f64::INFINITY; points.len()];
    let mut queue = BinaryHeap::new();
    seeds.iter().for_each(|&vertex| {
        distances[vertex] = 0.0;
        queue.push(Visit {
            distance: 0.0,
            vertex,
        });
    });
    while let Some(Visit { distance, vertex }) = queue.pop() {
        if distance > distances[vertex] {
            continue;
        }
        for &next in &neighbors[vertex] {
            let candidate = distance + points[vertex].distance(points[next]);
            if candidate < distances[next] {
                distances[next] = candidate;
                queue.push(Visit {
                    distance: candidate,
                    vertex: next,
                });
            }
        }
    }
    distances
}

#[cfg(test)]
mod tests;
