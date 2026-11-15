//! `actor.stress`: the per-frame dressed clearance table the engine measures
//! itself. @req BODY-01
//!
//! Poses the declared body/garment parts of a rigged actor GLB at evenly
//! spaced clip times and grades every frame with the depth owner
//! ([`sculpto_mesh::check::garment_depth_report`]) through the pure
//! [`sculpto_mesh::actor::stress`] harness. The rows make "fits under a walk"
//! a measured claim instead of a rest-pose assumption: graded visible depth,
//! penetration and intersection counts per frame.
//!
//! The table is evidence, not acceptance: discrete frames never prove
//! between-frame clearance and the graded depth is not containment — the
//! report carries that statement verbatim.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_mesh::actor::{Actor, ActorError, StressError, StressPlan, stress};

use crate::{CommandError, ErrorKind, errors};

/// Default frame count for a clip run.
const DEFAULT_FRAMES: usize = 4;

/// The command input.
///
/// `clip` and `rest` are alternative poses; `frames` counts the evenly spaced
/// clip times (endpoints included) and is ignored for `rest`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorStressParams {
    /// Rigged actor GLB carrying the parts and the clip.
    pub input: PathBuf,
    /// Body part address (`Name` or `Name#i` for primitive `i`).
    pub body: String,
    /// Garment part addresses, each graded against the body every frame.
    pub garments: Vec<String>,
    /// Accepted graded visible depth in the GLB's own units (the accepted
    /// REST number: 0.15 source units = 0.0021 m in the metres GLB frame).
    pub tolerance: f32,
    /// Clip name; omit for the rest pose.
    #[serde(default)]
    pub clip: Option<String>,
    /// Pose the rest instead of a clip (one row; `frames` ignored).
    #[serde(default)]
    pub rest: bool,
    /// Evenly spaced clip frames, endpoints included (default 4).
    #[serde(default)]
    pub frames: Option<usize>,
    /// Also write the report JSON here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// The command input schema.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(ActorStressParams))
}

/// The registry handler: parse, pose, grade, report.
///
/// # Errors
/// Typed refusals for malformed input, missing/ambiguous parts or clips, a
/// clip time outside its span and a posed pair the depth owner refuses.
pub(crate) fn run(input: &Value) -> Result<Value, CommandError> {
    let params: ActorStressParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_input",
            error.to_string(),
        )
    })?;
    if params.clip.is_some() && params.rest {
        return Err(errors::invalid(
            "clip",
            "`clip` and `rest` are alternative poses; declare one",
        ));
    }
    if params.clip.is_none() && !params.rest {
        return Err(errors::invalid("clip", "declare a `clip` or set `rest`"));
    }
    let frames = match params.frames {
        Some(0) => return Err(errors::invalid("frames", "at least one frame is required")),
        Some(frames) => frames,
        None if params.clip.is_some() => DEFAULT_FRAMES,
        None => 1,
    };
    if !params.tolerance.is_finite() || params.tolerance < 0.0 {
        return Err(errors::invalid(
            "tolerance",
            format!("{} is not a finite non-negative depth", params.tolerance),
        ));
    }
    if params.garments.is_empty() {
        return Err(errors::invalid(
            "garments",
            "at least one garment part is required",
        ));
    }

    let bytes = std::fs::read(&params.input).map_err(|error| errors::io(&params.input, error))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| actor_error(&error))?;
    let plan = StressPlan {
        body: &params.body,
        garments: params.garments.iter().map(String::as_str).collect(),
        clip: params.clip.as_deref(),
        frames,
        tolerance: params.tolerance,
    };
    let report = stress(&actor, &plan).map_err(|error| stress_error(&error))?;
    let value = serde_json::to_value(&report).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidRequest,
            "actor_stress_report",
            format!("cannot serialize the stress report: {error}"),
        )
    })?;
    if let Some(path) = &params.report {
        write_report(path, &value)?;
    }
    Ok(value)
}

/// Map an [`ActorError`] onto the stable error contract.
fn actor_error(error: &ActorError) -> CommandError {
    match error {
        ActorError::Selection { matches: 0, .. } => CommandError::new(
            ErrorKind::NotFound,
            "actor_stress_part_missing",
            error.to_string(),
        ),
        ActorError::Selection { .. } | ActorError::DuplicatePart { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_part_ambiguous",
            error.to_string(),
        ),
        ActorError::Clip { matches: 0, .. } => CommandError::new(
            ErrorKind::NotFound,
            "actor_stress_clip_missing",
            error.to_string(),
        ),
        ActorError::Clip { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_clip_ambiguous",
            error.to_string(),
        ),
        ActorError::Time { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_time",
            error.to_string(),
        ),
        ActorError::Decode(_) => CommandError::new(
            ErrorKind::InvalidRequest,
            "actor_stress_glb_invalid",
            error.to_string(),
        ),
        ActorError::Unsupported { .. } => CommandError::new(
            ErrorKind::Unsupported,
            "actor_stress_unsupported",
            error.to_string(),
        ),
        ActorError::Invalid { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_invalid",
            error.to_string(),
        ),
        // The actor enum is `#[non_exhaustive]`; future variants are caller
        // input defects until the source names them.
        _ => CommandError::new(
            ErrorKind::InvalidRequest,
            "actor_stress_actor",
            error.to_string(),
        ),
    }
}

/// Map a [`StressError`] onto the stable error contract.
fn stress_error(error: &StressError) -> CommandError {
    match error {
        StressError::Actor(actor) => actor_error(actor),
        StressError::Fit(_) => CommandError::new(
            ErrorKind::Refused,
            "actor_stress_fit_refused",
            error.to_string(),
        ),
        StressError::Plan { .. } => CommandError::new(
            ErrorKind::InvalidParams,
            "actor_stress_plan",
            error.to_string(),
        ),
        _ => CommandError::new(
            ErrorKind::InvalidRequest,
            "actor_stress_error",
            error.to_string(),
        ),
    }
}

fn write_report(path: &PathBuf, report: &Value) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(report).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "actor_stress_report",
            format!("{}: {error}", path.display()),
        )
    })?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    std::fs::write(path, format!("{text}\n")).map_err(|error| errors::io(path, error))
}
