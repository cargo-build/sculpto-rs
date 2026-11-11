//! Resolve skin palettes and prove one rest basis for a document. @req DECL0008

use std::collections::BTreeSet;
use std::fmt;

use glam::{DMat4, DVec3};

use super::{JointId, Rig, RigError, RigFrame};
use crate::mesh::Skin;
use crate::skin::SkinWeightError;

/// Maximum reconstruction error in the rig's source units.
pub const REST_TOLERANCE: f64 = 1.0e-4;

/// Authored inputs to the rest proof for one shape.
#[derive(Debug, Clone, Copy)]
pub struct SkinRestSource<'a> {
    /// Position in the owning document.
    pub shape: usize,
    /// Authored skeleton root, resolved by the adapter.
    pub root: JointId,
    /// Skin-local source vertices, in the rig's declared frame.
    pub positions: &'a [[f32; 3]],
    /// Source palette, skin-to-bone matrices and authored influences.
    pub skin: &'a Skin,
    /// Global-to-skin matrix G in the same basis as the rig.
    pub global_to_skin: [[f32; 4]; 4],
}

/// Rest errors measured in source units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestMeasurement {
    /// Largest displacement of a joint's origin or unit axes, for diagnosis.
    pub max_joint: f64,
    /// Largest weighted displacement contributed by a single influence.
    pub max_influence: f64,
    /// Largest displacement of an actual weighted vertex.
    pub max_vertex: f64,
}

/// A borrowed skin with an exact palette-to-rig mapping.
#[derive(Debug)]
pub struct BoundRestSkin<'a> {
    source: SkinRestSource<'a>,
    palette: Vec<JointId>,
    measurement: RestMeasurement,
}

impl<'a> BoundRestSkin<'a> {
    /// The authored data used in the proof.
    pub fn source(&self) -> SkinRestSource<'a> {
        self.source
    }

    /// One joint identity per source palette entry.
    pub fn palette(&self) -> &[JointId] {
        &self.palette
    }

    /// Reconstruction errors under the selected document basis.
    pub fn measurement(&self) -> RestMeasurement {
        self.measurement
    }
}

/// Checked rest data for a single rig and its skinned shapes.
#[derive(Debug)]
pub struct RestBinding<'a> {
    rig: &'a Rig,
    basis: [[f64; 4]; 4],
    skins: Vec<BoundRestSkin<'a>>,
}

impl<'a> RestBinding<'a> {
    /// Resolve the inputs and prove their common rest basis.
    ///
    /// Identity is preferred when the authored worlds already agree with G.
    /// Otherwise the first shape and palette entry supply one candidate for
    /// the entire document. Every weighted contribution and vertex must pass.
    pub fn prove(
        rig: &'a Rig,
        frame: RigFrame,
        sources: &[SkinRestSource<'a>],
    ) -> Result<Self, RestBindError> {
        let resolved = resolve_sources(rig, frame, sources)?;
        let first = resolved.first().ok_or(RestBindError::NoSkins)?;
        let measure = |basis| {
            resolved
                .iter()
                .map(|shape| {
                    shape.measure(basis).map_err(|defect| RestBindError::Shape {
                        shape: shape.source.shape,
                        defect,
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let (basis, measurements) = match measure(DMat4::IDENTITY) {
            Ok(measurements) => (DMat4::IDENTITY, measurements),
            Err(_) => {
                let basis = first.skin_to_global * first.world_binds[0].inverse();
                (basis, measure(basis)?)
            }
        };
        Ok(Self {
            rig,
            basis: basis.to_cols_array_2d(),
            skins: resolved
                .into_iter()
                .zip(measurements)
                .map(|(shape, measurement)| BoundRestSkin {
                    source: shape.source,
                    palette: shape.palette,
                    measurement,
                })
                .collect(),
        })
    }

    /// Resolve the inputs and prove one explicitly selected basis. @req DECL0008
    ///
    /// Derived skins carry an exact intended basis: it is validated as finite,
    /// affine and invertible, then measured without any automatic candidate
    /// fallback, so a wrong basis is refused instead of silently replaced.
    /// The resolution and measurement owners are the automatic proof's.
    ///
    /// # Errors
    /// A frame mismatch, an unusable basis, no skinned shape, or any per-shape
    /// resolution or tolerance defect measured at `basis`.
    pub fn prove_with_basis<'b>(
        rig: &'a Rig,
        frame: RigFrame,
        sources: &[SkinRestSource<'b>],
        basis: [[f64; 4]; 4],
    ) -> Result<RestBinding<'b>, RestBindError>
    where
        'a: 'b,
    {
        if rig.frame() != frame {
            return Err(RestBindError::Frame {
                rig: rig.frame(),
                skin: frame,
            });
        }
        let basis = DMat4::from_cols_array_2d(&basis);
        if !basis.is_finite() {
            return Err(RestBindError::Basis(BasisDefect::NonFinite));
        }
        if basis.row(3).to_array() != [0.0, 0.0, 0.0, 1.0] {
            return Err(RestBindError::Basis(BasisDefect::NonAffine));
        }
        if !invertible(basis) {
            return Err(RestBindError::Basis(BasisDefect::Singular));
        }
        let resolved = resolve_sources(rig, frame, sources)?;
        if resolved.is_empty() {
            return Err(RestBindError::NoSkins);
        }
        let measurements = resolved
            .iter()
            .map(|shape| {
                shape.measure(basis).map_err(|defect| RestBindError::Shape {
                    shape: shape.source.shape,
                    defect,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RestBinding {
            rig,
            basis: basis.to_cols_array_2d(),
            skins: resolved
                .into_iter()
                .zip(measurements)
                .map(|(shape, measurement)| BoundRestSkin {
                    source: shape.source,
                    palette: shape.palette,
                    measurement,
                })
                .collect(),
        })
    }

    /// The authored hierarchy and rest transforms.
    pub fn rig(&self) -> &'a Rig {
        self.rig
    }

    /// Common column-major transform applied before joint worlds.
    pub fn basis(&self) -> [[f64; 4]; 4] {
        self.basis
    }

    /// Skinned shapes in input order.
    pub fn skins(&self) -> &[BoundRestSkin<'a>] {
        &self.skins
    }
}

/// Resolve every source against one rig, frame-checked.
///
/// The shared owner of the automatic and explicit proofs' source resolution.
fn resolve_sources<'a>(
    rig: &Rig,
    frame: RigFrame,
    sources: &[SkinRestSource<'a>],
) -> Result<Vec<ResolvedSkin<'a>>, RestBindError> {
    if rig.frame() != frame {
        return Err(RestBindError::Frame {
            rig: rig.frame(),
            skin: frame,
        });
    }
    sources
        .iter()
        .copied()
        .map(|source| {
            ResolvedSkin::new(rig, source).map_err(|defect| RestBindError::Shape {
                shape: source.shape,
                defect,
            })
        })
        .collect()
}

struct ResolvedSkin<'a> {
    source: SkinRestSource<'a>,
    palette: Vec<JointId>,
    world_binds: Vec<DMat4>,
    global_to_skin: DMat4,
    skin_to_global: DMat4,
}

impl<'a> ResolvedSkin<'a> {
    fn new(rig: &Rig, source: SkinRestSource<'a>) -> Result<Self, SkinRestDefect> {
        let skin = source.skin;
        if skin.bones.is_empty() || source.positions.is_empty() {
            return Err(SkinRestDefect::EmptySkin);
        }
        if skin.bind_inverse.len() != skin.bones.len() {
            return Err(SkinRestDefect::BindCount {
                bones: skin.bones.len(),
                binds: skin.bind_inverse.len(),
            });
        }
        if skin.influences.rows() != source.positions.len() {
            return Err(SkinRestDefect::VertexCount {
                positions: source.positions.len(),
                rows: skin.influences.rows(),
            });
        }
        skin.influences
            .validate(skin.bones.len())
            .map_err(SkinRestDefect::Weights)?;
        if let Some(vertex) = source
            .positions
            .iter()
            .position(|position| !position.iter().all(|value| value.is_finite()))
        {
            return Err(SkinRestDefect::Position { vertex });
        }
        rig.rest_world(source.root).map_err(SkinRestDefect::Rig)?;
        let global_to_skin =
            checked_affine(source.global_to_skin).ok_or(SkinRestDefect::GlobalToSkin)?;
        let skin_to_global = global_to_skin.inverse();
        let mut seen = BTreeSet::new();
        let mut palette = Vec::with_capacity(skin.bones.len());
        let mut world_binds = Vec::with_capacity(skin.bones.len());
        for (bone, (name, bind)) in skin.bones.iter().zip(&skin.bind_inverse).enumerate() {
            let joint = rig.joint_by_name(name).map_err(SkinRestDefect::Rig)?;
            if !seen.insert(joint) {
                return Err(SkinRestDefect::DuplicateJoint { joint });
            }
            if source.root != joint
                && !rig
                    .is_ancestor(source.root, joint)
                    .map_err(SkinRestDefect::Rig)?
            {
                return Err(SkinRestDefect::OutsideRoot {
                    root: source.root,
                    joint,
                });
            }
            let world = rig.rest_world(joint).map_err(SkinRestDefect::Rig)?;
            let bind = checked_affine(*bind).ok_or(SkinRestDefect::Bind { bone })?;
            let product = DMat4::from_cols_array_2d(&world) * bind;
            if !invertible(product) {
                return Err(SkinRestDefect::Bind { bone });
            }
            palette.push(joint);
            world_binds.push(product);
        }
        Ok(Self {
            source,
            palette,
            world_binds,
            global_to_skin,
            skin_to_global,
        })
    }

    fn measure(&self, basis: DMat4) -> Result<RestMeasurement, SkinRestDefect> {
        let transforms: Vec<_> = self.world_binds.iter().map(|world| basis * world).collect();
        let mut max_joint = 0.0_f64;
        for (bone, transform) in transforms.iter().enumerate() {
            let error = joint_error(*transform * self.global_to_skin);
            if !error.is_finite() {
                return Err(SkinRestDefect::Bind { bone });
            }
            max_joint = max_joint.max(error);
        }
        let mut max_influence = 0.0_f64;
        let mut max_vertex = 0.0_f64;
        for (vertex, position) in self.source.positions.iter().enumerate() {
            let x = DVec3::from_array(position.map(f64::from));
            let expected = self.skin_to_global.transform_point3(x);
            let row = self
                .source
                .skin
                .influences
                .normalized_row(vertex)
                .map_err(SkinRestDefect::Weights)?;
            let mut total = DVec3::ZERO;
            let mut outlier = None;
            for (bone, weight) in row.filter(|(_, weight)| *weight > 0.0) {
                let actual = transforms[usize::from(bone)].transform_point3(x);
                let contribution = (actual - expected) * weight;
                let error = contribution.length();
                if !error.is_finite() {
                    return Err(SkinRestDefect::InfluenceRest {
                        vertex,
                        bone,
                        error,
                    });
                }
                if error > REST_TOLERANCE {
                    outlier.get_or_insert((bone, error));
                }
                max_influence = max_influence.max(error);
                total += contribution;
            }
            let error = total.length();
            if !error.is_finite() || error > REST_TOLERANCE {
                return Err(SkinRestDefect::VertexRest { vertex, error });
            }
            if let Some((bone, error)) = outlier {
                return Err(SkinRestDefect::InfluenceRest {
                    vertex,
                    bone,
                    error,
                });
            }
            max_vertex = max_vertex.max(error);
        }
        Ok(RestMeasurement {
            max_joint,
            max_influence,
            max_vertex,
        })
    }
}

fn checked_affine(matrix: [[f32; 4]; 4]) -> Option<DMat4> {
    let matrix = DMat4::from_cols_array_2d(&matrix.map(|column| column.map(f64::from)));
    (matrix.row(3).to_array() == [0.0, 0.0, 0.0, 1.0] && invertible(matrix)).then_some(matrix)
}

fn invertible(matrix: DMat4) -> bool {
    let determinant = matrix.determinant();
    matrix.is_finite()
        && determinant.is_finite()
        && determinant != 0.0
        && matrix.inverse().is_finite()
}

fn joint_error(transform: DMat4) -> f64 {
    if !transform.is_finite() {
        return f64::INFINITY;
    }
    [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z]
        .into_iter()
        .map(|point| transform.transform_point3(point).distance(point))
        .fold(0.0, f64::max)
}

/// Why a document's authored skins could not be bound at rest.
#[derive(Debug, Clone, PartialEq)]
pub enum RestBindError {
    /// A rest proof requires at least one skinned shape.
    NoSkins,
    /// The caller declared a different frame for the skin.
    Frame { rig: RigFrame, skin: RigFrame },
    /// The explicitly selected proof basis is unusable.
    Basis(BasisDefect),
    /// One shape or palette entry is inconsistent.
    Shape {
        shape: usize,
        defect: SkinRestDefect,
    },
}

/// The failed requirement for an explicitly selected proof basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasisDefect {
    /// A basis cell is not finite.
    NonFinite,
    /// The basis moves the homogeneous lane, so it is not affine.
    NonAffine,
    /// The basis is singular, so its inverse does not exist.
    Singular,
}

impl fmt::Display for BasisDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite => write!(formatter, "proof basis is not finite"),
            Self::NonAffine => write!(formatter, "proof basis is not affine"),
            Self::Singular => write!(formatter, "proof basis is singular"),
        }
    }
}

impl std::error::Error for BasisDefect {}

/// The failed requirement for one shape.
#[derive(Debug, Clone, PartialEq)]
pub enum SkinRestDefect {
    /// A skinned shape must have vertices and a palette.
    EmptySkin,
    /// Every palette entry needs an authored skin-to-bone transform.
    BindCount { bones: usize, binds: usize },
    /// There must be one weight row per position.
    VertexCount { positions: usize, rows: usize },
    /// A source vertex is not finite.
    Position { vertex: usize },
    /// Source influence validation failed.
    Weights(SkinWeightError),
    /// The declared root or a named joint could not be resolved.
    Rig(RigError),
    /// More than one palette entry resolves to the same joint.
    DuplicateJoint { joint: JointId },
    /// A palette joint is outside the declared skeleton subtree.
    OutsideRoot { root: JointId, joint: JointId },
    /// G is non-finite, non-affine or singular.
    GlobalToSkin,
    /// A bind or its product with the rest world is invalid.
    Bind { bone: usize },
    /// One weighted contribution exceeds the reconstruction tolerance.
    InfluenceRest {
        vertex: usize,
        bone: u16,
        error: f64,
    },
    /// A weighted vertex exceeds the reconstruction tolerance.
    VertexRest { vertex: usize, error: f64 },
}

impl fmt::Display for RestBindError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSkins => write!(formatter, "no skinned shape to bind"),
            Self::Frame { rig, skin } => write!(
                formatter,
                "rig frame {rig:?} differs from skin frame {skin:?}"
            ),
            Self::Basis(defect) => write!(formatter, "{defect}"),
            Self::Shape { shape, defect } => write!(formatter, "shape {shape}: {defect}"),
        }
    }
}

impl std::error::Error for RestBindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Shape {
                defect: SkinRestDefect::Weights(error),
                ..
            } => Some(error),
            Self::Shape {
                defect: SkinRestDefect::Rig(error),
                ..
            } => Some(error),
            Self::Basis(defect) => Some(defect),
            _ => None,
        }
    }
}

impl fmt::Display for SkinRestDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySkin => write!(formatter, "skin has no vertices or palette"),
            Self::BindCount { bones, binds } => write!(
                formatter,
                "{bones} palette entries have {binds} bind matrices"
            ),
            Self::VertexCount { positions, rows } => {
                write!(formatter, "{positions} vertices have {rows} skin rows")
            }
            Self::Position { vertex } => write!(formatter, "vertex {vertex} is not finite"),
            Self::Weights(error) => write!(formatter, "{error}"),
            Self::Rig(error) => write!(formatter, "{error}"),
            Self::DuplicateJoint { joint } => write!(formatter, "palette maps joint {joint} twice"),
            Self::OutsideRoot { root, joint } => {
                write!(formatter, "joint {joint} is outside skeleton root {root}")
            }
            Self::GlobalToSkin => write!(formatter, "invalid global-to-skin matrix"),
            Self::Bind { bone } => write!(formatter, "invalid bind for palette entry {bone}"),
            Self::InfluenceRest {
                vertex,
                bone,
                error,
            } => write!(
                formatter,
                "vertex {vertex} palette entry {bone} contribution error {error:.6e} exceeds {REST_TOLERANCE:.1e}"
            ),
            Self::VertexRest { vertex, error } => write!(
                formatter,
                "vertex {vertex} rest error {error:.6e} exceeds {REST_TOLERANCE:.1e}"
            ),
        }
    }
}
