//! Argv surface for the per-frame dressed clearance harness. @req DECL0013

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use sculpto_mesh::actor::{Actor, StressPlan, StressReport, stress};

const USAGE: &str = "usage: sculpto actor stress --input actor.glb --body NAME --garment NAME \
    [--garment NAME ...] (--rest | --clip NAME --frames N) --tolerance UNITS [--out report.json]
  part names address mesh nodes; Name#i picks primitive i of a multi-primitive node
  (product GLBs carry body and underwear as primitives of one node)
  tolerance is the accepted graded visible depth in the GLB's own units
  exit status: 2 bad arguments, 1 execution refusal or a failed frame verdict";

#[derive(Debug)]
struct Arguments<'arg> {
    input: &'arg str,
    body: &'arg str,
    garments: Vec<&'arg str>,
    clip: Option<&'arg str>,
    frames: Option<usize>,
    tolerance: Option<f32>,
    output: Option<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor stress: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run_stress(arguments) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("actor stress: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut garments = Vec::new();
    let mut rest = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--rest" if !rest => rest = true,
            "--garment" => garments.push(iter.next().ok_or("--garment needs a value")?.as_str()),
            "--input" | "--body" | "--clip" | "--frames" | "--tolerance" | "--out" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let pose = match (rest, flags.get("--clip")) {
        (true, None) => (true, None),
        (false, Some(clip)) => (false, Some(clip.as_str())),
        _ => return Err("select --rest or --clip".to_owned()),
    };
    let frames = match flags.get("--frames") {
        Some(frames) => match frames.parse::<usize>() {
            Ok(frames) if frames > 0 => Some(frames),
            _ => return Err("--frames must be a count of at least 1".to_owned()),
        },
        None if rest => None,
        None => return Err("--frames is required with --clip".to_owned()),
    };
    if garments.is_empty() {
        return Err("at least one --garment is required".to_owned());
    }
    let tolerance = match flags.get("--tolerance") {
        Some(value) => match value.parse::<f32>() {
            Ok(tolerance) if tolerance.is_finite() && tolerance >= 0.0 => Some(tolerance),
            _ => return Err("--tolerance must be a finite non-negative depth".to_owned()),
        },
        None => return Err("--tolerance is required".to_owned()),
    };
    let required = |key| {
        flags
            .get(key)
            .map(|value| value.as_str())
            .ok_or_else(|| format!("{key} is required"))
    };
    let input = required("--input")?;
    let body = required("--body")?;
    let output = flags.get("--out").map(|value| value.as_str());
    if Some(input) == output {
        return Err("input and output paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        body,
        garments,
        clip: pose.1,
        frames,
        tolerance,
        output,
    })
}

fn run_stress(arguments: Arguments<'_>) -> Result<bool, String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let plan = StressPlan {
        body: arguments.body,
        garments: arguments.garments,
        clip: arguments.clip,
        frames: arguments.frames.unwrap_or(1),
        tolerance: arguments.tolerance.unwrap_or_default(),
    };
    let report = stress(&actor, &plan).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.output {
        write_report(Path::new(path), &report)?;
    }
    for row in &report.frames {
        let time = row
            .time_seconds
            .map_or_else(|| "rest".to_owned(), |time| format!("t={time:.6}"));
        for garment in &row.garments {
            println!(
                "frame {} {time} {}: visible_max={:.6} penetrations={} violations={} body_pairs={} self_pairs={} {}",
                row.index,
                garment.part,
                garment.visible_max,
                garment.penetrations,
                garment.violations,
                garment.body_pairs,
                garment.self_pairs,
                if garment.passed { "PASS" } else { "FAIL" },
            );
        }
    }
    let worst = report
        .frames
        .iter()
        .map(|row| row.visible_max)
        .fold(0.0_f32, f32::max);
    println!(
        "actor stress: {} frames, worst visible_max={:.6} at tolerance {:.6}: {}",
        report.frames.len(),
        worst,
        report.tolerance,
        if report.passed { "PASS" } else { "FAIL" }
    );
    Ok(report.passed)
}

fn write_report(path: &Path, report: &StressReport) -> Result<(), String> {
    let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut output = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut output, report)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    output
        .flush()
        .map_err(|error| format!("{}: {error}", path.display()))
}
