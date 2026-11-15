//! Explicit joint-angle probe surface: pose named joints, write the probe clip
//! and a world-space displacement report. @req DECL0013

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, Axis, JointPose, Pose, probe, write_probe_clip};
use serde_json::{Value, json};

use super::write_obj;

const USAGE: &str = "usage: sculpto actor pose --input actor.glb --joint NAME --angle DEG \
    [--axis x|y|z] [--joint NAME --angle DEG ...] [--part NAME ...] --out posed.glb \
    [--report posed.json] [--obj posed.obj] [--clip NAME]";

struct ProbeArg<'arg> {
    name: &'arg str,
    angle: Option<f64>,
    axis: Axis,
}

struct Arguments<'arg> {
    input: &'arg str,
    probes: Vec<ProbeArg<'arg>>,
    parts: Vec<&'arg str>,
    output: &'arg str,
    report: Option<&'arg str>,
    obj: Option<&'arg str>,
    clip: &'arg str,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor pose: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match pose(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor pose: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut probes: Vec<ProbeArg<'_>> = Vec::new();
    let mut parts = Vec::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--joint" => {
                let name = iter.next().ok_or("--joint needs a value")?.as_str();
                probes.push(ProbeArg {
                    name,
                    angle: None,
                    axis: Axis::X,
                });
            }
            "--angle" => {
                let value = iter.next().ok_or("--angle needs a value")?;
                let angle = value
                    .parse::<f64>()
                    .map_err(|error| format!("invalid --angle: {error}"))?;
                probes
                    .last_mut()
                    .ok_or("--angle must follow a --joint")?
                    .angle = Some(angle);
            }
            "--axis" => {
                let value = iter.next().ok_or("--axis needs a value")?;
                let axis = Axis::parse(value).map_err(|error| error.to_string())?;
                probes
                    .last_mut()
                    .ok_or("--axis must follow a --joint")?
                    .axis = axis;
            }
            "--part" => parts.push(iter.next().ok_or("--part needs a value")?.as_str()),
            "--input" | "--out" | "--report" | "--obj" | "--clip" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    if probes.is_empty() {
        return Err("at least one --joint is required".to_owned());
    }
    for probe in &probes {
        if probe.angle.is_none() {
            return Err(format!("joint {:?} needs an --angle", probe.name));
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
    let obj = flags.get("--obj").copied();
    let clip = flags.get("--clip").copied().unwrap_or("Pose");
    if input == output || report == Some(input) || obj == Some(input) {
        return Err("input, output, report and obj paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        probes,
        parts,
        output,
        report,
        obj,
        clip,
    })
}

fn pose(arguments: Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let parts: Vec<&str> = match arguments.parts.is_empty() {
        true => actor.mesh_parts(),
        false => arguments.parts.clone(),
    };
    let requests: Vec<JointPose<'_>> = arguments
        .probes
        .iter()
        .map(|probe| JointPose {
            name: probe.name,
            axis: probe.axis,
            angle_deg: probe.angle.expect("validated angle"),
        })
        .collect();
    let outcome = probe(&actor, &parts, &requests).map_err(|error| error.to_string())?;

    // A probe is a static snapshot: one constant rotation per posed joint,
    // written as the walk clip so the viewer plays it directly.
    let (glb, clip_index) =
        write_probe_clip(&bytes, &outcome, arguments.clip).map_err(|error| error.to_string())?;
    std::fs::write(arguments.output, &glb)
        .map_err(|error| format!("{}: {error}", arguments.output))?;

    if let Some(obj) = arguments.obj {
        let posed = Actor::from_glb(&glb).map_err(|error| error.to_string())?;
        let sample = posed
            .sample(
                &parts,
                Pose::Clip {
                    name: arguments.clip,
                    time: 0.0,
                },
            )
            .map_err(|error| error.to_string())?;
        write_obj(Path::new(obj), &sample)?;
    }

    let report = report(&outcome, &arguments, clip_index, &parts);
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    println!("{text}");
    Ok(())
}

fn axis_letter(axis: Axis) -> &'static str {
    match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
    }
}

fn report(
    outcome: &sculpto_mesh::actor::PoseOutcome,
    arguments: &Arguments<'_>,
    clip_index: usize,
    parts: &[&str],
) -> Value {
    json!({
        "verb": "actor pose",
        "input": arguments.input,
        "output": arguments.output,
        "clip": {"name": arguments.clip, "index": clip_index},
        "coordinates": "glTF Y-up world space, metres",
        "parts": parts,
        "vertex_count": outcome.vertex_count,
        "joints": outcome.probes.iter().map(|joint| json!({
            "name": joint.name,
            "node": joint.node,
            "axis": axis_letter(joint.axis),
            "angle_deg": joint.angle_deg,
            "delta_quaternion_xyzw": joint.delta_quaternion,
            "rest_rotation_xyzw": joint.rest_rotation,
            "posed_rotation_xyzw": joint.posed_rotation,
            "world_pivot": joint.world_pivot,
            "moved_vertices": joint.moved_vertices,
            "max_displacement_m": joint.max_displacement,
            "farthest_vertex": joint.farthest_vertex,
            "farthest_rest": joint.farthest_rest,
            "farthest_posed": joint.farthest_posed,
        })).collect::<Vec<_>>(),
    })
}
