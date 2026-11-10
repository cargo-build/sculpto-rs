//! The five garment rule commands (F03).
//!
//! Each command carries one rule group the Blender add-on used to apply in
//! Python: `outfit.offsets`, `outfit.place`, `outfit.band`,
//! `outfit.projection` and `outfit.weights`. The typed parameter structs are
//! the schema source; the handlers read the same JSON by hand so the argv and
//! MCP surfaces build one shape. The numbers live in
//! [`sculpto_mesh::garment`], never in the adapter ([DECL0002]).
//!
//! [DECL0002]: ../../../doc/rules/0002-DECL-rules-inside-the-core.md

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde_json::{Value, json};

use sculpto_mesh::garment::{
    self, BodyExtent, GarmentKind, Influences, RawExtent, RawTriangle, TransferError,
    TransferOptions,
};
use sculpto_mesh::weight_norm::WeightError;

use crate::errors;
use crate::handlers::{missing, string};
use crate::{CommandError, ErrorKind};

/// The body a garment class is placed against, metres.
#[derive(Debug, Clone, JsonSchema)]
pub struct PlaceBody {
    /// Lowest body vertex, `z`.
    pub min_z: f64,
    /// Highest body vertex, `z`.
    pub max_z: f64,
    /// Front body surface, `y`.
    pub front_y: f64,
    /// Narrowest `x` of the width reference (the rest-pose body).
    pub width_min_x: f64,
    /// Widest `x` of the width reference.
    pub width_max_x: f64,
}

/// The raw asset before placement, world coordinates, metres.
#[derive(Debug, Clone, JsonSchema)]
pub struct PlaceRaw {
    /// Lowest raw `x`.
    pub min_x: f64,
    /// Highest raw `x`.
    pub max_x: f64,
    /// Lowest raw `z`.
    pub min_z: f64,
    /// Highest raw `z`.
    pub max_z: f64,
}

/// Parameters of `outfit.offsets`.
#[derive(Debug, Clone, JsonSchema)]
pub struct OffsetsParams {
    /// Body vertex positions, in the body's own frame.
    pub positions: Vec<[f32; 3]>,
    /// Body polygons, each a list of vertex indices.
    pub polygons: Vec<Vec<u32>>,
    /// Body face indices the region rule selected.
    pub keep: Vec<u32>,
}

/// Parameters of `outfit.place`.
#[derive(Debug, Clone, JsonSchema)]
pub struct PlaceParams {
    /// Panel id: `upper-body`, `trousers` or `footwear`.
    pub kind: String,
    /// Body extent the class is placed against.
    pub body: PlaceBody,
    /// Raw extent before placement.
    pub raw: PlaceRaw,
}

/// Parameters of `outfit.band`.
#[derive(Debug, Clone, JsonSchema)]
pub struct BandParams {
    /// Panel id: `upper-body`, `trousers` or `footwear`.
    pub kind: String,
    /// Body vertex positions, world space.
    pub positions: Vec<[f32; 3]>,
    /// Body polygons, each a list of vertex indices.
    pub polygons: Vec<Vec<u32>>,
    /// Hem line, world `z`.
    pub hem_z: f64,
    /// Collar line, world `z`.
    pub collar_z: f64,
}

/// One raw triangle: corner positions and the UV each corner carries.
#[derive(Debug, Clone, JsonSchema)]
pub struct RawTriangleInput {
    /// Corner positions, world space.
    pub positions: [[f32; 3]; 3],
    /// Corner UVs, parallel to `positions`.
    pub uvs: [[f32; 2]; 3],
}

/// Parameters of `outfit.projection`.
#[derive(Debug, Clone, JsonSchema)]
pub struct ProjectionParams {
    /// One origin per garment loop, world space.
    pub origins: Vec<[f32; 3]>,
    /// Cast direction, world space; the front relief's `-y` when absent.
    pub axis: Option<[f64; 3]>,
    /// Raw triangles to cast against.
    pub raw: Vec<RawTriangleInput>,
}

/// Parameters of `outfit.weights`.
///
/// `stage = transfer` needs `garment_positions`, `body_positions` and
/// `body_weights`; `stage = unpose` needs `positions`, `weights` and
/// `skinning`. The transfer options follow the native bone-weight copy and
/// default to ten units, ten neighbours, no masks, a new unweighted garment,
/// every source bone copied and no spreading.
#[derive(Debug, Clone, JsonSchema)]
pub struct WeightsParams {
    /// `transfer` or `unpose`.
    pub stage: String,
    /// Garment vertex positions, world space.
    pub garment_positions: Option<Vec<[f32; 3]>>,
    /// Body vertex positions, world space.
    pub body_positions: Option<Vec<[f32; 3]>>,
    /// Per-body-vertex influences, bone name to weight.
    pub body_weights: Option<Vec<BTreeMap<String, f32>>>,
    /// Transfer proximity radius; 10 when absent.
    pub radius: Option<f32>,
    /// Body neighbours blended per garment vertex; 10 when absent.
    pub max_results: Option<usize>,
    /// Per-body-vertex mask; a value above zero excludes the vertex.
    pub body_mask: Option<Vec<f32>>,
    /// Per-garment-vertex mask; a value above zero keeps its current weights.
    pub garment_mask: Option<Vec<f32>>,
    /// Current per-garment-vertex influences; absent means a new garment.
    pub current_weights: Option<Vec<BTreeMap<String, f32>>>,
    /// Body bones to copy, in order; absent or empty copies all source bones.
    pub copy: Option<Vec<String>>,
    /// Extra garment bones that absorb the normalization remainder.
    pub normalizable: Option<Vec<String>>,
    /// Spread an unassigned remainder over the normalizable bones.
    pub spread: Option<bool>,
    /// Re-run the native all-normalizable pass after the copy.
    pub normalize: Option<bool>,
    /// Per-garment-vertex positions the unpose restores.
    pub positions: Option<Vec<[f32; 3]>>,
    /// Per-garment-vertex influences the unpose blends.
    pub weights: Option<Vec<BTreeMap<String, f32>>>,
    /// Per-bone skinning matrix, row major.
    pub skinning: Option<BTreeMap<String, [[f32; 4]; 4]>>,
}

/// Schema derived from the typed `outfit.offsets` parameters.
pub fn offsets_schema() -> Value {
    schema_for::<OffsetsParams>()
}

/// Schema derived from the typed `outfit.place` parameters.
pub fn place_schema() -> Value {
    schema_for::<PlaceParams>()
}

/// Schema derived from the typed `outfit.band` parameters.
pub fn band_schema() -> Value {
    schema_for::<BandParams>()
}

/// Schema derived from the typed `outfit.projection` parameters.
pub fn projection_schema() -> Value {
    schema_for::<ProjectionParams>()
}

/// Schema derived from the typed `outfit.weights` parameters.
pub fn weights_schema() -> Value {
    schema_for::<WeightsParams>()
}

fn schema_for<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("schema serialises")
}

fn array<'a>(input: &'a Value, field: &str) -> Result<&'a Vec<Value>, CommandError> {
    input
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| missing(field))
}

fn object<'a>(input: &'a Value, field: &str) -> Result<&'a Value, CommandError> {
    input
        .get(field)
        .filter(|value| value.is_object())
        .ok_or_else(|| missing(field))
}

fn number(value: &Value, field: &str, index: usize) -> Result<f64, CommandError> {
    value
        .as_f64()
        .ok_or_else(|| errors::invalid(field, format!("entry {index} is not a number")))
}

fn field_number(holder: &Value, field: &str, name: &str) -> Result<f64, CommandError> {
    holder.get(name).map_or_else(
        || Err(missing(name)),
        |value| {
            value
                .as_f64()
                .ok_or_else(|| errors::invalid(field, format!("`{name}` is not a number")))
        },
    )
}

fn triples(input: &Value, field: &str) -> Result<Vec<[f32; 3]>, CommandError> {
    array(input, field)?
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let row = row.as_array().filter(|row| row.len() == 3).ok_or_else(|| {
                errors::invalid(field, format!("entry {index} is not three numbers"))
            })?;
            Ok([
                number(&row[0], field, index)? as f32,
                number(&row[1], field, index)? as f32,
                number(&row[2], field, index)? as f32,
            ])
        })
        .collect()
}

fn triple(input: &Value, field: &str) -> Result<[f64; 3], CommandError> {
    let row = array(input, field)?;
    if row.len() != 3 {
        return Err(errors::invalid(field, "three numbers are required"));
    }
    Ok([
        number(&row[0], field, 0)?,
        number(&row[1], field, 1)?,
        number(&row[2], field, 2)?,
    ])
}

fn pairs(input: &Value, field: &str) -> Result<Vec<[f32; 2]>, CommandError> {
    array(input, field)?
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let row = row.as_array().filter(|row| row.len() == 2).ok_or_else(|| {
                errors::invalid(field, format!("entry {index} is not two numbers"))
            })?;
            Ok([
                number(&row[0], field, index)? as f32,
                number(&row[1], field, index)? as f32,
            ])
        })
        .collect()
}

fn polygons(input: &Value, field: &str) -> Result<Vec<Vec<u32>>, CommandError> {
    array(input, field)?
        .iter()
        .enumerate()
        .map(|(entry, row)| {
            let row = row.as_array().filter(|row| row.len() >= 3).ok_or_else(|| {
                errors::invalid(field, format!("entry {entry} has fewer than three indices"))
            })?;
            row.iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or_else(|| {
                            errors::invalid(
                                field,
                                format!("entry {entry} is not a list of indices"),
                            )
                        })
                })
                .collect()
        })
        .collect()
}

fn indices(input: &Value, field: &str) -> Result<Vec<u32>, CommandError> {
    array(input, field)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| errors::invalid(field, format!("entry {index} is not an index")))
        })
        .collect()
}

fn influences(input: &Value, field: &str) -> Result<Vec<Influences>, CommandError> {
    array(input, field)?
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let row = row
                .as_object()
                .ok_or_else(|| errors::invalid(field, format!("entry {index} is not an object")))?;
            row.iter()
                .map(|(bone, weight)| {
                    weight
                        .as_f64()
                        .map(|weight| (bone.clone(), weight as f32))
                        .ok_or_else(|| {
                            errors::invalid(
                                field,
                                format!("entry {index}: `{bone}` is not a number"),
                            )
                        })
                })
                .collect()
        })
        .collect()
}

fn optional_number(input: &Value, field: &str) -> Result<Option<f64>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| errors::invalid(field, "is not a number")),
    }
}

fn optional_count(input: &Value, field: &str) -> Result<Option<usize>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| errors::invalid(field, "is not a non-negative integer")),
    }
}

fn optional_bool(input: &Value, field: &str) -> Result<Option<bool>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| errors::invalid(field, "is not a boolean")),
    }
}

fn optional_numbers(input: &Value, field: &str) -> Result<Option<Vec<f32>>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => array(input, field)?
            .iter()
            .enumerate()
            .map(|(index, value)| number(value, field, index).map(|value| value as f32))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
    }
}

fn optional_strings(input: &Value, field: &str) -> Result<Option<Vec<String>>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(values) => values
            .as_array()
            .ok_or_else(|| errors::invalid(field, "is not an array of strings"))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| errors::invalid(field, "is not an array of strings"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
    }
}

fn optional_influences(
    input: &Value,
    field: &str,
) -> Result<Option<Vec<Influences>>, CommandError> {
    match input.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => influences(input, field).map(Some),
    }
}

fn skinning(input: &Value) -> Result<BTreeMap<String, [[f32; 4]; 4]>, CommandError> {
    object(input, "skinning")?
        .as_object()
        .expect("object")
        .iter()
        .map(|(bone, matrix)| {
            let values = matrix
                .as_array()
                .filter(|values| values.len() == 16)
                .ok_or_else(|| {
                    errors::invalid("skinning", format!("`{bone}` is not a 16-number matrix"))
                })?;
            let mut rows = [[0.0f32; 4]; 4];
            for (index, value) in values.iter().enumerate() {
                rows[index / 4][index % 4] = number(value, "skinning", index)? as f32;
            }
            Ok((bone.clone(), rows))
        })
        .collect()
}

/// Suffix the offset surface and its boundary rings. @req DECL0002
pub fn offsets(input: &Value) -> Result<Value, CommandError> {
    let positions = triples(input, "positions")?;
    let polygons = polygons(input, "polygons")?;
    let keep = indices(input, "keep")?;
    let surface =
        garment::offset_surface(&positions, &polygons, &keep).map_err(errors::garment_rule)?;
    Ok(json!({
        "vertices": surface.vertices,
        "positions": surface.positions,
        "polygons": surface.polygons,
        "offsets": surface.offsets,
        "boundary": surface.boundary,
        "boundary_vertices": surface.boundary.iter().filter(|ring| **ring).count(),
    }))
}

/// Scale and move a raw asset in front of the body. @req DECL0002
pub fn place(input: &Value) -> Result<Value, CommandError> {
    let kind_id = string(input, "kind")?;
    let kind = GarmentKind::from_id(&kind_id)
        .ok_or_else(|| errors::not_found("garment kind", &kind_id))?;
    let body = object(input, "body")?;
    let raw = object(input, "raw")?;
    let placement = garment::place(
        kind,
        BodyExtent {
            min_z: field_number(body, "body", "min_z")?,
            max_z: field_number(body, "body", "max_z")?,
            front_y: field_number(body, "body", "front_y")?,
            width_min_x: field_number(body, "body", "width_min_x")?,
            width_max_x: field_number(body, "body", "width_max_x")?,
        },
        RawExtent {
            min_x: field_number(raw, "raw", "min_x")?,
            max_x: field_number(raw, "raw", "max_x")?,
            min_z: field_number(raw, "raw", "min_z")?,
            max_z: field_number(raw, "raw", "max_z")?,
        },
    )
    .map_err(errors::garment_rule)?;
    Ok(json!({
        "kind": kind.id(),
        "scale": placement.scale,
        "location": placement.location,
        "hem_z": placement.hem_z,
        "collar_z": placement.collar_z,
        "width": placement.width,
    }))
}

/// The band cut: body faces the garment class covers. @req DECL0002
pub fn band(input: &Value) -> Result<Value, CommandError> {
    let kind_id = string(input, "kind")?;
    let kind = GarmentKind::from_id(&kind_id)
        .ok_or_else(|| errors::not_found("garment kind", &kind_id))?;
    let positions = triples(input, "positions")?;
    let polygons = polygons(input, "polygons")?;
    let hem_z = field_number(input, "band", "hem_z")?;
    let collar_z = field_number(input, "band", "collar_z")?;
    let faces = garment::band(
        &positions,
        &polygons,
        hem_z,
        collar_z,
        kind.rule().arm_exclusion,
    );
    Ok(json!({ "faces": faces, "count": faces.len() }))
}

/// Garment UVs by ray-casting the placed raw asset. @req DECL0002
pub fn projection(input: &Value) -> Result<Value, CommandError> {
    let origins = triples(input, "origins")?;
    let axis = match input.get("axis") {
        Some(_) => triple(input, "axis")?,
        None => garment::PROJECT_AXIS,
    };
    let raw: Vec<RawTriangle> = array(input, "raw")?
        .iter()
        .enumerate()
        .map(|(index, triangle)| {
            let positions = triples(triangle, "positions")?;
            let uvs = pairs(triangle, "uvs")?;
            if positions.len() != 3 || uvs.len() != 3 {
                return Err(errors::invalid(
                    "raw",
                    format!("entry {index} is not a triangle"),
                ));
            }
            Ok(RawTriangle {
                positions: [positions[0], positions[1], positions[2]],
                uvs: [uvs[0], uvs[1], uvs[2]],
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let projected = garment::project(&origins, axis, &raw);
    let hits = projected.iter().filter(|loop_uv| loop_uv.hit).count();
    Ok(json!({
        "uvs": projected.iter().map(|loop_uv| loop_uv.uv).collect::<Vec<_>>(),
        "hits": hits,
        "misses": projected.len() - hits,
    }))
}

/// Weight transfer and unpose inversion. @req DECL0002
pub fn weights(input: &Value) -> Result<Value, CommandError> {
    match string(input, "stage")?.as_str() {
        "transfer" => {
            let garment_positions = triples(input, "garment_positions")?;
            let body_positions = triples(input, "body_positions")?;
            let body_weights = influences(input, "body_weights")?;
            let defaults = TransferOptions::default();
            let options = TransferOptions {
                radius: optional_number(input, "radius")?
                    .map_or(defaults.radius, |value| value as f32),
                max_results: optional_count(input, "max_results")?.unwrap_or(defaults.max_results),
                body_mask: optional_numbers(input, "body_mask")?,
                garment_mask: optional_numbers(input, "garment_mask")?,
                current: optional_influences(input, "current_weights")?,
                copy: optional_strings(input, "copy")?.unwrap_or(defaults.copy),
                normalizable: optional_strings(input, "normalizable")?
                    .unwrap_or(defaults.normalizable),
                spread: optional_bool(input, "spread")?.unwrap_or(defaults.spread),
                normalize: optional_bool(input, "normalize")?.unwrap_or(defaults.normalize),
            };
            let transferred =
                garment::transfer(&garment_positions, &body_positions, &body_weights, &options)
                    .map_err(weight_transfer)?;
            Ok(json!({
                "stage": "transfer",
                "vertices": transferred.len(),
                "weights": transferred,
            }))
        }
        "unpose" => {
            let positions = triples(input, "positions")?;
            let weights = influences(input, "weights")?;
            let skinning = skinning(input)?;
            let unposed =
                garment::unpose(&positions, &weights, &skinning).map_err(errors::garment_rule)?;
            Ok(json!({
                "stage": "unpose",
                "vertices": unposed.positions.len(),
                "positions": unposed.positions,
                "moved": unposed.moved,
                "skipped": unposed.skipped,
            }))
        }
        other => Err(CommandError::new(
            ErrorKind::InvalidParams,
            "unknown_weight_stage",
            format!("`stage` must be `transfer` or `unpose`, not `{other}`"),
        )),
    }
}

/// A refused weight copy → the stable `kind` + `code` contract.
fn weight_transfer(error: TransferError) -> CommandError {
    let (kind, code) = match &error {
        TransferError::EmptyGarment => (ErrorKind::InvalidParams, "weight_empty_garment"),
        TransferError::EmptyBody => (ErrorKind::InvalidParams, "weight_empty_body"),
        TransferError::ZeroMaxResults => (ErrorKind::InvalidParams, "weight_zero_max_results"),
        TransferError::LengthMismatch { .. } => {
            (ErrorKind::InvalidParams, "weight_length_mismatch")
        }
        TransferError::Mask { .. } => (ErrorKind::InvalidParams, "weight_mask"),
        TransferError::MissingCurrentWeights { .. } => {
            (ErrorKind::InvalidParams, "weight_missing_current")
        }
        TransferError::InvalidPreservedWeights { .. } => {
            (ErrorKind::Refused, "weight_invalid_preserved")
        }
        TransferError::Influence { error, .. } => weight_class(error),
        TransferError::Proximity(_) => (ErrorKind::InvalidParams, "weight_proximity"),
        TransferError::Normalize(error) => weight_class(error),
        TransferError::Vertex { error, .. } => weight_class(error),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A weight-class or packing refusal → kind and stable code.
fn weight_class(error: &WeightError) -> (ErrorKind, &'static str) {
    match error {
        WeightError::EmptyBoneName => (ErrorKind::InvalidParams, "weight_empty_bone_name"),
        WeightError::DuplicateBone { .. } => (ErrorKind::InvalidParams, "weight_duplicate_bone"),
        WeightError::AmbiguousClass { .. } => (ErrorKind::InvalidParams, "weight_ambiguous_class"),
        WeightError::ModifiedOutOfRange { .. } => {
            (ErrorKind::InvalidParams, "weight_modified_out_of_range")
        }
        WeightError::UnclassifiedBone { .. } => (ErrorKind::Refused, "weight_unclassified_bone"),
        WeightError::NonFiniteWeight { .. } => (ErrorKind::InvalidParams, "weight_non_finite"),
        WeightError::NegativeWeight { .. } => (ErrorKind::InvalidParams, "weight_negative"),
        WeightError::ImpossibleLockedMass { .. } => {
            (ErrorKind::Refused, "weight_impossible_locked_mass")
        }
        WeightError::TooManyLocked { .. } => (ErrorKind::Refused, "weight_too_many_locked"),
        WeightError::LockedMassOverOne { .. } => {
            (ErrorKind::Refused, "weight_locked_mass_over_one")
        }
        WeightError::MissingUnlockedMass { .. } => {
            (ErrorKind::Refused, "weight_missing_unlocked_mass")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_keeps_the_response_shape_and_interpolates_by_default() {
        let output = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]],
            "body_weights": [
                {"Chest": 1.0, "Spine": 0.0},
                {"Chest": 0.5, "Spine": 0.5},
            ],
        }))
        .expect("the default transfer should answer");
        assert_eq!(output["stage"], "transfer");
        assert_eq!(output["vertices"], 1);
        let vertex = output["weights"][0].as_object().expect("influences");
        assert_eq!(vertex["Chest"].as_f64(), Some(0.75));
        assert_eq!(vertex["Spine"].as_f64(), Some(0.25));
    }

    #[test]
    fn transfer_reads_the_extended_options_and_preserves_a_masked_vertex() {
        let output = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            "body_weights": [
                {"Chest": 0.2, "Spine": 0.8},
                {"Chest": 0.8, "Spine": 0.2},
            ],
            "radius": 5.0,
            "max_results": 1,
            "body_mask": [1.0, 0.0],
            "copy": ["Chest", "Spine"],
            "normalize": true,
        }))
        .expect("the masked transfer should answer");
        let vertex = output["weights"][0].as_object().expect("influences");
        let chest = vertex["Chest"].as_f64().expect("Chest");
        let spine = vertex["Spine"].as_f64().expect("Spine");
        assert!(
            (chest - 0.8).abs() < 1e-6,
            "the excluded body vertex must not blend"
        );
        assert!((spine - 0.2).abs() < 1e-6);

        let preserved = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{"Chest": 1.0}],
            "garment_mask": [1.0],
            "current_weights": [{"Hips": 0.7, "Spine": 0.3}],
        }))
        .expect("a preserved vertex keeps its map");
        let hips = preserved["weights"][0]["Hips"]
            .as_f64()
            .expect("preserved Hips");
        assert!((hips - 0.7).abs() < 1e-6, "preserved Hips is {hips}");
        assert!(preserved["weights"][0].get("Chest").is_none());
    }

    #[test]
    fn invalid_transfer_inputs_are_typed_refusals() {
        let negative = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{"Chest": -1.0}],
        }))
        .expect_err("a negative weight is refused");
        assert_eq!(negative.kind(), ErrorKind::InvalidParams);
        assert_eq!(negative.code(), "weight_negative");
        assert!(
            negative.message().contains("body_weights") && negative.message().contains("vertex 0"),
            "the refusal names the input and vertex: {}",
            negative.message()
        );

        let masked = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{"Chest": 1.0}],
            "garment_mask": [1.0],
        }))
        .expect_err("a mask needs current weights");
        assert_eq!(masked.code(), "weight_missing_current");

        let no_neighbours = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{"Chest": 1.0}],
            "max_results": 0,
        }))
        .expect_err("a zero neighbour cap is refused");
        assert_eq!(no_neighbours.code(), "weight_zero_max_results");

        let short_mask = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{"Chest": 1.0}],
            "body_mask": [0.0, 1.0],
        }))
        .expect_err("a mask needs one value per vertex");
        assert_eq!(short_mask.code(), "weight_length_mismatch");

        let unweighted = weights(&json!({
            "stage": "transfer",
            "garment_positions": [[0.0, 0.0, 0.0]],
            "body_positions": [[1.0, 0.0, 0.0]],
            "body_weights": [{}],
        }))
        .expect_err("a vertex with no source weight is refused");
        assert_eq!(unweighted.kind(), ErrorKind::Refused);
        assert_eq!(unweighted.code(), "weight_missing_unlocked_mass");
    }
}
