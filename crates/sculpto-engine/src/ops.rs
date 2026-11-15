//! Parameterised operations shared by the argv surface and `sculpto run`.
//!
//! Both surfaces construct the same parameter struct and call the same function,
//! so a command cannot drift between them; the only difference is how the values
//! arrive (flags or JSON) and how the outcome is rendered (text or envelope).
//!
//! Failures are [`CommandError`] from the first line, not strings converted at
//! the boundary: [`crate::errors`] owns the exhaustive variant → `kind` + `code`
//! mapping, so a new domain variant breaks the build here.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CommandError, ErrorKind};
use sculpto_bake::BakeError;
use sculpto_bake::BakePlan;
use sculpto_bake::CoverageState;
use sculpto_core::mesh::{MeshAsset, TextureSlot};
use sculpto_core::rig::Rig;
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::{SliderSize, document::Document};
use sculpto_gltf::sanitize::{Repair, SanitizeError};
use sculpto_mesh::conform::{Conform, ConformError};
use sculpto_mesh::intake_gltf::{DefectLimits, TopologyDefects, read_raw};
use sculpto_mesh::outfit::{
    CheckShape, OutfitError, OutfitManifest, ShapeInfo, ShapeReport, TextureRef, census,
    completeness_defects, texture_report,
};
use sculpto_mesh::surface::SurfaceBinding;
use sculpto_mesh::wrap::repair::final_push::FinalPushOptions;
use sculpto_nif::write::header::{NifTarget, NifWriteError};
use sculpto_nif::write::le_bundle::write_le_bundle;
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::{
    DroppedInfluence, InfluencePolicy, WrittenNif, write_rigged_shape, write_rigged_shape_deleted,
    write_shape_with_policy, write_tri,
};
use sculpto_nif::{NifDocument, NifRigError, NifShape, TriFile};

use crate::errors;
use crate::obj::{
    pick_shape, pick_shape_index, read_obj_positions, read_obj_surface, write_obj, write_obj_uv,
};

mod dress;
mod head;
mod mesh_densify;
mod slider;
mod stack_fit;
pub use dress::{DressMeasurements, DressOutcome, DressParams, dress_garment};
pub use head::{
    AssignedShape, BodiesConfig, BodyDescriptor, CheckStatus, CompatibilityCheck,
    CompatibilityReport, RoleBundle, SkinTextureSet, apply_default_skin, head_compatibility,
    load_bodies_config, load_shape, resolve_default_head,
};
pub use mesh_densify::{DensifyOutcome, DensifyParams, densify_mesh};
pub use slider::{ConformOutcome, ConformParams, conform_sliders};
pub use stack_fit::{ObstacleProvenance, StackFitOutcome, StackFitParams, StackFitStep, stack_fit};

/// A source NIF and its optional authored outfit declaration.
#[derive(Debug, Clone, JsonSchema)]
pub struct OutfitInspectParams {
    /// Outfit NIF to inspect.
    pub nif: PathBuf,
    /// JSON manifest bound to this source.
    pub manifest: Option<PathBuf>,
    /// Loose data roots searched before the archives, in order (loose first).
    pub texture_roots: Vec<PathBuf>,
    /// BSA archives indexed for texture resolution, in priority order.
    pub archives: Vec<PathBuf>,
}

/// The texture resolution summary `outfit.inspect` reports.
///
/// `resolved` counts resolved non-empty slot references; `unresolved` lists
/// the unique unresolved paths, sorted; `shapes` carries per-shape rows in
/// source order (MOD-03 D-5).
#[derive(Debug, Clone, Serialize)]
pub struct TextureResolution {
    /// Number of resolved slot references.
    pub resolved: usize,
    /// Unique unresolved authored paths, sorted.
    pub unresolved: Vec<String>,
    /// Per-shape texture rows, parallel to `OutfitInspectOutcome::shapes`.
    pub shapes: Vec<Vec<TextureRef>>,
}

/// What the outfit contains and how each garment part should move.
#[derive(Debug)]
pub struct OutfitInspectOutcome {
    /// Authored outfit name when a manifest was supplied.
    pub outfit: Option<String>,
    /// Census and cloth selection in source order.
    pub shapes: Vec<ShapeReport>,
    /// Texture resolution of every authored reference.
    pub textures: TextureResolution,
}

/// Read an outfit without changing its geometry or rig.
///
/// # Errors
/// Refuses unreadable sources, invalid manifests, selections that cannot
/// bind to the imported shapes, and archives that do not index.
pub fn inspect_outfit(params: &OutfitInspectParams) -> Result<OutfitInspectOutcome, CommandError> {
    let source =
        std::fs::canonicalize(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let manifest = params
        .manifest
        .as_deref()
        .map(|path| read_outfit_manifest(path, &source))
        .transpose()?;
    let bytes = std::fs::read(&source).map_err(|error| errors::io(&source, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let shapes: Vec<_> = document
        .shapes
        .iter()
        .map(|shape| ShapeInfo {
            name: &shape.name,
            vertices: shape.positions.len(),
            triangles: shape.triangles.len(),
        })
        .collect();
    let shapes = census(&shapes, manifest.as_ref()).map_err(errors::outfit)?;
    let indexes = params
        .archives
        .iter()
        .map(|archive| sculpto_core::bsa::index_bsa(archive).map_err(errors::bsa))
        .collect::<Result<Vec<_>, _>>()?;
    let materials: Vec<&[sculpto_core::mesh::MaterialSlot]> = document
        .shapes
        .iter()
        .map(|shape| shape.materials.as_slice())
        .collect();
    let texture_rows = texture_report(&materials, &params.texture_roots, &indexes);
    let mut unresolved: Vec<String> = texture_rows
        .iter()
        .flatten()
        .filter(|reference| reference.source.is_none())
        .map(|reference| reference.path.clone())
        .collect();
    unresolved.sort_unstable();
    unresolved.dedup();
    let resolved = texture_rows
        .iter()
        .flatten()
        .filter(|reference| reference.source.is_some())
        .count();
    Ok(OutfitInspectOutcome {
        outfit: manifest.map(|manifest| manifest.outfit),
        shapes,
        textures: TextureResolution {
            resolved,
            unresolved,
            shapes: texture_rows,
        },
    })
}

/// Everything `outfit.check` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct OutfitCheckParams {
    /// Assembled outfit NIF bundle to gate.
    pub nif: PathBuf,
    /// JSON manifest bound to this bundle (required, unlike inspect).
    pub manifest: PathBuf,
}

/// One manifest garment's measured completeness row.
#[derive(Debug)]
pub struct OutfitCheckShape {
    /// Manifest garment name.
    pub name: String,
    /// Zero-based shape ordinal in the bundle; `None` when the name is absent.
    pub shape_index: Option<usize>,
    /// Vertices of the matched shape; 0 when the name is absent.
    pub vertices: usize,
    /// Triangles of the matched shape; 0 when absent or empty.
    pub triangles: usize,
    /// The manifest slot's material expectation.
    pub requires_material: bool,
    /// At least one material slot of the matched shape names a diffuse path.
    pub has_diffuse: bool,
}

/// What `outfit.check` produced; `Ok` implies the complete verdict.
#[derive(Debug)]
pub struct OutfitCheckOutcome {
    /// Manifest outfit identity.
    pub outfit: String,
    /// One row per manifest garment slot, in manifest order.
    pub shapes: Vec<OutfitCheckShape>,
}

/// Gate an assembled outfit bundle against its manifest.
///
/// Every manifest garment shape must be present with triangles, and a slot
/// that `requires_material` must name a diffuse texture path. Unlike
/// [`inspect_outfit`], presence is evaluated here instead of through the
/// strict census binding, so a missing shape is reported as an incomplete
/// outfit rather than an unknown-shape refusal.
///
/// # Errors
/// Refuses unreadable sources, invalid manifests and any incomplete garment
/// requirement; the refusal message names every failing shape.
pub fn check_outfit(params: &OutfitCheckParams) -> Result<OutfitCheckOutcome, CommandError> {
    let source =
        std::fs::canonicalize(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let manifest = read_outfit_manifest(&params.manifest, &source)?;
    if manifest.outfit.trim().is_empty() {
        return Err(errors::outfit(OutfitError::BlankOutfit));
    }
    let bytes = std::fs::read(&source).map_err(|error| errors::io(&source, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let shapes: Vec<_> = document
        .shapes
        .iter()
        .map(|shape| CheckShape {
            name: &shape.name,
            triangles: shape.triangles.len(),
            has_diffuse: shape
                .materials
                .iter()
                .any(|material| material.texture(TextureSlot::Diffuse).is_some()),
        })
        .collect();
    let failures = completeness_defects(&shapes, &manifest);
    let rows = manifest
        .garment_shapes
        .iter()
        .filter(|slot| !manifest.body_shapes.contains(&slot.name))
        .map(|slot| {
            let index = document
                .shapes
                .iter()
                .position(|shape| shape.name == slot.name);
            OutfitCheckShape {
                name: slot.name.clone(),
                vertices: index.map_or(0, |index| document.shapes[index].positions.len()),
                triangles: index.map_or(0, |index| document.shapes[index].triangles.len()),
                has_diffuse: index.is_some_and(|index| {
                    document.shapes[index]
                        .materials
                        .iter()
                        .any(|material| material.texture(TextureSlot::Diffuse).is_some())
                }),
                shape_index: index,
                requires_material: slot.requires_material,
            }
        })
        .collect();
    if failures.is_empty() {
        return Ok(OutfitCheckOutcome {
            outfit: manifest.outfit,
            shapes: rows,
        });
    }
    let detail = failures
        .iter()
        .map(|failure| failure.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    Err(CommandError::new(
        ErrorKind::Refused,
        "outfit_incomplete",
        format!("outfit is incomplete: {detail}"),
    ))
}

fn read_outfit_manifest(path: &Path, nif: &Path) -> Result<OutfitManifest, CommandError> {
    let path = std::fs::canonicalize(path).map_err(|error| errors::io(path, error))?;
    let bytes = std::fs::read(&path).map_err(|error| errors::io(&path, error))?;
    let manifest: OutfitManifest = serde_json::from_slice(&bytes).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "outfit_manifest_invalid",
            format!("{}: {error}", path.display()),
        )
    })?;
    if manifest.nif.as_os_str().is_empty() {
        return Err(errors::invalid("manifest.nif", "source path is required"));
    }
    let declared = path.parent().unwrap_or(Path::new(".")).join(&manifest.nif);
    let expected =
        std::fs::canonicalize(&declared).map_err(|error| errors::io(&declared, error))?;
    if expected.as_path() != nif {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "outfit_source_mismatch",
            format!(
                "{} declares {}, but the request names {}",
                path.display(),
                declared.display(),
                nif.display()
            ),
        ));
    }
    Ok(manifest)
}

/// Everything `bake` needs.
#[derive(Debug, Clone, JsonSchema)]
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
    /// Coverage state of the body: `clothed` hides the chest anatomy through
    /// the set's authored coverage morph (`NipBGone=100`, an explicit override
    /// for that slider wins with its own value); `nude` keeps the preset
    /// values. Absent defaults to `clothed` when the set carries the morph,
    /// and bakes as authored otherwise — an explicit `clothed` on a set
    /// without the morph is refused, never silently nude.
    pub coverage: Option<String>,
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
    /// Coverage state the bake ended in.
    pub coverage: CoverageState,
    /// Effective coverage morph percent when coverage applied.
    pub coverage_percent: Option<f32>,
}

/// Resolve a preset and write the morphed shape as an OBJ.
pub fn bake(params: &BakeParams) -> Result<BakeOutcome, CommandError> {
    let size = endpoint("endpoint", &params.endpoint)?;
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
    let set = project
        .sets
        .iter()
        .find(|set| set.name == params.set)
        .ok_or_else(|| errors::not_found("set", &params.set))?;
    let requested = match params.coverage.as_deref() {
        None => None,
        Some(token) => Some(
            sculpto_bake::Coverage::parse(token)
                .ok_or_else(|| errors::invalid("coverage", "must be `clothed` or `nude`"))?,
        ),
    };
    let mut overrides = params.sliders.clone();
    let (coverage, coverage_percent) =
        sculpto_bake::apply_coverage(set, requested, &mut overrides).map_err(errors::bake)?;
    let plan = BakePlan::resolve_overrides(&project, preset, &params.set, size, &home, &overrides)
        .map_err(errors::bake)?;
    if coverage_percent.is_some_and(|percent| percent != 0.0)
        && !plan
            .sliders
            .iter()
            .any(|slider| slider.name == sculpto_bake::COVERAGE_SLIDER)
    {
        // The set declares the morph, but the plan did not take it: either the
        // morph data stayed unresolved or the slider class is deferred (UV,
        // clamp, zap). A clothed body that lost its coverage morph is the
        // silent miss this refuses. @req BODY-01
        let detail = if plan
            .unresolved
            .iter()
            .any(|reference| reference.slider == sculpto_bake::COVERAGE_SLIDER)
        {
            "unresolved morph data"
        } else if plan
            .deferred
            .iter()
            .any(|slider| slider.name == sculpto_bake::COVERAGE_SLIDER)
        {
            "deferred slider class"
        } else {
            "not planned"
        };
        return Err(errors::bake(BakeError::CoverageUnapplied {
            set: set.name.clone(),
            slider: sculpto_bake::COVERAGE_SLIDER.to_string(),
            detail: detail.to_string(),
        }));
    }
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
    let target = set.target_for_shape(&shape.name).unwrap_or(&shape.name);
    let baked = plan.apply(target, &shape.positions).map_err(errors::bake)?;
    write_obj(&params.out, &baked, &shape.triangles)
        .map_err(|error| errors::io(&params.out, error))?;
    Ok(BakeOutcome {
        out: params.out.clone(),
        vertices: baked.len(),
        triangles: shape.triangles.len(),
        sliders: plan.sliders.len(),
        unresolved: plan.unresolved.len(),
        deferred: plan.deferred.len(),
        coverage,
        coverage_percent,
    })
}

/// Everything `export.obj` needs.
#[derive(Debug, Clone, JsonSchema)]
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

/// Everything `export.pair` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct ExportPairParams {
    /// Input NIF; its Bethesda user version selects the written target
    /// (SE is 12, LE is 11).
    pub nif: PathBuf,
    /// `.osp` project.
    pub project: PathBuf,
    /// `SliderPresets` collection naming the preset.
    pub presets: PathBuf,
    /// Preset name; both endpoint plans resolve through it.
    pub preset: String,
    /// Slider set name.
    pub set: String,
    /// Output stem: the pair lands at `<out>_0.nif` and `<out>_1.nif`. A
    /// trailing `.nif` and a trailing `_0`/`_1` are stripped first, so any
    /// output path of a previous run is accepted back unchanged.
    pub out: PathBuf,
    /// Two endpoints, `["small"|"big", "small"|"big"]`: the first resolves the
    /// `_0` file, the second the `_1` file. Defaults to small, big.
    pub size: Option<Vec<String>>,
    /// Shape name; the largest shape when absent.
    pub shape: Option<String>,
    /// Optional family skeleton NIF to complete the source joint hierarchy.
    pub rig_template: Option<PathBuf>,
    /// BodySlide home; the project's grandparent when absent.
    pub home: Option<PathBuf>,
}

/// What `export.pair` produced.
#[derive(Debug, Clone)]
pub struct ExportPairOutcome {
    /// Output of the first endpoint (the `_0` file).
    pub out_0: PathBuf,
    /// Output of the second endpoint (the `_1` file).
    pub out_1: PathBuf,
    /// Endpoint the `_0` file carries.
    pub size_0: SliderSize,
    /// Endpoint the `_1` file carries.
    pub size_1: SliderSize,
    /// Written target: `se` or `le`.
    pub target: &'static str,
    /// Shape name.
    pub shape: String,
    /// Vertices per file.
    pub vertices: usize,
    /// Triangles per file.
    pub triangles: usize,
    /// Native lane reductions for the `_0` and `_1` files.
    pub dropped_influences: [Vec<DroppedInfluence>; 2],
    /// Zap outcome per file (`_0`, `_1`): kept, deleted, hidden or dropped.
    pub zap: [ZapFile; 2],
}

/// The `big`/`small` wire token, shared by `bake` and `export.pair`.
fn endpoint(field: &str, value: &str) -> Result<SliderSize, CommandError> {
    match value {
        "big" => Ok(SliderSize::Big),
        "small" => Ok(SliderSize::Small),
        other => Err(errors::invalid(
            field,
            format!("must be big or small, got `{other}`"),
        )),
    }
}

/// Strip a trailing `.nif` and a trailing `_0`/`_1` from the caller's `out`.
///
/// The rule is total: every input yields exactly one pair (`<stem>_0.nif`,
/// `<stem>_1.nif`), and re-running with either output of a previous run
/// reproduces the same pair, so no input is ambiguous enough to refuse.
fn pair_stem(out: &Path) -> PathBuf {
    let Some(name) = out.file_name().and_then(std::ffi::OsStr::to_str) else {
        return out.to_path_buf();
    };
    let without_extension = name.strip_suffix(".nif").unwrap_or(name);
    let stem = without_extension
        .strip_suffix("_0")
        .or_else(|| without_extension.strip_suffix("_1"))
        .unwrap_or(without_extension);
    match out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(stem),
        _ => PathBuf::from(stem),
    }
}

/// The written target follows the input's BS stream version, so a pair always
/// matches its source game.
///
/// Skyrim LE and SE both carry user version 12; only `user_version_2` (BSVER
/// 83 vs 100) separates them. User version still disambiguates files whose
/// stream version is absent, and 11 marks a legacy Fallout-era or pre-fix LE
/// header.
fn nif_target(bsver: u32, user_version: u32) -> Result<NifTarget, CommandError> {
    match (bsver, user_version) {
        (83, _) => Ok(NifTarget::Le),
        (100..=129, _) => Ok(NifTarget::Se),
        (_, 12) => Ok(NifTarget::Se),
        (_, 11) => Ok(NifTarget::Le),
        (bsver, user_version) => Err(CommandError::new(
            ErrorKind::Unsupported,
            "nif_target_unsupported",
            format!(
                "input BS version {bsver} with user version {user_version} is neither SE nor LE"
            ),
        )),
    }
}

/// Bake one shape at both weight endpoints and write them as NIFs.
///
/// BodySlide's build names the low-weight output `<file>_0.nif` and the
/// high-weight output `<file>_1.nif`; both plans resolve through the same
/// preset, so the pair differs only in the endpoint that produced it. Both
/// byte buffers are assembled before either file is written: a refused model
/// or a failed second plan leaves the output directory untouched.
pub fn export_pair(params: &ExportPairParams) -> Result<ExportPairOutcome, CommandError> {
    let sizes = match &params.size {
        None => [SliderSize::Small, SliderSize::Big],
        Some(values) if values.len() == 2 => {
            [endpoint("size", &values[0])?, endpoint("size", &values[1])?]
        }
        Some(values) => {
            return Err(errors::invalid(
                "size",
                format!("must be two endpoints, got {}", values.len()),
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
    let plans = [
        BakePlan::resolve(&project, preset, &params.set, sizes[0], &home).map_err(errors::bake)?,
        BakePlan::resolve(&project, preset, &params.set, sizes[1], &home).map_err(errors::bake)?,
    ];
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let target = nif_target(document.bsver, document.header.user_version)?;
    let index = pick_shape_index(&document.shapes, params.shape.as_deref())?;
    let shape = &document.shapes[index];
    let set = project
        .sets
        .iter()
        .find(|set| set.name == params.set)
        .ok_or_else(|| errors::not_found("set", &params.set))?;
    let morph_target = set.target_for_shape(&shape.name).unwrap_or(&shape.name);
    let rig = export_rig(&document, index, params.rig_template.as_deref())?;
    let source = rig.as_deref().map(|rig| (&document, rig, index));
    let base = MeshAsset::from(shape);
    let keep_zapped = set.output_file.keep_zapped_shapes;
    // Reference order per shape: morphs, then the already-present normals and
    // tangents, then deletion (BodySlideApp.cpp:4408-4434). Morphs move only
    // positions, so deletion is applied to the morphed arena. @req BLD-01
    let encode = |plan: &BakePlan| -> Result<(Option<WrittenNif>, ZapFile), CommandError> {
        let mut asset = base.clone();
        asset.positions = plan
            .apply(morph_target, &shape.positions)
            .map_err(errors::bake)?;
        let Some(indices) = zap_indices_for(plan, morph_target) else {
            return encode_nif(&asset, target, source)
                .map(|written| (Some(written), ZapFile::Kept));
        };
        if keep_zapped && indices.len() == asset.positions.len() {
            // Native keeps a fully zapped shape in the file with its hidden
            // flag set instead of deleting it (BodySlideApp.cpp:4420-4421).
            asset.hidden = true;
            return encode_nif(&asset, target, source)
                .map(|written| (Some(written), ZapFile::Hidden));
        }
        let removed = asset.remove_vertices(&indices).map_err(errors::mesh)?;
        if removed.positions.is_empty() || removed.triangles.is_empty() {
            // Fully zapped and not kept: the shape is dropped, so this
            // endpoint writes no file (DeleteShape, BodySlideApp.cpp:4426).
            return Ok((None, ZapFile::Dropped));
        }
        let zap = ZapFile::Deleted {
            vertices: removed.positions.len(),
            triangles: removed.triangles.len(),
        };
        encode_nif_deleted(&removed, target, source, &indices).map(|written| (Some(written), zap))
    };
    let [(body_0, zap_0), (body_1, zap_1)] = [encode(&plans[0])?, encode(&plans[1])?];
    let stem = pair_stem(&params.out);
    let outs = [
        PathBuf::from(format!("{}_0.nif", stem.display())),
        PathBuf::from(format!("{}_1.nif", stem.display())),
    ];
    for (out, body) in outs.iter().zip([&body_0, &body_1]) {
        if let Some(body) = body {
            std::fs::write(out, &body.bytes).map_err(|error| errors::io(out, error))?;
        }
    }
    Ok(ExportPairOutcome {
        out_0: outs[0].clone(),
        out_1: outs[1].clone(),
        size_0: sizes[0],
        size_1: sizes[1],
        target: match target {
            NifTarget::Se => "se",
            NifTarget::Le => "le",
        },
        shape: shape.name.clone(),
        vertices: shape.positions.len(),
        triangles: shape.triangles.len(),
        dropped_influences: [body_0, body_1]
            .map(|body| body.map_or_else(Vec::new, |body| body.dropped_influences)),
        zap: [zap_0, zap_1],
    })
}

/// How zap sliders left the built shape in one output file. @req BLD-01
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZapFile {
    /// No zap fired for this endpoint; the whole shape survives.
    Kept,
    /// Zaps deleted vertices; the counts are the surviving arena that was
    /// written.
    Deleted { vertices: usize, triangles: usize },
    /// Every vertex is zapped and `KeepZappedShapes` held the shape in the
    /// file, hidden.
    Hidden,
    /// Every vertex (or every triangle) is zapped and the shape was dropped;
    /// no file was written for this endpoint.
    Dropped,
}

impl ZapFile {
    /// Stable wire name for the JSON surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::Deleted { .. } => "deleted",
            Self::Hidden => "hidden",
            Self::Dropped => "dropped",
        }
    }
}

/// The zap indices one plan deletes for `target`, when it deletes any.
fn zap_indices_for(plan: &BakePlan, target: &str) -> Option<Vec<u16>> {
    plan.zap_indices()
        .into_iter()
        .find(|(name, indices)| name == target && !indices.is_empty())
        .map(|(_, indices)| indices)
}

/// Target game version of a written NIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
pub enum ExportTarget {
    /// Skyrim Special Edition, one `BSTriShape`.
    #[schemars(rename = "se")]
    Se,
    /// Skyrim Legendary Edition, one `NiTriShape`.
    #[schemars(rename = "le")]
    Le,
}

impl ExportTarget {
    /// Parse the wire name used by the JSON surfaces.
    pub fn parse(name: &str) -> Result<Self, CommandError> {
        match name {
            "se" => Ok(Self::Se),
            "le" => Ok(Self::Le),
            other => Err(errors::invalid(
                "target",
                format!("must be se or le, got `{other}`"),
            )),
        }
    }

    /// Stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Se => "se",
            Self::Le => "le",
        }
    }
}

/// Everything `export.nif` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct ExportNifParams {
    /// Input NIF.
    pub nif: PathBuf,
    /// Output NIF.
    pub out: PathBuf,
    /// Target game version.
    pub target: ExportTarget,
    /// Shape name; the largest shape when absent.
    pub shape: Option<String>,
    /// Optional family skeleton NIF to complete the source joint hierarchy.
    pub rig_template: Option<PathBuf>,
}

/// What `export.nif` produced.
#[derive(Debug, Clone)]
pub struct ExportNifOutcome {
    /// Output path.
    pub out: PathBuf,
    /// Target written.
    pub target: ExportTarget,
    /// Vertices written.
    pub vertices: usize,
    /// Triangles written.
    pub triangles: usize,
    /// Bytes written.
    pub bytes: usize,
    /// Influences omitted from the target's four-lane view.
    pub dropped_influences: Vec<DroppedInfluence>,
}

/// Re-encode one shape of a source NIF for the requested Skyrim target.
///
/// A skinned shape whose source names no skeleton root (a writer-built bundle)
/// takes the unproven writer instead of the checked rest proof, which has no
/// authored root to anchor. A requested `rig_template` still refuses when the
/// completed family rig cannot satisfy that proof.
///
/// # Errors
/// An unknown shape name is `shape_not_found`; mesh, rig and writer refusals map
/// through [`errors::mesh`], [`errors::nif_write`] and [`errors::invalid`]. No
/// output file is written on any failure.
pub fn export_nif(params: &ExportNifParams) -> Result<ExportNifOutcome, CommandError> {
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let index = match &params.shape {
        // The caller chose from a list, so the miss names what the file holds.
        Some(name) => document
            .shapes
            .iter()
            .position(|shape| shape.name == *name)
            .ok_or_else(|| unknown_shape(&document.shapes, name))?,
        None => pick_shape_index(&document.shapes, None)?,
    };
    let asset = MeshAsset::from(&document.shapes[index]);
    let rig = export_rig(&document, index, params.rig_template.as_deref())?;
    let source = rig.as_deref().map(|rig| (&document, rig, index));
    let written = encode_nif(&asset, params.target.into(), source)?;
    std::fs::write(&params.out, &written.bytes).map_err(|error| errors::io(&params.out, error))?;
    Ok(ExportNifOutcome {
        out: params.out.clone(),
        target: params.target,
        vertices: asset.positions.len(),
        triangles: asset.triangles.len(),
        bytes: written.bytes.len(),
        dropped_influences: written.dropped_influences,
    })
}

/// Validate one model and encode it for the target.
///
pub fn write_nif(asset: &MeshAsset, target: ExportTarget) -> Result<WrittenNif, CommandError> {
    encode_nif(asset, target.into(), None)
}

/// Everything `export.bundle` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct ExportBundleParams {
    /// Input NIF.
    pub nif: PathBuf,
    /// Output NIF.
    pub out: PathBuf,
    /// Target game version.
    pub target: ExportTarget,
    /// Shape names in export order; every document shape in scene order when absent.
    pub shapes: Option<Vec<String>>,
}

/// One shape's written counts, in bundle order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeCount {
    /// Shape name, as authored.
    pub name: String,
    /// Vertices written for this shape.
    pub vertices: usize,
    /// Triangles written for this shape.
    pub triangles: usize,
}

/// What `export.bundle` produced.
#[derive(Debug, Clone)]
pub struct ExportBundleOutcome {
    /// Output path.
    pub out: PathBuf,
    /// Target written.
    pub target: ExportTarget,
    /// Per-shape counts in write order.
    pub shapes: Vec<ShapeCount>,
    /// Total vertices written.
    pub vertices: usize,
    /// Total triangles written.
    pub triangles: usize,
    /// Bytes written.
    pub bytes: usize,
}

/// Re-encode all or selected shapes of a source NIF into one multi-shape NIF.
///
/// Names, geometry and skins travel through [`MeshAsset`] unchanged, so the
/// name-based role attribution and each shape's binding survive the re-encode.
/// The landed bundle writers own the encoding with `InfluencePolicy::Preserve`:
/// an unrepresentable weight row is refused, never silently dropped.
///
/// # Errors
/// An unknown shape name is `shape_not_found`; bundle defects (no shapes,
/// repeated names, palettes with no common skeleton root) and per-shape writer
/// refusals map through [`errors::nif_write`]. No output file is written on any
/// failure.
pub fn export_nif_bundle(params: &ExportBundleParams) -> Result<ExportBundleOutcome, CommandError> {
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let selected = match &params.shapes {
        // The caller chose from a list, so a miss names what the file holds.
        Some(names) => names
            .iter()
            .map(|name| {
                document
                    .shapes
                    .iter()
                    .find(|shape| shape.name == *name)
                    .ok_or_else(|| unknown_shape(&document.shapes, name))
            })
            .collect::<Result<Vec<_>, _>>()?,
        None => document.shapes.iter().collect(),
    };
    let mut assets = selected
        .iter()
        .map(|shape| MeshAsset::from(*shape))
        .collect::<Vec<_>>();
    for asset in &assets {
        asset.validate().map_err(errors::mesh)?;
    }
    // Same LE projection as `encode_nif`: the LE writer refuses authored
    // tangents, so the target projection omits them instead of refusing every
    // real shape. SE writes them.
    if params.target == ExportTarget::Le {
        for asset in &mut assets {
            asset.tangents.clear();
        }
    }
    let bytes = match params.target {
        ExportTarget::Se => write_se_bundle(&assets),
        ExportTarget::Le => write_le_bundle(&assets),
    }
    .map_err(errors::nif_write)?;
    std::fs::write(&params.out, &bytes).map_err(|error| errors::io(&params.out, error))?;
    let shapes = assets
        .iter()
        .map(|asset| ShapeCount {
            name: asset.name.clone(),
            vertices: asset.positions.len(),
            triangles: asset.triangles.len(),
        })
        .collect::<Vec<_>>();
    Ok(ExportBundleOutcome {
        out: params.out.clone(),
        target: params.target,
        vertices: shapes.iter().map(|shape| shape.vertices).sum(),
        triangles: shapes.iter().map(|shape| shape.triangles).sum(),
        shapes,
        bytes: bytes.len(),
    })
}

/// One sparse offset in [`ExportTriParams`]: a base-shape vertex and its move.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriOffsetParams {
    /// Index into the base shape's vertex arena.
    pub vertex: u32,
    /// Displacement in the base shape's units.
    pub position: [f32; 3],
}

/// One named `.tri` morph: the slider Outfit Studio will list, and its offsets.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TriMorphParams {
    /// Slider name; native Outfit Studio adds an empty slider when the project
    /// does not own the name yet.
    pub name: String,
    /// Sparse offsets in caller order.
    pub offsets: Vec<TriOffsetParams>,
}

/// Everything `export.tri` needs.
///
/// The command is the product surface over [`sculpto_nif::write_tri`]; it carries
/// its morph data instead of reading it from a NIF, because no reader produces
/// morph targets today. `shape` and `vertex_count` are required: the format
/// binds morphs to a shape by name and addresses base-shape vertices, so a
/// default would invent the identity the file claims. @req DECL0007
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportTriParams {
    /// Output `.tri` path.
    pub out: PathBuf,
    /// Shape name the morphs bind to.
    pub shape: String,
    /// Vertex count of the base shape the offsets address.
    pub vertex_count: u32,
    /// Morphs in write order.
    pub morphs: Vec<TriMorphParams>,
}

/// What `export.tri` produced.
#[derive(Debug, Clone)]
pub struct ExportTriOutcome {
    /// Output path.
    pub out: PathBuf,
    /// Shape name written.
    pub shape: String,
    /// Vertex count written.
    pub vertex_count: u32,
    /// Morphs written.
    pub morphs: usize,
    /// Sparse offsets written across every morph.
    pub offsets: usize,
    /// Bytes written.
    pub bytes: usize,
}

/// Write sparse morph targets as a Skyrim `.tri` slider file.
///
/// The bytes come from the one landed encoder: the parameters project into the
/// core `.tri` model, which [`sculpto_nif::TriShape`] converts into the writer's
/// shape, so no second encoder exists. The writer's own limits (name length,
/// `u16` morph and vertex counts, offsets inside `vertex_count`, one offset per
/// vertex) refuse before anything is written.
///
/// # Errors
/// A non-finite displacement and a vertex index above the format's `u16` field
/// are `invalid_value`; writer refusals map through [`errors::tri`] and the file
/// I/O through [`errors::io`]. No output file is written on any failure.
pub fn export_tri(params: &ExportTriParams) -> Result<ExportTriOutcome, CommandError> {
    let shape = sculpto_core::mesh::TriShape {
        name: params.shape.clone(),
        vertex_count: params.vertex_count,
        morphs: params
            .morphs
            .iter()
            .map(|morph| {
                let offsets = morph
                    .offsets
                    .iter()
                    .map(|offset| {
                        // The `.tri` field is a `u16` while a caller names a
                        // model-space index, so the narrowing is refused, never
                        // truncated.
                        let vertex = u16::try_from(offset.vertex).map_err(|_| {
                            errors::invalid(
                                "morphs",
                                format!(
                                    "morph `{}` addresses vertex {}, above the `.tri` `u16` field",
                                    morph.name, offset.vertex
                                ),
                            )
                        })?;
                        if !offset.position.iter().all(|lane| lane.is_finite()) {
                            return Err(errors::invalid(
                                "morphs",
                                format!(
                                    "morph `{}` has a non-finite offset at vertex {}",
                                    morph.name, offset.vertex
                                ),
                            ));
                        }
                        Ok(sculpto_core::mesh::TriOffset {
                            vertex,
                            position: offset.position,
                        })
                    })
                    .collect::<Result<Vec<_>, CommandError>>()?;
                Ok(sculpto_core::mesh::TriMorph {
                    name: morph.name.clone(),
                    offsets,
                })
            })
            .collect::<Result<Vec<_>, CommandError>>()?,
    };
    let file = TriFile::new(vec![sculpto_nif::TriShape::from(&shape)]);
    let bytes = write_tri(&file).map_err(errors::tri)?;
    std::fs::write(&params.out, &bytes).map_err(|error| errors::io(&params.out, error))?;
    Ok(ExportTriOutcome {
        out: params.out.clone(),
        shape: shape.name,
        vertex_count: shape.vertex_count,
        morphs: shape.morphs.len(),
        offsets: shape.morphs.iter().map(|morph| morph.offsets.len()).sum(),
        bytes: bytes.len(),
    })
}

impl From<ExportTarget> for NifTarget {
    fn from(value: ExportTarget) -> Self {
        match value {
            ExportTarget::Se => Self::Se,
            ExportTarget::Le => Self::Le,
        }
    }
}

fn export_rig<'a>(
    document: &'a NifDocument,
    index: usize,
    template_path: Option<&Path>,
) -> Result<Option<Cow<'a, Rig>>, CommandError> {
    if document.shapes[index].skin.is_none() {
        return match template_path {
            Some(_) => Err(errors::invalid("rig_template", "requires a skinned shape")),
            None => Ok(None),
        };
    }
    let authored = document
        .rig
        .as_ref()
        .ok_or_else(|| errors::nif_write(NifWriteError::InvalidRig(NifRigError::MissingRig)))?;
    let Some(path) = template_path else {
        // The bundle writer leaves its scene root unnamed, so a writer-built
        // source re-reads with no captured skeleton root and the checked rest
        // proof has nothing to anchor. The unproven writer keeps that export
        // available; an authored root, or a requested family template, still
        // takes the checked path. @req DECL0008
        return match document
            .sources
            .get(index)
            .and_then(|source| source.skeleton_root.as_deref())
        {
            Some(_) => Ok(Some(Cow::Borrowed(authored))),
            None => Ok(None),
        };
    };
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let template = document.rig.as_ref().ok_or_else(|| {
        errors::invalid(
            "rig_template",
            format!("{} contains no node graph", path.display()),
        )
    })?;
    let ([authored_root], [template_root]) = (authored.roots(), template.roots()) else {
        return Err(errors::invalid(
            "rig_template",
            "the source and template each need one scene root",
        ));
    };
    let completed = complete_family_rig(
        authored,
        template,
        TemplateRoots {
            authored: *authored_root,
            template: *template_root,
        },
    )
    .map_err(|error| {
        CommandError::new(
            ErrorKind::Refused,
            "nif_rig_template_incompatible",
            format!("{}: {error}", path.display()),
        )
    })?;
    Ok(Some(Cow::Owned(completed)))
}

fn encode_nif(
    asset: &MeshAsset,
    target: NifTarget,
    source: Option<(&NifDocument, &Rig, usize)>,
) -> Result<WrittenNif, CommandError> {
    encode_nif_with_zap(asset, target, source, None)
}

/// [`encode_nif`] after zap deletion: a skinned source re-encodes through the
/// deletion-checked rig binding, so the zapped file keeps its skeleton. @req
/// BLD-01
fn encode_nif_with_zap(
    asset: &MeshAsset,
    target: NifTarget,
    source: Option<(&NifDocument, &Rig, usize)>,
    deleted: Option<&[u16]>,
) -> Result<WrittenNif, CommandError> {
    asset.validate().map_err(errors::mesh)?;
    // This LE writer has no NBT layout and refuses authored tangents. A
    // texture-ready model always carries them (the reader synthesizes them from
    // UVs and normals), so the target projection omits them instead of refusing
    // every real shape. SE writes them.
    let projected = match target {
        NifTarget::Se => Cow::Borrowed(asset),
        NifTarget::Le => {
            let mut projection = asset.clone();
            projection.tangents.clear();
            Cow::Owned(projection)
        }
    };
    let output = match (source, deleted) {
        (Some((document, rig, index)), Some(deleted)) => write_rigged_shape_deleted(
            &projected,
            document,
            rig,
            index,
            deleted,
            target,
            InfluencePolicy::NativeFour,
        ),
        (Some((document, rig, index)), None) => write_rigged_shape(
            &projected,
            document,
            rig,
            index,
            target,
            InfluencePolicy::NativeFour,
        ),
        (None, _) => write_shape_with_policy(&projected, target, InfluencePolicy::NativeFour),
    };
    output.map_err(errors::nif_write)
}

/// Deletion-aware [`encode_nif`]: `deleted` is the zap index list already
/// applied to `asset`.
fn encode_nif_deleted(
    asset: &MeshAsset,
    target: NifTarget,
    source: Option<(&NifDocument, &Rig, usize)>,
    deleted: &[u16],
) -> Result<WrittenNif, CommandError> {
    encode_nif_with_zap(asset, target, source, Some(deleted))
}

/// Everything `gltf.reset_weights` and `gltf.clamp_texcoord` need.
#[derive(Debug, Clone, JsonSchema)]
pub struct GlbFileParams {
    /// GLB file to repair in place.
    pub glb: PathBuf,
}

/// What a GLB repair produced.
#[derive(Debug, Clone)]
pub struct GlbRepairOutcome {
    /// Repaired file.
    pub glb: PathBuf,
    /// JSON sites the repair changed; zero leaves the bytes untouched.
    pub changed: usize,
    /// File size after the repair.
    pub bytes: usize,
}

/// Zero every mesh's default morph weights in a GLB.
pub fn gltf_reset_weights(params: &GlbFileParams) -> Result<GlbRepairOutcome, CommandError> {
    repair_glb(params, sculpto_gltf::sanitize::reset_mesh_weights)
}

/// Clamp every `texCoord: -1` in a GLB's materials to UV set 0.
pub fn gltf_clamp_texcoord(params: &GlbFileParams) -> Result<GlbRepairOutcome, CommandError> {
    repair_glb(params, sculpto_gltf::sanitize::clamp_texcoord)
}

/// Apply one adapter repair to the file it names.
///
/// The adapter owns the container; this function owns the read and the write,
/// and an already-clean document is not rewritten, so a re-run is a no-op.
fn repair_glb(
    params: &GlbFileParams,
    repair: fn(&[u8]) -> Result<Repair, SanitizeError>,
) -> Result<GlbRepairOutcome, CommandError> {
    let bytes = std::fs::read(&params.glb).map_err(|error| errors::io(&params.glb, error))?;
    let repaired = repair(&bytes).map_err(errors::gltf)?;
    let size = match &repaired.bytes {
        Some(rewritten) => {
            std::fs::write(&params.glb, rewritten)
                .map_err(|error| errors::io(&params.glb, error))?;
            rewritten.len()
        }
        None => bytes.len(),
    };
    Ok(GlbRepairOutcome {
        glb: params.glb.clone(),
        changed: repaired.changed,
        bytes: size,
    })
}

fn unknown_shape(shapes: &[NifShape], name: &str) -> CommandError {
    let available: Vec<&str> = shapes.iter().map(|shape| shape.name.as_str()).collect();
    let listed = match available.is_empty() {
        true => "none".to_string(),
        false => available.join(", "),
    };
    CommandError::new(
        ErrorKind::NotFound,
        "shape_not_found",
        format!("shape `{name}` not found; available shapes: {listed}"),
    )
}

/// Everything `outfit.fit` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct FitParams {
    /// Garment NIF.
    pub nif: PathBuf,
    /// Garment shape.
    pub shape: String,
    /// Body shape inside the same NIF, the authored reference. Required unless
    /// `body_ref` supplies an external reference on the plain fit path.
    pub body_shape: Option<String>,
    /// Morphed body OBJ.
    pub body: PathBuf,
    /// Output OBJ.
    pub out: PathBuf,
    /// External reference body OBJ with the target body's topology. When given,
    /// the garment binds to this control surface in triangle frames instead of
    /// the embedded `body_shape`, so a culled NIF body no longer locks the fit.
    pub body_ref: Option<PathBuf>,
    /// Optional second body for the follow measurement.
    pub body_after: Option<PathBuf>,
    /// Output for the follow measurement.
    pub out_after: Option<PathBuf>,
    /// Generated garment OSD for a slider preview.
    pub conform: Option<PathBuf>,
    /// Name of the conformed slider to apply.
    pub slider: Option<String>,
    /// Slider percentage for the selected block.
    pub percent: Option<f32>,
    /// Source body NIF for a slider or manifest fit. Defaults to the garment NIF.
    pub body_nif: Option<PathBuf>,
    /// Source-bound region manifest. All declared shapes belong to the garment NIF.
    pub manifest: Option<PathBuf>,
    /// Optional combined collision OBJ: body and every separate part a garment
    /// can reach (hands, feet), triangulated in one declared frame.
    pub collision: Option<PathBuf>,
    /// Declared frame of `collision`. The only supported value is
    /// `source-scene`: NIF source-scene coordinates in the OBJ display frame.
    pub collision_frame: Option<String>,
    /// Passes the opt-in final push-out stage may run. Absent for no pass.
    /// @req DECL0013
    pub final_push: Option<u32>,
    /// Accepted visible depth of the final push-out stage, in input units.
    /// Absent for no pass; required together with `final_push`.
    pub final_push_tolerance: Option<f32>,
}

impl FitParams {
    /// The embedded body shape name the legacy and guarded paths bind to.
    ///
    /// `body_ref` replaces the embedded reference on the plain path only; every
    /// other path still needs the named shape.
    pub fn required_body_shape(&self) -> Result<&str, CommandError> {
        self.body_shape.as_deref().ok_or_else(|| {
            CommandError::new(
                ErrorKind::InvalidParams,
                "missing_field",
                "`body_shape` is required when `body_ref` is absent",
            )
        })
    }

    /// Whether a guarded, preview or follow stage is requested.
    fn guarded_request(&self) -> bool {
        self.conform.is_some()
            || self.slider.is_some()
            || self.percent.is_some()
            || self.body_nif.is_some()
            || self.manifest.is_some()
            || self.collision.is_some()
            || self.collision_frame.is_some()
            || self.final_push.is_some()
            || self.final_push_tolerance.is_some()
            || self.body_after.is_some()
            || self.out_after.is_some()
    }

    /// Resolve the opt-in final push-out request.
    ///
    /// Half a request is refused by name instead of being read as either
    /// extreme: a pass budget without a tolerance has no accepted depth, and a
    /// tolerance without a budget cannot run. @req DECL0013
    pub fn final_push(&self) -> Result<Option<FinalPushOptions>, CommandError> {
        match (self.final_push, self.final_push_tolerance) {
            (None, None) => Ok(None),
            (Some(passes), Some(tolerance)) => Ok(Some(FinalPushOptions { passes, tolerance })),
            (Some(_), None) => Err(errors::invalid(
                "final_push_tolerance",
                "required when `final_push` is given",
            )),
            (None, Some(_)) => Err(errors::invalid(
                "final_push",
                "required when `final_push_tolerance` is given",
            )),
        }
    }
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
    /// Mean authored offset against the reference, nearest-vertex metric in
    /// both transports.
    pub offset_mean: f32,
    /// Largest authored offset against the reference.
    pub offset_max: f32,
    /// Follow measurement when a second body was given.
    pub follow: Option<(f32, f32, f32)>,
    /// Triangle contacts against the selected or caller-declared obstacle.
    pub intersection_pairs: Option<usize>,
    /// Inside groups when previewing a conformed slider.
    pub inside_after: Option<usize>,
    /// Explicit ordinary and cloth measurements when a manifest was supplied.
    pub regions: Option<sculpto_mesh::outfit::rigging::RegionFitReport>,
    /// Guarded preview measurements when no region manifest was supplied.
    pub repair: Option<sculpto_mesh::wrap::repair::FitRepairReport>,
    /// The caller-declared collision scope that was measured, when one was given.
    pub collision: Option<CollisionScope>,
    /// Binding provenance of the plain fit path; guarded paths report their
    /// binding through `regions`/`repair` instead.
    pub provenance: Option<FitProvenance>,
}

/// Which control surface a plain fit bound to, and how it was transported.
///
/// `reference_body_sha256` digests the reference geometry that built the
/// binding: the embedded shape's position bytes for `embedded_nif`, the
/// reference OBJ file bytes for `external_reference`. `target_body_sha256` is
/// always the target OBJ file bytes.
#[derive(Debug, Clone)]
pub struct FitProvenance {
    /// `embedded_nif` or `external_reference`.
    pub reference_source: &'static str,
    /// File the reference geometry was read from.
    pub reference_path: PathBuf,
    /// SHA-256 of the reference geometry (embedded positions or OBJ file bytes).
    pub reference_body_sha256: String,
    /// Reference control surface vertices.
    pub reference_vertices: usize,
    /// Reference control surface triangles.
    pub reference_triangles: usize,
    /// Target body OBJ.
    pub target_path: PathBuf,
    /// SHA-256 of the target body file bytes.
    pub target_body_sha256: String,
    /// Target body vertices.
    pub target_vertices: usize,
    /// Target body triangles; `None` on the positions-only legacy transport,
    /// which never reads target faces.
    pub target_triangles: Option<usize>,
    /// `nearest_vertex_offset` or `surface_triangle_frame`.
    pub transport_method: &'static str,
    /// Signed normal-offset range of the garment against the reference control
    /// surface, from the triangle-frame binding; `None` on the legacy transport,
    /// which stores no normal frame. @req DECL0013
    pub normal_offset_range: Option<[f64; 2]>,
}

/// The collision surface a fit actually measured against.
///
/// The scope is what the caller declared. The tool cannot prove that it covers
/// a whole character, so this is not whole-actor clearance by itself. @req DECL0013
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollisionScope {
    /// Declared input frame.
    pub frame: String,
    /// Surface vertices measured as obstacles.
    pub vertices: usize,
    /// Surface triangles measured as obstacles.
    pub triangles: usize,
    /// Always `caller-declared`: the tool does not verify scope completeness.
    pub scope: &'static str,
}

/// Bind a garment to its authored body and apply the bind to a morphed body.
///
/// With `body_ref` the garment binds to an external reference control surface in
/// triangle frames ([`SurfaceBinding`]) and transports onto the target body, so a
/// culled NIF body no longer locks the fit. Without it the legacy embedded-shape
/// [`Conform`] runs unchanged.
pub fn garment_fit(params: &FitParams) -> Result<FitOutcome, CommandError> {
    if params.body_ref.is_some() && params.guarded_request() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "outfit_fit_body_ref_scope",
            "`body_ref` retargets the plain fit only; drop `manifest`, `conform`/`slider`/`percent`, `body_nif`, `collision`, `final_push` and `body_after`/`out_after` to use it",
        ));
    }
    if params.conform.is_some()
        || params.slider.is_some()
        || params.percent.is_some()
        || (params.body_nif.is_some() && params.manifest.is_none())
    {
        return slider::preview(params);
    }
    if params.manifest.is_some() {
        return slider::fit_regions(params);
    }
    if params.collision.is_some() || params.collision_frame.is_some() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "outfit_fit_collision_scope",
            "a collision surface needs a guarded fit: supply `manifest`, or a `conform`, `slider` and `percent` preview; this path measures no contact",
        ));
    }
    if params.final_push.is_some() || params.final_push_tolerance.is_some() {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "outfit_fit_final_push",
            "the final push-out stage needs a guarded fit: supply `manifest`, or a `conform`, `slider` and `percent` preview; this path grades no visible depth",
        ));
    }
    let bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let garment = fit_shape(&document.shapes, &params.shape, "missing_shape", "shape")?;
    match &params.body_ref {
        Some(reference_path) => fit_retarget(params, garment, reference_path),
        None => fit_legacy(params, &document, garment),
    }
}

/// The legacy single-NIF conform: bind to the embedded body shape, apply to the
/// morphed body. Unchanged geometry; only provenance and the topology hint are new.
fn fit_legacy(
    params: &FitParams,
    document: &NifDocument,
    garment: &NifShape,
) -> Result<FitOutcome, CommandError> {
    let body_shape = params.required_body_shape()?;
    let reference = fit_shape(
        &document.shapes,
        body_shape,
        "missing_body_shape",
        "body shape",
    )?;
    let body = read_obj_positions(&params.body)?;
    let conform =
        Conform::bind(&garment.positions, &reference.positions).map_err(errors::conform)?;
    let fitted = conform.apply(&body).map_err(|error| match error {
        ConformError::Topology { expected, actual } => CommandError::new(
            ErrorKind::Refused,
            "topology_mismatch",
            format!(
                "target body has {actual} vertices, but NIF shape `{}` has {expected}; supply --body-ref with matching base body topology",
                reference.name
            ),
        ),
        other => errors::conform(other),
    })?;
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
    let provenance = FitProvenance {
        reference_source: "embedded_nif",
        reference_path: params.nif.clone(),
        reference_body_sha256: positions_sha256(&reference.positions),
        reference_vertices: reference.positions.len(),
        reference_triangles: reference.triangles.len(),
        target_path: params.body.clone(),
        target_body_sha256: file_sha256(&params.body)?,
        target_vertices: body.len(),
        target_triangles: None,
        transport_method: "nearest_vertex_offset",
        normal_offset_range: None,
    };
    Ok(FitOutcome {
        out: params.out.clone(),
        garment: garment.name.clone(),
        reference: reference.name.clone(),
        vertices: fitted.len(),
        offset_mean: conform.offset_mean(),
        offset_max: conform.offset_max(),
        follow,
        intersection_pairs: None,
        inside_after: None,
        regions: None,
        repair: None,
        collision: None,
        provenance: Some(provenance),
    })
}

/// Retarget the garment binding onto an external reference control surface, then
/// transport it onto the target body with triangle frames.
fn fit_retarget(
    params: &FitParams,
    garment: &NifShape,
    reference_path: &Path,
) -> Result<FitOutcome, CommandError> {
    let (reference, reference_triangles) = read_obj_surface(reference_path)?;
    let (body, body_triangles) = read_obj_surface(&params.body)?;
    if reference.len() != body.len() || reference_triangles != body_triangles {
        let mut message = format!(
            "reference body has {} vertices / {} triangles, but target body has {} vertices / {} triangles",
            reference.len(),
            reference_triangles.len(),
            body.len(),
            body_triangles.len()
        );
        // Equal counts mean the ordered triangle lists differ; name the first
        // divergent triangle so the refusal is actionable, not self-contradictory.
        if reference.len() == body.len() && reference_triangles.len() == body_triangles.len() {
            let first = reference_triangles
                .iter()
                .zip(body_triangles.iter())
                .position(|(reference, target)| reference != target)
                .expect("equal counts with differing lists have a first difference");
            message.push_str(&format!(
                "; ordered triangle lists differ (first at index {first}: reference {:?}, target {:?})",
                reference_triangles[first], body_triangles[first]
            ));
        }
        return Err(CommandError::new(
            ErrorKind::Refused,
            "reference_target_topology_mismatch",
            message,
        ));
    }
    let binding = SurfaceBinding::bind(&garment.positions, &reference, &reference_triangles)
        .map_err(errors::surface)?;
    let fitted = binding
        .apply(&body, &body_triangles)
        .map_err(errors::surface)?;
    write_obj(&params.out, &fitted, &garment.triangles)
        .map_err(|error| errors::io(&params.out, error))?;
    // Authored-thickness provenance only; the transport above is the triangle
    // frame, never this nearest-vertex conform.
    let offsets = Conform::bind(&garment.positions, &reference).map_err(errors::conform)?;
    let reference_name = reference_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| reference_path.display().to_string());
    let provenance = FitProvenance {
        reference_source: "external_reference",
        reference_path: reference_path.to_path_buf(),
        reference_body_sha256: file_sha256(reference_path)?,
        reference_vertices: reference.len(),
        reference_triangles: reference_triangles.len(),
        target_path: params.body.clone(),
        target_body_sha256: file_sha256(&params.body)?,
        target_vertices: body.len(),
        target_triangles: Some(body_triangles.len()),
        transport_method: "surface_triangle_frame",
        normal_offset_range: Some(binding.normal_offset_range()),
    };
    Ok(FitOutcome {
        out: params.out.clone(),
        garment: garment.name.clone(),
        reference: reference_name,
        vertices: fitted.len(),
        offset_mean: offsets.offset_mean(),
        offset_max: offsets.offset_max(),
        follow: None,
        intersection_pairs: None,
        inside_after: None,
        regions: None,
        repair: None,
        collision: None,
        provenance: Some(provenance),
    })
}

/// A named fit shape, refused with the available list when the NIF lacks it.
fn fit_shape<'a>(
    shapes: &'a [NifShape],
    name: &str,
    code: &'static str,
    label: &str,
) -> Result<&'a NifShape, CommandError> {
    shapes
        .iter()
        .find(|shape| shape.name == name)
        .ok_or_else(|| {
            let available: Vec<&str> = shapes.iter().map(|shape| shape.name.as_str()).collect();
            CommandError::new(
                ErrorKind::NotFound,
                code,
                format!(
                    "{label} `{name}` not found in NIF; available: [{}]",
                    available.join(", ")
                ),
            )
        })
}

/// SHA-256 of a file's bytes, hex-encoded.
pub(crate) fn file_sha256(path: &Path) -> Result<String, CommandError> {
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    Ok(sha256_hex(&bytes))
}

/// SHA-256 of embedded reference geometry (little-endian position bytes).
fn positions_sha256(positions: &[[f32; 3]]) -> String {
    let mut bytes = Vec::with_capacity(positions.len() * 12);
    for [x, y, z] in positions {
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.extend_from_slice(&z.to_le_bytes());
    }
    sha256_hex(&bytes)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Everything `intake.check` needs.
#[derive(Debug, Clone, JsonSchema)]
pub struct IntakeCheckParams {
    /// Raw garment export to audit: `.glb`, `.gltf` or `.obj`, as generated.
    pub file: PathBuf,
}

/// What `intake.check` measured.
#[derive(Debug, Clone)]
pub struct IntakeCheckOutcome {
    /// Garment name, as authored; empty when the file names none.
    pub name: String,
    /// Vertex count.
    pub vertices: usize,
    /// Triangle count.
    pub triangles: usize,
    /// The measured defect counts, never repaired.
    pub counts: TopologyDefects,
    /// The limits the verdict was measured against.
    pub limits: DefectLimits,
    /// `accept` while every count is within its limit; over a limit the
    /// command refuses and no outcome exists.
    pub verdict: &'static str,
}

/// Audit a raw garment export: measured defect counts against the declared
/// limits. Read-only: nothing is written, nothing is repaired.
pub fn intake_check(params: &IntakeCheckParams) -> Result<IntakeCheckOutcome, CommandError> {
    let garment = read_raw(&params.file).map_err(errors::intake)?;
    let counts = garment.defects();
    let limits = DefectLimits::default();
    limits.check(counts).map_err(errors::intake_defects)?;
    Ok(IntakeCheckOutcome {
        name: garment.name,
        vertices: garment.positions.len(),
        triangles: garment.triangles.len(),
        counts,
        limits,
        verdict: "accept",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pair_stem_rule_strips_weight_suffixes() {
        let cases = [
            ("out/body", "out/body"),
            ("out/body.nif", "out/body"),
            ("out/body_0", "out/body"),
            ("out/body_1", "out/body"),
            ("out/body_0.nif", "out/body"),
            ("body", "body"),
            ("body_1.nif", "body"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                pair_stem(Path::new(input)),
                PathBuf::from(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn the_pair_stem_rule_is_total() {
        // Every accepted input yields the same pair, so nothing is ambiguous
        // enough to refuse: feeding either output back reproduces the pair.
        for input in ["body", "body_0", "body_1", "body.nif"] {
            let stem = pair_stem(Path::new(input));
            assert_eq!(
                PathBuf::from(format!("{}_0.nif", stem.display())),
                Path::new("body_0.nif"),
                "{input}"
            );
            assert_eq!(
                PathBuf::from(format!("{}_1.nif", stem.display())),
                Path::new("body_1.nif"),
                "{input}"
            );
        }
    }

    #[test]
    fn endpoints_parse_and_refuse() {
        assert_eq!(endpoint("size", "big"), Ok(SliderSize::Big));
        assert_eq!(endpoint("size", "small"), Ok(SliderSize::Small));
        let error = endpoint("size", "huge").expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::InvalidParams);
        assert_eq!(error.code(), "invalid_value");
    }

    #[test]
    fn the_target_follows_the_input_stream_version() {
        assert_eq!(nif_target(100, 12), Ok(NifTarget::Se));
        assert_eq!(nif_target(83, 12), Ok(NifTarget::Le));
        assert_eq!(nif_target(0, 12), Ok(NifTarget::Se));
        assert_eq!(nif_target(0, 11), Ok(NifTarget::Le));
        let error = nif_target(0, 1).expect_err("refused");
        assert_eq!(error.kind(), ErrorKind::Unsupported);
        assert_eq!(error.code(), "nif_target_unsupported");
    }
}
