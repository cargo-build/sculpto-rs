//! Stack-aware fit: dress the body layer by layer, obstacle carried forward.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde_json::{Value, json};

use sculpto_mesh::accumulated_surface::{AccumulatedError, AccumulatedSurface, BODY_SURFACE};
use sculpto_mesh::layer_stack::{self, LayerStack, StackError, StackReport, resolve_body};
use sculpto_mesh::wrap::FitOptions;
use sculpto_mesh::wrap::repair::final_push::{
    FinalPushOptions, FinalPushReport, final_push_layered,
};

use crate::obj::{read_obj_mesh, write_obj};
use crate::{CommandError, ErrorKind, errors};

/// Default pass budget per layer when the caller does not name one.
const DEFAULT_PASSES: u32 = 4;

/// Everything `outfit.stack_fit` needs.
///
/// The command is the product surface over the chained-push precedent: it takes
/// the same manifest `garment stack` grades (`body`, ranked `layers`, declared
/// `pairs`, anchors, boot mode), fits each layer against the accumulated
/// surface of the body plus the already-fitted layers below it, writes every
/// fitted OBJ, and reports per-layer obstacle provenance plus the graded pair
/// rows. A layer that could be graded but not moved to tolerance is reported
/// by the stack verdict, never silently passed. @req DECL0013 DECL0015
#[derive(Debug, Clone, JsonSchema)]
pub struct StackFitParams {
    /// Layer manifest: `body`, ranked `layers` (each may declare `out`), and
    /// the same `pairs`/`anchor`/`mode` declarations `garment stack` reads.
    pub manifest: PathBuf,
    /// Accepted visible depth for every graded pair and push, in input units.
    pub tolerance: f32,
    /// Body/collision OBJ override; must agree with a declared manifest body.
    pub body: Option<PathBuf>,
    /// Pass budget for each layer's final push-out, `1..=10`. Defaults to 4.
    pub passes: Option<u32>,
}

/// Which surface one step graded and pushed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObstacleProvenance {
    /// Provenance label, for example `accumulated[body, robe]`.
    pub surface: String,
    /// Member surfaces in dressing order, the body first.
    pub layers: Vec<String>,
    /// Composite vertices.
    pub vertices: usize,
    /// Composite triangles.
    pub triangles: usize,
    /// FNV-1a 64 over the composite geometry, for step-to-step identity.
    pub digest: String,
}

/// One executed layer step: the obstacle it used, its push record and the
/// contact breakdown that proves which member the graded set touched.
#[derive(Debug, Clone)]
pub struct StackFitStep {
    /// Declared layer id.
    pub layer: String,
    /// Declared order.
    pub order: u32,
    /// Source OBJ the step read.
    pub source: PathBuf,
    /// Fitted OBJ the step wrote.
    pub out: PathBuf,
    /// Vertices of the layer.
    pub vertices: usize,
    /// Triangles of the layer.
    pub triangles: usize,
    /// The composite obstacle this step graded and pushed against.
    pub obstacle: ObstacleProvenance,
    /// The push-out record over that obstacle.
    pub push: FinalPushReport,
    /// Graded source vertices per obstacle member, zeros kept.
    pub contact_breakdown: Vec<(String, usize)>,
}

/// What `outfit.stack_fit` produced.
#[derive(Debug, Clone)]
pub struct StackFitOutcome {
    /// Manifest the stack was read from.
    pub manifest: PathBuf,
    /// Body/collision OBJ every layer dresses over.
    pub body: PathBuf,
    /// Accepted visible depth.
    pub tolerance: f32,
    /// Pass budget each layer had.
    pub passes: u32,
    /// One step per layer, dressing order.
    pub steps: Vec<StackFitStep>,
    /// The fitted stack graded by `garment stack`'s rule.
    pub stack: StackReport,
}

/// Fit an ordered layer stack against the accumulated obstacle surface.
///
/// Layer 1 pushes against the body; layer *k* pushes against the composite of
/// the body and the fitted layers `1..k-1`, never the nude body silently —
/// every step's report names the surface it used and the members its graded
/// set touched. After the last layer the fitted OBJs are graded by
/// [`layer_stack::grade`], so the pair rows cannot drift from `garment stack`.
///
/// # Errors
/// Typed refusals, never a silent body fallback: an unreadable or invalid
/// manifest (`stack_fit_manifest_invalid`), a missing body
/// (`stack_fit_body_missing`) or layer (`stack_fit_layer_missing`), a body or
/// layer that cannot serve as an obstacle (`stack_fit_body_underivable`,
/// `stack_fit_layer_underivable`), a layer whose push was refused
/// (`stack_fit_layer_push`) or whose remaining violations cannot move
/// (`stack_fit_layer_unfittable`), and a post-fit grading refusal
/// (`stack_fit_grade`). Files already written when a later step refuses stay
/// written; each step's report row names what it produced.
pub fn stack_fit(params: &StackFitParams) -> Result<StackFitOutcome, CommandError> {
    let text = std::fs::read_to_string(&params.manifest)
        .map_err(|error| errors::io(&params.manifest, error))?;
    let stack =
        LayerStack::parse_json(&text, &params.manifest).map_err(|error| manifest_error(&error))?;
    let body_path =
        resolve_body(&stack, params.body.as_deref()).map_err(|error| manifest_error(&error))?;
    if !params.tolerance.is_finite() || params.tolerance < 0.0 {
        return Err(errors::invalid(
            "tolerance",
            format!(
                "must be a finite non-negative number, got `{}`",
                params.tolerance
            ),
        ));
    }
    let passes = match params.passes {
        None => DEFAULT_PASSES,
        // The bound is the fitter's `MAX_FINAL_PUSH_PASSES`, crate-private in
        // `sculpto-mesh`; the writer below refuses drift by name too.
        Some(passes) if (1..=10).contains(&passes) => passes,
        Some(passes) => {
            return Err(errors::invalid(
                "passes",
                format!("each layer's pass budget must be 1..=10, got {passes}"),
            ));
        }
    };

    let body = read_obj_mesh(&body_path).map_err(|error| obj_error("body", &body_path, &error))?;
    let mut accumulated = AccumulatedSurface::from_body(&body.0, &body.1)
        .map_err(|error| underivable(BODY_SURFACE, &error))?;

    let mut order: Vec<usize> = (0..stack.layers.len()).collect();
    order.sort_by_key(|&index| (stack.layers[index].order, index));
    let mut steps = Vec::with_capacity(order.len());
    let mut fitted: BTreeMap<PathBuf, layer_stack::Geometry> = BTreeMap::new();
    fitted.insert(body_path.clone(), body.clone());
    for &index in &order {
        let layer = &stack.layers[index];
        let source =
            read_obj_mesh(&layer.obj).map_err(|error| obj_error("layer", &layer.obj, &error))?;
        // The step's obstacle is the accumulated composite graded by the
        // outermost-member rule, so a deep garment vertex can only grade
        // against the surface it actually crossed.
        let breakdown = accumulated
            .contact_breakdown((&source.0, &source.1))
            .map_err(|error| layer_push(&layer.id, layer.order, &error.to_string()))?;
        let (pushed, push) = final_push_layered(
            (&source.0, &source.1),
            &accumulated,
            None,
            &FitOptions::default(),
            &FinalPushOptions {
                passes,
                tolerance: params.tolerance,
            },
        )
        .map_err(|error| layer_push(&layer.id, layer.order, &error.to_string()))?;
        if push.violations > 0 && push.held == push.violations {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "stack_fit_layer_unfittable",
                format!(
                    "layer `{}` (order {}) still has {} violation(s) at max {:.6} and none can move: every remaining graded vertex is held",
                    layer.id, layer.order, push.violations, push.max
                ),
            ));
        }
        let out = layer.out.clone().unwrap_or_else(|| default_out(&layer.obj));
        let provenance = ObstacleProvenance {
            surface: accumulated.surface_label(),
            layers: accumulated
                .layers()
                .iter()
                .map(|member| member.id.clone())
                .collect(),
            vertices: accumulated.vertices(),
            triangles: accumulated.triangles(),
            digest: digest64(accumulated.obstacle()),
        };
        // The composite accepts the fitted layer before its file exists, so an
        // underivable surface leaves no half-written output behind it.
        accumulated
            .push_layer(&layer.id, layer.order, &pushed, &source.1)
            .map_err(|error| underivable(&layer.id, &error))?;
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        write_obj(&out, &to_source(&pushed), &source.1).map_err(|error| errors::io(&out, error))?;
        steps.push(StackFitStep {
            layer: layer.id.clone(),
            order: layer.order,
            source: layer.obj.clone(),
            out: out.clone(),
            vertices: source.0.len(),
            triangles: source.1.len(),
            obstacle: provenance,
            push,
            contact_breakdown: breakdown,
        });
        fitted.insert(out, (pushed, source.1));
    }

    // Grade the fitted artifacts by `garment stack`'s rule: the graded stack
    // reads each layer from its written output.
    let mut graded_stack = stack.clone();
    for layer in &mut graded_stack.layers {
        layer.obj = layer.out.clone().unwrap_or_else(|| default_out(&layer.obj));
    }
    let loader = |path: &Path| -> Result<layer_stack::Geometry, String> {
        fitted
            .get(path)
            .cloned()
            .ok_or_else(|| format!("missing fitted output {}", path.display()))
    };
    let report = layer_stack::grade(&graded_stack, &body_path, params.tolerance, loader)
        .map_err(|error| grade_refused(&error))?;
    Ok(StackFitOutcome {
        manifest: params.manifest.clone(),
        body: body_path,
        tolerance: params.tolerance,
        passes,
        steps,
        stack: report,
    })
}

/// Where a layer's fitted OBJ lands when the manifest does not name an `out`.
fn default_out(source: &Path) -> PathBuf {
    let stem = source.file_stem().unwrap_or_default();
    let name = format!("{}.stackfit.obj", stem.to_string_lossy());
    match source.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// Inverse of the `(x, z, -y)` display conversion `write_obj` applies, so the
/// file keeps the exact coordinates the step graded.
fn to_source(positions: &[[f32; 3]]) -> Vec<[f32; 3]> {
    positions.iter().map(|[x, y, z]| [*x, -*z, *y]).collect()
}

/// FNV-1a 64 over the composite obstacle geometry: a cheap, deterministic step
/// identity for the report, not a cryptographic claim.
fn digest64(mesh: sculpto_mesh::wrap::Mesh<'_>) -> String {
    let (positions, triangles) = mesh;
    let mut hash: u64 = 0xcbf29ce484222325;
    for position in positions {
        for lane in position {
            for byte in lane.to_le_bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    for triangle in triangles {
        for index in triangle {
            for byte in index.to_le_bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    format!("{hash:016x}")
}

fn manifest_error(error: &StackError) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "stack_fit_manifest_invalid",
        error.to_string(),
    )
}

/// Split OBJ read failures by their own contract: unreadable is I/O, unusable
/// content is a malformed value.
fn obj_error(role: &str, path: &Path, error: &crate::obj::ObjError) -> CommandError {
    let (io_kind, io_code, malformed_code) = if role == "body" {
        (
            ErrorKind::Io,
            "stack_fit_body_missing",
            "stack_fit_body_malformed",
        )
    } else {
        (
            ErrorKind::NotFound,
            "stack_fit_layer_missing",
            "stack_fit_layer_malformed",
        )
    };
    match error {
        crate::obj::ObjError::Io(message) => {
            CommandError::new(io_kind, io_code, format!("{}: {message}", path.display()))
        }
        crate::obj::ObjError::Malformed(message) => CommandError::new(
            ErrorKind::InvalidRequest,
            malformed_code,
            format!("{}: {message}", path.display()),
        ),
        // The whole-file reader never selects a block; the typed arm keeps the
        // match exhaustive.
        crate::obj::ObjError::MissingObject { name, .. } => CommandError::new(
            ErrorKind::NotFound,
            io_code,
            format!("{}: object `{name}` not found", path.display()),
        ),
    }
}

fn underivable(surface: &str, error: &AccumulatedError) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        if surface == BODY_SURFACE {
            "stack_fit_body_underivable"
        } else {
            "stack_fit_layer_underivable"
        },
        format!("cannot derive the accumulated surface from `{surface}`: {error}"),
    )
}

fn layer_push(layer: &str, order: u32, message: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        "stack_fit_layer_push",
        format!("layer `{layer}` (order {order}) cannot be fitted: {message}"),
    )
}

fn grade_refused(error: &StackError) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        "stack_fit_grade",
        format!("the fitted stack cannot be graded: {error}"),
    )
}

fn number(value: f32) -> Value {
    json!(value)
}

impl StackFitOutcome {
    /// The one report shape every surface prints.
    pub fn to_json(&self) -> Value {
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|step| {
                let per_pass: Vec<Value> = step
                    .push
                    .per_pass
                    .iter()
                    .map(|pass| {
                        json!({
                            "pass": pass.pass,
                            "moved": pass.moved,
                            "violations": pass.violations,
                            "max": number(pass.max),
                        })
                    })
                    .collect();
                let breakdown: serde_json::Map<String, Value> = step
                    .contact_breakdown
                    .iter()
                    .map(|(id, count)| (id.clone(), json!(count)))
                    .collect();
                json!({
                    "layer": step.layer,
                    "order": step.order,
                    "source": step.source.display().to_string(),
                    "out": step.out.display().to_string(),
                    "vertices": step.vertices,
                    "triangles": step.triangles,
                    "obstacle": {
                        "surface": step.obstacle.surface,
                        "layers": step.obstacle.layers,
                        "vertices": step.obstacle.vertices,
                        "triangles": step.obstacle.triangles,
                        "digest": step.obstacle.digest,
                    },
                    "fit": {
                        "tolerance": number(step.push.tolerance),
                        "budget": step.push.budget,
                        "passes": step.push.passes,
                        "before_violations": step.push.before_violations,
                        "before_max": number(step.push.before_max),
                        "per_pass": per_pass,
                        "after_violations": step.push.violations,
                        "after_max": number(step.push.max),
                        "held": step.push.held,
                    },
                    "contact_breakdown": Value::Object(breakdown),
                })
            })
            .collect();
        let pairs: Vec<Value> = self.stack.pairs.iter().map(pair_json).collect();
        let layers: Vec<Value> = self
            .stack
            .layers
            .iter()
            .map(|layer| {
                json!({
                    "id": layer.id,
                    "order": layer.order,
                    "classification": layer.classification.id(),
                    "pairs": layer.pairs,
                    "failed": layer.failed,
                    "visible_max": number(layer.visible_max),
                })
            })
            .collect();
        json!({
            "manifest": self.manifest.display().to_string(),
            "body": self.body.display().to_string(),
            "units": "source",
            "digest_kind": "fnv1a64",
            "semantics": "layer k is pushed against the accumulated obstacle of the body plus the fitted layers 1..k-1, graded by the outermost-member rule (a vertex grades against the highest-order member it is behind, so a deep vertex reaches the surface it crossed, never a covered inner one); every step names that surface and counts its graded vertices per obstacle member; the fitted outputs are then graded by the `garment stack` pair rule",
            "tolerance": number(self.tolerance),
            "passes": self.passes,
            "steps": steps,
            "layers": layers,
            "pairs": pairs,
            "pass": self.stack.pass,
            "failures": {
                "penetration": self.stack.penetrations,
                "boot_mode_violation": self.stack.boot_violations,
            },
        })
    }
}

/// One pair row, the fields `garment stack --json` prints.
fn pair_json(pair: &sculpto_mesh::layer_stack::PairGrade) -> Value {
    let worst: Vec<Value> = pair
        .worst
        .iter()
        .map(|&(vertex, depth)| json!({ "vertex": vertex, "depth": number(depth) }))
        .collect();
    json!({
        "outer": pair.outer,
        "inner": pair.inner,
        "kind": pair.kind.id(),
        "classification": pair.classification.id(),
        "tolerance": number(pair.tolerance),
        "vertices": pair.vertices,
        "inside": pair.inside,
        "inner_facing": pair.inner_facing,
        "hidden": pair.hidden,
        "visible": pair.visible,
        "visible_max": number(pair.visible_max),
        "violations": pair.violations,
        "worst": worst,
    })
}
