//! Source-disk recovery: retain original excluded fabric at a concave fold.
//!
//! Guide-facing selection can omit a real disk of original source fabric at a concave
//! fold while the kept surface keeps the fold's outline as an incidental boundary
//! loop. A native fill then has to invent surface where the original surface already
//! exists, and it re-uses the fold's own chords. This module lets a profile declare
//! that such an *original* disk may be recovered instead.
//!
//! Recovery is deliberately strict: the excluded faces must form one connected
//! oriented manifold disk whose entire boundary is exactly that loop, joining the kept
//! surface with opposite directed edges and touching no named aperture. Nothing is
//! welded, moved, collapsed or invented, so a recovered face keeps its original
//! positions, face identity and corner order.
//!
//! @req DECL0013

use std::collections::{HashMap, HashSet, VecDeque};

use super::SourceSurface;

/// One recovered original disk and the evidence that qualified it.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredDisk {
    /// Incidental loop index in selection order.
    pub loop_index: usize,
    /// Original surface face ids, ascending.
    pub faces: Vec<u32>,
    /// The loop's vertices, in walk order: the disk's complete boundary.
    pub boundary_vertices: Vec<u32>,
    /// Distinct surface vertices of the disk, ascending.
    pub vertices: Vec<u32>,
    /// Distinct edges of the disk, boundary included.
    pub edges: usize,
    /// `V - E + F` of the disk: one for a topological disk.
    pub euler: i64,
}

/// Why one incidental loop's excluded component was not recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryRefusal {
    /// The excluded face does not traverse the shared edge the other way.
    WindingConflict { edge: [u32; 2] },
    /// An excluded edge already carries three or more faces.
    NonManifoldEdge { edge: [u32; 2], faces: usize },
    /// A disk vertex is a bowtie: its faces are not one fan.
    NonManifoldVertex { vertex: u32 },
    /// The component's boundary is not exactly this loop.
    BoundaryMismatch { extra: usize, missing: usize },
    /// Interior faces disagree on the shared edge direction.
    InteriorWinding { edge: [u32; 2] },
    /// The component is not a disk (`V - E + F != 1`).
    NotADisk { euler: i64 },
    /// The component shares a vertex with a named aperture loop.
    TouchesAperture { vertex: u32 },
}

impl RecoveryRefusal {
    /// Stable name for receipts; no numeric or path detail leaks into the contract.
    pub fn name(&self) -> &'static str {
        match self {
            Self::WindingConflict { .. } => "winding-conflict",
            Self::NonManifoldEdge { .. } => "non-manifold-edge",
            Self::NonManifoldVertex { .. } => "non-manifold-vertex",
            Self::BoundaryMismatch { .. } => "boundary-mismatch",
            Self::InteriorWinding { .. } => "interior-winding",
            Self::NotADisk { .. } => "not-a-disk",
            Self::TouchesAperture { .. } => "touches-aperture",
        }
    }
}

/// What one selection pass recovered, and what it refused to claim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecoveryReport {
    /// Original faces recovered across every disk.
    pub recovered_faces: usize,
    /// Per-disk evidence, in loop order.
    pub disks: Vec<RecoveredDisk>,
    /// Incidental loops examined and refused, in loop order.
    pub refused: Vec<(usize, RecoveryRefusal)>,
}

/// Edge key: undirected, lower index first.
fn edge_key(left: u32, right: u32) -> (u32, u32) {
    match left <= right {
        true => (left, right),
        false => (right, left),
    }
}

/// Directed edge of one triangle, in winding order.
fn directed(face: [u32; 3], corner: usize) -> (u32, u32) {
    (face[corner], face[(corner + 1) % 3])
}

/// Undirected edge → the faces that carry it.
struct EdgeFaces {
    faces: HashMap<(u32, u32), Vec<u32>>,
}

impl EdgeFaces {
    fn build(surface: &SourceSurface, degenerate: &HashSet<u32>) -> Self {
        let mut faces: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for (index, triangle) in surface.triangles().iter().enumerate() {
            let index = index as u32;
            if degenerate.contains(&index) {
                continue;
            }
            for corner in 0..3 {
                let (left, right) = directed(*triangle, corner);
                faces.entry(edge_key(left, right)).or_default().push(index);
            }
        }
        Self { faces }
    }

    fn of(&self, left: u32, right: u32) -> &[u32] {
        self.faces
            .get(&edge_key(left, right))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// The loop's edges in the direction the kept face traverses them.
fn loop_directed(
    surface: &SourceSurface,
    edges: &EdgeFaces,
    kept: &HashSet<u32>,
    vertices: &[u32],
    closed: bool,
) -> Vec<(u32, u32)> {
    let mut pairs = vertices
        .windows(2)
        .map(|window| (window[0], window[1]))
        .collect::<Vec<_>>();
    if closed && let (Some(first), Some(last)) = (vertices.first(), vertices.last()) {
        pairs.push((*last, *first));
    }
    let mut directed_edges = Vec::with_capacity(pairs.len());
    for (left, right) in pairs {
        // The walk fixes only the vertex order; the kept face fixes the direction the
        // surface traverses the edge, which is what the winding comparison needs.
        let key = edge_key(left, right);
        let mut direction = None;
        for face in edges.of(left, right) {
            if !kept.contains(face) {
                continue;
            }
            let triangle = surface.triangles()[*face as usize];
            for corner in 0..3 {
                let (a, b) = directed(triangle, corner);
                if edge_key(a, b) == key {
                    direction = Some((a, b));
                }
            }
        }
        // The loop came from the kept boundary, so one face always traverses it.
        directed_edges.push(direction.expect("a boundary loop edge has one kept face"));
    }
    directed_edges
}

/// Recover the original excluded disk that closes one incidental loop.
///
/// `kept` holds the surface faces the selection retained (already pruned), `boundary`
/// the loop's vertices in walk order, and `aperture_vertices` every vertex of a
/// named-aperture loop. Returns `Ok(None)` when no loop edge has an excluded face
/// behind it, and a named refusal when a component exists but does not qualify.
pub fn recover_disk(
    surface: &SourceSurface,
    degenerate: &HashSet<u32>,
    kept: &HashSet<u32>,
    loop_index: usize,
    boundary_vertices: &[u32],
    closed: bool,
    aperture_vertices: &HashSet<u32>,
) -> Result<Option<RecoveredDisk>, RecoveryRefusal> {
    let edges = EdgeFaces::build(surface, degenerate);
    let loop_edges = loop_directed(surface, &edges, kept, boundary_vertices, closed);
    let mut seeds = Vec::new();
    for (left, right) in &loop_edges {
        let faces = edges.of(*left, *right);
        let others = faces
            .iter()
            .copied()
            .filter(|face| !kept.contains(face))
            .collect::<Vec<_>>();
        if others.is_empty() {
            continue;
        }
        if others.len() > 1 {
            return Err(RecoveryRefusal::NonManifoldEdge {
                edge: [*left, *right],
                faces: faces.len(),
            });
        }
        let other = others[0];
        let triangle = surface.triangles()[other as usize];
        let opposite = (0..3).any(|corner| directed(triangle, corner) == (*right, *left));
        if !opposite {
            return Err(RecoveryRefusal::WindingConflict {
                edge: [*left, *right],
            });
        }
        seeds.push(other);
    }
    seeds.sort_unstable();
    seeds.dedup();
    if seeds.is_empty() {
        return Ok(None);
    }

    // Grow one component across excluded faces only; a shared edge with a kept face is
    // a boundary edge and must belong to the loop.
    let mut component = HashSet::new();
    let mut queue = VecDeque::new();
    for seed in seeds {
        if component.insert(seed) {
            queue.push_back(seed);
        }
    }
    while let Some(face) = queue.pop_front() {
        for corner in 0..3 {
            let (left, right) = directed(surface.triangles()[face as usize], corner);
            let incident = edges.of(left, right);
            if incident.len() > 2 {
                return Err(RecoveryRefusal::NonManifoldEdge {
                    edge: [left, right],
                    faces: incident.len(),
                });
            }
            for other in incident {
                if kept.contains(other) || component.contains(other) {
                    continue;
                }
                if component.insert(*other) {
                    queue.push_back(*other);
                }
            }
        }
    }

    // Boundary: exactly the loop's edges, nothing else.
    let loop_edge_set = loop_edges
        .iter()
        .map(|(left, right)| edge_key(*left, *right))
        .collect::<HashSet<_>>();
    let mut extra = 0usize;
    let mut vertices: HashSet<u32> = HashSet::new();
    let mut edge_set: HashSet<(u32, u32)> = HashSet::new();
    let mut interior: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
    for face in &component {
        let triangle = surface.triangles()[*face as usize];
        for corner in 0..3 {
            let (left, right) = directed(triangle, corner);
            let key = edge_key(left, right);
            edge_set.insert(key);
            vertices.insert(left);
            match interior.get(&key) {
                None => {
                    interior.insert(key, (left, right));
                }
                Some((first, second)) => {
                    if (*first, *second) == (left, right) {
                        return Err(RecoveryRefusal::InteriorWinding {
                            edge: [left, right],
                        });
                    }
                }
            }
            let in_component = edges
                .of(left, right)
                .iter()
                .filter(|other| component.contains(other))
                .count();
            if in_component == 1 {
                if !loop_edge_set.contains(&key) {
                    extra += 1;
                }
            } else if in_component > 2 {
                return Err(RecoveryRefusal::NonManifoldEdge {
                    edge: [left, right],
                    faces: in_component,
                });
            }
        }
    }
    let missing = loop_edge_set
        .iter()
        .filter(|key| {
            edges
                .of(key.0, key.1)
                .iter()
                .filter(|face| component.contains(face))
                .count()
                != 1
        })
        .count();
    if extra != 0 || missing != 0 {
        return Err(RecoveryRefusal::BoundaryMismatch { extra, missing });
    }
    for vertex in &vertices {
        if aperture_vertices.contains(vertex) {
            return Err(RecoveryRefusal::TouchesAperture { vertex: *vertex });
        }
    }
    let euler = vertices.len() as i64 - edge_set.len() as i64 + component.len() as i64;
    if euler != 1 {
        return Err(RecoveryRefusal::NotADisk { euler });
    }
    let mut faces = component.into_iter().collect::<Vec<_>>();
    faces.sort_unstable();
    let mut vertex_list = vertices.into_iter().collect::<Vec<_>>();
    vertex_list.sort_unstable();
    Ok(Some(RecoveredDisk {
        loop_index,
        faces,
        boundary_vertices: boundary_vertices.to_vec(),
        vertices: vertex_list,
        edges: edge_set.len(),
        euler,
    }))
}
