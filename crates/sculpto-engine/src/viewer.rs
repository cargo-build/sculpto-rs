//! `viewer export`: the review GLB the engine writes itself.
//!
//! The Bevy viewer (`tools/bevy-viewer --interactive`) is the standard
//! interactive preview, so the review file must come from the same Rust engine
//! that produces the body — a Blender/Python detour is exactly the glue the
//! product forbids. This command reads the body OBJ plus its ranked garment
//! layers, scales the centimetre-like BodySlide frame to metres (71.4286
//! units per metre) so the viewer camera, lights and orbit speeds work
//! unmodified, and writes one palette-coloured mesh per part through
//! [`sculpto_gltf::review`].
//!
//! The file is a review artifact: it proves "this is what the parts look like
//! together", never "this fit passed". The report states that explicitly.
//! @req BODY-01

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_gltf::review::{BODY_COLOR, ReviewPart, hex, palette, write_review_glb};
use sculpto_mesh::layer_stack::{LayerStack, StackError};

use crate::obj::{ObjMesh, read_error, read_obj_mesh};
use crate::{CommandError, ErrorKind, errors};

/// BodySlide units per metre: the source OBJ frame is centimetre-like.
pub(crate) const UNITS_PER_METRE: f32 = 71.4286;

/// The command input. `layers` and `outfit` are alternative routes to the same
/// layer list; giving both is a refusal, not an undefined merge.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerExportParams {
    /// The body OBJ (assembled, sealed); it becomes the first part. The
    /// outfit manifest may supply it instead.
    #[serde(default)]
    pub body: Option<PathBuf>,
    /// GLB output path.
    pub out: PathBuf,
    /// Ranked layers; the palette rank picks the material colour.
    #[serde(default)]
    pub layers: Vec<ViewerLayer>,
    /// A `garment stack` layers.json instead of `layers`; paths resolve
    /// beside the manifest, and `body` may come from the manifest.
    #[serde(default)]
    pub outfit: Option<PathBuf>,
    /// Also write the report JSON here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// One ranked layer: the palette rank and its OBJ.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewerLayer {
    /// Palette rank `1..=5`; the colour comes from the review palette.
    pub order: u32,
    /// Layer OBJ in the body's frame.
    pub obj: PathBuf,
}

/// The command input schema.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(ViewerExportParams))
}

/// The registry handler: parse, export, report.
///
/// # Errors
/// Typed refusals for the declared defect classes; a written GLB is never
/// accompanied by an `Ok`-with-verdict report of a failure.
pub(crate) fn export(input: &Value) -> Result<Value, CommandError> {
    let params: ViewerExportParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "viewer_export_input",
            error.to_string(),
        )
    })?;
    run(&params)
}

/// Export one review GLB and render its report.
fn run(params: &ViewerExportParams) -> Result<Value, CommandError> {
    if !params.layers.is_empty() && params.outfit.is_some() {
        return Err(errors::invalid(
            "outfit",
            "`layers` and `outfit` are alternative layer routes; declare one",
        ));
    }
    let mut layers = params.layers.clone();
    let mut body: Option<PathBuf> = params.body.clone();
    if let Some(manifest) = &params.outfit {
        let stack = read_manifest(manifest)?;
        body = match (body, stack.body.clone()) {
            (None, declared) => declared,
            (Some(argv), Some(declared)) if argv == declared => Some(argv),
            (Some(argv), Some(declared)) => {
                return Err(errors::invalid(
                    "body",
                    format!(
                        "`--body` {} and the manifest body {} disagree",
                        argv.display(),
                        declared.display()
                    ),
                ));
            }
            (Some(argv), None) => Some(argv),
        };
        layers = stack
            .layers
            .iter()
            .map(|layer| ViewerLayer {
                order: layer.order,
                obj: layer.obj.clone(),
            })
            .collect();
    }
    let body = body.ok_or_else(|| {
        errors::invalid(
            "body",
            "no body: the flag is absent and the manifest declares none",
        )
    })?;

    // A non-OBJ layer cannot be a palette layer; refuse it before any read.
    for layer in &layers {
        let extension = layer
            .obj
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        if extension.as_deref() != Some("obj") {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "viewer_layer_not_obj",
                format!(
                    "layer {} path {} is not an OBJ; only .obj layers carry palette ranks",
                    layer.order,
                    layer.obj.display()
                ),
            ));
        }
    }

    // One colour per rank: a rank outside 1..=5 has no palette slot and two
    // layers on one rank would open indistinguishable, so both are refused
    // before any read.
    let mut seen = Vec::new();
    for layer in &layers {
        if !(1..=5).contains(&layer.order) {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "viewer_palette_order",
                format!("layer order {} is outside the palette 1..=5", layer.order),
            ));
        }
        if seen.contains(&layer.order) {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "viewer_palette_duplicate",
                format!("two layers declare palette rank {}", layer.order),
            ));
        }
        seen.push(layer.order);
    }

    let scale = 1.0 / UNITS_PER_METRE;
    let (body_mesh, body_name) = read_part(&body, "body")?;
    let mut parts = vec![ReviewPart {
        name: body_name,
        positions: scaled(&body_mesh.0, scale),
        triangles: body_mesh.1,
        color: BODY_COLOR,
    }];
    let mut rows = vec![part_row(parts[0].clone(), "body", 0)];
    let mut ordered = layers;
    ordered.sort_by_key(|layer| layer.order);
    for layer in ordered {
        let rank = u8::try_from(layer.order).expect("the palette gate validated 1..=5");
        let Some(color) = palette(rank) else {
            return Err(CommandError::new(
                ErrorKind::InvalidParams,
                "viewer_palette_order",
                format!("layer order {} is outside the palette 1..=5", layer.order),
            ));
        };
        let (mesh, name) = read_part(&layer.obj, "layer")?;
        let part = ReviewPart {
            name: name.clone(),
            positions: scaled(&mesh.0, scale),
            triangles: mesh.1,
            color,
        };
        rows.push(part_row(part.clone(), "layer", layer.order));
        parts.push(part);
    }

    let bytes = write_review_glb(&parts).map_err(glb_error)?;
    write_out(&params.out, &bytes)?;
    if let Some(parent) = params.out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }

    let bounds = scene_bounds(&parts);
    let report = json!({
        "out": params.out.display().to_string(),
        "bytes": bytes.len(),
        "units_per_metre": UNITS_PER_METRE,
        "scale": scale,
        "parts": rows,
        "bounds": bounds,
        "statement": "review artifact, not a fit result",
        "review_only": true,
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

fn read_part(path: &Path, role: &str) -> Result<(ObjMesh, String), CommandError> {
    let mesh = read_obj_mesh(path).map_err(read_error)?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| role.to_string());
    Ok((mesh, name))
}

fn scaled(positions: &[[f32; 3]], scale: f32) -> Vec<[f32; 3]> {
    positions
        .iter()
        .map(|[x, y, z]| [x * scale, y * scale, z * scale])
        .collect()
}

fn part_row(part: ReviewPart, role: &str, order: u32) -> Value {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in &part.positions {
        for (axis, value) in position.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
        }
    }
    json!({
        "name": part.name,
        "role": role,
        "order": order,
        "vertices": part.positions.len(),
        "triangles": part.triangles.len(),
        "bbox_min": min,
        "bbox_max": max,
        "material": hex(part.color),
    })
}

fn scene_bounds(parts: &[ReviewPart]) -> Value {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for part in parts {
        for position in &part.positions {
            for (axis, value) in position.iter().enumerate() {
                min[axis] = min[axis].min(*value);
                max[axis] = max[axis].max(*value);
            }
        }
    }
    json!({ "min": min, "max": max })
}

fn read_manifest(path: &Path) -> Result<LayerStack, CommandError> {
    let text = std::fs::read_to_string(path).map_err(|error| errors::io(path, error))?;
    LayerStack::parse_json(&text, path).map_err(|error| manifest_error(&error))
}

fn manifest_error(error: &StackError) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "viewer_outfit_manifest_invalid",
        error.to_string(),
    )
}

/// The writer's refusals keep their own names: an empty part is a domain
/// refusal, a framing failure is an engine defect, not caller input.
fn glb_error(error: sculpto_gltf::GltfError) -> CommandError {
    match &error {
        sculpto_gltf::GltfError::ReviewNoParts
        | sculpto_gltf::GltfError::ReviewPartEmpty { .. } => {
            CommandError::new(ErrorKind::Refused, "viewer_part_empty", error.to_string())
        }
        sculpto_gltf::GltfError::TooLarge { field, size } => CommandError::new(
            ErrorKind::Unsupported,
            "viewer_glb_too_large",
            format!("{field} ({size}) exceeds the glTF u32 limit"),
        ),
        sculpto_gltf::GltfError::Serialize(detail) | sculpto_gltf::GltfError::Encode(detail) => {
            CommandError::new(
                ErrorKind::Io,
                "viewer_glb_write",
                format!("GLB write failed: {detail}"),
            )
        }
        other => CommandError::new(
            ErrorKind::Unsupported,
            "viewer_glb_unsupported",
            format!("{other}"),
        ),
    }
}

fn write_out(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    std::fs::write(path, bytes).map_err(|error| errors::io(path, error))
}

fn write_report(path: &Path, report: &Value) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(report).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "viewer_report",
            format!("{}: {error}", path.display()),
        )
    })?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    std::fs::write(path, format!("{text}\n")).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "viewer_report",
            format!("{}: {error}", path.display()),
        )
    })
}
