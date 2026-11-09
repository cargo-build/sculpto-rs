//! The add-on's garment rule groups, as pure functions (F03).
//!
//! Five rule groups used to be numeric Python in
//! `tools/blender_addon/sculpto_garment`; each is a command now ([DECL0002]).
//! These functions take the data Blender owns — positions, topology, measured
//! bounds, skinning matrices — and return the decision, so the add-on keeps
//! only data construction.
//!
//! The Python implementation is the reference for every number here: it stays
//! the measured behavior, and the tests pin the worked examples it produced.
//!
//! [DECL0002]: ../../../doc/rules/0002-DECL-rules-inside-the-core.md

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use glam::Mat4;

/// Clearance the reconstructed surface keeps from the body, metres.
pub const GARMENT_OFFSET_M: f64 = 0.008;
/// Extra clearance at boundary rings, so openings do not cut into the body, metres.
pub const BOUNDARY_RELIEF_M: f64 = 0.004;
/// Vertices of a face that must sit inside the band for the face to stay.
pub const REGION_MAJORITY: usize = 2;
/// Mean `|x|` beyond which an arm-excluding class drops a face, metres.
pub const REGION_ARM_LIMIT_M: f64 = 0.21;
/// UV a ray-cast miss receives.
pub const PROJECT_MISS_UV: [f64; 2] = [0.5, 0.5];
/// World-space cast direction; the front relief faces `-y`.
pub const PROJECT_AXIS: [f64; 3] = [0.0, -1.0, 0.0];
/// The raw sits this far in front of the body's front surface, metres.
pub const PLACE_FRONT_GAP_M: f64 = 0.05;
/// The raw is lifted by this fraction of the body height, as well.
pub const PLACE_FRONT_RISE_FRAC: f64 = 0.02;

/// Why a garment rule could not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GarmentRuleError {
    /// The mesh carries no vertices or no triangles.
    EmptyMesh,
    /// A kept face index is not a triangle of the mesh.
    FaceOutOfRange { face: u32, faces: usize },
    /// The raw asset has no usable extent.
    DegenerateRaw,
    /// A class rule needs a fraction its row does not carry.
    MissingParameter(&'static str),
    /// No body vertex was found for a garment vertex.
    NoNearestVertex,
}

impl fmt::Display for GarmentRuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMesh => write!(formatter, "the mesh has no vertices or no triangles"),
            Self::FaceOutOfRange { face, faces } => {
                write!(formatter, "face {face} is not one of the {faces} triangles")
            }
            Self::DegenerateRaw => write!(formatter, "the raw asset has no usable extent"),
            Self::MissingParameter(name) => {
                write!(formatter, "the garment class needs `{name}`")
            }
            Self::NoNearestVertex => write!(formatter, "no body vertex lies near a garment vertex"),
        }
    }
}

impl std::error::Error for GarmentRuleError {}

/// What a garment class is scaled against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthBasis {
    /// The rest-pose body's width.
    BodyWidth,
    /// The body's height.
    Height,
}

/// The decisions one garment class makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GarmentRule {
    /// Body measure the class width is a multiple of.
    pub width_basis: WidthBasis,
    /// Width multiple: `width_factor * width_base` is the placed raw's width.
    pub width_factor: f64,
    /// Collar height as a fraction of the body height, for a non-sole class.
    pub collar_frac: Option<f64>,
    /// Anchor the hem on the floor instead of the collar on a fraction.
    pub hem_from_sole: bool,
    /// Sole height above the floor, metres, for a sole-anchored class.
    pub sole_offset_m: Option<f64>,
    /// A waist-anchored hem may not pass this height above the floor, metres.
    pub floor_clamp_m: Option<f64>,
    /// The class band overlaps hanging arms.
    pub arm_exclusion: bool,
}

/// One garment class the add-on exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GarmentKind {
    /// Tunics, shirts and bodices: collar at the neck base, arms kept.
    UpperBody,
    /// Waist-to-ankle legs: waist anchor, arms excluded, hem above the floor.
    Trousers,
    /// Boots and shoes: sole on the floor, arms excluded.
    Footwear,
}

impl GarmentKind {
    /// Every class, in panel order.
    pub fn all() -> [Self; 3] {
        [Self::UpperBody, Self::Trousers, Self::Footwear]
    }

    /// The class a panel id names, `None` when the add-on does not expose it.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|kind| kind.id() == id)
    }

    /// Stable panel id.
    pub fn id(self) -> &'static str {
        match self {
            Self::UpperBody => "upper-body",
            Self::Trousers => "trousers",
            Self::Footwear => "footwear",
        }
    }

    /// The class's numeric decisions.
    pub fn rule(self) -> GarmentRule {
        match self {
            Self::UpperBody => GarmentRule {
                width_basis: WidthBasis::BodyWidth,
                width_factor: 1.15,
                collar_frac: Some(0.87),
                hem_from_sole: false,
                sole_offset_m: None,
                floor_clamp_m: None,
                arm_exclusion: false,
            },
            Self::Trousers => GarmentRule {
                width_basis: WidthBasis::Height,
                // Hip width approximated as 0.42 x the body height.
                width_factor: 1.1 * 0.42,
                collar_frac: Some(0.62),
                hem_from_sole: false,
                sole_offset_m: None,
                floor_clamp_m: Some(0.02),
                arm_exclusion: true,
            },
            Self::Footwear => GarmentRule {
                width_basis: WidthBasis::Height,
                // One foot's sole length, generous.
                width_factor: 2.4 * 0.10,
                collar_frac: None,
                hem_from_sole: true,
                sole_offset_m: Some(0.01),
                floor_clamp_m: None,
                arm_exclusion: true,
            },
        }
    }
}

/// What `place` reads off the body, in the body's own frame, metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyExtent {
    /// Lowest body vertex.
    pub min_z: f64,
    /// Highest body vertex.
    pub max_z: f64,
    /// Front surface the raw is set in front of.
    pub front_y: f64,
    /// Narrowest `x` of the width reference.
    pub width_min_x: f64,
    /// Widest `x` of the width reference.
    pub width_max_x: f64,
}

/// What `place` reads off the raw asset, in world coordinates, metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawExtent {
    /// Lowest raw `x`.
    pub min_x: f64,
    /// Highest raw `x`.
    pub max_x: f64,
    /// Lowest raw `z`.
    pub min_z: f64,
    /// Highest raw `z`.
    pub max_z: f64,
}

/// Where a class puts the raw asset, and the coverage that results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Uniform object scale.
    pub scale: f64,
    /// Object location, metres.
    pub location: [f64; 3],
    /// Hem height, metres.
    pub hem_z: f64,
    /// Collar height, metres.
    pub collar_z: f64,
    /// Placed width, metres.
    pub width: f64,
}

/// Scale and move a raw asset in front of the body it will dress. @req DECL0002
pub fn place(
    kind: GarmentKind,
    body: BodyExtent,
    raw: RawExtent,
) -> Result<Placement, GarmentRuleError> {
    let rule = kind.rule();
    let height = body.max_z - body.min_z;
    let raw_width = raw.max_x - raw.min_x;
    let raw_height = raw.max_z - raw.min_z;
    if raw_width <= 1e-6 || raw_height <= 1e-6 {
        return Err(GarmentRuleError::DegenerateRaw);
    }
    let width_base = match rule.width_basis {
        WidthBasis::BodyWidth => body.width_max_x - body.width_min_x,
        WidthBasis::Height => height,
    };
    let scale = (rule.width_factor * width_base) / raw_width;
    let (hem_z, collar_z) = match rule.hem_from_sole {
        // Sole on the floor, shaft rises by the item's own proportions:
        // anchoring the collar at ankle height buries a knee-high shaft.
        true => {
            let hem = body.min_z + rule.sole_offset_m.unwrap_or_default();
            (hem, hem + raw_height * scale)
        }
        false => {
            let collar_frac = rule
                .collar_frac
                .ok_or(GarmentRuleError::MissingParameter("collar_frac"))?;
            let collar = body.min_z + collar_frac * height;
            let mut hem = collar - raw_height * scale;
            if let Some(clamp) = rule.floor_clamp_m {
                // A waist-anchored hem cannot go through the floor.
                hem = hem.max(body.min_z + clamp);
            }
            (hem, collar)
        }
    };
    let body_cx = (body.width_min_x + body.width_max_x) / 2.0;
    Ok(Placement {
        scale,
        location: [
            body_cx - (raw.min_x + raw_width / 2.0) * scale,
            body.front_y - PLACE_FRONT_GAP_M - height * PLACE_FRONT_RISE_FRAC,
            hem_z - raw.min_z * scale,
        ],
        hem_z,
        collar_z,
        width: raw_width * scale,
    })
}

/// Body faces whose vertices mostly sit between the hem and collar lines.
///
/// A strict all-vertices rule zigzags the hem and collar into torn-looking
/// edges, so a face stays when `REGION_MAJORITY` of its vertices are inside.
/// An arm-excluding class also drops faces whose mean `|x|` is beyond
/// `REGION_ARM_LIMIT_M`: the trousers and footwear bands overlap hanging arms.
/// A face is the body's own polygon, so a quad body keeps its four corners.
/// @req DECL0002
pub fn band(
    positions: &[[f32; 3]],
    polygons: &[Vec<u32>],
    hem_z: f64,
    collar_z: f64,
    arm_exclusion: bool,
) -> Vec<u32> {
    polygons
        .iter()
        .enumerate()
        .filter(|(_, polygon)| {
            let inside = polygon
                .iter()
                .filter(|index| {
                    let z = f64::from(positions[**index as usize][2]);
                    (hem_z..=collar_z).contains(&z)
                })
                .count();
            if inside < REGION_MAJORITY {
                return false;
            }
            !arm_exclusion || {
                let mean: f64 = polygon
                    .iter()
                    .map(|index| f64::from(positions[*index as usize][0]))
                    .sum::<f64>()
                    / polygon.len() as f64;
                mean.abs() <= REGION_ARM_LIMIT_M
            }
        })
        .map(|(index, _)| index as u32)
        .collect()
}

/// The garment surface built from a body, before its normals are applied.
///
/// The trim (drop the unselected faces, then the vertices left unlinked) and
/// the boundary test are topology, not Blender state, so the core answers them
/// and the add-on only displaces by the returned amounts.
#[derive(Debug, Clone, PartialEq)]
pub struct OffsetSurface {
    /// Kept body vertex indices, ascending.
    pub vertices: Vec<u32>,
    /// Kept positions, parallel to [`Self::vertices`].
    pub positions: Vec<[f32; 3]>,
    /// Kept polygons, remapped into [`Self::positions`].
    pub polygons: Vec<Vec<u32>>,
    /// Displacement along the vertex normal, metres.
    pub offsets: Vec<f64>,
    /// The vertex owns an edge of a single kept face.
    pub boundary: Vec<bool>,
}

/// The offset surface over the selected body faces. @req DECL0002
pub fn offset_surface(
    positions: &[[f32; 3]],
    polygons: &[Vec<u32>],
    keep_faces: &[u32],
) -> Result<OffsetSurface, GarmentRuleError> {
    if positions.is_empty() || polygons.is_empty() {
        return Err(GarmentRuleError::EmptyMesh);
    }
    let mut keep = vec![false; polygons.len()];
    for &face in keep_faces {
        let index = face as usize;
        if index >= polygons.len() {
            return Err(GarmentRuleError::FaceOutOfRange {
                face,
                faces: polygons.len(),
            });
        }
        keep[index] = true;
    }
    let mut referenced = vec![false; positions.len()];
    for (polygon, kept) in polygons.iter().zip(&keep) {
        if *kept {
            for index in polygon {
                referenced[*index as usize] = true;
            }
        }
    }
    let mut remap = vec![u32::MAX; positions.len()];
    let mut vertices = Vec::new();
    for (index, used) in referenced.iter().enumerate() {
        if *used {
            remap[index] = vertices.len() as u32;
            vertices.push(index as u32);
        }
    }
    let kept_polygons: Vec<Vec<u32>> = polygons
        .iter()
        .zip(&keep)
        .filter(|(_, kept)| **kept)
        .map(|(polygon, _)| polygon.iter().map(|index| remap[*index as usize]).collect())
        .collect();
    let kept_positions: Vec<[f32; 3]> = vertices
        .iter()
        .map(|&index| positions[index as usize])
        .collect();
    // An edge of one kept face is a boundary edge; a vertex on one is a boundary
    // vertex, which is what `BMVert::is_boundary` reports after the trim.
    let mut edges: HashMap<[u32; 2], usize> = HashMap::new();
    for polygon in &kept_polygons {
        for corner in 0..polygon.len() {
            let mut edge = [polygon[corner], polygon[(corner + 1) % polygon.len()]];
            edge.sort_unstable();
            *edges.entry(edge).or_default() += 1;
        }
    }
    let mut boundary = vec![false; kept_positions.len()];
    for (edge, count) in &edges {
        if *count == 1 {
            boundary[edge[0] as usize] = true;
            boundary[edge[1] as usize] = true;
        }
    }
    Ok(OffsetSurface {
        offsets: boundary
            .iter()
            .map(|&ring| match ring {
                true => GARMENT_OFFSET_M + BOUNDARY_RELIEF_M,
                false => GARMENT_OFFSET_M,
            })
            .collect(),
        boundary,
        positions: kept_positions,
        polygons: kept_polygons,
        vertices,
    })
}

/// The projected atlas one loop receives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopUv {
    /// Atlas coordinate.
    pub uv: [f32; 2],
    /// The ray met the raw asset.
    pub hit: bool,
}

/// One raw triangle: corner positions and the UV each corner carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawTriangle {
    /// Corner positions, world space.
    pub positions: [[f32; 3]; 3],
    /// Corner UVs, parallel to [`Self::positions`].
    pub uvs: [[f32; 2]; 3],
}

/// Garment UVs by ray-casting the placed raw asset. @req DECL0002
///
/// The hit's barycentric weights index the raw triangle's corner UVs; nearest-
/// corner snapping fragments a shared catalog atlas into mottle instead. A miss
/// receives [`PROJECT_MISS_UV`].
pub fn project(origins: &[[f32; 3]], direction: [f64; 3], raw: &[RawTriangle]) -> Vec<LoopUv> {
    let positions: Vec<[f32; 3]> = raw.iter().flat_map(|triangle| triangle.positions).collect();
    let triangles: Vec<[u32; 3]> = (0..raw.len())
        .map(|index| {
            let base = index as u32 * 3;
            [base, base + 1, base + 2]
        })
        .collect();
    let hits = crate::check::first_hits(
        origins,
        direction.map(|value| value as f32),
        &positions,
        &triangles,
    );
    hits.iter()
        .map(|hit| match hit {
            Some(hit) => LoopUv {
                uv: interpolate(hit.location, raw[hit.triangle]),
                hit: true,
            },
            None => LoopUv {
                uv: PROJECT_MISS_UV.map(|value| value as f32),
                hit: false,
            },
        })
        .collect()
}

/// Barycentric interpolation of the hit point over the raw triangle's corners.
fn interpolate(location: [f32; 3], triangle: RawTriangle) -> [f32; 2] {
    let corner = |index: usize| triangle.positions[index].map(f64::from);
    let [a, b, c] = [corner(0), corner(1), corner(2)];
    let point = location.map(f64::from);
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let ab_ab = dot(ab, ab);
    let ab_ac = dot(ab, ac);
    let ac_ac = dot(ac, ac);
    let denominator = ab_ab * ac_ac - ab_ac * ab_ac;
    if denominator.abs() < 1e-12 {
        return triangle.uvs[0];
    }
    let ap = [point[0] - a[0], point[1] - a[1], point[2] - a[2]];
    let d20 = dot(ap, ab);
    let d21 = dot(ap, ac);
    let mut v = (ac_ac * d20 - ab_ac * d21) / denominator;
    let mut w = (ab_ab * d21 - ab_ac * d20) / denominator;
    let mut u = 1.0 - v - w;
    v = v.clamp(0.0, 1.0);
    w = w.clamp(0.0, 1.0);
    u = u.clamp(0.0, (1.0 - w - v + 1e-6).max(0.0));
    let uv = |index: usize| triangle.uvs[index].map(f64::from);
    let [uv0, uv1, uv2] = [uv(0), uv(1), uv(2)];
    [
        (uv0[0] * u + uv1[0] * v + uv2[0] * w) as f32,
        (uv0[1] * u + uv1[1] * v + uv2[1] * w) as f32,
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// One vertex's bone influences, keyed by bone name.
pub type Influences = BTreeMap<String, f32>;

/// Weights the nearest body vertex carries, normalized to sum to one.
///
/// The body already carries the reviewed rigging, so each garment vertex takes
/// the body vertex's weights instead of a bone-space reconstruction. Ties resolve
/// to the lowest body index, so a rerun is identical. @req DECL0002
pub fn transfer(
    garment_positions: &[[f32; 3]],
    body_positions: &[[f32; 3]],
    body_weights: &[Influences],
) -> Result<Vec<Influences>, GarmentRuleError> {
    if garment_positions.is_empty() || body_positions.is_empty() {
        return Err(GarmentRuleError::EmptyMesh);
    }
    let grid = Nearest::build(body_positions);
    garment_positions
        .iter()
        .map(|&point| {
            let nearest = grid
                .nearest(point)
                .ok_or(GarmentRuleError::NoNearestVertex)?;
            Ok(normalize(&body_weights[nearest as usize]))
        })
        .collect()
}

/// Divide the influences by their sum; an empty set stays empty.
fn normalize(weights: &Influences) -> Influences {
    let total: f32 = weights.values().sum();
    let divisor = match total {
        0.0 => 1.0,
        other => other,
    };
    weights
        .iter()
        .map(|(bone, weight)| (bone.clone(), weight / divisor))
        .collect()
}

/// The rest geometry an unpose recovered, and what it could not.
#[derive(Debug, Clone, PartialEq)]
pub struct Unposed {
    /// Rest positions, parallel to the input.
    pub positions: Vec<[f32; 3]>,
    /// Vertices whose weighted skin matrix was inverted.
    pub moved: usize,
    /// Vertices with no usable influence or a singular matrix.
    pub skipped: usize,
}

/// Undo the posed deformation on the garment's own vertices.
///
/// The garment is built on the posed body, but the armature modifier deforms
/// from the rest pose: applying the frame's skin twice overshoots. For weights
/// `{(bone, w)}` and the frame's skinning matrices `M`, the built vertex is
/// `v1 = (sum w*M) @ v0`, so `v0 = (sum w*M)^-1 @ v1` restores the geometry the
/// modifier expects. `skinning` maps a bone name to its world-space matrix, row
/// major. @req DECL0002
pub fn unpose(
    positions: &[[f32; 3]],
    weights: &[Influences],
    skinning: &BTreeMap<String, [[f32; 4]; 4]>,
) -> Result<Unposed, GarmentRuleError> {
    if positions.is_empty() || positions.len() != weights.len() {
        return Err(GarmentRuleError::EmptyMesh);
    }
    let mut moved = 0;
    let mut skipped = 0;
    let mut result = Vec::with_capacity(positions.len());
    for (position, influences) in positions.iter().zip(weights) {
        let mut blend = Mat4::ZERO;
        let mut total = 0.0f32;
        for (bone, weight) in influences {
            let Some(matrix) = skinning.get(bone) else {
                continue;
            };
            if *weight <= 0.0 {
                continue;
            }
            blend += Mat4::from_cols_array(&row_major_to_columns(matrix)) * *weight;
            total += weight;
        }
        if total <= 0.0 {
            skipped += 1;
            result.push(*position);
            continue;
        }
        let Some(inverse) = blend.try_inverse().filter(|inverse| inverse.is_finite()) else {
            skipped += 1;
            result.push(*position);
            continue;
        };
        let rest = inverse.transform_point3(glam::Vec3::from_array(*position));
        result.push(rest.to_array());
        moved += 1;
    }
    Ok(Unposed {
        positions: result,
        moved,
        skipped,
    })
}

/// Column-major order `glam` reads from a row-major matrix.
fn row_major_to_columns(matrix: &[[f32; 4]; 4]) -> [f32; 16] {
    [
        matrix[0][0],
        matrix[1][0],
        matrix[2][0],
        matrix[3][0],
        matrix[0][1],
        matrix[1][1],
        matrix[2][1],
        matrix[3][1],
        matrix[0][2],
        matrix[1][2],
        matrix[2][2],
        matrix[3][2],
        matrix[0][3],
        matrix[1][3],
        matrix[2][3],
        matrix[3][3],
    ]
}

/// Uniform bucket grid over the body vertices for the nearest-vertex query.
struct Nearest {
    cell: f32,
    rings: i32,
    buckets: HashMap<[i32; 3], Vec<u32>>,
    positions: Vec<[f32; 3]>,
}

impl Nearest {
    fn build(positions: &[[f32; 3]]) -> Self {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for position in positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
        // One bucket per expected vertex, on the largest axis: a flat or
        // degenerate extent would collapse a volume-based spacing to zero.
        let largest = (0..3)
            .map(|axis| f64::from(max[axis] - min[axis]))
            .fold(0.0f64, f64::max);
        let across = (positions.len() as f64).cbrt().ceil().max(1.0);
        let cell = (largest / across).max(1e-4f64) as f32;
        let mut buckets: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (index, position) in positions.iter().enumerate() {
            buckets
                .entry(Self::key(*position, cell))
                .or_default()
                .push(index as u32);
        }
        Self {
            cell,
            rings: across as i32 + 1,
            buckets,
            positions: positions.to_vec(),
        }
    }

    fn key(position: [f32; 3], cell: f32) -> [i32; 3] {
        position.map(|value| (value / cell).floor() as i32)
    }

    /// Nearest vertex index; ties go to the lowest index inside the searched
    /// ring, and the ring stops once no outside vertex can be closer.
    fn nearest(&self, point: [f32; 3]) -> Option<u32> {
        let home = Self::key(point, self.cell);
        let mut best: Option<(u32, f32)> = None;
        for ring in 0..=self.rings {
            for dz in -ring..=ring {
                for dy in -ring..=ring {
                    for dx in -ring..=ring {
                        if ring > 0 && dx.abs() != ring && dy.abs() != ring && dz.abs() != ring {
                            continue;
                        }
                        let key = [home[0] + dx, home[1] + dy, home[2] + dz];
                        let Some(bucket) = self.buckets.get(&key) else {
                            continue;
                        };
                        for &index in bucket {
                            let distance = distance_squared(point, self.positions[index as usize]);
                            if best.is_none_or(|(_, current)| distance < current) {
                                best = Some((index, distance));
                            }
                        }
                    }
                }
            }
            if let Some((_, distance)) = best
                && distance < (ring as f32 * self.cell).powi(2)
            {
                break;
            }
        }
        best.map(|(index, _)| index)
    }
}

fn distance_squared(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|axis| (a[axis] - b[axis]).powi(2)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 x 5 plank of triangles in the XZ plane; `x` and `z` are the columns.
    fn plank() -> (Vec<[f32; 3]>, Vec<Vec<u32>>) {
        let columns = [-0.30f32, -0.10, 0.10, 0.30];
        let rows = [0.0f32, 0.4, 0.8, 1.2, 1.6];
        let positions = rows
            .iter()
            .flat_map(|z| columns.iter().map(move |x| [*x, 0.0, *z]))
            .collect::<Vec<_>>();
        let mut triangles = Vec::new();
        for row in 0..rows.len() - 1 {
            for column in 0..columns.len() - 1 {
                let a = (row * columns.len() + column) as u32;
                let b = a + 1;
                let c = a + columns.len() as u32;
                let d = c + 1;
                triangles.push(vec![a, b, c]);
                triangles.push(vec![b, d, c]);
            }
        }
        (positions, triangles)
    }

    #[test]
    fn the_band_keeps_a_majority_and_drops_arm_faces() {
        let (positions, polygons) = plank();
        let keep = band(&positions, &polygons, 0.5, 1.3, false);
        assert_eq!(keep, vec![7, 9, 11, 12, 13, 14, 15, 16, 17, 18, 20, 22]);
        let legs = band(&positions, &polygons, 0.5, 1.3, true);
        assert_eq!(legs, vec![7, 9, 13, 14, 15, 16, 20, 22]);
    }

    #[test]
    fn the_offset_surface_relieves_only_the_boundary_ring() {
        let (positions, polygons) = plank();
        // The 2 x 2 cell block keeps one interior vertex at its centre.
        let faces = [0, 1, 2, 3, 6, 7, 8, 9];
        let surface = offset_surface(&positions, &polygons, &faces).expect("trimmed");
        assert_eq!(surface.positions.len(), 9);
        assert_eq!(surface.polygons.len(), 8);
        assert_eq!(surface.boundary.iter().filter(|ring| **ring).count(), 8);
        let interior = surface
            .boundary
            .iter()
            .position(|ring| !ring)
            .expect("one interior vertex");
        assert_eq!(surface.positions[interior], [-0.1, 0.0, 0.4]);
        assert_eq!(surface.offsets[interior], GARMENT_OFFSET_M);
        assert_eq!(
            surface.offsets.iter().cloned().fold(0.0, f64::max),
            GARMENT_OFFSET_M + BOUNDARY_RELIEF_M
        );
    }

    #[test]
    fn a_face_outside_the_mesh_is_a_named_refusal() {
        let (positions, polygons) = plank();
        let error = offset_surface(&positions, &polygons, &[u32::MAX]).expect_err("refused");
        assert_eq!(
            error,
            GarmentRuleError::FaceOutOfRange {
                face: u32::MAX,
                faces: polygons.len()
            }
        );
        assert_eq!(
            offset_surface(&[], &[], &[]).expect_err("empty"),
            GarmentRuleError::EmptyMesh
        );
    }

    fn body() -> BodyExtent {
        BodyExtent {
            min_z: 0.0,
            max_z: 1.6,
            front_y: 0.0,
            width_min_x: -0.3,
            width_max_x: 0.3,
        }
    }

    fn raw() -> RawExtent {
        RawExtent {
            min_x: -0.5,
            max_x: 0.5,
            min_z: -0.5,
            max_z: 0.5,
        }
    }

    #[test]
    fn placement_matches_the_documented_class_fractions() {
        let upper = place(GarmentKind::UpperBody, body(), raw()).expect("placed");
        assert_eq!(upper.scale, 1.15 * 0.6);
        assert_eq!(upper.collar_z, 0.87 * 1.6);
        assert_eq!(upper.hem_z, upper.collar_z - upper.scale);
        assert_eq!(upper.location[1], -0.082);
        let trousers = place(GarmentKind::Trousers, body(), raw()).expect("placed");
        assert_eq!(trousers.scale, 1.1 * 0.42 * 1.6);
        assert_eq!(trousers.collar_z, 0.62 * 1.6);
        let footwear = place(GarmentKind::Footwear, body(), raw()).expect("placed");
        assert_eq!(footwear.hem_z, 0.01);
        assert_eq!(footwear.collar_z, 0.01 + footwear.scale);
    }

    #[test]
    fn a_waist_anchored_hem_stops_at_the_floor() {
        // A tall raw would put the waist-anchored hem below the floor.
        let tall = RawExtent {
            min_z: 0.0,
            max_z: 2.0,
            ..raw()
        };
        let placed = place(GarmentKind::Trousers, body(), tall).expect("placed");
        assert_eq!(placed.hem_z, 0.02);
    }

    #[test]
    fn a_degenerate_raw_is_refused() {
        let flat = RawExtent {
            min_x: 0.0,
            max_x: 0.0,
            ..raw()
        };
        assert_eq!(
            place(GarmentKind::UpperBody, body(), flat).expect_err("refused"),
            GarmentRuleError::DegenerateRaw
        );
    }

    fn triangle(positions: [[f32; 3]; 3], uvs: [[f32; 2]; 3]) -> RawTriangle {
        RawTriangle { positions, uvs }
    }

    #[test]
    fn projection_interpolates_the_hit_and_snaps_the_miss() {
        let raw = [triangle(
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        )];
        let origins = [[0.25, 1.0, 0.25], [0.5, 1.0, 0.25], [2.0, 1.0, 2.0]];
        let projected = project(&origins, PROJECT_AXIS, &raw);
        assert!(projected[0].hit);
        assert!((projected[0].uv[0] - 0.25).abs() < 1e-6);
        assert!((projected[0].uv[1] - 0.25).abs() < 1e-6);
        assert!((projected[1].uv[0] - 0.5).abs() < 1e-6);
        assert!((projected[1].uv[1] - 0.25).abs() < 1e-6);
        assert!(!projected[2].hit);
        assert_eq!(projected[2].uv, PROJECT_MISS_UV.map(|value| value as f32));
    }

    fn influences(pairs: &[(&str, f32)]) -> Influences {
        pairs
            .iter()
            .map(|(bone, weight)| ((*bone).to_string(), *weight))
            .collect()
    }

    #[test]
    fn transfer_takes_the_nearest_body_vertex_and_normalizes() {
        let body = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let weights = [
            influences(&[("Chest", 2.0)]),
            influences(&[("Chest", 0.25), ("Hips", 0.75)]),
        ];
        let garment = [[0.1, 0.0, 0.0], [0.9, 0.0, 0.0]];
        let transferred = transfer(&garment, &body, &weights).expect("transferred");
        assert_eq!(transferred[0]["Chest"], 1.0);
        assert_eq!(transferred[1]["Hips"], 0.75);
        assert_eq!(transferred[0].len(), 1);
    }

    #[test]
    fn unpose_inverts_the_weighted_skin_matrix() {
        let skinning = BTreeMap::from([
            (
                "Chest".to_string(),
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.5],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            ),
            (
                "Hips".to_string(),
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            ),
        ]);
        let posed = [[1.0, 2.0, 3.5]];
        let weights = [influences(&[("Chest", 1.0)])];
        let rest = unpose(&posed, &weights, &skinning).expect("unposed");
        assert_eq!(rest.moved, 1);
        assert_eq!(rest.positions[0][2], 3.0);
        let unknown = unpose(&posed, &[influences(&[("Tail", 1.0)])], &skinning).expect("unposed");
        assert_eq!(unknown.skipped, 1);
        assert_eq!(unknown.positions[0], posed[0]);
    }
}
