//! `actor skin`: author the skin for an actor's own skeleton from the flesh.
//!
//! Flags only; the engine owns the authoring, every proof and the report, so the
//! CLI cannot disagree with the report. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::skin::{SkinParams, run as skin_run, verdict};

const USAGE: &str = "usage: sculpto actor skin --actor actor.glb [--out skinned.glb] \
    [--report skin.json]";

pub(super) fn run(args: &[String]) -> ExitCode {
    let params = match arguments(args) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor skin: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match job(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor skin: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<SkinParams, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--actor" | "--out" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let actor = flags
        .get("--actor")
        .map(|value| (*value).to_owned())
        .ok_or("--actor is required")?;
    let out = flags.get("--out").map(|value| (*value).to_owned());
    let report = flags.get("--report").map(|value| (*value).to_owned());
    if out.as_deref() == Some(actor.as_str()) || report.as_deref() == Some(actor.as_str()) {
        return Err("input and output paths must differ".to_owned());
    }
    Ok(SkinParams {
        actor: actor.into(),
        out: out.map(Into::into),
        report: report.map(Into::into),
    })
}

fn job(params: &SkinParams) -> Result<(), String> {
    let report = skin_run(params).map_err(|error| error.to_string())?;
    println!("actor skin: {}", verdict(&report));
    let gates = &report["gates"];
    println!(
        "  side isolation {} / {} violations, leaf mass {:.3e}, discarded over 0.05 {}",
        gates["side_isolation"]["left_violations"]
            .as_u64()
            .unwrap_or_default(),
        gates["side_isolation"]["right_violations"]
            .as_u64()
            .unwrap_or_default(),
        gates["leaf_weight_mass"].as_f64().unwrap_or_default(),
        gates["discarded_over_0_05"].as_u64().unwrap_or_default(),
    );
    println!(
        "  method {}: {} deform bones, {} leaves excluded",
        report["method"]["name"].as_str().unwrap_or("?"),
        report["palette"]["deform_bones"]
            .as_u64()
            .unwrap_or_default(),
        report["palette"]["leaves_excluded"]
            .as_array()
            .map(|leaves| leaves.len())
            .unwrap_or_default(),
    );
    Ok(())
}
