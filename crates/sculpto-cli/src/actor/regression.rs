//! `actor rig-regression`: the four-way default-vs-adapted rig gate.
//!
//! One declaration file, one numeric table, one exit status; the engine owns
//! every measurement and the verdict. @req DECL0013

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_engine::regression::{RegressionParams, run_params, table_lines};

const USAGE: &str =
    "usage: sculpto actor rig-regression --config regression.json [--report report.json]
  the config holds the rig.regression input: one block per actor naming the
  default artifact (the own walk), the adapted artifact, the two clip names,
  the clothing anchor clip, context clips and the clothing part addresses
  exit status: 2 bad arguments, 1 a failed gate or execution refusal";

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor rig-regression: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut params: RegressionParams = match std::fs::read(arguments.config)
        .map_err(|error| format!("{}: {error}", arguments.config))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", arguments.config))
        }) {
        Ok(params) => params,
        Err(error) => {
            eprintln!("actor rig-regression: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Some(report) = arguments.report {
        params.report = Some(report.into());
    }
    match run_params(&params) {
        Ok(report) => {
            for line in table_lines(&report) {
                println!("{line}");
            }
            match report["pass"].as_bool() {
                Some(true) => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            }
        }
        Err(error) => {
            eprintln!("actor rig-regression: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Arguments<'arg> {
    config: &'arg str,
    report: Option<&'arg str>,
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--config" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let config = flags
        .get("--config")
        .copied()
        .ok_or("--config is required")?;
    Ok(Arguments {
        config,
        report: flags.get("--report").copied(),
    })
}
