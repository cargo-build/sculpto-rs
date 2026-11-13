//! Conform authored body sliders and apply the stored garment response.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use sculpto_bake::{BakePlan, DeferredSlider, PlannedShape, PlannedSlider};
use sculpto_core::SliderSize;
use sculpto_core::document::Document;
use sculpto_core::shape_data::{MorphBlock, MorphContainer, SUPPORTED_VERSION, VertexOffset};
use sculpto_mesh::conform::Conform;
use sculpto_mesh::delta_conform::{DeltaOptions, ProximityCache, ProximityOptions, generate_delta};
use sculpto_mesh::outfit::rigging::fit_rigged_regions;
use sculpto_mesh::outfit::{ClothRegion, ShapeInfo, ShapeRole, census};
use sculpto_mesh::wrap::repair::fit_and_repair;
use sculpto_mesh::wrap::{FitOptions, GapMode};
use sculpto_nif::{NifDocument, NifShape, ShapePlacement};

use crate::obj::{read_obj_positions, read_obj_surface, write_obj};
use crate::{CommandError, ErrorKind, errors};

use super::{CollisionScope, FitOutcome, FitParams};

/// The one declared collision frame this build can place: coordinates already
/// in the body shape's NIF source scene, written in the shared OBJ display
/// frame (`(x, z, -y)` of that scene). No global transform is inferred.
const COLLISION_FRAME_SOURCE_SCENE: &str = "source-scene";

/// A garment and the BodySlide project that defines its reference motion.
#[derive(Debug, Clone, JsonSchema)]
pub struct ConformParams {
    /// Authored garment NIF.
    pub nif: PathBuf,
    /// Garment shape to receive the offsets.
    pub shape: String,
    /// Source body NIF. Defaults to the garment NIF.
    pub body_nif: Option<PathBuf>,
    /// Body shape used by the slider set.
    pub body_shape: String,
    /// BodySlide project.
    pub project: PathBuf,
    /// BodySlide preset collection.
    pub presets: PathBuf,
    /// Preset in that collection. Output blocks retain full slider strength.
    pub preset: String,
    /// Slider set in the project.
    pub set: String,
    /// BodySlide home. Defaults to the project's grandparent.
    pub home: Option<PathBuf>,
    /// Output named OSD with blocks keyed by garment shape plus slider name.
    pub out: PathBuf,
}

/// Persisted garment slider data and the source sliders deferred by the bake.
#[derive(Debug, Clone)]
pub struct ConformOutcome {
    pub out: PathBuf,
    pub vertices: usize,
    pub sliders: Vec<String>,
    pub deferred: Vec<DeferredSlider>,
}

/// Transfer the selected body's positional sliders to the garment. @req DECL0013
///
/// # Errors
/// Refuses unresolved reference blocks, malformed meshes and offsets, and
/// data the named OSD format cannot store.
pub fn conform_sliders(params: &ConformParams) -> Result<ConformOutcome, CommandError> {
    let document = read_nif(&params.nif)?;
    let source = body_document(&document, params.body_nif.as_deref())?;
    let garment = PlacedShape::named(&document, &params.shape)?;
    let body = PlacedShape::named(&source, &params.body_shape)?;
    let (plan, names, target) = conform_plan(params)?;
    let cache = ProximityCache::build(
        &body.source_positions(),
        &garment.source_positions(),
        &ProximityOptions::default(),
        None,
    )
    .map_err(errors::delta_conform)?;

    let deferred: BTreeSet<_> = plan
        .deferred
        .iter()
        .map(|slider| slider.name.as_str())
        .collect();
    let sliders: Vec<_> = names
        .into_iter()
        .filter(|name| !deferred.contains(name.as_str()))
        .collect();
    let active: BTreeMap<_, _> = plan
        .sliders
        .iter()
        .map(|slider| (slider.name.as_str(), slider))
        .collect();
    let zeros = vec![[0.0; 3]; body.shape.positions.len()];
    let blocks = sliders
        .iter()
        .map(|name| {
            let offsets = match active.get(name.as_str()) {
                Some(slider) => garment_offsets(
                    slider,
                    &target,
                    &zeros,
                    &cache,
                    body.placement,
                    garment.placement,
                )?,
                None => Vec::new(),
            };
            Ok(MorphBlock {
                name: block_name(&garment.shape.name, name),
                offsets,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let bytes = MorphContainer {
        version: SUPPORTED_VERSION,
        blocks,
    }
    .to_osd_bytes()
    .map_err(errors::document)?;
    std::fs::write(&params.out, bytes).map_err(|error| errors::io(&params.out, error))?;
    Ok(ConformOutcome {
        out: params.out.clone(),
        vertices: garment.shape.positions.len(),
        sliders,
        deferred: plan.deferred,
    })
}

fn conform_plan(params: &ConformParams) -> Result<(BakePlan, Vec<String>, String), CommandError> {
    let Document::Project(mut project) =
        Document::parse_file(&params.project).map_err(errors::document)?
    else {
        return Err(errors::invalid("project", "expected a slider-set project"));
    };
    let Document::Presets(presets) =
        Document::parse_file(&params.presets).map_err(errors::document)?
    else {
        return Err(errors::invalid("presets", "expected a preset collection"));
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == params.preset)
        .ok_or_else(|| errors::not_found("preset", &params.preset))?;
    let set = project
        .sets
        .iter_mut()
        .find(|set| set.name == params.set)
        .ok_or_else(|| errors::not_found("set", &params.set))?;
    let target = set
        .target_for_shape(&params.body_shape)
        .map(str::to_owned)
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                "conform_body_target",
                format!(
                    "set `{}` has no source shape `{}`",
                    set.name, params.body_shape
                ),
            )
        })?;
    for slider in &mut set.sliders {
        slider.data.retain(|reference| reference.target == target);
    }
    let names: Vec<_> = set
        .sliders
        .iter()
        .map(|slider| slider.name.clone())
        .collect();
    let overrides = names.iter().cloned().map(|name| (name, 100.0)).collect();
    let home = params
        .home
        .as_deref()
        .or_else(|| params.project.parent().and_then(Path::parent))
        .ok_or_else(|| errors::invalid("home", "cannot derive the BodySlide home"))?;
    let plan = BakePlan::resolve_overrides(
        &project,
        preset,
        &params.set,
        SliderSize::Big,
        home,
        &overrides,
    )
    .map_err(errors::bake)?;
    if let Some(reference) = plan.unresolved.first() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "conform_unresolved_reference",
            format!(
                "slider `{}` target `{}`: {:?}",
                reference.slider, reference.target, reference.reason
            ),
        ));
    }
    Ok((plan, names, target))
}

fn garment_offsets(
    slider: &PlannedSlider,
    target: &str,
    zeros: &[[f32; 3]],
    cache: &ProximityCache,
    body: ShapePlacement,
    garment: ShapePlacement,
) -> Result<Vec<VertexOffset>, CommandError> {
    let deltas = slider.apply(target, zeros).map_err(errors::bake)?;
    let body_diffs = slider
        .shapes
        .iter()
        .filter(|shape| shape.target == target)
        .flat_map(|shape| &shape.offsets)
        .map(|offset| {
            let index = usize::from(offset.index);
            (index, body.offset_in_source(deltas[index]))
        })
        .collect();
    generate_delta(cache, &body_diffs, &DeltaOptions::default())
        .map_err(errors::delta_conform)?
        .into_iter()
        .map(|(index, delta)| {
            let index = u16::try_from(index).map_err(|_| {
                CommandError::new(
                    ErrorKind::Refused,
                    "conform_vertex_index",
                    format!("garment vertex {index} exceeds the named OSD index width"),
                )
            })?;
            Ok(VertexOffset {
                index,
                delta: garment.offset_in_mesh(delta),
            })
        })
        .collect()
}

pub(super) fn preview(params: &FitParams) -> Result<FitOutcome, CommandError> {
    let (Some(container), Some(name), Some(percent)) =
        (&params.conform, &params.slider, params.percent)
    else {
        return Err(errors::invalid(
            "conform",
            "slider preview requires conform, slider and percent",
        ));
    };
    if name.trim().is_empty() || !percent.is_finite() {
        return Err(errors::invalid(
            "slider",
            "requires a name and a finite percentage",
        ));
    }
    let document = read_nif(&params.nif)?;
    let source = body_document(&document, params.body_nif.as_deref())?;
    let garment = PlacedShape::named(&document, &params.shape)?;
    let body = PlacedShape::named(&source, &params.body_shape)?;
    let target = target_positions(params, &body)?;
    let block_name = block_name(&garment.shape.name, name);
    let data = MorphContainer::parse_file(container).map_err(errors::document)?;
    let block = data
        .blocks
        .into_iter()
        .find(|block| block.name == block_name)
        .ok_or_else(|| errors::not_found("conformed_slider", name))?;
    let slider = PlannedSlider {
        name: name.clone(),
        percent,
        invert: false,
        shapes: vec![PlannedShape {
            target: garment.shape.name.clone(),
            block: block.name,
            offsets: block.offsets,
        }],
    };
    let morphed = slider
        .apply(&garment.shape.name, &garment.shape.positions)
        .map_err(errors::bake)?;
    let fitted: Vec<_> = morphed
        .into_iter()
        .map(|point| garment.placement.position_in_source(point))
        .collect();
    finish_fit(params, &document, &garment, &body, fitted, target)
}

pub(super) fn fit_regions(params: &FitParams) -> Result<FitOutcome, CommandError> {
    let document = read_nif(&params.nif)?;
    let source = body_document(&document, params.body_nif.as_deref())?;
    let garment = PlacedShape::named(&document, &params.shape)?;
    let body = PlacedShape::named(&source, &params.body_shape)?;
    let target = target_positions(params, &body)?;
    finish_fit(
        params,
        &document,
        &garment,
        &body,
        garment.source_positions(),
        target,
    )
}

fn target_positions(
    params: &FitParams,
    body: &PlacedShape<'_>,
) -> Result<Vec<[f32; 3]>, CommandError> {
    let target = read_obj_positions(&params.body)?;
    if target.len() != body.shape.positions.len() {
        return Err(errors::invalid(
            "body",
            format!(
                "{} positions, expected {} in source shape order",
                target.len(),
                body.shape.positions.len(),
            ),
        ));
    }
    Ok(target
        .into_iter()
        .map(|point| body.placement.position_in_source(point))
        .collect())
}

fn cloth_regions(
    params: &FitParams,
    document: &NifDocument,
) -> Result<Vec<ClothRegion>, CommandError> {
    let path = params
        .manifest
        .as_deref()
        .ok_or_else(|| errors::invalid("manifest", "region fit requires a manifest"))?;
    let source =
        std::fs::canonicalize(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let manifest = super::read_outfit_manifest(path, &source)?;
    let shapes: Vec<_> = document
        .shapes
        .iter()
        .map(|shape| ShapeInfo {
            name: &shape.name,
            vertices: shape.positions.len(),
            triangles: shape.triangles.len(),
        })
        .collect();
    let report = census(&shapes, Some(&manifest)).map_err(errors::outfit)?;
    let garment = report
        .iter()
        .find(|shape| shape.name == params.shape)
        .ok_or_else(|| errors::not_found("shape", &params.shape))?;
    if garment.role != ShapeRole::Garment {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "outfit_fit_role",
            "region fitting requires a garment shape",
        ));
    }
    Ok(
        match manifest
            .garment_shapes
            .into_iter()
            .find(|slot| slot.name == params.shape)
        {
            Some(slot) => slot.blender_cloth,
            None => Vec::new(),
        },
    )
}

/// A caller-declared complete collision surface, read and validated before any
/// fit output exists. @req DECL0013
struct CollisionSurface {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
}

impl CollisionSurface {
    /// Read the declared collision OBJ, or report why it cannot be used.
    ///
    /// Alignment is not inferable from coordinates: a surface declared in the
    /// supported frame but assembled somewhere else still passes. This checks
    /// the declaration, finiteness, faces and index ranges only.
    fn declared(params: &FitParams) -> Result<Option<Self>, CommandError> {
        let Some(path) = &params.collision else {
            return match &params.collision_frame {
                None => Ok(None),
                Some(_) => Err(CommandError::new(
                    ErrorKind::Refused,
                    "outfit_fit_collision_frame",
                    "`collision_frame` declares a frame but `collision` names no surface",
                )),
            };
        };
        let frame = params.collision_frame.as_deref().ok_or_else(|| {
            CommandError::new(
                ErrorKind::Refused,
                "outfit_fit_collision_frame",
                format!(
                    "a collision surface requires a declared frame; supported: `{COLLISION_FRAME_SOURCE_SCENE}`"
                ),
            )
        })?;
        if frame != COLLISION_FRAME_SOURCE_SCENE {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "outfit_fit_collision_frame",
                format!(
                    "unsupported collision frame `{frame}`; supported: `{COLLISION_FRAME_SOURCE_SCENE}`"
                ),
            ));
        }
        let (positions, triangles) = read_obj_surface(path)?;
        if triangles.is_empty() {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "outfit_fit_collision_surface",
                format!(
                    "{}: collision surface has no faces; declare the assembled parts as triangles",
                    path.display()
                ),
            ));
        }
        if let Some(vertex) = positions
            .iter()
            .position(|point| point.iter().any(|value| !value.is_finite()))
        {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "outfit_fit_collision_surface",
                format!(
                    "{}: collision vertex {vertex} is not finite",
                    path.display()
                ),
            ));
        }
        Ok(Some(Self {
            positions,
            triangles,
        }))
    }

    fn scope(&self) -> CollisionScope {
        CollisionScope {
            frame: COLLISION_FRAME_SOURCE_SCENE.to_owned(),
            vertices: self.positions.len(),
            triangles: self.triangles.len(),
            scope: "caller-declared",
        }
    }
}

fn finish_fit(
    params: &FitParams,
    document: &NifDocument,
    garment: &PlacedShape<'_>,
    body: &PlacedShape<'_>,
    fitted: Vec<[f32; 3]>,
    target: Vec<[f32; 3]>,
) -> Result<FitOutcome, CommandError> {
    if params.body_after.is_some() || params.out_after.is_some() {
        return Err(errors::invalid(
            "body_after",
            "fit one body state per region or slider request",
        ));
    }
    let collision = CollisionSurface::declared(params)?;
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        opening_direction: Some([0.0, 0.0, 1.0]),
        ..FitOptions::default()
    };
    let garment_mesh = (fitted.as_slice(), garment.shape.triangles.as_slice());
    // The declared surface replaces the single body shape as the obstacle only;
    // correspondence and the output frame stay on the authored body.
    let body_mesh = match &collision {
        Some(surface) => (surface.positions.as_slice(), surface.triangles.as_slice()),
        None => (target.as_slice(), body.shape.triangles.as_slice()),
    };
    let (fitted, pairs, inside, regions) = match &params.manifest {
        Some(_) => {
            let regions = cloth_regions(params, document)?;
            let (fitted, report) = fit_rigged_regions(
                garment_mesh,
                body_mesh,
                &regions,
                None,
                &options,
            )
            .map_err(|error| {
                CommandError::new(ErrorKind::Refused, "outfit_region_fit", error.to_string())
            })?;
            let pairs = report.intersection_pairs;
            (fitted, pairs, None, Some(report))
        }
        None => {
            let (fitted, report) = fit_and_repair(garment_mesh, body_mesh, None, &options)
                .map_err(|error| {
                    CommandError::new(ErrorKind::Refused, "conform_fit_repair", error.to_string())
                })?;
            (
                fitted,
                report.intersection_pairs,
                Some(report.inside_after),
                None,
            )
        }
    };
    let reference = Conform::bind(&garment.source_positions(), &body.source_positions())
        .map_err(errors::conform)?;
    let output: Vec<_> = fitted
        .into_iter()
        .map(|point| body.placement.position_in_mesh(point))
        .collect();
    if output.iter().flatten().any(|value| !value.is_finite()) {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "conform_fit_placement",
            "fitted garment is outside the output frame's finite range",
        ));
    }
    write_obj(&params.out, &output, &garment.shape.triangles)
        .map_err(|error| errors::io(&params.out, error))?;
    Ok(FitOutcome {
        out: params.out.clone(),
        garment: garment.shape.name.clone(),
        reference: body.shape.name.clone(),
        vertices: output.len(),
        offset_mean: reference.offset_mean(),
        offset_max: reference.offset_max(),
        follow: None,
        intersection_pairs: Some(pairs),
        inside_after: inside,
        regions,
        collision: collision.as_ref().map(CollisionSurface::scope),
    })
}

fn block_name(shape: &str, slider: &str) -> String {
    format!("{shape}{slider}")
}

fn read_nif(path: &Path) -> Result<NifDocument, CommandError> {
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    sculpto_nif::read(&bytes).map_err(errors::nif)
}

fn body_document<'a>(
    garment: &'a NifDocument,
    path: Option<&Path>,
) -> Result<Cow<'a, NifDocument>, CommandError> {
    match path {
        Some(path) => read_nif(path).map(Cow::Owned),
        None => Ok(Cow::Borrowed(garment)),
    }
}

struct PlacedShape<'a> {
    shape: &'a NifShape,
    placement: ShapePlacement,
}

impl<'a> PlacedShape<'a> {
    fn named(document: &'a NifDocument, name: &str) -> Result<Self, CommandError> {
        let mut shapes = document
            .shapes
            .iter()
            .enumerate()
            .filter(|(_, shape)| shape.name == name);
        let (index, shape) = shapes
            .next()
            .ok_or_else(|| errors::not_found("shape", name))?;
        if shapes.next().is_some() {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "conform_ambiguous_shape",
                format!("more than one shape named `{name}`"),
            ));
        }
        let placement = document.shape_placement(index).map_err(|error| {
            CommandError::new(
                ErrorKind::Refused,
                "conform_shape_placement",
                error.to_string(),
            )
        })?;
        Ok(Self { shape, placement })
    }

    fn source_positions(&self) -> Vec<[f32; 3]> {
        self.shape
            .positions
            .iter()
            .copied()
            .map(|point| self.placement.position_in_source(point))
            .collect()
    }
}
