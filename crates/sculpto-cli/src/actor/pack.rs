//! Paths and schedule serialization for the core cloth packer. @req DECL0013

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::process::ExitCode;

use sculpto_mesh::actor::{ClothPacking, pack_cloth};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor pack-cloth --input actor.glb --targets targets.glb \
    --node NAME --clip NAME --schedule schedule.json --out result.glb [--report result.json]";

struct Arguments<'arg> {
    input: &'arg str,
    targets: &'arg str,
    node: &'arg str,
    clip: &'arg str,
    schedule: &'arg str,
    output: &'arg str,
    report: Option<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor pack-cloth: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match pack(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor pack-cloth: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags = HashMap::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--input" | "--targets" | "--node" | "--clip" | "--schedule" | "--out" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let required = |key| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("{key} is required"))
    };
    let arguments = Arguments {
        input: required("--input")?,
        targets: required("--targets")?,
        node: required("--node")?,
        clip: required("--clip")?,
        schedule: required("--schedule")?,
        output: required("--out")?,
        report: flags.get("--report").copied(),
    };
    match arguments.report == Some(arguments.output) {
        true => Err("output and report paths must differ".to_owned()),
        false => Ok(arguments),
    }
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{path}: {error}"))
}

fn pack(arguments: Arguments<'_>) -> Result<(), String> {
    let schedule: Value = serde_json::from_slice(&read(arguments.schedule)?)
        .map_err(|error| format!("{}: {error}", arguments.schedule))?;
    let times = schedule
        .get("time_seconds")
        .and_then(Value::as_array)
        .ok_or("schedule requires a time_seconds array")?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| "schedule time_seconds requires numbers".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let packed = pack_cloth(
        &read(arguments.input)?,
        &read(arguments.targets)?,
        ClothPacking::new(arguments.node, arguments.clip, &times),
    )
    .map_err(|error| error.to_string())?;
    let report = json!({
        "input": arguments.input, "targets": arguments.targets, "schedule": arguments.schedule,
        "node_name": arguments.node, "node": packed.node, "mesh": packed.mesh,
        "clip_name": arguments.clip, "clip": packed.clip, "primitives": packed.primitives,
        "targets_count": packed.time_seconds.len(), "time_seconds": packed.time_seconds,
        "glb_bytes": packed.glb.len(), "original_binary_prefix_unchanged": true,
        "skeletal_channels_unchanged": true, "cloth_uv_indices_unchanged": true,
        "scope": "Baked cloth packing only; collision, fit and loop qualification remain separate",
    });
    let mut report_bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    report_bytes.push(b'\n');
    write_outputs(
        arguments.output,
        &packed.glb,
        arguments.report,
        &report_bytes,
    )?;
    println!(
        "actor pack-cloth: {} targets, {} primitives, {} bytes",
        packed.time_seconds.len(),
        packed.primitives,
        packed.glb.len()
    );
    Ok(())
}

fn create(path: &str) -> Result<File, String> {
    File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("{path}: {error}"))
}

fn write_outputs(
    output_path: &str,
    bytes: &[u8],
    report_path: Option<&str>,
    report_bytes: &[u8],
) -> Result<(), String> {
    let mut output = create(output_path)?;
    let mut created_report = false;
    let result = (|| {
        let mut report = match report_path {
            Some(path) => {
                let report = create(path)?;
                created_report = true;
                Some(report)
            }
            None => None,
        };
        output
            .write_all(bytes)
            .and_then(|()| output.flush())
            .map_err(|error| format!("{output_path}: {error}"))?;
        if let Some(file) = report.as_mut() {
            file.write_all(report_bytes)
                .and_then(|()| file.flush())
                .map_err(|error| format!("report: {error}"))?;
        }
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(mut error) => {
            drop(output);
            let paths = std::iter::once(output_path).chain(report_path.filter(|_| created_report));
            paths.for_each(|path| match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(cleanup) => error.push_str(&format!("; cleanup {path}: {cleanup}")),
            });
            Err(error)
        }
    }
}
