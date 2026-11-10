//! Triangle and vector primitives shared by the fitting and contact surfaces.
//!
//! The fitting solver (`wrap`) and the contact morph (`contact_morph`) measure
//! the same meshes, so weld groups, clamped barycentrics, the open-edge lookup
//! and the vector arithmetic have one owner here. A seam, an edge crossing or a
//! clamped corner then cannot disagree between the two owners. @req DECL0013

use std::collections::HashMap;

/// Groups of points closer than `tolerance` around each first ungrouped point,
/// on a spatial hash (`CoincidentGroups`, `MeshAnalysis.cpp:244-288`). @req DECL0013
pub(crate) fn coincident_groups(points: &[[f32; 3]], tolerance: f32) -> Vec<Vec<usize>> {
    let cell = tolerance.max(1e-6);
    let key = |point: [f32; 3]| -> (i64, i64, i64) {
        (
            (point[0] / cell).floor() as i64,
            (point[1] / cell).floor() as i64,
            (point[2] / cell).floor() as i64,
        )
    };
    let mut grid: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    points.iter().enumerate().for_each(|(index, &point)| {
        grid.entry(key(point)).or_default().push(index);
    });
    let tolerance_sq = tolerance * tolerance;
    let mut group_of = vec![usize::MAX; points.len()];
    let mut groups = Vec::new();
    for (index, &point) in points.iter().enumerate() {
        if group_of[index] != usize::MAX {
            continue;
        }
        let home = key(point);
        let mut group = vec![index];
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(cell_points) = grid.get(&(home.0 + dx, home.1 + dy, home.2 + dz))
                    else {
                        continue;
                    };
                    for &other in cell_points {
                        let delta = sub(point, points[other]);
                        if other != index
                            && group_of[other] == usize::MAX
                            && dot(delta, delta) <= tolerance_sq
                        {
                            group.push(other);
                        }
                    }
                }
            }
        }
        if group.len() < 2 {
            continue;
        }
        let group_index = groups.len();
        for &member in &group {
            group_of[member] = group_index;
        }
        groups.push(group);
    }
    groups
}

/// Barycentric weights of `p` clamped onto the triangle
/// (`BarycentricOnTriangle`, `SurfaceFit.cpp:38-59`; the same `Barycentric` at
/// `MeshAnalysis.cpp:499-522` feeds the closest surface points). @req DECL0013
pub(crate) fn barycentric_clamped(p: [f32; 3], tri: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = tri;
    let v0 = sub(b, a);
    let v1 = sub(c, a);
    let v2 = sub(p, a);
    let d00 = dot(v0, v0);
    let d01 = dot(v0, v1);
    let d11 = dot(v1, v1);
    let d20 = dot(v2, v0);
    let d21 = dot(v2, v1);
    let denominator = d00 * d11 - d01 * d01;
    if denominator.abs() < 1e-20 {
        return [1.0, 0.0, 0.0];
    }
    let v = ((d11 * d20 - d01 * d21) / denominator).max(0.0);
    let w = ((d00 * d21 - d01 * d20) / denominator).max(0.0);
    let u = (1.0 - v - w).max(0.0);
    let sum = u + v + w;
    if sum > 0.0 {
        [u / sum, v / sum, w / sum]
    } else {
        [1.0, 0.0, 0.0]
    }
}

/// The unnormalized winding normal of a triangle
/// (`nifly::Triangle::trinormal`, `Object3d.hpp:1173-1175`). @req DECL0013
pub(crate) fn face_normal_of(face: [[f32; 3]; 3]) -> [f32; 3] {
    cross(sub(face[1], face[0]), sub(face[2], face[0]))
}

pub(crate) fn edge_pairs(face: &[usize; 3]) -> [(usize, usize); 3] {
    [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])]
}

/// Edges used by exactly one triangle, and the vertices of those edges
/// (`ClippingFixer.cpp:247-268`). A body's openings point their vertex normals
/// sideways, so an open edge is how a point past an opening stays outside.
pub(crate) struct OpenEdges {
    usage: HashMap<(u32, u32), usize>,
    vertices: Vec<bool>,
}

impl OpenEdges {
    pub(crate) fn build(faces: &[[u32; 3]], vertices: usize) -> Self {
        let key = |a: u32, b: u32| (a.min(b), a.max(b));
        let mut usage: HashMap<(u32, u32), usize> = HashMap::new();
        for face in faces {
            for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
                *usage.entry(key(a, b)).or_insert(0) += 1;
            }
        }
        let mut open = vec![false; vertices];
        for ((a, b), uses) in &usage {
            if *uses == 1 {
                open[*a as usize] = true;
                open[*b as usize] = true;
            }
        }
        Self {
            usage,
            vertices: open,
        }
    }

    /// An edge of only one triangle: the body ends there.
    pub(crate) fn used_once(&self, a: u32, b: u32) -> bool {
        self.usage
            .get(&(a.min(b), a.max(b)))
            .is_some_and(|uses| *uses == 1)
    }

    pub(crate) fn vertex_is_open(&self, vertex: u32) -> bool {
        self.vertices[vertex as usize]
    }
}

/// Whether the closest point at `bary` of `face` lies past one of the body's
/// openings (`ClippingFixer.cpp:305-315`). @req DECL0013
pub(crate) fn past_open_edge(open: &OpenEdges, face: [u32; 3], bary: [f32; 3]) -> bool {
    const ON_EDGE: f32 = 1e-3;
    let [u, v, w] = bary;
    let (near_u, near_v, near_w) = (u < ON_EDGE, v < ON_EDGE, w < ON_EDGE);
    (near_u && open.used_once(face[1], face[2]))
        || (near_v && open.used_once(face[2], face[0]))
        || (near_w && open.used_once(face[0], face[1]))
        || (near_v && near_w && open.vertex_is_open(face[0]))
        || (near_u && near_w && open.vertex_is_open(face[1]))
        || (near_u && near_v && open.vertex_is_open(face[2]))
}

pub(crate) fn first_non_finite(positions: &[[f32; 3]]) -> Option<usize> {
    positions
        .iter()
        .position(|position| !position.iter().all(|value| value.is_finite()))
}

pub(crate) fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// A zero vector stays zero, as `nifly::Vector3::Normalize` does
/// (`Object3d.hpp:124-132`).
pub(crate) fn unit(v: [f32; 3]) -> [f32; 3] {
    let len = length(v);
    if len <= 1e-12 {
        [0.0, 0.0, 0.0]
    } else {
        scale(v, 1.0 / len)
    }
}
