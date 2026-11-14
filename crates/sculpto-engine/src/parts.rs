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
    if let Some(refusal) = report.refusals.first() {
        let extra = report.refusals.len() - 1;
        let message = match extra {
            0 => refusal.to_string(),
            more => format!("{refusal}; +{more} more"),
        };
        return Err(CommandError::new(
            ErrorKind::Refused,
            refusal.code.wire_code(),
            message,
        ));
    }
    Ok(render(&report))
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

/// The success report; a refused audit never reaches this, so the verdict here
/// is always `accept`.
fn render(report: &parts::PartAudit) -> Value {
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
                        "coverage": seam.coverage,
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
