//! `actor joints`: report every joint's articulation against the schema —
//! type, axis, range, observed value, excess — for a pose or a whole clip,
//! with an optional flesh survey of the stored axes. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::joints::{self, Severity};
use sculpto_mesh::actor::{Actor, Pose};
use serde_json::json;

const USAGE: &str =
    "usage: sculpto actor joints --input actor.glb [--rest | --clip NAME --frames N] \
    [--survey --radius MM] [--report joints.json]
    exit 0: no violations; exit 1: violations found; exit 2: usage/refusal";

#[derive(Debug)]
struct Arguments<'arg> {
    input: &'arg str,
    clip: Option<&'arg str>,
    frames: usize,
    survey: bool,
    radius_mm: f64,
    report: Option<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor joints: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match report(arguments) {
        Ok(violations) => match violations {
            0 => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        },
        Err(error) => {
            eprintln!("actor joints: {error}");
            ExitCode::from(2)
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut survey = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--input" | "--clip" | "--frames" | "--radius" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            "--rest" => {
                flags.insert("--rest", "1");
            }
            "--survey" => survey = true,
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let input = flags
        .get("--input")
        .copied()
        .ok_or_else(|| "--input is required".to_owned())?;
    let clip = flags.get("--clip").copied();
    if clip.is_some() && flags.contains_key("--rest") {
        return Err("--clip and --rest are mutually exclusive".to_owned());
    }
    let frames = match flags.get("--frames") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|error| format!("invalid --frames: {error}"))?,
        None => 30,
    };
    let radius_mm = match flags.get("--radius") {
        Some(value) => value
            .parse::<f64>()
            .map_err(|error| format!("invalid --radius: {error}"))?,
        None => 40.0,
    };
    if !(5.0..=300.0).contains(&radius_mm) {
        return Err("--radius must be within 5..=300 mm".to_owned());
    }
    Ok(Arguments {
        input,
        clip,
        frames,
        survey,
        radius_mm,
        report: flags.get("--report").copied(),
    })
}

fn report(arguments: Arguments<'_>) -> Result<usize, String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let parts: Vec<&str> = actor.mesh_parts();
    let violations;
    let mut document = json!({
        "verb": "actor joints",
        "input": arguments.input,
        "coordinates": "glTF Y-up world space, metres; axes in the driven bone's rest-local frame",
        "schema": {
            "palette": "mixamorig",
            "entries": joints::schema().map_err(|error| error.to_string())?.len(),
            "file": "crates/sculpto-engine/src/joints.toml",
            "axis_route": "world anchor in the actor's own rest frame, mirrored when its left side is at -x",
        },
        "left_at_plus_x": joints::left_convention(&actor).map_err(|error| error.to_string())?,
        "viewer_clamps": joints::clamp_table(&actor).map_err(|error| error.to_string())?,
    });
    match arguments.clip {
        Some(clip) => {
            let (sweeps, start, end) = joints::check_clip(&actor, clip, arguments.frames)
                .map_err(|error| error.to_string())?;
            violations = sweeps
                .iter()
                .filter(|row| row.severity == Severity::Violation)
                .count();
            let closest = sweeps
                .iter()
                .filter(|row| row.severity == Severity::Ok && row.axis != "off-axis")
                .min_by(|a, b| {
                    let margin = |row: &sculpto_engine::joints::SweepRow| {
                        (row.observed_min_deg - row.min_deg).min(row.max_deg - row.observed_max_deg)
                    };
                    margin(a)
                        .partial_cmp(&margin(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            document["pose"] = json!({
                "kind": "clip",
                "name": clip,
                "frames": arguments.frames.max(2),
                "span_s": [start, end],
            });
            document["sweeps"] = sweeps
                .iter()
                .map(|row| {
                    json!({
                        "joint": row.joint,
                        "node_name": row.node_name,
                        "axis": row.axis,
                        "observed_deg": [row.observed_min_deg, row.observed_max_deg],
                        "limit_deg": [row.min_deg, row.max_deg],
                        "worst_deg": row.worst_deg,
                        "worst_time_s": row.worst_time,
                        "excess_deg": row.excess_deg,
                        "basis": row.basis.token(),
                        "severity": row.severity.token(),
                    })
                })
                .collect::<Vec<_>>()
                .into();
            if let Some(closest) = closest {
                document["closest_to_limit"] = json!({
                    "joint": closest.joint,
                    "axis": closest.axis,
                    "observed_deg": [closest.observed_min_deg, closest.observed_max_deg],
                    "limit_deg": [closest.min_deg, closest.max_deg],
                    "margin_deg": (closest.observed_min_deg - closest.min_deg)
                        .min(closest.max_deg - closest.observed_max_deg),
                });
            }
        }
        None => {
            let check =
                joints::check_pose(&actor, Pose::Rest).map_err(|error| error.to_string())?;
            violations = check
                .rows
                .iter()
                .filter(|row| row.severity == Severity::Violation)
                .count()
                + check
                    .residuals
                    .iter()
                    .filter(|row| row.severity == Severity::Violation)
                    .count();
            document["pose"] = json!({"kind": "rest"});
            document["rows"] = check
                .rows
                .iter()
                .map(|row| {
                    json!({
                        "joint": row.joint,
                        "node_name": row.node_name,
                        "axis": row.axis,
                        "value_deg": row.value_deg,
                        "limit_deg": [row.min_deg, row.max_deg],
                        "excess_deg": row.excess_deg,
                        "basis": row.basis.token(),
                        "severity": row.severity.token(),
                    })
                })
                .collect::<Vec<_>>()
                .into();
            document["residuals"] = check
                .residuals
                .iter()
                .map(|row| {
                    json!({
                        "joint": row.joint,
                        "node_name": row.node_name,
                        "residual_deg": row.residual_deg,
                        "slack_deg": row.slack_deg,
                        "severity": row.severity.token(),
                    })
                })
                .collect::<Vec<_>>()
                .into();
        }
    }
    if arguments.survey {
        let rows = joints::survey(&actor, &parts, arguments.radius_mm)
            .map_err(|error| error.to_string())?;
        document["survey"] = json!({
            "radius_mm": arguments.radius_mm,
            "method": "dominant spread of the flesh slab around the pivot, perpendicular to the bone",
            "rows": rows.iter().map(|row| json!({
                "joint": row.joint,
                "node_name": row.node_name,
                "schema_axis_world": row.schema_axis_world,
                "measured_axis_world": row.measured_axis_world,
                "angle_deg": row.angle_deg,
                "pivot_to_centre_mm": row.pivot_to_centre_mm,
                "vertices": row.vertices,
            })).collect::<Vec<_>>(),
        });
    }
    document["violation_count"] = json!(violations);
    let text = serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    println!("{text}");
    Ok(violations)
}
