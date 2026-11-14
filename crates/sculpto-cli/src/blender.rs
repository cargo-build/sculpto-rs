//! `sculpto blender …` — the headless Blender host boundary.
//!
//! The command reads one versioned request, resolves it against the request
//! file's directory, runs the pinned host and prints the validated result. A
//! refusal is a result document with `status: "refused"` and exit 1; an
//! invalid request or environment failure is exit 2.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sculpto_blender::ClothBakeRequest;

const USAGE: &str = "\
usage: sculpto blender cloth-bake --request <json> [options]

options:
  --request <json>    versioned sculpto.blender.cloth_bake request
  --out <dir>         override output.directory
  --blender <exe>     override the Blender executable
  --runner <py>       override the runner script (digest is recomputed)
";

/// Dispatch the `blender` argv family.
pub fn run(args: &[String]) -> ExitCode {
    let Some(action) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    if action != "cloth-bake" {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    }
    cloth_bake(&args[1..])
}

fn cloth_bake(args: &[String]) -> ExitCode {
    let mut request_path = None;
    let mut out = None;
    let mut blender = None;
    let mut runner = None;
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        let value = match args.get(index + 1) {
            Some(value) => PathBuf::from(value),
            None => {
                eprintln!("{flag} requires a value");
                return ExitCode::from(2);
            }
        };
        match flag.as_str() {
            "--request" => request_path = Some(value),
            "--out" => out = Some(value),
            "--blender" => blender = Some(value),
            "--runner" => runner = Some(value),
            other => {
                eprintln!("unknown option {other}");
                eprint!("{USAGE}");
                return ExitCode::from(2);
            }
        }
        index += 2;
    }
    let Some(request_path) = request_path else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&request_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{}: {error}", request_path.display());
            return ExitCode::from(2);
        }
    };
    let mut request = match ClothBakeRequest::from_json(&bytes) {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if let Some(out) = out {
        request.output.directory = out;
    }
    if let Some(blender) = blender {
        request.blender.executable = Some(blender);
    }
    if let Some(runner) = runner {
        request.runner.path = runner;
        request.runner.sha256 = None;
    }

    let base = absolute_base(&request_path);
    match sculpto_blender::run_with_base(request, &base) {
        Ok(outcome) => {
            match serde_json::to_string_pretty(&outcome.result) {
                Ok(text) => println!("{text}"),
                Err(error) => {
                    eprintln!("cannot print the result: {error}");
                    return ExitCode::FAILURE;
                }
            }
            if outcome.accepted() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn absolute_base(request_path: &Path) -> PathBuf {
    let absolute = if request_path.is_absolute() {
        request_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(request_path))
            .unwrap_or_else(|_| request_path.to_path_buf())
    };
    absolute
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
