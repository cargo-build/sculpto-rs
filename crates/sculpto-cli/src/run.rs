//! `sculpto run` and `sculpto commands`: the command surface over the engine
//! registry.
//!
//! The wire contract is one JSON envelope per invocation, on stdout. Exit codes
//! are part of the contract: `0` success, `2` an invalid request, `1` a domain
//! refusal or an environment failure. The split mirrors the argv surface, where
//! usage errors already exit `2`.
//!
//! The handlers here are bindings, not a second implementation: each one builds
//! the same parameter struct the argv path builds and calls the same function in
//! [`crate::ops`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use sculpto_engine::{CommandError, ErrorKind, Handler, catalogue, dispatch_with, envelope};
use serde_json::{Value, json};

use crate::ops;

const USAGE: &str = "\
usage: sculpto run --cmd <id> [--input <json|->]
       sculpto commands [--json]

run options:
  --cmd <id>        command id from `sculpto commands`
  --input <json>    JSON input; `-` reads stdin; defaults to null

commands options:
  --json            print the catalogue as JSON instead of a table
";

/// Handlers this binary supplies for catalogued ids.
fn reject(code: &'static str, message: impl Into<String>) -> ExitCode {
    let error = CommandError::new(ErrorKind::InvalidRequest, code, message);
    println!("{}", envelope(Err(error)));
    ExitCode::from(2)
}

const HANDLERS: &[(&str, Handler)] = &[
    ("commands.list", commands_list_handler),
    ("bake", bake_handler),
    ("outfit.fit", fit_handler),
    ("export.obj", export_obj_handler),
];

/// `sculpto commands`.
pub fn commands(args: &[String]) -> ExitCode {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return reject("unknown_option", format!("unknown option {other}")),
        }
    }
    if json {
        let rows: Vec<Value> = catalogue()
            .iter()
            .map(|spec| {
                json!({
                    "id": spec.id.as_str(),
                    "summary": spec.summary,
                    "mutates": spec.mutates,
                    "implemented": spec.implemented || HANDLERS.iter().any(|(id, _)| *id == spec.id.as_str()),
                })
            })
            .collect();
        println!("{}", json!({ "commands": rows }));
        return ExitCode::SUCCESS;
    }
    for spec in catalogue() {
        let implemented =
            spec.implemented || HANDLERS.iter().any(|(id, _)| *id == spec.id.as_str());
        let marker = if implemented { " " } else { "*" };
        let class = if spec.mutates { "write" } else { "read " };
        println!(
            "{marker} {:<16} {class}  {}",
            spec.id.as_str(),
            spec.summary
        );
    }
    println!("\n* catalogued, handler not wired yet");
    ExitCode::SUCCESS
}

/// `sculpto run`.
pub fn run(args: &[String]) -> ExitCode {
    let mut command: Option<String> = None;
    let mut input: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--cmd" => {
                index += 1;
                match args.get(index) {
                    Some(value) => command = Some(value.clone()),
                    None => return reject("missing_option_value", "--cmd needs a value"),
                }
            }
            "--input" => {
                index += 1;
                match args.get(index) {
                    Some(value) => input = Some(value.clone()),
                    None => return reject("missing_option_value", "--input needs a value or -"),
                }
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => return reject("unknown_option", format!("unknown option {other}")),
        }
        index += 1;
    }

    let Some(command) = command else {
        return reject("missing_field", "--cmd is required");
    };

    let payload = match input.as_deref() {
        None => Value::Null,
        Some("-") => {
            let mut buffer = String::new();
            if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buffer) {
                return reject("stdin", format!("cannot read stdin: {error}"));
            }
            match serde_json::from_str(&buffer) {
                Ok(value) => value,
                Err(error) => {
                    return reject("invalid_json", format!("--input is not JSON: {error}"));
                }
            }
        }
        Some(text) => match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                return reject("invalid_json", format!("--input is not JSON: {error}"));
            }
        },
    };

    let result = dispatch_with(&command, &payload, HANDLERS);
    let exit = match &result {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => match error.kind() {
            ErrorKind::InvalidRequest | ErrorKind::InvalidParams | ErrorKind::NotFound => {
                ExitCode::from(2)
            }
            _ => ExitCode::from(1),
        },
    };
    println!("{}", envelope(result));
    exit
}

fn missing(field: &str) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "missing_field",
        format!("`{field}` is required"),
    )
}

fn string(input: &Value, field: &str) -> Result<String, CommandError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| missing(field))
}

fn optional_string(input: &Value, field: &str) -> Option<String> {
    input.get(field).and_then(Value::as_str).map(str::to_string)
}

fn path(input: &Value, field: &str) -> Result<PathBuf, CommandError> {
    string(input, field).map(PathBuf::from)
}

fn optional_path(input: &Value, field: &str) -> Option<PathBuf> {
    optional_string(input, field).map(PathBuf::from)
}

fn commands_list_handler(_input: &Value) -> Result<Value, CommandError> {
    let rows: Vec<Value> = catalogue()
        .iter()
        .map(|spec| {
            let implemented =
                spec.implemented || HANDLERS.iter().any(|(id, _)| *id == spec.id.as_str());
            json!({
                "id": spec.id.as_str(),
                "summary": spec.summary,
                "mutates": spec.mutates,
                "implemented": implemented,
            })
        })
        .collect();
    Ok(json!({ "commands": rows }))
}

fn bake_handler(input: &Value) -> Result<Value, CommandError> {
    let mut sliders = BTreeMap::new();
    if let Some(map) = input.get("sliders").and_then(Value::as_object) {
        for (name, value) in map {
            let percent = value.as_f64().ok_or_else(|| {
                CommandError::new(
                    ErrorKind::InvalidParams,
                    "slider_not_a_number",
                    format!("slider `{name}` is not a number"),
                )
            })?;
            sliders.insert(name.clone(), percent as f32);
        }
    }
    let params = ops::BakeParams {
        project: path(input, "project")?,
        presets: path(input, "presets")?,
        set: string(input, "set")?,
        preset: string(input, "preset")?,
        endpoint: string(input, "endpoint")?,
        out: path(input, "out")?,
        source: optional_path(input, "source"),
        shape: optional_string(input, "shape"),
        home: optional_path(input, "home"),
        sliders,
    };
    let outcome = ops::bake(&params)?;
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
        "sliders": outcome.sliders,
        "unresolved": outcome.unresolved,
        "deferred": outcome.deferred,
    }))
}

fn fit_handler(input: &Value) -> Result<Value, CommandError> {
    let params = ops::FitParams {
        nif: path(input, "nif")?,
        shape: string(input, "shape")?,
        body_shape: string(input, "body_shape")?,
        body: path(input, "body")?,
        out: path(input, "out")?,
        body_after: optional_path(input, "body_after"),
        out_after: optional_path(input, "out_after"),
    };
    let outcome = ops::garment_fit(&params)?;
    let follow = outcome.follow.map(|(body, garment, difference)| {
        json!({ "body": body, "garment": garment, "difference": difference })
    });
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "vertices": outcome.vertices,
        "garment": outcome.garment,
        "reference": outcome.reference,
        "offset_mean": outcome.offset_mean,
        "offset_max": outcome.offset_max,
        "follow": follow,
    }))
}

fn export_obj_handler(input: &Value) -> Result<Value, CommandError> {
    let params = ops::ExportObjParams {
        nif: path(input, "nif")?,
        shape: optional_string(input, "shape"),
        list: input.get("list").and_then(Value::as_bool).unwrap_or(false),
        out: optional_path(input, "out"),
    };
    match ops::export_obj(&params)? {
        ops::ExportObjOutcome::Listed(rows) => {
            let shapes: Vec<Value> = rows
                .iter()
                .map(|row| {
                    json!({
                        "name": row.name,
                        "vertices": row.vertices,
                        "triangles": row.triangles,
                    })
                })
                .collect();
            Ok(json!({ "shapes": shapes }))
        }
        ops::ExportObjOutcome::Written {
            out,
            shape,
            vertices,
            triangles,
        } => Ok(json!({
            "out": out.display().to_string(),
            "shape": shape,
            "vertices": vertices,
            "triangles": triangles,
        })),
    }
}
