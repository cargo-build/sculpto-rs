//! `actor rebind`: seat an actor's mesh onto another skeleton.
//!
//! Flags only; the engine owns the transfer, every proof and the report, so the
//! CLI cannot disagree with the report. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::rebind::{RebindParams, run as rebind_run, verdict};

const USAGE: &str = "usage: sculpto actor rebind --actor actor.glb --skeleton skeleton.glb \
    [--out rebound.glb] [--report rebind.json]";

pub(super) fn run(args: &[String]) -> ExitCode {
    let params = match arguments(args) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor rebind: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match job(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor rebind: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<RebindParams, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--actor" | "--skeleton" | "--out" | "--report" => {
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
    let skeleton = required("--skeleton")?;
    let out = flags.get("--out").map(|value| (*value).to_owned());
    let report = flags.get("--report").map(|value| (*value).to_owned());
    if out.as_deref() == Some(actor.as_str()) || report.as_deref() == Some(actor.as_str()) {
        return Err("input and output paths must differ".to_owned());
    }
    Ok(RebindParams {
        actor: actor.into(),
        skeleton: skeleton.into(),
        out: out.map(Into::into),
        report: report.map(Into::into),
    })
}

fn job(params: &RebindParams) -> Result<(), String> {
    let report = rebind_run(params).map_err(|error| error.to_string())?;
    println!("actor rebind: {}", verdict(&report));
    if let Some(groups) = report["weight_census"]["groups"].as_array() {
        for group in groups {
            let targets = group["targets"]
                .as_array()
                .map(|targets| {
                    targets
                        .iter()
                        .filter_map(|target| target.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            println!(
                "  folded {:8} {:6.2}% -> {}",
                group["group"].as_str().unwrap_or("?"),
                group["share"].as_f64().unwrap_or(0.0) * 100.0,
                targets,
            );
        }
    }
    Ok(())
}
