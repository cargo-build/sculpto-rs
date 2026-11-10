//! Non-rigid garment fit onto a body surface (`fit_to_body`).
//!
//! Ported from BodySlide and Outfit Studio's `SurfaceFit`
//! (GPL-3.0, `src/components/SurfaceFit.cpp` at 58dfbd30): the garment is
//! welded into solver unknowns, each unknown is pulled to a gap outside the
//! body's closest surface point, and the displacement field comes from a
//! preconditioned conjugate-gradient solve of a Laplacian-regularized
//! least-squares system — data terms plus squared-Laplacian and membrane
//! smoothness, which is what keeps a sleeve or a boot's own form. Stages run
//! stiff to loose, strained edges give way, unpulled pieces follow the
//! closest pulled ones, rigid parts move as a whole, and final passes push
//! out clipping vertices and body pokes. The projection is topology-free,
//! so a garment authored on one body fits another of a different mesh.
//! @req DECL0013
//!
//! Divergences from the source, kept deliberate: empty or malformed input is
//! rejected ([`FitError`]) where the source silently returns unchanged
//! positions and skips bad triangles while welding; the body is re-oriented
//! per connected component so an inward-wound shell keeps a meaningful
//! clearance sign (the source trusts winding); fully locked vertices are
//! forced to exactly zero displacement after every repair, rigid and
//! clipping step, where the source only holds them with a heavy weight,
//! which is approximate. Body pokes and far-side culling use bounded rays
//! through the shared diagnostic BVH, excluding incident body triangles.
//! The inline flip repair (`flip_rings`, `flip_strength`)
//! mirrors the source's separate `fix_flipped_triangles` tool; the source
//! fit itself repairs no flips. Closest-point, normal-interpolation and
//! ray math is f32 here except where the shared seam provides f64.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use crate::check::SurfaceIndex;
use crate::mask::VertexMask;
use crate::point_query::PointIndex;
use crate::surface_util::{
    OpenEdges, add, barycentric_clamped, coincident_groups, cross, dot, edge_pairs, face_normal_of,
    first_non_finite, length, past_open_edge, scale, sub, unit,
};

/// A mesh as positions plus triangles indexing them.
pub type Mesh<'a> = (&'a [[f32; 3]], &'a [[u32; 3]]);

/// A vertex closer than this to its closest point counts as on the surface
/// whatever the direction (`SurfaceFit.cpp:27`).
const ON_SURFACE: f32 = 1e-3;
/// Least cosine between the line to the closest point and the surface normal
/// there; below it the pull would drag the vertex sideways onto an open edge
/// (`SurfaceFit.cpp:31`).
const MIN_LINE_AGREEMENT: f32 = 0.6;
/// The solver keeps pieces without any pull from drifting
/// (`System::Regularization`, `SurfaceFit.cpp:250`).
const REGULARIZATION: f32 = 1e-6;
/// Membrane (first-order) smoothness share against the squared Laplacian
/// (`System::MembraneShare`, `SurfaceFit.cpp:253`).
const MEMBRANE_SHARE: f32 = 0.1;
/// Weight of inside/poke pushes in the clipping passes (`Fit`, `push`).
const CLIPPING_PUSH: f32 = 20.0;
/// Lock hold weight scale inside one solve (`Fit`, `lockWeight`).
const LOCK_WEIGHT: f32 = 1000.0;
/// How far a welded body's vertex normals weld copies (`Fit:436`).
const BODY_WELD_DISTANCE: f32 = 1e-4;
/// Strain threshold of the `strained_edges` report (`Fit:790`).
const REPORT_STRAIN_THRESHOLD: f32 = 0.25;
/// Rigid parts this small stay at least half rigid in clipping passes
/// (`Fit:694`).
const SMALL_PART: usize = 64;
/// Upper bounds the source's MCP tool clamps these option counts to
/// (`McpToolsFitting.cpp:651-656`).
const MAX_STAGES: u32 = 10;
const MAX_ITERATIONS: u32 = 20;
const MAX_RELIEF_SMOOTHING: u32 = 100;
const MAX_CLIPPING_PASSES: u32 = 20;
const MAX_FLIP_RINGS: u32 = 20;
/// This port's bound; the source clamps nothing here.
const MAX_SOLVER_ITERATIONS: u32 = 100_000;

/// How the target gap is chosen, mirroring the source `gapMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GapMode {
    /// One constant gap everywhere; snaps the garment onto the body shape.
    #[default]
    Constant,
    /// Keep each vertex's current standoff (at least `min_gap`); only pulls
    /// clipping vertices out and follows the body under the garment.
    Keep,
    /// Use the gap each vertex has to the garment's authored body
    /// ([`fit_to_body_from`]); the fit re-applies the authored standoff to
    /// the new body.
    FromShape,
}

/// Parameters of one fit. Defaults are the native `SurfaceFit::Options`
/// values (`SurfaceFit.h:25-68`); the Outfit Studio tool clamps some of them
/// differently per call (`McpToolsFitting.cpp:645-663`, e.g. `minGap` 0.05
/// and `relief` 10 for `constant` mode), which stays the caller's choice.
///
/// Per-vertex arrays are validated against the garment vertex count: empty
/// means absent, like the source.
#[derive(Debug, Clone, PartialEq)]
pub struct FitOptions {
    /// How the target gap is chosen.
    pub gap_mode: GapMode,
    /// Distance outside the body each vertex is fitted to when it has no
    /// entry in `target_gaps`. Floored to `min_gap` after the relief.
    pub gap: f32,
    /// Per-vertex target gap (`targetGaps`); empty for `gap` everywhere.
    /// Copies of a welded vertex: the last copy's value wins, like the source.
    pub target_gaps: Vec<f32>,
    /// Taubin smoothing passes that find the garment's relief (`reliefSmoothing`);
    /// each vertex's stand-out goes onto its gap so relief is not pressed flat.
    pub relief_smoothing: u32,
    /// Least gap any vertex is fitted to, after the relief is added.
    pub min_gap: f32,
    /// Vertices farther than this from the body are not pulled; they follow
    /// their neighbors.
    pub max_distance: f32,
    /// Least cosine between a vertex normal and the body normal at its
    /// closest point; `-1` pulls all.
    pub min_normal_agreement: f32,
    /// Also pull the garment onto body points it does not reach yet
    /// (`symmetric`); for morphing a body onto another.
    pub symmetric: bool,
    /// Smoothness weight of the last stage; earlier stages are stiffer.
    /// Floored to `1e-4` like the source.
    pub stiffness: f32,
    /// Stiff-to-loose stages.
    pub stages: u32,
    /// Correspondence updates per stage.
    pub iterations: u32,
    /// Conjugate-gradient iterations per update.
    pub solver_iterations: u32,
    /// Copies of a vertex closer than this move as one (`weldDistance`);
    /// `0` welds nothing.
    pub weld_distance: f32,
    /// Per-vertex rigid part id, `-1` for none (`rigidParts`); each part
    /// moves as a whole after the fit, like rigidify does. Copies of a
    /// welded vertex: the last copy's id wins, like the source.
    pub rigid_parts: Vec<i32>,
    /// Rigid parts may also grow or shrink evenly (`rigidScale`).
    pub rigid_scale: bool,
    /// Final passes that push out what still clips: vertices inside the body
    /// and body pokes between vertices. `0` for none.
    pub clipping_passes: u32,
    /// How deep the body can poke through the garment to count; `0` disables
    /// poke handling.
    pub poke_depth: f32,
    /// Where the fit would stretch or squash edges by more than this ratio,
    /// the pull gives way and the garment keeps its form. `0` for no limit.
    pub max_strain: f32,
    /// Rings of neighbours around flipped triangles that may be damped by
    /// this port's inline flip repair; `0` disables it.
    pub flip_rings: u32,
    /// Blend toward the authored form per flip-repair pass.
    pub flip_strength: f32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            gap_mode: GapMode::default(),
            gap: 0.3,
            target_gaps: Vec::new(),
            relief_smoothing: 0,
            min_gap: 0.02,
            max_distance: 10.0,
            min_normal_agreement: 0.0,
            symmetric: false,
            stiffness: 1.0,
            stages: 4,
            iterations: 3,
            solver_iterations: 100,
            weld_distance: 1e-4,
            rigid_parts: Vec::new(),
            rigid_scale: false,
            clipping_passes: 4,
            poke_depth: 1.5,
            max_strain: 0.3,
            flip_rings: 4,
            flip_strength: 0.5,
        }
    }
}

/// What one fit changed and how well it fit, mirroring the native `Result`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitReport {
    /// Garment vertices given.
    pub vertices: usize,
    /// Solver unknowns after welding.
    pub unknowns: usize,
    /// Unknowns pulled to the body in the last correspondence update.
    pub pulled: usize,
    /// Unknowns of pieces with no pull at all, moved with the closest
    /// pulled vertices.
    pub followed: usize,
    /// Unknowns where the pull gave way to the strain cap, summed over the
    /// rounds (`relaxed`).
    pub kept_form: usize,
    /// Mean distance of the pulled unknowns from their target gap.
    pub mean_gap_error: f32,
    /// Largest distance of a pulled unknown from its target gap.
    pub max_gap_error: f32,
    /// Garment unknowns inside the body before the fit.
    pub inside_before: usize,
    /// Garment unknowns inside the body after the fit.
    pub inside_after: usize,
    /// Body vertices poking through the garment before the fit.
    pub poke_before: usize,
    /// Body vertices poking through the garment after the fit.
    pub poke_after: usize,
    /// Triangles whose normal turned by more than 90 degrees.
    pub flipped: usize,
    /// Mean displacement over all vertices.
    pub mean_move: f32,
    /// Largest single displacement.
    pub max_move: f32,
    /// Largest edge strain (longer over shorter, minus 1).
    pub max_strain: f32,
    /// Mean edge strain over the mesh's unique edges.
    pub mean_strain: f32,
    /// Edges strained by more than 0.25.
    pub strained_edges: usize,
}

/// Why a fit was refused. Validation happens before any query is built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FitError {
    /// The garment has no vertices.
    EmptyGarment,
    /// The body has no vertices.
    EmptyBody,
    /// The body has no triangles to fit onto.
    EmptyBodyFaces,
    /// A garment vertex position is not finite.
    NonFiniteGarmentPosition { vertex: usize },
    /// A body vertex position is not finite.
    NonFiniteBodyPosition { vertex: usize },
    /// Finite inputs exceeded the numeric range of the fit calculation.
    NonFiniteResult { vertex: usize },
    /// A gap-source vertex position is not finite.
    NonFiniteGapSourcePosition { vertex: usize },
    /// A gap-from vertex position is not finite.
    NonFiniteGapFromPosition { vertex: usize },
    /// A garment triangle references a missing vertex.
    GarmentFaceIndex { triangle: usize, vertex: u32 },
    /// A body triangle references a missing vertex.
    BodyFaceIndex { triangle: usize, vertex: u32 },
    /// A gap-from triangle references a missing vertex.
    GapFromFaceIndex { triangle: usize, vertex: u32 },
    /// The gap source is not a copy of the garment.
    GapSourceLength { expected: usize, actual: usize },
    /// `gap_mode from_shape` needs the authored body of `fit_to_body_from`.
    FromShapeNeedsSource,
    /// `fit_to_body_from` measures authored standoff; another gap mode would
    /// ignore `gap_from`.
    SourceNeedsFromShape,
    /// Explicit target gaps would be fully overwritten by the measured
    /// from-shape gaps.
    TargetGapsWithFromShape,
    /// The target-gap list is not empty and not one per garment vertex.
    TargetGapLength { expected: usize, actual: usize },
    /// A target gap is not finite.
    TargetGapValue { vertex: usize, value: f32 },
    /// The lock mask is not one value per garment vertex.
    LockLength { expected: usize, actual: usize },
    /// The rigid-part list is not empty and not one per garment vertex.
    RigidPartLength { expected: usize, actual: usize },
    /// A rigid part id is below `-1`.
    RigidPartId { vertex: usize, part: i32 },
    /// `gap` is not finite.
    Gap(f32),
    /// `min_gap` is negative or not finite.
    MinGap(f32),
    /// `max_distance` is below the native floor of 1e-3 or not finite.
    MaxDistance(f32),
    /// `min_normal_agreement` is outside `-1..=1` or not finite.
    MinNormalAgreement(f32),
    /// `stiffness` is below the native floor of 1e-4 or not finite.
    Stiffness(f32),
    /// The staged stiffness or derived lock weight would overflow.
    StiffnessSchedule { stiffness: f32, stages: u32 },
    /// `stages` is zero or above the tool's bound of 10.
    Stages(u32),
    /// `iterations` is zero or above the tool's bound of 20.
    Iterations(u32),
    /// `solver_iterations` is zero or above this port's bound.
    SolverIterations(u32),
    /// `weld_distance` is negative or not finite; `0` welds nothing.
    WeldDistance(f32),
    /// `relief_smoothing` is above the tool's bound of 100.
    ReliefSmoothing(u32),
    /// `clipping_passes` is above the tool's bound of 20.
    ClippingPasses(u32),
    /// `poke_depth` is negative or not finite.
    PokeDepth(f32),
    /// `max_strain` is negative or not finite.
    MaxStrain(f32),
    /// `flip_rings` is above this port's bound of 20.
    FlipRings(u32),
    /// `flip_strength` is outside `0..=1` or not finite.
    FlipStrength(f32),
}

impl fmt::Display for FitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(formatter, "the garment has no vertices"),
            Self::EmptyBody => write!(formatter, "the body has no vertices"),
            Self::EmptyBodyFaces => write!(formatter, "the body has no triangles"),
            Self::NonFiniteGarmentPosition { vertex } => {
                write!(
                    formatter,
                    "garment vertex {vertex} has a non-finite position"
                )
            }
            Self::NonFiniteBodyPosition { vertex } => {
                write!(formatter, "body vertex {vertex} has a non-finite position")
            }
            Self::NonFiniteGapSourcePosition { vertex } => {
                write!(
                    formatter,
                    "gap source vertex {vertex} has a non-finite position"
                )
            }
            Self::NonFiniteGapFromPosition { vertex } => {
                write!(
                    formatter,
                    "gap-from vertex {vertex} has a non-finite position"
                )
            }
            Self::GarmentFaceIndex { triangle, vertex } => write!(
                formatter,
                "garment triangle {triangle} references missing vertex {vertex}"
            ),
            Self::BodyFaceIndex { triangle, vertex } => write!(
                formatter,
                "body triangle {triangle} references missing vertex {vertex}"
            ),
            Self::GapFromFaceIndex { triangle, vertex } => write!(
                formatter,
                "gap-from triangle {triangle} references missing vertex {vertex}"
            ),
            Self::GapSourceLength { expected, actual } => write!(
                formatter,
                "the gap source has {actual} vertices for {expected} garment vertices; it has to be a copy of the same mesh"
            ),
            Self::FromShapeNeedsSource => write!(
                formatter,
                "gap mode from_shape measures the gap to the authored body; use fit_to_body_from"
            ),
            Self::SourceNeedsFromShape => write!(
                formatter,
                "fit_to_body_from measures authored standoff; it needs gap mode from_shape"
            ),
            Self::TargetGapsWithFromShape => write!(
                formatter,
                "explicit target gaps would be overwritten by the measured from-shape gaps"
            ),
            Self::TargetGapLength { expected, actual } => write!(
                formatter,
                "target gaps: {actual} values for {expected} garment vertices"
            ),
            Self::TargetGapValue { vertex, value } => write!(
                formatter,
                "target gap {value} of garment vertex {vertex} is not finite"
            ),
            Self::LockLength { expected, actual } => write!(
                formatter,
                "the lock mask has {actual} values for {expected} garment vertices"
            ),
            Self::RigidPartLength { expected, actual } => write!(
                formatter,
                "rigid parts: {actual} values for {expected} garment vertices"
            ),
            Self::RigidPartId { vertex, part } => write!(
                formatter,
                "garment vertex {vertex} has rigid part {part}, below the -1 none-id"
            ),
            Self::Gap(value) => write!(formatter, "the gap {value} is not finite"),
            Self::MinGap(value) => {
                write!(formatter, "the least gap {value} is negative or not finite")
            }
            Self::MaxDistance(value) => write!(
                formatter,
                "the maximum distance {value} must be finite and at least 1e-3"
            ),
            Self::MinNormalAgreement(value) => write!(
                formatter,
                "the least normal agreement {value} is outside -1..=1 or not finite"
            ),
            Self::Stiffness(value) => write!(
                formatter,
                "the stiffness {value} must be finite and at least 1e-4"
            ),
            Self::StiffnessSchedule { stiffness, stages } => write!(
                formatter,
                "stiffness {stiffness} across {stages} stages overflows solver weights"
            ),
            Self::NonFiniteResult { vertex } => write!(
                formatter,
                "the fit exceeds the numeric range at garment vertex {vertex}"
            ),
            Self::Stages(value) => write!(
                formatter,
                "the stage count {value} is outside 1..={MAX_STAGES}"
            ),
            Self::Iterations(value) => write!(
                formatter,
                "the iteration count {value} is outside 1..={MAX_ITERATIONS}"
            ),
            Self::SolverIterations(value) => write!(
                formatter,
                "the solver iteration count {value} is outside 1..={MAX_SOLVER_ITERATIONS}"
            ),
            Self::WeldDistance(value) => write!(
                formatter,
                "the weld distance {value} is negative or not finite; 0 welds nothing"
            ),
            Self::ReliefSmoothing(value) => write!(
                formatter,
                "the relief smoothing {value} is above {MAX_RELIEF_SMOOTHING}"
            ),
            Self::ClippingPasses(value) => write!(
                formatter,
                "the clipping pass count {value} is above {MAX_CLIPPING_PASSES}"
            ),
            Self::PokeDepth(value) => write!(
                formatter,
                "the poke depth {value} is negative or not finite"
            ),
            Self::MaxStrain(value) => {
                write!(
                    formatter,
                    "the strain cap {value} is negative or not finite"
                )
            }
            Self::FlipRings(value) => {
                write!(
                    formatter,
                    "the flip ring count {value} is above {MAX_FLIP_RINGS}"
                )
            }
            Self::FlipStrength(value) => write!(
                formatter,
                "the flip strength {value} is outside 0..=1 or not finite"
            ),
        }
    }
}

impl std::error::Error for FitError {}

impl FitOptions {
    /// Reject values the source would silently clamp or skip, so no option is
    /// quietly ignored. @req DECL0013
    fn validate(&self) -> Result<(), FitError> {
        if !self.gap.is_finite() {
            return Err(FitError::Gap(self.gap));
        }
        if !self.min_gap.is_finite() || self.min_gap < 0.0 {
            return Err(FitError::MinGap(self.min_gap));
        }
        if !self.max_distance.is_finite() || self.max_distance < 1e-3 {
            return Err(FitError::MaxDistance(self.max_distance));
        }
        if !self.min_normal_agreement.is_finite()
            || !(-1.0..=1.0).contains(&self.min_normal_agreement)
        {
            return Err(FitError::MinNormalAgreement(self.min_normal_agreement));
        }
        if !self.stiffness.is_finite() || self.stiffness < 1e-4 {
            return Err(FitError::Stiffness(self.stiffness));
        }
        if self.stages == 0 || self.stages > MAX_STAGES {
            return Err(FitError::Stages(self.stages));
        }
        let peak_lambda = self.stiffness * 4.0f32.powi((self.stages - 1) as i32);
        if !(LOCK_WEIGHT * (1.0 + peak_lambda)).is_finite() {
            return Err(FitError::StiffnessSchedule {
                stiffness: self.stiffness,
                stages: self.stages,
            });
        }
        if self.iterations == 0 || self.iterations > MAX_ITERATIONS {
            return Err(FitError::Iterations(self.iterations));
        }
        if self.solver_iterations == 0 || self.solver_iterations > MAX_SOLVER_ITERATIONS {
            return Err(FitError::SolverIterations(self.solver_iterations));
        }
        if !self.weld_distance.is_finite() || self.weld_distance < 0.0 {
            return Err(FitError::WeldDistance(self.weld_distance));
        }
        if self.relief_smoothing > MAX_RELIEF_SMOOTHING {
            return Err(FitError::ReliefSmoothing(self.relief_smoothing));
        }
        if self.clipping_passes > MAX_CLIPPING_PASSES {
            return Err(FitError::ClippingPasses(self.clipping_passes));
        }
        if !self.poke_depth.is_finite() || self.poke_depth < 0.0 {
            return Err(FitError::PokeDepth(self.poke_depth));
        }
        if !self.max_strain.is_finite() || self.max_strain < 0.0 {
            return Err(FitError::MaxStrain(self.max_strain));
        }
        if self.flip_rings > MAX_FLIP_RINGS {
            return Err(FitError::FlipRings(self.flip_rings));
        }
        if !self.flip_strength.is_finite() || !(0.0..=1.0).contains(&self.flip_strength) {
            return Err(FitError::FlipStrength(self.flip_strength));
        }
        Ok(())
    }
}

/// Fit `garment` onto `body` with the `constant` or `keep` gap modes and
/// return the moved positions plus a report.
///
/// Both meshes must be in the same frame. `lock` uses the shared
/// [`VertexMask`] convention (`1` stays, `0` free); fully locked vertices
/// keep their positions bit-identically. @req DECL0013
///
/// # Errors
///
/// Refuses [`FitError::FromShapeNeedsSource`] for `from_shape`, and any
/// malformed geometry, length or option listed there, before any query.
pub fn fit_to_body(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitReport), FitError> {
    if options.gap_mode == GapMode::FromShape {
        return Err(FitError::FromShapeNeedsSource);
    }
    fit_with_targets(garment, body, None, lock, options)
}

/// Fit with `gap_mode from_shape`: the target gaps follow the standoff the
/// garment has to its authored body `gap_from`, optionally measured on a copy
/// of the garment in its source frame (`gap_source`, same vertex order), as
/// the Outfit Studio tool does. @req DECL0013
///
/// # Errors
///
/// Refuses any mode but `from_shape`, `target_gaps` (they would be
/// overwritten), and any malformed geometry, length or option listed in
/// [`FitError`], before any query.
pub fn fit_to_body_from(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    gap_from: Mesh<'_>,
    gap_source: Option<&[[f32; 3]]>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitReport), FitError> {
    if options.gap_mode != GapMode::FromShape {
        return Err(FitError::SourceNeedsFromShape);
    }
    if !options.target_gaps.is_empty() {
        return Err(FitError::TargetGapsWithFromShape);
    }
    options.validate()?;
    validate_geometry(garment, body)?;
    let measured = gap_source.unwrap_or(garment.0);
    if measured.len() != garment.0.len() {
        return Err(FitError::GapSourceLength {
            expected: garment.0.len(),
            actual: measured.len(),
        });
    }
    if let Some(vertex) = first_non_finite(measured) {
        return Err(FitError::NonFiniteGapSourcePosition { vertex });
    }
    validate_faces(gap_from.0, gap_from.1, |triangle, vertex| {
        FitError::GapFromFaceIndex { triangle, vertex }
    })?;
    if let Some(vertex) = first_non_finite(gap_from.0) {
        return Err(FitError::NonFiniteGapFromPosition { vertex });
    }
    // SignedDistancesToBody: measured where the fit pulls, `FLT_MAX`
    // (here `None`) beyond the search radius falls back to `options.gap`.
    let authored = BodySurface::build(gap_from.0, gap_from.1);
    let targets: Vec<f32> = signed_standoffs(measured, &authored, options.max_distance)
        .into_iter()
        .map(|distance| distance.unwrap_or(options.gap).max(options.min_gap))
        .collect();
    fit_with_targets(garment, body, Some(&targets), lock, options)
}

/// The shared fit core after any from-shape measurement.
fn fit_with_targets(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    measured_gaps: Option<&[f32]>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitReport), FitError> {
    options.validate()?;
    validate_geometry(garment, body)?;
    validate_lengths(garment.0.len(), lock, options, measured_gaps.is_some())?;

    let mesh = weld(garment.0, garment.1, options.weld_distance);
    let count = mesh.rest.len();
    let max_distance = options.max_distance;

    // Per unknown: target gap, lock and rigid part from its vertex copies
    // (`Fit:392-405`). Locks take the strongest copy; the per-vertex lists
    // take the last copy, like the source.
    let mut gap_of = vec![options.gap; count];
    let mut lock_of = vec![0.0f32; count];
    let mut part_of = vec![-1i32; count];
    let per_vertex_gaps = !options.target_gaps.is_empty();
    for (vertex, unknown) in mesh.unknown_of.iter().enumerate() {
        if let Some(gaps) = measured_gaps {
            gap_of[*unknown] = gaps[vertex];
        } else if per_vertex_gaps {
            gap_of[*unknown] = options.target_gaps[vertex];
        }
        if let Some(mask) = lock {
            lock_of[*unknown] = lock_of[*unknown].max(mask.as_slice()[vertex]);
        }
        if !options.rigid_parts.is_empty() {
            part_of[*unknown] = options.rigid_parts[vertex];
        }
    }

    if options.relief_smoothing > 0 {
        add_relief(&mesh, &mut gap_of, options);
    }
    for gap in &mut gap_of {
        *gap = gap.max(options.min_gap);
    }

    let surface = BodySurface::build(body.0, body.1);

    // The gaps as they are, measured the way the fit pulls (`Fit:440-448`).
    if options.gap_mode == GapMode::Keep {
        for (index, &rest) in mesh.rest.iter().enumerate() {
            if let Some(hit) = surface.hit(rest, max_distance)
                && clean_pull(rest, hit.point, hit.normal)
            {
                gap_of[index] = dot(sub(rest, hit.point), hit.normal).max(options.min_gap);
            }
        }
    }

    // Body points sampled for the symmetric pulls (`Fit:452-456`).
    let pulling_surface: Vec<usize> = if options.symmetric {
        let step = (body.0.len() / 40_000).max(1);
        (0..body.0.len()).step_by(step).collect()
    } else {
        Vec::new()
    };

    let mut solver = FitState {
        mesh,
        options,
        surface,
        gap_of,
        lock_of,
        part_of,
        max_distance,
        pulling_surface,
        displacement: vec![[0.0; 3]; count],
        pull_scale: vec![1.0; count],
        last_weight: vec![0.0; count],
        followed: 0,
        kept_form: 0,
    };

    let (inside_before, poke_before) = solver.clipping_counts();

    // Stiff first, so the garment moves as a whole before the details fit.
    let final_lambda = options.stiffness;
    for stage in 0..options.stages {
        let stages_left = options.stages - 1 - stage;
        let lambda = final_lambda * 4.0f32.powi(stages_left as i32);
        for _ in 0..options.iterations {
            solver.iterate(lambda);
        }
    }

    if options.max_strain > 0.0 {
        solver.relax_strain();
    }
    repair_flips(&solver.mesh, &mut solver.displacement, options);
    solver.follow_components();
    solver.apply_rigid_parts(1.0);
    // Fully locked unknowns are forced to exactly zero, and fractional locks
    // scale the displacement (`Fit:696-697`).
    for (index, displacement) in solver.displacement.iter_mut().enumerate() {
        let lock = solver.lock_of[index];
        *displacement = if lock >= 1.0 {
            [0.0, 0.0, 0.0]
        } else {
            scale(*displacement, 1.0 - lock)
        };
    }
    for pass in 0..options.clipping_passes {
        if !solver.clipping_pass(pass) {
            break;
        }
    }
    let positions: Vec<[f32; 3]> = garment
        .0
        .iter()
        .zip(solver.mesh.unknown_of.iter())
        .map(|(&rest, &unknown)| add(rest, solver.displacement[unknown]))
        .collect();
    if let Some(vertex) = first_non_finite(&positions) {
        return Err(FitError::NonFiniteResult { vertex });
    }
    let (inside_after, poke_after) = solver.clipping_counts();

    // How well the pulled unknowns fit now (`Fit:760-778`).
    let fitted: Vec<[f32; 3]> = solver
        .mesh
        .rest
        .iter()
        .zip(solver.displacement.iter())
        .map(|(&rest, &displacement)| add(rest, displacement))
        .collect();
    let mut pulled = 0usize;
    let mut error_sum = 0.0f64;
    let mut max_gap_error = 0.0f32;
    for (index, &position) in fitted.iter().enumerate() {
        if solver.last_weight[index] <= 0.0 || solver.lock_of[index] >= 1.0 {
            continue;
        }
        let Some(hit) = solver.surface.hit(position, max_distance) else {
            continue;
        };
        pulled += 1;
        let signed = dot(sub(position, hit.point), hit.normal);
        let error = (signed - solver.gap_of[index]).abs();
        error_sum += f64::from(error);
        max_gap_error = max_gap_error.max(error);
    }
    let mean_gap_error = if pulled > 0 {
        (error_sum / pulled as f64) as f32
    } else {
        0.0
    };

    let move_sum: f64 = positions
        .iter()
        .zip(garment.0.iter())
        .map(|(after, before)| f64::from(length(sub(*after, *before))))
        .sum();
    let max_move = positions
        .iter()
        .zip(garment.0.iter())
        .map(|(after, before)| length(sub(*after, *before)))
        .fold(0.0f32, f32::max);
    let flipped = count_flipped(garment.0, &positions, garment.1);
    let (_, strained_edges, max_strain, mean_strain) =
        strain_stats(garment.0, &positions, garment.1, REPORT_STRAIN_THRESHOLD);

    Ok((
        positions,
        FitReport {
            vertices: garment.0.len(),
            unknowns: count,
            pulled,
            followed: solver.followed,
            kept_form: solver.kept_form,
            mean_gap_error,
            max_gap_error,
            inside_before,
            inside_after,
            poke_before,
            poke_after,
            flipped,
            mean_move: (move_sum / garment.0.len().max(1) as f64) as f32,
            max_move,
            max_strain,
            mean_strain,
            strained_edges,
        },
    ))
}

/// One running fit: displacement, correspondence state and reports.
struct FitState<'a> {
    mesh: Welded,
    options: &'a FitOptions,
    surface: BodySurface<'a>,
    gap_of: Vec<f32>,
    lock_of: Vec<f32>,
    part_of: Vec<i32>,
    max_distance: f32,
    /// Body vertices sampled for the symmetric pulls (`Fit:452-456`); empty
    /// when `symmetric` is off.
    pulling_surface: Vec<usize>,
    displacement: Vec<[f32; 3]>,
    pull_scale: Vec<f32>,
    last_weight: Vec<f32>,
    followed: usize,
    kept_form: usize,
}

impl FitState<'_> {
    /// One correspondence update and solve (`Fit::iterate`, `513-591`).
    fn iterate(&mut self, lambda: f32) {
        let count = self.mesh.rest.len();
        let current: Vec<[f32; 3]> = self
            .mesh
            .rest
            .iter()
            .zip(self.displacement.iter())
            .map(|(&rest, &displacement)| add(rest, displacement))
            .collect();
        let current_normals = triangle_normal_sums(&current, &self.mesh.faces);
        let mut system = System::new(&self.mesh, lambda);
        for index in 0..count {
            if self.lock_of[index] >= 1.0 {
                continue;
            }
            let Some(hit) = self.surface.hit(current[index], self.max_distance) else {
                continue;
            };
            if self.options.min_normal_agreement > -1.0
                && dot(current_normals[index], hit.normal) < self.options.min_normal_agreement
            {
                continue;
            }
            if !clean_pull(current[index], hit.point, hit.normal) {
                continue;
            }
            let distance = length(sub(current[index], hit.point));
            system.data_weight[index] =
                falloff(distance, self.max_distance) * self.pull_scale[index];
            system.data_target[index] = match self.options.gap_mode {
                GapMode::Keep => {
                    // Only along the normal, so nothing slides across the body.
                    let signed = dot(sub(current[index], hit.point), hit.normal);
                    add(
                        sub(current[index], self.mesh.rest[index]),
                        scale(hit.normal, self.gap_of[index] - signed),
                    )
                }
                GapMode::Constant | GapMode::FromShape => sub(
                    add(hit.point, scale(hit.normal, self.gap_of[index])),
                    self.mesh.rest[index],
                ),
            };
        }

        // Masked vertices stay where they are, and their neighbors bend
        // around them smoothly (`Fit:544-553`).
        let lock_weight = LOCK_WEIGHT * (1.0 + lambda);
        for index in 0..count {
            let lock = self.lock_of[index];
            if lock > 0.0 {
                let keep = lock_weight * lock;
                let pull = system.data_weight[index] * (1.0 - lock);
                system.data_target[index] =
                    scale(system.data_target[index], pull / (pull + keep).max(1e-20));
                system.data_weight[index] = pull + keep;
            }
        }

        if self.options.symmetric && !self.mesh.faces.is_empty() {
            self.add_symmetric_pulls(&mut system, &current, &current_normals);
        }

        self.last_weight.clone_from(&system.data_weight);
        solve(
            &system,
            &mut self.displacement,
            self.options.solver_iterations,
        );
    }

    /// Body points pull the garment onto parts it does not reach yet
    /// (`Fit:555-587`).
    fn add_symmetric_pulls(
        &self,
        system: &mut System<'_>,
        current: &[[f32; 3]],
        current_normals: &[[f32; 3]],
    ) {
        let faces: Vec<[u32; 3]> = self
            .mesh
            .faces
            .iter()
            .map(|face| [face[0] as u32, face[1] as u32, face[2] as u32])
            .collect();
        let index = SurfaceIndex::build(current, &faces);
        for &vertex in &self.pulling_surface {
            let point = self.surface.positions[vertex];
            let Some(hit) = surface_hit(
                &index,
                &faces,
                current,
                current_normals,
                point,
                self.max_distance,
            ) else {
                continue;
            };
            if self.options.min_normal_agreement > -1.0
                && dot(hit.normal, self.surface.normals[vertex]) < self.options.min_normal_agreement
            {
                continue;
            }
            if !clean_pull(point, hit.point, hit.normal) {
                continue;
            }
            let face = self.mesh.faces[hit.triangle];
            // Half as strong as the garment's own pulls, and not on locked
            // vertices.
            let locked = face.iter().map(|&u| self.lock_of[u]).fold(0.0, f32::max);
            let weakest = face
                .iter()
                .map(|&u| self.pull_scale[u])
                .fold(f32::INFINITY, f32::min);
            let weight = 0.5 * falloff(hit.distance, self.max_distance) * (1.0 - locked) * weakest;
            if weight <= 0.0 {
                continue;
            }
            let target = add(point, scale(self.surface.normals[vertex], self.options.gap));
            system.pulls.push(SurfacePull {
                unknowns: face,
                bary: hit.bary,
                target,
                weight,
            });
        }
    }

    /// Where the pull would stretch or squash edges by more than the cap, the
    /// pull there and around it gives way (`Fit:604-641`).
    fn relax_strain(&mut self) {
        let count = self.mesh.rest.len();
        let mut edges_of: Vec<Vec<usize>> = vec![Vec::new(); count];
        for face in &self.mesh.faces {
            edges_of[face[0]].push(face[1]);
            edges_of[face[0]].push(face[2]);
            edges_of[face[1]].push(face[2]);
        }
        for _ in 0..4 {
            let mut strained = vec![false; count];
            let mut found = 0usize;
            for a in 0..count {
                for &b in &edges_of[a] {
                    let before = length(sub(self.mesh.rest[a], self.mesh.rest[b]));
                    let after = length(sub(
                        add(self.mesh.rest[a], self.displacement[a]),
                        add(self.mesh.rest[b], self.displacement[b]),
                    ));
                    let (shorter, longer) = if before < after {
                        (before, after)
                    } else {
                        (after, before)
                    };
                    if shorter > 1e-6 && longer / shorter - 1.0 > self.options.max_strain {
                        found += usize::from(!strained[a]) + usize::from(!strained[b]);
                        strained[a] = true;
                        strained[b] = true;
                    }
                }
            }
            if found == 0 {
                return;
            }
            self.kept_form += found;
            let mut relax = strained.clone();
            for (index, &is_strained) in strained.iter().enumerate() {
                if is_strained {
                    for &neighbour in &self.mesh.neighbours[index] {
                        relax[neighbour] = true;
                    }
                }
            }
            for (index, &is_relaxed) in relax.iter().enumerate() {
                if is_relaxed {
                    self.pull_scale[index] *= 0.25;
                }
            }
            let final_lambda = self.options.stiffness.max(1e-4);
            for _ in 0..self.options.iterations {
                self.iterate(final_lambda);
            }
        }
    }

    /// Pieces that nothing pulled move with the closest pulled vertices, as a
    /// whole (`Fit:643-680`).
    fn follow_components(&mut self) {
        let count = self.mesh.rest.len();
        let component = components(&self.mesh);
        let component_count = component.iter().copied().max().map_or(0, |top| top + 1);
        let mut component_pull = vec![0.0f64; component_count];
        for (index, &component) in component.iter().enumerate() {
            if self.lock_of[index] < 1.0 {
                component_pull[component] += f64::from(self.last_weight[index]);
            }
        }
        let pulled: Vec<usize> = (0..count)
            .filter(|&index| component_pull[component[index]] > 0.0)
            .collect();
        if pulled.is_empty() {
            return;
        }
        let pulled_points: Vec<[f32; 3]> =
            pulled.iter().map(|&index| self.mesh.rest[index]).collect();
        // The source queries its insertion-order kd tree with radius 0.
        let tree = PointIndex::build_native(&pulled_points);
        let mut follow = vec![[0.0f32; 3]; component_count];
        let mut follow_count = vec![0usize; component_count];
        for index in 0..count {
            let component = component[index];
            if component_pull[component] > 0.0 || self.lock_of[index] >= 1.0 {
                continue;
            }
            let nearest = tree.nearest(self.mesh.rest[index]);
            follow[component] = add(follow[component], self.displacement[pulled[nearest]]);
            follow_count[component] += 1;
        }
        for index in 0..count {
            let component = component[index];
            if follow_count[component] > 0 {
                self.displacement[index] =
                    scale(follow[component], 1.0 / follow_count[component] as f32);
                self.followed += 1;
            }
        }
    }

    /// Rigid parts move as a whole, by the motion that best fits the
    /// displacement of their vertices (`Fit:682-688`).
    fn apply_rigid_parts(&mut self, share: f32) {
        if !self.part_of.iter().any(|&part| part >= 0) {
            return;
        }
        let part_sizes = part_sizes(&self.part_of);
        let influence: Vec<f32> = self
            .part_of
            .iter()
            .map(|&part| {
                if part < 0 {
                    0.0
                } else if part_sizes[&part] <= SMALL_PART {
                    share.max(0.5)
                } else {
                    share
                }
            })
            .collect();
        self.displacement = rigidify_deltas(
            &self.mesh.rest,
            &self.displacement,
            &self.part_of,
            &influence,
            self.options.rigid_scale,
        );
    }

    /// The mesh's clipping at a displacement: unknowns inside the body and
    /// body pokes between them (`Fit::findClipping`, `463-501`).
    fn clipping(&self) -> (Vec<bool>, Vec<SurfacePull>) {
        let count = self.mesh.rest.len();
        let current: Vec<[f32; 3]> = self
            .mesh
            .rest
            .iter()
            .zip(self.displacement.iter())
            .map(|(&rest, &displacement)| add(rest, displacement))
            .collect();
        let mut inside = vec![false; count];
        for (index, &point) in current.iter().enumerate() {
            if let Some(hit) = self.surface.hit(point, self.max_distance)
                && clean_pull(point, hit.point, hit.normal)
                && dot(sub(point, hit.point), hit.normal) < 0.0
            {
                inside[index] = true;
            }
        }
        let pokes = if self.mesh.faces.is_empty() || self.options.poke_depth <= 0.0 {
            Vec::new()
        } else {
            self.body_pokes(&current)
        };
        (inside, pokes)
    }

    fn clipping_counts(&self) -> (usize, usize) {
        let (inside, pokes) = self.clipping();
        (inside.iter().filter(|&&value| value).count(), pokes.len())
    }

    /// Surface vertices with the garment close behind them, looking into the
    /// body: they show through it (`ClippingFixer::BodyPokeThrough`).
    /// The shared BVH finds the nearest inward crossing in the native depth
    /// interval; a closer off-ray triangle cannot hide the crossing.
    fn body_pokes(&self, current: &[[f32; 3]]) -> Vec<SurfacePull> {
        let faces: Vec<[u32; 3]> = self
            .mesh
            .faces
            .iter()
            .map(|face| [face[0] as u32, face[1] as u32, face[2] as u32])
            .collect();
        let index = SurfaceIndex::build(current, &faces);
        let mut pokes = Vec::new();
        for (vertex, &point) in self.surface.positions.iter().enumerate() {
            let normal = self.surface.normals[vertex];
            let inward = scale(normal, -1.0);
            if inward.iter().all(|component| component.abs() < 1e-4) {
                continue;
            }
            let Some((hit, depth)) =
                index.first_hit(point, inward, 1e-5, self.options.poke_depth, &[])
            else {
                continue;
            };
            let face = self.mesh.faces[hit.triangle];
            let corners = [current[face[0]], current[face[1]], current[face[2]]];
            if self.surface.culls_poke(vertex, inward, depth) {
                continue;
            }
            let bary = barycentric_clamped(hit.location, corners);
            pokes.push(SurfacePull {
                unknowns: face,
                bary,
                target: add(point, scale(normal, self.options.min_gap)),
                weight: 1.0,
            });
        }
        pokes
    }

    /// One clipping pass; `false` when nothing clips (`Fit:702-750`).
    fn clipping_pass(&mut self, pass: u32) -> bool {
        let count = self.mesh.rest.len();
        let (inside, pokes) = self.clipping();
        let inside_count = inside.iter().filter(|&&value| value).count();
        if inside_count == 0 && pokes.is_empty() {
            return false;
        }
        let current: Vec<[f32; 3]> = self
            .mesh
            .rest
            .iter()
            .zip(self.displacement.iter())
            .map(|(&rest, &displacement)| add(rest, displacement))
            .collect();
        let lambda = self.options.stiffness;
        let mut system = System::new(&self.mesh, lambda);
        // Hold the fit; everything else that pushes spreads over the
        // neighbors through the smoothness.
        for index in 0..count {
            system.data_weight[index] = 1.0;
            system.data_target[index] = self.displacement[index];
            let lock = self.lock_of[index];
            if lock > 0.0 {
                system.data_weight[index] = LOCK_WEIGHT * lock + 1.0;
                continue;
            }
            if !inside[index] {
                continue;
            }
            let Some(hit) = self.surface.hit(current[index], self.max_distance) else {
                continue;
            };
            // Out to the least gap, and a little more each pass that still
            // finds it inside.
            system.data_weight[index] = CLIPPING_PUSH;
            system.data_target[index] = sub(
                add(
                    hit.point,
                    scale(hit.normal, self.options.min_gap * (1.0 + pass as f32)),
                ),
                self.mesh.rest[index],
            );
        }
        for mut poke in pokes {
            let locked = poke
                .unknowns
                .iter()
                .map(|&unknown| self.lock_of[unknown])
                .fold(0.0, f32::max);
            if locked >= 1.0 {
                continue;
            }
            poke.weight = CLIPPING_PUSH * (1.0 - locked);
            system.pulls.push(poke);
        }
        solve(
            &system,
            &mut self.displacement,
            self.options.solver_iterations,
        );
        // Rigid parts stay rigid at first, and give way bit by bit where they
        // still clip, so the last passes don't undo their own pushes.
        let share = 1.0 - f64::from(pass + 1) / f64::from(self.options.clipping_passes);
        self.apply_rigid_parts(share as f32);
        // The source holds fully locked unknowns with a heavy weight only;
        // here they are exact, through every pass.
        for (index, displacement) in self.displacement.iter_mut().enumerate() {
            if self.lock_of[index] >= 1.0 {
                *displacement = [0.0, 0.0, 0.0];
            }
        }
        true
    }
}

/// The body as the fit queries it: one shared surface index over oriented
/// faces, plus welded vertex normals.
struct BodySurface<'a> {
    positions: &'a [[f32; 3]],
    faces: Vec<[u32; 3]>,
    normals: Vec<[f32; 3]>,
    index: SurfaceIndex,
    incident_faces: Vec<Vec<usize>>,
}

impl<'a> BodySurface<'a> {
    fn build(positions: &'a [[f32; 3]], faces: &[[u32; 3]]) -> Self {
        let faces = orient_faces(positions, faces);
        let normals = welded_vertex_normals(positions, &faces);
        let index = SurfaceIndex::build(positions, &faces);
        let mut incident_faces = vec![Vec::new(); positions.len()];
        for (index, face) in faces.iter().enumerate() {
            for &vertex in face {
                incident_faces[vertex as usize].push(index);
            }
        }
        Self {
            positions,
            faces,
            normals,
            index,
            incident_faces,
        }
    }

    /// Native `SurfaceQuery::Closest` through the shared seam: the nearest
    /// triangle within `radius`, with the barycentrically interpolated
    /// vertex normal there.
    fn hit(&self, point: [f32; 3], radius: f32) -> Option<SurfaceHit> {
        surface_hit(
            &self.index,
            &self.faces,
            self.positions,
            &self.normals,
            point,
            radius,
        )
    }

    /// Native far-side culling ignores source-vertex incident triangles.
    fn culls_poke(&self, vertex: usize, inward: [f32; 3], depth: f32) -> bool {
        self.index
            .first_hit(
                self.positions[vertex],
                inward,
                1e-3,
                depth,
                &self.incident_faces[vertex],
            )
            .is_some_and(|(_, distance)| distance < depth)
    }
}

/// One closest-point hit on a triangle surface.
#[derive(Debug, Clone, Copy)]
struct SurfaceHit {
    /// Index into the faces the query was built from.
    triangle: usize,
    point: [f32; 3],
    normal: [f32; 3],
    distance: f32,
    bary: [f32; 3],
}

/// Shared-seam closest point with interpolated normal, mirroring
/// `SurfaceQuery::Closest` (`SurfaceFit.cpp:339-363`).
fn surface_hit(
    index: &SurfaceIndex,
    faces: &[[u32; 3]],
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    point: [f32; 3],
    radius: f32,
) -> Option<SurfaceHit> {
    let nearest = index.closest(point)?;
    if nearest.distance > radius {
        return None;
    }
    let face = faces[nearest.triangle];
    let corners = [
        positions[face[0] as usize],
        positions[face[1] as usize],
        positions[face[2] as usize],
    ];
    let bary = barycentric_clamped(nearest.point, corners);
    let summed = add(
        add(
            scale(normals[face[0] as usize], bary[0]),
            scale(normals[face[1] as usize], bary[1]),
        ),
        scale(normals[face[2] as usize], bary[2]),
    );
    let mut normal = unit(summed);
    if normal == [0.0, 0.0, 0.0] {
        normal = unit(face_normal_of(corners));
    }
    Some(SurfaceHit {
        triangle: nearest.triangle,
        point: nearest.point,
        normal,
        distance: nearest.distance,
        bary,
    })
}

/// The welded garment the solver works on: one unknown per group of vertex
/// copies (`Welded`, `SurfaceFit.cpp:80-85`).
struct Welded {
    unknown_of: Vec<usize>,
    rest: Vec<[f32; 3]>,
    faces: Vec<[usize; 3]>,
    neighbours: Vec<Vec<usize>>,
}

/// Weld coincident vertex copies into shared unknowns (`Weld`, `87-127`).
fn weld(positions: &[[f32; 3]], faces: &[[u32; 3]], weld_distance: f32) -> Welded {
    let mut unknown_of = vec![usize::MAX; positions.len()];
    let mut rest: Vec<[f32; 3]> = Vec::new();
    if weld_distance > 0.0 {
        for group in coincident_groups(positions, weld_distance) {
            let unknown = rest.len();
            rest.push(positions[group[0]]);
            for &vertex in &group {
                unknown_of[vertex] = unknown;
            }
        }
    }
    for (vertex, &position) in positions.iter().enumerate() {
        if unknown_of[vertex] != usize::MAX {
            continue;
        }
        unknown_of[vertex] = rest.len();
        rest.push(position);
    }
    let mut welded_faces = Vec::with_capacity(faces.len());
    let mut neighbour_sets = vec![BTreeSet::new(); rest.len()];
    for &face in faces {
        let corners = [
            unknown_of[face[0] as usize],
            unknown_of[face[1] as usize],
            unknown_of[face[2] as usize],
        ];
        if corners[0] == corners[1] || corners[1] == corners[2] || corners[0] == corners[2] {
            continue;
        }
        for (a, b) in edge_pairs(&corners) {
            neighbour_sets[a].insert(b);
            neighbour_sets[b].insert(a);
        }
        welded_faces.push(corners);
    }
    Welded {
        unknown_of,
        rest,
        faces: welded_faces,
        neighbours: neighbour_sets
            .into_iter()
            .map(|set| set.into_iter().collect())
            .collect(),
    }
}

/// Area-weighted unit vertex normal sums over a welded mesh
/// (`TriangleNormalSums`, `SurfaceFit.cpp:129-143`).
fn triangle_normal_sums(positions: &[[f32; 3]], faces: &[[usize; 3]]) -> Vec<[f32; 3]> {
    let mut sums = vec![[0.0f32; 3]; positions.len()];
    for face in faces {
        let normal = cross(
            sub(positions[face[1]], positions[face[0]]),
            sub(positions[face[2]], positions[face[0]]),
        );
        for &corner in face {
            sums[corner] = add(sums[corner], normal);
        }
    }
    sums.iter().copied().map(unit).collect()
}

/// Welded vertex normals, one per input vertex
/// (`VertexNormals`, `SurfaceFit.cpp:373-380`).
fn welded_vertex_normals(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mesh = weld(positions, faces, BODY_WELD_DISTANCE);
    let sums = triangle_normal_sums(&mesh.rest, &mesh.faces);
    mesh.unknown_of
        .iter()
        .map(|&unknown| sums[unknown])
        .collect()
}

/// Reorder triangles so every closed connected component's winding gives
/// outward normals (positive signed volume); open shells keep their authored
/// winding, whose cone-volume sign means nothing.
///
/// The source trusts the caller's winding; authored bodies here mix shells,
/// and an inward-facing one would invert every clearance sign.
fn orient_faces(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Vec<[u32; 3]> {
    let count = positions.len();
    let mut parent: Vec<usize> = (0..count).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut faces = faces.to_vec();
    for face in &faces {
        for (a, b) in [
            (face[0] as usize, face[1] as usize),
            (face[1] as usize, face[2] as usize),
        ] {
            let (a, b) = (find(&mut parent, a), find(&mut parent, b));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut edge_uses: HashMap<(usize, usize), usize> = HashMap::new();
    for face in &faces {
        for (a, b) in [
            (face[0] as usize, face[1] as usize),
            (face[1] as usize, face[2] as usize),
            (face[2] as usize, face[0] as usize),
        ] {
            *edge_uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    let mut open = vec![false; count];
    for ((a, _b), uses) in &edge_uses {
        if *uses == 1 {
            open[find(&mut parent, *a)] = true;
        }
    }
    let mut volume = vec![0.0f64; count];
    for face in &faces {
        let (a, b, c) = (
            positions[face[0] as usize],
            positions[face[1] as usize],
            positions[face[2] as usize],
        );
        let root = find(&mut parent, face[0] as usize);
        volume[root] += f64::from(dot(a, cross(b, c))) / 6.0;
    }
    for face in &mut faces {
        let root = find(&mut parent, face[0] as usize);
        if !open[root] && volume[root] < 0.0 {
            face.swap(1, 2);
        }
    }
    faces
}

/// Taubin relief: alternating shrink/inflate passes measure how far each
/// vertex stands out along its normal; that goes onto its gap
/// (`Fit:407-432`).
fn add_relief(mesh: &Welded, gaps: &mut [f32], options: &FitOptions) {
    let mut smoothed = mesh.rest.clone();
    let step = |smoothed: &mut Vec<[f32; 3]>, factor: f32| {
        let mut next = smoothed.clone();
        for (index, neighbours) in mesh.neighbours.iter().enumerate() {
            if neighbours.is_empty() {
                continue;
            }
            let mut mean = [0.0f32; 3];
            for &neighbour in neighbours {
                mean = add(mean, smoothed[neighbour]);
            }
            next[index] = add(
                smoothed[index],
                scale(
                    sub(scale(mean, 1.0 / neighbours.len() as f32), smoothed[index]),
                    factor,
                ),
            );
        }
        *smoothed = next;
    };
    for _ in 0..options.relief_smoothing {
        step(&mut smoothed, 0.5);
        step(&mut smoothed, -0.53);
    }
    let normals = triangle_normal_sums(&mesh.rest, &mesh.faces);
    for index in 0..mesh.rest.len() {
        gaps[index] += dot(sub(mesh.rest[index], smoothed[index]), normals[index]);
    }
}

/// A pull of a point between unknowns, given by a triangle and barycentric
/// weights, towards a surface point (`SurfacePull`, `SurfaceFit.cpp:71-77`).
#[derive(Debug, Clone, Copy)]
struct SurfacePull {
    unknowns: [usize; 3],
    bary: [f32; 3],
    target: [f32; 3],
    weight: f32,
}

/// The linear system of one solve: data pulls on single unknowns, pulls on
/// points between unknowns, and the smoothness of the displacement (`System`,
/// `SurfaceFit.cpp:147-254`).
struct System<'mesh> {
    mesh: &'mesh Welded,
    lambda: f32,
    data_weight: Vec<f32>,
    data_target: Vec<[f32; 3]>,
    pulls: Vec<SurfacePull>,
    inverse_degree: Vec<f32>,
}

impl<'mesh> System<'mesh> {
    fn new(mesh: &'mesh Welded, lambda: f32) -> Self {
        let inverse_degree = mesh
            .neighbours
            .iter()
            .map(|neighbours| {
                if neighbours.is_empty() {
                    0.0
                } else {
                    1.0 / neighbours.len() as f32
                }
            })
            .collect();
        Self {
            mesh,
            lambda,
            data_weight: vec![0.0; mesh.rest.len()],
            data_target: vec![[0.0; 3]; mesh.rest.len()],
            pulls: Vec::new(),
            inverse_degree,
        }
    }

    fn laplacian(&self, d: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; d.len()];
        for (index, neighbours) in self.mesh.neighbours.iter().enumerate() {
            if neighbours.is_empty() {
                continue;
            }
            let mut mean = [0.0f32; 3];
            for &neighbour in neighbours {
                mean = add(mean, d[neighbour]);
            }
            out[index] = sub(d[index], scale(mean, self.inverse_degree[index]));
        }
        out
    }

    fn laplacian_transposed(&self, y: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; y.len()];
        for (index, neighbours) in self.mesh.neighbours.iter().enumerate() {
            if neighbours.is_empty() {
                continue;
            }
            let mut sum = [0.0f32; 3];
            for &neighbour in neighbours {
                sum = add(sum, scale(y[neighbour], self.inverse_degree[neighbour]));
            }
            out[index] = sub(y[index], sum);
        }
        out
    }

    /// `A` times `d`, pulls included.
    fn apply(&self, d: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let mut out = self.laplacian_transposed(&self.laplacian(d));
        let membrane = self.lambda * MEMBRANE_SHARE;
        for index in 0..d.len() {
            out[index] = add(
                scale(out[index], self.lambda),
                scale(d[index], self.data_weight[index] + REGULARIZATION),
            );
            let mut difference = [0.0f32; 3];
            for &neighbour in &self.mesh.neighbours[index] {
                difference = add(difference, sub(d[index], d[neighbour]));
            }
            out[index] = add(out[index], scale(difference, membrane));
        }
        for pull in &self.pulls {
            let at = barycentric_point(d, pull.unknowns, pull.bary);
            for corner in 0..3 {
                out[pull.unknowns[corner]] = add(
                    out[pull.unknowns[corner]],
                    scale(at, pull.weight * pull.bary[corner]),
                );
            }
        }
        out
    }

    fn right_hand_side(&self) -> Vec<[f32; 3]> {
        let mut b = self
            .data_weight
            .iter()
            .zip(self.data_target.iter())
            .map(|(&weight, &target)| scale(target, weight))
            .collect::<Vec<_>>();
        for pull in &self.pulls {
            let rest_at = barycentric_point(&self.mesh.rest, pull.unknowns, pull.bary);
            let want = sub(pull.target, rest_at);
            for corner in 0..3 {
                b[pull.unknowns[corner]] = add(
                    b[pull.unknowns[corner]],
                    scale(want, pull.weight * pull.bary[corner]),
                );
            }
        }
        b
    }

    fn diagonal(&self) -> Vec<f32> {
        let mut diagonal = Vec::with_capacity(self.mesh.rest.len());
        for (index, neighbours) in self.mesh.neighbours.iter().enumerate() {
            let mut ltl = if neighbours.is_empty() { 0.0 } else { 1.0 };
            for &neighbour in neighbours {
                ltl += self.inverse_degree[neighbour] * self.inverse_degree[neighbour];
            }
            diagonal.push(
                self.lambda * ltl
                    + self.data_weight[index]
                    + REGULARIZATION
                    + self.lambda * MEMBRANE_SHARE * neighbours.len() as f32,
            );
        }
        for pull in &self.pulls {
            for corner in 0..3 {
                diagonal[pull.unknowns[corner]] += pull.weight * pull.bary[corner].powi(2);
            }
        }
        diagonal
    }
}

/// Preconditioned conjugate gradients from the displacement as it is
/// (`Solve`, `SurfaceFit.cpp:264-299`). Dot products accumulate in f64.
fn solve(system: &System<'_>, displacement: &mut [[f32; 3]], max_iterations: u32) {
    let b = system.right_hand_side();
    let diagonal = system.diagonal();
    let count = displacement.len();
    let ad = system.apply(displacement);
    let mut residual: Vec<[f32; 3]> = (0..count).map(|index| sub(b[index], ad[index])).collect();
    let mut z: Vec<[f32; 3]> = residual
        .iter()
        .zip(diagonal.iter())
        .map(|(&value, &diagonal)| scale(value, 1.0 / diagonal))
        .collect();
    let mut p = z.clone();
    let mut rz = dot_all(&residual, &z);
    let b_norm = dot_all(&b, &b).max(1e-20);
    for _ in 0..max_iterations {
        if dot_all(&residual, &residual) < 1e-12 * b_norm {
            break;
        }
        let ap = system.apply(&p);
        let pap = dot_all(&p, &ap);
        if pap <= 0.0 {
            break;
        }
        let alpha = (rz / pap) as f32;
        for index in 0..count {
            displacement[index] = add(displacement[index], scale(p[index], alpha));
            residual[index] = sub(residual[index], scale(ap[index], alpha));
            z[index] = scale(residual[index], 1.0 / diagonal[index]);
        }
        let rz_new = dot_all(&residual, &z);
        let beta = (rz_new / rz.max(1e-30)) as f32;
        rz = rz_new;
        for index in 0..count {
            p[index] = add(z[index], scale(p[index], beta));
        }
    }
}

/// Damp the displacement around flipped triangles, ring by ring. The source
/// ships this as its separate `fix_flipped_triangles` tool; this port runs
/// it inline, before locks are applied.
fn repair_flips(mesh: &Welded, displacement: &mut [[f32; 3]], options: &FitOptions) {
    if options.flip_rings == 0 || options.flip_strength <= 0.0 {
        return;
    }
    let flipped_faces = |displacement: &[[f32; 3]]| -> Vec<usize> {
        mesh.faces
            .iter()
            .enumerate()
            .filter_map(|(index, face)| {
                let before = cross(
                    sub(mesh.rest[face[1]], mesh.rest[face[0]]),
                    sub(mesh.rest[face[2]], mesh.rest[face[0]]),
                );
                let after = cross(
                    sub(
                        add(mesh.rest[face[1]], displacement[face[1]]),
                        add(mesh.rest[face[0]], displacement[face[0]]),
                    ),
                    sub(
                        add(mesh.rest[face[2]], displacement[face[2]]),
                        add(mesh.rest[face[0]], displacement[face[0]]),
                    ),
                );
                (dot(before, after) < 0.0).then_some(index)
            })
            .collect()
    };
    for _ in 0..options.flip_rings {
        let flipped = flipped_faces(displacement);
        if flipped.is_empty() {
            return;
        }
        let mut affected = vec![false; mesh.rest.len()];
        for &face in &flipped {
            for &corner in &mesh.faces[face] {
                affected[corner] = true;
                for &neighbour in &mesh.neighbours[corner] {
                    affected[neighbour] = true;
                }
            }
        }
        for (index, &is_affected) in affected.iter().enumerate() {
            if is_affected {
                displacement[index] = scale(displacement[index], 1.0 - options.flip_strength);
            }
        }
    }
}

/// Connected pieces of the welded mesh (`Components`, `SurfaceFit.cpp:302-323`).
fn components(mesh: &Welded) -> Vec<usize> {
    let mut component = vec![usize::MAX; mesh.rest.len()];
    let mut next = 0usize;
    let mut stack = Vec::new();
    for start in 0..mesh.rest.len() {
        if component[start] != usize::MAX {
            continue;
        }
        component[start] = next;
        stack.push(start);
        while let Some(index) = stack.pop() {
            for &neighbour in &mesh.neighbours[index] {
                if component[neighbour] == usize::MAX {
                    component[neighbour] = next;
                    stack.push(neighbour);
                }
            }
        }
        next += 1;
    }
    component
}

/// Members per rigid part id, over unknowns.
fn part_sizes(part_of: &[i32]) -> BTreeMap<i32, usize> {
    let mut sizes = BTreeMap::new();
    for &part in part_of {
        if part >= 0 {
            *sizes.entry(part).or_insert(0) += 1;
        }
    }
    sizes
}

/// The similarity transform of Horn's method: rotation, optional uniform
/// scale and translation (`SimilarityTransform`, `MeshAnalysis.h:115`).
struct Similarity {
    rotation: [[f64; 3]; 3],
    scale: f64,
    translation: [f64; 3],
}

impl Similarity {
    fn identity() -> Self {
        Self {
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
            translation: [0.0; 3],
        }
    }

    fn apply(&self, point: [f32; 3]) -> [f64; 3] {
        let point = point.map(f64::from);
        let mut out = [0.0f64; 3];
        for (row, value) in out.iter_mut().enumerate() {
            *value = self.scale
                * (self.rotation[row][0] * point[0]
                    + self.rotation[row][1] * point[1]
                    + self.rotation[row][2] * point[2])
                + self.translation[row];
        }
        out
    }
}

/// Weighted best-fit rotation, optional scale and translation mapping `from`
/// onto `to` (`BestFitTransform`, `MeshAnalysis.cpp:526-608`).
fn best_fit_transform(
    from: &[[f32; 3]],
    to: &[[f32; 3]],
    weights: &[f32],
    allow_scale: bool,
) -> Similarity {
    let mut result = Similarity::identity();
    let count = from.len().min(to.len()).min(weights.len());
    let mut weight_sum = 0.0f64;
    let mut from_centroid = [0.0f64; 3];
    let mut to_centroid = [0.0f64; 3];
    for index in 0..count {
        let weight = f64::from(weights[index]);
        if weight <= 0.0 {
            continue;
        }
        weight_sum += weight;
        for axis in 0..3 {
            from_centroid[axis] += weight * f64::from(from[index][axis]);
            to_centroid[axis] += weight * f64::from(to[index][axis]);
        }
    }
    if weight_sum <= 0.0 {
        return result;
    }
    for axis in 0..3 {
        from_centroid[axis] /= weight_sum;
        to_centroid[axis] /= weight_sum;
    }

    let mut covariance = [[0.0f64; 3]; 3];
    let mut spread = 0.0f64;
    for index in 0..count {
        let weight = f64::from(weights[index]);
        if weight <= 0.0 {
            continue;
        }
        let p = [
            f64::from(from[index][0]) - from_centroid[0],
            f64::from(from[index][1]) - from_centroid[1],
            f64::from(from[index][2]) - from_centroid[2],
        ];
        let q = [
            f64::from(to[index][0]) - to_centroid[0],
            f64::from(to[index][1]) - to_centroid[1],
            f64::from(to[index][2]) - to_centroid[2],
        ];
        for row in 0..3 {
            for column in 0..3 {
                covariance[row][column] += weight * p[row] * q[column];
            }
        }
        spread += weight * (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]);
    }

    // Horn's method: the rotation is the unit quaternion of the largest
    // eigenvalue of this matrix.
    let matrix = [
        [
            covariance[0][0] + covariance[1][1] + covariance[2][2],
            covariance[1][2] - covariance[2][1],
            covariance[2][0] - covariance[0][2],
            covariance[0][1] - covariance[1][0],
        ],
        [
            covariance[1][2] - covariance[2][1],
            covariance[0][0] - covariance[1][1] - covariance[2][2],
            covariance[0][1] + covariance[1][0],
            covariance[2][0] + covariance[0][2],
        ],
        [
            covariance[2][0] - covariance[0][2],
            covariance[0][1] + covariance[1][0],
            -covariance[0][0] + covariance[1][1] - covariance[2][2],
            covariance[1][2] + covariance[2][1],
        ],
        [
            covariance[0][1] - covariance[1][0],
            covariance[2][0] + covariance[0][2],
            covariance[1][2] + covariance[2][1],
            -covariance[0][0] - covariance[1][1] + covariance[2][2],
        ],
    ];
    let quaternion = largest_eigenvector(matrix);
    let length = (quaternion[0] * quaternion[0]
        + quaternion[1] * quaternion[1]
        + quaternion[2] * quaternion[2]
        + quaternion[3] * quaternion[3])
        .sqrt();
    if length > 0.0 {
        let [qw, qx, qy, qz] = quaternion.map(|value| value / length);
        result.rotation = [
            [
                1.0 - 2.0 * (qy * qy + qz * qz),
                2.0 * (qx * qy - qw * qz),
                2.0 * (qx * qz + qw * qy),
            ],
            [
                2.0 * (qx * qy + qw * qz),
                1.0 - 2.0 * (qx * qx + qz * qz),
                2.0 * (qy * qz - qw * qx),
            ],
            [
                2.0 * (qx * qz - qw * qy),
                2.0 * (qy * qz + qw * qx),
                1.0 - 2.0 * (qx * qx + qy * qy),
            ],
        ];
    }

    if allow_scale && spread > 1e-12 {
        let dot = (0..3)
            .map(|row| {
                (0..3)
                    .map(|column| result.rotation[row][column] * covariance[column][row])
                    .sum::<f64>()
            })
            .sum::<f64>();
        if dot > 0.0 {
            result.scale = dot / spread;
        }
    }

    for (row, &target) in to_centroid.iter().enumerate() {
        let moved = result.scale
            * (result.rotation[row][0] * from_centroid[0]
                + result.rotation[row][1] * from_centroid[1]
                + result.rotation[row][2] * from_centroid[2]);
        result.translation[row] = target - moved;
    }
    result
}

/// Eigenvector of the largest eigenvalue of a symmetric 4x4 matrix, by Jacobi
/// rotations (`LargestEigenvector`, `MeshAnalysis.cpp:445-497`).
fn largest_eigenvector(mut matrix: [[f64; 4]; 4]) -> [f64; 4] {
    let mut vectors = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for _ in 0..50 {
        let mut off_diagonal = 0.0;
        let mut diagonal = 0.0;
        for (p, row) in matrix.iter().enumerate() {
            diagonal += row[p] * row[p];
            for value in row.iter().skip(p + 1) {
                off_diagonal += value * value;
            }
        }
        if off_diagonal <= 1e-24 * diagonal || off_diagonal < 1e-300 {
            break;
        }
        for p in 0..4 {
            for q in (p + 1)..4 {
                if matrix[p][q] == 0.0 {
                    continue;
                }
                let theta = (matrix[q][q] - matrix[p][p]) / (2.0 * matrix[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in &mut matrix {
                    let (akp, akq) = (row[p], row[q]);
                    row[p] = c * akp - s * akq;
                    row[q] = s * akp + c * akq;
                }
                let (before_q, from_q) = matrix.split_at_mut(q);
                for (apk, aqk) in before_q[p].iter_mut().zip(from_q[0].iter_mut()) {
                    let (old_p, old_q) = (*apk, *aqk);
                    *apk = c * old_p - s * old_q;
                    *aqk = s * old_p + c * old_q;
                }
                for row in &mut vectors {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut largest = 0;
    for index in 1..4 {
        if matrix[index][index] > matrix[largest][largest] {
            largest = index;
        }
    }
    [
        vectors[0][largest],
        vectors[1][largest],
        vectors[2][largest],
        vectors[3][largest],
    ]
}

/// Replace each part's displacement with the displacement of its best-fit
/// rigid motion, blended by influence (`RigidifyDeltas`,
/// `MeshAnalysis.cpp:610-637`).
fn rigidify_deltas(
    rest: &[[f32; 3]],
    deltas: &[[f32; 3]],
    parts: &[i32],
    influence: &[f32],
    allow_scale: bool,
) -> Vec<[f32; 3]> {
    let mut result = deltas.to_vec();
    let count = rest
        .len()
        .min(deltas.len())
        .min(parts.len())
        .min(influence.len());
    let mut members: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for vertex in 0..count {
        if parts[vertex] >= 0 && influence[vertex] > 0.0 {
            members.entry(parts[vertex]).or_default().push(vertex);
        }
    }
    for (_, part) in members {
        let from: Vec<[f32; 3]> = part.iter().map(|&vertex| rest[vertex]).collect();
        let to: Vec<[f32; 3]> = part
            .iter()
            .map(|&vertex| add(rest[vertex], deltas[vertex]))
            .collect();
        let weights: Vec<f32> = part.iter().map(|&vertex| influence[vertex]).collect();
        let fit = best_fit_transform(&from, &to, &weights, allow_scale);
        for &vertex in &part {
            let rigid = sub(
                fit.apply(rest[vertex]).map(|value| value as f32),
                rest[vertex],
            );
            let blend = influence[vertex].min(1.0);
            result[vertex] = add(scale(deltas[vertex], 1.0 - blend), scale(rigid, blend));
        }
    }
    result
}

/// Signed standoffs to a surface, the way the from-shape mode measures them
/// (`SignedDistancesToBody`, `ClippingFixer.cpp:235-320`): positive outside,
/// `None` beyond the radius; past an open edge the unsigned distance reads.
fn signed_standoffs(
    points: &[[f32; 3]],
    surface: &BodySurface<'_>,
    radius: f32,
) -> Vec<Option<f32>> {
    let open = OpenEdges::build(&surface.faces, surface.positions.len());
    points
        .iter()
        .map(|&point| {
            let hit = surface.hit(point, radius)?;
            let face = surface.faces[hit.triangle];
            let offset = sub(point, hit.point);
            Some(match past_open_edge(&open, face, hit.bary) {
                true => length(offset),
                false => dot(offset, hit.normal),
            })
        })
        .collect()
}

/// Triangles whose normal turned by more than 90 degrees; degenerate ones
/// have no facing to flip (`FlippedTriangles`, `MeshAnalysis.cpp:124-147`).
fn count_flipped(before: &[[f32; 3]], after: &[[f32; 3]], faces: &[[u32; 3]]) -> usize {
    let min_area_sq = 1e-8f32;
    faces
        .iter()
        .filter(|face| {
            let corners = |positions: &[[f32; 3]]| {
                [
                    positions[face[0] as usize],
                    positions[face[1] as usize],
                    positions[face[2] as usize],
                ]
            };
            let n1 = face_normal_of(corners(before));
            let n2 = face_normal_of(corners(after));
            length(n1) >= min_area_sq && length(n2) >= min_area_sq && dot(n1, n2) < 0.0
        })
        .count()
}

/// Edge strain between two position sets: unique-edge count, edges over the
/// threshold, largest and mean strain (`AnalyzeStrain`,
/// `MeshAnalysis.cpp:711-747`).
fn strain_stats(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    faces: &[[u32; 3]],
    threshold: f32,
) -> (usize, usize, f32, f32) {
    let mut edges = BTreeSet::new();
    for face in faces {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            if a != b {
                edges.insert((a.min(b), a.max(b)));
            }
        }
    }
    let mut total = 0usize;
    let mut stretched = 0usize;
    let mut sum = 0.0f64;
    let mut max_strain = 0.0f32;
    for (a, b) in edges {
        let (a, b) = (a as usize, b as usize);
        let before_length = length(sub(before[a], before[b]));
        let after_length = length(sub(after[a], after[b]));
        if before_length < 1e-5 || after_length < 1e-5 {
            continue;
        }
        let strain = (after_length / before_length).max(before_length / after_length) - 1.0;
        total += 1;
        sum += f64::from(strain);
        if strain > threshold {
            stretched += 1;
        }
        max_strain = max_strain.max(strain);
    }
    let mean = if total > 0 {
        (sum / total as f64) as f32
    } else {
        0.0
    };
    (total, stretched, max_strain, mean)
}

/// Outfit Studio's `CleanPull`: a pull must run roughly along the normal
/// unless the point is already on the surface (`SurfaceFit.cpp:365-371`).
fn clean_pull(point: [f32; 3], closest: [f32; 3], normal: [f32; 3]) -> bool {
    let line = sub(point, closest);
    let distance = length(line);
    if distance < ON_SURFACE {
        return true;
    }
    dot(line, normal).abs() / distance >= MIN_LINE_AGREEMENT
}

/// Distance falloff of a pull: `(1 - t^2)^2` (`Falloff`, `SurfaceFit.cpp:65-69`).
fn falloff(distance: f32, max_distance: f32) -> f32 {
    let t = (distance / max_distance).clamp(0.0, 1.0);
    let f = 1.0 - t * t;
    f * f
}

/// The point at barycentric weights over three unknowns.
fn barycentric_point(values: &[[f32; 3]], unknowns: [usize; 3], bary: [f32; 3]) -> [f32; 3] {
    add(
        add(
            scale(values[unknowns[0]], bary[0]),
            scale(values[unknowns[1]], bary[1]),
        ),
        scale(values[unknowns[2]], bary[2]),
    )
}

fn validate_geometry(garment: Mesh<'_>, body: Mesh<'_>) -> Result<(), FitError> {
    if garment.0.is_empty() {
        return Err(FitError::EmptyGarment);
    }
    if body.0.is_empty() {
        return Err(FitError::EmptyBody);
    }
    if body.1.is_empty() {
        return Err(FitError::EmptyBodyFaces);
    }
    if let Some(vertex) = first_non_finite(garment.0) {
        return Err(FitError::NonFiniteGarmentPosition { vertex });
    }
    if let Some(vertex) = first_non_finite(body.0) {
        return Err(FitError::NonFiniteBodyPosition { vertex });
    }
    validate_faces(garment.0, garment.1, |triangle, vertex| {
        FitError::GarmentFaceIndex { triangle, vertex }
    })?;
    validate_faces(body.0, body.1, |triangle, vertex| FitError::BodyFaceIndex {
        triangle,
        vertex,
    })?;
    Ok(())
}

fn validate_faces(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
    error: fn(triangle: usize, vertex: u32) -> FitError,
) -> Result<(), FitError> {
    for (triangle, face) in faces.iter().enumerate() {
        for &vertex in face {
            if vertex as usize >= positions.len() {
                return Err(error(triangle, vertex));
            }
        }
    }
    Ok(())
}

fn validate_lengths(
    vertices: usize,
    lock: Option<&VertexMask>,
    options: &FitOptions,
    measured: bool,
) -> Result<(), FitError> {
    if let Some(mask) = lock
        && mask.len() != vertices
    {
        return Err(FitError::LockLength {
            expected: vertices,
            actual: mask.len(),
        });
    }
    if !options.target_gaps.is_empty() && !measured {
        if options.target_gaps.len() != vertices {
            return Err(FitError::TargetGapLength {
                expected: vertices,
                actual: options.target_gaps.len(),
            });
        }
        for (vertex, value) in options.target_gaps.iter().enumerate() {
            if !value.is_finite() {
                return Err(FitError::TargetGapValue {
                    vertex,
                    value: *value,
                });
            }
        }
    }
    if !options.rigid_parts.is_empty() && options.rigid_parts.len() != vertices {
        return Err(FitError::RigidPartLength {
            expected: vertices,
            actual: options.rigid_parts.len(),
        });
    }
    for (vertex, part) in options.rigid_parts.iter().enumerate() {
        if *part < -1 {
            return Err(FitError::RigidPartId {
                vertex,
                part: *part,
            });
        }
    }
    Ok(())
}

fn dot_all(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            f64::from(x[0]) * f64::from(y[0])
                + f64::from(x[1]) * f64::from(y[1])
                + f64::from(x[2]) * f64::from(y[2])
        })
        .sum()
}
