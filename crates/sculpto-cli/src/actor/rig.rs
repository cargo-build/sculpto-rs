//! Rig-correspondence JSON surface for the pure actor sampler. @req DECL0013

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, Pose, RigRole, RigSample};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor rig --input actor.glb --part NAME [--part NAME ...] \
    --joint NAME [--joint NAME ...] (--rest | --clip NAME --time SECONDS) --out rig.json";

struct Arguments<'arg> {
    input: &'arg str,
    parts: Vec<&'arg str>,
    joints: Vec<&'arg str>,
    pose: Pose<'arg>,
    output: &'arg str,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor rig: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match rig(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor rig: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut parts = Vec::new();
    let mut joints = Vec::new();
    let mut rest = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--rest" if !rest => rest = true,
            "--part" => parts.push(iter.next().ok_or("--part needs a value")?.as_str()),
            "--joint" => joints.push(iter.next().ok_or("--joint needs a value")?.as_str()),
            "--input" | "--clip" | "--time" | "--out" => {
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
    let pose = match (rest, flags.get("--clip"), flags.get("--time")) {
        (true, None, None) => Pose::Rest,
        (false, Some(&name), Some(time)) => {
            let time = time
                .parse::<f64>()
                .map_err(|error| format!("invalid --time: {error}"))?;
            match time.is_finite() {
                true => Pose::Clip { name, time },
                false => return Err("--time must be finite".to_owned()),
            }
        }
        _ => return Err("select --rest or both --clip and --time".to_owned()),
    };
    if parts.is_empty() {
        return Err("at least one --part is required".to_owned());
    }
    if joints.is_empty() {
        return Err("at least one --joint is required".to_owned());
    }
    let input = required("--input")?;
    let output = required("--out")?;
    if input == output {
        return Err("input and output paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        parts,
        joints,
        pose,
        output,
    })
}

fn rig(arguments: Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let sample = actor
        .rig_sample(&arguments.parts, arguments.pose, &arguments.joints)
        .map_err(|error| error.to_string())?;
    let report = report(&sample, arguments.pose)?;
    let file =
        File::create(arguments.output).map_err(|error| format!("{}: {error}", arguments.output))?;
    let mut output = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut output, &report)
        .map_err(|error| format!("{}: {error}", arguments.output))?;
    output
        .flush()
        .map_err(|error| format!("{}: {error}", arguments.output))?;
    println!(
        "actor rig: {} vertices, {} triangles, {} rig nodes ({} requested), {} parts",
        sample.sample.vertices.len(),
        sample.sample.triangles.len(),
        sample.rig.len(),
        sample.requested.len(),
        sample.sample.parts.len()
    );
    Ok(())
}

/// The complete JSON document written for one rig sample.
fn report(sample: &RigSample, pose: Pose<'_>) -> Result<Value, String> {
    let pose = match pose {
        Pose::Rest => json!({"kind": "rest"}),
        Pose::Clip { name, time } => {
            json!({"kind": "clip", "name": name, "time_seconds": time})
        }
        _ => return Err("unsupported actor pose".to_owned()),
    };
    Ok(json!({
        "coordinates": "glTF Y-up world space", "pose": pose,
        "vertex_count": sample.sample.vertices.len(),
        "triangle_count": sample.sample.triangles.len(),
        "vertices": sample.sample.vertices,
        "triangles": sample.sample.triangles,
        "parts": sample.sample.parts.iter().map(|part| json!({
            "name": part.name, "node": part.node, "mesh": part.mesh, "primitive": part.primitive,
            "vertex_start": part.vertices.start, "vertex_count": part.vertices.len(),
            "triangle_start": part.triangles.start, "triangle_count": part.triangles.len(),
        })).collect::<Vec<_>>(),
        "rig": sample.rig.iter().map(|node| json!({
            "node": node.node, "name": node.name, "parent": node.parent,
            "role": match node.role { RigRole::Requested => "requested",
                                      RigRole::Connector => "connector" },
            "world_column_major": node.world,
        })).collect::<Vec<_>>(),
        "requested": sample.requested.iter().map(|joint| json!({
            "name": joint.name, "node": joint.node, "rig": joint.rig,
        })).collect::<Vec<_>>(),
        "influences": sample.influences.iter().enumerate().map(|(vertex, bindings)| json!({
            "vertex": vertex,
            "bindings": bindings.iter().map(|binding| json!({
                "rig": binding.rig, "weight": binding.weight,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}
