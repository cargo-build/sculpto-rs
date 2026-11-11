//! Outfit shape census and authored Blender Cloth selection. @req DECL0010

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The source asset and its authored part identities.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutfitManifest {
    /// Reusable outfit name.
    pub outfit: String,
    /// Source file, relative to this manifest unless absolute.
    pub nif: PathBuf,
    /// Exact names that override the body's inferred shape roles.
    #[serde(default)]
    pub body_shapes: Vec<String>,
    /// Exact garment names, with any authored cloth selection.
    #[serde(default)]
    pub garment_shapes: Vec<ShapeSlot>,
}

/// A named garment shape from the source.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeSlot {
    /// Exact source shape name.
    pub name: String,
    /// Material expectation for the assembled outfit.
    #[serde(default)]
    pub requires_material: bool,
    /// Regions selected for Blender Cloth.
    #[serde(default)]
    pub blender_cloth: Vec<ClothRegion>,
}

/// One named region selected for Blender Cloth.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClothRegion {
    /// Region identity within the shape.
    pub name: String,
    /// Zero-based source triangle indices. Omit for the entire shape.
    pub triangles: Option<Vec<usize>>,
}

/// The source facts the census needs from each imported shape.
#[derive(Debug, Clone, Copy)]
pub struct ShapeInfo<'a> {
    /// Source shape name.
    pub name: &'a str,
    /// Vertex count.
    pub vertices: usize,
    /// Triangle count.
    pub triangles: usize,
}

/// Whether the source shape is a body part or garment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeRole {
    /// Body geometry.
    Body,
    /// Garment geometry.
    Garment,
}

/// The two supported deformation routes for garment geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClothPath {
    /// Use the usual fitted and weighted rig.
    OrdinaryRigged,
    /// Select the region for Blender Cloth.
    BlenderCloth,
}

/// Why this part received its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    /// An authored region in the manifest.
    Manifest,
    /// Garment faces outside every Blender Cloth region.
    OrdinaryRig,
}

/// The path for one portion of a shape.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ClothPart {
    /// The authored region name, when one is supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Zero-based source triangle indices. Absent for the entire shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub triangle_indices: Option<Vec<usize>>,
    /// How this portion of the garment moves.
    pub path: ClothPath,
    /// Origin of the decision.
    pub source: SelectionSource,
}

/// The census and selection for one shape, in source file order.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ShapeReport {
    /// Zero-based shape ordinal in the source file.
    pub shape_index: usize,
    /// Source shape name.
    pub name: String,
    /// Vertex count.
    pub vertices: usize,
    /// Triangle count.
    pub triangles: usize,
    /// Body or garment.
    pub role: ShapeRole,
    /// Whether the manifest requires a material for this shape.
    pub requires_material: bool,
    /// Complete disjoint selection for garments. Empty for bodies.
    pub cloth: Vec<ClothPart>,
}

/// Why an outfit manifest could not bind to a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutfitError {
    /// The manifest omitted its outfit identity.
    BlankOutfit,
    /// A named manifest shape does not exist in the source.
    UnknownShape(String),
    /// More than one source shape has the referenced name.
    AmbiguousShape(String),
    /// A shape was declared more than once.
    DuplicateShape(String),
    /// The cloth regions cannot be resolved.
    InvalidCloth { shape: String, defect: ClothDefect },
}

/// A conflict or invalid face selection in one garment shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClothDefect {
    /// A Blender Cloth region has no name.
    BlankRegion,
    /// Two regions use one name.
    DuplicateRegion(String),
    /// A region contains no faces.
    EmptyRegion(String),
    /// Whole-shape selection overlaps a second region.
    WholeWithRegions,
    /// One face is outside the source triangle range.
    TriangleOutOfRange {
        region: String,
        triangle: usize,
        triangles: usize,
    },
    /// A face belongs to more than one declaration.
    RepeatedTriangle { triangle: usize },
}

impl fmt::Display for OutfitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankOutfit => write!(formatter, "manifest outfit name is blank"),
            Self::UnknownShape(name) => write!(formatter, "source has no shape `{name}`"),
            Self::AmbiguousShape(name) => {
                write!(formatter, "source has multiple shapes named `{name}`")
            }
            Self::DuplicateShape(name) => {
                write!(formatter, "manifest declares shape `{name}` more than once")
            }
            Self::InvalidCloth { shape, defect } => {
                write!(formatter, "shape `{shape}` cloth selection: {defect}")
            }
        }
    }
}

impl std::error::Error for OutfitError {}

impl fmt::Display for ClothDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BlankRegion => write!(formatter, "region name is blank"),
            Self::DuplicateRegion(name) => write!(formatter, "region `{name}` is declared twice"),
            Self::EmptyRegion(name) => write!(formatter, "region `{name}` has no source triangles"),
            Self::WholeWithRegions => {
                write!(formatter, "whole-shape selection overlaps another region")
            }
            Self::TriangleOutOfRange {
                region,
                triangle,
                triangles,
            } => write!(
                formatter,
                "region `{region}` names triangle {triangle} outside the {triangles} source triangles"
            ),
            Self::RepeatedTriangle { triangle } => {
                write!(formatter, "source triangle {triangle} is selected twice")
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ManifestRole<'a> {
    Body,
    Garment(&'a ShapeSlot),
}

/// Resolve shape roles and account for every garment face exactly once.
/// @req DECL0010
pub fn census(
    shapes: &[ShapeInfo<'_>],
    manifest: Option<&OutfitManifest>,
) -> Result<Vec<ShapeReport>, OutfitError> {
    let mut assigned = vec![None; shapes.len()];
    if let Some(manifest) = manifest {
        if manifest.outfit.trim().is_empty() {
            return Err(OutfitError::BlankOutfit);
        }
        for name in &manifest.body_shapes {
            bind_role(shapes, &mut assigned, name, ManifestRole::Body)?;
        }
        for slot in &manifest.garment_shapes {
            bind_role(
                shapes,
                &mut assigned,
                &slot.name,
                ManifestRole::Garment(slot),
            )?;
        }
    }
    shapes
        .iter()
        .zip(assigned)
        .enumerate()
        .map(|(index, (shape, assigned))| shape_report(index, shape, assigned))
        .collect()
}

fn bind_role<'a>(
    shapes: &[ShapeInfo<'_>],
    assigned: &mut [Option<ManifestRole<'a>>],
    name: &str,
    role: ManifestRole<'a>,
) -> Result<(), OutfitError> {
    let mut matches = shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| shape.name == name);
    let (index, _) = matches
        .next()
        .ok_or_else(|| OutfitError::UnknownShape(name.into()))?;
    if matches.next().is_some() {
        return Err(OutfitError::AmbiguousShape(name.into()));
    }
    if assigned[index].replace(role).is_some() {
        return Err(OutfitError::DuplicateShape(name.into()));
    }
    Ok(())
}

fn shape_report(
    shape_index: usize,
    shape: &ShapeInfo<'_>,
    assignment: Option<ManifestRole<'_>>,
) -> Result<ShapeReport, OutfitError> {
    let (role, slot) = match assignment {
        Some(ManifestRole::Body) => (ShapeRole::Body, None),
        Some(ManifestRole::Garment(slot)) => (ShapeRole::Garment, Some(slot)),
        None if is_body(shape.name) => (ShapeRole::Body, None),
        None => (ShapeRole::Garment, None),
    };
    let cloth = match role {
        ShapeRole::Body => Vec::new(),
        ShapeRole::Garment => {
            let regions = slot.map_or(&[][..], |slot| slot.blender_cloth.as_slice());
            select_cloth(shape.triangles, regions).map_err(|defect| OutfitError::InvalidCloth {
                shape: shape.name.into(),
                defect,
            })?
        }
    };
    Ok(ShapeReport {
        shape_index,
        name: shape.name.into(),
        vertices: shape.vertices,
        triangles: shape.triangles,
        role,
        requires_material: slot.is_some_and(|slot| slot.requires_material),
        cloth,
    })
}

fn is_body(name: &str) -> bool {
    [
        "BaseShape",
        "CBBE",
        "CBBE Body",
        "HIMBO",
        "HIMBO - Body",
        "3BA",
        "3BA_Vagina",
        "3BA_Anus",
    ]
    .iter()
    .any(|body| name.eq_ignore_ascii_case(body))
}

fn select_cloth(triangles: usize, regions: &[ClothRegion]) -> Result<Vec<ClothPart>, ClothDefect> {
    if regions.is_empty() {
        return Ok(vec![ordinary(None)]);
    }
    let mut names = BTreeSet::new();
    let mut covered = vec![false; triangles];
    let mut parts = Vec::with_capacity(regions.len() + 1);
    for region in regions {
        if region.name.trim().is_empty() {
            return Err(ClothDefect::BlankRegion);
        }
        if !names.insert(region.name.as_str()) {
            return Err(ClothDefect::DuplicateRegion(region.name.clone()));
        }
        if triangles == 0 || region.triangles.as_ref().is_some_and(Vec::is_empty) {
            return Err(ClothDefect::EmptyRegion(region.name.clone()));
        }
        let indices = match &region.triangles {
            Some(indices) => Some(claim_faces(&mut covered, region, indices)?),
            None if regions.len() == 1 => None,
            None => return Err(ClothDefect::WholeWithRegions),
        };
        let whole_shape = indices.is_none();
        parts.push(ClothPart {
            region: Some(region.name.clone()),
            triangle_indices: indices,
            path: ClothPath::BlenderCloth,
            source: SelectionSource::Manifest,
        });
        if whole_shape {
            return Ok(parts);
        }
    }
    let remainder: Vec<_> = covered
        .iter()
        .enumerate()
        .filter_map(|(triangle, &selected)| (!selected).then_some(triangle))
        .collect();
    if !remainder.is_empty() {
        parts.push(ordinary(Some(remainder)));
    }
    Ok(parts)
}

fn claim_faces(
    covered: &mut [bool],
    region: &ClothRegion,
    indices: &[usize],
) -> Result<Vec<usize>, ClothDefect> {
    let triangles = covered.len();
    for &triangle in indices {
        let slot = covered
            .get_mut(triangle)
            .ok_or_else(|| ClothDefect::TriangleOutOfRange {
                region: region.name.clone(),
                triangle,
                triangles,
            })?;
        if std::mem::replace(slot, true) {
            return Err(ClothDefect::RepeatedTriangle { triangle });
        }
    }
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    Ok(sorted)
}

fn ordinary(triangle_indices: Option<Vec<usize>>) -> ClothPart {
    ClothPart {
        region: None,
        triangle_indices,
        path: ClothPath::OrdinaryRigged,
        source: SelectionSource::OrdinaryRig,
    }
}
