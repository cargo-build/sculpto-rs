//! Author joint-parented sections on an explicit A-pose material reference.
use std::{collections::HashMap, process::ExitCode};

const USAGE: &str = "sculpto actor hybrid --actor input.glb --reference a-pose.glb [--pelvis-donor native-a.glb] [--mark-transitions] --out result.glb [--report result.json]";

pub(super) fn run(args: &[String]) -> ExitCode {
    match job(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("actor hybrid: {e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
fn job(args: &[String]) -> Result<(), String> {
    let mut args = args.iter().map(String::as_str);
    let mut flags = HashMap::new();
    let mut mark = false;
    while let Some(k) = args.next() {
        if k == "--mark-transitions" && !mark {
            mark = true;
            continue;
        }
        if !matches!(
            k,
            "--actor" | "--reference" | "--pelvis-donor" | "--out" | "--report"
        ) {
            return Err(format!("unexpected argument {k}"));
        }
        let v = args.next().ok_or_else(|| format!("{k} needs a value"))?;
        if flags.insert(k, v).is_some() {
            return Err(format!("duplicate {k}"));
        }
    }
    let path = |key| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("{key} is required"))
    };
    let read = |p: &str| std::fs::read(p).map_err(|e| format!("{p}: {e}"));
    let input = read(path("--actor")?)?;
    let reference = read(path("--reference")?)?;
    let donor = flags.get("--pelvis-donor").copied().map(read).transpose()?;
    let out = path("--out")?;
    let (bytes, report) =
        sculpto_engine::hybrid::prepare(&input, &reference, donor.as_deref(), mark)
            .map_err(|e| e.to_string())?;
    let mut json = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    json.push(b'\n');
    super::pack::write_outputs(out, &bytes, flags.get("--report").copied(), &json)?;
    println!("Wrote {out}, {} bytes", bytes.len());
    Ok(())
}
