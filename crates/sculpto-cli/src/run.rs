//! `sculpto run` and `sculpto commands`: transports over the engine registry.
//!
//! The wire contract is one JSON envelope per invocation, on stdout. Exit codes
//! are part of the contract: `0` success, `2` an invalid request, `1` a domain
//! refusal or an environment failure. The split mirrors the argv surface, where
//! usage errors already exit `2`.
//!
//! There is no handler table here. The registry owns the handlers, so
//! `sculpto run --cmd <id>` and a programmatic `sculpto_engine::dispatch` take
//! the same path by construction.

use std::process::ExitCode;

use sculpto_engine::{CommandError, ErrorKind, catalogue, dispatch, envelope};
use serde_json::{Value, json};

const USAGE: &str = "\
usage: sculpto run --cmd <id> [--input <json|->]
       sculpto commands [--json]

run options:
  --cmd <id>        command id from `sculpto commands`
  --input <json>    JSON input; `-` reads stdin; defaults to null

commands options:
  --json            print the catalogue as JSON instead of a table
";

fn reject(code: &'static str, message: impl Into<String>) -> ExitCode {
    let error = CommandError::new(ErrorKind::InvalidRequest, code, message);
    println!("{}", envelope(Err(error)));
    ExitCode::from(2)
}

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
                    "implemented": spec.implemented(),
                    "input_schema": spec.input_schema(),
                })
            })
            .collect();
        println!("{}", json!({ "commands": rows }));
        return ExitCode::SUCCESS;
    }
    for spec in catalogue() {
        let marker = if spec.implemented() { " " } else { "*" };
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

    dispatch_payload(&command, &payload)
}

pub(crate) fn dispatch_payload(command: &str, payload: &Value) -> ExitCode {
    let result = dispatch(command, payload);
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
