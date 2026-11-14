//! The accumulated obstacle surface a stack fit carries forward. @req DECL0013 DECL0015
//!
//! A stack-aware fit dresses the body in order: layer *k* is pushed against the
//! composite of the body and the already-fitted layers `1..k-1`, never against
//! the nude body every time. [`AccumulatedSurface`] owns that composite: it
//! concatenates validated surfaces and remembers which layer every triangle
//! came from, so a fit report can prove which surface a step actually graded
//! against — the audit rule is that no step may claim a stacked fit while
//! querying the body alone.
//!
//! Grading the composite is [`AccumulatedSurface::layered_visible_depths`], the
//! outermost-member rule: a garment vertex grades against the highest-order
//! member it is *behind*, never against the globally-nearest triangle. Bare
//! concatenation is not enough — a vertex pushed deep through an inner layer
//! (a cloak through the robe into the body) ends up nearer the body's covered
//! triangles, and nearest-triangle grading would push it outward from the
//! body, straight back into the robe; the exact-Bijin cloak chain measured
//! that failure at `cloak vs robe` 2.184120 with 27 violations before this
//! rule. The outermost-member rule keeps the precedent's chained-push
//! acceptance: over covered regions the obstacle is the covering layer, over
//! bare skin it is the body. The per-vertex classification (facing, the
//! DECL0015 contact-point occlusion) is [`crate::check`]'s, so a layered
//! contact cannot drift from `garment depth`.
//!
//! Every appended surface must be derivable as an obstacle: non-empty, finite,
//! in-range, without exactly-degenerate triangles (they cannot name an outward
//! side, so a push against them has no direction) and without non-manifold
//! edges (more than two faces per edge make the winding ambiguous). Open
//! boundaries stay legal: garments are open shells at collars, hems and cuffs.

use std::collections::HashMap;
use std::ops::Range;

use crate::check::{SurfaceIndex, VisibleDepth, occlusion_hides_vertex, unit_triangle_normal};
use crate::surface_util::vertex_normals;
use crate::wrap::{FitError, Mesh, validate_geometry};

/// The layer id the body always carries in a provenance list.
pub const BODY_SURFACE: &str = "body";

/// One member surface of an [`AccumulatedSurface`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccumulatedLayerInfo {
    /// `body`, or the fitted layer's declared id.
    pub id: String,
    /// The body's `0`, or the layer's declared rank.
    pub order: u32,
    /// Appended vertices.
    pub vertices: usize,
    /// Appended triangles.
    pub triangles: usize,
    /// Triangle index range inside the composite, for provenance lookup.
    pub triangle_range: Range<usize>,
}

/// Why a surface cannot become part of the accumulated obstacle.
#[derive(Debug, Clone, PartialEq)]
pub enum AccumulatedError {
    /// The surface has no vertices or no triangles.
    Empty {
        /// `body`, or the layer's declared id.
        surface: String,
    },
    /// A position is not finite.
    NonFinitePosition {
        /// `body`, or the layer's declared id.
        surface: String,
        /// Offending vertex index.
        vertex: usize,
    },
    /// A triangle references a missing vertex.
    FaceIndex {
        /// `body`, or the layer's declared id.
        surface: String,
        /// Offending triangle index.
        triangle: usize,
        /// The missing vertex id.
        vertex: u32,
    },
    /// A triangle has exactly zero area, so it cannot name an outward side.
    DegenerateTriangle {
        /// `body`, or the layer's declared id.
        surface: String,
        /// Count of exactly-degenerate triangles.
        count: usize,
    },
    /// An edge carries more than two faces, so winding is ambiguous.
    NonManifoldEdges {
        /// `body`, or the layer's declared id.
        surface: String,
        /// Count of edges with more than two faces.
        count: usize,
    },
    /// A duplicate layer id was appended.
    DuplicateLayer {
        /// The repeated id.
        id: String,
    },
    /// A layer was appended at or below an already-present rank.
    OrderNotIncreasing {
        /// The offending layer's id.
        id: String,
        /// Its declared order.
        order: u32,
        /// The order the previous layer carries.
        previous: u32,
    },
}

impl std::fmt::Display for AccumulatedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty { surface } => {
                write!(formatter, "`{surface}` has no vertices or no triangles")
            }
            Self::NonFinitePosition { surface, vertex } => {
                write!(formatter, "`{surface}` vertex {vertex} is not finite")
            }
            Self::FaceIndex {
                surface,
                triangle,
                vertex,
            } => write!(
                formatter,
                "`{surface}` triangle {triangle} references missing vertex {vertex}"
            ),
            Self::DegenerateTriangle { surface, count } => write!(
                formatter,
                "`{surface}` has {count} exactly-degenerate triangle(s); an obstacle face must name an outward side"
            ),
            Self::NonManifoldEdges { surface, count } => write!(
                formatter,
                "`{surface}` has {count} non-manifold edge(s) (more than two faces); winding cannot name a consistent outward field"
            ),
            Self::DuplicateLayer { id } => {
                write!(
                    formatter,
                    "layer `{id}` is already part of the accumulated surface"
                )
            }
            Self::OrderNotIncreasing {
                id,
                order,
                previous,
            } => write!(
                formatter,
                "layer `{id}` declares order {order}, not above the previous layer's {previous}"
            ),
        }
    }
}

impl std::error::Error for AccumulatedError {}

/// One graded garment vertex and the member surface it graded against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayeredDepth {
    /// The graded vertex: index, depth and push direction.
    pub depth: VisibleDepth,
    /// Index into the accumulated surface's member list.
    pub member: usize,
}

/// The composite obstacle: the body plus the fitted layers so far.
///
/// Built by [`AccumulatedSurface::from_body`] and extended one fitted layer at
/// a time by [`AccumulatedSurface::push_layer`]. The composite is what a stack
/// fit grades and pushes against through
/// [`AccumulatedSurface::layered_visible_depths`]; the per-triangle origin is
/// [`AccumulatedSurface::surface_of_triangle`].
#[derive(Debug, Clone, PartialEq)]
pub struct AccumulatedSurface {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    layers: Vec<AccumulatedLayerInfo>,
}

impl AccumulatedSurface {
    /// Start the composite from the body.
    ///
    /// # Errors
    /// [`AccumulatedError`] when the body cannot serve as an obstacle.
    pub fn from_body(
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<Self, AccumulatedError> {
        let (vertices, count) = validate(BODY_SURFACE, positions, triangles)?;
        Ok(Self {
            positions: positions.to_vec(),
            triangles: triangles.to_vec(),
            layers: vec![AccumulatedLayerInfo {
                id: BODY_SURFACE.to_string(),
                order: 0,
                vertices,
                triangles: count,
                triangle_range: 0..count,
            }],
        })
    }

    /// Append one fitted layer as an obstacle for the layers above it.
    ///
    /// The composite never reorders: `id` must be new and `order` strictly
    /// above the last appended layer's, so the dressing sequence stays the one
    /// the manifest declared.
    ///
    /// # Errors
    /// [`AccumulatedError`] when the layer cannot serve as an obstacle, repeats
    /// an id, or does not dress above the previous layer.
    pub fn push_layer(
        &mut self,
        id: &str,
        order: u32,
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<(), AccumulatedError> {
        if self.layers.iter().any(|layer| layer.id == id) {
            return Err(AccumulatedError::DuplicateLayer { id: id.to_string() });
        }
        let previous = self.layers.last().expect("from_body seeds the body").order;
        if order <= previous {
            return Err(AccumulatedError::OrderNotIncreasing {
                id: id.to_string(),
                order,
                previous,
            });
        }
        validate(id, positions, triangles)?;
        let vertex_base = self.positions.len() as u32;
        let triangle_start = self.triangles.len();
        self.positions.extend_from_slice(positions);
        self.triangles.extend(triangles.iter().map(|face| {
            [
                face[0] + vertex_base,
                face[1] + vertex_base,
                face[2] + vertex_base,
            ]
        }));
        self.layers.push(AccumulatedLayerInfo {
            id: id.to_string(),
            order,
            vertices: positions.len(),
            triangles: triangles.len(),
            triangle_range: triangle_start..self.triangles.len(),
        });
        Ok(())
    }

    /// The full composite as one mesh, for validation and geometry digests.
    pub fn obstacle(&self) -> Mesh<'_> {
        (&self.positions, &self.triangles)
    }

    /// Total composite vertices.
    pub fn vertices(&self) -> usize {
        self.positions.len()
    }

    /// Total composite triangles.
    pub fn triangles(&self) -> usize {
        self.triangles.len()
    }

    /// Member surfaces in dressing order, the body first.
    pub fn layers(&self) -> &[AccumulatedLayerInfo] {
        &self.layers
    }

    /// The member a composite triangle came from: `body` or a layer id.
    ///
    /// # Panics
    /// Only on an out-of-range composite index, which this type cannot build.
    pub fn surface_of_triangle(&self, triangle: usize) -> &str {
        self.layers
            .iter()
            .find(|layer| layer.triangle_range.contains(&triangle))
            .map(|layer| layer.id.as_str())
            .unwrap_or(BODY_SURFACE)
    }

    /// The provenance label, for example `accumulated[body, robe]`.
    pub fn surface_label(&self) -> String {
        let ids: Vec<&str> = self.layers.iter().map(|layer| layer.id.as_str()).collect();
        format!("accumulated[{}]", ids.join(", "))
    }

    /// The graded depths of `garment` against the composite, outermost member
    /// first: every garment vertex grades against the highest-order member it
    /// is behind, with that member's nearest triangle supplying the depth, the
    /// push direction and the facing/occlusion classification.
    ///
    /// A vertex behind a lower member only (bare skin under an open hem)
    /// grades against that member, so bare-skin contacts stay measured; a
    /// vertex behind no member is outside and not graded. The classification
    /// is [`visible_depths`]'s per member, so the graded set cannot drift from
    /// `garment depth` against any single member.
    ///
    /// # Errors
    /// [`FitError`] when the garment cannot be graded: the shared fit-input
    /// defects of the garment and of one member surface.
    pub fn layered_visible_depths(&self, garment: Mesh<'_>) -> Result<Vec<LayeredDepth>, FitError> {
        type MemberMesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);
        let members: Vec<MemberMesh> = self
            .layers
            .iter()
            .map(|member| self.member_mesh(member))
            .collect();
        let indexes: Vec<SurfaceIndex> = members
            .iter()
            .map(|(positions, triangles)| SurfaceIndex::build(positions, triangles))
            .collect();
        validate_geometry(garment, (&members[0].0, &members[0].1))?;
        let garment_surface = SurfaceIndex::build(garment.0, garment.1);
        let garment_normals = vertex_normals(garment.0, garment.1);
        let mut graded = Vec::new();
        for (vertex, &point) in garment.0.iter().enumerate() {
            // Highest order first: the first member the vertex is behind is
            // its contact surface; lower members are never consulted for it.
            for member in (0..self.layers.len()).rev() {
                let (positions, triangles) = &members[member];
                let Some(closest) = indexes[member].closest(point) else {
                    return Err(FitError::EmptyBodyFaces);
                };
                let face = triangles[closest.triangle];
                let corners = [
                    positions[face[0] as usize],
                    positions[face[1] as usize],
                    positions[face[2] as usize],
                ];
                let Some(normal) = unit_triangle_normal(corners) else {
                    // A degenerate nearest face cannot name a side; the member
                    // reads the vertex as outside, exactly like `visible_depths`.
                    continue;
                };
                let offset = [
                    f64::from(point[0]) - f64::from(closest.point[0]),
                    f64::from(point[1]) - f64::from(closest.point[1]),
                    f64::from(point[2]) - f64::from(closest.point[2]),
                ];
                let signed = offset[0] * normal[0] + offset[1] * normal[1] + offset[2] * normal[2];
                if signed >= 0.0 {
                    continue;
                }
                let facing = garment_normals[vertex][0] as f64 * normal[0]
                    + garment_normals[vertex][1] as f64 * normal[1]
                    + garment_normals[vertex][2] as f64 * normal[2];
                let hidden = occlusion_hides_vertex(
                    &garment_surface,
                    garment.1,
                    closest.point,
                    normal,
                    vertex,
                );
                if facing >= 0.0 && !hidden {
                    let depth = -signed as f32;
                    if !depth.is_finite() {
                        return Err(FitError::NonFiniteResult { vertex });
                    }
                    graded.push(LayeredDepth {
                        depth: VisibleDepth {
                            vertex,
                            depth,
                            normal: Some(normal),
                        },
                        member,
                    });
                }
                break;
            }
        }
        Ok(graded)
    }

    /// Count the graded vertices of `garment` per obstacle member.
    ///
    /// The graded set is [`AccumulatedSurface::layered_visible_depths`]'s, the
    /// same rule a stack fit pushes; each graded vertex is attributed to the
    /// member it graded against. Zeros are kept, so the breakdown names every
    /// member of the accumulated surface, not only the touched ones — the
    /// proof that an outer layer evaluated against lower layers, not the body.
    ///
    /// # Errors
    /// [`FitError`] when the garment cannot be graded against the composite.
    pub fn contact_breakdown(&self, garment: Mesh<'_>) -> Result<Vec<(String, usize)>, FitError> {
        let graded = self.layered_visible_depths(garment)?;
        let mut counts: Vec<(String, usize)> = self
            .layers
            .iter()
            .map(|layer| (layer.id.clone(), 0))
            .collect();
        for layered in &graded {
            counts[layered.member].1 += 1;
        }
        Ok(counts)
    }

    /// One member's geometry remapped into its own arena.
    fn member_mesh(&self, member: &AccumulatedLayerInfo) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut positions = Vec::with_capacity(member.vertices);
        let mut remap: HashMap<u32, u32> = HashMap::with_capacity(member.vertices);
        let mut triangles = Vec::with_capacity(member.triangles);
        for face in &self.triangles[member.triangle_range.clone()] {
            let mut local = [0u32; 3];
            for (corner, &vertex) in face.iter().enumerate() {
                let next = remap.len() as u32;
                let mapped = *remap.entry(vertex).or_insert(next);
                if mapped == next {
                    positions.push(self.positions[vertex as usize]);
                }
                local[corner] = mapped;
            }
            triangles.push(local);
        }
        (positions, triangles)
    }
}

/// One surface must be derivable as an obstacle before it joins the composite.
fn validate(
    surface: &str,
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> Result<(usize, usize), AccumulatedError> {
    if positions.is_empty() || triangles.is_empty() {
        return Err(AccumulatedError::Empty {
            surface: surface.to_string(),
        });
    }
    if let Some(vertex) =
        (0..positions.len()).find(|&vertex| positions[vertex].iter().any(|lane| !lane.is_finite()))
    {
        return Err(AccumulatedError::NonFinitePosition {
            surface: surface.to_string(),
            vertex,
        });
    }
    if let Some((triangle, vertex)) = triangles.iter().enumerate().find_map(|(triangle, face)| {
        face.iter()
            .find(|&&vertex| vertex as usize >= positions.len())
            .map(|&vertex| (triangle, vertex))
    }) {
        return Err(AccumulatedError::FaceIndex {
            surface: surface.to_string(),
            triangle,
            vertex,
        });
    }
    let degenerate = triangles
        .iter()
        .filter(|&&[a, b, c]| face_normal(positions, &[a, b, c]) == [0.0, 0.0, 0.0])
        .count();
    if degenerate > 0 {
        return Err(AccumulatedError::DegenerateTriangle {
            surface: surface.to_string(),
            count: degenerate,
        });
    }
    let mut edges: HashMap<[u64; 2], usize> = HashMap::new();
    for face in triangles {
        for pair in [
            [face[0] as u64, face[1] as u64],
            [face[1] as u64, face[2] as u64],
            [face[2] as u64, face[0] as u64],
        ] {
            let key = [pair[0].min(pair[1]), pair[0].max(pair[1])];
            *edges.entry(key).or_insert(0) += 1;
        }
    }
    let non_manifold = edges.values().filter(|&&faces| faces > 2).count();
    if non_manifold > 0 {
        return Err(AccumulatedError::NonManifoldEdges {
            surface: surface.to_string(),
            count: non_manifold,
        });
    }
    Ok((positions.len(), triangles.len()))
}

/// Outward (winding-order) normal of one triangle, unnormalized.
fn face_normal(positions: &[[f32; 3]], face: &[u32; 3]) -> [f32; 3] {
    let a = positions[face[0] as usize];
    let b = positions[face[1] as usize];
    let c = positions[face[2] as usize];
    let edge_ba = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let edge_ca = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    [
        edge_ba[1] * edge_ca[2] - edge_ba[2] * edge_ca[1],
        edge_ba[2] * edge_ca[0] - edge_ba[0] * edge_ca[2],
        edge_ba[0] * edge_ca[1] - edge_ba[1] * edge_ca[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::visible_depths;

    /// Outward-wound unit cube of half-extent `half`.
    fn cube(half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let positions = [
            [-half, -half, -half],
            [half, -half, -half],
            [half, half, -half],
            [-half, half, -half],
            [-half, -half, half],
            [half, -half, half],
            [half, half, half],
            [-half, half, half],
        ]
        .to_vec();
        let triangles = vec![
            [4u32, 5, 6],
            [4, 6, 7],
            [1, 0, 3],
            [1, 3, 2],
            [1, 2, 6],
            [1, 6, 5],
            [0, 4, 7],
            [0, 7, 3],
            [3, 7, 6],
            [3, 6, 2],
            [0, 1, 5],
            [0, 5, 4],
        ];
        (positions, triangles)
    }

    /// One sheet facing `+x` at `x`.
    fn sheet(x: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        (
            vec![[x, 0.0, 0.0], [x, 0.1, 0.0], [x, 0.0, 0.1]],
            vec![[0u32, 1, 2]],
        )
    }

    #[test]
    fn the_body_seeds_the_provenance() {
        let (positions, triangles) = cube(0.4);
        let accumulated = AccumulatedSurface::from_body(&positions, &triangles).expect("a body");
        assert_eq!(accumulated.surface_label(), "accumulated[body]");
        assert_eq!(accumulated.vertices(), positions.len());
        assert_eq!(accumulated.triangles(), triangles.len());
        assert_eq!(accumulated.layers().len(), 1);
        assert_eq!(accumulated.layers()[0].order, 0);
        assert_eq!(accumulated.surface_of_triangle(0), "body");
        assert_eq!(accumulated.surface_of_triangle(triangles.len() - 1), "body");
        // One member only: the layered rule is plain `visible_depths`.
        let (garment, faces) = sheet(0.3);
        let body_mesh = (&positions as &[[f32; 3]], &triangles as &[[u32; 3]]);
        let graded = accumulated
            .layered_visible_depths((&garment, &faces))
            .expect("gradable");
        let plain = visible_depths((&garment, &faces), body_mesh).expect("gradable");
        assert_eq!(
            graded.iter().map(|each| each.depth).collect::<Vec<_>>(),
            plain
        );
    }

    #[test]
    fn a_fitted_layer_offsets_its_triangles_and_names_them() {
        let (body, body_faces) = cube(0.4);
        let (robe, robe_faces) = cube(0.5);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("robe", 40, &robe, &robe_faces)
            .expect("a valid layer");

        assert_eq!(accumulated.surface_label(), "accumulated[body, robe]");
        assert_eq!(accumulated.vertices(), body.len() + robe.len());
        assert_eq!(accumulated.triangles(), body_faces.len() + robe_faces.len());
        let robe_start = body_faces.len();
        assert_eq!(accumulated.surface_of_triangle(robe_start - 1), "body");
        assert_eq!(accumulated.surface_of_triangle(robe_start), "robe");
        let member = &accumulated.layers()[1];
        assert_eq!(member.triangle_range, robe_start..accumulated.triangles());
        // The appended triangles address the composite vertex arena.
        for face in &accumulated.triangles[robe_start..] {
            assert!(face.iter().all(|&vertex| (vertex as usize) >= body.len()));
        }
    }

    #[test]
    fn the_outermost_member_owns_a_vertex_the_body_could_steal() {
        // A sheet at 0.44 sits outside the body (nearest body face 0.04 away,
        // in front of it, so not behind) and inside the robe (0.06 behind its
        // +x face): the robe owns the contact at depth 0.06, where a graded
        // body would read nothing at all — the property plain nearest-triangle
        // composite grading loses.
        let (body, body_faces) = cube(0.4);
        let (robe, robe_faces) = cube(0.5);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("robe", 40, &robe, &robe_faces)
            .expect("a valid layer");
        let (garment, faces) = sheet(0.44);
        let graded = accumulated
            .layered_visible_depths((&garment, &faces))
            .expect("gradable");
        assert_eq!(graded.len(), 3, "every sheet vertex is graded");
        for layered in &graded {
            assert_eq!(layered.member, 1, "the robe owns the contact");
            assert!((layered.depth.depth - 0.06).abs() < 1e-5);
        }
    }

    #[test]
    fn a_vertex_deep_inside_both_members_is_pushed_out_of_the_outer_one() {
        // A probe deep inside the body, off the face-center axes so the
        // nearest member triangles are unique: it is 0.3 behind the robe's +y
        // face and 0.2 behind the body's, and the layered depth must read the
        // robe's 0.3 — the push that also clears the body.
        let (body, body_faces) = cube(0.4);
        let (robe, robe_faces) = cube(0.5);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("robe", 40, &robe, &robe_faces)
            .expect("a valid layer");
        let garment = (
            vec![[0.1, 0.2, 0.1], [0.1, 0.2, 0.2], [0.2, 0.2, 0.1]],
            vec![[0u32, 1, 2]],
        );
        let graded = accumulated
            .layered_visible_depths((&garment.0, &garment.1))
            .expect("gradable");
        assert_eq!(graded.len(), 3);
        for layered in &graded {
            assert_eq!(layered.member, 1, "the robe owns a deep vertex");
            assert!(
                (layered.depth.depth - 0.3).abs() < 1e-5,
                "depth {}",
                layered.depth.depth
            );
        }
    }

    #[test]
    fn bare_skin_in_front_of_an_open_shell_grades_against_the_body() {
        // The cape floats behind the body, facing away: the sheet is in front
        // of it (not behind), so the body below keeps the contact. An open
        // shell captures only what is behind its own local surface.
        let (body, body_faces) = cube(0.4);
        let cape = (
            vec![
                [-1.5, -0.35, -0.35],
                [-1.5, 0.35, -0.35],
                [-1.5, 0.35, 0.35],
                [-1.5, -0.35, 0.35],
            ],
            vec![[0u32, 1, 2], [0, 2, 3]],
        );
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("cape", 10, &cape.0, &cape.1)
            .expect("a valid layer");
        let (garment, faces) = sheet(0.3);
        let graded = accumulated
            .layered_visible_depths((&garment, &faces))
            .expect("gradable");
        assert_eq!(graded.len(), 3);
        for layered in &graded {
            assert_eq!(layered.member, 0, "the body owns bare skin");
            assert!((layered.depth.depth - 0.1).abs() < 1e-5);
        }
    }

    #[test]
    fn the_breakdown_counts_per_member_and_keeps_zeros() {
        let (body, body_faces) = cube(0.4);
        let (robe, robe_faces) = cube(0.5);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("robe", 40, &robe, &robe_faces)
            .expect("a valid layer");
        let (garment, faces) = sheet(0.46);
        let breakdown = accumulated
            .contact_breakdown((&garment, &faces))
            .expect("gradable");
        assert_eq!(
            breakdown,
            vec![(BODY_SURFACE.to_string(), 0), ("robe".to_string(), 3),],
            "every member is listed, zeros kept"
        );
    }

    #[test]
    fn underivable_surfaces_are_refused_by_name() {
        let (body, body_faces) = cube(0.4);
        let empty = AccumulatedSurface::from_body(&[], &body_faces)
            .expect_err("no vertices")
            .to_string();
        assert!(empty.contains("`body` has no vertices"), "{empty}");

        let mut non_finite = body.clone();
        non_finite[0][0] = f32::NAN;
        let refused = AccumulatedSurface::from_body(&non_finite, &body_faces)
            .expect_err("not finite")
            .to_string();
        assert!(refused.contains("vertex 0 is not finite"), "{refused}");

        let out_of_range = vec![[0u32, 1, 99]];
        let refused = AccumulatedSurface::from_body(&body, &out_of_range)
            .expect_err("face index")
            .to_string();
        assert!(
            refused.contains("references missing vertex 99"),
            "{refused}"
        );

        // Three corners on one line: exactly zero area, no outward side.
        let collinear = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let degenerate = vec![[0u32, 1, 2]];
        let refused = AccumulatedSurface::from_body(&collinear, &degenerate)
            .expect_err("degenerate")
            .to_string();
        assert!(refused.contains("exactly-degenerate"), "{refused}");

        // Three proper triangles fanned on one edge: that edge carries three
        // faces, so winding cannot name a consistent outward field.
        let fan = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, -1.0, 0.0],
        ];
        let fanned = vec![[0u32, 1, 2], [0, 1, 3], [0, 1, 4]];
        let refused = AccumulatedSurface::from_body(&fan, &fanned)
            .expect_err("non-manifold")
            .to_string();
        assert!(refused.contains("non-manifold edge"), "{refused}");
    }

    #[test]
    fn a_duplicate_or_underdressed_layer_is_refused() {
        let (body, body_faces) = cube(0.4);
        let (robe, robe_faces) = cube(0.5);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("robe", 40, &robe, &robe_faces)
            .expect("a valid layer");
        let duplicate = accumulated
            .push_layer("robe", 50, &robe, &robe_faces)
            .expect_err("repeated id");
        assert!(
            duplicate.to_string().contains("already part"),
            "{duplicate}"
        );
        let under = accumulated
            .push_layer("cloak", 40, &robe, &robe_faces)
            .expect_err("not above the previous order");
        assert!(under.to_string().contains("not above"), "{under}");
    }

    #[test]
    fn an_open_shell_is_a_legal_obstacle() {
        // A single triangle is one boundary-only surface: open rims stay legal,
        // only non-derivability refuses.
        let open = sheet(0.5);
        let (body, body_faces) = cube(0.4);
        let mut accumulated = AccumulatedSurface::from_body(&body, &body_faces).expect("a body");
        accumulated
            .push_layer("cape", 10, &open.0, &open.1)
            .expect("an open shell is a legal obstacle");
        assert_eq!(accumulated.surface_label(), "accumulated[body, cape]");
    }
}
