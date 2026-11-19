//! Argv adapter for `body.neutralize`.

use std::process::ExitCode;

use serde_json::{Map, Value, json};

use crate::run;

const USAGE: &str = "\
usage: sculpto body neutralize --project <osp> --presets <xml> --out <dir>
                              [--preset <name>]... [--endpoint both|small|big]
                              [--set <name>] [--shape <name>] [--home <dir>]

Repeat --project and --presets for multiple input files. Omit --preset to
select every entry in the supplied collections. Each preset's declared set
is used by default. Output is a JSON report with processed and skipped rows.
";

pub(crate) fn run(args: &[String]) -> ExitCode {
    if matches!(args, [help] if matches!(help.as_str(), "--help" | "-h")) {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match input(args) {
        Ok(input) => run::dispatch_payload("body.neutralize", &input),
        Err(error) => {
            eprintln!("body neutralize: {error}");
            ExitCode::from(2)
        }
    }
}

fn input(args: &[String]) -> Result<Value, String> {
    let mut input = Map::new();
    let mut projects = Vec::new();
    let mut presets = Vec::new();
    let mut names = Vec::new();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        let key = flag
            .strip_prefix("--")
            .ok_or_else(|| format!("unexpected argument `{flag}`"))?;
        let value = args
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match key {
            "project" => projects.push(value),
            "presets" => presets.push(value),
            "preset" => names.push(value),
            "endpoint" | "set" | "shape" | "home" | "out" => {
                if input.insert(key.into(), json!(value)).is_some() {
                    return Err(format!("`{flag}` may only appear once"));
                }
            }
            _ => return Err(format!("unknown option `{flag}`")),
        }
    }
    input.insert("projects".into(), json!(projects));
    input.insert("presets".into(), json!(presets));
    input.insert("names".into(), json!(names));
    Ok(Value::Object(input))
}
