//! Export the weighted control or the pose-corrected actor for a matched comparison.

use std::collections::HashMap;
use std::process::ExitCode;

const USAGE: &str = "sculpto actor knee-fold --actor input.glb --out result.glb [--weights-only] [--report result.json]";

pub(super) fn run(args: &[String]) -> ExitCode {
    match job(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor knee-fold: {error}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn job(args: &[String]) -> Result<(), String> {
    let mut args = args.iter().map(String::as_str);
    let mut flags = HashMap::new();
    let mut shapes = true;
    while let Some(key) = args.next() {
        match key {
            "--weights-only" if shapes => shapes = false,
            "--actor" | "--out" | "--report" => {
                let val = args.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, val).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key}")),
        }
    }
    let input = flags.get("--actor").ok_or("--actor is required")?;
    let output = flags.get("--out").ok_or("--out is required")?;
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let (glb, report) =
        sculpto_engine::knee_fold::prepare(&bytes, shapes).map_err(|e| e.to_string())?;
    let mut json = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    json.push(b'\n');
    super::pack::write_outputs(output, &glb, flags.get("--report").copied(), &json)?;
    println!("Wrote {output}, {} bytes", glb.len());
    Ok(())
}
