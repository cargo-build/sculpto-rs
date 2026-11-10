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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use glam::Mat4;

use crate::delta_conform::{
    DeltaConformError, DeltaOptions, ProximityCache, ProximityOptions, generate_delta,
};
use crate::mask::{MaskError, VertexMask};
use crate::weight_norm::{
    MAX_INFLUENCES, SUM_TOLERANCE, WeightDefect, WeightError, WeightNormalizer,
    first_weight_defect, pack_influences,
};

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

/// Proximity radius a transfer uses when the caller sets none (`Automorph.h:96`).
pub const DEFAULT_TRANSFER_RADIUS: f32 = 10.0;
/// Neighbours a transfer blends when the caller sets none (`Automorph.h:99`).
pub const DEFAULT_TRANSFER_MAX_RESULTS: usize = 10;

/// How a weight copy reads the body and classifies the garment's bones.
///
/// The defaults are the native copy: a ten-unit radius, ten blended neighbours,
/// no masks, a new unweighted garment, every source bone copied, no extra
/// normalizable bones, and neither remainder spreading nor the second
/// normalization pass (`BoneWeightCopyOptions`, `ProjectOperations.h:66-80`).
#[derive(Debug, Clone, PartialEq)]
pub struct TransferOptions {
    /// Inclusive proximity radius; zero uses the native insertion-tree mode.
    pub radius: f32,
    /// Body neighbours blended per garment vertex, applied after exclusion.
    pub max_results: usize,
    /// Per-body-vertex mask; a value above zero excludes the vertex.
    pub body_mask: Option<Vec<f32>>,
    /// Per-garment-vertex mask; a value above zero keeps the vertex's whole map.
    pub garment_mask: Option<Vec<f32>>,
    /// Current garment influences; absent means a new, unweighted garment.
    pub current: Option<Vec<Influences>>,
    /// Source bones to copy, in order; empty copies every source bone.
    pub copy: Vec<String>,
    /// Extra bones that absorb the normalization remainder.
    pub normalizable: Vec<String>,
    /// Native remainder spreading over the normalizable class.
    pub spread: bool,
    /// Native `BoneWeightCopyOptions::normalize` second all-normalizable pass.
    pub normalize: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            radius: DEFAULT_TRANSFER_RADIUS,
            max_results: DEFAULT_TRANSFER_MAX_RESULTS,
            body_mask: None,
            garment_mask: None,
            current: None,
            copy: Vec::new(),
            normalizable: Vec::new(),
            spread: false,
            normalize: false,
        }
    }
}

/// Why a weight copy was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum TransferError {
    /// The garment has no vertices to weight.
    EmptyGarment,
    /// The body has no vertices to copy weights from.
    EmptyBody,
    /// The neighbour cap is zero, so no weight could be blended.
    ZeroMaxResults,
    /// A list does not match the vertices it describes.
    LengthMismatch {
        /// Input field name.
        field: &'static str,
        /// Entries the field needs.
        expected: usize,
        /// Entries the field carries.
        actual: usize,
    },
    /// A mask is not a finite weight per vertex.
    Mask {
        /// Input field name.
        field: &'static str,
        /// Why the mask was refused.
        error: MaskError,
    },
    /// A garment mask above zero has no current weights to preserve.
    MissingCurrentWeights { vertex: usize },
    /// A preserved vertex cannot satisfy the unchanged sum/influence contract.
    InvalidPreservedWeights {
        vertex: usize,
        mass: f64,
        positive: usize,
    },
    /// A supplied influence map is not a valid weight set.
    Influence {
        /// Input field name.
        field: &'static str,
        /// Vertex inside that field.
        vertex: usize,
        /// Why the map was refused.
        error: WeightError,
    },
    /// The proximity cache or a per-bone blend refused the geometry.
    Proximity(DeltaConformError),
    /// The copy selection does not form valid weight classes.
    Normalize(WeightError),
    /// A copied vertex could not be normalized or packed.
    Vertex {
        /// Garment vertex.
        vertex: usize,
        /// Why the vertex was refused.
        error: WeightError,
    },
}

impl fmt::Display for TransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(formatter, "the garment has no vertices to weight"),
            Self::EmptyBody => {
                write!(formatter, "the body has no vertices to copy weights from")
            }
            Self::ZeroMaxResults => write!(formatter, "`max_results` must be at least one"),
            Self::LengthMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "`{field}` has {actual} entries for {expected} vertices"
            ),
            Self::Mask { field, error } => write!(formatter, "`{field}`: {error}"),
            Self::MissingCurrentWeights { vertex } => write!(
                formatter,
                "garment vertex {vertex} is masked but has no current weights"
            ),
            Self::InvalidPreservedWeights {
                vertex,
                mass,
                positive,
            } => write!(
                formatter,
                "masked garment vertex {vertex} has weight sum {mass} and {positive} positive influences"
            ),
            Self::Influence {
                field,
                vertex,
                error,
            } => write!(formatter, "`{field}` vertex {vertex}: {error}"),
            Self::Proximity(error) => write!(formatter, "the proximity copy failed: {error}"),
            Self::Normalize(error) => write!(formatter, "the weight classes are invalid: {error}"),
            Self::Vertex { vertex, error } => write!(formatter, "garment vertex {vertex}: {error}"),
        }
    }
}

impl std::error::Error for TransferError {}

/// Copy body skin weights onto a garment with the reference's interpolation.
///
/// Every selected body bone becomes one sparse weight field; each garment
/// vertex follows the inverse-distance blend of its cached body neighbours
/// (`Automorph::GenerateResultDiff` over the Y channel). The resulting per-vertex
/// maps are then normalized by [`WeightNormalizer`]
/// (`BoneWeightAutoNormalizer`) and packed to
/// [`MAX_INFLUENCES`](crate::weight_norm::MAX_INFLUENCES) positive influences
/// that sum to one within
/// [`SUM_TOLERANCE`](crate::weight_norm::SUM_TOLERANCE).
///
/// A body mask above zero removes that body vertex from the cache before the
/// neighbour cap (`OutfitProject.cpp:2868-2876`). A garment mask above zero
/// keeps that vertex's whole current map, so it is never zeroed, normalized or
/// packed (`OutfitProject.cpp:2891`, `2910`). Its existing sum and influence
/// count must already satisfy the output contract; otherwise the copy refuses.
/// A fresh one-neighbour copy preserves the accepted nearest-copy precision,
/// including weights below the native edit normalizer's pruning epsilon.
///
/// Bones the garment already carries outside `copy` and `normalizable` stay
/// locked and bit-identical. The native rigid-prop detection
/// (`keepRigidParts`) is not ported here.
///
/// # Errors
///
/// Fails on a body weight count or current-weight count that does not match its
/// vertices, an empty garment or body, a zero neighbour cap, a mask that is not
/// one finite weight in `0.0..=1.0` per vertex, a masked vertex without current
/// weights, a weight that is not a finite non-negative number, an invalid bone
/// selection, refused geometry, and a vertex the copy cannot give a positive
/// weight. A vertex is never returned with a zero sum.
///
/// @req DECL0002
pub fn transfer(
    garment_positions: &[[f32; 3]],
    body_positions: &[[f32; 3]],
    body_weights: &[Influences],
    options: &TransferOptions,
) -> Result<Vec<Influences>, TransferError> {
    if body_weights.len() != body_positions.len() {
        return Err(TransferError::LengthMismatch {
            field: "body_weights",
            expected: body_positions.len(),
            actual: body_weights.len(),
        });
    }
    if garment_positions.is_empty() {
        return Err(TransferError::EmptyGarment);
    }
    if body_positions.is_empty() {
        return Err(TransferError::EmptyBody);
    }
    if options.max_results == 0 {
        return Err(TransferError::ZeroMaxResults);
    }

    let body_mask = transfer_mask(
        options.body_mask.as_deref(),
        "body_mask",
        body_positions.len(),
    )?;
    let garment_mask = transfer_mask(
        options.garment_mask.as_deref(),
        "garment_mask",
        garment_positions.len(),
    )?;
    body_weights
        .iter()
        .enumerate()
        .try_for_each(|(vertex, influences)| {
            validate_influences("body_weights", vertex, influences)
        })?;
    let mut weights = current_rows(
        options.current.as_deref(),
        garment_positions.len(),
        &garment_mask,
    )?;
    validate_preserved_weights(&weights, &garment_mask)?;
    let source_weights = normalized_source_rows(body_weights);

    let copy = match options.copy.is_empty() {
        true => source_bones(body_weights),
        false => options.copy.clone(),
    };
    let locked = locked_bones(&weights, &copy, &options.normalizable);
    let bones: Vec<String> = copy.iter().chain(&options.normalizable).cloned().collect();
    let normalizer = WeightNormalizer::new(&bones, &locked, copy.len(), options.spread)
        .map_err(TransferError::Normalize)?;

    let excluded = body_mask
        .as_ref()
        .map(|mask| mask.masked().into_keys().collect::<BTreeSet<usize>>());
    let cache = ProximityCache::build(
        body_positions,
        garment_positions,
        &ProximityOptions {
            radius: options.radius,
        },
        excluded.as_ref(),
    )
    .map_err(TransferError::Proximity)?;

    // A fresh nearest copy has no edited garment weights to prioritize. Retain
    // its published small-weight behavior instead of pruning through edit epsilon.
    if options.max_results == 1
        && weights.iter().all(Influences::is_empty)
        && options.normalizable.is_empty()
        && !options.spread
        && !options.normalize
    {
        return (0..garment_positions.len())
            .map(|vertex| {
                let neighbor = cache
                    .neighbors(vertex)
                    .and_then(|neighbors| neighbors.first())
                    .ok_or(TransferError::Vertex {
                        vertex,
                        error: WeightError::MissingUnlockedMass { remainder: 1.0 },
                    })?;
                let selected: Influences = source_weights[neighbor.vertex]
                    .iter()
                    .filter(|(bone, _)| copy.contains(bone))
                    .map(|(bone, weight)| (bone.clone(), *weight))
                    .collect();
                pack_influences(&selected, &locked)
                    .map_err(|error| TransferError::Vertex { vertex, error })
            })
            .collect();
    }

    let touched = copy_bones(
        &cache,
        &source_weights,
        &copy,
        &garment_mask,
        &mut weights,
        options.max_results,
    )?;
    normalize_vertices(&normalizer, &mut weights, &touched)?;
    if options.normalize {
        // The reference's second pass treats every copied bone as normalizable,
        // which closes the sums the copy left short (`ProjectOperations.cpp:627-647`).
        let closer =
            WeightNormalizer::new(&bones, &locked, 0, false).map_err(TransferError::Normalize)?;
        normalize_vertices(&closer, &mut weights, &touched)?;
    }
    pack_vertices(&mut weights, &locked, &garment_mask)?;
    Ok(weights)
}

/// Preserve source proportions and explicit zero keys before native edit clamping. @req DECL0002
fn normalized_source_rows(rows: &[Influences]) -> Vec<Influences> {
    rows.iter()
        .map(|row| {
            let total: f32 = row.values().sum();
            match total {
                0.0 => row.clone(),
                value if value.is_finite() => row
                    .iter()
                    .map(|(bone, weight)| (bone.clone(), weight / value))
                    .collect(),
                _ => {
                    let wide_total: f64 = row.values().map(|weight| f64::from(*weight)).sum();
                    row.iter()
                        .map(|(bone, weight)| {
                            (bone.clone(), (f64::from(*weight) / wide_total) as f32)
                        })
                        .collect()
                }
            }
        })
        .collect()
}

/// Invalid locked-by-mask input must refuse rather than violate output invariants. @req DECL0002
fn validate_preserved_weights(
    weights: &[Influences],
    mask: &Option<VertexMask>,
) -> Result<(), TransferError> {
    weights.iter().enumerate().try_for_each(|(vertex, row)| {
        if !preserved(mask, vertex) {
            return Ok(());
        }
        let mass: f64 = row.values().map(|weight| f64::from(*weight)).sum();
        let positive = row.values().filter(|&&weight| weight > 0.0).count();
        match positive <= MAX_INFLUENCES && (mass - 1.0).abs() <= f64::from(SUM_TOLERANCE) {
            true => Ok(()),
            false => Err(TransferError::InvalidPreservedWeights {
                vertex,
                mass,
                positive,
            }),
        }
    })
}

/// Copy every selected bone field onto the garment, in selection order.
fn copy_bones(
    cache: &ProximityCache,
    body_weights: &[Influences],
    copy: &[String],
    garment_mask: &Option<VertexMask>,
    weights: &mut [Influences],
    max_results: usize,
) -> Result<BTreeSet<usize>, TransferError> {
    let mut touched = BTreeSet::new();
    for bone in copy {
        let delta = copy_bone_delta(cache, body_weights, bone, max_results)
            .map_err(TransferError::Proximity)?;
        // Native zeroes the garment's existing weights for this bone before it
        // writes the interpolated result (`OutfitProject.cpp:2887-2917`).
        weights
            .iter_mut()
            .enumerate()
            .for_each(|(vertex, influences)| {
                if preserved(garment_mask, vertex) || !influences.contains_key(bone) {
                    return;
                }
                influences.insert(bone.clone(), 0.0);
                touched.insert(vertex);
            });
        delta.iter().for_each(|(vertex, offset)| {
            if preserved(garment_mask, *vertex) {
                return;
            }
            weights[*vertex].insert(bone.clone(), offset[1]);
            touched.insert(*vertex);
        });
    }
    Ok(touched)
}

/// One bone's sparse weight field, blended onto the cached garment neighbours.
///
/// A body vertex that carries the bone contributes its weight on the Y channel,
/// including an explicit zero; a missing entry stays missing, so the nearest
/// neighbour without the bone cancels it (`Automorph.cpp:571-610`).
fn copy_bone_delta(
    cache: &ProximityCache,
    body_weights: &[Influences],
    bone: &str,
    max_results: usize,
) -> Result<BTreeMap<usize, [f32; 3]>, DeltaConformError> {
    let body_diffs: BTreeMap<usize, [f32; 3]> = body_weights
        .iter()
        .enumerate()
        .filter_map(|(vertex, influences)| {
            influences
                .get(bone)
                .map(|weight| (vertex, [0.0, *weight, 0.0]))
        })
        .collect();
    generate_delta(
        cache,
        &body_diffs,
        &DeltaOptions {
            mask: None,
            max_results,
            no_squeeze: false,
            solid_mode: false,
            axis_x: false,
            axis_y: true,
            axis_z: false,
            transform: None,
        },
    )
}

/// Every bone the body carries, sorted: the default copy selection.
fn source_bones(body_weights: &[Influences]) -> Vec<String> {
    body_weights
        .iter()
        .flat_map(|influences| influences.keys().cloned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Bones the garment already carries outside the copy and normalizable classes.
///
/// The class lists hold a handful of bones, so the linear membership test is
/// cheaper than a second index.
fn locked_bones(
    weights: &[Influences],
    copy: &[String],
    normalizable: &[String],
) -> BTreeSet<String> {
    weights
        .iter()
        .flat_map(|influences| influences.keys())
        .filter(|bone| !copy.contains(*bone) && !normalizable.contains(*bone))
        .cloned()
        .collect()
}

/// Start from the garment's current maps, or from an empty new garment.
fn current_rows(
    current: Option<&[Influences]>,
    vertices: usize,
    garment_mask: &Option<VertexMask>,
) -> Result<Vec<Influences>, TransferError> {
    match current {
        None => match (0..vertices).find(|vertex| preserved(garment_mask, *vertex)) {
            Some(vertex) => Err(TransferError::MissingCurrentWeights { vertex }),
            None => Ok(vec![Influences::new(); vertices]),
        },
        Some(rows) => match rows.len() != vertices {
            true => Err(TransferError::LengthMismatch {
                field: "current_weights",
                expected: vertices,
                actual: rows.len(),
            }),
            false => {
                rows.iter()
                    .enumerate()
                    .try_for_each(|(vertex, influences)| {
                        validate_influences("current_weights", vertex, influences)
                    })?;
                Ok(rows.to_vec())
            }
        },
    }
}

/// Validate one native mask: one finite value in `0.0..=1.0` per vertex.
fn transfer_mask(
    mask: Option<&[f32]>,
    field: &'static str,
    vertices: usize,
) -> Result<Option<VertexMask>, TransferError> {
    mask.map(|values| {
        let mask = VertexMask::from_values(values.to_vec())
            .map_err(|error| TransferError::Mask { field, error })?;
        match mask.len() != vertices {
            true => Err(TransferError::LengthMismatch {
                field,
                expected: vertices,
                actual: mask.len(),
            }),
            false => Ok(mask),
        }
    })
    .transpose()
}

/// A mask value above zero marks a preserved garment or an excluded body vertex.
fn preserved(mask: &Option<VertexMask>, vertex: usize) -> bool {
    mask.as_ref()
        .and_then(|mask| mask.get(vertex))
        .is_some_and(|value| value > 0.0)
}

/// A supplied weight map must be finite and non-negative.
fn validate_influences(
    field: &'static str,
    vertex: usize,
    influences: &Influences,
) -> Result<(), TransferError> {
    match first_weight_defect(influences) {
        Some((bone, WeightDefect::NonFinite { weight })) => Err(TransferError::Influence {
            field,
            vertex,
            error: WeightError::NonFiniteWeight {
                bone: bone.to_string(),
                weight,
            },
        }),
        Some((bone, WeightDefect::Negative { weight })) => Err(TransferError::Influence {
            field,
            vertex,
            error: WeightError::NegativeWeight {
                bone: bone.to_string(),
                weight,
            },
        }),
        None => Ok(()),
    }
}

/// Normalize every vertex the copy touched, ascending.
fn normalize_vertices(
    normalizer: &WeightNormalizer,
    weights: &mut [Influences],
    touched: &BTreeSet<usize>,
) -> Result<(), TransferError> {
    for vertex in touched {
        normalizer
            .adjust(&mut weights[*vertex])
            .map_err(|error| TransferError::Vertex {
                vertex: *vertex,
                error,
            })?;
    }
    Ok(())
}

/// Pack every unpreserved vertex into the NIF influence budget.
fn pack_vertices(
    weights: &mut [Influences],
    locked: &BTreeSet<String>,
    garment_mask: &Option<VertexMask>,
) -> Result<(), TransferError> {
    for (vertex, influences) in weights.iter_mut().enumerate() {
        if preserved(garment_mask, vertex) {
            continue;
        }
        let packed = pack_influences(influences, locked)
            .map_err(|error| TransferError::Vertex { vertex, error })?;
        *influences = packed;
    }
    Ok(())
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
    fn transfer_takes_the_nearest_body_vertex_at_one_neighbour() {
        let body = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let weights = [
            influences(&[("Chest", 2.0)]),
            influences(&[("Chest", 0.25), ("Hips", 0.75)]),
        ];
        let garment = [[0.1, 0.0, 0.0], [0.9, 0.0, 0.0]];
        let options = TransferOptions {
            max_results: 1,
            ..TransferOptions::default()
        };
        let transferred = transfer(&garment, &body, &weights, &options).expect("transferred");
        assert_eq!(transferred[0]["Chest"], 1.0);
        assert_eq!(transferred[1]["Hips"], 0.75);
        assert_eq!(transferred[0].len(), 1);
    }

    #[test]
    fn transfer_interpolates_by_default_and_keeps_a_masked_vertex() {
        let body = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]];
        let weights = [
            influences(&[("Chest", 1.0), ("Spine", 0.0)]),
            influences(&[("Chest", 0.5), ("Spine", 0.5)]),
        ];
        let garment = [[0.0, 0.0, 0.0]];
        let transferred =
            transfer(&garment, &body, &weights, &TransferOptions::default()).expect("transferred");
        // Equal distances: Chest (1 + 0.5) / 2 and Spine (0 + 0.5) / 2.
        assert_eq!(transferred[0]["Chest"], 0.75);
        assert_eq!(transferred[0]["Spine"], 0.25);

        let current = [influences(&[("Hips", 0.7), ("Chest", 0.3)])];
        let preserved = TransferOptions {
            garment_mask: Some(vec![1.0]),
            current: Some(current.to_vec()),
            ..TransferOptions::default()
        };
        assert_eq!(
            transfer(&garment, &body, &weights, &preserved).expect("preserved"),
            current
        );
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
