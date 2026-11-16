//! `actor tpose`: bake the arm chain onto a reference skeleton's T-pose rest.
//!
//! Flags only; the engine owns the bake, the rest proof and the report, so the
//! CLI cannot disagree with the artifact. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::rest::{TPoseParams, run as tpose_run, verdict};

const USAGE: &str = "usage: sculpto actor tpose --actor actor.glb --reference skeleton.glb \
    [--out tposed.glb] [--report tpose.json] \
    [--pose pose-actor.glb --pose-clip NAME (--pose-time SECONDS | --pose-scan)]";

pub(super) fn run(args: &[String]) -> ExitCode {
    let params = match arguments(args) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor tpose: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match job(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor tpose: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<TPoseParams, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut scan = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--pose-scan" => {
                scan = true;
            }
            "--actor" | "--reference" | "--out" | "--report" | "--pose" | "--pose-clip"
            | "--pose-time" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let required = |key: &str| {
        flags
            .get(key)
            .map(|value| (*value).to_owned())
            .ok_or_else(|| format!("{key} is required"))
    };
    let actor = required("--actor")?;
    let reference = required("--reference")?;
    let out = flags.get("--out").map(|value| (*value).to_owned());
    let report = flags.get("--report").map(|value| (*value).to_owned());
    if out.as_deref() == Some(actor.as_str()) || report.as_deref() == Some(actor.as_str()) {
        return Err("input and output paths must differ".to_owned());
    }
    let value = |key: &str| flags.get(key).map(|value| (*value).to_owned());
    let pose_time = match value("--pose-time") {
        Some(text) => Some(
            text.parse::<f64>()
                .map_err(|error| format!("invalid --pose-time: {error}"))?,
        ),
        None => None,
    };
    Ok(TPoseParams {
        actor: actor.into(),
        reference: reference.into(),
        out: out.map(Into::into),
        report: report.map(Into::into),
        pose: value("--pose").map(Into::into),
        pose_clip: value("--pose-clip"),
        pose_time,
        pose_scan: scan,
    })
}

fn job(params: &TPoseParams) -> Result<(), String> {
    let report = tpose_run(params).map_err(|error| error.to_string())?;
    println!("actor tpose: {}", verdict(&report));
    if let Some(aims) = report["aims"].as_array() {
        for aim in aims {
            println!(
                "  {:16} {:<28} turned {:7.3}°  residual {:0.6}°  below horizontal {:+.3}°",
                aim["role"].as_str().unwrap_or("?"),
                aim["bone"].as_str().unwrap_or("?"),
                aim["turn_degrees"].as_f64().unwrap_or(0.0),
                aim["residual_degrees"].as_f64().unwrap_or(0.0),
                aim["below_horizontal_after_deg"].as_f64().unwrap_or(0.0),
            );
        }
    }
    Ok(())
}
