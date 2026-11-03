//! Bind a garment to the body it was authored on.
//!
//! A garment NIF carries the body shape the author fitted it to. Binding each
//! garment vertex to its nearest vertex of that shape and storing the authored
//! offset keeps the authored thickness: a later body morph moves the garment by
//! the same offset from the morphed body vertex instead of collapsing it onto
//! the skin. Positions are frame-agnostic; the caller passes the garment, the
//! reference shape and every body in the same frame.

use std::fmt;

/// One garment vertex pinned to a body vertex with its authored offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Binding {
    /// Index into the reference body shape.
    pub vertex: usize,
    /// Garment position minus the reference vertex at bind time.
    pub offset: [f32; 3],
}

/// Why a conform could not be built or applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformError {
    /// The garment has no vertices.
    EmptyGarment,
    /// The reference body shape has no vertices.
    EmptyReference,
    /// The body does not have the reference shape's vertex count.
    Topology { expected: usize, actual: usize },
}

impl fmt::Display for ConformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(formatter, "the garment has no vertices"),
            Self::EmptyReference => write!(formatter, "the reference body shape has no vertices"),
            Self::Topology { expected, actual } => write!(
                formatter,
                "the body has {actual} vertices, but the garment was bound to a shape with {expected}"
            ),
        }
    }
}

impl std::error::Error for ConformError {}

/// A garment bound to the body shape it was authored on.
///
/// Binding is `O(n log n)` in the reference size; applying is linear in the
/// garment size and does not rebuild the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Conform {
    bindings: Vec<Binding>,
    reference_vertices: usize,
    offset_mean: f32,
    offset_max: f32,
}

/// How garment and body moved between two morphs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    /// Largest movement of a bound body vertex.
    pub body: f32,
    /// Largest movement of the garment.
    pub garment: f32,
    /// Largest difference between a garment movement and its body vertex.
    pub difference: f32,
}

impl Conform {
    /// Pin every garment vertex to its nearest reference vertex.
    pub fn bind(garment: &[[f32; 3]], reference: &[[f32; 3]]) -> Result<Self, ConformError> {
        if garment.is_empty() {
            return Err(ConformError::EmptyGarment);
        }
        if reference.is_empty() {
            return Err(ConformError::EmptyReference);
        }
        let tree = KdTree::build(reference);
        let bindings: Vec<Binding> = garment
            .iter()
            .map(|position| {
                let vertex = tree.nearest(*position);
                Binding {
                    vertex,
                    offset: subtract(*position, reference[vertex]),
                }
            })
            .collect();
        let offset_sum: f32 = bindings.iter().map(|binding| length(binding.offset)).sum();
        let offset_max = bindings
            .iter()
            .map(|binding| length(binding.offset))
            .fold(0.0, f32::max);
        let garment_vertices = bindings.len();
        Ok(Self {
            bindings,
            reference_vertices: reference.len(),
            offset_mean: offset_sum / garment_vertices as f32,
            offset_max,
        })
    }

    /// Per-vertex bindings, parallel to the garment positions.
    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// Mean authored offset between the garment and its reference shape.
    pub fn offset_mean(&self) -> f32 {
        self.offset_mean
    }

    /// Largest authored offset between the garment and its reference shape.
    pub fn offset_max(&self) -> f32 {
        self.offset_max
    }

    /// Place the garment on a morphed body with the reference topology.
    pub fn apply(&self, body: &[[f32; 3]]) -> Result<Vec<[f32; 3]>, ConformError> {
        if body.len() != self.reference_vertices {
            return Err(ConformError::Topology {
                expected: self.reference_vertices,
                actual: body.len(),
            });
        }
        Ok(self
            .bindings
            .iter()
            .map(|binding| add(body[binding.vertex], binding.offset))
            .collect())
    }

    /// Compare how the garment and the body moved between two morphs.
    ///
    /// `difference` is zero when every garment vertex moved exactly with the
    /// body vertex it is bound to, which is the conform gate for a slider change.
    pub fn follow(&self, before: &[[f32; 3]], after: &[[f32; 3]]) -> Result<Follow, ConformError> {
        let garment_before = self.apply(before)?;
        let garment_after = self.apply(after)?;
        let mut follow = Follow {
            body: 0.0,
            garment: 0.0,
            difference: 0.0,
        };
        for (index, binding) in self.bindings.iter().enumerate() {
            let body_delta = subtract(after[binding.vertex], before[binding.vertex]);
            let garment_delta = subtract(garment_after[index], garment_before[index]);
            follow.body = follow.body.max(length(body_delta));
            follow.garment = follow.garment.max(length(garment_delta));
            follow.difference = follow
                .difference
                .max(length(subtract(garment_delta, body_delta)));
        }
        Ok(follow)
    }
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn length(vector: [f32; 3]) -> f32 {
    vector[0]
        .mul_add(
            vector[0],
            vector[1].mul_add(vector[1], vector[2] * vector[2]),
        )
        .sqrt()
}

fn squared_distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    let delta = subtract(left, right);
    delta[0].mul_add(delta[0], delta[1].mul_add(delta[1], delta[2] * delta[2]))
}

/// Static three-dimensional k-d tree over the reference positions.
struct KdTree<'a> {
    points: &'a [[f32; 3]],
    nodes: Vec<KdNode>,
    root: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct KdNode {
    point: usize,
    axis: usize,
    left: Option<usize>,
    right: Option<usize>,
}

impl<'a> KdTree<'a> {
    fn build(points: &'a [[f32; 3]]) -> Self {
        let mut tree = Self {
            points,
            nodes: Vec::with_capacity(points.len()),
            root: None,
        };
        let mut indices: Vec<usize> = (0..points.len()).collect();
        tree.root = tree.build_node(&mut indices, 0);
        tree
    }

    fn build_node(&mut self, indices: &mut [usize], depth: usize) -> Option<usize> {
        if indices.is_empty() {
            return None;
        }
        let middle = indices.len() / 2;
        let axis = depth % 3;
        let points = self.points;
        indices.select_nth_unstable_by(middle, |left, right| {
            points[*left][axis].total_cmp(&points[*right][axis])
        });
        let node = self.nodes.len();
        self.nodes.push(KdNode {
            point: indices[middle],
            axis,
            left: None,
            right: None,
        });
        let (left, rest) = indices.split_at_mut(middle);
        let left = self.build_node(left, depth + 1);
        let right = self.build_node(&mut rest[1..], depth + 1);
        if let Some(entry) = self.nodes.get_mut(node) {
            entry.left = left;
            entry.right = right;
        }
        Some(node)
    }

    fn nearest(&self, target: [f32; 3]) -> usize {
        let mut best = (0usize, f32::INFINITY);
        self.search(self.root, target, &mut best);
        best.0
    }

    fn search(&self, node: Option<usize>, target: [f32; 3], best: &mut (usize, f32)) {
        let Some(index) = node else {
            return;
        };
        let Some(entry) = self.nodes.get(index) else {
            return;
        };
        let Some(point) = self.points.get(entry.point) else {
            return;
        };
        let distance = squared_distance(*point, target);
        if distance < best.1 {
            *best = (entry.point, distance);
        }
        let delta = target[entry.axis] - point[entry.axis];
        let (near, far) = if delta < 0.0 {
            (entry.left, entry.right)
        } else {
            (entry.right, entry.left)
        };
        self.search(near, target, best);
        if delta * delta < best.1 {
            self.search(far, target, best);
        }
    }
}
