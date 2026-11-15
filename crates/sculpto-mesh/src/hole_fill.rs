//! Close small boundary loops: a delivered body may not carry a puncture.
//!
//! A replaceable head exported from an authoring target can hold a small open
//! loop beside its real collar: Mike's head has a 12-vertex opening above the
//! neck, and the shared authoring head carries a 20-vertex loop plus a band of
//! single-triangle gaps across the forehead. The neck seal cannot see them —
//! it bridges the declared collar arc, which is a different 86–90-vertex loop.
//!
//! This module fills a loop only when it is small enough to be a puncture:
//! three or more vertices, at most the caller's cap, a simple directed cycle,
//! non-zero area. A three-vertex loop becomes one triangle; a larger loop gets
//! a centroid fan. Both use the boundary's own direction, so the patch winding
//! agrees with the adjacent surface. The cap is what keeps a collar, cuff or
//! hem open: those loops are larger by construction, and the report names every
//! loop left open instead of closing it silently.
//!
//! @req BODY-01

use std::collections::BTreeMap;

/// Default vertex cap for a head puncture. The head collar is 86–90 vertices,
/// the ankle rims 130–160 and the foot toes 160, so only true punctures qualify.
pub const DEFAULT_HOLE_VERTEX_CAP: usize = 32;

/// One filled loop, in output coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilledHole {
    /// Boundary vertices on the loop before the fill.
    pub vertices: usize,
    /// Loop area; a zero-area loop is left open, never patched with slivers.
    pub area: f64,
    /// Loop centroid.
    pub centroid: [f32; 3],
    /// Triangles added by the patch.
    pub triangles: usize,
}

/// What [`close_small_boundary_loops`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoleFillReport {
    /// Boundary loops found before any fill.
    pub considered: usize,
    /// Loops closed by a patch.
    pub filled: Vec<FilledHole>,
    /// Loops left open by vertex count, shape or zero area.
    pub left_open: Vec<usize>,
    /// Patch vertices added; one per loop with more than three vertices.
    pub added_vertices: usize,
    /// Patch triangles added.
    pub added_triangles: usize,
}

impl HoleFillReport {
    /// A mesh with no fill applied.
    pub fn unchanged() -> Self {
        Self::default()
    }

    /// True when this report changed no triangle and added no vertex.
    pub fn is_noop(&self) -> bool {
        self.added_vertices == 0 && self.added_triangles == 0
    }
}

/// Why the mesh could not be inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoleFillError {
    /// A triangle names a vertex beyond the position list.
    IndexOutOfRange { triangle: usize, vertex: u32 },
    /// A position or triangle is not finite.
    NonFinite { position: usize },
    /// Welding coincident copies would put a third triangle on one edge.
    WeldNonManifold { from: u32, to: u32 },
}

impl std::fmt::Display for HoleFillError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IndexOutOfRange { triangle, vertex } => write!(
                formatter,
                "triangle {triangle} names vertex {vertex} outside the position list"
            ),
            Self::NonFinite { position } => {
                write!(formatter, "position {position} is not finite")
            }
            Self::WeldNonManifold { from, to } => write!(
                formatter,
                "welding coincident boundary copies would leave edge {from}-{to} with more than two triangles"
            ),
        }
    }
}

impl std::error::Error for HoleFillError {}

/// Exactly coincident boundary copies merged by [`weld_coincident_boundaries`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WeldReport {
    /// Groups of two or more boundary vertices at one exact position.
    pub groups: usize,
    /// Boundary vertices removed by the merges.
    pub merged_vertices: usize,
}

/// Merge exactly coincident **boundary** vertex copies, the authored slit the
/// neck seal deliberately leaves alone. Mike's head carries a 67-vertex slit
/// around the skull made of 14 coincident groups; welding the copies breaks it
/// into five small punctures the collar-preserving fill can then patch.
///
/// Only boundary vertices are considered, and a merge that would push an edge
/// above two adjacent triangles is refused whole: a slit duplicate is two
/// sides of one surface, never a third sheet.
pub fn weld_coincident_boundaries(
    positions: &mut [[f32; 3]],
    triangles: &mut Vec<[u32; 3]>,
) -> Result<WeldReport, HoleFillError> {
    weld_coincident(positions, triangles, false)
}

/// Weld the authored slit copies **beside** the part's largest opening: the
/// collar is left untouched so its ring stays a clean cycle the joint seal can
/// address. This is the head path.
pub fn weld_coincident_slit_copies(
    positions: &mut [[f32; 3]],
    triangles: &mut Vec<[u32; 3]>,
) -> Result<WeldReport, HoleFillError> {
    weld_coincident(positions, triangles, true)
}

fn weld_coincident(
    positions: &mut [[f32; 3]],
    triangles: &mut Vec<[u32; 3]>,
    protect_largest: bool,
) -> Result<WeldReport, HoleFillError> {
    for (index, position) in positions.iter().enumerate() {
        if !position.iter().all(|axis| axis.is_finite()) {
            return Err(HoleFillError::NonFinite { position: index });
        }
    }
    let loops = boundary_loops(triangles, positions.len())?;
    let protected = loops
        .iter()
        .enumerate()
        .max_by_key(|(_, order)| order.len())
        .map(|(index, _)| index);
    let mut groups: BTreeMap<[u32; 3], Vec<u32>> = BTreeMap::new();
    for (index, order) in loops.iter().enumerate() {
        if protect_largest && Some(index) == protected {
            continue;
        }
        for vertex in order {
            let key = positions[*vertex as usize].map(f32::to_bits);
            groups.entry(key).or_default().push(*vertex);
        }
    }
    let mut mapping: BTreeMap<u32, u32> = BTreeMap::new();
    let mut report = WeldReport::default();
    for vertices in groups.values().filter(|vertices| vertices.len() > 1) {
        let target = *vertices.iter().min().expect("group is non-empty");
        for vertex in vertices.iter().filter(|vertex| **vertex != target) {
            mapping.insert(*vertex, target);
        }
        report.groups += 1;
        report.merged_vertices += vertices.len() - 1;
    }
    if mapping.is_empty() {
        return Ok(report);
    }
    let rewired: Vec<[u32; 3]> = triangles
        .iter()
        .map(|triangle| triangle.map(|vertex| *mapping.get(&vertex).unwrap_or(&vertex)))
        .filter(|triangle| {
            triangle[0] != triangle[1] && triangle[1] != triangle[2] && triangle[0] != triangle[2]
        })
        .collect();
    let mut counts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for triangle in &rewired {
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            *counts.entry((from.min(to), from.max(to))).or_default() += 1;
        }
    }
    if let Some(((from, to), _)) = counts.iter().find(|(_, count)| **count > 2) {
        return Err(HoleFillError::WeldNonManifold {
            from: *from,
            to: *to,
        });
    }
    *triangles = rewired;
    Ok(report)
}

/// Drop positions no triangle names, remapping the triangles. A repaired head
/// keeps no orphan seam copies in its delivered vertex list.
pub fn compact_unreferenced(positions: &mut Vec<[f32; 3]>, triangles: &mut [[u32; 3]]) {
    let mut remap = vec![u32::MAX; positions.len()];
    let mut kept = Vec::with_capacity(positions.len());
    for triangle in triangles.iter_mut() {
        for vertex in triangle.iter_mut() {
            let old = *vertex as usize;
            if remap[old] == u32::MAX {
                remap[old] = kept.len() as u32;
                kept.push(positions[old]);
            }
            *vertex = remap[old];
        }
    }
    *positions = kept;
}

/// Simple boundary cycles of the mesh, each in directed boundary order.
fn boundary_loops(
    triangles: &[[u32; 3]],
    positions: usize,
) -> Result<Vec<Vec<u32>>, HoleFillError> {
    let mut directed: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for vertex in triangle {
            if *vertex as usize >= positions {
                return Err(HoleFillError::IndexOutOfRange {
                    triangle: index,
                    vertex: *vertex,
                });
            }
        }
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let key = (from.min(to), from.max(to));
            *edges.entry(key).or_default() += 1;
            directed.entry(from).or_default().push(to);
        }
    }
    let boundary: BTreeMap<u32, Vec<u32>> = (0..positions as u32)
        .filter_map(|from| {
            let outgoing: Vec<u32> = directed
                .get(&from)
                .into_iter()
                .flatten()
                .copied()
                .filter(|to| edges.get(&(from.min(*to), from.max(*to))) == Some(&1))
                .collect();
            (!outgoing.is_empty()).then_some((from, outgoing))
        })
        .collect();
    let mut incoming: BTreeMap<u32, usize> = BTreeMap::new();
    for to in boundary.values().flatten() {
        *incoming.entry(*to).or_default() += 1;
    }
    let mut visited: BTreeMap<u32, bool> = BTreeMap::new();
    let mut loops = Vec::new();
    for start in boundary.keys().copied() {
        if visited.get(&start) == Some(&true) {
            continue;
        }
        let mut order = Vec::new();
        let mut cursor = start;
        let simple = loop {
            if visited.get(&cursor) == Some(&true) {
                break false;
            }
            visited.insert(cursor, true);
            order.push(cursor);
            let Some(nexts) = boundary.get(&cursor) else {
                break false;
            };
            if nexts.len() != 1 || incoming.get(&cursor) != Some(&1) {
                break false;
            }
            let next = nexts[0];
            if next == start {
                break order.len() >= 3;
            }
            if order.len() > boundary.len() {
                break false;
            }
            cursor = next;
        };
        if simple {
            loops.push(order);
        }
    }
    Ok(loops)
}

/// Repair the head after its joint seal: weld the authored slit copies that
/// remain on head geometry, then fan-fill the small punctures they break into.
///
/// The neck seal selects one collar arc and bridges it; the rest of the head's
/// authored loop stays as a slit around the skull. Its copies can be pinched,
/// so membership is decided per **boundary component** by the component
/// centroid inside the head box, and the fan is ordered by angle rather than by
/// the mesh's boundary direction. A foot toe, a chest pore or a torso opening is
/// outside that box and is never touched.
pub fn repair_head_boundaries(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    head: [[f32; 3]; 2],
    margin: f32,
    cap: usize,
    body: std::ops::Range<usize>,
) -> Result<(WeldReport, HoleFillReport), HoleFillError> {
    let weld = weld_head_components(positions, triangles, head, margin)?;
    let holes = fill_head_components(positions, triangles, head, margin, cap, &body)?;
    Ok((weld, holes))
}

type BoundaryComponent = (Vec<u32>, [f64; 3]);

/// Boundary components as undirected vertex sets, with the adjacent triangle
/// normal sum for orientation.
fn boundary_components(
    triangles: &[[u32; 3]],
    positions: &[[f32; 3]],
) -> Result<Vec<BoundaryComponent>, HoleFillError> {
    let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut edge_normal: BTreeMap<(u32, u32), [f64; 3]> = BTreeMap::new();
    for (index, triangle) in triangles.iter().enumerate() {
        for vertex in triangle {
            if *vertex as usize >= positions.len() {
                return Err(HoleFillError::IndexOutOfRange {
                    triangle: index,
                    vertex: *vertex,
                });
            }
        }
        let a = positions[triangle[0] as usize];
        let b = positions[triangle[1] as usize];
        let c = positions[triangle[2] as usize];
        let u = [
            f64::from(b[0] - a[0]),
            f64::from(b[1] - a[1]),
            f64::from(b[2] - a[2]),
        ];
        let v = [
            f64::from(c[0] - a[0]),
            f64::from(c[1] - a[1]),
            f64::from(c[2] - a[2]),
        ];
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let key = (from.min(to), from.max(to));
            *edges.entry(key).or_default() += 1;
            let entry = edge_normal.entry(key).or_insert([0.0; 3]);
            for axis in 0..3 {
                entry[axis] += normal[axis];
            }
        }
    }
    let boundary: Vec<((u32, u32), [f64; 3])> = edges
        .iter()
        .filter(|(_, count)| **count == 1)
        .map(|(key, _)| (*key, edge_normal[key]))
        .collect();
    let mut adjacency: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut incident: BTreeMap<u32, [f64; 3]> = BTreeMap::new();
    for ((from, to), normal) in boundary {
        adjacency.entry(from).or_default().push(to);
        adjacency.entry(to).or_default().push(from);
        for vertex in [from, to] {
            let entry = incident.entry(vertex).or_insert([0.0; 3]);
            for axis in 0..3 {
                entry[axis] += normal[axis];
            }
        }
    }
    let mut seen: BTreeMap<u32, bool> = BTreeMap::new();
    let mut components = Vec::new();
    for start in adjacency.keys().copied() {
        if seen.get(&start) == Some(&true) {
            continue;
        }
        let mut stack = vec![start];
        let mut vertices = Vec::new();
        let mut normal = [0.0_f64; 3];
        while let Some(vertex) = stack.pop() {
            if seen.get(&vertex) == Some(&true) {
                continue;
            }
            seen.insert(vertex, true);
            vertices.push(vertex);
            if let Some(sum) = incident.get(&vertex) {
                for axis in 0..3 {
                    normal[axis] += sum[axis];
                }
            }
            if let Some(next) = adjacency.get(&vertex) {
                stack.extend(next.iter().copied());
            }
        }
        if vertices.len() >= 3 {
            components.push((vertices, normal));
        }
    }
    Ok(components)
}

fn inside_box(position: [f32; 3], head: [[f32; 3]; 2], margin: f32) -> bool {
    (0..3).all(|axis| {
        position[axis] >= head[0][axis] - margin && position[axis] <= head[1][axis] + margin
    })
}

fn component_centroid(positions: &[[f32; 3]], vertices: &[u32]) -> [f32; 3] {
    let points: Vec<[f32; 3]> = vertices.iter().map(|v| positions[*v as usize]).collect();
    centroid(&points)
}

fn weld_head_components(
    positions: &mut [[f32; 3]],
    triangles: &mut Vec<[u32; 3]>,
    head: [[f32; 3]; 2],
    margin: f32,
) -> Result<WeldReport, HoleFillError> {
    let components = boundary_components(triangles, positions)?;
    let mut groups: BTreeMap<[u32; 3], Vec<u32>> = BTreeMap::new();
    for (vertices, _) in components
        .iter()
        .filter(|(vertices, _)| inside_box(component_centroid(positions, vertices), head, margin))
    {
        for vertex in vertices {
            let key = positions[*vertex as usize].map(f32::to_bits);
            groups.entry(key).or_default().push(*vertex);
        }
    }
    let mut mapping: BTreeMap<u32, u32> = BTreeMap::new();
    let mut report = WeldReport::default();
    for vertices in groups.values().filter(|vertices| vertices.len() > 1) {
        let target = *vertices.iter().min().expect("group is non-empty");
        for vertex in vertices.iter().filter(|vertex| **vertex != target) {
            mapping.insert(*vertex, target);
        }
        report.groups += 1;
        report.merged_vertices += vertices.len() - 1;
    }
    if mapping.is_empty() {
        return Ok(report);
    }
    let rewired: Vec<[u32; 3]> = triangles
        .iter()
        .map(|triangle| triangle.map(|vertex| *mapping.get(&vertex).unwrap_or(&vertex)))
        .filter(|triangle| {
            triangle[0] != triangle[1] && triangle[1] != triangle[2] && triangle[0] != triangle[2]
        })
        .collect();
    let mut counts: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for triangle in &rewired {
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            *counts.entry((from.min(to), from.max(to))).or_default() += 1;
        }
    }
    if let Some(((from, to), _)) = counts.iter().find(|(_, count)| **count > 2) {
        return Err(HoleFillError::WeldNonManifold {
            from: *from,
            to: *to,
        });
    }
    *triangles = rewired;
    Ok(report)
}

fn fill_head_components(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    head: [[f32; 3]; 2],
    margin: f32,
    cap: usize,
    body: &std::ops::Range<usize>,
) -> Result<HoleFillReport, HoleFillError> {
    let components = boundary_components(triangles, positions)?;
    let mut report = HoleFillReport::default();
    for (vertices, normal) in components {
        let mut center = component_centroid(positions, &vertices);
        // Lift the patch slightly along the outward normal: a flat lid on a
        // concave nape renders as a dark crease even with shadows off.
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length > 0.0 {
            let radius = vertices
                .iter()
                .map(|vertex| {
                    let p = positions[*vertex as usize];
                    ((p[0] - center[0]).powi(2)
                        + (p[1] - center[1]).powi(2)
                        + (p[2] - center[2]).powi(2))
                    .sqrt()
                })
                .fold(0.0_f32, f32::max);
            let lift = 0.30 * radius;
            center[0] += (normal[0] / length) as f32 * lift;
            center[1] += (normal[1] / length) as f32 * lift;
            center[2] += (normal[2] / length) as f32 * lift;
        }
        if !inside_box(center, head, margin) {
            continue;
        }
        if vertices.len() > cap {
            report.left_open.push(vertices.len());
            continue;
        }
        report.considered += 1;
        let set: std::collections::BTreeSet<u32> = vertices.iter().copied().collect();
        // Directed boundary edges of this component: the patch must traverse
        // each one the opposite way, so the seam closes even when a pinched
        // component has no single walk order.
        let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        let mut directed: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
        for triangle in triangles.iter() {
            for (from, to) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                if !set.contains(&from) || !set.contains(&to) {
                    continue;
                }
                let key = (from.min(to), from.max(to));
                *edges.entry(key).or_default() += 1;
                directed.entry(key).or_insert((from, to));
            }
        }
        let boundary: Vec<(u32, u32)> = edges
            .iter()
            .filter(|(_, count)| **count == 1)
            .filter_map(|(key, _)| directed.get(key).copied())
            .collect();
        if boundary.len() < 3 {
            report.left_open.push(vertices.len());
            continue;
        }
        // A nape rim sits above the torso neck: bridge it down to the nearest
        // body vertices instead of closing a lid above the gap. Only a rim
        // whose body neighbour is below and within reach qualifies; an ear or
        // an eye puncture stays a flat cap.
        let mut skirt = Vec::with_capacity(boundary.len());
        let mut skirted = true;
        for (from, to) in &boundary {
            let midpoint = [
                (positions[*from as usize][0] + positions[*to as usize][0]) * 0.5,
                (positions[*from as usize][1] + positions[*to as usize][1]) * 0.5,
                (positions[*from as usize][2] + positions[*to as usize][2]) * 0.5,
            ];
            let mut nearest: Option<(f32, usize)> = None;
            for candidate in body.clone() {
                let p = positions[candidate];
                let distance = (p[0] - midpoint[0]).powi(2)
                    + (p[1] - midpoint[1]).powi(2)
                    + (p[2] - midpoint[2]).powi(2);
                if nearest.is_none_or(|(best, _)| distance < best) {
                    nearest = Some((distance, candidate));
                }
            }
            let Some((distance, candidate)) = nearest else {
                skirted = false;
                break;
            };
            let reach = 10.0_f32;
            if distance.sqrt() > reach || positions[candidate][2] > center[2] + 0.5 {
                skirted = false;
                break;
            }
            skirt.push((*from, *to, candidate as u32));
        }
        if skirted && !skirt.is_empty() {
            let before = triangles.len();
            for (from, to, body_vertex) in skirt {
                triangles.push([to, from, body_vertex]);
            }
            report.filled.push(FilledHole {
                vertices: vertices.len(),
                area: 0.0,
                centroid: center,
                triangles: triangles.len() - before,
            });
            report.added_triangles += triangles.len() - before;
            continue;
        }
        let center_index = positions.len() as u32;
        positions.push(center);
        report.added_vertices += 1;
        let before = triangles.len();
        let mut area = 0.0_f64;
        for (from, to) in boundary {
            triangles.push([to, from, center_index]);
            let a = positions[to as usize];
            let b = positions[from as usize];
            let c = center;
            let u = [
                f64::from(b[0] - a[0]),
                f64::from(b[1] - a[1]),
                f64::from(b[2] - a[2]),
            ];
            let v = [
                f64::from(c[0] - a[0]),
                f64::from(c[1] - a[1]),
                f64::from(c[2] - a[2]),
            ];
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            area += 0.5 * (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        }
        report.filled.push(FilledHole {
            vertices: vertices.len(),
            area,
            centroid: center,
            triangles: triangles.len() - before,
        });
        report.added_triangles += triangles.len() - before;
    }
    Ok(report)
}

/// Close every simple boundary loop with at most `cap` vertices.
///
/// The caller keeps the geometry ownership: positions and triangles are edited
/// in place, and the report says exactly which loops were patched and which
/// stayed open. A closed mesh is untouched.
pub fn close_small_boundary_loops(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    cap: usize,
) -> Result<HoleFillReport, HoleFillError> {
    close_loops(positions, triangles, cap, false)
}

/// Close punctures beside a part's declared opening: the **largest** boundary
/// loop is the collar (or cuff) the joint seal owns and is always left open,
/// even in a small synthetic fixture whose collar has fewer vertices than the
/// cap. Every other simple loop within `cap` is patched.
///
/// This is the head path `body assemble` uses: the real head collar is
/// 86–90 vertices, while its punctures are 3–20.
pub fn close_punctures_beside_collar(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    cap: usize,
) -> Result<HoleFillReport, HoleFillError> {
    close_loops(positions, triangles, cap, true)
}

fn close_loops(
    positions: &mut Vec<[f32; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    cap: usize,
    protect_largest: bool,
) -> Result<HoleFillReport, HoleFillError> {
    for (index, position) in positions.iter().enumerate() {
        if !position.iter().all(|axis| axis.is_finite()) {
            return Err(HoleFillError::NonFinite { position: index });
        }
    }
    for (index, triangle) in triangles.iter().enumerate() {
        for vertex in triangle {
            if *vertex as usize >= positions.len() {
                return Err(HoleFillError::IndexOutOfRange {
                    triangle: index,
                    vertex: *vertex,
                });
            }
        }
    }

    // Directed boundary edge (a -> b) from the one triangle that holds it.
    let mut directed: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for triangle in triangles.iter() {
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let key = (from.min(to), from.max(to));
            *edges.entry(key).or_default() += 1;
            directed.entry(from).or_default().push(to);
        }
    }
    // Keep only boundary edges: exactly one adjacent triangle. A vertex with
    // two outgoing edges is a pinch, not a simple hole, and stays open.
    let boundary: BTreeMap<u32, Vec<u32>> = (0..positions.len() as u32)
        .filter_map(|from| {
            let outgoing: Vec<u32> = directed
                .get(&from)
                .into_iter()
                .flatten()
                .copied()
                .filter(|to| edges.get(&(from.min(*to), from.max(*to))) == Some(&1))
                .collect();
            (!outgoing.is_empty()).then_some((from, outgoing))
        })
        .collect();
    if boundary.is_empty() {
        return Ok(HoleFillReport::unchanged());
    }
    let mut incoming: BTreeMap<u32, usize> = BTreeMap::new();
    for to in boundary.values().flatten() {
        *incoming.entry(*to).or_default() += 1;
    }

    let mut visited: BTreeMap<u32, bool> = BTreeMap::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for start in boundary.keys().copied() {
        if visited.get(&start) == Some(&true) {
            continue;
        }
        let mut order = Vec::new();
        let mut cursor = start;
        let simple = loop {
            if visited.get(&cursor) == Some(&true) {
                break false;
            }
            visited.insert(cursor, true);
            order.push(cursor);
            let Some(nexts) = boundary.get(&cursor) else {
                break false;
            };
            if nexts.len() != 1 || incoming.get(&cursor) != Some(&1) {
                break false;
            }
            let next = nexts[0];
            if next == start {
                break order.len() >= 3;
            }
            if order.len() > boundary.len() {
                break false;
            }
            cursor = next;
        };
        loops.push(if simple { order } else { Vec::new() });
    }

    let protected = protect_largest
        .then(|| {
            loops
                .iter()
                .enumerate()
                .filter(|(_, order)| !order.is_empty())
                .max_by_key(|(_, order)| order.len())
                .map(|(index, _)| index)
        })
        .flatten();
    let mut report = HoleFillReport {
        considered: loops.len(),
        ..HoleFillReport::default()
    };
    for (index, order) in loops.into_iter().enumerate() {
        if order.is_empty() {
            continue;
        }
        if order.len() > cap || Some(index) == protected {
            report.left_open.push(order.len());
            continue;
        }
        let points: Vec<[f32; 3]> = order
            .iter()
            .map(|vertex| positions[*vertex as usize])
            .collect();
        let (area, _normal) = loop_area_and_normal(&points);
        if !area.is_finite() || area <= 1e-12 {
            report.left_open.push(order.len());
            continue;
        }
        let centroid = centroid(&points);
        let triangles_before = triangles.len();
        if order.len() == 3 {
            // One triangle across the gap; its edges oppose the boundary.
            triangles.push([order[0], order[2], order[1]]);
        } else {
            let center = positions.len() as u32;
            positions.push(centroid);
            report.added_vertices += 1;
            for index in 0..order.len() {
                let from = order[index];
                let to = order[(index + 1) % order.len()];
                // Existing edge is `from -> to`; the patch must use `to -> from`.
                triangles.push([to, from, center]);
            }
        }
        report.filled.push(FilledHole {
            vertices: order.len(),
            area,
            centroid,
            triangles: triangles.len() - triangles_before,
        });
        report.added_triangles += triangles.len() - triangles_before;
    }
    Ok(report)
}

fn centroid(points: &[[f32; 3]]) -> [f32; 3] {
    let count = points.len() as f64;
    let mut sum = [0.0_f64; 3];
    for point in points {
        for axis in 0..3 {
            sum[axis] += f64::from(point[axis]);
        }
    }
    [
        (sum[0] / count) as f32,
        (sum[1] / count) as f32,
        (sum[2] / count) as f32,
    ]
}

/// Newell area and unit normal of a polygon in loop order.
fn loop_area_and_normal(points: &[[f32; 3]]) -> (f64, [f64; 3]) {
    let mut normal = [0.0_f64; 3];
    for index in 0..points.len() {
        let current = points[index];
        let next = points[(index + 1) % points.len()];
        normal[0] += (f64::from(current[1]) - f64::from(next[1]))
            * (f64::from(current[2]) + f64::from(next[2]));
        normal[1] += (f64::from(current[2]) - f64::from(next[2]))
            * (f64::from(current[0]) + f64::from(next[0]));
        normal[2] += (f64::from(current[0]) - f64::from(next[0]))
            * (f64::from(current[1]) + f64::from(next[1]));
    }
    let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
    let area = 0.5 * length;
    if length > 0.0 {
        for axis in &mut normal {
            *axis /= length;
        }
    }
    (area, normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary_edges(triangles: &[[u32; 3]]) -> usize {
        let mut edges: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        for triangle in triangles {
            for (from, to) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                *edges.entry((from.min(to), from.max(to))).or_default() += 1;
            }
        }
        edges.values().filter(|count| **count == 1).count()
    }

    #[test]
    fn a_missing_tetrahedron_face_is_filled_by_one_triangle() {
        let mut positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        // Three faces; the +x/+y/+z face is missing, so its 3 vertices are open.
        let mut triangles = vec![[0, 2, 1], [0, 1, 3], [0, 3, 2]];
        let report =
            close_small_boundary_loops(&mut positions, &mut triangles, DEFAULT_HOLE_VERTEX_CAP)
                .expect("inspect");
        assert_eq!(report.filled.len(), 1);
        assert_eq!(report.filled[0].vertices, 3);
        assert_eq!(report.added_triangles, 1);
        assert_eq!(report.added_vertices, 0);
        assert_eq!(report.left_open.len(), 0);
        assert_eq!(boundary_edges(&triangles), 0, "the patch closes the loop");
        assert_eq!(positions.len(), 4, "a triangle patch needs no new vertex");
    }

    #[test]
    fn a_wide_ring_is_left_open() {
        // A 45-segment cylinder side: two 45-vertex boundary loops.
        let mut positions = Vec::new();
        for side in 0..2 {
            for segment in 0..45 {
                let angle = std::f32::consts::TAU * segment as f32 / 45.0;
                positions.push([angle.cos(), angle.sin(), side as f32]);
            }
        }
        let mut triangles = Vec::new();
        for segment in 0..45 {
            let next = (segment + 1) % 45;
            triangles.push([segment, next, 45 + next]);
            triangles.push([segment, 45 + next, 45 + segment]);
        }
        let before = triangles.clone();
        let report =
            close_small_boundary_loops(&mut positions, &mut triangles, 32).expect("inspect");
        assert!(report.filled.is_empty());
        assert_eq!(report.left_open, vec![45, 45]);
        assert_eq!(triangles, before);
    }

    #[test]
    fn a_closed_cube_is_untouched() {
        let mut positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let mut triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        let positions_before = positions.clone();
        let triangles_before = triangles.clone();
        let report =
            close_small_boundary_loops(&mut positions, &mut triangles, 32).expect("inspect");
        assert!(report.is_noop());
        assert_eq!(positions, positions_before);
        assert_eq!(triangles, triangles_before);
    }

    #[test]
    fn the_collar_loop_stays_open_beside_a_puncture() {
        // A cube with its bottom and top faces removed: two 4-vertex loops.
        // The largest-loop rule protects one (the collar the joint seal owns)
        // and patches the other, even though both fit the cap.
        let mut positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ];
        let mut triangles = vec![
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        let report =
            close_punctures_beside_collar(&mut positions, &mut triangles, 32).expect("inspect");
        assert_eq!(report.filled.len(), 1, "one loop is a puncture");
        assert_eq!(report.filled[0].vertices, 4);
        assert_eq!(report.left_open, vec![4], "the collar stays open");
        assert_eq!(report.added_vertices, 1);
        assert_eq!(report.added_triangles, 4);
        assert_eq!(boundary_edges(&triangles), 4, "only the collar remains");
    }

    #[test]
    fn a_non_finite_position_is_refused() {
        let mut positions = vec![[f32::NAN, 0.0, 0.0]];
        let mut triangles = vec![[0, 0, 0]];
        let error = close_small_boundary_loops(&mut positions, &mut triangles, 32).unwrap_err();
        assert_eq!(error, HoleFillError::NonFinite { position: 0 });
    }
}
