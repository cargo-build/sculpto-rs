//! Parametric walk-cycle surface: generate a symmetric gait on the actor's own
//! rig, write it as the walk clip and report per-joint ranges plus measured
//! world-space symmetry. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, GaitParameters, gait, write_gait_clip};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor gait --input actor.glb --cycle SECONDS --samples N \
    [--hip DEG] [--knee DEG] [--ankle DEG] [--toe DEG] [--arm DEG] [--elbow DEG] \
    [--bob METRES] [--pelvis-rot DEG] [--head-counter FRACTION] [--knee-phase FRACTION] \
    --out gait.glb \
    [--report gait.json] [--clip NAME]";

struct Arguments<'arg> {
    input: &'arg str,
    params: GaitParameters,
    output: &'arg str,
    report: Option<&'arg str>,
    clip: &'arg str,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor gait: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match gait_command(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor gait: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--input" | "--out" | "--report" | "--clip" | "--cycle" | "--samples" | "--hip"
            | "--knee" | "--ankle" | "--toe" | "--arm" | "--elbow" | "--bob" | "--pelvis-rot"
            | "--head-counter" | "--knee-phase" => {
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
    let number = |key: &str, default: f64| -> Result<f64, String> {
        match flags.get(key) {
            Some(value) => value
                .parse::<f64>()
                .map_err(|error| format!("invalid {key}: {error}")),
            None => Ok(default),
        }
    };
    let defaults = GaitParameters::default();
    let samples = match flags.get("--samples") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|error| format!("invalid --samples: {error}"))?,
        None => defaults.samples,
    };
    let params = GaitParameters {
        cycle_seconds: number("--cycle", defaults.cycle_seconds)?,
        samples,
        hip_amplitude_deg: number("--hip", defaults.hip_amplitude_deg)?,
        knee_amplitude_deg: number("--knee", defaults.knee_amplitude_deg)?,
        ankle_amplitude_deg: number("--ankle", defaults.ankle_amplitude_deg)?,
        toe_amplitude_deg: number("--toe", defaults.toe_amplitude_deg)?,
        arm_amplitude_deg: number("--arm", defaults.arm_amplitude_deg)?,
        elbow_amplitude_deg: number("--elbow", defaults.elbow_amplitude_deg)?,
        pelvis_bob: number("--bob", defaults.pelvis_bob)?,
        pelvis_rotation_deg: number("--pelvis-rot", defaults.pelvis_rotation_deg)?,
        head_counter: number("--head-counter", defaults.head_counter)?,
        knee_phase: number("--knee-phase", defaults.knee_phase)?,
    };
    let input = required("--input")?;
    let output = required("--out")?;
    let report = flags.get("--report").copied();
    let clip = flags.get("--clip").copied().unwrap_or("Gait");
    if input == output || report == Some(input) {
        return Err("input, output and report paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        params,
        output,
        report,
        clip,
    })
}

fn gait_command(arguments: Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let outcome = gait(&actor, &arguments.params).map_err(|error| error.to_string())?;

    let (glb, clip_index) =
        write_gait_clip(&bytes, &outcome, arguments.clip).map_err(|error| error.to_string())?;
    std::fs::write(arguments.output, &glb)
        .map_err(|error| format!("{}: {error}", arguments.output))?;

    let report = report(&outcome, &arguments, clip_index);
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    println!("{text}");
    Ok(())
}

fn report(
    outcome: &sculpto_mesh::actor::GaitOutcome,
    arguments: &Arguments<'_>,
    clip_index: usize,
) -> Value {
    let params = &arguments.params;
    let measures = &outcome.measures;
    json!({
        "verb": "actor gait",
        "input": arguments.input,
        "output": arguments.output,
        "clip": {"name": arguments.clip, "index": clip_index},
        "coordinates": "glTF Y-up world space, metres",
        "parameters": {
            "cycle_seconds": params.cycle_seconds,
            "samples": params.samples,
            "hip_amplitude_deg": params.hip_amplitude_deg,
            "knee_amplitude_deg": params.knee_amplitude_deg,
            "ankle_amplitude_deg": params.ankle_amplitude_deg,
            "toe_amplitude_deg": params.toe_amplitude_deg,
            "arm_amplitude_deg": params.arm_amplitude_deg,
            "elbow_amplitude_deg": params.elbow_amplitude_deg,
            "pelvis_bob_m": params.pelvis_bob,
            "pelvis_rotation_deg": params.pelvis_rotation_deg,
            "head_counter": params.head_counter,
            "knee_phase": params.knee_phase,
        },
        "time_seconds": outcome.times,
        "joint_ranges": outcome.ranges.iter().map(|range| json!({
            "name": range.name, "node": range.node, "role": range.role,
            "min_deg": range.min_deg, "max_deg": range.max_deg,
        })).collect::<Vec<_>>(),
        "measures": {
            "pelvis_vertical_range_m": measures.pelvis_vertical_range,
            "foot_lift_left_m": measures.foot_lift_left,
            "foot_lift_right_m": measures.foot_lift_right,
            "arm_swing_left_deg_pp": measures.arm_swing_left_deg,
            "arm_swing_right_deg_pp": measures.arm_swing_right_deg,
            "chest_yaw_deg_pp": measures.chest_yaw_deg,
            "head_yaw_deg_pp": measures.head_yaw_deg,
            "shoulder_travel_left_m": measures.shoulder_travel_left,
            "shoulder_travel_right_m": measures.shoulder_travel_right,
        },
    })
}
