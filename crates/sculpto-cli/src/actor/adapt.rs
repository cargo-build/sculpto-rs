//! `actor adapt`: convert a clip from any skeleton onto one actor's palette.
//!
//! Flags only; the engine owns every measurement, the written GLB and the
//! report, so the CLI cannot disagree with the report. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::adapt::{AdaptParams, run as adapt_run, verdict};

const USAGE: &str = "usage: sculpto actor adapt --actor actor.glb --source clip.glb \
    [--source-clip NAME] [--clip NAME] [--semantics pose|motion] \
    [--root-motion keep|split|zero] [--role NAME ...] [--out adapted.glb] \
    [--report adapt.json] [--frames N] [--require-compatible-rest]";

pub(super) fn run(args: &[String]) -> ExitCode {
    let params = match arguments(args) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor adapt: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match job(&params) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor adapt: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<AdaptParams, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut roles = Vec::new();
    let mut require_compatible = false;
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--source-clip" | "--clip" | "--semantics" | "--root-motion" | "--actor"
            | "--source" | "--out" | "--report" | "--frames" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            "--role" => roles.push(iter.next().ok_or("--role needs a value")?.clone()),
            "--require-compatible-rest" => require_compatible = true,
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let required = |key: &str| {
        flags
            .get(key)
            .map(|value| (*value).to_owned())
            .ok_or_else(|| format!("{key} is required"))
    };
    let frames = match flags.get("--frames") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|error| format!("invalid --frames: {error}"))?
            .max(2),
        None => 30,
    };
    Ok(AdaptParams {
        actor: required("--actor")?.into(),
        source: required("--source")?.into(),
        source_clip: flags.get("--source-clip").map(|value| (*value).to_owned()),
        clip: flags.get("--clip").map(|value| (*value).to_owned()),
        semantics: flags.get("--semantics").map(|value| (*value).to_owned()),
        root_motion: flags.get("--root-motion").map(|value| (*value).to_owned()),
        out: flags.get("--out").map(|value| (*value).into()),
        report: flags.get("--report").map(|value| (*value).into()),
        require_roles: roles,
        allow_incompatible_rest: !require_compatible,
        frames,
    })
}

fn job(params: &AdaptParams) -> Result<(), String> {
    let report = adapt_run(params).map_err(|error| error.to_string())?;
    let clip = &report["written_clip"];
    println!(
        "actor adapt: {} · {} copy · {} bones of {} · pose error {:.2}° median / {:.2}° max · \
         rest residual {:.4} · stance foot {:.3} m/s · edge stretch {:.2}x",
        verdict(&report),
        clip["semantics"].as_str().unwrap_or("?"),
        clip["mapped_bones"],
        report["admission"]["palette_bones"],
        clip["pose_error_median_degrees"].as_f64().unwrap_or(0.0),
        clip["pose_error_max_degrees"].as_f64().unwrap_or(0.0),
        clip["max_rest_translation_residual"]
            .as_f64()
            .unwrap_or(0.0),
        report["gates"]["stance_foot"]["max_horizontal_speed"]
            .as_f64()
            .unwrap_or(0.0),
        report["gates"]["edge_stretch"]["max_ratio"]
            .as_f64()
            .unwrap_or(0.0),
    );
    Ok(())
}
