//! Read-only body-part socket audit over OBJ files; nothing is written and no
//! placement changes. The measured thresholds live in [`sculpto_mesh::parts`].

use std::path::PathBuf;

use crate::obj::read_obj_surface;
use crate::{CommandError, ErrorKind};
use schemars::JsonSchema;
use sculpto_mesh::parts::{self, PartOverride};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    /// Multi-part body OBJ: the torso shell plus its attached part shells, as
    /// shipped.
    body: PathBuf,
    /// Replacement part OBJs audited against the body's own sockets; each file
    /// is one part and the shipped copy stays the measurement baseline.
    parts: Option<Vec<PathBuf>>,
    /// Refuse (the default) or report: with `strict: false` the command
    /// returns the full per-part report with `verdict: "refuse"` and the
    /// typed `refusals` list instead of an error, so the measured rows stay
    /// reachable when a part fails.
    strict: Option<bool>,
}

pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(Params))
}

pub(crate) fn audit(input: &Value) -> Result<Value, CommandError> {
    let params: Params = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(ErrorKind::InvalidParams, "parts_input", error.to_string())
    })?;
    let (positions, triangles) = read_obj_surface(&params.body)?;
    let mut files = Vec::new();
    for file in params.parts.iter().flatten() {
        files.push(read_obj_surface(file)?);
    }
    let overrides: Vec<PartOverride<'_>> = files
        .iter()
        .map(|(positions, triangles)| PartOverride {
            positions,
            triangles,
        })
        .collect();
    let report = parts::audit(&positions, &triangles, &overrides).map_err(parts_error)?;
    let strict = params.strict.unwrap_or(true);
    if report.refusals.is_empty() {
        return Ok(render(&report));
    }
    if !strict {
        // The measured rows and every typed refusal stay reachable exactly
        // when a part fails; the verdict says so.
        return Ok(render(&report));
    }
    let message = report
        .refusals
        .iter()
        .map(|refusal| refusal.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    let first = report.refusals.first().expect("refusals checked");
    Err(CommandError::new(
        ErrorKind::Refused,
        first.code.wire_code(),
        message,
    ))
}

fn parts_error(error: parts::PartsError) -> CommandError {
    let (kind, code) = match &error {
        parts::PartsError::EmptyInput => (ErrorKind::InvalidRequest, "parts_empty"),
        parts::PartsError::NonFinite { .. } => (ErrorKind::InvalidParams, "parts_nonfinite"),
        parts::PartsError::BadIndex { .. } => (ErrorKind::InvalidParams, "parts_bad_index"),
        parts::PartsError::UnmatchedOverride { .. } => {
            (ErrorKind::InvalidParams, "part_override_unmatched")
        }
        parts::PartsError::OverrideConflict { .. } => {
            (ErrorKind::InvalidParams, "part_override_conflict")
        }
    };
    CommandError::new(kind, code, error.to_string())
}

/// The full report; with `strict: false` it is also the refusal payload, so
/// `verdict` distinguishes accept from refuse and `refusals` carries the
/// typed entries.
fn render(report: &parts::PartAudit) -> Value {
    let refusals: Vec<Value> = report
        .refusals
        .iter()
        .map(|refusal| {
            json!({
                "code": refusal.code.wire_code(),
                "part": refusal.part,
                "socket": refusal.socket,
                "observed": refusal.observed,
                "bound": refusal.bound,
                "message": refusal.to_string(),
            })
        })
        .collect();
    let parts: Vec<Value> = report
        .parts
        .iter()
        .map(|part| {
            let seams: Vec<Value> = part
                .seams
                .iter()
                .map(|seam| {
                    json!({
                        "socket": seam.socket,
                        "kind": seam.kind.name(),
                        "ring": seam.ring,
                        "socket_ring": seam.socket_ring,
                        "slot_min": seam.slot_min,
                        "ring_min": seam.ring_min,
                        "ring_max": seam.ring_max,
                        "coincident": seam.coincident,
                        "near": seam.near,
                        "coverage": seam.coverage,
                        "offset": seam.offset,
                    })
                })
                .collect();
            json!({
                "name": part.name,
                "class": part.class.name(),
                "vertices": part.vertices,
                "override": part.overridden,
                "seams": seams,
                "verdict": part.verdict,
                "note": part.note,
            })
        })
        .collect();
    let open_sockets: Vec<Value> = report
        .open_sockets
        .iter()
        .map(|socket| {
            json!({
                "component": socket.component,
                "ring": socket.ring,
                "centroid": socket.centroid,
                "note": "open socket: no part shipped",
            })
        })
        .collect();
    json!({
        "components": report.components,
        "shells": report.shells,
        "torso": report.torso,
        "parts": parts,
        "open_sockets": open_sockets,
        "refusals": refusals,
        "verdict": report.verdict(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_and_missing_body_are_invalid_params() {
        let error = audit(&json!({ "body": "x.obj", "typo": 1 })).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidParams);
        assert_eq!(error.code(), "parts_input");
        let error = audit(&json!({})).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidParams);
    }
}
