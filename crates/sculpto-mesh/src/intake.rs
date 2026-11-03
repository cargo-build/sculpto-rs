//! Garment intake audit: the measured state a mesh arrives in, and the route
//! that follows from it — fit the existing surface, repair a bounded
//! construction defect, or split the parts. Open boundaries at collar, cuffs
//! and hem are normal cloth, not defects.

use std::collections::{HashMap, HashSet};

use crate::aura::{Segment, extent, plane_axes, up_axis};
use crate::classify::{ClassifyError, component_groups, major_components};

/// Sections sampled across the garment's height.
const SAMPLES: usize = 6;
/// Point-touches at coarse-mesh band corners register as intersecting pairs;
/// below this fraction of triangles they are noise, not a construction defect.
const SELF_INTERSECTION_FLOOR: f32 = 0.05;

/// The route the adaptation should take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The surface is coherent: press onto the corset-aura as it is.
    Fit,
    /// A bounded construction defect to fix before fitting; `defect` names it.
    Repair { defect: &'static str },
    /// Several garments in one mesh: split and fit each part.
    SplitParts,
}

impl Route {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Fit => "fit",
            Self::Repair { .. } => "repair",
            Self::SplitParts => "split parts",
        }
    }
}

/// One boundary loop of the mesh: an opening such as a collar, cuff or hem.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Opening {
    /// Boundary edges in the loop.
    pub edges: usize,
    /// Edge-length-weighted midpoint of the boundary curve, so a differently tessellated
    /// opening reports the same position. @req DECL0013
    pub center: [f32; 3],
}

/// The audit result: measured state plus the route it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct IntakeReport {
    /// Garment vertex count.
    pub vertices: usize,
    /// Garment triangle count.
    pub triangles: usize,
    /// Referenced positions after exact seam identification; render data is unchanged.
    pub topology_vertices: usize,
    /// Components in the input index graph, before identifying render seams.
    pub indexed_components: usize,
    /// Major connected components (≥ 10% of referenced vertices each).
    pub major_components: usize,
    /// Each major component's share of referenced vertices, in percent.
    pub shares: Vec<f32>,
    /// Boundary-loop openings, largest first.
    pub openings: Vec<Opening>,
    /// Most chains any section showed strictly inside the garment's own outer
    /// shell: 0 is a clean shell, 1+ means interior surfaces.
    pub max_interior_chains: usize,

    /// Garment/self triangle-intersection pairs at rest.
    pub self_intersections: usize,
    /// The route verdict.
    pub route: Route,
    /// Why, one line per check.
    pub evidence: Vec<String>,
}

/// Audit a garment against its body and name the route.
pub fn audit(
    body: &[[f32; 3]],
    _body_triangles: &[[u32; 3]],
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
) -> Result<IntakeReport, ClassifyError> {
    if body.is_empty() {
        return Err(ClassifyError::EmptyBody);
    }
    if garment.is_empty() {
        return Err(ClassifyError::EmptyGarment);
    }
    if garment_triangles.is_empty() {
        return Err(ClassifyError::NoGarmentTriangles);
    }
    for (triangle, [a, b, c]) in garment_triangles.iter().enumerate() {
        if [*a, *b, *c]
            .iter()
            .any(|index| *index as usize >= garment.len())
        {
            return Err(ClassifyError::BadIndex {
                triangle,
                vertex: [*a, *b, *c].iter().copied().max().unwrap_or_default() as usize,
            });
        }
    }
    let axis = up_axis(body);
    let (body_min, body_max) = extent(body, axis);
    let height = body_max - body_min;
    if height <= f32::EPSILON {
        return Err(ClassifyError::FlatBody);
    }

    let vertices = garment.len();
    let indexed_components = component_groups(garment, garment_triangles).len();
    let topology = crate::topology::position_topology(garment, garment_triangles);
    let garment = topology.positions.as_slice();
    let garment_triangles = topology.triangles.as_slice();
    let parts = major_components(garment, garment_triangles);
    let component_total = component_groups(garment, garment_triangles).len();
    let referenced: usize = parts.iter().map(|(positions, _)| positions.len()).sum();
    let shares: Vec<f32> = parts
        .iter()
        .map(|(positions, _)| 100.0 * positions.len() as f32 / referenced.max(1) as f32)
        .collect();

    let openings = boundary_loops(garment, garment_triangles);
    let max_interior_chains = interior_chains(garment, garment_triangles, axis, height);
    let self_intersections = crate::check::self_intersection_pairs(garment, garment_triangles);
    let mut faces = HashSet::new();
    let ambiguous_faces = garment_triangles.iter().any(|triangle| {
        let mut key = *triangle;
        key.sort_unstable();
        key[0] == key[1] || key[1] == key[2] || !faces.insert(key)
    });

    let mut evidence = Vec::new();
    if vertices != garment.len() || indexed_components != component_total {
        evidence.push(format!(
            "render/index topology: {vertices} vertices, {indexed_components} components; \
             exact-position topology: {} vertices, {component_total} components; \
             coincident layers remain ambiguous, render attributes are unchanged",
            garment.len()
        ));
    }
    if parts.is_empty() && component_total > 1 {
        evidence.push(format!(
            "vertex graph splits into {component_total} components, none reaching 10% \
             (UV seams?): the audit treats the union"
        ));
    }

    let route = if parts.len() >= 2 {
        let listed = shares
            .iter()
            .map(|share| format!("{share:.0}%"))
            .collect::<Vec<_>>()
            .join("/");
        evidence.push(format!(
            "{} major components ({listed}): several garments in one mesh",
            parts.len()
        ));
        Route::SplitParts
    } else if ambiguous_faces {
        evidence.push(
            "coincident or collapsed faces need construction review before seam welding"
                .to_string(),
        );
        Route::Repair {
            defect: "coincident or collapsed faces",
        }
    } else if self_intersections as f32 > SELF_INTERSECTION_FLOOR * garment_triangles.len() as f32 {
        evidence.push(format!(
            "{self_intersections} self-intersecting triangle pairs at rest \
             (above the {:.0}% noise floor)",
            100.0 * SELF_INTERSECTION_FLOOR
        ));
        Route::Repair {
            defect: "self-intersections at rest",
        }
    } else if openings.is_empty() {
        evidence.push(
            "no physical boundary openings: a closed render shell is not a prepared cloth surface"
                .to_string(),
        );
        Route::Repair {
            defect: "closed surface requires a cloth/control surface",
        }
    } else if max_interior_chains >= 1 {
        evidence.push(format!(
            "{max_interior_chains} chain(s) inside the outer shell in a section: \
             interior surfaces"
        ));
        Route::Repair {
            defect: "interior surfaces",
        }
    } else {
        let described = openings
            .iter()
            .map(|opening| {
                format!(
                    "{} edges at {:.2}",
                    opening.edges,
                    (opening.center[axis] - body_min) / height
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        evidence.push(format!(
            "one coherent shell, {} openings ({}): collar, cuffs and hem are \
             cloth, not defects",
            openings.len(),
            described
        ));
        Route::Fit
    };
    Ok(IntakeReport {
        vertices,
        triangles: garment_triangles.len(),
        topology_vertices: garment.len(),
        indexed_components,
        major_components: parts.len(),
        shares,
        openings,
        max_interior_chains,
        self_intersections,
        route,
        evidence,
    })
}

/// Boundary loops: edges used by exactly one triangle, chained through shared
/// vertices. Wire edges (a dangling vertex) close into their own group.
fn boundary_loops(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<Opening> {
    let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
    for [a, b, c] in triangles {
        for (left, right) in [(a, b), (b, c), (c, a)] {
            let key = if left <= right {
                (*left, *right)
            } else {
                (*right, *left)
            };
            *counts.entry(key).or_default() += 1;
        }
    }
    let mut adjacency: HashMap<u32, Vec<u32>> = HashMap::new();
    for ((left, right), count) in counts {
        if count == 1 {
            adjacency.entry(left).or_default().push(right);
            adjacency.entry(right).or_default().push(left);
        }
    }
    let mut loops = Vec::new();
    let mut visited: HashMap<u32, bool> = HashMap::new();
    for start in adjacency.keys().copied().collect::<Vec<_>>() {
        if visited.get(&start).copied().unwrap_or(false) {
            continue;
        }
        let mut edges = 0usize;
        let mut walked = Vec::new();
        let mut closed = false;
        let mut current = start;
        let mut previous = u32::MAX;
        loop {
            if visited.get(&current).copied().unwrap_or(false) {
                break;
            }
            visited.insert(current, true);
            walked.push(current);
            let neighbors = &adjacency[&current];
            let next = neighbors
                .iter()
                .copied()
                .find(|neighbor| {
                    *neighbor != previous && !visited.get(neighbor).copied().unwrap_or(false)
                })
                .or_else(|| {
                    neighbors
                        .iter()
                        .copied()
                        .find(|neighbor| *neighbor == start)
                });
            edges += 1;
            let Some(next) = next else {
                break;
            };
            if next == start {
                closed = true;
                break;
            }
            previous = current;
            current = next;
        }
        if !walked.is_empty() {
            let (center, _) = crate::control::curve_center(positions, &walked, closed);
            loops.push(Opening { edges, center });
        }
    }
    loops.sort_by_key(|opening| std::cmp::Reverse(opening.edges));
    loops
}

/// The most interior chains over sampled sections: chains that lie strictly
/// inside the garment's own outermost chain in the same section.
fn interior_chains(
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
    axis: usize,
    height: f32,
) -> usize {
    let (gmin, gmax) = extent(garment, axis);
    let margin = 0.02 * height;
    let step = if gmax - margin > gmin + margin {
        (gmax - margin - (gmin + margin)) / (SAMPLES - 1) as f32
    } else {
        0.0
    };
    (0..SAMPLES)
        .map(|index| {
            let cut = gmin + margin + step * index as f32;
            let chains = section_groups(garment, garment_triangles, axis, cut);
            interior_flags(&chains)
                .into_iter()
                .filter(|inside| *inside)
                .count()
        })
        .fold(0, usize::max)
}

struct SectionChain {
    segments: Vec<Segment>,
    closed: bool,
}

fn strictly_inside(point: [f32; 2], chain: &SectionChain) -> bool {
    let [x, y] = point.map(f64::from);
    let mut crossings = 0;
    for (start, end) in &chain.segments {
        let [ax, ay] = start.map(f64::from);
        let [bx, by] = end.map(f64::from);
        let side = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
        if side == 0.0 && x >= ax.min(bx) && x <= ax.max(bx) && y >= ay.min(by) && y <= ay.max(by) {
            return false;
        }
        if (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay) {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

fn interior_flags(chains: &[SectionChain]) -> Vec<bool> {
    chains
        .iter()
        .enumerate()
        .map(|(index, chain)| {
            chains.iter().enumerate().any(|(other, enclosing)| {
                index != other
                    && enclosing.closed
                    && chain.segments.iter().all(|(a, b)| {
                        [*a, *b, [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]]
                            .into_iter()
                            .all(|point| strictly_inside(point, enclosing))
                    })
            })
        })
        .collect()
}

/// Source-edge identities connect section segments without merging nearby layers.
fn section_segment(
    positions: &[[f32; 3]],
    triangle: [u32; 3],
    axis: usize,
    height: f32,
) -> Option<([[u32; 2]; 2], Segment)> {
    let (u, v) = plane_axes(axis);
    if triangle
        .iter()
        .all(|index| positions[*index as usize][axis] == height)
    {
        return None;
    }
    let mut points = Vec::with_capacity(3);
    for (a, b) in [
        (triangle[0], triangle[1]),
        (triangle[1], triangle[2]),
        (triangle[2], triangle[0]),
    ] {
        let key = [a.min(b), a.max(b)];
        let pa = positions[key[0] as usize];
        let pb = positions[key[1] as usize];
        let da = pa[axis] - height;
        let db = pb[axis] - height;
        if da == 0.0 {
            points.push(([key[0], key[0]], [pa[u], pa[v]]));
        }
        if db == 0.0 {
            points.push(([key[1], key[1]], [pb[u], pb[v]]));
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            let t = da / (da - db);
            points.push((
                key,
                [pa[u] + t * (pb[u] - pa[u]), pa[v] + t * (pb[v] - pa[v])],
            ));
        }
    }
    points.sort_unstable_by_key(|(key, _)| *key);
    points.dedup_by_key(|(key, _)| *key);
    match points.as_slice() {
        [(a, pa), (b, pb)] if pa != pb => Some(([*a, *b], (*pa, *pb))),
        _ => None,
    }
}

fn section_groups(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    axis: usize,
    height: f32,
) -> Vec<SectionChain> {
    let mut unique = HashSet::new();
    let segments: Vec<_> = triangles
        .iter()
        .filter_map(|triangle| section_segment(positions, *triangle, axis, height))
        .filter(|(keys, _)| unique.insert(*keys))
        .collect();
    let mut adjacency: HashMap<[u32; 2], Vec<usize>> = HashMap::new();
    segments.iter().enumerate().for_each(|(index, (keys, _))| {
        keys.iter()
            .for_each(|key| adjacency.entry(*key).or_default().push(index));
    });
    let mut visited = vec![false; segments.len()];
    let mut chains = Vec::new();
    for start in 0..segments.len() {
        if visited[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut chain = SectionChain {
            segments: Vec::new(),
            closed: true,
        };
        while let Some(index) = stack.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            let (keys, segment) = segments[index];
            chain.segments.push(segment);
            keys.iter().for_each(|key| {
                let neighbors = &adjacency[key];
                chain.closed &= neighbors.len() == 2;
                stack.extend(neighbors.iter().copied().filter(|next| !visited[*next]));
            });
        }
        chains.push(chain);
    }
    chains
}

/// What stripping interior surfaces did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StripReport {
    /// Triangles removed (their nearest chain was interior).
    pub removed: usize,
    /// Triangles kept.
    pub kept: usize,
    /// Height bands evaluated.
    pub bands: usize,
}

/// Remove triangles whose centroid lies in a contour chain strictly inside
/// the garment's own outer shell — the repair the audit's `interior
/// surfaces` route names. Chain-level decisions only: a triangle follows its
/// nearest chain, so a shell panel that briefly dips inward keeps its
/// triangles unless its whole chain is interior in that band.
pub fn strip_interior(
    body: &[[f32; 3]],
    _body_triangles: &[[u32; 3]],
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
) -> Result<(Vec<[u32; 3]>, StripReport), ClassifyError> {
    if body.is_empty() {
        return Err(ClassifyError::EmptyBody);
    }
    if garment.is_empty() {
        return Err(ClassifyError::EmptyGarment);
    }
    if garment_triangles.is_empty() {
        return Err(ClassifyError::NoGarmentTriangles);
    }
    let axis = up_axis(body);
    let (plane_u, plane_v) = plane_axes(axis);
    let topology = crate::topology::position_topology(garment, garment_triangles);
    let (gmin, gmax) = extent(garment, axis);

    let bands = SAMPLES * 4;
    let span = (gmax - gmin).max(f32::EPSILON);
    let band_layouts: Vec<Band> = (0..bands)
        .map(|band| {
            let cut = gmin + span * (band as f32 + 0.5) / bands as f32;
            let chains = section_groups(&topology.positions, &topology.triangles, axis, cut);
            let interior = interior_flags(&chains);
            Band { chains, interior }
        })
        .collect();

    let kept: Vec<[u32; 3]> = garment_triangles
        .iter()
        .filter(|[a, b, c]| {
            let centroid = [
                (garment[*a as usize][axis]
                    + garment[*b as usize][axis]
                    + garment[*c as usize][axis])
                    / 3.0,
                (garment[*a as usize][plane_u]
                    + garment[*b as usize][plane_u]
                    + garment[*c as usize][plane_u])
                    / 3.0,
                (garment[*a as usize][plane_v]
                    + garment[*b as usize][plane_v]
                    + garment[*c as usize][plane_v])
                    / 3.0,
            ];
            let band =
                (((centroid[0] - gmin) / span * bands as f32).floor() as usize).min(bands - 1);
            let Band { chains, interior } = &band_layouts[band];

            if chains.is_empty() {
                return true;
            }
            let point = [centroid[1], centroid[2]];
            let nearest = chains
                .iter()
                .zip(interior)
                .map(|(chain, inside)| {
                    (
                        *inside,
                        chain
                            .segments
                            .iter()
                            .flat_map(|(start, end)| [start, end])
                            .map(|p| (p[0] - point[0]).hypot(p[1] - point[1]))
                            .fold(f32::INFINITY, f32::min),
                    )
                })
                .min_by(|left, right| left.1.total_cmp(&right.1));
            !nearest.is_some_and(|(inside, _)| inside)
        })
        .copied()
        .collect();
    let removed = garment_triangles.len() - kept.len();
    let kept_count = kept.len();
    Ok((
        kept,
        StripReport {
            removed,
            kept: kept_count,
            bands,
        },
    ))
}

/// One band's chains and their interior flags.
struct Band {
    chains: Vec<SectionChain>,
    interior: Vec<bool>,
}
