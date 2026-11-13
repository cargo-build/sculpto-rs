//! Pure component constraints shared by JSON and MCP; no source asset is mutated.

use crate::{CommandError, ErrorKind};
use schemars::JsonSchema;
use sculpto_mesh::seam::{SeamAnchor, SeamError, SeamOptions, fit_seam};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Anchor {
    vertex: usize,
    target: [f32; 3],
}

/// Required lengths use source-point units; no frame is inferred. @req DECL0007
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Params {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    anchors: Vec<Anchor>,
    collar_radius: f32,
    max_shift: f32,
}

pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(Params))
}

pub(crate) fn fit(input: &Value) -> Result<Value, CommandError> {
    let params: Params = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "component_seam_input",
            error.to_string(),
        )
    })?;
    let anchors: Vec<_> = params
        .anchors
        .iter()
        .map(|anchor| SeamAnchor {
            vertex: anchor.vertex,
            target: anchor.target,
        })
        .collect();
    let options = SeamOptions {
        collar_radius: params.collar_radius,
        max_shift: params.max_shift,
    };
    let fit =
        fit_seam(&params.positions, &params.triangles, &anchors, &options).map_err(|error| {
            let (kind, code) = match error {
                SeamError::ConflictingTargets { .. } => {
                    (ErrorKind::Refused, "component_seam_conflict")
                }
                SeamError::ShiftLimit { .. } => (ErrorKind::Refused, "component_seam_shift_limit"),
                SeamError::NonFiniteOutput { .. } => {
                    (ErrorKind::Refused, "component_seam_nonfinite_output")
                }
                SeamError::InvalidOptions
                | SeamError::EmptyInput
                | SeamError::NonFinite { .. }
                | SeamError::InvalidIndex { .. } => {
                    (ErrorKind::InvalidParams, "component_seam_input")
                }
            };
            CommandError::new(kind, code, error.to_string())
        })?;
    Ok(
        json!({ "positions": fit.positions, "changed_vertices": fit.changed_vertices, "max_shift": fit.max_shift }),
    )
}
