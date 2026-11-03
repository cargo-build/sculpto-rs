//! File and serialization surface for the pure actor sampler. @req DECL0013

mod pack;
mod rig;

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, Pose, Sample};

const USAGE: &str = "usage: sculpto actor sample --input actor.glb --part NAME [--part NAME ...] \
    (--rest | --clip NAME --time SECONDS) --out actor.obj [--report actor.json]";

#[derive(Debug)]
struct Arguments<'arg> {
    input: &'arg str,
    parts: Vec<&'arg str>,
    pose: Pose<'arg>,
    output: &'arg str,
    report: Option<&'arg str>,
}

pub(crate) fn run(args: &[String]) -> ExitCode {
    if args.first().is_some_and(|command| command == "pack-cloth") {
        return pack::run(&args[1..]);
    }
    if args.first().is_some_and(|command| command == "rig") {
        return rig::run(&args[1..]);
    }
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match sample(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut parts = Vec::new();
    let mut rest = false;
    let mut iter = args.iter();
    match iter.next().map(String::as_str) {
        Some("sample") => {}
        _ => return Err("expected actor sample".to_owned()),
    }
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--rest" if !rest => rest = true,
            "--part" => parts.push(iter.next().ok_or("--part needs a value")?.as_str()),
            "--input" | "--clip" | "--time" | "--out" | "--report" => {
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
    let input = required("--input")?;
    let output = required("--out")?;
    let report = flags.get("--report").copied();
    if input == output || report == Some(input) || report == Some(output) {
        return Err("input, output and report paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        parts,
        pose,
        output,
        report,
    })
}

fn sample(arguments: Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let sample = actor
        .sample(&arguments.parts, arguments.pose)
        .map_err(|error| error.to_string())?;
    write_obj(Path::new(arguments.output), &sample)?;
    if let Some(path) = arguments.report {
        write_report(Path::new(path), &sample, arguments.pose)?;
    }
    println!(
        "actor: {} vertices, {} triangles, {} primitives",
        sample.vertices.len(),
        sample.triangles.len(),
        sample.parts.len()
    );
    Ok(())
}

fn write_obj(path: &Path, sample: &Sample) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut output = BufWriter::new(file);
    let write = |output: &mut BufWriter<File>| -> std::io::Result<()> {
        writeln!(
            output,
            "# Sculpto actor: world-space glTF Y-up; no axis conversion"
        )?;
        sample
            .vertices
            .iter()
            .try_for_each(|point| writeln!(output, "v {} {} {}", point[0], point[1], point[2]))?;
        sample.parts.iter().try_for_each(|part| {
            writeln!(
                output,
                "g node_{}_mesh_{}_primitive_{}",
                part.node, part.mesh, part.primitive
            )?;
            sample.triangles[part.triangles.clone()]
                .iter()
                .try_for_each(|triangle| {
                    writeln!(
                        output,
                        "f {} {} {}",
                        triangle[0] + 1,
                        triangle[1] + 1,
                        triangle[2] + 1
                    )
                })
        })?;
        output.flush()
    };
    write(&mut output).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_report(path: &Path, sample: &Sample, pose: Pose<'_>) -> Result<(), String> {
    let pose = match pose {
        Pose::Rest => serde_json::json!({"kind": "rest"}),
        Pose::Clip { name, time } => {
            serde_json::json!({"kind": "clip", "name": name, "time_seconds": time})
        }
        _ => return Err("unsupported actor pose".to_owned()),
    };
    let report = serde_json::json!({
        "coordinates": "glTF Y-up world space", "pose": pose,
        "vertex_count": sample.vertices.len(), "triangle_count": sample.triangles.len(),
        "parts": sample.parts.iter().map(|part| serde_json::json!({
            "name": part.name, "node": part.node, "mesh": part.mesh, "primitive": part.primitive,
            "vertex_start": part.vertices.start, "vertex_count": part.vertices.len(),
            "triangle_start": part.triangles.start, "triangle_count": part.triangles.len(),
        })).collect::<Vec<_>>(),
    });
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut output = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut output, &report)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    output
        .flush()
        .map_err(|error| format!("{}: {error}", path.display()))
}
