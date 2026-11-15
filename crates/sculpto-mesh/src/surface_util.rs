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

/// Connected components of a face set, as one label per vertex.
///
/// One label per connected piece is what separates an authored dart (two
/// coincident lips of the *same* piece) from an authored seam (the same point
/// on two pieces). @req DECL0013
pub(crate) fn component_labels(vertices: usize, faces: &[[u32; 3]]) -> Vec<u32> {
    let mut parent: Vec<usize> = (0..vertices).collect();
    fn find(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    for face in faces {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            let (a, b) = (find(&mut parent, a as usize), find(&mut parent, b as usize));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut label_of: HashMap<usize, u32> = HashMap::new();
    (0..vertices)
        .map(|vertex| {
            let root = find(&mut parent, vertex);
            let next = label_of.len() as u32;
            *label_of.entry(root).or_insert(next)
        })
        .collect()
}

/// Remap `faces` onto welded ids and drop every face whose remapped corners
/// collide (`Weld`'s face filter, `SurfaceFit.cpp:110-120`).
///
/// A collapsed face names no surface: keeping it would leave a triangle whose
/// normal, area and standoff are all zero. The weld and the dress seam merge
/// share this rule so the two cannot disagree on it. @req DECL0013
pub(crate) fn remap_faces(faces: &[[u32; 3]], merged_of: &[u32]) -> (Vec<[u32; 3]>, usize) {
    let mut kept = Vec::with_capacity(faces.len());
    let mut dropped = 0;
    for face in faces {
        let corners = face.map(|corner| merged_of[corner as usize]);
        if corners[0] == corners[1] || corners[1] == corners[2] || corners[0] == corners[2] {
            dropped += 1;
            continue;
        }
        kept.push(corners);
    }
    (kept, dropped)
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

    /// How many edges one triangle uses: the mesh's open border.
    pub(crate) fn boundary_edges(&self) -> usize {
        self.usage.values().filter(|&&uses| uses == 1).count()
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

/// How far a closest surface point may sit from a triangle corner or edge and
/// still name that shared element, as a fraction of the triangle's longest side.
/// The nearest-point search reports an f32 position, so this band is what
/// absorbs its rounding; inside it the point is on the shared element for every
/// triangle that shares it.
const SHARED_ELEMENT_EPSILON: f32 = 1e-3;

/// Cosine of the cone around a member's surface normal inside which the member
/// is the vertex's contact surface: `cos 45°`, the exact `1/√2`.
///
/// A member is a contact surface for a vertex only when the vertex lies under
/// its surface rather than past its silhouette: the behind-depth must be at
/// least the lateral reach. The nearest-triangle half-space alone reaches
/// arbitrarily far — on the layered trousers a shin vertex 49.41 u below a leg
/// hem was owned at depth 2.86, 145 vertices 5–53 u from the panty were pushed
/// along such a horizon, and the measured far-field captures reach 22.8 u at a
/// lateral offset larger than their depth. Those are not contacts: they are
/// grazing views of a distant surface, and they resolve against the member
/// below instead.
pub(crate) const COVERAGE_MIN_COSINE: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The side one surface names at a closest point.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SurfaceSide {
    /// Outward unit pseudo-normal of the closest point's shared element.
    pub(crate) normal: [f32; 3],
}

/// One surface's side at any closest point.
///
/// A nearest-point search returns one triangle, and at a hem corner three
/// triangles tie for the *same* closest point while their face normals
/// disagree — the layered trousers read −17.89 / +55.10 / +40.60 signed at one
/// such corner, so following the returned triangle made ownership a tie of the
/// search. The side here is the shared element's: the face normal inside a
/// triangle, the sum of the incident face normals on an edge, the area-weighted
/// vertex normal at a corner. Tied triangles name the same shared element, so
/// they name the same side. @req DECL0013 DECL0015
pub(crate) struct SideField {
    faces: Vec<Option<[f32; 3]>>,
    vertices: Vec<[f32; 3]>,
    /// Summed incident face normals of every edge.
    edges: HashMap<(u32, u32), [f32; 3]>,
}

impl SideField {
    pub(crate) fn build(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Self {
        let vertices = vertex_normals(positions, faces);
        let sides: Vec<Option<[f32; 3]>> = faces
            .iter()
            .map(|face| {
                let normal = unit(face_normal_of(
                    face.map(|vertex| positions[vertex as usize]),
                ));
                (dot(normal, normal) > 0.0).then_some(normal)
            })
            .collect();
        // An edge sums its incident face normals: two flat faces agree on it (a
        // quad's diagonal keeps the face normal exactly) and a crease gets the
        // pseudo-normal between its sides.
        let mut edges: HashMap<(u32, u32), [f32; 3]> = HashMap::new();
        for (face, side) in faces.iter().zip(&sides) {
            let Some(side) = side else {
                continue;
            };
            for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
                let entry = edges.entry((a.min(b), a.max(b))).or_insert([0.0; 3]);
                *entry = add(*entry, *side);
            }
        }
        Self {
            faces: sides,
            vertices,
            edges,
        }
    }

    /// The side `point` reads at its closest point on `triangle`, or `None` for
    /// a triangle that cannot name a side.
    pub(crate) fn side(
        &self,
        positions: &[[f32; 3]],
        faces: &[[u32; 3]],
        triangle: usize,
        point: [f32; 3],
    ) -> Option<SurfaceSide> {
        let face = faces[triangle];
        let face_side = *self.faces.get(triangle)?.as_ref()?;
        let corners = face.map(|vertex| positions[vertex as usize]);
        let epsilon = SHARED_ELEMENT_EPSILON
            * [(0, 1), (1, 2), (2, 0)]
                .map(|(a, b)| length(sub(corners[b], corners[a])))
                .into_iter()
                .fold(0.0f32, f32::max);
        let mut nearest = 0;
        let mut nearest_distance = f32::INFINITY;
        for (index, &corner) in corners.iter().enumerate() {
            let distance = length(sub(point, corner));
            if distance < nearest_distance {
                nearest = index;
                nearest_distance = distance;
            }
        }
        if nearest_distance <= epsilon {
            return Some(SurfaceSide {
                normal: self.vertices[face[nearest] as usize],
            });
        }
        for (a, b) in [(0usize, 1usize), (1, 2), (2, 0)] {
            if segment_distance(point, corners[a], corners[b]) > epsilon {
                continue;
            }
            let key = (face[a].min(face[b]), face[a].max(face[b]));
            let Some(&sum) = self.edges.get(&key) else {
                break;
            };
            let normal = unit(sum);
            if dot(normal, normal) == 0.0 {
                break;
            }
            return Some(SurfaceSide { normal });
        }
        Some(SurfaceSide { normal: face_side })
    }
}

/// The signed offset of `point` from one member's surface at its closest point,
/// or `None` when the member is not its contact surface.
///
/// A member is a contact surface only inside [`COVERAGE_MIN_COSINE`] of its
/// normal axis, on either side: behind, it owns the vertex; in front, it is the
/// pull target. Outside it the vertex only grazes the member's horizon and
/// belongs to the member below.
pub(crate) fn contact_offset(side: SurfaceSide, offset: [f32; 3]) -> Option<f64> {
    let signed = f64::from(dot(offset, side.normal));
    match signed.abs() < f64::from(COVERAGE_MIN_COSINE) * f64::from(length(offset)) {
        true => None,
        false => Some(signed),
    }
}

/// Distance from `point` to the segment `a..b`.
fn segment_distance(point: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let along = sub(b, a);
    let squared = dot(along, along);
    let offset = sub(point, a);
    let position = match squared > 0.0 {
        true => (dot(offset, along) / squared).clamp(0.0, 1.0),
        false => 0.0,
    };
    length(sub(point, add(a, scale(along, position))))
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

/// Area-weighted vertex normals turned to point away from the body's inside,
/// whatever the winding of its triangles, with the sign that was applied
/// (`ClippingFixer.cpp:139-162,188-206,322-326`). @req DECL0013
pub(crate) fn outward_vertex_normals(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> (Vec<[f32; 3]>, f32) {
    if positions.is_empty() {
        return (Vec::new(), 1.0);
    }
    let sign = outward_sign(positions, faces);
    (
        vertex_normals(positions, faces)
            .into_iter()
            .map(|normal| scale(normal, sign))
            .collect(),
        sign,
    )
}

/// Unwelded area-weighted unit normals shared by native clipping consumers.
/// @req DECL0013
pub(crate) fn vertex_normals(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<[f32; 3]> {
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
    normals.into_iter().map(unit).collect()
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
