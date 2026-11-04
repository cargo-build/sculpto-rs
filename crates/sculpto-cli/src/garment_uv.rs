//! Thin JSON/IO boundary for the shared UV atlas qualifier. @req DECL0013

use std::fs::{self, OpenOptions};
use std::path::PathBuf;

use sculpto_mesh::uv;
use serde_json::{Value, json};

use crate::{parse_flags, required};

pub fn check(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    if let Some(key) = flags
        .keys()
        .find(|key| !matches!(key.as_str(), "uv" | "report"))
    {
        return Err(format!("unknown uv-check flag `--{key}`"));
    }
    let input = PathBuf::from(required(&flags, "uv")?);
    let output = PathBuf::from(required(&flags, "report")?);
    let bytes = fs::read(&input).map_err(|error| format!("{}: {error}", input.display()))?;
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{}: invalid UV JSON: {error}", input.display()))?;
    let triangles: Vec<[[f32; 2]; 3]> = serde_json::from_value(
        value
            .get_mut("triangles")
            .map(Value::take)
            .ok_or_else(|| "UV JSON requires `triangles`".to_string())?,
    )
    .map_err(|error| format!("{}: invalid UV triangles: {error}", input.display()))?;
    let report =
        uv::qualify(&triangles).map_err(|error| format!("{}: {error}", input.display()))?;
    let document = json!({
        "triangles": triangles.len(),
        "degenerate_triangles": report.degenerate_triangles,
        "overlap_pairs": report.overlap_pairs,
        "out_of_bounds_corners": report.out_of_bounds_corners,
        "verdict": {"passed": report.is_valid()},
    });
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(|error| format!("{}: {error}", output.display()))?;
    serde_json::to_writer_pretty(file, &document)
        .map_err(|error| format!("{}: {error}", output.display()))?;
    match report.is_valid() {
        true => Ok(()),
        false => Err(format!(
            "UV atlas failed: {} degenerate triangles, {} positive-area pairs, {} out-of-bounds corners; report {}",
            report.degenerate_triangles.len(),
            report.overlap_pairs.len(),
            report.out_of_bounds_corners.len(),
            output.display()
        )),
    }
}
