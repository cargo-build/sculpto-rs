//! Shared point-neighborhood queries; source indices remain stable.

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn squared_distance(left: [f32; 3], right: [f32; 3]) -> f32 {
    let delta = subtract(left, right);
    delta[0].mul_add(delta[0], delta[1].mul_add(delta[1], delta[2] * delta[2]))
}

/// Static three-dimensional k-d tree over the reference positions.
pub(crate) struct PointIndex<'points> {
    points: &'points [[f32; 3]],
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

impl<'points> PointIndex<'points> {
    /// Radius neighbors use reference distance order before any exclusion/cap. @req DECL0013
    pub(crate) fn within_radius(&self, target: [f32; 3], radius: f32) -> Vec<(usize, f32)> {
        let mut found = Vec::new();
        self.search_radius(self.root, target, f64::from(radius).powi(2), &mut found);
        found.sort_by(|left, right| left.1.total_cmp(&right.1).then(left.0.cmp(&right.0)));
        found
    }

    fn search_radius(
        &self,
        node: Option<usize>,
        target: [f32; 3],
        radius_squared: f64,
        found: &mut Vec<(usize, f32)>,
    ) {
        let Some(index) = node else { return };
        let entry = self.nodes[index];
        let point = self.points[entry.point];
        let distance_squared = point
            .iter()
            .zip(target)
            .map(|(left, right)| (f64::from(*left) - f64::from(right)).powi(2))
            .sum::<f64>();
        if distance_squared <= radius_squared {
            found.push((entry.point, distance_squared.sqrt() as f32));
        }
        let delta = f64::from(target[entry.axis]) - f64::from(point[entry.axis]);
        let (near, far) = match delta < 0.0 {
            true => (entry.left, entry.right),
            false => (entry.right, entry.left),
        };
        self.search_radius(near, target, radius_squared, found);
        if delta * delta <= radius_squared {
            self.search_radius(far, target, radius_squared, found);
        }
    }

    pub(crate) fn build(points: &'points [[f32; 3]]) -> Self {
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

    pub(crate) fn nearest(&self, target: [f32; 3]) -> usize {
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
