//! Triangle-pair intersection counting for fit gates.
//!
//! Vertex-inside tests miss folds and cut-ins: a triangle can pass through a
//! surface between its vertices. This module measures triangle pairs that
//! actually intersect, with a bounding-volume hierarchy over one side so the
//! scan stays near-linear. Coplanar contacts use projected edge crossings and
//! barycentric containment, including boundary touches and coincident faces.

use std::cmp::Ordering;

type Vec3 = [f32; 3];

/// A triangle with a precomputed bounding box.
#[derive(Clone, Copy)]
struct Trie {
    corners: [[f32; 3]; 3],
    min: Vec3,
    max: Vec3,
}

impl Trie {
    fn new(corners: [[f32; 3]; 3]) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for corner in corners {
            for k in 0..3 {
                min[k] = min[k].min(corner[k]);
                max[k] = max[k].max(corner[k]);
            }
        }
        Self { corners, min, max }
    }

    fn overlaps(&self, other: &Trie, epsilon: f32) -> bool {
        (0..3)
            .all(|k| self.min[k] <= other.max[k] + epsilon && other.min[k] <= self.max[k] + epsilon)
    }

    fn centroid(&self, axis: usize) -> f32 {
        (self.corners[0][axis] + self.corners[1][axis] + self.corners[2][axis]) / 3.0
    }
}

/// Median-split BVH over triangles.
struct Bvh {
    nodes: Vec<Node>,
    order: Vec<usize>,
    tries: Vec<Trie>,
}

struct Node {
    min: Vec3,
    max: Vec3,
    /// Left child index, right child index (or triangle range when `count > 0`).
    left: u32,
    right: u32,
    start: u32,
    count: u32,
}

impl Bvh {
    fn build(tries: Vec<Trie>) -> Self {
        let count = tries.len();
        let mut order: Vec<usize> = (0..count).collect();
        let mut nodes = Vec::with_capacity(2 * count.max(1));
        // Iterative median split via an explicit stack of (start, end) ranges.
        nodes.push(Node {
            min: [0.0; 3],
            max: [0.0; 3],
            left: 0,
            right: 0,
            start: 0,
            count: 0,
        });
        let mut stack = vec![(0usize, count, 0usize)];
        while let Some((start, end, node)) = stack.pop() {
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for &t in &order[start..end] {
                for k in 0..3 {
                    min[k] = min[k].min(tries[t].min[k]);
                    max[k] = max[k].max(tries[t].max[k]);
                }
            }
            let extent = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
            let axis = (0..3)
                .max_by(|&a, &b| extent[a].partial_cmp(&extent[b]).unwrap_or(Ordering::Equal))
                .unwrap_or(0);
            let len = end - start;
            if len <= 2 {
                nodes[node] = Node {
                    min,
                    max,
                    left: u32::MAX,
                    right: u32::MAX,
                    start: start as u32,
                    count: len as u32,
                };
                continue;
            }
            let mid = start + len / 2;
            order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
                tries[a]
                    .centroid(axis)
                    .partial_cmp(&tries[b].centroid(axis))
                    .unwrap_or(Ordering::Equal)
            });
            let left = nodes.len();
            nodes.push(Node {
                min: [0.0; 3],
                max: [0.0; 3],
                left: 0,
                right: 0,
                start: 0,
                count: 0,
            });
            let right = nodes.len();
            nodes.push(Node {
                min: [0.0; 3],
                max: [0.0; 3],
                left: 0,
                right: 0,
                start: 0,
                count: 0,
            });
            nodes[node] = Node {
                min,
                max,
                left: left as u32,
                right: right as u32,
                start: 0,
                count: 0,
            };
            stack.push((start, mid, left));
            stack.push((mid, end, right));
        }
        Self {
            nodes,
            order,
            tries,
        }
    }

    fn query(&self, probe: &Trie, epsilon: f32, out: &mut impl FnMut(usize)) {
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(node) = stack.pop() {
            let entry = &self.nodes[node];
            if !(0..3).all(|k| {
                entry.min[k] <= probe.max[k] + epsilon && probe.min[k] <= entry.max[k] + epsilon
            }) {
                continue;
            }
            if entry.count > 0 {
                for &t in &self.order[entry.start as usize..(entry.start + entry.count) as usize] {
                    if self.tries[t].overlaps(probe, epsilon) {
                        out(t);
                    }
                }
            } else {
                stack.push(entry.left as usize);
                stack.push(entry.right as usize);
            }
        }
    }
}

type Vec3d = [f64; 3];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContactScope {
    Any,
    BeyondSharedFeatures,
}

fn sub64(a: Vec3d, b: Vec3d) -> Vec3d {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross64(a: Vec3d, b: Vec3d) -> Vec3d {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot64(a: Vec3d, b: Vec3d) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn to64(v: Vec3) -> Vec3d {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// Cross-product magnitude below which a triangle carries no surface.
const DEGENERATE_SPAN: f64 = 1e-15;

/// Whether a position triangle has zero area: repeated corners, coincident
/// corners or a cross product under [`DEGENERATE_SPAN`] in f64. The same
/// threshold the intersection tests use to skip unsurfaceable triangles.
pub(crate) fn zero_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> bool {
    let normal = cross64(sub64(to64(b), to64(a)), sub64(to64(c), to64(a)));
    dot64(normal, normal).sqrt() < DEGENERATE_SPAN
}

fn orient2d(p: [f64; 2], q: [f64; 2], r: [f64; 2]) -> f64 {
    (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
}

fn segments_cross_2d(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o1 = orient2d(a, b, c);
    let o2 = orient2d(a, b, d);
    let o3 = orient2d(c, d, a);
    let o4 = orient2d(c, d, b);

    ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
}

fn point_in_triangle_2d(p: [f64; 2], tri: &[[f64; 2]; 3]) -> bool {
    let a = tri[0];
    let b = tri[1];
    let c = tri[2];
    let area2 = orient2d(a, b, c);
    if area2.abs() < 1e-15 {
        return false;
    }
    let inv = 1.0 / area2;
    let d1 = orient2d(b, c, p) * inv;
    let d2 = orient2d(c, a, p) * inv;
    let d3 = orient2d(a, b, p) * inv;
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

fn coplanar_triangles_intersect(
    a: &[[f64; 3]; 3],
    b: &[[f64; 3]; 3],
    normal: Vec3d,
    scope: ContactScope,
) -> bool {
    let axis = (0..3)
        .max_by(|&i, &j| {
            normal[i]
                .abs()
                .partial_cmp(&normal[j].abs())
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(2);
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let a_2d = [[a[0][u], a[0][v]], [a[1][u], a[1][v]], [a[2][u], a[2][v]]];
    let b_2d = [[b[0][u], b[0][v]], [b[1][u], b[1][v]], [b[2][u], b[2][v]]];

    for i in 0..3 {
        for j in 0..3 {
            if segments_cross_2d(a_2d[i], a_2d[(i + 1) % 3], b_2d[j], b_2d[(j + 1) % 3]) {
                return true;
            }
        }
    }

    a.iter().zip(&a_2d).any(|(corner, &p)| {
        (scope == ContactScope::Any || !b.contains(corner)) && point_in_triangle_2d(p, &b_2d)
    }) || b.iter().zip(&b_2d).any(|(corner, &p)| {
        (scope == ContactScope::Any || !a.contains(corner)) && point_in_triangle_2d(p, &a_2d)
    })
}

fn triangle_plane_interval(p_t: [f64; 3], h: [f64; 3], eps: f64) -> Option<[f64; 2]> {
    let s = [
        if h[0] > eps {
            1
        } else if h[0] < -eps {
            -1
        } else {
            0
        },
        if h[1] > eps {
            1
        } else if h[1] < -eps {
            -1
        } else {
            0
        },
        if h[2] > eps {
            1
        } else if h[2] < -eps {
            -1
        } else {
            0
        },
    ];
    if (s[0] > 0 && s[1] > 0 && s[2] > 0) || (s[0] < 0 && s[1] < 0 && s[2] < 0) {
        return None;
    }
    if s[0] == 0 && s[1] == 0 && s[2] == 0 {
        return None;
    }

    let interp = |i: usize, j: usize| -> f64 {
        match (s[i], s[j]) {
            (0, _) => p_t[i],
            (_, 0) => p_t[j],
            _ => {
                let denom = h[i] - h[j];
                let factor = if denom.abs() < 1e-15 {
                    0.5
                } else {
                    h[i] / denom
                };
                p_t[i] + (p_t[j] - p_t[i]) * factor
            }
        }
    };

    let (t1, t2) = if s[0] != 0 && s[0] == s[1] {
        (interp(2, 0), interp(2, 1))
    } else if s[1] != 0 && s[1] == s[2] {
        (interp(0, 1), interp(0, 2))
    } else if s[2] != 0 && s[2] == s[0] {
        (interp(1, 2), interp(1, 0))
    } else if s[0] == 0 && s[1] == 0 {
        (p_t[0], p_t[1])
    } else if s[1] == 0 && s[2] == 0 {
        (p_t[1], p_t[2])
    } else if s[2] == 0 && s[0] == 0 {
        (p_t[2], p_t[0])
    } else if s[0] == 0 {
        if s[1] == s[2] {
            (p_t[0], p_t[0])
        } else {
            (p_t[0], interp(1, 2))
        }
    } else if s[1] == 0 {
        if s[0] == s[2] {
            (p_t[1], p_t[1])
        } else {
            (p_t[1], interp(0, 2))
        }
    } else if s[2] == 0 {
        if s[0] == s[1] {
            (p_t[2], p_t[2])
        } else {
            (p_t[2], interp(0, 1))
        }
    } else {
        return None;
    };

    Some([t1.min(t2), t1.max(t2)])
}

/// Whether two triangles intersect (Möller-style interval testing in f64
/// with coplanar 2D overlap fallback).
fn triangles_intersect(a: &Trie, b: &Trie, scope: ContactScope) -> bool {
    let a_corners = [to64(a.corners[0]), to64(a.corners[1]), to64(a.corners[2])];
    let b_corners = [to64(b.corners[0]), to64(b.corners[1]), to64(b.corners[2])];

    let e_a1 = sub64(a_corners[1], a_corners[0]);
    let e_a2 = sub64(a_corners[2], a_corners[0]);
    let n_a = cross64(e_a1, e_a2);
    let len_na = dot64(n_a, n_a).sqrt();
    if len_na < DEGENERATE_SPAN {
        return false;
    }
    let unit_na = [n_a[0] / len_na, n_a[1] / len_na, n_a[2] / len_na];

    let e_b1 = sub64(b_corners[1], b_corners[0]);
    let e_b2 = sub64(b_corners[2], b_corners[0]);
    let n_b = cross64(e_b1, e_b2);
    let len_nb = dot64(n_b, n_b).sqrt();
    if len_nb < DEGENERATE_SPAN {
        return false;
    }
    let unit_nb = [n_b[0] / len_nb, n_b[1] / len_nb, n_b[2] / len_nb];

    let scale_a = dot64(e_a1, e_a1)
        .max(dot64(e_a2, e_a2))
        .max(dot64(
            sub64(a_corners[2], a_corners[1]),
            sub64(a_corners[2], a_corners[1]),
        ))
        .sqrt();
    let scale_b = dot64(e_b1, e_b1)
        .max(dot64(e_b2, e_b2))
        .max(dot64(
            sub64(b_corners[2], b_corners[1]),
            sub64(b_corners[2], b_corners[1]),
        ))
        .sqrt();
    let eps_a = 1e-10 * scale_a.max(1e-9);
    let eps_b = 1e-10 * scale_b.max(1e-9);

    let h_a = [
        dot64(unit_nb, sub64(a_corners[0], b_corners[0])),
        dot64(unit_nb, sub64(a_corners[1], b_corners[0])),
        dot64(unit_nb, sub64(a_corners[2], b_corners[0])),
    ];
    if (h_a[0] > eps_a && h_a[1] > eps_a && h_a[2] > eps_a)
        || (h_a[0] < -eps_a && h_a[1] < -eps_a && h_a[2] < -eps_a)
    {
        return false;
    }

    let h_b = [
        dot64(unit_na, sub64(b_corners[0], a_corners[0])),
        dot64(unit_na, sub64(b_corners[1], a_corners[0])),
        dot64(unit_na, sub64(b_corners[2], a_corners[0])),
    ];
    if (h_b[0] > eps_b && h_b[1] > eps_b && h_b[2] > eps_b)
        || (h_b[0] < -eps_b && h_b[1] < -eps_b && h_b[2] < -eps_b)
    {
        return false;
    }

    let line_dir = cross64(unit_na, unit_nb);
    let len_dir = dot64(line_dir, line_dir).sqrt();

    if h_a.iter().all(|h| h.abs() <= eps_a) && h_b.iter().all(|h| h.abs() <= eps_b) {
        return coplanar_triangles_intersect(&a_corners, &b_corners, unit_na, scope);
    }
    if len_dir <= f64::EPSILON {
        return false;
    }

    let d_line = [
        line_dir[0] / len_dir,
        line_dir[1] / len_dir,
        line_dir[2] / len_dir,
    ];
    let a_t = [
        0.0,
        dot64(sub64(a_corners[1], a_corners[0]), d_line),
        dot64(sub64(a_corners[2], a_corners[0]), d_line),
    ];
    let b_t = [
        dot64(sub64(b_corners[0], a_corners[0]), d_line),
        dot64(sub64(b_corners[1], a_corners[0]), d_line),
        dot64(sub64(b_corners[2], a_corners[0]), d_line),
    ];

    let interval_a = match triangle_plane_interval(a_t, h_a, eps_a) {
        Some(iv) => iv,
        None => return false,
    };
    let interval_b = match triangle_plane_interval(b_t, h_b, eps_b) {
        Some(iv) => iv,
        None => return false,
    };

    let eps_line = 1e-10 * scale_a.min(scale_b).max(1e-9);
    let overlap = [
        interval_a[0].max(interval_b[0]),
        interval_a[1].min(interval_b[1]),
    ];
    if overlap[0] > overlap[1] + eps_line {
        return false;
    }
    match scope {
        ContactScope::Any => true,
        ContactScope::BeyondSharedFeatures => {
            let shared = a_corners
                .iter()
                .zip(a_t)
                .filter(|(corner, _)| b_corners.contains(corner))
                .map(|(_, t)| [t, t])
                .reduce(|a, b| [a[0].min(b[0]), a[1].max(b[1])]);
            match shared {
                Some([start, end]) => overlap[0] < start - eps_line || overlap[1] > end + eps_line,
                None => true,
            }
        }
    }
}

fn triangles(pos: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<Trie> {
    tris.iter()
        .map(|t| Trie::new([pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]]))
        .collect()
}

/// Intersecting triangle pairs (index within each list), for diagnosis.
pub fn intersection_pair_list(
    a_positions: &[[f32; 3]],
    a_triangles: &[[u32; 3]],
    b_positions: &[[f32; 3]],
    b_triangles: &[[u32; 3]],
) -> Vec<(usize, usize)> {
    if a_triangles.is_empty() || b_triangles.is_empty() {
        return Vec::new();
    }
    let a = triangles(a_positions, a_triangles);
    let b = triangles(b_positions, b_triangles);
    let bvh = Bvh::build(b);
    let mut found = Vec::new();
    for (index, probe) in a.iter().enumerate() {
        let mut visit = |candidate: usize| {
            if triangles_intersect(probe, &bvh.tries[candidate], ContactScope::Any) {
                found.push((index, candidate));
            }
        };
        bvh.query(probe, 1e-6, &mut visit);
    }
    found
}

fn point_triangle_distance(p: [f32; 3], tri: &Trie) -> f32 {
    let difference = sub64(p.map(f64::from), closest_triangle_point(p, tri));
    dot64(difference, difference).sqrt() as f32
}

fn closest_triangle_point(p: [f32; 3], tri: &Trie) -> [f64; 3] {
    let p = p.map(f64::from);
    let a = tri.corners[0].map(f64::from);
    let b = tri.corners[1].map(f64::from);
    let c = tri.corners[2].map(f64::from);
    let ab = sub64(b, a);
    let ac = sub64(c, a);
    let ap = sub64(p, a);
    let d1 = dot64(ab, ap);
    let d2 = dot64(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub64(p, b);
    let d3 = dot64(ab, bp);
    let d4 = dot64(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let t = d1 / (d1 - d3);
        let q = [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]];
        return q;
    }
    let cp = sub64(p, c);
    let d5 = dot64(ab, cp);
    let d6 = dot64(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let t = d2 / (d2 - d6);
        let q = [a[0] + t * ac[0], a[1] + t * ac[1], a[2] + t * ac[2]];
        return q;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let q = [
            b[0] + t * (c[0] - b[0]),
            b[1] + t * (c[1] - b[1]),
            b[2] + t * (c[2] - b[2]),
        ];
        return q;
    }
    let denominator = 1.0 / (va + vb + vc);
    let v = vb * denominator;
    let w = vc * denominator;
    [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ]
}

fn box_distance(point: [f32; 3], min: [f32; 3], max: [f32; 3]) -> f32 {
    let mut total = 0.0;
    for k in 0..3 {
        let outside = (min[k] - point[k]).max(point[k] - max[k]).max(0.0);
        total += outside * outside;
    }
    total.sqrt()
}

impl Bvh {
    /// Exact nearest triangle distance by bbox-distance-pruned traversal.
    fn nearest(&self, point: [f32; 3]) -> Option<(usize, f32)> {
        if self.tries.is_empty() {
            return None;
        }
        let mut best = f32::INFINITY;
        let mut triangle = None;
        let mut stack = vec![(
            0usize,
            box_distance(point, self.nodes[0].min, self.nodes[0].max),
        )];
        while let Some((node, bound)) = stack.pop() {
            if bound >= best {
                continue;
            }
            let entry = &self.nodes[node];
            if entry.count > 0 {
                for &t in &self.order[entry.start as usize..(entry.start + entry.count) as usize] {
                    let distance = point_triangle_distance(point, &self.tries[t]);
                    if distance < best {
                        best = distance;
                        triangle = Some(t);
                    }
                }
            } else {
                for child in [entry.left as usize, entry.right as usize] {
                    let child_entry = &self.nodes[child];
                    let child_bound = box_distance(point, child_entry.min, child_entry.max);
                    if child_bound < best {
                        stack.push((child, child_bound));
                    }
                }
            }
        }
        triangle.map(|index| (index, best))
    }
}

/// Distance from each query point to the nearest triangle of a mesh.
pub fn nearest_distances(
    points: &[[f32; 3]],
    mesh_positions: &[[f32; 3]],
    mesh_triangles: &[[u32; 3]],
) -> Vec<f32> {
    let tries = triangles(mesh_positions, mesh_triangles);
    let bvh = Bvh::build(tries);
    points
        .iter()
        .map(|&p| match bvh.nearest(p) {
            Some((_, distance)) => distance,
            None => f32::INFINITY,
        })
        .collect()
}

/// The first triangle a ray meets, with the hit point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Triangle index in the queried triangle list.
    pub triangle: usize,
    /// World-space hit point.
    pub location: [f32; 3],
}

/// Nearest hit of one ray per origin. A direction of zero length hits nothing.
/// @req DECL0002
pub fn first_hits(
    origins: &[[f32; 3]],
    direction: [f32; 3],
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> Vec<Option<RayHit>> {
    let Some(ray) = unit_ray(direction) else {
        return vec![None; origins.len()];
    };
    let bvh = Bvh::build(triangles(positions, faces));
    origins
        .iter()
        .map(|&origin| {
            bvh.first_hit(origin, ray, f64::NEG_INFINITY, f64::INFINITY, &[])
                .map(|(hit, _)| hit)
        })
        .collect()
}

fn unit_ray(direction: [f32; 3]) -> Option<[f64; 3]> {
    let length = direction
        .iter()
        .map(|&c| f64::from(c).powi(2))
        .sum::<f64>()
        .sqrt();
    (length > 0.0 && length.is_finite()).then(|| direction.map(|c| f64::from(c) / length))
}

impl Bvh {
    /// Nearest non-excluded triangle in `(minimum, maximum]` along the unit ray.
    fn first_hit(
        &self,
        origin: [f32; 3],
        ray: [f64; 3],
        minimum: f64,
        maximum: f64,
        excluded: &[usize],
    ) -> Option<(RayHit, f64)> {
        if self.nodes.is_empty() {
            return None;
        }
        let origin = origin.map(f64::from);
        let mut best = maximum;
        let mut hit = None;
        let mut stack = vec![0usize];
        while let Some(node) = stack.pop() {
            let entry = &self.nodes[node];
            if !ray_box(origin, ray, entry.min, entry.max, best) {
                continue;
            }
            if entry.count > 0 {
                for &triangle in
                    &self.order[entry.start as usize..(entry.start + entry.count) as usize]
                {
                    if !excluded.contains(&triangle)
                        && let Some((distance, location)) =
                            ray_triangle(origin, ray, &self.tries[triangle].corners)
                        && distance > minimum
                        && distance <= best
                        && (hit.is_none() || distance < best)
                    {
                        best = distance;
                        hit = Some((
                            RayHit {
                                triangle,
                                location: location.map(|value| value as f32),
                            },
                            distance,
                        ));
                    }
                }
            } else {
                stack.push(entry.left as usize);
                stack.push(entry.right as usize);
            }
        }
        hit
    }
}

/// Whether the ray reaches the box before `limit`.
fn ray_box(origin: [f64; 3], ray: [f64; 3], min: [f32; 3], max: [f32; 3], limit: f64) -> bool {
    let mut near = 0.0f64;
    let mut far = limit;
    for axis in 0..3 {
        let low = f64::from(min[axis]);
        let high = f64::from(max[axis]);
        let direction = ray[axis];
        let origin = origin[axis];
        if direction.abs() < f64::EPSILON {
            if origin < low || origin > high {
                return false;
            }
            continue;
        }
        let mut first = (low - origin) / direction;
        let mut second = (high - origin) / direction;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if near > far {
            return false;
        }
    }
    far >= 0.0
}

/// Möller–Trumbore: the ray parameter and point where it crosses the triangle.
fn ray_triangle(
    origin: [f64; 3],
    ray: [f64; 3],
    corners: &[[f32; 3]; 3],
) -> Option<(f64, [f64; 3])> {
    let corner = |index: usize| corners[index].map(f64::from);
    let [a, b, c] = [corner(0), corner(1), corner(2)];
    let edge_1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let edge_2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        ray[1] * edge_2[2] - ray[2] * edge_2[1],
        ray[2] * edge_2[0] - ray[0] * edge_2[2],
        ray[0] * edge_2[1] - ray[1] * edge_2[0],
    ];
    let determinant = edge_1[0] * cross[0] + edge_1[1] * cross[1] + edge_1[2] * cross[2];
    if determinant.abs() < 1e-12 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let offset = [origin[0] - a[0], origin[1] - a[1], origin[2] - a[2]];
    let u = (offset[0] * cross[0] + offset[1] * cross[1] + offset[2] * cross[2]) * inverse;
    if !(-1e-9..=1.0 + 1e-9).contains(&u) {
        return None;
    }
    let second = [
        offset[1] * edge_1[2] - offset[2] * edge_1[1],
        offset[2] * edge_1[0] - offset[0] * edge_1[2],
        offset[0] * edge_1[1] - offset[1] * edge_1[0],
    ];
    let v = (ray[0] * second[0] + ray[1] * second[1] + ray[2] * second[2]) * inverse;
    if v < -1e-9 || u + v > 1.0 + 1e-9 {
        return None;
    }
    let distance =
        (edge_2[0] * second[0] + edge_2[1] * second[1] + edge_2[2] * second[2]) * inverse;
    if distance < 0.0 {
        return None;
    }
    Some((
        distance,
        [
            origin[0] + ray[0] * distance,
            origin[1] + ray[1] * distance,
            origin[2] + ray[2] * distance,
        ],
    ))
}

/// A shared surface hit keeps the original triangle index. @req DECL0013
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClosestSurfacePoint {
    pub triangle: usize,
    pub point: [f32; 3],
    pub distance: f32,
}

/// Reuse one diagnostic BVH across fitting correspondence iterations. @req DECL0013
pub(crate) struct SurfaceIndex {
    bvh: Bvh,
}

impl SurfaceIndex {
    /// Callers validate finite geometry and triangle indices before construction. @req DECL0013
    pub(crate) fn build(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Self {
        Self {
            bvh: Bvh::build(triangles(positions, faces)),
        }
    }

    pub(crate) fn closest(&self, point: [f32; 3]) -> Option<ClosestSurfacePoint> {
        self.bvh
            .nearest(point)
            .map(|(triangle, distance)| ClosestSurfacePoint {
                triangle,
                point: closest_triangle_point(point, &self.bvh.tries[triangle])
                    .map(|value| value as f32),
                distance,
            })
    }

    /// Reuse the cached tree for native poke and far-side queries. @req DECL0013
    pub(crate) fn first_hit(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        minimum: f32,
        maximum: f32,
        excluded: &[usize],
    ) -> Option<(RayHit, f32)> {
        self.bvh
            .first_hit(
                origin,
                unit_ray(direction)?,
                f64::from(minimum),
                f64::from(maximum),
                excluded,
            )
            .map(|(hit, distance)| (hit, distance as f32))
    }
}

/// Closest points reuse the diagnostic BVH and its f64 triangle predicate. @req DECL0013
pub(crate) fn closest_surface_points(
    points: &[[f32; 3]],
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> Vec<Option<ClosestSurfacePoint>> {
    let index = SurfaceIndex::build(positions, faces);
    points.iter().map(|&point| index.closest(point)).collect()
}

/// Shared acceleration for validated control-surface binding. @req DECL0013
pub(crate) fn nearest_triangle_indices(
    points: &[[f32; 3]],
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> Vec<Option<usize>> {
    let bvh = Bvh::build(triangles(positions, faces));
    points
        .iter()
        .map(|&point| bvh.nearest(point).map(|(index, _)| index))
        .collect()
}

/// Source face indices involved in physical self contacts, using the shared
/// witness semantics including exact-position seams. @req DECL0013
pub fn internally_crossing_triangles(positions: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<u32> {
    let mut flagged = vec![false; tris.len()];
    self_intersection_witnesses(positions, tris)
        .into_iter()
        .for_each(|witness| {
            flagged[witness.left] = true;
            flagged[witness.right] = true;
        });
    flagged
        .iter()
        .enumerate()
        .filter_map(|(i, &f)| f.then_some(i as u32))
        .collect()
}

/// Count intersecting triangle pairs between two meshes.
pub fn intersection_pairs(
    a_positions: &[[f32; 3]],
    a_triangles: &[[u32; 3]],
    b_positions: &[[f32; 3]],
    b_triangles: &[[u32; 3]],
) -> usize {
    if a_triangles.is_empty() || b_triangles.is_empty() {
        return 0;
    }
    let a = triangles(a_positions, a_triangles);
    let b = triangles(b_positions, b_triangles);
    let bvh = Bvh::build(b);
    let mut pairs = 0usize;
    for probe in &a {
        let mut visit = |candidate: usize| {
            if triangles_intersect(probe, &bvh.tries[candidate], ContactScope::Any) {
                pairs += 1;
            }
        };
        bvh.query(probe, 1e-6, &mut visit);
    }
    pairs
}

/// What kind of physical self contact a pair is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessKind {
    /// Triangles intersect beyond any shared vertex or edge.
    Crossing,
    /// Coincident duplicate faces: zero enclosed volume, never a legal surface.
    Duplicate,
}

/// One physical self contact at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Witness {
    pub left: usize,
    pub right: usize,
    pub kind: WitnessKind,
}

/// Every physical self contact at rest. The single authority for both the count
/// and the witness list: `self_intersection_pairs` counts these.
///
/// UV seams are adjacent, duplicate faces are not. @req DECL0013
pub fn self_intersection_witnesses(positions: &[[f32; 3]], tris: &[[u32; 3]]) -> Vec<Witness> {
    if tris.len() < 2 {
        return Vec::new();
    }
    let crate::topology::Identical {
        positions,
        triangles: tris,
        ..
    } = crate::topology::position_topology(positions, tris);
    let face_keys: Vec<_> = tris
        .iter()
        .map(|triangle| {
            let mut key = *triangle;
            key.sort_unstable();
            key
        })
        .collect();
    let tries = triangles(&positions, &tris);
    let bvh = Bvh::build(tries.clone());
    let mut witnesses = Vec::new();
    for (index, probe) in tries.iter().enumerate() {
        let mut visit = |candidate: usize| {
            if candidate <= index {
                return;
            }
            if face_keys[index] == face_keys[candidate] {
                witnesses.push(Witness {
                    left: index,
                    right: candidate,
                    kind: WitnessKind::Duplicate,
                });
            } else if triangles_intersect(
                probe,
                &tries[candidate],
                ContactScope::BeyondSharedFeatures,
            ) {
                witnesses.push(Witness {
                    left: index,
                    right: candidate,
                    kind: WitnessKind::Crossing,
                });
            }
        };
        bvh.query(probe, 1e-6, &mut visit);
    }
    witnesses
}

/// Count physical self contacts; UV seams are adjacent, duplicate faces are not.
pub fn self_intersection_pairs(positions: &[[f32; 3]], tris: &[[u32; 3]]) -> usize {
    self_intersection_witnesses(positions, tris).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_classified_plane_endpoint_has_its_exact_projection() {
        [
            ([0.0, 1.0, 2.0], [1e-10, 1e-3, 1e-18], 2.0),
            ([2.0, 0.0, 1.0], [-1e-18, 1e-10, 1e-3], 2.0),
            ([1.0, 2.0, 0.0], [-1e-3, 1e-18, -1e-10], 2.0),
        ]
        .into_iter()
        .for_each(|(projection, heights, endpoint)| {
            assert_eq!(
                triangle_plane_interval(projection, heights, 1e-12),
                Some([endpoint, endpoint])
            );
        });
    }

    #[test]
    fn render_seams_do_not_become_physical_self_contacts() {
        let positions = [
            [0., 0., 0.],
            [1., 0., 0.],
            [1., 1., 0.],
            [0., 0., 0.],
            [1., 1., 0.],
            [0., 1., 0.],
        ];
        assert_eq!(
            self_intersection_pairs(&positions, &[[0, 1, 2], [3, 4, 5]]),
            0
        );
    }

    #[test]
    fn duplicate_faces_still_fail_after_position_identification() {
        let positions = [
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
        ];
        assert_eq!(
            self_intersection_pairs(&positions, &[[0, 1, 2], [3, 4, 5]]),
            1
        );
        assert_eq!(
            self_intersection_pairs(&positions, &[[0, 1, 2], [2, 1, 0]]),
            1
        );
        let mut separated = positions;
        separated[3..].iter_mut().for_each(|point| point[2] = 1e-7);
        assert_eq!(
            self_intersection_pairs(&separated, &[[0, 1, 2], [3, 4, 5]]),
            0
        );
    }

    fn crossing() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        // Vertical triangle in the xz plane crossing a horizontal one.
        (
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.0, 1.0]],
            vec![[0, 1, 2]],
        )
    }

    #[test]
    fn separated_triangles_do_not_intersect() {
        let (a_pos, a_tri) = crossing();
        let b_pos = vec![[0.0, 2.0, 0.0], [1.0, 2.0, 0.0], [0.5, 2.0, 1.0]];
        assert_eq!(
            intersection_pairs(&a_pos, &a_tri, &b_pos, &b_tri_identity()),
            0
        );
    }

    #[test]
    fn small_separated_coplanar_triangles_do_not_intersect() {
        // Hypotenuse of A is x + y = 0.0005. B is in x + y >= 0.0007.
        // Bounding boxes overlap in [0.00035, 0.0005] x [0.00035, 0.0005].
        let a_pos = vec![[0.0, 0.0, 0.0], [0.0005, 0.0, 0.0], [0.0, 0.0005, 0.0]];
        let b_pos = vec![
            [0.00035, 0.00035, 0.0],
            [0.0005, 0.00035, 0.0],
            [0.00035, 0.0005, 0.0],
        ];
        let tri = vec![[0, 1, 2]];
        assert_eq!(intersection_pairs(&a_pos, &tri, &b_pos, &tri), 0);
    }

    #[test]
    fn coincident_triangles_and_boundary_contacts_intersect() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let triangles = [[0, 1, 2]];
        assert_eq!(
            intersection_pairs(&positions, &triangles, &positions, &triangles),
            1
        );
        let touching = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        assert_eq!(
            intersection_pairs(&positions, &triangles, &touching, &triangles),
            1
        );
        let adjacent = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]];
        assert_eq!(
            intersection_pairs(&positions, &triangles, &adjacent, &triangles),
            1
        );
    }

    fn b_tri_identity() -> Vec<[u32; 3]> {
        vec![[0, 1, 2]]
    }

    #[test]
    fn small_thin_crossing_triangles_intersect() {
        // Triangle A in z=0 plane, size 0.5mm.
        let a_pos = vec![
            [-0.00025, -0.00025, 0.0],
            [0.00025, -0.00025, 0.0],
            [0.0, 0.00025, 0.0],
        ];
        // Triangle B in x=0 plane, size 0.5mm, crossing edge of A at (0, -0.00025, 0).
        let b_pos = vec![
            [0.0, -0.0004, -0.00025],
            [0.0, 0.0, -0.00025],
            [0.0, -0.0002, 0.00025],
        ];
        let tri = vec![[0, 1, 2]];
        assert_eq!(intersection_pairs(&a_pos, &tri, &b_pos, &tri), 1);
    }

    #[test]
    fn reversed_winding_triangles_intersect() {
        // Same as crossing_triangles_intersect, but with reversed winding on both.
        let a_pos = vec![[0.5, 0.0, 1.2], [0.5, 1.0, 0.2], [0.5, -1.0, 0.2]];
        let b_pos = vec![[0.5, 0.5, 1.5], [1.0, 0.5, 0.5], [0.0, 0.5, 0.5]];
        assert_eq!(
            intersection_pairs(&a_pos, &b_tri_identity(), &b_pos, &b_tri_identity()),
            1
        );
    }

    #[test]
    fn scale_and_translation_invariance() {
        // Base test cases scaled by 1000x and translated by large offset.
        let scale = 1000.0;
        let offset = [150.0, -250.0, 300.0];
        let transform = |pts: &[[f32; 3]]| -> Vec<[f32; 3]> {
            pts.iter()
                .map(|p| {
                    [
                        p[0] * scale + offset[0],
                        p[1] * scale + offset[1],
                        p[2] * scale + offset[2],
                    ]
                })
                .collect()
        };

        // Separated coplanar remains separated:
        let a_pos = vec![[0.0, 0.0, 0.0], [0.0005, 0.0, 0.0], [0.0, 0.0005, 0.0]];
        let b_pos = vec![
            [0.00035, 0.00035, 0.0],
            [0.0005, 0.00035, 0.0],
            [0.00035, 0.0005, 0.0],
        ];
        let a_xf = transform(&a_pos);
        let b_xf = transform(&b_pos);
        assert_eq!(
            intersection_pairs(&a_xf, &b_tri_identity(), &b_xf, &b_tri_identity()),
            0
        );

        // Crossing remains crossing:
        let c_a = vec![[0.5, -1.0, 0.2], [0.5, 1.0, 0.2], [0.5, 0.0, 1.2]];
        let c_b = vec![[0.0, 0.5, 0.5], [1.0, 0.5, 0.5], [0.5, 0.5, 1.5]];
        let ca_xf = transform(&c_a);
        let cb_xf = transform(&c_b);
        assert_eq!(
            intersection_pairs(&ca_xf, &b_tri_identity(), &cb_xf, &b_tri_identity()),
            1
        );
    }

    #[test]
    fn crossing_triangles_intersect() {
        // Vertical sheet in the x=0.5 plane pierced by a horizontal sheet.
        let a_pos = vec![[0.5, -1.0, 0.2], [0.5, 1.0, 0.2], [0.5, 0.0, 1.2]];
        let b_pos = vec![[0.0, 0.5, 0.5], [1.0, 0.5, 0.5], [0.5, 0.5, 1.5]];
        assert_eq!(
            intersection_pairs(&a_pos, &b_tri_identity(), &b_pos, &b_tri_identity()),
            1
        );
    }

    #[test]
    fn coplanar_overlap_is_detected_by_vertices() {
        let a_pos = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 2.0, 0.0]];
        let b_pos = vec![[0.5, 0.5, 0.0], [1.5, 0.5, 0.0], [1.0, 1.5, 0.0]];
        assert_eq!(
            intersection_pairs(&a_pos, &b_tri_identity(), &b_pos, &b_tri_identity()),
            1
        );
    }

    #[test]
    fn folded_sheet_counts_self_pairs() {
        // Two opposite-facing sheets crossing through each other.
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.2, -0.1, 0.2],
            [0.2, 0.1, 0.2],
            [0.8, 0.1, 0.2],
        ];
        let tris = vec![[0, 1, 2], [3, 4, 5]];
        assert!(self_intersection_pairs(&positions, &tris) >= 1);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn nearest_distance_is_exact() {
        let pos = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let tris = vec![[0u32, 1, 2]];
        let queries = vec![[0.2, 0.2, 2.0], [0.2, 0.2, 0.0], [2.0, 0.0, 0.0]];
        let distances = nearest_distances(&queries, &pos, &tris);
        assert!((distances[0] - 2.0).abs() < 1e-5, "above: {}", distances[0]);
        assert!(distances[1].abs() < 1e-5, "inside: {}", distances[1]);
        assert!(
            (distances[2] - 1.0).abs() < 1e-5,
            "corner: {}",
            distances[2]
        );
    }
}
