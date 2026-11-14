//! Seal the open joint rims of an assembled character: bridge the paired
//! rings, merge coincident copies and smooth the seam. The mesh owner lives in
//! [`sculpto_mesh::joint_seal`]; this module is the engine surface.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_mesh::joint_seal::{self, JointReport, JointRequest, SealOptions, SealOutcome};

use crate::obj::{read_obj_surface, write_obj};
use crate::{CommandError, ErrorKind};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Joint {
    /// Part shell name, as `body.audit_parts` reports it.
    part: String,
    /// Socket shell name.
    socket: String,
    /// Seat the part onto the socket first (the hands path); the neck never seats.
    seat: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    /// Multi-part body OBJ: the torso plus its head/hand shells.
    body: PathBuf,
    /// Output OBJ the sealed character is written to.
    out: PathBuf,
    /// Declared joints to seal, in order.
    joints: Vec<Joint>,
    /// Smoothing passes (default 2).
    iterations: Option<u32>,
    /// Ring distance that may move (default 1).
    band: Option<u32>,
    /// Hard displacement bound per vertex, in input units.
    max_shift: Option<f32>,
    /// Candidate-rim proximity band; derived from the socket ring when absent.
    arc_band: Option<f64>,
    /// Explicit vertical seat of the part shell along the up axis, applied and
    /// reported before pairing (default 0: nothing is inferred).
    head_drop: Option<f32>,
    /// Smooth the seam (default true).
    smooth: Option<bool>,
    /// Merge exactly coincident boundary copies beyond the pinch (default false).
    weld_coincident: Option<bool>,
}

pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(Params))
}

pub(crate) fn seal(input: &Value) -> Result<Value, CommandError> {
    let params: Params = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(ErrorKind::InvalidParams, "seal_input", error.to_string())
    })?;
    let (positions, triangles) = read_obj_surface(&params.body)?;
    let requests: Vec<JointRequest> = params
        .joints
        .iter()
        .map(|joint| JointRequest {
            part: joint.part.clone(),
            socket: joint.socket.clone(),
            seat: joint.seat.unwrap_or(false),
        })
        .collect();
    let defaults = SealOptions::default();
    let options = SealOptions {
        iterations: params.iterations.unwrap_or(defaults.iterations),
        band: params.band.unwrap_or(defaults.band),
        max_shift: params.max_shift,
        arc_band: params.arc_band,
        head_drop: params.head_drop.unwrap_or(defaults.head_drop),
        smooth: params.smooth.unwrap_or(defaults.smooth),
        weld_coincident: params.weld_coincident.unwrap_or(defaults.weld_coincident),
    };
    let outcome =
        joint_seal::seal_joints(&positions, &triangles, &requests, &options).map_err(seal_error)?;
    write_obj(&params.out, &outcome.positions, &outcome.triangles).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "seal_out",
            format!("{}: {error}", params.out.display()),
        )
    })?;
    Ok(render(&params.out, &outcome))
}

fn seal_error(error: joint_seal::SealError) -> CommandError {
    let kind = if error.parameter_problem() {
        ErrorKind::InvalidParams
    } else {
        ErrorKind::Refused
    };
    CommandError::new(kind, error.code(), error.to_string())
}

fn ring(value: &joint_seal::RingFacts) -> Value {
    json!({
        "vertices": value.vertices,
        "centre": value.centre,
        "radius_min": value.radius_min,
        "radius_mean": value.radius_mean,
        "radius_max": value.radius_max,
        "planarity": value.planarity,
        "closed": value.closed,
    })
}

fn smoothing_row(smoothing: &joint_seal::SmoothingReport) -> Value {
    json!({
        "enabled": smoothing.enabled,
        "iterations": smoothing.iterations,
        "factor": smoothing.factor,
        "band": smoothing.band,
        "max_shift": smoothing.max_shift,
        "max_displacement": smoothing.max_displacement,
        "changed_vertices": smoothing.changed_vertices,
        "changed_outside_band": smoothing.changed_outside_band,
        "max_dihedral_before_deg": smoothing.max_dihedral_before_deg,
        "max_dihedral_after_deg": smoothing.max_dihedral_after_deg,
        "flipped_triangles": smoothing.flipped_triangles,
    })
}

fn seat_row(seat: &joint_seal::SeatReport) -> Value {
    json!({
        "head_drop": seat.head_drop,
        "scale": seat.scale,
        "rotation": seat.rotation,
        "translation": seat.translation,
        "gap": seat.gap,
        "rms": seat.rms,
        "max_residual": seat.max_residual,
        "plane_dev": seat.plane_dev,
    })
}

fn joint_row(joint: &JointReport) -> Value {
    json!({
        "joint": joint.joint,
        "part": joint.part_shell,
        "socket": joint.socket_shell,
        "rings": [ring(&joint.rings[0]), ring(&joint.rings[1])],
        "selected_collar": joint.selected_collar,
        "seam_choice": joint.seam_choice,
        "pinch_edges": joint.pinch_edges,
        "gap_min": joint.gap_min,
        "gap_mean": joint.gap_mean,
        "gap_max": joint.gap_max,
        "arc_band_used": joint.arc_band_used,
        "correspondence": joint.correspondence,
        "bridge_triangles": joint.bridge_triangles,
        "edges_consumed": joint.edges_consumed,
        "boundary_ring_before": joint.boundary_ring_before,
        "boundary_ring_after": joint.boundary_ring_after,
        "boundary_arc_before": joint.boundary_arc_before,
        "boundary_arc_after": joint.boundary_arc_after,
        "boundary_joint_shells_before": joint.boundary_joint_shells_before,
        "boundary_joint_shells_after": joint.boundary_joint_shells_after,
        "boundary_mesh_before": joint.boundary_mesh_before,
        "boundary_mesh_after": joint.boundary_mesh_after,
        "non_manifold_before": joint.non_manifold_before,
        "non_manifold_after": joint.non_manifold_after,
        "winding_conflicts": joint.winding_conflicts,
        "min_bridge_area": joint.min_bridge_area,
        "max_aspect": joint.max_aspect,
        "merged_duplicates": joint.merged_duplicates,
        "handedness": joint.handedness,
        "seat": joint.seat.as_ref().map(seat_row),
        "smoothing": smoothing_row(&joint.smoothing),
    })
}

fn render(out: &std::path::Path, outcome: &SealOutcome) -> Value {
    let report = &outcome.report;
    json!({
        "out": out,
        "joints": report.joints.iter().map(joint_row).collect::<Vec<_>>(),
        "components_before": report.components_before,
        "components_after": report.components_after,
        "input_digest": report.input_digest,
        "output_digest": report.output_digest,
        "vertices": outcome.positions.len(),
        "triangles": outcome.triangles.len(),
        "units": "input",
        "uv": "absent",
        "material": "not_carried",
    })
}
