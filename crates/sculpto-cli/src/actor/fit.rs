//! `actor fit`: derive extremity rest frames from the flesh they dominate.
//!
//! The verb is a rig step, not a render: it rewrites the joint rest transforms
//! and inverse bind matrices of an exported actor so every extremity pivot sits
//! on the geometry it drives, then reports the before/after measurement of the
//! assigned gates. The rest pose is untouched by construction, which the command
//! proves by sampling both files and comparing every vertex.

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, FitOutcome, LengthMode, Pose, Sample, fit, write_fit};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor fit --input actor.glb --out fitted.glb \
    [--report fitted.json] [--lengths flesh|preserve] [--part NAME ...]";

struct Arguments<'arg> {
    input: &'arg str,
    output: &'arg str,
    report: Option<&'arg str>,
    lengths: LengthMode,
    parts: Vec<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor fit: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor fit: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut parts = Vec::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--part" => parts.push(iter.next().ok_or("--part needs a value")?.as_str()),
            "--input" | "--out" | "--report" | "--lengths" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let required = |key| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("{key} is required"))
    };
    let input = required("--input")?;
    let output = required("--out")?;
    let report = flags.get("--report").copied();
    let lengths = match flags.get("--lengths").copied() {
        None => LengthMode::Flesh,
        Some(value) => LengthMode::parse(value).map_err(|error| error.to_string())?,
    };
    if input == output || report == Some(input) || report == Some(output) {
        return Err("input, output and report paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        output,
        report,
        lengths,
        parts,
    })
}

fn execute(arguments: Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let parts: Vec<&str> = match arguments.parts.is_empty() {
        true => actor.mesh_parts(),
        false => arguments.parts.clone(),
    };
    let extent = sculpto_mesh::actor::extent(&actor, &parts).map_err(|error| error.to_string())?;
    let before = actor
        .sample(&parts, Pose::Rest)
        .map_err(|error| error.to_string())?;
    let outcome = fit(&actor, &parts, arguments.lengths).map_err(|error| error.to_string())?;
    let written = write_fit(&bytes, &outcome).map_err(|error| error.to_string())?;
    std::fs::write(arguments.output, &written)
        .map_err(|error| format!("{}: {error}", arguments.output))?;

    // The rest pose must survive the rewrite: sample the written file and
    // compare every vertex with the input's rest sample.
    let fitted = Actor::from_glb(&written).map_err(|error| error.to_string())?;
    let after = fitted
        .sample(&parts, Pose::Rest)
        .map_err(|error| error.to_string())?;
    let max_shift = rest_shift(&before, &after);
    let limit = 1e-5 * extent;

    let report = report(
        &arguments,
        &outcome,
        written.len(),
        extent,
        max_shift,
        limit,
        &parts,
    );
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    println!(
        "actor fit: {} chains fitted, {} unfitted, {} joints moved, max pivot shift {:.1} mm, \
         rest reconstruction {:.3e} (limit {:.3e})",
        outcome.chains.len(),
        outcome.unfitted.len(),
        outcome.moved_joints,
        outcome.max_pivot_shift * 1000.0,
        max_shift,
        limit
    );
    Ok(())
}

/// Largest per-vertex distance between two same-length rest samples.
fn rest_shift(before: &Sample, after: &Sample) -> f64 {
    before
        .vertices
        .iter()
        .zip(&after.vertices)
        .map(|(left, right)| {
            let delta = [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
            (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt()
        })
        .fold(0.0_f64, f64::max)
}

fn report(
    arguments: &Arguments<'_>,
    outcome: &FitOutcome,
    written_bytes: usize,
    extent: f64,
    rest_shift: f64,
    rest_limit: f64,
    parts: &[&str],
) -> Value {
    let centroid_limit = 0.015_f64;
    let mut outside = Vec::new();
    let mut within = 0usize;
    for chain in &outcome.chains {
        for joint in &chain.joints {
            match joint.centroid_after {
                Some(distance) if distance <= centroid_limit => within += 1,
                Some(_) => outside.push(json!({
                    "chain": chain.label,
                    "joint": joint.name,
                    "centroid_m": joint.centroid_after,
                })),
                None => {}
            }
        }
    }
    let worst = |pick: fn(&sculpto_mesh::actor::ChainFit) -> f64| {
        outcome.chains.iter().map(pick).fold(0.0_f64, f64::max)
    };
    let worst_joint = |pick: fn(&sculpto_mesh::actor::JointFit) -> f64| {
        outcome
            .chains
            .iter()
            .flat_map(|chain| chain.joints.iter())
            .filter(|joint| joint.owned)
            .map(pick)
            .fold(0.0_f64, f64::max)
    };
    json!({
        "verb": "actor fit",
        "input": arguments.input,
        "output": arguments.output,
        "bytes": written_bytes,
        "lengths": arguments.lengths.as_str(),
        "coordinates": "actor units, the frame the input GLB carries",
        "parts": parts,
        "vertex_count": outcome.vertex_count,
        "extent": extent,
        "chains": outcome.chains.iter().map(|chain| json!({
            "label": chain.label,
            "kind": chain.kind,
            "flexion": chain.flexion.as_str(),
            "axis": chain.axis,
            "flesh_vertices": chain.flesh_vertices,
            "axis_before_degrees": chain.axis_before_degrees,
            "axis_after_degrees": chain.axis_after_degrees,
            "flexion_axis_off_plane_before_degrees": chain.flexion_axis_off_plane_before_degrees,
            "flexion_axis_off_plane_after_degrees": chain.flexion_axis_off_plane_after_degrees,
            "joints": chain.joints.iter().map(|joint| json!({
                "name": joint.name,
                "node": joint.node,
                "owned": joint.owned,
                "placed_by": joint.placed_by.as_str(),
                "band_vertices": joint.band_vertices,
                "band_span_m": joint.band_span,
                "dominated": joint.dominated,
                "before_head": joint.before_head,
                "after_head": joint.after_head,
                "perpendicular_before_m": joint.perpendicular_before,
                "perpendicular_after_m": joint.perpendicular_after,
                "centroid_before_m": joint.centroid_before,
                "centroid_after_m": joint.centroid_after,
                "segment_before_m": joint.segment_before,
                "segment_after_m": joint.segment_after,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "unfitted": outcome.unfitted.iter().map(|chain| json!({
            "label": chain.label,
            "reason": chain.reason,
            "dominated": chain.dominated,
        })).collect::<Vec<_>>(),
        "summary": {
            "chains": outcome.chains.len(),
            "unfitted": outcome.unfitted.len(),
            "moved_joints": outcome.moved_joints,
            "max_pivot_shift_m": outcome.max_pivot_shift,
            "skin_base_spread": outcome.skin_base_spread,
            "worst_axis_after_degrees": worst(|chain| chain.axis_after_degrees),
            "worst_perpendicular_after_m": worst_joint(|joint| joint.perpendicular_after),
            "worst_centroid_after_m": worst_joint(|joint| joint.centroid_after.unwrap_or(0.0)),
        },
        "gates": {
            "axis_limit_degrees": 5.0,
            "axis_pass": worst(|chain| chain.axis_after_degrees) <= 5.0,
            "perpendicular_limit_m": 0.015,
            "perpendicular_pass": worst_joint(|joint| joint.perpendicular_after) <= 0.015,
            "literal_centroid_limit_m": centroid_limit,
            "literal_centroid_within": within,
            "literal_centroid_outside": outside,
            "rest_reconstruction_limit_m": rest_limit,
            "rest_reconstruction_m": rest_shift,
            "rest_reconstruction_pass": rest_shift <= rest_limit,
        },
    })
}
