//! Typed command handlers. Every surface reaches these through the registry in
//! [`crate::dispatch`]; the JSON body is parsed here and the parameter struct is
//! the same one the argv surface builds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde_json::{Value, json};

use crate::errors;
use crate::ops;
use crate::{CommandError, ErrorKind};

/// Parameters of commands that take no input.
#[derive(Debug, Clone, JsonSchema)]
pub struct EmptyParams {}

/// Schema of a command with no input.
pub fn empty_schema() -> Value {
    schema_for::<EmptyParams>()
}

/// Schema derived from the typed `bake` parameters.
pub fn bake_schema() -> Value {
    schema_for::<ops::BakeParams>()
}

/// Schema derived from the typed `fit` parameters.
pub fn fit_schema() -> Value {
    schema_for::<ops::FitParams>()
}

/// Schema derived from the typed `export.obj` parameters.
pub fn export_obj_schema() -> Value {
    schema_for::<ops::ExportObjParams>()
}

/// Schema derived from the typed `export.pair` parameters.
pub fn export_pair_schema() -> Value {
    schema_for::<ops::ExportPairParams>()
}

/// Schema derived from the typed `intake.check` parameters.
pub fn intake_check_schema() -> Value {
    schema_for::<ops::IntakeCheckParams>()
}

/// Schema derived from the typed `export.nif` parameters.
pub fn export_nif_schema() -> Value {
    schema_for::<ops::ExportNifParams>()
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialises")
}

pub fn version(_input: &Value) -> Result<Value, CommandError> {
    Ok(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

fn missing(field: &str) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "missing_field",
        format!("`{field}` is required"),
    )
}

fn string_list(input: &Value, field: &str) -> Result<Option<Vec<String>>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(values) => values
            .as_array()
            .ok_or_else(|| errors::invalid(field, "must be an array of strings"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| errors::invalid(field, "must be an array of strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
    }
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

pub fn bake(input: &Value) -> Result<Value, CommandError> {
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

pub fn fit(input: &Value) -> Result<Value, CommandError> {
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

pub fn export_obj(input: &Value) -> Result<Value, CommandError> {
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

pub fn export_pair(input: &Value) -> Result<Value, CommandError> {
    let params = ops::ExportPairParams {
        nif: path(input, "nif")?,
        project: path(input, "project")?,
        presets: path(input, "presets")?,
        preset: string(input, "preset")?,
        set: string(input, "set")?,
        out: path(input, "out")?,
        size: string_list(input, "size")?,
        shape: optional_string(input, "shape"),
        home: optional_path(input, "home"),
    };
    let outcome = ops::export_pair(&params)?;
    let endpoint = |size: sculpto_core::SliderSize| match size {
        sculpto_core::SliderSize::Small => "small",
        sculpto_core::SliderSize::Big => "big",
    };
    Ok(json!({
        "out_0": outcome.out_0.display().to_string(),
        "out_1": outcome.out_1.display().to_string(),
        "endpoint_0": endpoint(outcome.size_0),
        "endpoint_1": endpoint(outcome.size_1),
        "target": outcome.target,
        "shape": outcome.shape,
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
    }))
}

pub fn export_nif(input: &Value) -> Result<Value, CommandError> {
    let params = ops::ExportNifParams {
        nif: path(input, "nif")?,
        out: path(input, "out")?,
        target: ops::ExportTarget::parse(&string(input, "target")?)?,
        shape: optional_string(input, "shape"),
    };
    let outcome = ops::export_nif(&params)?;
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "target": outcome.target.as_str(),
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
        "bytes": outcome.bytes,
    }))
}

pub fn intake_check(input: &Value) -> Result<Value, CommandError> {
    let params = ops::IntakeCheckParams {
        file: path(input, "file")?,
    };
    let outcome = ops::intake_check(&params)?;
    let counts = &outcome.counts;
    let limits = &outcome.limits;
    Ok(json!({
        "name": outcome.name,
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
        "counts": {
            "non_manifold_edges": counts.non_manifold_edges,
            "degenerate_triangles": counts.degenerate_triangles,
            "duplicate_positions": counts.duplicate_positions,
            "missing_uvs": counts.missing_uvs,
            "open_boundary_edges": counts.open_boundary_edges,
        },
        "limits": {
            "non_manifold_edges": limits.non_manifold_edges,
            "degenerate_triangles": limits.degenerate_triangles,
            "duplicate_positions": limits.duplicate_positions,
            "missing_uvs": limits.missing_uvs,
            "open_boundary_edges": limits.open_boundary_edges,
        },
        "verdict": outcome.verdict,
    }))
}
