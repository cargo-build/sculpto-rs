//! `actor havok`: decode one Skyrim Havok clip and bind its tracks by index.
//!
//! Flags only; the engine owns every measurement, the written GLB and the report,
//! so the CLI cannot disagree with the report. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::havok::{HavokParams, run as havok_run};

const USAGE: &str = "usage: sculpto actor havok (--archive CLIP.bsa --member PATH | --file CLIP.hkx) \
    --skeleton skeleton.hkx [--actor actor.glb] [--scale 0.014] [--out source.glb] \
    [--report havok.json]";

pub(super) fn run(args: &[String]) -> ExitCode {
    let params = match arguments(args) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor havok: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match job(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor havok: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<HavokParams, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--archive" | "--member" | "--file" | "--skeleton" | "--actor" | "--scale"
            | "--out" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let optional = |key: &str| flags.get(key).map(|value| (*value).to_owned());
    let required = |key: &str| {
        flags
            .get(key)
            .map(|value| (*value).to_owned())
            .ok_or_else(|| format!("{key} is required"))
    };
    let scale = match flags.get("--scale") {
        Some(value) => value
            .parse::<f64>()
            .map_err(|error| format!("invalid --scale: {error}"))?,
        None => sculpto_engine::havok::HAVOK_METRE,
    };
    Ok(HavokParams {
        archive: optional("--archive").map(Into::into).unwrap_or_default(),
        member: optional("--member"),
        file: optional("--file").map(Into::into),
        skeleton: required("--skeleton")?.into(),
        actor: optional("--actor").map(Into::into),
        scale,
        out: optional("--out").map(Into::into),
        report: optional("--report").map(Into::into),
    })
}

fn job(params: &HavokParams) -> Result<(), String> {
    let report = havok_run(params).map_err(|error| error.to_string())?;
    match report.get("frames") {
        None => println!(
            "actor havok: rig {} · {} bones",
            report["rig_skeleton"]["name"].as_str().unwrap_or("?"),
            report["rig_skeleton"]["bones"],
        ),
        Some(_) => println!(
            "actor havok: {} · {} frames over {:.3}s · {} tracks · {} palette bones, \
             {} undriven · {} unbound tracks",
            report["class"].as_str().unwrap_or("?"),
            report["frames"],
            report["duration_s"].as_f64().unwrap_or(0.0),
            report["tracks"],
            report["palette_bones"],
            report["undriven_palette"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
            report["unbound_tracks"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default(),
        ),
    }
    Ok(())
}
