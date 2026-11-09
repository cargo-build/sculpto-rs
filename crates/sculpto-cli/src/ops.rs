//! Parameterised operations shared by the argv surface and `sculpto run`.
//!
//! Both surfaces construct the same parameter struct and call the same function,
//! so a command cannot drift between them; the only difference is how the values
//! arrive (flags or JSON) and how the outcome is rendered (text or envelope).
//!
//! Failures are [`CommandError`] from the first line, not strings converted at
//! the boundary: [`crate::errors`] owns the exhaustive variant → `kind` + `code`
//! mapping, so a new domain variant breaks the build here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sculpto_bake::BakePlan;
use sculpto_core::{SliderSize, document::Document};
use sculpto_engine::CommandError;
use sculpto_mesh::conform::Conform;

use crate::errors;
use crate::{pick_shape, read_obj_positions, write_obj, write_obj_uv};

/// Everything `bake` needs.
#[derive(Debug, Clone)]
pub struct BakeParams {
    /// `.osp` project.
    pub project: PathBuf,
    /// `SliderPresets` collection.
    pub presets: PathBuf,
    /// Slider set name.
    pub set: String,
    /// Preset name.
    pub preset: String,
    /// `big` or `small`.
    pub endpoint: String,
    /// Output OBJ.
    pub out: PathBuf,
    /// Source NIF; derived from the set and the home when absent.
    pub source: Option<PathBuf>,
    /// Shape name; the largest shape when absent.
    pub shape: Option<String>,
    /// BodySlide home; the project's grandparent when absent.
    pub home: Option<PathBuf>,
    /// Slider overrides in percent.
    pub sliders: BTreeMap<String, f32>,
}

/// What `bake` produced.
#[derive(Debug, Clone)]
pub struct BakeOutcome {
    /// Output path.
    pub out: PathBuf,
    /// Vertices written.
    pub vertices: usize,
    /// Triangles written.
    pub triangles: usize,
    /// Sliders in the plan.
    pub sliders: usize,
    /// Sliders the plan could not resolve.
    pub unresolved: usize,
    /// Sliders deliberately deferred.
    pub deferred: usize,
}

/// Resolve a preset and write the morphed shape as an OBJ.
pub fn bake(params: &BakeParams) -> Result<BakeOutcome, CommandError> {
    let size = match params.endpoint.as_str() {
        "big" => SliderSize::Big,
        "small" => SliderSize::Small,
        other => {
            return Err(errors::invalid(
                "endpoint",
                format!("must be big or small, got `{other}`"),
            ));
        }
    };
    let project = match Document::parse_file(&params.project).map_err(errors::document)? {
        Document::Project(project) => project,
        Document::Presets(_) => {
            return Err(errors::invalid(
                "project",
                format!("{} is not a project", params.project.display()),
            ));
        }
    };
    let presets = match Document::parse_file(&params.presets).map_err(errors::document)? {
        Document::Presets(presets) => presets,
        Document::Project(_) => {
            return Err(errors::invalid(
                "presets",
                format!("{} is not a presets file", params.presets.display()),
            ));
        }
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == params.preset)
        .ok_or_else(|| errors::not_found("preset", &params.preset))?;
    let home = match &params.home {
        Some(home) => home.clone(),
        None => params
            .project
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| errors::invalid("home", "cannot derive the BodySlide home; pass it"))?
            .to_path_buf(),
    };
    let plan =
        BakePlan::resolve_overrides(&project, preset, &params.set, size, &home, &params.sliders)
            .map_err(errors::bake)?;
    let set = project
        .sets
        .iter()
        .find(|set| set.name == params.set)
        .ok_or_else(|| errors::not_found("set", &params.set))?;
    let source = match &params.source {
        Some(source) => source.clone(),
        None => home
            .join("ShapeData")
            .join(set.data_folder.replace('\\', "/"))
            .join(&set.source_file),
    };
    let bytes = std::fs::read(&source).map_err(|error| errors::io(&source, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let shape = pick_shape(&document.shapes, params.shape.as_deref())?;
    let baked = plan
        .apply(&shape.name, &shape.positions)
        .map_err(errors::bake)?;
    write_obj(&params.out, &baked, &shape.triangles)
        .map_err(|error| errors::io(&params.out, error))?;
    Ok(BakeOutcome {
        out: params.out.clone(),
        vertices: baked.len(),
        triangles: shape.triangles.len(),
        sliders: plan.sliders.len(),
        unresolved: plan.unresolved.len(),
        deferred: plan.deferred.len(),
    })
}

/// Everything `export.obj` needs.
#[derive(Debug, Clone)]
pub struct ExportObjParams {
    /// Input NIF.
    pub nif: PathBuf,
    /// Shape name; the largest shape when absent.
    pub shape: Option<String>,
    /// List the shapes and write nothing.
    pub list: bool,
    /// Output OBJ; required unless `list`.
    pub out: Option<PathBuf>,
}

/// One shape row in a `list` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeRow {
    /// Shape name.
    pub name: String,
    /// Vertex count.
    pub vertices: usize,
    /// Triangle count.
    pub triangles: usize,
}

/// What `export.obj` produced.
#[derive(Debug, Clone)]
pub enum ExportObjOutcome {
    /// The caller asked for the shape list.
    Listed(Vec<ShapeRow>),
    /// One shape was written.
    Written {
        /// Output path.
        out: PathBuf,
        /// Shape name.
        shape: String,
        /// Vertex count.
        vertices: usize,
        /// Triangle count.
        triangles: usize,
    },
}

/// Read one NIF shape and write it as an OBJ.
pub fn export_obj(params: &ExportObjParams) -> Result<ExportObjOutcome, CommandError> {
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    if params.list {
        // A BodySlide NIF carries the body and the garment together; the largest
        // shape is often the body, so the caller must see the names before
        // choosing. @req DECL0013
        return Ok(ExportObjOutcome::Listed(
            document
                .shapes
                .iter()
                .map(|shape| ShapeRow {
                    name: shape.name.clone(),
                    vertices: shape.positions.len(),
                    triangles: shape.triangles.len(),
                })
                .collect(),
        ));
    }
    let out = params
        .out
        .clone()
        .ok_or_else(|| errors::invalid("out", "required unless `list` is true"))?;
    let shape = pick_shape(&document.shapes, params.shape.as_deref())?;
    write_obj_uv(&out, &shape.positions, &shape.triangles, &shape.uvs)
        .map_err(|error| errors::io(&out, error))?;
    Ok(ExportObjOutcome::Written {
        out,
        shape: shape.name.clone(),
        vertices: shape.positions.len(),
        triangles: shape.triangles.len(),
    })
}

/// Everything `outfit.fit` needs.
#[derive(Debug, Clone)]
pub struct FitParams {
    /// Garment NIF.
    pub nif: PathBuf,
    /// Garment shape.
    pub shape: String,
    /// Body shape inside the same NIF, the authored reference.
    pub body_shape: String,
    /// Morphed body OBJ.
    pub body: PathBuf,
    /// Output OBJ.
    pub out: PathBuf,
    /// Optional second body for the follow measurement.
    pub body_after: Option<PathBuf>,
    /// Output for the follow measurement.
    pub out_after: Option<PathBuf>,
}

/// What `outfit.fit` produced.
#[derive(Debug, Clone)]
pub struct FitOutcome {
    /// Output path.
    pub out: PathBuf,
    /// Garment name.
    pub garment: String,
    /// Reference shape name.
    pub reference: String,
    /// Vertices written.
    pub vertices: usize,
    /// Mean offset against the reference.
    pub offset_mean: f32,
    /// Largest offset.
    pub offset_max: f32,
    /// Follow measurement when a second body was given.
    pub follow: Option<(f32, f32, f32)>,
}

/// Bind a garment to its authored body and apply the bind to a morphed body.
pub fn garment_fit(params: &FitParams) -> Result<FitOutcome, CommandError> {
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let garment = pick_shape(&document.shapes, Some(&params.shape))?;
    let reference = pick_shape(&document.shapes, Some(&params.body_shape))?;
    let body = read_obj_positions(&params.body)?;
    let conform =
        Conform::bind(&garment.positions, &reference.positions).map_err(errors::conform)?;
    let fitted = conform.apply(&body).map_err(errors::conform)?;
    write_obj(&params.out, &fitted, &garment.triangles)
        .map_err(|error| errors::io(&params.out, error))?;
    let mut follow = None;
    if let Some(after_body) = &params.body_after {
        let out_after = params
            .out_after
            .clone()
            .ok_or_else(|| errors::invalid("out_after", "required when `body_after` is given"))?;
        let after = read_obj_positions(after_body)?;
        let fitted_after = conform.apply(&after).map_err(errors::conform)?;
        write_obj(&out_after, &fitted_after, &garment.triangles)
            .map_err(|error| errors::io(&out_after, error))?;
        let measured = conform.follow(&body, &after).map_err(errors::conform)?;
        follow = Some((measured.body, measured.garment, measured.difference));
    }
    Ok(FitOutcome {
        out: params.out.clone(),
        garment: garment.name.clone(),
        reference: reference.name.clone(),
        vertices: fitted.len(),
        offset_mean: conform.offset_mean(),
        offset_max: conform.offset_max(),
        follow,
    })
}
