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

use crate::{CommandError, ErrorKind};
use sculpto_bake::BakePlan;
use sculpto_core::mesh::MeshAsset;
use sculpto_core::rig::Rig;
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::{SliderSize, document::Document};
use sculpto_gltf::sanitize::{Repair, SanitizeError};
use sculpto_mesh::conform::Conform;
use sculpto_mesh::intake_gltf::{DefectLimits, TopologyDefects, read_raw};
use sculpto_mesh::outfit::{OutfitManifest, ShapeInfo, ShapeReport, census};
use sculpto_nif::write::header::{NifTarget, NifWriteError};
use sculpto_nif::write::{
    DroppedInfluence, InfluencePolicy, WrittenNif, write_rigged_shape, write_shape_with_policy,
};
use sculpto_nif::{NifDocument, NifRigError, NifShape};

use crate::errors;
use crate::obj::{pick_shape, pick_shape_index, read_obj_positions, write_obj, write_obj_uv};

mod slider;
pub use slider::{ConformOutcome, ConformParams, conform_sliders};

/// A source NIF and its optional authored outfit declaration.
#[derive(Debug, Clone, JsonSchema)]
pub struct OutfitInspectParams {
    /// Outfit NIF to inspect.
    pub nif: PathBuf,
    /// JSON manifest bound to this source.
    pub manifest: Option<PathBuf>,
}

/// What the outfit contains and how each garment part should move.
#[derive(Debug)]
pub struct OutfitInspectOutcome {
    /// Authored outfit name when a manifest was supplied.
    pub outfit: Option<String>,
    /// Census and cloth selection in source order.
    pub shapes: Vec<ShapeReport>,
}

/// Read an outfit without changing its geometry or rig.
///
/// # Errors
/// Refuses unreadable sources, invalid manifests and selections that cannot
/// bind to the imported shapes.
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
    Ok(OutfitInspectOutcome {
        outfit: manifest.map(|manifest| manifest.outfit),
        shapes,
    })
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
    let mut asset = MeshAsset::from(shape);
    let mut encode = |plan: &BakePlan| {
        asset.positions = plan
            .apply(morph_target, &shape.positions)
            .map_err(errors::bake)?;
        encode_nif(&asset, target, source)
    };
    let bodies = [encode(&plans[0])?, encode(&plans[1])?];
    let stem = pair_stem(&params.out);
    let outs = [
        PathBuf::from(format!("{}_0.nif", stem.display())),
        PathBuf::from(format!("{}_1.nif", stem.display())),
    ];
    for (out, body) in outs.iter().zip(&bodies) {
        std::fs::write(out, &body.bytes).map_err(|error| errors::io(out, error))?;
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
        dropped_influences: bodies.map(|body| body.dropped_influences),
    })
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
        return Ok(Some(Cow::Borrowed(authored)));
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
    let output = match source {
        Some((document, rig, index)) => write_rigged_shape(
            &projected,
            document,
            rig,
            index,
            target,
            InfluencePolicy::NativeFour,
        ),
        None => write_shape_with_policy(&projected, target, InfluencePolicy::NativeFour),
    };
    output.map_err(errors::nif_write)
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
    /// Generated garment OSD for a slider preview.
    pub conform: Option<PathBuf>,
    /// Name of the conformed slider to apply.
    pub slider: Option<String>,
    /// Slider percentage for the selected block.
    pub percent: Option<f32>,
    /// Source body NIF for a slider preview. Defaults to the garment NIF.
    pub body_nif: Option<PathBuf>,
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
    /// Measured triangle contacts when previewing a conformed slider.
    pub intersection_pairs: Option<usize>,
    /// Inside groups when previewing a conformed slider.
    pub inside_after: Option<usize>,
}

/// Bind a garment to its authored body and apply the bind to a morphed body.
pub fn garment_fit(params: &FitParams) -> Result<FitOutcome, CommandError> {
    if params.conform.is_some()
        || params.slider.is_some()
        || params.percent.is_some()
        || params.body_nif.is_some()
    {
        return slider::preview(params);
    }
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
        intersection_pairs: None,
        inside_after: None,
    })
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
