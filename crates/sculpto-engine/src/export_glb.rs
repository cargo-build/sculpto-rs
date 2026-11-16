//! `export.glb`: one checked, clipped actor GLB from a skinned NIF.
//!
//! The standard bodies already exist on disk as single skinned NIFs whose body
//! and native underwear shapes carry authored weights, and the product needs
//! them to *move*: a skinless review GLB cannot. This operation composes the
//! landed building blocks — family rig completion, the rest proof, the shared
//! GLB encoder and the actor evaluator — into one file:
//!
//! 1. select the body and garment shapes by name and refuse any unskinned one;
//! 2. complete the family rig from the declared template NIF;
//! 3. merge body and garments into one [`MeshAsset`]: one palette by bone name,
//!    authored weights preserved, per-part [`PartitionId`]s, every shape
//!    converted into the body's skin frame;
//! 4. prove the merged skin reconstructs the source rest at the body basis;
//! 5. map named clips from a declared source actor GLB by bone name, gate them
//!    against the completed rig's rest, and convert them into the rig's units;
//! 6. write through [`write_rigged_glb`] with the clips in declared order.
//!
//! Everything that would otherwise move silently is a typed refusal: an
//! unskinned or missing shape, an incomplete rig, a clip bone the rig does not
//! name, a rest-incompatible clip, a duplicate part and a constant clip.
//! @req DECL0008

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use glam::{DMat3, DMat4, DQuat, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{REST_TOLERANCE, RestBindError, RestBinding, SkinRestSource};
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::rig::{JointId, Rig, RigFrame};
use sculpto_core::skin::SkinWeightError;
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{Clip, ClipChannel, GltfError, write_rigged_glb};
use sculpto_mesh::actor::{Actor, ActorError, SourceClip, SourceProperty, SourceValues};
use sculpto_nif::{NifDocument, NifShape};

use crate::retarget::Correspondence;
use crate::{CommandError, ErrorKind, errors};

/// Admitted rest bone-position residual, as a fraction of the skeleton extent.
pub const DEFAULT_REST_TRANSLATION_TOLERANCE: f64 = 0.05;
/// Admitted spread of the per-bone unit ratios, relative to their median.
pub const DEFAULT_REST_SCALE_TOLERANCE: f64 = 0.02;
/// Admitted constant bone-roll difference a clip may need normalized, degrees.
/// Above it the clip's own frames are too far from the reference stance for a
/// channel import: the roll removal would be doing the retargeting.
pub const DEFAULT_ROLL_TOLERANCE_DEGREES: f64 = 60.0;
/// A channel whose samples stay within this distance is constant, not motion.
const CONSTANT_EPSILON: f64 = 1e-6;
/// Largest weighted mass a re-derived bind may carry before the source itself
/// is refused: a few stray influences are a local defect, a whole skin is not.
const REPAIR_MASS_CAP: f64 = 0.02;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportGlbParams {
    /// Source NIF that carries the body and its garment shapes.
    pub nif: PathBuf,
    /// Shape name that becomes the skinned body and the palette authority.
    pub body_shape: String,
    /// Additional skinned shapes merged into the same mesh, in this order.
    #[serde(default)]
    pub garment_shapes: Vec<String>,
    /// Family skeleton template that completes the authored rig.
    #[serde(default)]
    pub skeleton: Option<PathBuf>,
    /// Source actor GLB whose clips are mapped by bone name.
    #[serde(default)]
    pub clips: Option<PathBuf>,
    /// Clip names to write, in written order (index 0 first).
    #[serde(default)]
    pub clip_names: Vec<String>,
    /// Source units per metre; the written file is scaled to metres.
    #[serde(default)]
    pub units_per_metre: Option<f64>,
    /// Admitted rest bone-position residual, relative to the skeleton extent.
    #[serde(default)]
    pub rest_translation_tolerance: Option<f64>,
    /// Admitted spread of the per-bone unit ratios, relative to the median.
    #[serde(default)]
    pub rest_scale_tolerance: Option<f64>,
    /// Admitted constant bone-roll difference, degrees.
    #[serde(default)]
    pub rest_roll_tolerance_degrees: Option<f64>,
    /// GLB output path.
    pub out: PathBuf,
    /// Also write the report JSON here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// The command input schema.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(ExportGlbParams))
}

/// The registry handler: parse, export, report.
pub(crate) fn export(input: &Value) -> Result<Value, CommandError> {
    let params: ExportGlbParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "export_glb_input",
            error.to_string(),
        )
    })?;
    run(&params)
}

/// One merged part: the shape it came from and its ranges in the merged arena.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedPart {
    /// Source shape index in the NIF document.
    pub shape: usize,
    /// Authored shape name.
    pub name: String,
    /// Vertex range in the merged arena.
    pub vertices: Range<usize>,
    /// Triangle range in the merged arena.
    pub triangles: Range<usize>,
    /// Written triangle partition id.
    pub partition: u16,
    /// Largest change the shape's frame conversion applied to one coordinate.
    pub conversion: f64,
}

/// One checked body-plus-garments mesh bound to a completed rig.
#[derive(Debug)]
pub struct MergedActor {
    /// Merged geometry, one palette, authored weights.
    pub asset: MeshAsset,
    /// Shape identity the rest proof and the writer agree on.
    pub shape: usize,
    /// Skeleton root joint of the merged skin.
    pub root: JointId,
    /// Source global-to-skin matrix of the body shape.
    pub global_to_skin: [[f32; 4]; 4],
    /// Common rest basis every merged shape is measured at.
    pub basis: [[f64; 4]; 4],
    /// Parts in merge order.
    pub parts: Vec<MergedPart>,
    /// One name per palette entry.
    pub palette: Vec<String>,
    /// Largest rest inconsistency of the binds that are written, measured as
    /// `|world · bind · G − I|`; at most [`REST_TOLERANCE`].
    pub bind_residual: f64,
    /// Largest rest inconsistency the source authored, before any repair.
    pub source_bind_mismatch: f64,
    /// Weighted mass of the repaired bones, as a fraction of all influences.
    pub repaired_mass: f64,
    /// Binds re-derived from the source rest because the authored one disagreed.
    pub bind_repairs: Vec<BindRepair>,
}

/// One declared bind re-derivation: the authored matrix disagreed with the
/// file's own node rest, so the rest world defines it instead.
#[derive(Debug, Clone, PartialEq)]
pub struct BindRepair {
    /// Palette bone.
    pub bone: String,
    /// Shape whose authored bind was replaced.
    pub shape: usize,
    /// The authored matrix's rest inconsistency before the repair.
    pub before: f64,
    /// Weighted mass this bone carries, as a fraction of all influences.
    pub weight_mass: f64,
}

impl MergedActor {
    /// The rest proof of the merged skin at the body basis.
    ///
    /// # Errors
    /// The checked binding refuses a frame, palette or rest mismatch.
    pub fn proof<'a>(&'a self, rig: &'a Rig) -> Result<RestBinding<'a>, RestBindError> {
        let skin = self
            .asset
            .skin
            .as_ref()
            .expect("a merged actor always carries its skin");
        RestBinding::prove_with_basis(
            rig,
            RigFrame::SourceZUp,
            &[SkinRestSource {
                shape: self.shape,
                root: self.root,
                positions: &self.asset.positions,
                skin,
                global_to_skin: self.global_to_skin,
            }],
            self.basis,
        )
    }
}

/// One clip converted onto the destination rig.
#[derive(Debug)]
pub struct ClipPlan {
    /// The written clip.
    pub clip: Clip,
    /// Written duration in seconds.
    pub duration: f64,
    /// Source to rig unit ratio applied to every translation sample.
    pub scale: f64,
    /// Channels that found their bone by name.
    pub mapped_bones: usize,
    /// Palette bones the clip drives, sorted by name; the input to the
    /// admission gate's role coverage.
    pub mapped_bone_names: Vec<String>,
    /// Channels dropped because their bone carries no weight in this skin.
    pub dropped_bones: Vec<String>,
    /// Channels whose bone is a root of the destination rig: the rig's own
    /// placement is authoritative, so only the rest-relative motion is applied.
    pub root_bones: usize,
    /// Channels whose samples are not constant.
    pub moving_channels: usize,
    /// Largest motion sampled on one channel: rotation degrees.
    pub max_motion_degrees: f64,
    /// Largest motion sampled on one channel, rig units.
    pub max_motion_translation: f64,
    /// Largest rest-rotation difference between source and rig, degrees.
    pub max_rest_rotation_degrees: f64,
    /// Largest anchor-pose bone-position residual, relative to the extent.
    pub max_rest_translation_residual: f64,
    /// The anchor stance is within the declared residual of the bind pose.
    pub rest_pose_comparable: bool,
    /// Bone carrying the largest anchor-pose residual.
    pub rest_pose_outlier: String,
    /// Largest bone-frame difference from the reference pose, degrees.
    pub max_frame_delta_degrees: f64,
    /// Largest bone-direction change from the reference pose, degrees.
    pub max_swing_degrees: f64,
    /// Bone with the largest frame difference.
    pub frame_outlier: String,
    /// Largest constant bone-roll difference normalized away, degrees.
    pub roll_normalization_degrees: f64,
    /// Smallest per-bone unit ratio that shares the rig's unit.
    pub scale_min: f64,
    /// Largest per-bone unit ratio that shares the rig's unit.
    pub scale_max: f64,
    /// Share of measurable bone offsets that agree with the shared unit ratio.
    pub scale_shared: f64,
    /// Offsets whose ratio does not share the rig's unit, reported, not hidden.
    pub scale_outliers: Vec<ScaleOutlier>,
    /// The semantics the clip was converted with.
    pub semantics: ClipSemantics,
    /// Median rest world-orientation difference after the fitted frame, degrees:
    /// the measured convention gap between the two skeletons' bone frames.
    pub axis_convention_degrees: f64,
    /// Median bone-direction error against the source's own direction, degrees.
    pub direction_error_median_degrees: f64,
    /// Worst bone-direction error, degrees.
    pub direction_error_max_degrees: f64,
    /// Median written-world orientation error against the source pose, degrees.
    pub pose_error_median_degrees: f64,
    /// Worst written-world orientation error against the source pose, degrees.
    pub pose_error_max_degrees: f64,
    /// Bone carrying the worst pose error.
    pub pose_error_outlier: String,
    /// The root-motion policy applied.
    pub root_motion: RootMotion,
    /// Palette bone the travel is written on, when one was resolved.
    pub root_motion_bone: Option<String>,
    /// Source bone whose travel is read, when one was resolved.
    pub source_root_bone: Option<String>,
    /// Distance the linear ramp carried off the pelvis, rig units.
    pub root_motion_travel: f64,
    /// Distance the pelvis travels over the clip, rig units.
    pub root_travel: f64,
}

/// Admitted clip rest differences.
#[derive(Debug, Clone, Copy)]
pub struct ClipTolerances {
    /// Rest bone-position residual, relative to the skeleton extent.
    pub translation_relative: f64,
    /// Spread of the per-bone unit ratios, relative to the median.
    pub scale_relative: f64,
    /// Admitted constant bone-roll difference to normalize, degrees.
    pub roll_degrees: f64,
}

impl Default for ClipTolerances {
    fn default() -> Self {
        Self {
            translation_relative: DEFAULT_REST_TRANSLATION_TOLERANCE,
            scale_relative: DEFAULT_REST_SCALE_TOLERANCE,
            roll_degrees: DEFAULT_ROLL_TOLERANCE_DEGREES,
        }
    }
}

/// Export one rigged, clipped actor GLB and render its report.
///
/// # Errors
/// Typed refusals for every declared defect class before any output byte.
pub fn run(params: &ExportGlbParams) -> Result<Value, CommandError> {
    if params.clip_names.is_empty() && params.clips.is_some() {
        return Err(errors::invalid(
            "clip_names",
            "`clips` names a source GLB but no `clip_names` were declared",
        ));
    }
    if !params.clip_names.is_empty() && params.clips.is_none() {
        return Err(errors::invalid(
            "clips",
            format!(
                "{} clip name(s) declared without a source GLB",
                params.clip_names.len()
            ),
        ));
    }
    let mut seen = BTreeMap::new();
    for (index, name) in params.clip_names.iter().enumerate() {
        match seen.insert(name.as_str(), index) {
            None => {}
            Some(first) => {
                return Err(errors::invalid(
                    "clip_names",
                    format!("clip `{name}` is declared twice (positions {first} and {index})"),
                ));
            }
        }
    }

    let nif_bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&nif_bytes).map_err(errors::nif)?;
    let completed = complete_rig(&document, params.skeleton.as_deref())?;
    let (rig, _) = follow_hips(&completed)?;

    let body = resolve_shape(&document, &params.body_shape, "body_shape")?;
    let mut garment_indices = Vec::with_capacity(params.garment_shapes.len());
    for name in &params.garment_shapes {
        let index = resolve_shape(&document, name, "garment_shapes")?;
        if index == body || garment_indices.contains(&index) {
            return Err(refused(
                "export_glb_duplicate_part",
                format!("shape {name:?} is selected more than once"),
            ));
        }
        garment_indices.push(index);
    }

    let merged = merge_shapes(&document, &rig, body, &garment_indices)?;
    let mut merged = merged;
    let scale = match params.units_per_metre {
        Some(units) if units.is_finite() && units > 0.0 => 1.0 / units,
        Some(units) => {
            return Err(errors::invalid(
                "units_per_metre",
                format!("{units} is not a positive finite unit count"),
            ));
        }
        None => 1.0,
    };
    if (scale - 1.0).abs() > f64::EPSILON {
        scale_merged(&mut merged, scale);
    }

    let tolerances = ClipTolerances {
        translation_relative: params
            .rest_translation_tolerance
            .unwrap_or(DEFAULT_REST_TRANSLATION_TOLERANCE),
        scale_relative: params
            .rest_scale_tolerance
            .unwrap_or(DEFAULT_REST_SCALE_TOLERANCE),
        roll_degrees: params
            .rest_roll_tolerance_degrees
            .unwrap_or(DEFAULT_ROLL_TOLERANCE_DEGREES),
    };
    let mut clips = match (&params.clips, params.clip_names.is_empty()) {
        (Some(path), false) => {
            let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
            let actor = Actor::from_glb(&bytes).map_err(actor_error)?;
            let mut plans = Vec::with_capacity(params.clip_names.len());
            // The first declared clip's first sample is the shared standing
            // reference every clip is measured from.
            let first = actor.clip(&params.clip_names[0]).map_err(actor_error)?;
            let anchor = ClipAnchor::of(&actor, &first)?;
            for (index, name) in params.clip_names.iter().enumerate() {
                let source = match index {
                    0 => first.clone(),
                    _ => actor.clip(name).map_err(actor_error)?,
                };
                plans.push(plan_clip(
                    &source,
                    &actor,
                    &anchor,
                    &rig,
                    &merged.palette,
                    &tolerances,
                )?);
            }
            plans
        }
        _ => Vec::new(),
    };

    // The written file is in the declared unit, so the rig the writer proves
    // and drives is the same scaled rig the geometry was converted to.
    let rig = match (scale - 1.0).abs() > f64::EPSILON {
        true => scale_rig(&rig, scale)?,
        false => rig,
    };
    if (scale - 1.0).abs() > f64::EPSILON {
        for plan in &mut clips {
            scale_clip(&mut plan.clip, scale);
        }
    }
    let proof = merged.proof(&rig).map_err(rest_error)?;
    let skin = proof
        .skins()
        .first()
        .expect("the merged proof carries its one skin");
    let measurement = skin.measurement();
    let written: Vec<Clip> = clips.iter().map(|plan| plan.clip.clone()).collect();
    let bytes =
        write_rigged_glb(&merged.asset, &proof, merged.shape, &written).map_err(glb_error)?;
    write_bytes(&params.out, &bytes)?;

    let sha256 = hex(&Sha256::digest(&bytes));
    let mut rows = Vec::with_capacity(merged.parts.len());
    for part in &merged.parts {
        rows.push(json!({
            "name": part.name,
            "shape": part.shape,
            "partition": part.partition,
            "vertices": part.vertices.len(),
            "triangles": part.triangles.len(),
            "conversion": part.conversion,
        }));
    }
    let clip_rows: Vec<Value> = clips
        .iter()
        .enumerate()
        .map(|(index, plan)| {
            json!({
                "index": index,
                "name": plan.clip.name,
                "duration": plan.duration,
                "samples": plan.clip.times.len(),
                "channels": plan.clip.channels.len(),
                "mapped_bones": plan.mapped_bones,
                "dropped_bones": plan.dropped_bones,
                "root_bones": plan.root_bones,
                "moving_channels": plan.moving_channels,
                "unit_scale": plan.scale,
                "unit_scale_min": plan.scale_min,
                "unit_scale_max": plan.scale_max,
                "unit_scale_shared": plan.scale_shared,
                "unit_scale_outliers": plan
                    .scale_outliers
                    .iter()
                    .map(|outlier| json!({
                        "bone": outlier.bone,
                        "ratio": outlier.ratio,
                        "source_length": outlier.source_length,
                        "rig_length": outlier.rig_length,
                    }))
                    .collect::<Vec<_>>(),
                "max_motion_degrees": plan.max_motion_degrees,
                "max_motion_translation": plan.max_motion_translation,
                "max_rest_rotation_degrees": plan.max_rest_rotation_degrees,
                "max_rest_translation_residual": plan.max_rest_translation_residual,
                "rest_pose_comparable": plan.rest_pose_comparable,
                "rest_pose_outlier": plan.rest_pose_outlier,
                "max_frame_delta_degrees": plan.max_frame_delta_degrees,
                "max_swing_degrees": plan.max_swing_degrees,
                "frame_outlier": plan.frame_outlier,
                "roll_normalization_degrees": plan.roll_normalization_degrees,
            })
        })
        .collect();
    let report = json!({
        "out": params.out.display().to_string(),
        "bytes": bytes.len(),
        "sha256": sha256,
        "source": {
            "nif": params.nif.display().to_string(),
            "nif_sha256": hex(&Sha256::digest(&nif_bytes)),
            "skeleton": params.skeleton.as_ref().map(|path| path.display().to_string()),
            "shapes": document.shapes.len(),
            "rig_joints": rig.joints().len(),
        },
        "body_shape": params.body_shape,
        "parts": rows,
        "palette": {
            "bones": merged.palette.len(),
            "authored": merged
                .asset
                .skin
                .as_ref()
                .map(|skeleton| skeleton.bones.len())
                .unwrap_or(0),
            "bind_residual": merged.bind_residual,
            "source_bind_mismatch": merged.source_bind_mismatch,
            "repaired_mass": merged.repaired_mass,
            "bind_repairs": merged
                .bind_repairs
                .iter()
                .map(|repair| json!({
                    "bone": repair.bone,
                    "shape": repair.shape,
                    "before": repair.before,
                    "weight_mass": repair.weight_mass,
                }))
                .collect::<Vec<_>>(),
            "influences": influence_summary(merged.asset.skin.as_ref()),
        },
        "rest": {
            "basis": merged.basis,
            "max_vertex": measurement.max_vertex,
            "max_influence": measurement.max_influence,
            "max_joint": measurement.max_joint,
        },
        "units": {
            "units_per_metre": params.units_per_metre,
            "scale": scale,
            "metres": params.units_per_metre.is_some(),
        },
        "clips": clip_rows,
        "attributes": {
            "normals": !merged.asset.normals.is_empty(),
            "uvs": !merged.asset.uvs.is_empty(),
            "tangents": !merged.asset.tangents.is_empty(),
        },
        "bounds": bounds(&merged.asset.positions),
        "statement": "checked skinned actor: the rest proof and the clip rest gate name what was measured; this is not a fit result",
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

/// Read the family template and complete the authored rig, or keep the authored
/// rig when no template is declared.
pub(crate) fn complete_rig(
    document: &NifDocument,
    template: Option<&Path>,
) -> Result<Rig, CommandError> {
    let authored = document.rig.as_ref().ok_or_else(|| {
        refused(
            "export_glb_incomplete_rig",
            "the source NIF authors no node graph; a skinned export needs one",
        )
    })?;
    let Some(path) = template else {
        return Ok(authored.clone());
    };
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let document = sculpto_nif::read(&bytes).map_err(errors::nif)?;
    let family = document.rig.as_ref().ok_or_else(|| {
        errors::invalid(
            "skeleton",
            format!("{} contains no node graph", path.display()),
        )
    })?;
    let ([authored_root], [template_root]) = (authored.roots(), family.roots()) else {
        return Err(errors::invalid(
            "skeleton",
            "the source and the template each need exactly one scene root",
        ));
    };
    complete_family_rig(
        authored,
        family,
        TemplateRoots {
            authored: *authored_root,
            template: *template_root,
        },
    )
    .map_err(|error| {
        refused(
            "export_glb_incomplete_rig",
            format!("{}: {error}", path.display()),
        )
    })
}

/// The palette bone that roots the torso chain, by SSE name.
pub const TORSO_ANCHOR: &str = "NPC Spine [Spn0]";

/// The palette bone that roots the hips, by SSE name.
pub const HIPS_ANCHOR: &str = "NPC Pelvis [Pelv]";

/// What [`follow_hips`] did to the completed rig, for the export report.
///
/// A palette without both anchors (`mixamorig:*`, a custom rig) keeps its
/// authored tree and says so here instead of silently pretending the rule ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorsoParent {
    /// The joint moved under the hips, when the palette carried both anchors.
    pub moved: Option<String>,
    /// Its new parent.
    pub parent: Option<String>,
    /// Its previous parent.
    pub previous: Option<String>,
    /// One sentence naming what happened, for the report.
    pub note: &'static str,
}

/// Parent the torso branch under the hips, preserving every rest world.
///
/// The palette authors the torso and the hips as two branches under the static
/// root offset, so a pelvis rotation moves the hips and leaves the shoulders
/// where they were (measured 0.00 mm of shoulder travel at 5° of pelvis yaw).
/// Every source clip is authored against the anatomical tree, where the spine
/// hangs off the hips; this step makes the destination agree with them once,
/// instead of every clip compensating. It is a graph edit only: rest worlds,
/// joint order and bind matrices are unchanged.
///
/// It walks the graph instead of naming both sides of the move: the torso branch
/// root is the child of the two branches' nearest common ancestor on the torso's
/// path, and the new parent is the hips joint in that branch's own naming family
/// (`CME Pelvis` for a `CME UBody` branch, `NPC Pelvis` for an `NPC Spine` one),
/// so a mirrored skeleton stays a mirror. @req DECL0008
///
/// # Errors
/// A named anchor that the rig declares twice, or a move the
/// [`complete_family_rig`] rest proof refuses.
pub fn follow_hips(rig: &Rig) -> Result<(Rig, TorsoParent), CommandError> {
    let opened = |note: &'static str| TorsoParent {
        moved: None,
        parent: None,
        previous: None,
        note,
    };
    let (torso, hips) = match (
        rig.joint_by_name(TORSO_ANCHOR),
        rig.joint_by_name(HIPS_ANCHOR),
    ) {
        (Ok(torso), Ok(hips)) => (torso, hips),
        (Err(sculpto_core::rig::RigError::UnknownJointName { .. }), _)
        | (_, Err(sculpto_core::rig::RigError::UnknownJointName { .. })) => {
            return Ok((rig.clone(), opened("palette carries no hips/torso anchors")));
        }
        (Err(error), _) | (_, Err(error)) => {
            return Err(refused("export_glb_incomplete_rig", error.to_string()));
        }
    };
    let name = |joint: JointId| rig.joint(joint).and_then(|node| node.name.clone());
    let family = |joint: JointId| {
        name(joint).and_then(|name| name.split_once(' ').map(|(family, _)| family.to_string()))
    };
    // The torso branch root is the last joint before the two branches meet.
    let mut branch = torso;
    let mut common = None;
    while let Some(parent) = rig.joint(branch).and_then(|node| node.parent) {
        if parent == hips || rig.is_ancestor(parent, hips).unwrap_or(false) {
            common = Some(parent);
            break;
        }
        branch = parent;
    }
    let Some(common) = common else {
        return Ok((rig.clone(), opened("torso and hips share no ancestor")));
    };
    if branch == hips {
        return Ok((rig.clone(), opened("hips already follow the torso")));
    }
    let target = match (family(branch), family(hips), name(hips)) {
        (Some(left), Some(right), Some(hips_name)) if left != right => match hips_name
            .split_once(' ')
            .map(|(_, rest)| format!("{left} {rest}"))
            .and_then(|mirror| rig.joint_by_name(&mirror).ok())
        {
            Some(mirror) => mirror,
            None => {
                return Ok((
                    rig.clone(),
                    opened("no hips counterpart in the torso's naming family"),
                ));
            }
        },
        _ => hips,
    };
    if rig.joint(branch).and_then(|node| node.parent) == Some(target) {
        return Ok((rig.clone(), opened("already follows the hips")));
    }
    let previous = name(common);
    let moved_rig =
        sculpto_core::rig::template::reparent(rig, branch, target).map_err(|error| {
            refused(
                "export_glb_incomplete_rig",
                format!(
                    "torso re-parent {} -> {}: {error}",
                    name(branch).unwrap_or_default(),
                    name(target).unwrap_or_default()
                ),
            )
        })?;
    Ok((
        moved_rig,
        TorsoParent {
            moved: name(branch),
            parent: name(target),
            previous,
            note: "torso now rooted under the hips",
        },
    ))
}

/// Resolve one declared shape name; both a missing and an ambiguous name refuse.
pub(crate) fn resolve_shape(
    document: &NifDocument,
    name: &str,
    field: &'static str,
) -> Result<usize, CommandError> {
    let matches: Vec<usize> = document
        .shapes
        .iter()
        .enumerate()
        .filter(|(_, shape)| shape.name == name)
        .map(|(index, _)| index)
        .collect();
    match matches.as_slice() {
        [index] => Ok(*index),
        [] => {
            let found: Vec<&str> = document
                .shapes
                .iter()
                .map(|shape| shape.name.as_str())
                .collect();
            Err(CommandError::new(
                ErrorKind::NotFound,
                "export_glb_shape_missing",
                format!("`{field}`: shape {name:?} not found, the file has {found:?}"),
            ))
        }
        _ => Err(refused(
            "export_glb_shape_ambiguous",
            format!(
                "`{field}`: shape {name:?} matches {} document shapes",
                matches.len()
            ),
        )),
    }
}

/// Merge the body and its garments into one checked skin at the body's frame.
///
/// Every selected shape keeps its authored weights; only its palette indexes
/// change. A shape whose source `G` differs from the body's is converted into
/// the body frame (`C = G_body * G_shape⁻¹`, binds `C⁻¹`), which is the identity
/// for a native BodySlide document.
///
/// # Errors
/// An unskinned or degenerate selected shape, a source record without `G` or a
/// skeleton root, a bone the rig does not name, a shape whose conversion the
/// rest proof cannot support, and any checked binding defect of the source.
pub fn merge_shapes(
    document: &NifDocument,
    rig: &Rig,
    body: usize,
    garments: &[usize],
) -> Result<MergedActor, CommandError> {
    let body_shape = document.shapes.get(body).ok_or_else(|| {
        errors::invalid(
            "body_shape",
            format!("shape index {body} is outside the file"),
        )
    })?;
    if body_shape.skin.is_none() {
        return Err(refused(
            "export_glb_unskinned_shape",
            format!("body shape {:?} carries no authored skin", body_shape.name),
        ));
    }
    let body_source = document
        .sources
        .get(body)
        .ok_or_else(|| missing_source(body, body_shape))?;
    let global_to_skin = body_source
        .global_skin
        .ok_or_else(|| missing_source(body, body_shape))?;
    let root_name = body_source.skeleton_root.as_deref().ok_or_else(|| {
        refused(
            "export_glb_incomplete_rig",
            format!(
                "body shape {:?} names no skin skeleton root; the rest proof has nothing to anchor",
                body_shape.name
            ),
        )
    })?;
    let root = rig.joint_by_name(root_name).map_err(|error| {
        refused(
            "export_glb_incomplete_rig",
            format!("body skeleton root {root_name:?}: {error}"),
        )
    })?;

    let weighted = weighted_bones(document, body, garments)?;
    let (palette, index) = palette_of(rig, &weighted)?;
    // The first selected shape that weighs a bone supplies its authored bind,
    // converted into the body frame; the body is first, so it wins.
    let mut authored: Vec<Option<(usize, DMat4)>> = vec![None; palette.len()];
    let conversion = DMat4::from_cols_array_2d(&global_to_skin.map(|column| column.map(f64::from)));

    // One vertex arena can carry an attribute only when every merged shape has
    // it; a partially attributed set drops the attribute instead of shifting
    // lanes, and the report names what was dropped.
    let selected: Vec<usize> = std::iter::once(body)
        .chain(garments.iter().copied())
        .collect();
    let shared = |field: fn(&NifShape) -> usize| {
        selected
            .iter()
            .all(|index| document.shapes[*index].positions.len() == field(&document.shapes[*index]))
    };
    let has_normals = shared(|shape| shape.normals.len());
    let has_uvs = shared(|shape| shape.uvs.len());
    let has_tangents = shared(|shape| shape.tangents.len());
    let mut asset = MeshAsset {
        name: body_shape.name.clone(),
        triangles: Vec::new(),
        triangle_partitions: Vec::new(),
        ..Default::default()
    };
    let mut parts = Vec::with_capacity(1 + garments.len());
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();

    for (position_in_output, shape_index) in std::iter::once(body)
        .chain(garments.iter().copied())
        .enumerate()
    {
        let shape = document.shapes.get(shape_index).ok_or_else(|| {
            errors::invalid(
                "garment_shapes",
                format!("shape index {shape_index} is outside the file"),
            )
        })?;
        let skin = shape.skin.as_ref().ok_or_else(|| {
            refused(
                "export_glb_unskinned_shape",
                format!(
                    "shape {:?} carries no authored skin; only skinned shapes merge into one actor",
                    shape.name
                ),
            )
        })?;
        if shape.positions.is_empty() || shape.triangles.is_empty() {
            return Err(refused(
                "export_glb_empty_shape",
                format!("shape {:?} has no geometry to write", shape.name),
            ));
        }
        let source = document
            .sources
            .get(shape_index)
            .ok_or_else(|| missing_source(shape_index, shape))?;
        let shape_skin_to_global = DMat4::from_cols_array_2d(
            &source
                .global_skin
                .ok_or_else(|| missing_source(shape_index, shape))?
                .map(|column| column.map(f64::from)),
        )
        .inverse();
        // `C` maps this shape's skin frame into the body's skin frame.
        let shape_conversion = conversion * shape_skin_to_global;
        let linear = DMat3::from_mat4(shape_conversion);
        let normal_matrix = linear.inverse().transpose();
        let mut max_delta = 0.0_f64;

        let base = asset.positions.len();
        for point in &shape.positions {
            let converted =
                shape_conversion.transform_point3(DVec3::from_array(point.map(f64::from)));
            max_delta = max_delta.max(converted.distance(DVec3::from_array(point.map(f64::from))));
            asset
                .positions
                .push(converted.to_array().map(|value| value as f32));
        }
        if has_normals {
            for normal in &shape.normals {
                let converted =
                    (normal_matrix * DVec3::from_array(normal.map(f64::from))).normalize_or_zero();
                asset
                    .normals
                    .push(converted.to_array().map(|value| value as f32));
            }
        }
        if has_uvs {
            asset.uvs.extend_from_slice(&shape.uvs);
        }
        if has_tangents {
            for tangent in &shape.tangents {
                let converted = (linear
                    * DVec3::new(
                        f64::from(tangent[0]),
                        f64::from(tangent[1]),
                        f64::from(tangent[2]),
                    ))
                .normalize_or_zero();
                asset.tangents.push([
                    converted.x as f32,
                    converted.y as f32,
                    converted.z as f32,
                    tangent[3],
                ]);
            }
        }
        let first_triangle = asset.triangles.len();
        for triangle in &shape.triangles {
            let mut shifted = [0_u32; 3];
            for (lane, vertex) in triangle.iter().enumerate() {
                shifted[lane] = vertex
                    .checked_add(u32::try_from(base).map_err(|_| {
                        refused("export_glb_too_large", "merged vertex count exceeds u32")
                    })?)
                    .ok_or_else(|| {
                        refused("export_glb_too_large", "merged triangle index exceeds u32")
                    })?;
            }
            asset.triangles.push(shifted);
        }
        let partition = u16::try_from(position_in_output).map_err(|_| {
            refused(
                "export_glb_too_many_parts",
                format!(
                    "{} parts exceed the u16 partition id",
                    position_in_output + 1
                ),
            )
        })?;
        asset.triangle_partitions.extend(std::iter::repeat_n(
            PartitionId(partition),
            shape.triangles.len(),
        ));

        let bones = row_bones(skin, &index, shape)?;
        if !skin.bind_inverse.is_empty() {
            let back = shape_conversion.inverse();
            for (bone, slot) in bones.iter().enumerate() {
                if authored[*slot].is_some() {
                    continue;
                }
                let bind = DMat4::from_cols_array_2d(
                    &skin.bind_inverse[bone].map(|column| column.map(f64::from)),
                );
                authored[*slot] = Some((shape_index, bind * back));
            }
        }
        for vertex in 0..skin.influences.rows() {
            let row = skin.influences.row(vertex).ok_or_else(|| {
                errors::invalid(
                    "skin",
                    format!("shape {:?} vertex {vertex} has no row", shape.name),
                )
            })?;
            let mut slots: BTreeMap<u16, f32> = BTreeMap::new();
            for (bone, weight) in row.joints.iter().zip(row.weights) {
                let slot = u16::try_from(bones[usize::from(*bone)]).map_err(|_| {
                    refused("export_glb_too_large", "palette entry exceeds the u16 lane")
                })?;
                *slots.entry(slot).or_insert(0.0) += weight;
            }
            for (slot, weight) in slots {
                joints.push(slot);
                values.push(weight);
            }
            offsets
                .push(u32::try_from(joints.len()).map_err(|_| {
                    refused("export_glb_too_large", "influence entries exceed u32")
                })?);
        }

        parts.push(MergedPart {
            shape: shape_index,
            name: shape.name.clone(),
            vertices: base..asset.positions.len(),
            triangles: first_triangle..asset.triangles.len(),
            partition,
            conversion: max_delta,
        });
    }

    let bound = bind_palette(
        rig,
        ArenaInfluences {
            palette: &palette,
            authored: &authored,
            positions: &asset.positions,
            joints,
            values,
            offsets,
            source_mass: None,
        },
        global_to_skin,
        root,
        body,
    )?;
    asset.skin = Some(bound.skin.clone());
    Ok(MergedActor {
        asset,
        shape: body,
        root,
        global_to_skin,
        basis: bound.basis,
        parts,
        palette,
        bind_residual: bound.bind_residual,
        source_bind_mismatch: bound.source_bind_mismatch,
        repaired_mass: bound.repaired_mass,
        bind_repairs: bound.repairs,
    })
}

/// One merged arena's influences, before any bind is chosen.
pub(crate) struct ArenaInfluences<'a> {
    /// One name per palette entry.
    pub(crate) palette: &'a [String],
    /// The authored bind per palette slot, with the shape that supplied it.
    pub(crate) authored: &'a [Option<(usize, DMat4)>],
    /// Merged vertices in the body skin frame.
    pub(crate) positions: &'a [[f32; 3]],
    /// Packed influence slots into the palette.
    pub(crate) joints: Vec<u16>,
    /// Packed influence weights, parallel to `joints`.
    pub(crate) values: Vec<f32>,
    /// Row starts into `joints`/`values`, one per vertex plus the end.
    pub(crate) offsets: Vec<u32>,
    /// Weighted mass per palette slot to charge against [`REPAIR_MASS_CAP`],
    /// when the client synthesized weights the source never carried.
    ///
    /// The cap asks whether the *source* skin disagrees with its own rest, so a
    /// caller whose arena mixes copied and synthesized influences measures it on
    /// the copied mass alone; `None` measures the whole arena.
    pub(crate) source_mass: Option<&'a [f64]>,
}

/// One checked bind set: the skin plus what the rest proof measured.
pub(crate) struct BoundPalette {
    pub(crate) skin: Skin,
    pub(crate) bind_residual: f64,
    pub(crate) source_bind_mismatch: f64,
    pub(crate) repaired_mass: f64,
    pub(crate) repairs: Vec<BindRepair>,
    pub(crate) basis: [[f64; 4]; 4],
}

/// Choose one bind per palette bone and prove the arena's rest at the body basis.
///
/// An authored bind is kept only when the influences it actually carries
/// reconstruct the rest; otherwise the bind is re-derived from the rest world
/// and the repair is reported. A repair that carries more than
/// [`REPAIR_MASS_CAP`] of the mass named by
/// [`ArenaInfluences::source_mass`] is a source defect, not a bind repair, and
/// refuses. @req DECL0008
///
/// # Errors
/// A palette name the rig does not resolve, weights the skin refuses, a whole
/// skin disagreeing with the source rest, and any rest proof defect.
pub(crate) fn bind_palette(
    rig: &Rig,
    arena: ArenaInfluences<'_>,
    global_to_skin: [[f32; 4]; 4],
    root: JointId,
    shape: usize,
) -> Result<BoundPalette, CommandError> {
    let ArenaInfluences {
        palette,
        authored,
        positions,
        joints,
        values,
        offsets,
        source_mass,
    } = arena;
    let global = DMat4::from_cols_array_2d(&global_to_skin.map(|column| column.map(f64::from)));
    let inverse_global = global.inverse();
    let mut mass: Vec<f64> = vec![0.0; palette.len()];
    let mut total = 0.0_f64;
    for (slot, weight) in joints.iter().zip(&values) {
        let weight = f64::from(*weight);
        mass[usize::from(*slot)] += weight;
        total += weight;
    }
    // The repair cap is charged against the mass the source itself carried.
    let (capped, capped_total) = match source_mass {
        Some(mass) if mass.len() == palette.len() => (mass, mass.iter().sum()),
        _ => (mass.as_slice(), total),
    };
    let worlds: Vec<DMat4> = palette
        .iter()
        .map(|name| {
            let joint = rig
                .joint_by_name(name)
                .expect("a palette name resolves uniquely in its own rig");
            DMat4::from_cols_array_2d(
                &rig.rest_world(joint)
                    .expect("a palette joint has a rest world"),
            )
        })
        .collect();
    // Which authored binds do not reconstruct this rest? The measure is the
    // displacement each bone's own influences would actually carry, so a matrix
    // difference on a bone with no weight costs nothing.
    let transforms: Vec<Option<DMat4>> = palette
        .iter()
        .enumerate()
        .map(|(slot, _)| authored[slot].map(|(_, bind)| worlds[slot] * bind))
        .collect();
    let mut worst_influence: Vec<f64> = vec![0.0; palette.len()];
    let mut source_bind_mismatch = 0.0_f64;
    for slot in 0..palette.len() {
        if let Some((_, bind)) = authored[slot] {
            source_bind_mismatch =
                source_bind_mismatch.max(rest_displacement(worlds[slot] * bind * global));
        }
    }
    for vertex in 0..positions.len() {
        let point = DVec3::from_array(positions[vertex].map(f64::from));
        let expected = inverse_global.transform_point3(point);
        let start = offsets[vertex] as usize;
        let end = offsets[vertex + 1] as usize;
        for lane in start..end {
            let slot = usize::from(joints[lane]);
            let weight = f64::from(values[lane]);
            let Some(transform) = transforms[slot] else {
                continue;
            };
            if weight <= 0.0 {
                continue;
            }
            let error = (transform.transform_point3(point) - expected).length() * weight;
            worst_influence[slot] = worst_influence[slot].max(error);
        }
    }
    let mut binds = Vec::with_capacity(palette.len());
    let mut repairs = Vec::new();
    let mut bind_residual = 0.0_f64;
    let mut repaired_mass = 0.0_f64;
    for (slot, name) in palette.iter().enumerate() {
        let derived = worlds[slot].inverse() * inverse_global;
        let chosen = match authored[slot] {
            Some((_, bind)) if worst_influence[slot] <= REST_TOLERANCE => {
                bind_residual = bind_residual.max(rest_displacement(worlds[slot] * bind * global));
                bind
            }
            Some((shape_index, _)) => {
                let weight_mass = match capped_total > 0.0 {
                    true => capped[slot] / capped_total,
                    false => 0.0,
                };
                repaired_mass += weight_mass;
                repairs.push(BindRepair {
                    bone: name.clone(),
                    shape: shape_index,
                    before: worst_influence[slot],
                    weight_mass,
                });
                derived
            }
            None => derived,
        };
        binds.push(
            chosen
                .to_cols_array_2d()
                .map(|column| column.map(|value| value as f32)),
        );
    }
    if repaired_mass > REPAIR_MASS_CAP {
        return Err(refused(
            "export_glb_source_rest",
            format!(
                "{} authored bind(s) disagree with the source rest and carry {repaired_mass:.6} of the weighted mass (cap {REPAIR_MASS_CAP}); a whole-skin disagreement is a source defect, not a bind repair: {:?}",
                repairs.len(),
                repairs
                    .iter()
                    .map(|repair| repair.bone.as_str())
                    .collect::<Vec<_>>(),
            ),
        ));
    }
    let influences =
        SkinWeights::new(offsets, joints, values, palette.len()).map_err(weight_error)?;
    let skin = Skin {
        bones: palette.to_vec(),
        bind_inverse: binds,
        influences,
    };
    // The automatic proof picks the basis (identity for a native document) and
    // measures every weighted vertex; the writer re-proves at the same basis.
    let basis = {
        let binding = RestBinding::prove(
            rig,
            RigFrame::SourceZUp,
            &[SkinRestSource {
                shape,
                root,
                positions,
                skin: &skin,
                global_to_skin,
            }],
        )
        .map_err(rest_error)?;
        binding.basis()
    };
    Ok(BoundPalette {
        skin,
        bind_residual,
        source_bind_mismatch,
        repaired_mass,
        repairs,
        basis,
    })
}

/// Every bone name the selected shapes actually weight.
pub(crate) fn weighted_bones(
    document: &NifDocument,
    body: usize,
    garments: &[usize],
) -> Result<BTreeSet<String>, CommandError> {
    let mut names = BTreeSet::new();
    for shape_index in std::iter::once(body).chain(garments.iter().copied()) {
        let shape = document.shapes.get(shape_index).ok_or_else(|| {
            errors::invalid(
                "garment_shapes",
                format!("shape index {shape_index} is outside the file"),
            )
        })?;
        let skin = shape.skin.as_ref().ok_or_else(|| {
            refused(
                "export_glb_unskinned_shape",
                format!("shape {:?} carries no authored skin", shape.name),
            )
        })?;
        for (bone, _) in skin.bones.iter().enumerate() {
            let carried = (0..skin.influences.rows()).any(|vertex| {
                skin.influences.row(vertex).is_some_and(|row| {
                    row.joints
                        .iter()
                        .zip(row.weights)
                        .any(|(index, weight)| usize::from(*index) == bone && *weight > 0.0)
                })
            });
            if carried {
                names.insert(skin.bones[bone].clone());
            }
        }
    }
    Ok(names)
}

/// The skin palette: every weighted bone and every ancestor of one, in rig
/// order.
///
/// A whole-rig palette is not writable by consumers (Bevy refuses a skin above
/// 256 joints), and a bone no vertex weights cannot deform anything; the
/// ancestors stay so each weighted bone keeps its rest chain. Clip channels of
/// bones outside the set carry no weight and are reported as dropped.
pub(crate) fn palette_of(
    rig: &Rig,
    weighted: &BTreeSet<String>,
) -> Result<(Vec<String>, BTreeMap<String, usize>), CommandError> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for joint in rig.joints() {
        if let Some(name) = &joint.name {
            *counts.entry(name.as_str()).or_insert(0) += 1;
        }
    }
    let mut needed: BTreeSet<usize> = BTreeSet::new();
    for name in weighted {
        let joint = rig.joint_by_name(name).map_err(|error| {
            refused(
                "export_glb_incomplete_rig",
                format!("weighted bone {name:?}: {error}"),
            )
        })?;
        let mut cursor = Some(joint);
        while let Some(current) = cursor {
            if !needed.insert(current.index()) {
                break;
            }
            cursor = rig.joint(current).and_then(|joint| joint.parent);
        }
    }
    let mut palette = Vec::new();
    let mut index = BTreeMap::new();
    for (slot, joint) in rig.joints().iter().enumerate() {
        if !needed.contains(&slot) {
            continue;
        }
        let Some(name) = &joint.name else {
            // An unnamed ancestor cannot enter a name-keyed palette; the rest
            // proof then refuses its weighted descendant, which is the honest
            // report.
            continue;
        };
        if counts.get(name.as_str()) != Some(&1) {
            continue;
        }
        index.insert(name.clone(), palette.len());
        palette.push(name.clone());
    }
    if palette.is_empty() {
        return Err(refused(
            "export_glb_incomplete_rig",
            "the completed rig names no joint, so no skin palette exists",
        ));
    }
    Ok((palette, index))
}

/// Per-bone palette slot for every vertex row of one shape.
fn row_bones(
    skin: &Skin,
    index: &BTreeMap<String, usize>,
    shape: &NifShape,
) -> Result<Vec<usize>, CommandError> {
    skin.bones
        .iter()
        .map(|bone| {
            index.get(bone.as_str()).copied().ok_or_else(|| {
                refused(
                    "export_glb_incomplete_rig",
                    format!(
                        "shape {:?} weighs bone {bone:?}, which the completed rig does not name uniquely; \
                         declare `skeleton` to complete the family rig",
                        shape.name
                    ),
                )
            })
        })
        .collect()
}

/// Displacement a transform applies to the origin and the three unit axes.
fn rest_displacement(transform: DMat4) -> f64 {
    [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z]
        .into_iter()
        .map(|point| transform.transform_point3(point).distance(point))
        .fold(0.0_f64, f64::max)
}

/// Reject the shapes the merge cannot keep faithful, with the measured number.
fn missing_source(shape: usize, shape_definition: &NifShape) -> CommandError {
    refused(
        "export_glb_incomplete_rig",
        format!(
            "shape {shape} ({:?}) has no source skin record with a global-to-skin matrix",
            shape_definition.name
        ),
    )
}

/// Which pose transfer a clip is converted with.
///
/// The two are not variations of one another: a motion copy writes the rig's rest
/// rotated by the clip's motion (exact per joint, and it carries the rest
/// difference of the two skeletons into every frame), while a pose copy writes the
/// clip's own world orientation with one fitted frame folded in (it reproduces the
/// source's pose, at the cost of a measured residual).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClipSemantics {
    /// The rig's rest pose moved by the clip's motion since its reference.
    #[default]
    Motion,
    /// The clip's own pose, one fitted frame folded in at the root.
    Pose,
}

impl ClipSemantics {
    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Motion => "motion",
            Self::Pose => "pose",
        }
    }
}

/// What the conversion does with the body's travel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RootMotion {
    /// Keep the whole trajectory on the pelvis; the actor root follows it.
    #[default]
    Keep,
    /// Zero the pelvis translation: an in-place cycle, which slides the stance
    /// foot at the clip's own speed and is reported as such.
    Zero,
    /// Split the linear ramp off as root motion and leave the residual sway.
    Split,
}

impl RootMotion {
    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Zero => "zero",
            Self::Split => "split",
        }
    }
}

/// Convert a mapped source clip onto the rig's rest and units.
///
/// Bone *rotations* are imported as world-space deltas applied to the
/// destination rest: `q(t) = Δworld(t) · rest_world`, so a per-bone rest
/// orientation that differs by a convention (the measured source carries 180°
/// flips on some bones) cannot distort the motion, and the destination rest is
/// reproduced exactly at the clip's rest frame. Every local rotation is then
/// derived from the destination parent world, so the chain stays consistent.
///
/// Bone *translations* stay local and rest-relative, scaled by the one measured
/// unit ratio; *scale* channels are rest-relative ratios.
///
/// # Errors
/// A channel whose bone the rig does not name uniquely, per-bone unit ratios
/// that are not one similarity, a rest skeleton whose bone positions the source
/// does not reproduce, a channel that cannot be evaluated, and a clip with no
/// moving sample.
pub fn plan_clip(
    source: &SourceClip,
    actor: &Actor,
    anchor: &ClipAnchor,
    rig: &Rig,
    palette: &[String],
    tolerances: &ClipTolerances,
) -> Result<ClipPlan, CommandError> {
    plan_clip_with(
        source,
        actor,
        anchor,
        rig,
        palette,
        tolerances,
        None,
        ClipSemantics::Motion,
        RootMotion::Keep,
    )
}

/// Convert one source clip with an explicit correspondence and semantics.
///
/// `correspondence` replaces name lookup with the resolved source-to-palette
/// map, so a clip authored on another skeleton reaches the palette; a channel
/// whose node drives nothing is counted, not refused. `semantics` chooses
/// between a motion copy (the destination rest rotated by the clip's motion) and
/// a pose copy (the clip's own world orientation, one fitted frame folded in),
/// and the plan reports the error each one produced.
///
/// # Errors
/// The same refusals [`plan_clip`] states, plus a unit or frame the mapped bones
/// cannot define.
#[allow(clippy::too_many_arguments)]
pub fn plan_clip_with(
    source: &SourceClip,
    actor: &Actor,
    anchor: &ClipAnchor,
    rig: &Rig,
    palette: &[String],
    tolerances: &ClipTolerances,
    correspondence: Option<&Correspondence>,
    semantics: ClipSemantics,
    root_motion: RootMotion,
) -> Result<ClipPlan, CommandError> {
    let slots: BTreeMap<&str, usize> = palette
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.as_str(), slot))
        .collect();
    // The resolved map: source node -> palette slot, name and folded segments.
    let resolved: BTreeMap<usize, (usize, String, Vec<usize>)> = match correspondence {
        Some(correspondence) => correspondence
            .mapping()
            .into_iter()
            .map(|(node, slot, name)| (node, (slot, name, correspondence.fold_of(slot).to_vec())))
            .collect(),
        None => BTreeMap::new(),
    };
    // Every rotation channel, by node: a folded segment is not a track of its
    // own, but its local rotation still has to reach the bone it folds into.
    let mut rotations: BTreeMap<usize, Vec<[f64; 4]>> = BTreeMap::new();
    if correspondence.is_some() {
        for channel in &source.channels {
            if let (SourceProperty::Rotation, SourceValues::Rotation(values)) =
                (channel.property, &channel.values)
            {
                rotations.insert(channel.node, values.clone());
            }
        }
    }
    let mut missing: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    for channel in &source.channels {
        let (slot, name, fold) = match correspondence {
            Some(_) => match resolved.get(&channel.node) {
                Some(entry) => entry.clone(),
                None => {
                    dropped.push(
                        channel
                            .node_name
                            .clone()
                            .unwrap_or_else(|| format!("node {}", channel.node)),
                    );
                    continue;
                }
            },
            None => {
                let name = channel.node_name.as_deref().ok_or_else(|| {
                    refused(
                        "export_glb_clip_bone_absent",
                        format!(
                            "clip {:?} channel targets unnamed node {}",
                            source.name, channel.node
                        ),
                    )
                })?;
                let Some(&slot) = slots.get(name) else {
                    match rig.joint_by_name(name) {
                        // A bone the rig names but no vertex weights: it cannot
                        // deform this skin, so its channel is dropped and counted.
                        Ok(_) => dropped.push(name.to_owned()),
                        Err(_) => missing.push(name.to_owned()),
                    }
                    continue;
                };
                (slot, name.to_owned(), Vec::new())
            }
        };
        let joint = match correspondence.is_some() {
            true => rig.joint_by_name(&name).map_err(|error| {
                refused(
                    "export_glb_clip_bone_absent",
                    format!("clip {:?} palette bone {name:?}: {error}", source.name),
                )
            })?,
            false => rig.joint_by_name(&name).map_err(|error| {
                refused(
                    "export_glb_clip_bone_absent",
                    format!("clip {:?} bone {name:?}: {error}", source.name),
                )
            })?,
        };
        let entry = match tracks.iter_mut().find(|track| track.node == channel.node) {
            Some(entry) => entry,
            None => {
                tracks.push(Track {
                    node: channel.node,
                    slot,
                    joint,
                    name: name.clone(),
                    translation: None,
                    rotation: None,
                    scale: None,
                    fold,
                    derived: false,
                });
                tracks.last_mut().expect("just pushed")
            }
        };
        match (channel.property, &channel.values) {
            (SourceProperty::Translation, SourceValues::Vector(values)) => {
                entry.translation = Some(values.clone());
            }
            (SourceProperty::Rotation, SourceValues::Rotation(values)) => {
                entry.rotation = Some(values.clone());
            }
            (SourceProperty::Scale, SourceValues::Vector(values)) => {
                entry.scale = Some(values.clone());
            }
            _ => {
                return Err(errors::invalid(
                    "clips",
                    format!(
                        "clip {:?} channel {name:?} carries samples it cannot write",
                        source.name
                    ),
                ));
            }
        }
    }
    dropped.sort_unstable();
    dropped.dedup();
    if !missing.is_empty() {
        missing.sort_unstable();
        missing.dedup();
        let shown: Vec<&str> = missing.iter().take(8).map(String::as_str).collect();
        return Err(refused(
            "export_glb_clip_bone_absent",
            format!(
                "clip {:?} drives {} bone(s) the completed rig does not name: {shown:?}{}",
                source.name,
                missing.len(),
                match missing.len() > shown.len() {
                    true => ", …",
                    false => "",
                }
            ),
        ));
    }
    if tracks.is_empty() {
        return Err(refused(
            "export_glb_clip_constant",
            format!("clip {:?} has no channel at all", source.name),
        ));
    }

    // Twist helpers are the palette's own machinery: no source skeleton of this
    // family carries them, and a fixed fraction of the parent-to-child roll is
    // what Unity, Rigify and Bulletproof all use (0.5 default, split by chain
    // position here). Holding them at rest shears the skin band at the elbow and
    // wrist; letting one own the whole rotation candy-wraps it.
    if let Some(correspondence) = correspondence {
        let mut twists: Vec<(usize, usize, String, JointId)> = correspondence
            .pairs
            .iter()
            .filter(|pair| pair.method == crate::retarget::Method::Derived)
            .map(|pair| {
                (
                    pair.slot,
                    pair.joint.index(),
                    pair.palette.clone(),
                    pair.joint,
                )
            })
            .collect();
        twists.sort_by_key(|row| row.0);
        for (slot, _index, name, joint) in twists {
            tracks.push(Track {
                node: usize::MAX,
                slot,
                joint,
                name,
                translation: None,
                rotation: Some(Vec::new()),
                scale: None,
                fold: Vec::new(),
                derived: true,
            });
        }
    }

    // The bone that carries the body's travel: the palette joint that is an
    // ancestor of the most mapped bones (ties broken by depth). An actor built
    // before `follow_hips` parents the pelvis and the spine as siblings under
    // `NPC COM`, so that is where the travel goes; an actor built after it has the
    // torso under the pelvis, and the same rule then answers `NPC Pelvis`. The
    // choice is computed, never named, and reported as `root_motion_bone`.
    let root_slot: Option<usize> = match correspondence {
        // The legacy name-mapped path keeps its own translation handling; the
        // root-motion policy is the cross-skeleton path's.
        None => None,
        Some(_) => {
            let mapped: Vec<usize> = tracks
                .iter()
                .filter(|track| !track.derived)
                .map(|track| track.slot)
                .collect();
            // Ancestor chains, as palette slots.
            let mut chains: Vec<BTreeSet<usize>> = Vec::new();
            for track in tracks.iter().filter(|track| !track.derived) {
                let mut chain: BTreeSet<usize> = BTreeSet::new();
                let mut cursor = rig
                    .joint(track.joint)
                    .and_then(|definition| definition.parent);
                while let Some(joint) = cursor {
                    if let Some(slot) = palette
                        .iter()
                        .position(|name| rig.joint_by_name(name).ok() == Some(joint))
                    {
                        chain.insert(slot);
                    }
                    cursor = rig.joint(joint).and_then(|definition| definition.parent);
                }
                chains.push(chain);
            }
            let mut best: Option<(usize, usize, usize)> = None; // (count, depth, slot)
            for slot in chains.iter().flat_map(|chain| chain.iter().copied()) {
                let count = chains.iter().filter(|chain| chain.contains(&slot)).count();
                let depth = rig
                    .joint_by_name(&palette[slot])
                    .ok()
                    .map(|joint| {
                        rig.topological_order()
                            .iter()
                            .position(|other| *other == joint)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);
                if best.is_none_or(|(best_count, best_depth, _)| {
                    count > best_count || (count == best_count && depth > best_depth)
                }) {
                    best = Some((count, depth, slot));
                }
            }
            best.filter(|_| !mapped.is_empty()).map(|(_, _, slot)| slot)
        }
    };
    let root_track = root_slot.map(|slot| {
        match tracks.iter().position(|track| track.slot == slot) {
            Some(position) => position,
            None => {
                let name = palette[slot].clone();
                let joint = rig
                    .joint_by_name(&name)
                    .expect("the root slot is a palette bone");
                tracks.push(Track {
                    node: usize::MAX,
                    slot,
                    joint,
                    name,
                    translation: Some(Vec::new()),
                    rotation: None,
                    scale: None,
                    fold: Vec::new(),
                    // No source channel drives it: it is the joint the travel is
                    // written on, so every source read skips it the same way a
                    // derived twist is skipped.
                    derived: true,
                });
                tracks.len() - 1
            }
        }
    });
    let source_root = match correspondence {
        Some(correspondence) => correspondence
            .pairs
            .iter()
            .find(|pair| {
                matches!(
                    pair.role.map(|role| role.part),
                    Some(crate::retarget::Part::Pelvis)
                )
            })
            .and_then(|pair| {
                tracks
                    .iter()
                    .position(|track| track.slot == pair.slot && !track.derived)
            }),
        None => None,
    }
    .or_else(|| root_track.filter(|position| !tracks[*position].derived));
    // The reference pose this clip's motion is measured from: the clip set's
    // first sample, never the exported node rest.
    let source_local = |node: usize| -> Result<sculpto_mesh::actor::LocalRest, CommandError> {
        match anchor.local(node) {
            Some(local) => Ok(local),
            None => actor.rest_local(node).map_err(actor_error),
        }
    };
    let source_world = |node: usize| -> Result<DMat4, CommandError> {
        match anchor.world(node) {
            Some(world) => Ok(world),
            None => rest_world_of(actor, node),
        }
    };

    // The unit is measured on the *world* positions the mapped bones occupy, not
    // on per-bone local offsets. A source authored for another body disagrees with
    // the rig in every bone length by construction (the Y-bot's clavicle is 11 cm
    // against the rig's 28.6 cm), so the median of per-bone ratios measures the
    // body, not the file's unit: on the measured pair it was biased by -4.52 % and
    // only 9 of 51 offsets agreed within 2 %. The ratio of the two skeletons'
    // radii about their own centroids is one number for a whole skeleton in one
    // unit and proportion does not move it.
    let origins: Vec<(usize, DVec3, DVec3)> = tracks
        .iter()
        .filter(|track| !track.derived)
        .map(|track| {
            let dest = rest_world(rig, track.joint);
            let source = source_world(track.node)?;
            Ok((track.slot, dest.w_axis.truncate(), source.w_axis.truncate()))
        })
        .collect::<Result<_, CommandError>>()?;
    let mean = |pick: fn(&(usize, DVec3, DVec3)) -> DVec3| -> DVec3 {
        match origins.is_empty() {
            true => DVec3::ZERO,
            false => origins.iter().map(pick).sum::<DVec3>() / origins.len() as f64,
        }
    };
    let target_center = mean(|row| row.1);
    let source_center = mean(|row| row.2);
    let (frame, radius_ratio) = match correspondence {
        Some(correspondence) => (
            correspondence.similarity.rotation,
            correspondence.similarity.unit,
        ),
        None => (rest_frame(&origins)?, 1.0),
    };
    // The lengths are measured between *world* joint origins, not between local
    // offsets: a source under a 0.01 ancestor carries centimetre locals at metre
    // world scale, and a ratio of local offsets would compare it against the wrong
    // frame (the defect the residual above had). A world length divided by a world
    // length is the same unit as `unit`.
    let source_parent_world = |node: usize| -> Result<DMat4, CommandError> {
        match actor.node_parents()[node] {
            Some(parent) => rest_world_of(actor, parent),
            None => Ok(DMat4::IDENTITY),
        }
    };
    let mut bone_lengths: Vec<(f64, f64, String)> = Vec::with_capacity(tracks.len());
    for track in tracks.iter().filter(|track| !track.derived) {
        let source = source_world(track.node)?.w_axis.truncate();
        let source_parent = source_parent_world(track.node)?.w_axis.truncate();
        let rig_origin = rest_world(rig, track.joint).w_axis.truncate();
        let rig_parent = match rig
            .joint(track.joint)
            .expect("a mapped bone is a rig joint")
            .parent
        {
            Some(parent) => rest_world(rig, parent).w_axis.truncate(),
            None => DVec3::ZERO,
        };
        bone_lengths.push((
            (rig_origin - rig_parent).length(),
            (source - source_parent).length(),
            track.name.clone(),
        ));
    }
    let longest_source = bone_lengths.iter().map(|row| row.1).fold(0.0_f64, f64::max);
    let longest_rig = bone_lengths.iter().map(|row| row.0).fold(0.0_f64, f64::max);
    let mut candidates: Vec<(f64, String, f64, f64)> = Vec::new();
    for (rig_length, source_length, name) in &bone_lengths {
        if *source_length <= longest_source * 1e-2 || *rig_length <= longest_rig * 1e-2 {
            continue;
        }
        candidates.push((
            rig_length / source_length,
            name.clone(),
            *source_length,
            *rig_length,
        ));
    }
    let unit = match correspondence {
        Some(_) => radius_ratio,
        None => {
            let source_radius: f64 = origins
                .iter()
                .map(|row| (row.2 - source_center).length_squared())
                .sum();
            let target_radius: f64 = origins
                .iter()
                .map(|row| (row.1 - target_center).length_squared())
                .sum();
            match source_radius > 1e-18 && target_radius > 1e-18 {
                true => (target_radius / source_radius).sqrt(),
                false => {
                    // Too few bones to place a radius: fall back to the per-bone
                    // lengths, which is what a two-joint fixture can still define.
                    let mut ratios: Vec<f64> = candidates.iter().map(|row| row.0).collect();
                    ratios.sort_by(f64::total_cmp);
                    match ratios.get(ratios.len() / 2).copied() {
                        Some(ratio) => ratio,
                        None => {
                            return Err(refused(
                                "export_glb_clip_rest_mismatch",
                                format!(
                                    "clip {:?} has no measurable rest extent against the rig; \
                                     the unit cannot be defined",
                                    source.name
                                ),
                            ));
                        }
                    }
                }
            }
        }
    };
    if !unit.is_finite() || unit <= 0.0 {
        return Err(refused(
            "export_glb_clip_rest_mismatch",
            format!(
                "clip {:?} rest positions give no usable unit ({unit})",
                source.name
            ),
        ));
    }
    // Rest compatibility: the two skeletons must be the same pose at one
    // similarity. Positions are convention free, so they carry the gate; per-bone
    // orientations are conventions and are reported, not gated. The expected
    // position is the *world* source position through the fitted rotation and the
    // world unit, centred on the rig's own centroid: multiplying a world position
    // by a per-local-unit ratio (the previous form) mixes two frames and reported
    // 0.988 on a correct conversion.
    let extent = origins
        .iter()
        .map(|row| (row.1 - target_center).length())
        .fold(0.0_f64, f64::max);
    let mut max_position_residual = 0.0_f64;
    let mut max_orientation = 0.0_f64;
    let mut worst_bone = String::new();
    for (track, (_, dest, source)) in tracks.iter().filter(|track| !track.derived).zip(&origins) {
        let expected = frame * ((*source - source_center) * unit) + target_center;
        let residual = match extent > 0.0 {
            true => dest.distance(expected) / extent,
            false => dest.distance(expected),
        };
        if residual > max_position_residual {
            max_position_residual = residual;
            worst_bone.clone_from(&track.name);
        }
        let dest_world = rest_world(rig, track.joint);
        let source_world = source_world(track.node)?;
        let orientation = quaternion_angle(
            &(rotation_of(dest_world)
                * (DQuat::from_mat3(&frame) * rotation_of(source_world)).inverse()),
        )
        .to_degrees();
        max_orientation = max_orientation.max(orientation);
    }
    // How far the two skeletons' *bone frames* sit apart after the one fitted
    // frame. A pose copy writes the source's absolute orientation, so it places
    // our child offsets along the source's own bone axis; that is only meaningful
    // when the two frames agree. The measured pair does not (SSE puts the bone
    // along local +Z, Mixamo along +Y), so the number is a precondition, not a
    // quality score.
    let axis_convention_degrees = {
        let mut angles: Vec<f64> = tracks
            .iter()
            .filter(|track| !track.derived)
            .map(|track| {
                let target = rotation_of(rest_world(rig, track.joint));
                let source = rotation_of(source_world(track.node)?);
                Ok(
                    quaternion_angle(&(target * (DQuat::from_mat3(&frame) * source).inverse()))
                        .to_degrees(),
                )
            })
            .collect::<Result<_, CommandError>>()?;
        angles.sort_by(f64::total_cmp);
        match angles.is_empty() {
            true => 0.0,
            false => angles[angles.len() / 2],
        }
    };
    // The anchor's stance is reported, not gated by itself: the conversion never
    // reads a bone position, so a source stance that differs from the bind pose
    // cannot displace the written pose. What it does mean is that only the motion
    // is imported, not the source's absolute stance; a cross-body source is
    // expected to be declared, not silently accepted.
    let pose_comparable = max_position_residual <= tolerances.translation_relative;

    // Per-bone ratios stay **reported**: they are the proportion difference of two
    // bodies, and a gate that refused them would refuse every cross-body source
    // (measured share 0.176 against the former 0.8 floor). What they still refuse
    // is the one case they were written for: two rigs that share a rest pose but
    // not a unit, which is a broken skeleton, not another body.
    let matching: Vec<f64> = candidates
        .iter()
        .map(|row| row.0)
        .filter(|ratio| (ratio / unit - 1.0).abs() <= tolerances.scale_relative)
        .collect();
    let scale_min = matching.iter().copied().fold(f64::INFINITY, f64::min);
    let scale_max = matching.iter().copied().fold(0.0_f64, f64::max);
    let share = match candidates.is_empty() {
        true => 1.0,
        false => matching.len() as f64 / candidates.len() as f64,
    };
    if share < 0.8 && pose_comparable {
        return Err(refused(
            "export_glb_clip_rest_mismatch",
            format!(
                "clip {:?} reproduces the rig's rest pose (residual {max_position_residual:.4}) but \
                 its bone offsets disagree with the one unit {unit:.6}: {} of {} measurable offsets \
                 are within {:.4} (floor 0.8); the skeleton's own lengths are inconsistent",
                source.name,
                matching.len(),
                candidates.len(),
                tolerances.scale_relative,
            ),
        ));
    }
    let mut scale_outliers = Vec::new();
    for (ratio, bone, source_length, rig_length) in &candidates {
        if (ratio / unit - 1.0).abs() > tolerances.scale_relative {
            scale_outliers.push(ScaleOutlier {
                bone: bone.clone(),
                ratio: *ratio,
                source_length: *source_length,
                rig_length: *rig_length,
            });
        }
    }
    let scale = unit;

    // Bone directions carry the motion convention free: the source's two clips
    // disagree about bone roll by tens of degrees on the same bone, so a frame
    // delta would turn a twist into a limb swing. Each bone is aligned onto the
    // source's own direction and keeps the rig rest's roll. A bone with several
    // children (a pelvis with thighs, a hand with fingers) takes the child whose
    // own rest direction is closest to the source's, so physics and attachment
    // children are never mistaken for the anatomical successor.
    let rest_source_worlds: Vec<DMat4> = tracks
        .iter()
        .map(|track| match track.derived {
            true => Ok(DMat4::IDENTITY),
            false => source_world(track.node),
        })
        .collect::<Result<_, CommandError>>()?;
    let directions: Vec<Option<(DVec3, usize)>> = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| match track.derived {
            true => None,
            false => direction_reference(rig, &tracks, index, &rest_source_worlds, frame, actor),
        })
        .collect();

    // Per-sample conversion.
    let samples = source.times.len();
    let source_worlds = SourceWorlds::new(actor, &tracks, samples)?;
    let rest_rotations: Vec<DQuat> = tracks
        .iter()
        .map(|track| DQuat::from_mat4(&rest_world(rig, track.joint)))
        .collect();
    let rest_source_rotations: Vec<DQuat> = tracks
        .iter()
        .map(|track| match track.derived {
            true => Ok(DQuat::IDENTITY),
            false => Ok(rotation_of(source_world(track.node)?)),
        })
        .collect::<Result<_, CommandError>>()?;
    // The source skeleton's own rest pose. A pose transfer is anchored here, not at
    // the clip's first sample: a walk's first sample is a *stance* (a bent knee,
    // a planted foot), and a delta measured from it carries only the change since
    // then, leaving our rest stance to travel with the body — the measured slide.
    let rest_source_rests: Vec<DQuat> = tracks
        .iter()
        .map(|track| match track.derived {
            true => Ok(DQuat::IDENTITY),
            false => Ok(rotation_of(rest_world_of(actor, track.node)?)),
        })
        .collect::<Result<_, CommandError>>()?;
    // Bone-frame deltas carry each rig's roll convention as well as the motion.
    // The convention is one constant twist per bone; it is measured here and
    // removed per sample, so the imported directions are the source's own.
    let mut twist_baseline: Vec<DQuat> = vec![DQuat::IDENTITY; tracks.len()];
    let mut max_roll = 0.0_f64;
    {
        let mut probe: Vec<Option<DMat4>> = vec![None; source_worlds.nodes()];
        for (position, track) in tracks.iter().enumerate() {
            if track.derived {
                continue;
            }
            let axis = directions[position].map(|(_, child)| {
                let bone = source_worlds
                    .world(track.node, 0, &mut probe)
                    .w_axis
                    .truncate();
                let joint = source_worlds.world(child, 0, &mut probe).w_axis.truncate();
                rest_source_rotations[position].inverse() * (joint - bone).normalize_or_zero()
            });
            let Some(axis) = axis else { continue };
            if axis.length_squared() < 0.5 {
                continue;
            }
            let sample = source_worlds.world(track.node, 0, &mut probe);
            let delta = rest_source_rotations[position].inverse() * rotation_of(sample);
            let swing = DQuat::from_rotation_arc(axis, delta * axis);
            let twist = swing.inverse() * delta;
            max_roll = max_roll.max(quaternion_angle(&twist).to_degrees());
            twist_baseline[position] = twist;
        }
    }
    if correspondence.is_none() && max_roll > tolerances.roll_degrees {
        return Err(refused(
            "export_glb_clip_rest_mismatch",
            format!(
                "clip {:?} needs up to {max_roll:.3}° of bone-roll normalization against the \
                 reference stance (admitted {:.3}°); its frames were baked on a different \
                 convention, so removing the difference would be retargeting. Re-bake the clip on \
                 this skeleton (Blender route) or declare a larger --rest-roll-tolerance-deg",
                source.name, tolerances.roll_degrees
            ),
        ));
    }

    let mut rotation_values: Vec<Vec<[f64; 4]>> = tracks
        .iter()
        .map(|track| match track.rotation {
            Some(_) => Vec::with_capacity(samples),
            None => Vec::new(),
        })
        .collect();
    let mut translation_values: Vec<Vec<[f64; 3]>> = tracks
        .iter()
        .map(|track| match track.translation {
            Some(_) => Vec::with_capacity(samples),
            None => Vec::new(),
        })
        .collect();
    let mut scale_values: Vec<Vec<[f64; 3]>> = tracks
        .iter()
        .map(|track| match track.scale {
            Some(_) => Vec::with_capacity(samples),
            None => Vec::new(),
        })
        .collect();
    // Report how far the clip's own frames sit from the reference and how much
    // of that is a roll convention (removed above), so the numbers name what the
    // import did instead of hiding it.
    let mut max_frame_delta = 0.0_f64;
    let mut max_swing = 0.0_f64;
    let mut frame_outlier = String::new();
    {
        let mut probe_worlds: Vec<Option<DMat4>> = vec![None; source_worlds.nodes()];
        for (position, track) in tracks.iter().enumerate() {
            if track.derived {
                continue;
            }
            let bone = source_worlds.world(track.node, 0, &mut probe_worlds);
            let frame_delta =
                quaternion_angle(&(rotation_of(bone) * rest_source_rotations[position].inverse()))
                    .to_degrees();
            let swing = match directions[position] {
                Some((_, child)) => match tracks.iter().position(|other| other.node == child) {
                    Some(child_position) => {
                        let anchor_bone = rest_source_worlds[position].w_axis.truncate();
                        let anchor_child = rest_source_worlds[child_position].w_axis.truncate();
                        let sample_child = source_worlds
                            .world(child, 0, &mut probe_worlds)
                            .w_axis
                            .truncate();
                        let anchor_direction = (anchor_child - anchor_bone).normalize_or_zero();
                        let sample_direction =
                            (sample_child - bone.w_axis.truncate()).normalize_or_zero();
                        match anchor_direction.length_squared() > 0.5
                            && sample_direction.length_squared() > 0.5
                        {
                            true => anchor_direction
                                .dot(sample_direction)
                                .clamp(-1.0, 1.0)
                                .acos()
                                .to_degrees(),
                            false => 0.0,
                        }
                    }
                    None => 0.0,
                },
                None => frame_delta,
            };
            if frame_delta - swing > max_frame_delta - max_swing {
                frame_outlier.clone_from(&track.name);
            }
            max_frame_delta = max_frame_delta.max(frame_delta);
            max_swing = max_swing.max(swing);
        }
    }

    // A folded segment's own *motion* composes after the bone it folds into:
    // `S_3 · (S_4 · S0_4⁻¹)`. The measured fourth finger segment carries 0.0000°
    // in the walk family, so the fold is exact there and only shows in a grip.
    let fold_reference: Vec<Vec<(usize, DQuat)>> = tracks
        .iter()
        .map(|track| {
            track
                .fold
                .iter()
                .map(|node| {
                    let rest = source_local(*node)?;
                    Ok((*node, DQuat::from_array(rest.rotation)))
                })
                .collect::<Result<Vec<_>, CommandError>>()
        })
        .collect::<Result<_, CommandError>>()?;
    let frame_rotation = DQuat::from_mat3(&frame);
    // The bone that carries the body's travel, and the source bone whose travel it
    // is. A SSE palette hangs the pelvis *beside* the spine off a common ancestor
    // (`NPC COM`), so translating the pelvis moves the legs and leaves the torso
    // behind: the travel belongs on the deepest joint every mapped bone descends
    // from. The source's own travel is read from its root motion bone.
    // The palette's twist helpers are *side* bones (children of the bone they
    // serve), not members of the chain: `ForearmTwist1/2` hang off `Forearm`
    // beside `Hand`, and `UpperarmTwist1/2` hang off `UpperArm` beside `Forearm`.
    // Their chain is therefore the parent and the parent's anatomical successor,
    // with the fraction from the helper's own index.
    let slot_roles: Vec<Option<crate::retarget::BoneRole>> = match correspondence {
        Some(correspondence) => palette
            .iter()
            .enumerate()
            .map(|(slot, _)| {
                correspondence
                    .pairs
                    .iter()
                    .find(|pair| pair.slot == slot)
                    .and_then(|pair| pair.role)
            })
            .collect(),
        None => vec![None; palette.len()],
    };
    let track_roles: Vec<Option<crate::retarget::BoneRole>> = tracks
        .iter()
        .map(|track| slot_roles.get(track.slot).copied().flatten())
        .collect();
    let chains: Vec<Option<TwistChain>> = tracks
        .iter()
        .enumerate()
        .map(|(position, track)| match track.derived {
            true => twist_chain(rig, &tracks, &track_roles, &slot_roles, position),
            false => None,
        })
        .collect();
    // D4: a mapped bone whose direct parent is not driven must derive its local
    // against the nearest *driven* ancestor's posed world, with the constant
    // offset of the static nodes between them folded out. Mike is the shape this
    // exists for: 52 of 52 mapped bones hang under an unmapped `CME` mirror, so a
    // rest-parent substitution drops the parent's posed world entirely.
    let closure: Vec<Option<(usize, DQuat)>> = tracks
        .iter()
        .map(|track| {
            let parent = rig.joint(track.joint)?.parent?;
            // Any track the plan writes is a valid derivation parent, derived or
            // not: its world is known before the locals pass. Skipping a derived
            // parent and substituting its own ancestor silently treated a rotating
            // twist helper as if it stood still, and the skin between the two
            // stretched by the difference.
            if let Some(found) = tracks.iter().position(|other| other.joint == parent) {
                return Some((found, DQuat::IDENTITY));
            }
            let mut cursor = Some(parent);
            while let Some(joint) = cursor {
                if let Some(found) = tracks
                    .iter()
                    .position(|other| other.joint == joint && !other.derived)
                {
                    let offset = rotation_of(rest_world(rig, joint)).inverse()
                        * rotation_of(rest_world(rig, parent));
                    return Some((found, offset));
                }
                cursor = rig.joint(joint).and_then(|definition| definition.parent);
            }
            None
        })
        .collect();
    let mut errors: Vec<f64> = Vec::with_capacity(samples);
    let mut error_outlier = String::new();
    let mut max_error = 0.0_f64;
    // Our bone's own axis in its local frame, and the source's own: the pair the
    // direction gate compares in world space, so a differing local axis shows up
    // as the error it produces rather than as a number about orientations.
    let rest_axes: Vec<Option<(DVec3, DVec3, usize)>> = tracks
        .iter()
        .enumerate()
        .map(|(position, _track)| {
            let (_, child) = directions[position]?;
            let child_position = tracks.iter().position(|other| other.node == child)?;
            let ours = DVec3::from_array(
                rig.joint(tracks[child_position].joint)?
                    .rest
                    .translation
                    .map(f64::from),
            )
            .normalize_or_zero();
            let theirs =
                DVec3::from_array(source_local(child).ok()?.translation).normalize_or_zero();
            match ours.length_squared() > 0.5 && theirs.length_squared() > 0.5 {
                true => Some((ours, theirs, child_position)),
                false => None,
            }
        })
        .collect();
    let mut direction_errors: Vec<f64> = Vec::with_capacity(samples);
    let mut written_position: Vec<DVec3> = vec![DVec3::ZERO; tracks.len()];
    let mut root_positions: Vec<DVec3> = Vec::with_capacity(samples);

    let mut worlds: Vec<Option<DMat4>> = vec![None; source_worlds.nodes()];
    for sample in 0..samples {
        worlds.iter_mut().for_each(|slot| *slot = None);
        // The world orientation every track is written at: the rig rest rotated
        // by the clip's motion (motion copy), or the clip's own world pose with
        // the one fitted frame folded in (pose copy).
        // A bone with no rotation channel keeps its rest orientation, and its
        // children must see that: the root-motion joint is exactly such a bone.
        let mut written: Vec<DQuat> = rest_rotations.clone();
        for (position, track) in tracks.iter().enumerate() {
            if track.derived {
                continue;
            }
            let bone = source_worlds.world(track.node, sample, &mut worlds);
            let source_rotation = rotation_of(bone);
            let mut world = match semantics {
                // The world rotation the source bone performed since its own
                // rest, applied to this rig's rest orientation. The delta is
                // taken in world space, so the source's bone-local axes never
                // enter: a rig whose bone runs along a different local axis
                // (SSE +Z against Mixamo +Y, measured 121.91° apart) still gets
                // the same anatomical motion, and this rig's rest, lengths and
                // skin frames stay its own.
                ClipSemantics::Motion => {
                    // The source's motion is expressed in the source's world; the one
                    // fitted frame takes it into this rig's world. Without that
                    // conjugation a source facing +Z and a body facing -Z swing the
                    // limbs the opposite way and the feet skate at twice the body
                    // speed (measured 3.49 m/s against the source's 0.084).
                    let delta = source_rotation * rest_source_rotations[position].inverse();
                    (frame_rotation * delta * frame_rotation.inverse()) * rest_rotations[position]
                }
                // The source's deviation from *its own* rest, applied to this rig's
                // rest: the stance is carried (a planted foot stays planted while
                // the body travels) and this rig's bone frames stay its own. An
                // absolute orientation copy is not offered: it places our child
                // offsets along the source's bone axis, measured 121.91° apart.
                ClipSemantics::Pose => {
                    let delta = source_rotation * rest_source_rests[position].inverse();
                    let fallback = || {
                        (frame_rotation * delta * frame_rotation.inverse())
                            * rest_rotations[position]
                    };
                    // Digits keep the delta copy: Mixamo's four phalanges fold into our
                    // three, their roll is not comparable, and aiming each one drags the
                    // fingertip (measured 5.1x on a 5 mm finger edge). The long bones
                    // are where the aim pays: it reproduces the elbow (flexion 26.5°
                    // against the source's 26.5°, bend plane half the error) where the
                    // world delta bends it about the source's roll (36.0°, plane 45.6°
                    // off, the operator's "arms bent sideways").
                    let aimable = track_roles[position].is_some_and(|role| {
                        matches!(
                            role.part,
                            crate::retarget::Part::Clavicle
                                | crate::retarget::Part::UpperArm
                                | crate::retarget::Part::Forearm
                                | crate::retarget::Part::Hand
                                | crate::retarget::Part::Thigh
                                | crate::retarget::Part::Calf
                                | crate::retarget::Part::Foot
                                | crate::retarget::Part::Spine(_)
                                | crate::retarget::Part::Neck
                                | crate::retarget::Part::Head
                        )
                    });
                    match directions[position].filter(|_| aimable) {
                        Some((_, child)) => {
                            let child_position = tracks
                                .iter()
                                .position(|other| other.node == child)
                                .expect("a direction child is a tracked bone");
                            let ours = DVec3::from_array(
                                rig.joint(tracks[child_position].joint)
                                    .expect("a mapped bone is a rig joint")
                                    .rest
                                    .translation
                                    .map(f64::from),
                            )
                            .normalize_or_zero();
                            let target = (frame_rotation
                                * (source_worlds
                                    .world(child, sample, &mut worlds)
                                    .w_axis
                                    .truncate()
                                    - bone.w_axis.truncate()))
                            .normalize_or_zero();
                            let rest_axis = rest_rotations[position] * ours;
                            match ours.length_squared() > 0.5 && target.length_squared() > 0.5 {
                                true => {
                                    DQuat::from_rotation_arc(rest_axis.normalize(), target)
                                        * rest_rotations[position]
                                }
                                false => fallback(),
                            }
                        }
                        None => fallback(),
                    }
                }
            };
            for (node, reference) in &fold_reference[position] {
                let Some(values) = rotations.get(node) else {
                    continue;
                };
                let folded = DQuat::from_array(values[sample.min(values.len() - 1)]);
                world *= folded * reference.inverse();
            }
            written[position] = world;
            // The gate 0131 recommends: the written world orientation against the
            // source's own, after the one fitted frame. A motion copy carries the
            // rest difference into every sample, so on a cross-body pair this is
            // tens of degrees by construction; a pose copy is the number under 5°.
            let error = quaternion_angle(&(world * (frame_rotation * source_rotation).inverse()))
                .to_degrees();
            if error > max_error {
                max_error = error;
                error_outlier.clone_from(&track.name);
            }
            if let Some((ours, theirs, _)) = rest_axes[position] {
                // The bone's own axis through each rig's world frame: the angle
                // between the two is what the eye sees as "the limb points
                // somewhere else", unlike an orientation difference.
                let mine = world * ours;
                let source_direction = (frame_rotation * source_rotation) * theirs;
                direction_errors.push(
                    mine.dot(source_direction)
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees(),
                );
            }
            errors.push(error);
            written_position[position] = bone.w_axis.truncate();
        }
        // The twist chain runs parent -> twist1 -> twist2 -> child; each twist
        // takes `depth / length` of the parent-to-child world deviation, so the
        // band between two bones is monotone instead of snapping at one joint.
        for (position, chain) in chains.iter().enumerate() {
            let Some(chain) = chain else { continue };
            let anchor_world = written[chain.anchor];
            let tip_world = written[chain.tip];
            let rest_anchor = DQuat::from_mat4(&rest_world(rig, tracks[chain.anchor].joint));
            let rest_tip = DQuat::from_mat4(&rest_world(rig, tracks[chain.tip].joint));
            let from = anchor_world * rest_anchor.inverse();
            let to = tip_world * rest_tip.inverse();
            // Only the *twist about the bone's own axis* is distributed. Handing the
            // band a fraction of the whole parent-to-child deviation moves it off the
            // bone's axis and shears the skin around the wrist (measured 3.45x on the
            // wrist band before this form): flexion belongs to the two bones, the
            // soft-tissue roll to the helpers between them.
            let axis = (rest_world(rig, tracks[chain.tip].joint).w_axis.truncate()
                - rest_world(rig, tracks[position].joint).w_axis.truncate())
            .normalize_or_zero();
            let deviation = match axis.length_squared() > 0.5 {
                true => {
                    let relative = from.inverse() * to;
                    let swing = DQuat::from_rotation_arc(axis, relative * axis);
                    let twist = swing.inverse() * relative;
                    from * slerp_shortest(DQuat::IDENTITY, twist, chain.fraction)
                }
                false => slerp_shortest(from, to, chain.fraction),
            };
            written[position] = deviation * rest_rotations[position];
        }
        // Every local is derived from the *written* parent world, so a chain the
        // source and the palette share stays consistent sample by sample.
        for (position, track) in tracks.iter().enumerate() {
            if track.rotation.is_none() {
                continue;
            }
            let parent_world = match closure[position] {
                Some((anchor, offset)) => written[anchor] * offset,
                None => match rig
                    .joint(track.joint)
                    .expect("a mapped bone is a rig joint")
                    .parent
                {
                    Some(parent) => rotation_of(rest_world(rig, parent)),
                    None => DQuat::IDENTITY,
                },
            };
            let local = parent_world.inverse() * written[position];
            rotation_values[position].push(local.normalize().to_array());
        }
        // Root translation is the one source distance that becomes a rig
        // distance; every other bone keeps the palette's own rest offset, because
        // an angle transfers across bodies and a length does not. The world delta
        // is already metric and already through the source's ancestors, so no
        // per-local unit ratio enters it.
        if let (Some(root), Some(source_position)) = (root_track, source_root)
            && !tracks[source_position].derived
        {
            let source_track = &tracks[source_position];
            let reference = source_world(source_track.node)?;
            let travel = written_position[source_position] - reference.w_axis.truncate();
            root_positions.push(travel);
            let track = &tracks[root];
            let rest = rig
                .joint(track.joint)
                .expect("a mapped bone is a rig joint")
                .rest;
            let rig_parent = match rig
                .joint(track.joint)
                .expect("a mapped bone is a rig joint")
                .parent
            {
                Some(parent) => rotation_of(rest_world(rig, parent)),
                None => DQuat::IDENTITY,
            };
            let mut local_delta = rig_parent.inverse() * (frame * travel * unit);
            if root_motion == RootMotion::Zero {
                local_delta = DVec3::ZERO;
            }
            let value = DVec3::from_array(rest.translation.map(f64::from)) + local_delta;
            translation_values[root].push(value.to_array());
        }
        for (position, track) in tracks.iter().enumerate() {
            // The travel of the source's own root bone is consumed by the
            // root-motion policy above; writing its local translation as well
            // would move the body twice.
            if track.derived || Some(position) == source_root || Some(position) == root_track {
                continue;
            }
            let Some(track_values) = &track.translation else {
                continue;
            };
            // A non-root translation channel is a source distance between two
            // bodies; only the motion copy keeps the old conversion, and only for
            // the coordinates that carry it.
            if semantics == ClipSemantics::Pose {
                continue;
            }
            let rest = rig
                .joint(track.joint)
                .expect("a mapped bone is a rig joint")
                .rest;
            let source_rest = source_local(track.node)?;
            let values = track_values;
            let delta =
                DVec3::from_array(values[sample]) - DVec3::from_array(source_rest.translation);
            let source_parent = match anchor.parent_world(actor, track.node) {
                Some(parent) => rotation_of(parent),
                None => rest_source_rotations[position],
            };
            let rig_parent = match rig
                .joint(track.joint)
                .expect("a mapped bone is a rig joint")
                .parent
            {
                Some(parent) => DQuat::from_mat4(&rest_world(rig, parent)),
                None => DQuat::IDENTITY,
            };
            let world_delta = source_parent * delta;
            let local_delta = rig_parent.inverse() * ((frame * world_delta) * unit);
            let value = DVec3::from_array(rest.translation.map(f64::from)) + local_delta;
            translation_values[position].push(value.to_array());
        }
        for (position, track) in tracks.iter().enumerate() {
            if track.derived {
                continue;
            }
            let Some(track_values) = &track.scale else {
                continue;
            };
            let rest = rig
                .joint(track.joint)
                .expect("a mapped bone is a rig joint")
                .rest;
            let source_rest = source_local(track.node)?;
            let values = track_values;
            let value = DVec3::from_array(values[sample]) / DVec3::from_array(source_rest.scale);
            scale_values[position].push((DVec3::splat(f64::from(rest.scale)) * value).to_array());
        }
    }
    let median_of = |values: &[f64]| -> f64 {
        let mut ordered = values.to_vec();
        ordered.sort_by(f64::total_cmp);
        match ordered.is_empty() {
            true => 0.0,
            false => ordered[ordered.len() / 2],
        }
    };
    let pose_error_median = median_of(&errors);
    let direction_error_median_degrees = median_of(&direction_errors);
    let direction_error_max_degrees = direction_errors.iter().copied().fold(0.0_f64, f64::max);
    // The root-motion split: the linear ramp is the travel a controller consumes,
    // the residual stays on the pelvis as sway. Reported, never guessed at.
    let root_travel = match (root_positions.first(), root_positions.last()) {
        (Some(first), Some(last)) => *last - *first,
        _ => DVec3::ZERO,
    };
    let mut root_motion_travel = 0.0_f64;
    if root_motion == RootMotion::Split
        && let Some(root) = root_track
        && let (Some(first), Some(last)) = (
            translation_values[root].first().copied(),
            translation_values[root].last().copied(),
        )
    {
        let ramp = DVec3::from_array(last) - DVec3::from_array(first);
        root_motion_travel = (frame * root_travel * unit).length();
        let times = source.times.as_slice();
        let start = times.first().copied().unwrap_or(0.0) as f64;
        let end = times.last().copied().unwrap_or(0.0) as f64;
        let span = (end - start).max(f64::EPSILON);
        let values = &mut translation_values[root];
        for (index, value) in values.iter_mut().enumerate() {
            let time = times.get(index).copied().unwrap_or(0.0) as f64;
            let fraction = (time - start) / span;
            *value = (DVec3::from_array(*value) - ramp * fraction).to_array();
        }
    }

    let times: Vec<f32> = source.times.clone();
    let duration = match (times.first(), times.last()) {
        (Some(first), Some(last)) => f64::from(*last) - f64::from(*first),
        _ => 0.0,
    };
    let mut channels = Vec::with_capacity(tracks.len());
    let mut moving_channels = 0;
    let mut max_motion_degrees = 0.0_f64;
    let mut max_motion_translation = 0.0_f64;
    let mut root_bones = 0;
    for (position, track) in tracks.iter().enumerate() {
        if rig
            .joint(track.joint)
            .expect("a mapped bone is a rig joint")
            .parent
            .is_none()
        {
            root_bones += 1;
        }
        if !rotation_values[position].is_empty() {
            let motion = magnitude(&rotation_values[position], |a, b| {
                quaternion_angle(&(DQuat::from_array(b) * DQuat::from_array(a).inverse()))
            });
            if motion > CONSTANT_EPSILON {
                moving_channels += 1;
                max_motion_degrees = max_motion_degrees.max(motion.to_degrees());
            }
            channels.push(ClipChannel::Rotation {
                bone: track.slot,
                values: rotation_values[position]
                    .iter()
                    .map(|value| value.map(|lane| lane as f32))
                    .collect(),
            });
        }
        if !translation_values[position].is_empty() {
            let motion = magnitude(&translation_values[position], |a, b| {
                DVec3::from_array(a).distance(DVec3::from_array(b))
            });
            if motion > CONSTANT_EPSILON {
                moving_channels += 1;
                max_motion_translation = max_motion_translation.max(motion);
            }
            channels.push(ClipChannel::Translation {
                bone: track.slot,
                values: translation_values[position]
                    .iter()
                    .map(|value| value.map(|lane| lane as f32))
                    .collect(),
            });
        }
        if !scale_values[position].is_empty() {
            let motion = magnitude(&scale_values[position], |a, b| {
                DVec3::from_array(a).distance(DVec3::from_array(b))
            });
            if motion > CONSTANT_EPSILON {
                moving_channels += 1;
                max_motion_translation = max_motion_translation.max(motion);
            }
            channels.push(ClipChannel::Scale {
                bone: track.slot,
                values: scale_values[position]
                    .iter()
                    .map(|value| value.map(|lane| lane as f32))
                    .collect(),
            });
        }
    }
    if moving_channels == 0 {
        return Err(refused(
            "export_glb_clip_constant",
            format!(
                "clip {:?} has {} channel(s) and none of them moves; a constant clip is a defect",
                source.name,
                channels.len()
            ),
        ));
    }
    Ok(ClipPlan {
        clip: Clip {
            name: source.name.clone(),
            times,
            channels,
        },
        duration,
        scale,
        mapped_bones: tracks.len(),
        mapped_bone_names: {
            let mut names: Vec<String> = tracks.iter().map(|track| track.name.clone()).collect();
            names.sort_unstable();
            names
        },
        dropped_bones: dropped.clone(),
        root_bones,
        moving_channels,
        max_motion_degrees,
        max_motion_translation,
        max_rest_rotation_degrees: max_orientation,
        max_rest_translation_residual: max_position_residual,
        rest_pose_comparable: pose_comparable,
        rest_pose_outlier: worst_bone.clone(),
        max_frame_delta_degrees: max_frame_delta,
        max_swing_degrees: max_swing,
        frame_outlier,
        roll_normalization_degrees: max_roll,
        scale_min,
        scale_max,
        scale_shared: share,
        scale_outliers,
        semantics,
        axis_convention_degrees,
        direction_error_median_degrees,
        direction_error_max_degrees,
        pose_error_median_degrees: pose_error_median,
        pose_error_max_degrees: max_error,
        pose_error_outlier: error_outlier,
        root_motion,
        root_motion_bone: root_track.map(|position| tracks[position].name.clone()),
        source_root_bone: source_root
            .filter(|position| !tracks[*position].derived)
            .map(|position| tracks[position].name.clone()),
        root_motion_travel,
        root_travel: (frame * root_travel * unit).length(),
    })
}

/// One bone offset whose unit ratio is not the rig's shared one.
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleOutlier {
    /// Bone name.
    pub bone: String,
    /// This bone's own ratio.
    pub ratio: f64,
    /// Source local offset length.
    pub source_length: f64,
    /// Rig local offset length.
    pub rig_length: f64,
}

/// The source rig's standing reference: one sample of one clip, used as the
/// pose every clip's motion is measured from.
///
/// The exported actor GLBs carry node rest transforms that are an armature
/// artifact, not the character's stance: on the measured ElfRanged file the
/// node rest differs from the idle pose by 76° at an upper arm. The reference
/// sample is the honest baseline, and it makes the written clip start at the
/// destination rest instead of at an artifact.
#[derive(Debug, Clone, Default)]
pub struct ClipAnchor {
    /// Node world transform at the reference sample.
    worlds: BTreeMap<usize, DMat4>,
    /// Node local transform at the reference sample.
    locals: BTreeMap<usize, sculpto_mesh::actor::LocalRest>,
}

impl ClipAnchor {
    /// Read the anchor from one clip, at its first sample.
    ///
    /// # Errors
    /// A clip without samples, and any channel defect the clip reader refuses.
    pub fn of(actor: &Actor, clip: &SourceClip) -> Result<Self, CommandError> {
        if clip.times.is_empty() {
            return Err(refused(
                "export_glb_clip_constant",
                format!("clip {:?} has no sample to anchor", clip.name),
            ));
        }
        let mut locals: BTreeMap<usize, sculpto_mesh::actor::LocalRest> = BTreeMap::new();
        for channel in &clip.channels {
            let entry = locals.entry(channel.node).or_insert_with(|| {
                actor
                    .rest_local(channel.node)
                    .expect("a clip channel targets a document node")
            });
            match (&channel.property, &channel.values) {
                (SourceProperty::Rotation, SourceValues::Rotation(values)) => {
                    entry.rotation = values[0];
                }
                (SourceProperty::Translation, SourceValues::Vector(values)) => {
                    entry.translation = values[0];
                }
                (SourceProperty::Scale, SourceValues::Vector(values)) => {
                    entry.scale = values[0];
                }
                _ => {}
            }
        }
        let mut worlds = BTreeMap::new();
        for node in locals.keys() {
            let world = anchor_world(actor, *node, 0, &locals, &mut BTreeMap::new());
            worlds.insert(*node, world);
        }
        Ok(Self { worlds, locals })
    }

    /// The anchor world of one source node, when the anchor clip animates it.
    pub fn world(&self, node: usize) -> Option<DMat4> {
        self.worlds.get(&node).copied()
    }

    /// The anchor local of one source node, when the anchor clip animates it.
    pub fn local(&self, node: usize) -> Option<sculpto_mesh::actor::LocalRest> {
        self.locals.get(&node).copied()
    }

    /// The anchor world of one source node's parent, when it has one.
    pub fn parent_world(&self, actor: &Actor, node: usize) -> Option<DMat4> {
        match actor.node_parents().get(node).copied().flatten() {
            Some(parent) => self.world(parent),
            None => None,
        }
    }
}

/// One node's world at one sample of a partially overridden pose.
fn anchor_world(
    actor: &Actor,
    node: usize,
    _sample: usize,
    locals: &BTreeMap<usize, sculpto_mesh::actor::LocalRest>,
    memo: &mut BTreeMap<usize, DMat4>,
) -> DMat4 {
    if let Some(found) = memo.get(&node) {
        return *found;
    }
    let local = match locals.get(&node) {
        Some(local) => local_matrix(*local),
        None => DMat4::from_cols_array_2d(
            &actor
                .rest_local_matrix(node)
                .expect("a document node's rest transform"),
        ),
    };
    let world = match actor.node_parents()[node] {
        Some(parent) => anchor_world(actor, parent, _sample, locals, memo) * local,
        None => local,
    };
    memo.insert(node, world);
    world
}

/// The rig direction one mapped bone is aligned onto, and the source node that
/// defines it: the first mapped child joint, so a leaf bone returns `None`.
fn direction_reference(
    rig: &Rig,
    tracks: &[Track],
    index: usize,
    anchor_worlds: &[DMat4],
    frame: DMat3,
    actor: &Actor,
) -> Option<(DVec3, usize)> {
    let track = &tracks[index];
    // The anatomical successor is the source graph's own first mapped child, so
    // both sides measure the same bone; a rig child such as a physics helper is
    // never substituted.
    let child =
        actor
            .node_parents()
            .iter()
            .enumerate()
            .find_map(|(node, parent)| match parent {
                Some(parent) if *parent == track.node => tracks
                    .iter()
                    .position(|other| other.node == node)
                    .map(|position| (position, node)),
                _ => None,
            })?;
    let (position, _node) = child;
    let origin = rest_world(rig, track.joint).w_axis.truncate();
    let direction =
        (rest_world(rig, tracks[position].joint).w_axis.truncate() - origin).normalize_or_zero();
    let source = frame
        * (anchor_worlds[position].w_axis.truncate() - anchor_worlds[index].w_axis.truncate())
            .normalize_or_zero();
    match direction.length_squared() > 0.5 && source.length_squared() > 0.5 {
        true => Some((direction, tracks[position].node)),
        false => None,
    }
}

/// One mapped source node and the channels it carries.
struct Track {
    node: usize,
    slot: usize,
    joint: JointId,
    name: String,
    translation: Option<Vec<[f64; 3]>>,
    rotation: Option<Vec<[f64; 4]>>,
    scale: Option<Vec<[f64; 3]>>,
    /// Source nodes whose local motion folds into this bone's rotation.
    fold: Vec<usize>,
    /// A palette bone no source drives: a twist the plan derives between its
    /// nearest mapped ancestor and descendant.
    derived: bool,
}

/// Where a derived twist sits in its chain: the mapped bones either side and
/// the fraction of the parent-to-child deviation it takes.
struct TwistChain {
    anchor: usize,
    tip: usize,
    fraction: f64,
}

/// The mapped ancestor and descendant a derived twist sits between, with its
/// chain fraction (`1/3`, `2/3` on a two-twist arm).
fn twist_chain(
    rig: &Rig,
    tracks: &[Track],
    roles: &[Option<crate::retarget::BoneRole>],
    slot_roles: &[Option<crate::retarget::BoneRole>],
    position: usize,
) -> Option<TwistChain> {
    use crate::retarget::{BoneRole, Part};
    // A side helper: its parent is the bone it serves, and the anatomical
    // successor of that parent is the other end of the chain.
    if let Some(BoneRole {
        side,
        part: Part::Twist { forearm, index },
    }) = roles[position]
    {
        let successor = match forearm {
            true => Part::Hand,
            false => Part::Forearm,
        };
        let parent = rig.joint(tracks[position].joint)?.parent?;
        // The parent may itself be a derived helper (the second twist of a pair):
        // its written world is already known when the chains are applied.
        let anchor = tracks.iter().position(|track| track.joint == parent)?;
        let tip = tracks.iter().position(|track| {
            slot_roles.get(track.slot).copied().flatten()
                == Some(BoneRole {
                    side,
                    part: successor,
                })
                && !track.derived
        })?;
        // Two helpers per family: 1/3 and 2/3 from the parent towards the
        // successor (Unity and Rigify both distribute roll by chain position).
        let family = slot_roles
            .iter()
            .filter(|role| {
                matches!(
                    role,
                    Some(BoneRole {
                        side: other_side,
                        part: Part::Twist { forearm: other, .. },
                    }) if *other_side == side && *other == forearm
                )
            })
            .count()
            .max(1);
        let fraction = f64::from(index) / (family as f64 + 1.0);
        return Some(TwistChain {
            anchor,
            tip,
            fraction: fraction.clamp(0.05, 0.95),
        });
    }
    let joint = tracks[position].joint;
    let mut children: BTreeMap<JointId, Vec<JointId>> = BTreeMap::new();
    for (index, definition) in rig.joints().iter().enumerate() {
        if let Some(parent) = definition.parent {
            children
                .entry(parent)
                .or_default()
                .push(JointId::new(index as u32));
        }
    }
    let mapped = |joint: JointId| -> Option<usize> {
        tracks
            .iter()
            .position(|track| track.joint == joint && !track.derived)
    };
    let mut up = 0_usize;
    let mut current = joint;
    let anchor = loop {
        let parent = rig.joint(current)?.parent?;
        up += 1;
        if let Some(found) = mapped(parent) {
            break found;
        }
        current = parent;
        if up > 8 {
            return None;
        }
    };
    let mut down = 0_usize;
    let mut current = joint;
    let tip = loop {
        let list = children.get(&current)?;
        if list.len() != 1 {
            return None;
        }
        let child = list[0];
        down += 1;
        if let Some(found) = mapped(child) {
            break found;
        }
        current = child;
        if down > 8 {
            return None;
        }
    };
    let total = (up + down) as f64;
    Some(TwistChain {
        anchor,
        tip,
        fraction: up as f64 / total,
    })
}

/// Interpolate two rotations the short way round: a sign flip between samples is
/// the same rotation, and the longer arc would spin the band backwards.
fn slerp_shortest(from: DQuat, to: DQuat, alpha: f64) -> DQuat {
    let mut to = to;
    if from.dot(to) < 0.0 {
        to = -to;
    }
    let theta = 2.0 * (from - to).length().atan2((from + to).length());
    match theta > 1e-9 {
        true => ((from * ((1.0 - alpha) * theta).sin() + to * (alpha * theta).sin()) / theta.sin())
            .normalize(),
        false => from,
    }
}

/// One rig joint's rest world transform.
fn rest_world(rig: &Rig, joint: JointId) -> DMat4 {
    DMat4::from_cols_array_2d(
        &rig.rest_world(joint)
            .expect("a mapped joint has a rest world"),
    )
}

/// One source node's rest world transform.
fn rest_world_of(actor: &Actor, node: usize) -> Result<DMat4, CommandError> {
    let mut worlds = vec![None; actor.node_parents().len()];
    rest_world_at(actor, node, &mut worlds)
}

fn rest_world_at(
    actor: &Actor,
    node: usize,
    worlds: &mut [Option<DMat4>],
) -> Result<DMat4, CommandError> {
    if let Some(found) = worlds[node] {
        return Ok(found);
    }
    let local = DMat4::from_cols_array_2d(&actor.rest_local_matrix(node).map_err(actor_error)?);
    let world = match actor.node_parents()[node] {
        Some(parent) => rest_world_at(actor, parent, worlds)? * local,
        None => local,
    };
    worlds[node] = Some(world);
    Ok(world)
}

fn local_matrix(local: sculpto_mesh::actor::LocalRest) -> DMat4 {
    DMat4::from_scale_rotation_translation(
        DVec3::from_array(local.scale),
        DQuat::from_array(local.rotation),
        DVec3::from_array(local.translation),
    )
}

/// Source node worlds for every sample, with the clip's channels applied.
struct SourceWorlds {
    parents: Vec<Option<usize>>,
    /// One local matrix per sample for every node the mapped bones reach.
    locals: BTreeMap<usize, Vec<DMat4>>,
}

impl SourceWorlds {
    fn new(actor: &Actor, tracks: &[Track], samples: usize) -> Result<Self, CommandError> {
        let parents: Vec<Option<usize>> = actor.node_parents().to_vec();
        let mut locals: BTreeMap<usize, Vec<DMat4>> = BTreeMap::new();
        for track in tracks.iter().filter(|track| !track.derived) {
            let mut node = Some(track.node);
            while let Some(current) = node {
                if locals.contains_key(&current) {
                    break;
                }
                let entry = tracks.iter().find(|other| other.node == current);
                let values: Vec<DMat4> = match entry {
                    Some(entry) => (0..samples)
                        .map(|sample| local_at(actor, entry, sample))
                        .collect::<Result<_, CommandError>>()?,
                    None => vec![DMat4::from_cols_array_2d(
                        &actor.rest_local_matrix(current).map_err(actor_error)?,
                    )],
                };
                locals.insert(current, values);
                node = parents[current];
            }
        }
        Ok(Self { parents, locals })
    }

    fn nodes(&self) -> usize {
        self.parents.len()
    }

    fn world(&self, node: usize, sample: usize, worlds: &mut [Option<DMat4>]) -> DMat4 {
        if let Some(found) = worlds[node] {
            return found;
        }
        let values = self
            .locals
            .get(&node)
            .expect("every reachable node has locals");
        let local = values[sample.min(values.len() - 1)];
        let world = match self.parents[node] {
            Some(parent) => self.world(parent, sample, worlds) * local,
            None => local,
        };
        worlds[node] = Some(world);
        world
    }
}

/// One node's local matrix at one sample.
fn local_at(actor: &Actor, track: &Track, sample: usize) -> Result<DMat4, CommandError> {
    let rest = actor.rest_local(track.node).map_err(actor_error)?;
    let mut rest = rest;
    if let Some(values) = &track.translation {
        rest.translation = values[sample];
    }
    if let Some(values) = &track.rotation {
        rest.rotation = values[sample];
    }
    if let Some(values) = &track.scale {
        rest.scale = values[sample];
    }
    Ok(local_matrix(rest))
}

/// The one rotation that maps the source skeleton's rest onto the rig's.
///
/// Three well-spread bone origins define an orthonormal frame on each side; the
/// mapping between those frames is the frame rotation. Poses that are not the
/// same skeleton leave a residual the caller measures.
fn rest_frame(origins: &[(usize, DVec3, DVec3)]) -> Result<DMat3, CommandError> {
    let center = |pick: fn(&(usize, DVec3, DVec3)) -> DVec3| {
        let total = origins
            .iter()
            .map(pick)
            .fold(DVec3::ZERO, |sum, point| sum + point);
        total / origins.len() as f64
    };
    let dest_center = center(|row| row.1);
    let source_center = center(|row| row.2);
    // Farthest bone from the centroid, then the bone that widens the frame most.
    let farthest = |pick: fn(&(usize, DVec3, DVec3)) -> DVec3, center: DVec3| {
        origins
            .iter()
            .map(|row| (pick(row) - center, pick(row)))
            .max_by(|a, b| a.0.length_squared().total_cmp(&b.0.length_squared()))
            .map(|(_, point)| point)
            .unwrap_or(center)
    };
    let dest_first = farthest(|row| row.1, dest_center);
    let source_first = farthest(|row| row.2, source_center);
    let axis = |point: DVec3, center: DVec3| (point - center).normalize_or_zero();
    let dest_u = axis(dest_first, dest_center);
    let source_u = axis(source_first, source_center);
    // Fewer than three bones, or a degenerate spread, cannot define a frame;
    // the identity is then the honest choice and the plan reports it.
    if origins.len() < 3 || dest_u.length_squared() < 0.5 || source_u.length_squared() < 0.5 {
        return Ok(DMat3::IDENTITY);
    }
    let widest = |u: DVec3, center: DVec3, pick: fn(&(usize, DVec3, DVec3)) -> DVec3| {
        origins
            .iter()
            .map(|row| (pick(row) - center).cross(u).length())
            .fold(0.0_f64, f64::max)
    };
    let width_dest = widest(dest_u, dest_center, |row| row.1);
    let width_source = widest(source_u, source_center, |row| row.2);
    if width_dest <= 0.0 || width_source <= 0.0 {
        return Ok(DMat3::IDENTITY);
    }
    let pick_second = |u: DVec3, center: DVec3, pick: fn(&(usize, DVec3, DVec3)) -> DVec3| {
        origins
            .iter()
            .map(|row| ((pick(row) - center).cross(u).length(), pick(row)))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, point)| point)
            .unwrap_or(center)
    };
    let dest_second = pick_second(dest_u, dest_center, |row| row.1);
    let source_second = pick_second(source_u, source_center, |row| row.2);
    let basis = |u: DVec3, second: DVec3, center: DVec3| {
        let v = axis(second, center).cross(u).normalize_or_zero();
        let w = u.cross(v);
        DMat3::from_cols(u, v, w)
    };
    let dest_basis = basis(dest_u, dest_second, dest_center);
    let source_basis = basis(source_u, source_second, source_center);
    Ok(dest_basis * source_basis.inverse())
}

/// Largest sample-to-sample spread of one channel's values.
fn magnitude<T: Copy>(samples: &[T], distance: impl Fn(T, T) -> f64) -> f64 {
    let Some(first) = samples.first().copied() else {
        return 0.0;
    };
    samples
        .iter()
        .map(|sample| distance(*sample, first))
        .fold(0.0, f64::max)
}

/// The rotation of one affine with any scale divided out.
///
/// A source under a `0.01` ancestor carries that factor into every world matrix,
/// and a quaternion read straight off a scaled basis is not unit length — the
/// failure is silent and total: the written pose comes out half-sized and turns
/// with the wrong sign.
fn rotation_of(matrix: DMat4) -> DQuat {
    let (_, rotation, _) = matrix.to_scale_rotation_translation();
    rotation
}

/// Rotation angle of one quaternion, radians in `[0, π]`.
fn quaternion_angle(value: &DQuat) -> f64 {
    2.0 * value.w.abs().clamp(-1.0, 1.0).acos()
}

/// Scale the whole merged scene into one unit system.
///
/// Every affine is conjugated by the same similarity (`S · M · S⁻¹`), so the
/// rest proof that held in source units still holds exactly.
pub(crate) fn scale_merged(merged: &mut MergedActor, scale: f64) {
    let similarity = DMat4::from_scale(DVec3::splat(scale));
    let inverse = similarity.inverse();
    for position in &mut merged.asset.positions {
        for axis in position {
            *axis = (*axis as f64 * scale) as f32;
        }
    }
    if let Some(skin) = merged.asset.skin.as_mut() {
        for bind in &mut skin.bind_inverse {
            *bind = conjugate(*bind, similarity, inverse);
        }
    }
    merged.global_to_skin = conjugate(merged.global_to_skin, similarity, inverse);
    merged.basis = conjugate_f64(merged.basis, similarity, inverse);
}

/// One rig in another unit: every rest translation is scaled, every rotation
/// and scale stays as authored.
pub(crate) fn scale_rig(rig: &Rig, scale: f64) -> Result<Rig, CommandError> {
    let joints = rig
        .joints()
        .iter()
        .map(|joint| sculpto_core::rig::Joint {
            name: joint.name.clone(),
            parent: joint.parent,
            rest: sculpto_core::rig::LocalTransform {
                translation: joint
                    .rest
                    .translation
                    .map(|value| (f64::from(value) * scale) as f32),
                ..joint.rest
            },
        })
        .collect();
    Rig::new(joints, rig.frame()).map_err(|error| {
        refused(
            "export_glb_rest",
            format!("the scaled rig does not validate: {error}"),
        )
    })
}

/// One clip in another unit: translation samples scale, rotations and ratios
/// do not.
pub(crate) fn scale_clip(clip: &mut Clip, scale: f64) {
    for channel in &mut clip.channels {
        if let ClipChannel::Translation { values, .. } = channel {
            for value in values {
                for lane in value {
                    *lane = (*lane as f64 * scale) as f32;
                }
            }
        }
    }
}

/// One `f32` affine conjugated by a similarity.
pub(crate) fn conjugate(matrix: [[f32; 4]; 4], similarity: DMat4, inverse: DMat4) -> [[f32; 4]; 4] {
    let matrix = DMat4::from_cols_array_2d(&matrix.map(|column| column.map(f64::from)));
    (similarity * matrix * inverse)
        .to_cols_array_2d()
        .map(|column| column.map(|value| value as f32))
}

/// One `f64` affine conjugated by a similarity.
pub(crate) fn conjugate_f64(
    matrix: [[f64; 4]; 4],
    similarity: DMat4,
    inverse: DMat4,
) -> [[f64; 4]; 4] {
    let matrix = DMat4::from_cols_array_2d(&matrix);
    (similarity * matrix * inverse).to_cols_array_2d()
}

/// Influence width statistics of the merged skin.
pub(crate) fn influence_summary(skin: Option<&Skin>) -> Value {
    let Some(skin) = skin else {
        return json!({ "vertices": 0, "widest": 0 });
    };
    let mut widest = 0;
    let mut worst_sum = 0.0_f64;
    for vertex in 0..skin.influences.rows() {
        let Some(row) = skin.influences.row(vertex) else {
            continue;
        };
        let positive = row.weights.iter().filter(|weight| **weight > 0.0).count();
        widest = widest.max(positive);
        let sum: f64 = row.weights.iter().map(|weight| f64::from(*weight)).sum();
        worst_sum = worst_sum.max((sum - 1.0).abs());
    }
    json!({
        "vertices": skin.influences.rows(),
        "widest": widest,
        "max_sum_error": worst_sum,
    })
}

/// Axis-aligned bounds of the written positions.
pub(crate) fn bounds(positions: &[[f32; 3]]) -> Value {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for (axis, value) in position.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
        }
    }
    json!({ "min": min, "max": max })
}

pub(crate) fn refused(code: &'static str, message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorKind::Refused, code, message)
}

pub(crate) fn weight_error(error: SkinWeightError) -> CommandError {
    refused("export_glb_skin", error.to_string())
}

pub(crate) fn actor_error(error: ActorError) -> CommandError {
    match &error {
        ActorError::Clip { name, matches } => CommandError::new(
            ErrorKind::NotFound,
            "export_glb_clip_missing",
            format!("clip {name:?}: {matches} match(es) in the source GLB"),
        ),
        ActorError::Unsupported { .. } => CommandError::new(
            ErrorKind::Unsupported,
            "export_glb_clip_unsupported",
            error.to_string(),
        ),
        _ => CommandError::new(
            ErrorKind::InvalidRequest,
            "export_glb_clip",
            error.to_string(),
        ),
    }
}

pub(crate) fn rest_error(error: RestBindError) -> CommandError {
    refused("export_glb_rest", error.to_string())
}

/// The writer's refusals keep their own names.
pub(crate) fn glb_error(error: GltfError) -> CommandError {
    match &error {
        GltfError::ConstantClip { .. } => refused("export_glb_clip_constant", error.to_string()),
        GltfError::ClipBoneOutOfRange { .. } => {
            refused("export_glb_clip_bone_absent", error.to_string())
        }
        GltfError::RigBinding { .. } => refused("export_glb_rest", error.to_string()),
        GltfError::Serialize(detail) | GltfError::Encode(detail) => CommandError::new(
            ErrorKind::Io,
            "export_glb_write",
            format!("GLB write failed: {detail}"),
        ),
        other => CommandError::new(
            ErrorKind::Unsupported,
            "export_glb_unsupported",
            other.to_string(),
        ),
    }
}

pub(crate) fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    std::fs::write(path, bytes).map_err(|error| errors::io(path, error))
}

pub(crate) fn write_report(path: &Path, report: &Value) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(report).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "export_glb_report",
            format!("{}: {error}", path.display()),
        )
    })?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    std::fs::write(path, format!("{text}\n")).map_err(|error| {
        CommandError::new(
            ErrorKind::Io,
            "export_glb_report",
            format!("{}: {error}", path.display()),
        )
    })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
