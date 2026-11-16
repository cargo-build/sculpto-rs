//! `actor adapt`: convert a clip authored on *any* skeleton onto an actor's own
//! palette, without touching the palette.
//!
//! The operation is a correspondence layer around the existing converter: the
//! source skeleton is resolved onto the actor's palette ([`crate::retarget`]),
//! the clip is planned with an explicit semantics and root-motion policy
//! ([`plan_clip_with`]), admitted by the same gate a same-family clip passes
//! ([`admit_clip`]), and written into a copy of the actor GLB whose nodes, skin
//! and rest are byte-identical. Every gate is measured on the written file, not
//! on the plan: the pose error, the palette reconstruction, the stance-foot slide
//! and the body's edge stretch all come from re-reading the output. @req DECL0013

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::{DMat4, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_core::rig::{Joint, LocalTransform, Rig, RigFrame};
use sculpto_gltf::pack::{PackWriter, Track};
use sculpto_mesh::actor::{Actor, Pose};

use crate::export_actor::{CLIP_ROLES, admit_clip};
use crate::export_glb::{
    ClipAnchor, ClipPlan, ClipSemantics, ClipTolerances, RootMotion, plan_clip_with, refused,
};
use crate::measures::{
    EDGE_STRETCH_CAP, RELATIVE_STRETCH, edge_stretch, measurable_parts, palette_reconstruction,
    toe_slide,
};
use crate::retarget::{Correspondence, Part, correspondence_report, require_mapped, role_coverage};
use crate::{CommandError, ErrorKind, errors};

/// A foot is in contact while within this height of its own minimum, metres.
pub use crate::measures::CONTACT_HEIGHT;
/// Admitted stance-foot speed while in contact, metres per second: 0.1 over one
/// 33 ms frame is 3.3 mm of slip, under the 5 mm the eye resolves at gait speed.
pub use crate::measures::CONTACT_SPEED;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdaptParams {
    /// Actor GLB whose palette the clip must drive, unchanged.
    pub actor: PathBuf,
    /// Source GLB that carries the clip, authored on any skeleton.
    pub source: PathBuf,
    /// Source clip name; when absent the file must carry exactly one.
    #[serde(default)]
    pub source_clip: Option<String>,
    /// Name the written clip takes; defaults to the source clip's own name.
    #[serde(default)]
    pub clip: Option<String>,
    /// Pose transfer: `pose` (source pose, the cross-body default) or `motion`.
    #[serde(default)]
    pub semantics: Option<String>,
    /// Root-motion policy: `keep` (default), `split` or `zero`.
    #[serde(default)]
    pub root_motion: Option<String>,
    /// Write the adapted actor GLB here.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Write the JSON report here.
    #[serde(default)]
    pub report: Option<PathBuf>,
    /// Roles the clip must drive; defaults to the rig's ten standard roles.
    #[serde(default)]
    pub require_roles: Vec<String>,
    /// Declare the source's rest pose incompatible on purpose: a cross-body
    /// source always is, and the measured rest table is the honest record.
    #[serde(default = "default_true")]
    pub allow_incompatible_rest: bool,
    /// Frames to sample for the toe-slide and edge-stretch gates.
    #[serde(default = "default_frames")]
    pub frames: usize,
}

fn default_true() -> bool {
    true
}

fn default_frames() -> usize {
    30
}

impl Default for AdaptParams {
    fn default() -> Self {
        Self {
            actor: PathBuf::new(),
            source: PathBuf::new(),
            source_clip: None,
            clip: None,
            semantics: None,
            root_motion: None,
            out: None,
            report: None,
            require_roles: Vec::new(),
            allow_incompatible_rest: true,
            frames: default_frames(),
        }
    }
}

/// The declared semantics, defaulting to the pose copy when the source skeleton
/// is not the actor's own (which is the only reason to adapt at all).
fn semantics_of(
    params: &AdaptParams,
    frame_gap_degrees: f64,
) -> Result<ClipSemantics, CommandError> {
    match params.semantics.as_deref() {
        // A source of another skeleton defaults to the pose copy: it carries the
        // clip's *stance* (a planted foot, a bent knee) as well as its motion, and
        // it keeps this rig's own bone frames. The motion copy is for a source that
        // shares our rest, where an anchored delta is the faithful reading.
        None => {
            let _ = frame_gap_degrees;
            Ok(ClipSemantics::Pose)
        }
        Some("pose") => Ok(ClipSemantics::Pose),
        Some("motion") => Ok(ClipSemantics::Motion),
        Some(other) => Err(errors::invalid(
            "semantics",
            format!("{other:?} is not a semantics; the modes are `pose` and `motion`"),
        )),
    }
}

fn root_motion_of(params: &AdaptParams) -> Result<RootMotion, CommandError> {
    match params.root_motion.as_deref() {
        None | Some("keep") => Ok(RootMotion::Keep),
        Some("split") => Ok(RootMotion::Split),
        Some("zero") => Ok(RootMotion::Zero),
        Some(other) => Err(errors::invalid(
            "root_motion",
            format!(
                "{other:?} is not a root-motion policy; the modes are `keep`, `split` and `zero`"
            ),
        )),
    }
}

/// Rest world transform of every node, from the actor's rest locals.
pub(crate) fn actor_rest_worlds(actor: &Actor) -> Result<Vec<DMat4>, CommandError> {
    fn resolve(
        actor: &Actor,
        node: usize,
        worlds: &mut Vec<Option<DMat4>>,
    ) -> Result<DMat4, CommandError> {
        if let Some(found) = worlds[node] {
            return Ok(found);
        }
        let local = DMat4::from_cols_array_2d(&actor.rest_local_matrix(node).map_err(actor_error)?);
        let world = match actor.node_parents()[node] {
            Some(parent) => resolve(actor, parent, worlds)? * local,
            None => local,
        };
        worlds[node] = Some(world);
        Ok(world)
    }
    let mut worlds: Vec<Option<DMat4>> = vec![None; actor.node_parents().len()];
    for node in 0..worlds.len() {
        resolve(actor, node, &mut worlds)?;
    }
    Ok(worlds
        .into_iter()
        .map(|world| world.expect("every node resolved"))
        .collect())
}

/// The actor's own palette as a rig: one joint per skin joint, in palette order,
/// with the rest the actor file already carries.
///
/// The palette is the skin's joint array, and the rig is its skeleton: a bone's
/// parent is its nearest palette ancestor, so a scaffolding node between two
/// joints is folded into the local without changing any world transform.
pub fn actor_rig(actor: &Actor) -> Result<(Rig, Vec<String>), CommandError> {
    let joints = actor.skin_joints().map_err(actor_error)?;
    if joints.is_empty() {
        return Err(refused(
            "adapt_palette",
            "the actor's skin binds no joint, so there is no palette to drive",
        ));
    }
    let name_of = |node: usize| -> Result<String, CommandError> {
        actor.node_name(node).map(str::to_owned).ok_or_else(|| {
            refused(
                "adapt_palette",
                format!("skin joint node {node} is unnamed, so a clip cannot name it"),
            )
        })
    };
    let palette: Vec<String> = joints
        .iter()
        .map(|node| name_of(*node))
        .collect::<Result<_, _>>()?;
    let worlds = actor_rest_worlds(actor)?;
    let slot_of: BTreeMap<usize, usize> = joints
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    let mut rows: Vec<(
        Option<sculpto_core::rig::JointId>,
        LocalTransform,
        Option<String>,
    )> = Vec::with_capacity(joints.len());
    for (slot, node) in joints.iter().enumerate() {
        // The nearest palette ancestor, walking through scaffolding nodes.
        let mut parent = actor.node_parents()[*node];
        let mut parent_slot = None;
        while let Some(candidate) = parent {
            if let Some(found) = slot_of.get(&candidate) {
                parent_slot = Some(*found);
                break;
            }
            parent = actor.node_parents()[candidate];
        }
        let parent_world = match parent_slot {
            Some(slot) => worlds[joints[slot]],
            None => DMat4::IDENTITY,
        };
        let local = parent_world.inverse() * worlds[*node];
        let (scale, rotation, translation) = local.to_scale_rotation_translation();
        if (scale.x - scale.y).abs() > 1e-6 || (scale.x - scale.z).abs() > 1e-6 {
            return Err(refused(
                "adapt_palette",
                format!(
                    "palette bone {:?} carries a non-uniform rest scale ({scale:?}); the rig model \
                     has one uniform scale per joint",
                    palette[slot]
                ),
            ));
        }
        rows.push((
            parent_slot.map(|slot| sculpto_core::rig::JointId::new(slot as u32)),
            LocalTransform {
                translation: translation.to_array().map(|value| value as f32),
                rotation: rotation.normalize().to_array().map(|value| value as f32),
                scale: scale.x as f32,
            },
            Some(palette[slot].clone()),
        ));
    }
    let joints: Vec<Joint> = rows
        .into_iter()
        .map(|(parent, rest, name)| Joint { name, parent, rest })
        .collect();
    let rig = Rig::new(joints, RigFrame::YUp).map_err(|error| {
        errors::invalid(
            "adapt_palette",
            format!("the actor's palette does not form a rig: {error}"),
        )
    })?;
    Ok((rig, palette))
}

fn actor_error(error: sculpto_mesh::actor::ActorError) -> CommandError {
    crate::export_glb::actor_error(error)
}

/// The clip the source GLB carries, by name or by being the only one.
pub(crate) fn source_clip_name(
    actor: &Actor,
    declared: Option<&str>,
) -> Result<String, CommandError> {
    let names: Vec<String> = actor
        .clip_names()
        .into_iter()
        .filter_map(|name| name.map(str::to_owned))
        .collect();
    match declared {
        Some(name) => match names.iter().any(|found| found == name) {
            true => Ok(name.to_owned()),
            false => Err(refused(
                "adapt_source_clip",
                format!("the source GLB carries no clip {name:?}; it carries {names:?}"),
            )),
        },
        None => match names.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(refused(
                "adapt_source_clip",
                "the source GLB carries no named clip",
            )),
            _ => Err(errors::invalid(
                "source_clip",
                format!(
                    "the source GLB carries {} clips {names:?}; declare one",
                    names.len()
                ),
            )),
        },
    }
}

/// Unique edges of one sample, with their rest length; coincident seam vertices
/// are dropped because a ratio on a zero-length edge says nothing.
fn edge_list(sample: &sculpto_mesh::actor::Sample) -> Vec<(usize, usize, f64)> {
    let mut edges = Vec::new();
    for part in &sample.parts {
        let mut seen: std::collections::BTreeSet<(usize, usize)> =
            std::collections::BTreeSet::new();
        for triangle in &sample.triangles[part.triangles.clone()] {
            for pair in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let ordered = match pair.0 <= pair.1 {
                    true => pair,
                    false => (pair.1, pair.0),
                };
                if !seen.insert(ordered) {
                    continue;
                }
                let length = DVec3::from_array(sample.vertices[ordered.0])
                    .distance(DVec3::from_array(sample.vertices[ordered.1]));
                if length >= 1e-4 {
                    edges.push((ordered.0, ordered.1, length));
                }
            }
        }
    }
    edges
}

/// Largest stretch among edges at least `floor` metres long, with the edge that
/// carried it.
fn max_ratio(
    vertices: &[[f64; 3]],
    edges: &[(usize, usize, f64)],
    floor: f64,
) -> (f64, Option<(usize, usize)>) {
    let mut worst = 0.0_f64;
    let mut witness = None;
    for (first, second, length) in edges {
        if *length < floor {
            continue;
        }
        let posed =
            DVec3::from_array(vertices[*first]).distance(DVec3::from_array(vertices[*second]));
        let ratio = posed / length;
        if ratio > worst {
            worst = ratio;
            witness = Some((*first, *second));
        }
    }
    (worst, witness)
}

/// The body's own baseline at the same joint angles, and the relative verdict.
///
/// The absolute stretch measures the *body*: on these two actors a bare
/// `actor pose` of 40 degrees of hip separation already tears the crotch strip, and
/// Mike's own shipped walk tears it harder than the adapted clip does. The gate is
/// therefore relative — the conversion may not stretch the body more than the body
/// stretches itself when only the witness joints are posed, which is the same
/// configuration with everything else at rest.
#[allow(clippy::too_many_arguments)]
fn relative_baseline(
    actor: &Actor,
    bytes: &[u8],
    clip: &str,
    plan: &ClipPlan,
    palette: &[String],
    peak_time: f64,
    frames: usize,
    absolute_five_mm: f64,
) -> Result<Value, CommandError> {
    let named = measurable_parts(actor);
    let parts: Vec<&str> = named.iter().map(String::as_str).collect();
    let rest = actor.sample(&parts, Pose::Rest).map_err(actor_error)?;
    let edges = edge_list(&rest);
    let sampled = actor
        .rig_sample(
            &parts,
            Pose::Clip {
                name: clip,
                time: peak_time,
            },
            &palette.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .map_err(actor_error)?;
    let (_, witness) = max_ratio(&sampled.sample.vertices, &edges, 5e-3);
    let Some((first, second)) = witness else {
        return Ok(json!({
            "pass": true,
            "reason": "no edge of 5 mm or more exists, so no witness to compare",
        }));
    };
    // Every joint that materially owns one of the two witness vertices, not only
    // the heaviest: a crotch vertex is 77 % pelvis and 13 % each thigh, and it is
    // the thighs that tear it. A baseline that posed only the pelvis would report
    // a body that does not move.
    let owners = |vertex: usize| -> Vec<String> {
        let Some(row) = sampled.influences.get(vertex) else {
            return Vec::new();
        };
        row.iter()
            .filter(|influence| influence.weight >= 0.05)
            .filter_map(|influence| sampled.rig.get(influence.rig)?.name.clone())
            .collect()
    };
    let mut bones: Vec<String> = owners(first).into_iter().chain(owners(second)).collect();
    bones.sort_unstable();
    bones.dedup();
    // A vertex moves with its influence bones *and their world orientations*, so the
    // baseline carries the ancestors of the witnesses as well. Derived helpers are
    // never ancestors of the bones they serve, so a bad twist distribution still
    // shows up as a difference instead of being baked into the baseline.
    let mut with_ancestors: Vec<String> = Vec::new();
    {
        let (rig, _) = actor_rig(actor)?;
        for name in &bones {
            let Ok(mut joint) = rig.joint_by_name(name) else {
                continue;
            };
            while let Some(parent) = rig.joint(joint).and_then(|definition| definition.parent) {
                if let Some(definition) = rig.joint(parent)
                    && let Some(parent_name) = definition.name.clone()
                {
                    with_ancestors.push(parent_name);
                }
                joint = parent;
            }
        }
    }
    bones.extend(with_ancestors);
    bones.sort_unstable();
    bones.dedup();
    // The same joints at the same angles, everything else at rest.
    let times = plan.clip.times.clone();
    let mut tracks: Vec<Track> = Vec::new();
    for channel in &plan.clip.channels {
        if let sculpto_gltf::ClipChannel::Rotation { bone, values } = channel
            && bones.iter().any(|name| palette.get(*bone) == Some(name))
        {
            tracks.push(Track::Rotation {
                node: *bone,
                values: values.clone(),
            });
        }
    }
    if tracks.is_empty() {
        return Ok(json!({
            "pass": true,
            "reason": "the witness joints carry no rotation channel",
            "witness_bones": bones,
        }));
    }
    // The writer addresses nodes, so the palette slot is mapped through the skin.
    let joints = actor.skin_joints().map_err(actor_error)?;
    for track in &mut tracks {
        if let Track::Rotation { node, .. } = track {
            *node = joints[*node];
        }
    }
    let mut writer = PackWriter::from_glb(bytes).map_err(|error| {
        CommandError::new(
            ErrorKind::Unsupported,
            "adapt_baseline",
            format!("the actor GLB cannot carry the baseline pose: {error}"),
        )
    })?;
    let name = format!("{} (baseline)", plan.clip.name);
    writer
        .insert_animation(1, &name, &times, &tracks)
        .map_err(|error| {
            CommandError::new(
                ErrorKind::Unsupported,
                "adapt_baseline",
                format!("the baseline pose cannot be packed: {error}"),
            )
        })?;
    let baseline_bytes = writer.finish().map_err(|error| {
        CommandError::new(
            ErrorKind::Unsupported,
            "adapt_baseline",
            format!("the baseline actor cannot be framed: {error}"),
        )
    })?;
    let baseline_actor = Actor::from_glb(&baseline_bytes).map_err(actor_error)?;
    // The same sweep the absolute number uses: a maximum over frames, not one
    // frame. Comparing a 30-frame maximum against a single frame would charge the
    // body's own peak to the conversion.
    let (start, end) = baseline_actor.clip_span(&name).map_err(actor_error)?;
    let mut baseline = 0.0_f64;
    for index in 0..frames.max(2) {
        let time = start + (end - start) * index as f64 / (frames.max(2) - 1) as f64;
        let posed = baseline_actor
            .sample(&parts, Pose::Clip { name: &name, time })
            .map_err(actor_error)?;
        let (ratio, _) = max_ratio(&posed.vertices, &edges, 5e-3);
        baseline = baseline.max(ratio);
    }
    let _ = peak_time;
    let relative = match baseline > 0.0 {
        true => absolute_five_mm / baseline,
        false => f64::INFINITY,
    };
    Ok(json!({
        "witness_bones": bones,
        "baseline_pose_ratio": baseline,
        "absolute_ratio": absolute_five_mm,
        "relative": relative,
        "bar": RELATIVE_STRETCH,
        "pass": relative <= RELATIVE_STRETCH,
    }))
}

/// Convert a clip from any skeleton onto one actor's palette.
///
/// # Errors
/// A missing or unreadable file, a palette the source cannot be resolved onto, an
/// ambiguous source clip, the converter's own refusals, and the admission gate's
/// role-coverage refusal.
pub fn run(params: &AdaptParams) -> Result<Value, CommandError> {
    let base_bytes =
        std::fs::read(&params.actor).map_err(|error| errors::io(&params.actor, error))?;
    let base = Actor::from_glb(&base_bytes).map_err(actor_error)?;
    let source_bytes =
        std::fs::read(&params.source).map_err(|error| errors::io(&params.source, error))?;
    let source = Actor::from_glb(&source_bytes).map_err(actor_error)?;
    let clip_name = source_clip_name(&source, params.source_clip.as_deref())?;
    let source_clip = source.clip_shared(&clip_name).map_err(actor_error)?;
    if source_clip.times.len() < 2 {
        return Err(refused(
            "adapt_source_clip",
            format!(
                "clip {clip_name:?} has {} sample(s); a conversion needs at least two",
                source_clip.times.len()
            ),
        ));
    }
    let (rig, palette) = actor_rig(&base)?;
    let correspondence = Correspondence::build(&source, &rig, &palette)?;
    require_mapped(&correspondence, &params.source.to_string_lossy())?;
    // The measured convention gap between the two bone frames, from the pairs the
    // correspondence resolved: the median rest world-orientation difference.
    let frame_gap_degrees = {
        let mut angles: Vec<f64> = correspondence
            .pairs
            .iter()
            .filter(|pair| pair.source.is_some())
            .map(|pair| pair.rest_angle_degrees)
            .collect();
        angles.sort_by(f64::total_cmp);
        match angles.is_empty() {
            true => 0.0,
            false => angles[angles.len() / 2],
        }
    };
    let semantics = semantics_of(params, frame_gap_degrees)?;
    let root_motion = root_motion_of(params)?;
    let anchor = ClipAnchor::of(&source, &source_clip)?;
    let tolerances = ClipTolerances::default();
    let plan = plan_clip_with(
        &source_clip,
        &source,
        &anchor,
        &rig,
        &palette,
        &tolerances,
        Some(&correspondence),
        semantics,
        root_motion,
    )?;
    let required: Vec<String> = match params.require_roles.is_empty() {
        true => CLIP_ROLES.iter().map(|role| (*role).to_owned()).collect(),
        false => params.require_roles.clone(),
    };
    // The admission gate's *rest* half comes from the converter, which measures
    // it; its role half reads SSE names and would report zero coverage for any
    // other palette (`clip_role` is SSE-bound), so the roles are counted from the
    // correspondence that resolved them.
    let coverage = role_coverage(&correspondence);
    let (covered, missing) = (coverage.covered.clone(), coverage.missing.clone());
    if !required.is_empty() {
        let refused_roles: Vec<&str> = required
            .iter()
            .filter(|role| missing.contains(&role.as_str()))
            .map(String::as_str)
            .collect();
        if !refused_roles.is_empty() {
            return Err(refused(
                "adapt_role_coverage",
                format!(
                    "the correspondence drives {} of the roles this clip must cover and misses \
                     {}; covered: {}",
                    covered.len(),
                    refused_roles.join(", "),
                    covered.join(", ")
                ),
            ));
        }
    }
    // The rigid-torso half of the admission gate is answered by the role check
    // above (`spine` is one of the roles `role_coverage` asserts), so the
    // converter's own copy of it is admitted rather than doubled.
    let rest_admission = admit_clip(
        &plan,
        &palette,
        &[],
        params.allow_incompatible_rest,
        true,
        tolerances.translation_relative,
    )?;
    // A name the actor already carries would make the written clip ambiguous for
    // every later read of that file, so a self-conversion gets a distinct one.
    let mut written_name = params.clip.clone().unwrap_or_else(|| clip_name.clone());
    if base
        .clip_names()
        .into_iter()
        .flatten()
        .any(|existing| existing == written_name)
    {
        written_name = format!("{written_name} (adapted)");
    }
    let written_clip = json!({
        "name": written_name,
        "source_clip": clip_name,
        "semantics": semantics.token(),
        "frame_gap_degrees": frame_gap_degrees,
        "root_motion": root_motion.token(),
        "root_motion_bone": plan.root_motion_bone,
        "source_root_bone": plan.source_root_bone,
        "mapped_bones": plan.mapped_bones,
        "dropped_channels": plan.dropped_bones.len(),
        "moving_channels": plan.moving_channels,
        "max_motion_degrees": plan.max_motion_degrees,
        "duration": plan.duration,
        "unit": plan.scale,
        "axis_convention_degrees": plan.axis_convention_degrees,
        "direction_error_median_degrees": plan.direction_error_median_degrees,
        "direction_error_max_degrees": plan.direction_error_max_degrees,
        "pose_error_median_degrees": plan.pose_error_median_degrees,
        "pose_error_max_degrees": plan.pose_error_max_degrees,
        "pose_error_outlier": plan.pose_error_outlier,
        "lever_rules": plan.lever_rules,
        "rest_pose_comparable": plan.rest_pose_comparable,
        "max_rest_rotation_degrees": plan.max_rest_rotation_degrees,
        "max_rest_translation_residual": plan.max_rest_translation_residual,
        "rest_pose_outlier": plan.rest_pose_outlier,
        "scale_shared": plan.scale_shared,
        "scale_outliers": plan.scale_outliers.len(),
        "root_travel": plan.root_travel,
        "root_motion_travel": plan.root_motion_travel,
        "roles_covered": covered,
        "roles_missing": missing,
        "untouched_bones": rest_admission.untouched_bones.len(),
    });
    // Write the clip into a copy of the actor: nodes, skin and rest are the base
    // file's, and only the animation block is appended.
    let mut tracks: Vec<Track> = Vec::with_capacity(plan.clip.channels.len());
    let mut skipped_scale = 0;
    let joints = base.skin_joints().map_err(actor_error)?;
    for channel in &plan.clip.channels {
        match channel {
            sculpto_gltf::ClipChannel::Rotation { bone, values } => {
                tracks.push(Track::Rotation {
                    node: joints[*bone],
                    values: values.clone(),
                });
            }
            sculpto_gltf::ClipChannel::Translation { bone, values } => {
                tracks.push(Track::Translation {
                    node: joints[*bone],
                    values: values.clone(),
                });
            }
            sculpto_gltf::ClipChannel::Scale { .. } => skipped_scale += 1,
        }
    }
    let mut writer = PackWriter::from_glb(&base_bytes).map_err(|error| {
        CommandError::new(
            ErrorKind::Unsupported,
            "adapt_write",
            format!("the actor GLB cannot be extended: {error}"),
        )
    })?;
    let written_index = writer
        .insert_animation(1, &written_name, &plan.clip.times, &tracks)
        .map_err(|error| {
            CommandError::new(
                ErrorKind::Unsupported,
                "adapt_write",
                format!("the adapted clip cannot be packed: {error}"),
            )
        })?;
    let out_bytes = writer.finish().map_err(|error| {
        CommandError::new(
            ErrorKind::Unsupported,
            "adapt_write",
            format!("the adapted actor cannot be framed: {error}"),
        )
    })?;
    if let Some(path) = &params.out {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        std::fs::write(path, &out_bytes).map_err(|error| errors::io(path, error))?;
    }
    let written = Actor::from_glb(&out_bytes).map_err(actor_error)?;
    let reconstruction = palette_reconstruction(&base, &written)?;
    let toes: Vec<String> = correspondence
        .pairs
        .iter()
        .filter(|pair| matches!(pair.role.map(|role| role.part), Some(Part::Toe)))
        .map(|pair| pair.palette.clone())
        .collect();
    // With the root motion zeroed the clip carries no travel by construction, so a
    // stance foot that is planted in the source must slide relative to the frozen
    // origin. Measuring it would grade the preview policy, not the conversion.
    let slide = match (root_motion, toes.is_empty()) {
        (RootMotion::Zero, _) => json!({
            "pass": true,
            "gate": false,
            "note": "root motion zeroed: the clip plays in place, so stance slide is not applicable",
        }),
        (_, true) => json!({"pass": false, "reason": "the palette carries no toe joint"}),
        (_, false) => toe_slide(&written, &written_name, &palette, &toes, params.frames)?,
    };
    let mut stretch = edge_stretch(&written, &written_name, params.frames)?;
    // The gate is relative to the body's own baseline: the same joints at the same
    // angles with everything else at rest. The absolute number stays in the report
    // as the body's property, not as the conversion's verdict.
    let peak = stretch["worst_time"].as_f64().unwrap_or(0.0);
    let absolute_five = stretch["max_ratio_edges_at_least_5mm"]
        .as_f64()
        .unwrap_or(0.0);
    let baseline = relative_baseline(
        &written,
        &out_bytes,
        &written_name,
        &plan,
        &palette,
        peak,
        params.frames,
        absolute_five,
    )?;
    stretch["absolute_pass"] = json!(absolute_five < EDGE_STRETCH_CAP);
    stretch["pass"] = baseline["pass"].clone();
    stretch["baseline"] = baseline;
    let report = json!({
        "actor": params.actor.to_string_lossy(),
        "source": params.source.to_string_lossy(),
        "written_clip": written_clip,
        "written_index": written_index,
        "skipped_scale_channels": skipped_scale,
        "correspondence": correspondence_report(&correspondence),
        "admission": {
            "required_roles": required,
            "roles_covered": covered,
            "roles_missing": missing,
            "roles_unavailable": coverage.unavailable,
            "mapped_bones": rest_admission.mapped_bones,
            "palette_bones": rest_admission.palette_bones,
            "allow_incompatible_rest": params.allow_incompatible_rest,
        },
        "gates": {
            // Not a gate. `0131`'s 5-degree pose bar assumed an absolute
            // orientation copy, which this pair cannot take (the two bone frames
            // differ by 121.91 degrees, and an absolute copy misplaces every child).
            // A rest-anchored transfer carries the rest difference by design, so the
            // number is the property of the two skeletons, not a quality score;
            // `0136` E10 already shows the bar does not reproduce.
            "pose_error": {
                "median_degrees": plan.pose_error_median_degrees,
                "max_degrees": plan.pose_error_max_degrees,
                "gate": false,
                "note": "the rest difference carried by a rest-anchored transfer; advisory",
            },
            "palette_reconstruction": reconstruction,
            "stance_foot": slide,
            "edge_stretch": stretch,
        },
    });
    if let Some(path) = &params.report {
        crate::export_glb::write_report(path, &report)?;
    }
    Ok(report)
}

/// The report's one-line verdict, for the caller that prints it.
pub fn verdict(report: &Value) -> String {
    let mut refused = Vec::new();
    // Only the entries that declare themselves a gate count: `pose_error` is an
    // advisory property of the two skeletons, not a gate (see the report note).
    for key in ["palette_reconstruction", "stance_foot", "edge_stretch"] {
        let gate = &report["gates"][key];
        if gate["gate"].as_bool() == Some(false) {
            continue;
        }
        if gate["pass"].as_bool() != Some(true) {
            refused.push(key);
        }
    }
    match refused.is_empty() {
        true => "adapted".to_owned(),
        false => format!("refused: {}", refused.join(", ")),
    }
}
