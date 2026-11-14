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

/// Schema derived from the typed `outfit.stack_fit` parameters.
pub fn stack_fit_schema() -> Value {
    schema_for::<ops::StackFitParams>()
}

/// Schema derived from the typed `outfit.dress` parameters.
pub fn dress_schema() -> Value {
    schema_for::<ops::DressParams>()
}

/// Schema for the authored garment slider conformance command.
pub fn conform_schema() -> Value {
    schema_for::<ops::ConformParams>()
}

/// Schema for the outfit census and cloth selection.
pub fn outfit_inspect_schema() -> Value {
    schema_for::<ops::OutfitInspectParams>()
}

/// Schema for the outfit completeness gate.
pub fn outfit_check_schema() -> Value {
    schema_for::<ops::OutfitCheckParams>()
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

/// Schema derived from the typed `export.bundle` parameters.
pub fn export_bundle_schema() -> Value {
    schema_for::<ops::ExportBundleParams>()
}

/// Schema derived from the typed `export.tri` parameters.
pub fn export_tri_schema() -> Value {
    schema_for::<ops::ExportTriParams>()
}

/// Schema derived from the typed GLB repair parameters.
pub fn glb_file_schema() -> Value {
    schema_for::<ops::GlbFileParams>()
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialises")
}

pub fn version(_input: &Value) -> Result<Value, CommandError> {
    Ok(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

pub(crate) fn missing(field: &str) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "missing_field",
        format!("`{field}` is required"),
    )
}

/// An optional finite number; a present but unusable value is refused, never
/// dropped, so a typo cannot silently fall back to a default.
fn optional_f32(input: &Value, field: &str) -> Result<Option<f32>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| value.is_finite())
            .map(Some)
            .ok_or_else(|| errors::invalid(field, "must be a finite number")),
    }
}

/// An optional non-negative whole number.
fn optional_u32(input: &Value, field: &str) -> Result<Option<u32>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| errors::invalid(field, "must be a nonnegative integer")),
    }
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

pub(crate) fn string(input: &Value, field: &str) -> Result<String, CommandError> {
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

fn checked_optional_path(input: &Value, field: &str) -> Result<Option<PathBuf>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) if !path.trim().is_empty() => Ok(Some(PathBuf::from(path))),
        Some(_) => Err(errors::invalid(field, "must be a nonempty path")),
    }
}

pub fn outfit_inspect(input: &Value) -> Result<Value, CommandError> {
    let manifest = checked_optional_path(input, "manifest")?;
    let texture_roots = string_list(input, "texture_roots")?
        .map(|values| values.into_iter().map(PathBuf::from).collect())
        .unwrap_or_default();
    let archives = string_list(input, "archives")?
        .map(|values| values.into_iter().map(PathBuf::from).collect())
        .unwrap_or_default();
    let outcome = ops::inspect_outfit(&ops::OutfitInspectParams {
        nif: path(input, "nif")?,
        manifest,
        texture_roots,
        archives,
    })?;
    Ok(json!({
        "outfit": outcome.outfit,
        "shapes": outcome.shapes,
        "manifest_errors": [],
        "textures": outcome.textures,
    }))
}

/// The complete verdict is constant on the success path: a partial outfit
/// returns the typed `outfit_incomplete` refusal, never an `Ok` verdict.
pub fn outfit_check(input: &Value) -> Result<Value, CommandError> {
    let outcome = ops::check_outfit(&ops::OutfitCheckParams {
        nif: path(input, "nif")?,
        manifest: path(input, "manifest")?,
    })?;
    let shapes: Vec<Value> = outcome
        .shapes
        .iter()
        .map(|shape| {
            json!({
                "name": shape.name,
                "shape_index": shape.shape_index,
                "vertices": shape.vertices,
                "triangles": shape.triangles,
                "requires_material": shape.requires_material,
                "has_diffuse": shape.has_diffuse,
            })
        })
        .collect();
    Ok(json!({
        "outfit": outcome.outfit,
        "verdict": "complete",
        "shapes": shapes,
    }))
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
    let percent = match input.get("percent") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .map(|value| value as f32)
                .filter(|value| value.is_finite())
                .ok_or_else(|| errors::invalid("percent", "must be a finite number"))?,
        ),
    };
    let slider = match input.get("slider") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if !name.trim().is_empty() => Some(name.clone()),
        Some(_) => return Err(errors::invalid("slider", "must be a nonempty name")),
    };
    let collision_frame = match input.get("collision_frame") {
        None | Some(Value::Null) => None,
        Some(Value::String(frame)) if !frame.trim().is_empty() => Some(frame.clone()),
        Some(_) => {
            return Err(errors::invalid(
                "collision_frame",
                "must be a nonempty frame name",
            ));
        }
    };
    let final_push = match input.get("final_push") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|passes| u32::try_from(passes).ok())
                .ok_or_else(|| errors::invalid("final_push", "must be a nonnegative integer"))?,
        ),
    };
    let final_push_tolerance = match input.get("final_push_tolerance") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .map(|value| value as f32)
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    errors::invalid("final_push_tolerance", "must be a finite number")
                })?,
        ),
    };
    let params = ops::FitParams {
        nif: path(input, "nif")?,
        shape: string(input, "shape")?,
        body_shape: optional_string(input, "body_shape"),
        body: path(input, "body")?,
        out: path(input, "out")?,
        body_ref: checked_optional_path(input, "body_ref")?,
        body_after: optional_path(input, "body_after"),
        out_after: optional_path(input, "out_after"),
        conform: checked_optional_path(input, "conform")?,
        slider,
        percent,
        body_nif: checked_optional_path(input, "body_nif")?,
        manifest: checked_optional_path(input, "manifest")?,
        collision: checked_optional_path(input, "collision")?,
        collision_frame,
        final_push,
        final_push_tolerance,
    };
    let outcome = ops::garment_fit(&params)?;
    let follow = outcome.follow.map(|(body, garment, difference)| {
        json!({ "body": body, "garment": garment, "difference": difference })
    });
    let mut result = json!({
        "out": outcome.out.display().to_string(),
        "vertices": outcome.vertices,
        "garment": outcome.garment,
        "reference": outcome.reference,
        "offset_mean": outcome.offset_mean,
        "offset_max": outcome.offset_max,
        "follow": follow,
    });
    if let Some(pairs) = outcome.intersection_pairs {
        result["intersection_pairs"] = json!(pairs);
    }
    if let Some(inside) = outcome.inside_after {
        result["inside_after"] = json!(inside);
    }
    if let Some(regions) = outcome.regions {
        result["regions"] = json!(regions);
    }
    if let Some(repair) = outcome.repair {
        result["repair"] = json!(repair);
    }
    if let Some(collision) = outcome.collision {
        result["collision"] = json!(collision);
    }
    if let Some(provenance) = outcome.provenance {
        result["reference_source"] = json!(provenance.reference_source);
        result["reference_path"] = json!(provenance.reference_path.display().to_string());
        result["reference_body_sha256"] = json!(provenance.reference_body_sha256);
        result["reference_vertices"] = json!(provenance.reference_vertices);
        result["reference_triangles"] = json!(provenance.reference_triangles);
        result["target_path"] = json!(provenance.target_path.display().to_string());
        result["target_body_sha256"] = json!(provenance.target_body_sha256);
        result["target_vertices"] = json!(provenance.target_vertices);
        result["target_triangles"] = json!(provenance.target_triangles);
        result["transport_method"] = json!(provenance.transport_method);
        result["normal_offset_range"] = json!(provenance.normal_offset_range);
    }
    Ok(result)
}

pub fn stack_fit(input: &Value) -> Result<Value, CommandError> {
    let tolerance = match input.get("tolerance") {
        None | Some(Value::Null) => {
            return Err(missing("tolerance"));
        }
        Some(value) => value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| value.is_finite())
            .ok_or_else(|| errors::invalid("tolerance", "must be a finite number"))?,
    };
    let passes = match input.get("passes") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|passes| u32::try_from(passes).ok())
                .filter(|passes| *passes > 0)
                .ok_or_else(|| errors::invalid("passes", "must be a positive integer"))?,
        ),
    };
    let params = ops::StackFitParams {
        manifest: path(input, "manifest")?,
        tolerance,
        body: checked_optional_path(input, "body")?,
        passes,
    };
    Ok(ops::stack_fit(&params)?.to_json())
}

pub fn dress(input: &Value) -> Result<Value, CommandError> {
    let params = ops::DressParams {
        body: path(input, "body")?,
        garment: path(input, "garment")?,
        out: path(input, "out")?,
        clearance: optional_f32(input, "clearance")?,
        contact: optional_f32(input, "contact")?,
        cuts: optional_u32(input, "cuts")?,
        iterations: optional_u32(input, "iterations")?,
        max_step: optional_f32(input, "max_step")?,
        relax: optional_f32(input, "relax")?,
        rounds: optional_u32(input, "rounds")?,
        poke_passes: optional_u32(input, "poke_passes")?,
        tolerance: optional_f32(input, "tolerance")?,
        passes: optional_u32(input, "passes")?,
        report: checked_optional_path(input, "report")?,
    };
    Ok(ops::dress_garment(&params)?.to_json())
}

pub fn conform(input: &Value) -> Result<Value, CommandError> {
    let outcome = ops::conform_sliders(&ops::ConformParams {
        nif: path(input, "nif")?,
        shape: string(input, "shape")?,
        body_nif: checked_optional_path(input, "body_nif")?,
        body_shape: string(input, "body_shape")?,
        project: path(input, "project")?,
        presets: path(input, "presets")?,
        preset: string(input, "preset")?,
        set: string(input, "set")?,
        home: checked_optional_path(input, "home")?,
        out: path(input, "out")?,
    })?;
    let deferred: Vec<_> = outcome
        .deferred
        .into_iter()
        .map(|slider| {
            let reason = match slider.reason {
                sculpto_bake::DeferredReason::Uv => "uv",
                sculpto_bake::DeferredReason::ClampSemantics => "clamp_semantics",
            };
            json!({ "name": slider.name, "reason": reason })
        })
        .collect();
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "vertices": outcome.vertices,
        "sliders": outcome.sliders,
        "deferred": deferred,
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

fn dropped_influences(drops: &[sculpto_nif::write::DroppedInfluence]) -> Vec<Value> {
    drops
        .iter()
        .map(|drop| json!({ "vertex": drop.vertex, "bone": drop.bone, "weight": drop.weight }))
        .collect()
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
        rig_template: checked_optional_path(input, "rig_template")?,
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
        "dropped_influences_0": dropped_influences(&outcome.dropped_influences[0]),
        "dropped_influences_1": dropped_influences(&outcome.dropped_influences[1]),
        "zap_0": outcome.zap[0].as_str(),
        "zap_1": outcome.zap[1].as_str(),
    }))
}

pub fn export_nif(input: &Value) -> Result<Value, CommandError> {
    let params = ops::ExportNifParams {
        nif: path(input, "nif")?,
        out: path(input, "out")?,
        target: ops::ExportTarget::parse(&string(input, "target")?)?,
        shape: optional_string(input, "shape"),
        rig_template: checked_optional_path(input, "rig_template")?,
    };
    let outcome = ops::export_nif(&params)?;
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "target": outcome.target.as_str(),
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
        "bytes": outcome.bytes,
        "dropped_influences": dropped_influences(&outcome.dropped_influences),
    }))
}

pub fn export_bundle(input: &Value) -> Result<Value, CommandError> {
    let params = ops::ExportBundleParams {
        nif: path(input, "nif")?,
        out: path(input, "out")?,
        target: ops::ExportTarget::parse(&string(input, "target")?)?,
        shapes: string_list(input, "shapes")?,
    };
    let outcome = ops::export_nif_bundle(&params)?;
    let shapes: Vec<Value> = outcome
        .shapes
        .iter()
        .map(|shape| {
            json!({
                "name": shape.name,
                "vertices": shape.vertices,
                "triangles": shape.triangles,
            })
        })
        .collect();
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "target": outcome.target.as_str(),
        "shapes": shapes,
        "vertices": outcome.vertices,
        "triangles": outcome.triangles,
        "bytes": outcome.bytes,
    }))
}

/// Parse the nested `export.tri` document through its typed parameters: the
/// morph list has two levels, so a field-by-field reader would duplicate the
/// same three shape rules.
pub fn export_tri(input: &Value) -> Result<Value, CommandError> {
    let params: ops::ExportTriParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "export_tri_input",
            error.to_string(),
        )
    })?;
    let outcome = ops::export_tri(&params)?;
    Ok(json!({
        "out": outcome.out.display().to_string(),
        "shape": outcome.shape,
        "vertex_count": outcome.vertex_count,
        "morphs": outcome.morphs,
        "offsets": outcome.offsets,
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

pub fn gltf_reset_weights(input: &Value) -> Result<Value, CommandError> {
    let params = ops::GlbFileParams {
        glb: path(input, "glb")?,
    };
    let outcome = ops::gltf_reset_weights(&params)?;
    Ok(json!({
        "glb": outcome.glb.display().to_string(),
        "meshes": outcome.changed,
        "bytes": outcome.bytes,
    }))
}

pub fn gltf_clamp_texcoord(input: &Value) -> Result<Value, CommandError> {
    let params = ops::GlbFileParams {
        glb: path(input, "glb")?,
    };
    let outcome = ops::gltf_clamp_texcoord(&params)?;
    Ok(json!({
        "glb": outcome.glb.display().to_string(),
        "texcoords": outcome.changed,
        "bytes": outcome.bytes,
    }))
}
