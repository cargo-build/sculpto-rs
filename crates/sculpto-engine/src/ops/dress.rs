//! Dress a raw garment onto the body with the contact solve.

use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde_json::{Value, json};

use sculpto_mesh::accumulated_surface::{AccumulatedError, AccumulatedSurface, BODY_SURFACE};
use sculpto_mesh::dress::{
    DressError, DressParams as SolveParams, DressProjections, MAX_CUTS, SeamWeldReport, dress,
    dress_against,
};
use sculpto_mesh::layer_stack::{LayerStack, StackError, resolve_body};
use sculpto_mesh::wrap::repair::final_push::{FinalPushOptions, FinalPushReport};

use crate::obj::{ObjError, display_to_source, read_obj_mesh, write_obj};
use crate::{CommandError, ErrorKind, errors};

/// Everything `outfit.dress` needs.
///
/// The command is the product surface over [`sculpto_mesh::dress`]: a raw
/// garment that does not share the body's topology is registered, subdivided
/// and resolved against its own nearest body point, then written as the OBJ a
/// dresser loads. Absent numerical parameters keep the module's documented
/// defaults; the two that decide acceptance — the declared clearance and the
/// contact standoff — are reported back with the result. @req DECL0013
#[derive(Debug, Clone, JsonSchema)]
pub struct DressParams {
    /// Body OBJ the garment is dressed against.
    pub body: PathBuf,
    /// Raw garment OBJ; its authored topology is not required to match the body.
    pub garment: PathBuf,
    /// Layer-stack manifest (`garment stack`'s shape): the garment is dressed
    /// against the accumulated surface of the body plus these already-fitted
    /// layers, outermost-member rule, instead of the body alone. Absent keeps
    /// the single-body behaviour.
    pub against: Option<PathBuf>,
    /// Output OBJ: the densified, dressed mesh.
    pub out: PathBuf,
    /// Declared standoff in input units, the largest accepted floating gap.
    pub clearance: Option<f32>,
    /// Smallest standoff kept; must be below `clearance`.
    pub contact: Option<f32>,
    /// Midpoint subdivision rounds before the contact solve, `0..=2`.
    pub cuts: Option<u32>,
    /// Contact iterations, `1..=32`.
    pub iterations: Option<u32>,
    /// Hard per-vertex displacement bound of one iteration, in input units.
    pub max_step: Option<f32>,
    /// Displacement-field relaxation weight per iteration, `0..=1`.
    pub relax: Option<f32>,
    /// Registration rounds interleaved with the contact solve, `0..=3`; `0`
    /// runs the contact solve only.
    pub rounds: Option<u32>,
    /// Body-poke repair rounds per contact pass; `0` leaves pokes alone.
    pub poke_passes: Option<u32>,
    /// Accepted visible depth for the bounded final push-out stage; absent
    /// runs no final push.
    pub tolerance: Option<f32>,
    /// Passes the final push-out stage may run; requires `tolerance`.
    pub passes: Option<u32>,
    /// Optional report path: the same JSON object the command returns.
    pub report: Option<PathBuf>,
}

/// One dressed garment and what the solve measured.
#[derive(Debug, Clone, PartialEq)]
pub struct DressOutcome {
    /// Written OBJ.
    pub out: PathBuf,
    /// Body the garment was dressed against.
    pub body: PathBuf,
    /// Garment that was dressed.
    pub garment: PathBuf,
    /// The layer manifest the obstacle was accumulated from, present only for
    /// a layered dress (`against`).
    pub against: Option<DressAgainst>,
    /// Solve parameters actually used.
    pub solve: SolveParams,
    /// Registration report, absent when registration was off.
    pub registration: Option<sculpto_mesh::wrap::FitReport>,
    /// Final push-out report, absent when the stage did not run.
    pub final_push: Option<FinalPushReport>,
    /// The seam merge: what the authored pieces became before subdivision.
    pub seam_weld: SeamWeldReport,
    /// The mesh and its band measurements.
    pub mesh: DressMeasurements,
}

/// The accumulated obstacle one layered dress used. @req DECL0013 DECL0015
#[derive(Debug, Clone, PartialEq)]
pub struct DressAgainst {
    /// Manifest the layers were read from.
    pub manifest: PathBuf,
    /// Which member the returned vertices resolved against.
    pub projections: DressProjections,
}

/// The report of one dress, flattened for the JSON envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DressMeasurements {
    /// Garment vertices given.
    pub source_vertices: usize,
    /// Garment triangles given.
    pub source_triangles: usize,
    /// Vertices written (after subdivision).
    pub vertices: usize,
    /// Triangles written (after subdivision).
    pub triangles: usize,
    /// Contact iterations executed.
    pub iterations: u32,
    /// The residual band was reached before the budget ended.
    pub converged: bool,
    /// Vertices the contact solve moved.
    pub moved: usize,
    /// Largest single displacement.
    pub max_move: f32,
    /// Vertices floating beyond the clearance before the solve.
    pub floating_before: usize,
    /// Largest floating gap before the solve.
    pub max_floating_gap_before: f32,
    /// Largest floating gap written.
    pub max_floating_gap: f32,
    /// Vertices penetrating the body before the solve.
    pub penetrating_before: usize,
    /// Deepest penetration before the solve.
    pub max_penetration_before: f32,
    /// Deepest penetration written.
    pub max_penetration: f32,
    /// Body vertices poking through the garment before the solve.
    pub pokes_before: usize,
    /// Body vertices poking through the written garment.
    pub pokes_after: usize,
    /// Triangles whose facing turned past 90 degrees.
    pub flipped: usize,
}

impl DressOutcome {
    /// The JSON body `outfit.dress` returns and writes to `report`.
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "out": self.out.display().to_string(),
            "body": self.body.display().to_string(),
            "garment": self.garment.display().to_string(),
            "clearance": self.solve.clearance,
            "contact": self.solve.contact,
            "cuts": self.solve.cuts,
            "rounds": self.solve.rounds,
            "poke_passes": self.solve.poke_passes,
            "max_step": self.solve.max_step,
            "relax": self.solve.relax,
            "iterations": self.mesh.iterations,
            "converged": self.mesh.converged,
            "source_vertices": self.mesh.source_vertices,
            "source_triangles": self.mesh.source_triangles,
            "vertices": self.mesh.vertices,
            "triangles": self.mesh.triangles,
            "moved": self.mesh.moved,
            "max_move": self.mesh.max_move,
            "floating_before": self.mesh.floating_before,
            "max_floating_gap_before": self.mesh.max_floating_gap_before,
            "max_floating_gap": self.mesh.max_floating_gap,
            "penetrating_before": self.mesh.penetrating_before,
            "max_penetration_before": self.mesh.max_penetration_before,
            "max_penetration": self.mesh.max_penetration,
            "pokes_before": self.mesh.pokes_before,
            "pokes_after": self.mesh.pokes_after,
            "flipped": self.mesh.flipped,
            "seam_weld": {
                "tolerance": self.seam_weld.tolerance,
                "welded_vertices": self.seam_weld.welded_vertices,
                "merged_vertices": self.seam_weld.merged_vertices,
                "merged_pairs": self.seam_weld.merged_pairs,
                "groups": self.seam_weld.groups,
                "vertex_map": self.seam_weld.vertex_map,
                "components_before": self.seam_weld.components_before,
                "components_after": self.seam_weld.components_after,
                "boundary_edges_before": self.seam_weld.boundary_edges_before,
                "boundary_edges_after": self.seam_weld.boundary_edges_after,
                "dropped_faces": self.seam_weld.dropped_faces,
            },
        });
        if let Some(registration) = self.registration {
            value["registration"] = json!({
                "vertices": registration.vertices,
                "unknowns": registration.unknowns,
                "pulled": registration.pulled,
                "followed": registration.followed,
                "kept_form": registration.kept_form,
                "mean_gap_error": registration.mean_gap_error,
                "max_gap_error": registration.max_gap_error,
                "inside_before": registration.inside_before,
                "inside_after": registration.inside_after,
                "poke_before": registration.poke_before,
                "poke_after": registration.poke_after,
                "flipped": registration.flipped,
                "mean_move": registration.mean_move,
                "max_move": registration.max_move,
                "max_strain": registration.max_strain,
                "mean_strain": registration.mean_strain,
                "strained_edges": registration.strained_edges,
            });
        }
        if let Some(against) = &self.against {
            let members: Vec<Value> = against
                .projections
                .members
                .iter()
                .map(|member| {
                    json!({
                        "id": member.id,
                        "order": member.order,
                        "vertices": member.vertices,
                    })
                })
                .collect();
            value["against"] = json!({
                "manifest": against.manifest.display().to_string(),
                "surface": against.projections.surface,
                "members": members,
                "projections": against.projections.vertex_members,
            });
        }
        if let Some(push) = &self.final_push {
            value["final_push"] = json!(push);
        }
        value
    }
}

/// Dress one garment and write it.
///
/// # Errors
///
/// Refuses unreadable or malformed OBJs as their own classified failures, and
/// every [`DressError`] with its domain code; the fitted OBJ is written before
/// the band verdict, so a refusal still leaves the measured geometry to review.
/// A declared `against` manifest adds its own typed refusals
/// ([`accumulated_obstacle`]).
pub fn dress_garment(params: &DressParams) -> Result<DressOutcome, CommandError> {
    let body =
        read_obj_mesh(&params.body).map_err(|error| obj_error("body", &params.body, &error))?;
    let garment = read_obj_mesh(&params.garment)
        .map_err(|error| obj_error("garment", &params.garment, &error))?;
    let solve = solve_params(params)?;
    let (positions, triangles, report) = match &params.against {
        None => dress((&garment.0, &garment.1), (&body.0, &body.1), &solve)
            .map_err(|error| solve_refused(&error))?,
        Some(manifest) => {
            let accumulated = accumulated_obstacle(manifest, &params.body, &body)?;
            dress_against((&garment.0, &garment.1), &accumulated, &solve)
                .map_err(|error| solve_refused(&error))?
        }
    };
    if let Some(parent) = params.out.parent() {
        fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    write_obj(
        &params.out,
        &positions
            .iter()
            .map(|&point| display_to_source(point))
            .collect::<Vec<_>>(),
        &triangles,
    )
    .map_err(|error| errors::io(&params.out, error))?;
    let outcome = DressOutcome {
        out: params.out.clone(),
        body: params.body.clone(),
        garment: params.garment.clone(),
        against: params
            .against
            .as_deref()
            .zip(report.projections.clone())
            .map(|(manifest, projections)| DressAgainst {
                manifest: manifest.to_path_buf(),
                projections,
            }),
        solve,
        registration: report.registration,
        final_push: report.final_push,
        seam_weld: report.seam_weld,
        mesh: DressMeasurements {
            source_vertices: report.source_vertices,
            source_triangles: report.source_triangles,
            vertices: report.vertices,
            triangles: report.triangles,
            iterations: report.iterations,
            converged: report.converged,
            moved: report.moved,
            max_move: report.max_move,
            floating_before: report.floating_before,
            max_floating_gap_before: report.max_floating_gap_before,
            max_floating_gap: report.max_floating_gap,
            penetrating_before: report.penetrating_before,
            max_penetration_before: report.max_penetration_before,
            max_penetration: report.max_penetration,
            pokes_before: report.pokes_before,
            pokes_after: report.pokes_after,
            flipped: report.flipped,
        },
    };
    if let Some(path) = &params.report {
        let text = serde_json::to_string_pretty(&outcome.to_json())
            .expect("the dress report is a JSON value");
        fs::write(path, text).map_err(|error| errors::io(path, error))?;
    }
    let missed = band_miss(&outcome);
    if let Some(message) = missed {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "dress_band_missed",
            message,
        ));
    }
    Ok(outcome)
}

/// The accumulated obstacle a layered dress grades against: the body plus the
/// manifest's already-fitted layers, in the dressing order the manifest
/// declares. @req DECL0013 DECL0015
///
/// The manifest lists only layers below the garment being dressed, so every
/// declared order must sit strictly above the previous one — an inversion
/// would dress the new garment under a layer it must clear.
///
/// # Errors
///
/// Typed refusals, never a silent body fallback: an unreadable or invalid
/// manifest or a body the manifest disagrees with
/// (`dress_against_manifest_invalid`), a body that cannot serve as an obstacle
/// (`dress_against_body_underivable`), a missing or malformed layer OBJ
/// (`dress_against_layer_missing`, `dress_against_layer_malformed`), a layer
/// that cannot join the composite (`dress_against_layer_underivable`), and a
/// layer whose order is not below the next outer one's
/// (`dress_against_order_not_below`).
fn accumulated_obstacle(
    manifest: &Path,
    body_path: &Path,
    body: &crate::obj::ObjMesh,
) -> Result<AccumulatedSurface, CommandError> {
    let text = fs::read_to_string(manifest).map_err(|error| errors::io(manifest, error))?;
    let stack = LayerStack::parse_json(&text, manifest)
        .map_err(|error| dress_against_manifest_invalid(&error))?;
    // The manifest body, when declared, must be the body being dressed over.
    resolve_body(&stack, Some(body_path))
        .map_err(|error| dress_against_manifest_invalid(&error))?;
    let mut accumulated = AccumulatedSurface::from_body(&body.0, &body.1)
        .map_err(|error| dress_against_underivable(BODY_SURFACE, &error))?;
    for layer in &stack.layers {
        let mesh = read_obj_mesh(&layer.obj)
            .map_err(|error| dress_against_obj(&layer.id, &layer.obj, &error))?;
        accumulated
            .push_layer(&layer.id, layer.order, &mesh.0, &mesh.1)
            .map_err(|error| dress_against_push(&layer.id, layer.order, &error))?;
    }
    Ok(accumulated)
}

fn dress_against_manifest_invalid(error: &StackError) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "dress_against_manifest_invalid",
        error.to_string(),
    )
}

fn dress_against_underivable(surface: &str, error: &AccumulatedError) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        if surface == BODY_SURFACE {
            "dress_against_body_underivable"
        } else {
            "dress_against_layer_underivable"
        },
        format!("cannot derive the accumulated surface from `{surface}`: {error}"),
    )
}

/// Split a declared layer's OBJ read failure by its own contract.
fn dress_against_obj(id: &str, path: &Path, error: &ObjError) -> CommandError {
    match error {
        ObjError::Io(message) => CommandError::new(
            ErrorKind::NotFound,
            "dress_against_layer_missing",
            format!("layer `{id}`: {}: {message}", path.display()),
        ),
        ObjError::Malformed(message) => CommandError::new(
            ErrorKind::InvalidRequest,
            "dress_against_layer_malformed",
            format!("layer `{id}`: {}: {message}", path.display()),
        ),
        ObjError::MissingObject { name, .. } => CommandError::new(
            ErrorKind::NotFound,
            "dress_against_layer_missing",
            format!(
                "layer `{id}`: {}: object `{name}` not found",
                path.display()
            ),
        ),
    }
}

fn dress_against_push(id: &str, order: u32, error: &AccumulatedError) -> CommandError {
    let code = match error {
        AccumulatedError::OrderNotIncreasing { .. } => "dress_against_order_not_below",
        _ => "dress_against_layer_underivable",
    };
    CommandError::new(
        ErrorKind::Refused,
        code,
        format!("layer `{id}` (order {order}) cannot join the accumulated surface: {error}"),
    )
}

/// The band promise the solve makes, as a refusal message: the returned mesh
/// keeps the declared clearance and the contact standoff. A caller that wants
/// the artifact without the promise reads the report instead.
fn band_miss(outcome: &DressOutcome) -> Option<String> {
    let mesh = &outcome.mesh;
    let gap = mesh.max_floating_gap > outcome.solve.clearance;
    let deep = mesh.max_penetration > outcome.solve.contact;
    match gap || deep {
        false => None,
        true => Some(format!(
            "the dressed garment misses its declared band: floating gap {:.6} (declared {:.6}), penetration {:.6} (contact {:.6}) after {} iteration(s); {} of {} vertices moved (max {:.6}), body pokes {} -> {}",
            mesh.max_floating_gap,
            outcome.solve.clearance,
            mesh.max_penetration,
            outcome.solve.contact,
            mesh.iterations,
            mesh.moved,
            mesh.vertices,
            mesh.max_move,
            mesh.pokes_before,
            mesh.pokes_after,
        )),
    }
}

/// Turn the declared options into solve parameters, refusing values the solve
/// would silently replace with its own bound.
fn solve_params(params: &DressParams) -> Result<SolveParams, CommandError> {
    let defaults = SolveParams::default();
    let cuts = params.cuts.unwrap_or(defaults.cuts);
    if cuts > MAX_CUTS {
        return Err(errors::invalid(
            "cuts",
            format!("must be 0..={MAX_CUTS}, got {cuts}"),
        ));
    }
    let final_push = match (params.tolerance, params.passes) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(errors::invalid(
                "passes",
                "needs the accepted visible depth in `tolerance`",
            ));
        }
        (Some(tolerance), passes) => Some(FinalPushOptions {
            passes: passes.unwrap_or(4),
            tolerance,
        }),
    };
    Ok(SolveParams {
        clearance: params.clearance.unwrap_or(defaults.clearance),
        contact: params.contact.unwrap_or(defaults.contact),
        cuts,
        iterations: params.iterations.unwrap_or(defaults.iterations),
        max_step: params.max_step.unwrap_or(defaults.max_step),
        relax: params.relax.unwrap_or(defaults.relax),
        rounds: params.rounds.unwrap_or(defaults.rounds),
        poke_passes: params.poke_passes.unwrap_or(defaults.poke_passes),
        final_push,
    })
}

/// The domain refusal, classified by the variant that produced it.
fn solve_refused(error: &DressError) -> CommandError {
    let code = match error {
        DressError::EmptyGarment
        | DressError::EmptyBody
        | DressError::EmptyBodyFaces
        | DressError::NonFiniteGarmentPosition { .. }
        | DressError::NonFiniteBodyPosition { .. }
        | DressError::GarmentFaceIndex { .. }
        | DressError::BodyFaceIndex { .. } => "dress_input",
        DressError::NonFiniteResult { .. } => "dress_non_finite",
        DressError::Clearance(_)
        | DressError::Contact { .. }
        | DressError::Cuts(_)
        | DressError::Iterations(_)
        | DressError::MaxStep(_)
        | DressError::Relax(_)
        | DressError::Rounds(_)
        | DressError::PokePasses(_) => "dress_parameter",
        DressError::Registration(_) => "dress_registration",
        DressError::FinalPush(_) => "dress_final_push",
    };
    let kind = match error {
        DressError::Clearance(_)
        | DressError::Contact { .. }
        | DressError::Cuts(_)
        | DressError::Iterations(_)
        | DressError::MaxStep(_)
        | DressError::Relax(_)
        | DressError::Rounds(_)
        | DressError::PokePasses(_) => ErrorKind::InvalidParams,
        DressError::NonFiniteResult { .. } => ErrorKind::Unsupported,
        _ => ErrorKind::Refused,
    };
    CommandError::new(kind, code, error.to_string())
}

pub(crate) fn obj_error(field: &str, path: &Path, error: &ObjError) -> CommandError {
    match error {
        ObjError::Io(message) => CommandError::new(
            ErrorKind::Io,
            "obj_unreadable",
            format!("{field} `{}`: {message}", path.display()),
        ),
        ObjError::Malformed(message) => CommandError::new(
            ErrorKind::InvalidParams,
            "obj_malformed",
            format!("{field} `{}`: {message}", path.display()),
        ),
        ObjError::MissingObject { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "obj_missing_object",
            format!("{field} `{}`: {error}", path.display()),
        ),
    }
}
