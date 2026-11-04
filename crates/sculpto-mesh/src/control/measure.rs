//! Measured state of a produced control surface: topology, apertures, contacts
//! and how far it sits from the source shell. @req DECL0013

use std::collections::{HashMap, HashSet};

use crate::check;

use super::{
    ApertureMatch, ApertureResolution, BoundaryLoop, ControlError, ControlProfile, SourceSurface,
    boundary_loops, fans, largest_component, resolve_apertures,
};

/// Declared agreement limits between the source shell exterior and the control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverageLimits {
    /// Distance that still counts as covered.
    pub tolerance: f32,
    /// Required covered share of the compared points, in `0..=1`.
    pub minimum: f32,
}

impl CoverageLimits {
    fn validate(&self) -> Result<(), ControlError> {
        if !self.tolerance.is_finite()
            || self.tolerance <= 0.0
            || !self.minimum.is_finite()
            || self.minimum <= 0.0
            || self.minimum > 1.0
        {
            return Err(ControlError::InvalidCoverage {
                tolerance: self.tolerance,
                minimum: self.minimum,
            });
        }
        Ok(())
    }
}

/// Distance summary for one direction of the source/control comparison.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deviation {
    pub points: usize,
    pub max: f32,
    pub mean: f32,
    pub p95: f32,
    /// Points within the declared tolerance.
    pub covered: usize,
    /// `covered / points`, in `0..=1`.
    pub coverage: f32,
}

/// Both directions: control fidelity to the source, and source coverage by the
/// control. One direction alone cannot show a lost silhouette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deviations {
    pub control_to_source: Deviation,
    pub source_exterior_to_control: Deviation,
}

/// Measured topology of a candidate control surface.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlDiagnostics {
    pub vertices: usize,
    pub triangles: usize,
    pub edges: usize,
    pub euler: i64,
    /// Triangle components by shared edges.
    pub components: usize,
    /// Vertices whose face fan is not a single disk, or whose boundary fan is not
    /// exactly two edges.
    pub non_manifold_vertices: usize,
    /// Edges used by no face or by more than two.
    pub non_manifold_edges: usize,
    /// Edges whose two incident faces traverse them in the same direction.
    pub orientation_conflicts: usize,
    /// Render vertices that identify with another position.
    pub coincident_vertices: usize,
    /// Lowest input vertex index for each exact-position ID in `loops`. @req DECL0013
    pub vertex_sources: Vec<u32>,
    pub loops: Vec<BoundaryLoop>,
    /// Nonadjacent triangle pairs that intersect at rest.
    pub self_contacts: usize,
    pub bounds: [[f32; 3]; 2],
}

/// Genus of a connected orientable surface; only valid after the manifold checks.
pub fn genus_of(euler: i64, loops: i64) -> i64 {
    (2 - loops - euler) / 2
}

/// What the profile's apertures matched on a surface that passed.
#[derive(Debug, Clone, PartialEq)]
pub struct Qualification {
    pub apertures: Vec<ApertureMatch>,
    pub incidental: usize,
    /// Genus measured from the Euler characteristic and the opening count.
    pub genus: i64,
    /// Genus the profile declared; the surface must match it.
    pub expected_genus: u32,
}

impl ControlDiagnostics {
    /// Genus this surface would have if it is one oriented manifold shell.
    ///
    /// `None` states that the surface is too broken for the Euler/boundary relation to
    /// mean anything; the value is reported, never used as a repair decision.
    pub fn measured_genus(&self) -> Option<i64> {
        let valid = self.components == 1
            && self.non_manifold_vertices == 0
            && self.non_manifold_edges == 0
            && self.orientation_conflicts == 0;
        match valid {
            true => Some(genus_of(self.euler, self.loops.len() as i64)),
            false => None,
        }
    }

    /// Measure a candidate control; invalid geometry is rejected, not repaired.
    pub fn measure(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Self, ControlError> {
        let surface = SourceSurface::identify(positions, triangles)?;
        let mesh_positions = surface.positions();
        let mesh_triangles = surface.triangles();
        let mut edge_faces: HashMap<(u32, u32), u32> = HashMap::new();
        for [a, b, c] in mesh_triangles {
            for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
                *edge_faces
                    .entry((left.min(right), left.max(right)))
                    .or_default() += 1;
            }
        }
        let components = largest_component(mesh_triangles).removed_components + 1;
        let non_manifold_edges = edge_faces
            .values()
            .filter(|count| **count != 1 && **count != 2)
            .count();
        // Directed incidence: two faces on one edge must traverse it oppositely.
        let mut directions: HashMap<(u32, u32), (u32, bool)> = HashMap::new();
        let mut orientation_conflicts = 0usize;
        for [a, b, c] in mesh_triangles {
            for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
                let key = (left.min(right), left.max(right));
                let direction = left < right;
                match directions.get_mut(&key) {
                    Some((faces, first)) => {
                        *faces += 1;
                        if *faces == 2 && *first == direction {
                            orientation_conflicts += 1;
                        }
                    }
                    None => {
                        directions.insert(key, (1, direction));
                    }
                }
            }
        }
        let boundary_edges = edge_faces
            .iter()
            .filter(|(_, count)| **count == 1)
            .map(|(edge, _)| *edge)
            .collect::<HashSet<_>>();
        let mut vertex_neighbours: HashMap<u32, HashSet<u32>> = HashMap::new();
        let mut vertex_faces: HashMap<u32, Vec<u32>> = HashMap::new();
        for (index, [a, b, c]) in mesh_triangles.iter().enumerate() {
            for (left, right) in [(*a, *b), (*b, *c), (*c, *a)] {
                vertex_neighbours.entry(left).or_default().insert(right);
                vertex_neighbours.entry(right).or_default().insert(left);
            }
            for vertex in [*a, *b, *c] {
                vertex_faces.entry(vertex).or_default().push(index as u32);
            }
        }
        // Actual fan connectivity, not just edge/face counts: a manifold vertex has
        // exactly one face fan and exactly two boundary edges when it is on a rim.
        let non_manifold_vertices = (0..mesh_positions.len() as u32)
            .filter(|probe| {
                let vertex = *probe;
                let boundary = vertex_neighbours
                    .get(&vertex)
                    .into_iter()
                    .flatten()
                    .filter(|neighbor| {
                        let neighbor = **neighbor;
                        boundary_edges.contains(&(vertex.min(neighbor), vertex.max(neighbor)))
                    })
                    .count();
                let fan_count = vertex_faces
                    .get(&vertex)
                    .map_or(0, |faces| fans(mesh_triangles, vertex, faces).len());
                fan_count > 1 || !matches!(boundary, 0 | 2)
            })
            .count();
        let bounds = bounds_of(mesh_positions);
        Ok(Self {
            vertices: positions.len(),
            triangles: triangles.len(),
            edges: edge_faces.len(),
            euler: positions.len() as i64 - edge_faces.len() as i64 + triangles.len() as i64,
            components,
            non_manifold_vertices,
            non_manifold_edges,
            orientation_conflicts,
            coincident_vertices: surface.coincident_vertices(),
            vertex_sources: surface.vertex_sources().to_vec(),
            loops: boundary_loops(mesh_positions, mesh_triangles),
            self_contacts: check::self_intersection_pairs(positions, triangles),
            bounds,
        })
    }

    /// Extent of the measured surface along each axis.
    pub fn extent(&self) -> [f32; 3] {
        [
            self.bounds[1][0] - self.bounds[0][0],
            self.bounds[1][1] - self.bounds[0][1],
            self.bounds[1][2] - self.bounds[0][2],
        ]
    }

    /// Resolve the profile apertures against the measured loops.
    pub fn apertures(&self, profile: &ControlProfile) -> Result<ApertureResolution, ControlError> {
        resolve_apertures(&self.loops, &profile.apertures)
    }

    /// The declared acceptance conjunction; the first failure names the defect.
    ///
    /// Ordering is deliberate: contacts and topology come before opening
    /// identification, because an ambiguous opening on a broken surface is not a
    /// statement about openings. @req DECL0013
    pub fn qualify(&self, profile: &ControlProfile) -> Result<Qualification, ControlError> {
        if self.self_contacts > 0 {
            return Err(ControlError::SelfContacts {
                pairs: self.self_contacts,
            });
        }
        if self.non_manifold_vertices > 0 {
            return Err(ControlError::NotManifold {
                vertices: self.non_manifold_vertices,
            });
        }
        if self.non_manifold_edges > 0 {
            return Err(ControlError::NonManifoldEdges {
                edges: self.non_manifold_edges,
            });
        }
        if self.orientation_conflicts > 0 {
            return Err(ControlError::InconsistentWinding {
                edges: self.orientation_conflicts,
            });
        }
        if self.components > 1 {
            return Err(ControlError::MultipleComponents {
                components: self.components,
            });
        }
        if self.coincident_vertices > 0 {
            return Err(ControlError::CoincidentVertices {
                vertices: self.coincident_vertices,
            });
        }
        let resolution = self.apertures(profile)?;
        if !resolution.incidental.is_empty() {
            return Err(ControlError::UnexpectedLoops {
                loops: self.loops.len(),
                apertures: resolution.matched.len(),
            });
        }
        // Connected, oriented and manifold were established above, so this quotient is
        // exact: an orientable surface with `b` boundaries has `euler = 2 - 2g - b`.
        let loops = self.loops.len() as i64;
        let genus = genus_of(self.euler, loops);
        let expected = 2 - 2 * i64::from(profile.expected_genus) - loops;
        if self.euler != expected {
            return Err(ControlError::UnexpectedTopology {
                euler: self.euler,
                loops: self.loops.len(),
                genus,
                expected_genus: profile.expected_genus,
            });
        }
        if self.triangles < profile.reduction.minimum_triangles {
            return Err(ControlError::ReductionBelowMinimum {
                triangles: self.triangles,
                minimum: profile.reduction.minimum_triangles,
            });
        }
        if self.triangles > profile.reduction.maximum_triangles {
            return Err(ControlError::ReductionAboveMaximum {
                triangles: self.triangles,
                maximum: profile.reduction.maximum_triangles,
            });
        }
        Ok(Qualification {
            apertures: resolution.matched,
            incidental: resolution.incidental.len(),
            genus,
            expected_genus: profile.expected_genus,
        })
    }
}

/// Unique vertices of the triangles an exterior selection kept, ascending.
pub fn exterior_vertices(surface: &SourceSurface, kept: &[u32]) -> Vec<u32> {
    let mut vertices = kept
        .iter()
        .flat_map(|index| surface.triangles()[*index as usize])
        .collect::<Vec<_>>();
    vertices.sort_unstable();
    vertices.dedup();
    vertices
}

/// Compare a control with its source in both directions.
pub fn deviations(
    source: &SourceSurface,
    kept: &[u32],
    control_positions: &[[f32; 3]],
    control_triangles: &[[u32; 3]],
    coverage: &CoverageLimits,
) -> Result<Deviations, ControlError> {
    coverage.validate()?;
    let control_to_source = deviation(
        control_positions,
        source.positions(),
        source.triangles(),
        coverage,
    );
    let exterior_points = exterior_vertices(source, kept)
        .into_iter()
        .map(|vertex| source.positions()[vertex as usize])
        .collect::<Vec<_>>();
    let source_exterior_to_control = deviation(
        &exterior_points,
        control_positions,
        control_triangles,
        coverage,
    );
    Ok(Deviations {
        control_to_source,
        source_exterior_to_control,
    })
}

/// Nearest-surface distance summary from `points` to one mesh.
pub fn deviation(
    points: &[[f32; 3]],
    mesh_positions: &[[f32; 3]],
    mesh_triangles: &[[u32; 3]],
    coverage: &CoverageLimits,
) -> Deviation {
    if points.is_empty() || mesh_triangles.is_empty() {
        return Deviation {
            points: points.len(),
            max: 0.0,
            mean: 0.0,
            p95: 0.0,
            covered: 0,
            coverage: 0.0,
        };
    }
    let distances = check::nearest_distances(points, mesh_positions, mesh_triangles);
    let count = distances.len() as f64;
    let sum = distances.iter().map(|value| f64::from(*value)).sum::<f64>();
    let covered = distances
        .iter()
        .filter(|value| **value <= coverage.tolerance)
        .count();
    Deviation {
        points: distances.len(),
        max: distances.iter().copied().fold(0.0f32, f32::max),
        mean: (sum / count) as f32,
        p95: percentile(&distances, 0.95),
        covered,
        coverage: covered as f32 / distances.len() as f32,
    }
}

/// Percentile with linear interpolation between closest ranks.
fn percentile(values: &[f32], fraction: f32) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    let position = f64::from(fraction) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    let weight = (position - lower as f64) as f32;
    match lower == upper {
        true => sorted[lower],
        false => sorted[lower] * (1.0 - weight) + sorted[upper] * weight,
    }
}

fn bounds_of(positions: &[[f32; 3]]) -> [[f32; 3]; 2] {
    positions.iter().fold(
        [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]],
        |[mut min, mut max], point| {
            for axis in 0..3 {
                min[axis] = min[axis].min(point[axis]);
                max[axis] = max[axis].max(point[axis]);
            }
            [min, max]
        },
    )
}
