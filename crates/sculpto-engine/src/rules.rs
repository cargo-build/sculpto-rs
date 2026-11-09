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

use sculpto_mesh::garment::{self, BodyExtent, GarmentKind, Influences, RawExtent, RawTriangle};

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
/// `skinning`.
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
            if body_positions.len() != body_weights.len() {
                return Err(errors::invalid(
                    "body_weights",
                    "one influence set per body vertex is required",
                ));
            }
            let transferred = garment::transfer(&garment_positions, &body_positions, &body_weights)
                .map_err(errors::garment_rule)?;
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
