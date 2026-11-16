//! `actor havok`: decode one Skyrim Havok clip and bind its tracks by index.
//!
//! A vanilla Skyrim clip (`hk_2010.2.0-r1` `hkaSplineCompressedAnimation`) carries no
//! bone names: its tracks are ordered sample streams, and an `hkaAnimationBinding`
//! maps track index → skeleton bone index. This module decodes the compressed splines
//! to per-frame local transforms, resolves the track indices through a rig skeleton
//! (`skeleton.hkx`) to bone names, names every track without a palette target and every
//! palette bone without a track, and writes the result as a source GLB the existing
//! `actor adapt` path consumes unchanged.
//!
//! Format provenance: the container and class layouts follow the documented hk_2010
//! reader (PyNifly `docs/hkx_animation_format.md` and `anim_skyrim.py`, whose spline
//! core credits PredatorCZ/HavokLib, GPL-3.0; this engine is GPL-3.0-only per
//! DECL0006). Every offset here is additionally verified against the real vanilla
//! bytes this spike decodes. The generic `__typeinfo__` metadata is not parsed: the
//! hk_2010 layouts of the classes read here are fixed, and a version mismatch is a
//! named refusal, not a guess.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use glam::{DQuat, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use sculpto_gltf::pack::{PackWriter, Track};
use sculpto_mesh::actor::Actor;

use crate::export_glb::refused;
use crate::{CommandError, ErrorKind, errors};

/// Havok-to-metre translation factor: the measured `unit_scale` of every SSE-named
/// clip this engine exported is 71.4286, i.e. one Havok unit is exactly 0.014 m.
pub const HAVOK_METRE: f64 = 0.014;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HavokParams {
    /// `.bsa` archive that carries the clip; empty when `file` names a loose clip.
    pub archive: PathBuf,
    /// Member path inside `archive`, as the BSA indexes it
    /// (`meshes/actors/character/animations/2hm_attackpowerforward.hkx`).
    #[serde(default)]
    pub member: Option<String>,
    /// Loose clip `.hkx`, used when `archive` is empty.
    #[serde(default)]
    pub file: Option<PathBuf>,
    /// The rig the clip's track indices point into (`skeleton.hkx`).
    pub skeleton: PathBuf,
    /// Actor GLB whose skin-joint palette names the binding targets. Without it the
    /// report binds tracks to rig names only and answers neither refusal list.
    #[serde(default)]
    pub actor: Option<PathBuf>,
    /// Havok-to-metre factor applied to every decoded translation.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Source GLB to write: rig nodes at their reference pose plus the decoded clip.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// JSON report path.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

fn default_scale() -> f64 {
    HAVOK_METRE
}

impl Default for HavokParams {
    fn default() -> Self {
        Self {
            archive: PathBuf::new(),
            member: None,
            file: None,
            skeleton: PathBuf::new(),
            actor: None,
            scale: default_scale(),
            out: None,
            report: None,
        }
    }
}

/// One decoded track: per-frame local transforms, in metres and unit quaternions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SplineTrack {
    pub translations: Vec<[f64; 3]>,
    pub rotations: Vec<[f64; 4]>,
    /// The track's position mask declares data (a static or spline axis). When
    /// false the bone's local translation never leaves its rest, and a written
    /// clip must carry no translation channel for it at all — a zero channel
    /// would override the rest and collapse the skeleton into its parents.
    pub has_translation: bool,
    /// The track's rotation mask declares data (static or spline).
    pub has_rotation: bool,
    /// The raw position mask byte, reported as evidence.
    pub pos_flags: u8,
}

/// A decoded `hkaSplineCompressedAnimation`.
#[derive(Debug, Clone, Default)]
pub struct SplineClip {
    pub duration: f64,
    pub frame_duration: f64,
    pub block_duration: f64,
    pub num_frames: usize,
    pub num_tracks: usize,
    pub num_float_tracks: usize,
    pub num_blocks: usize,
    pub max_frames_per_block: usize,
    pub mask_and_quant_size: usize,
    pub class: String,
    /// The packfile's own version string (`hk_2010.2.0-r1`).
    pub version_string: String,
    pub skeleton_name: String,
    pub blend_hint: u32,
    /// Track index → rig bone index, from the binding.
    pub track_to_bone: Vec<i32>,
    /// The binding object's first twelve raw words, for the report.
    pub binding_words_head: Vec<u32>,
    /// The binding carried no track→bone map, so tracks are bound by skeleton order.
    pub identity_binding: bool,
    /// Annotation track names (vanilla leaves them empty) and their events.
    pub annotation_tracks: Vec<(String, Vec<(f64, String)>)>,
    /// Root-motion samples `(x, y, z, yaw)` when the file carries a reference frame.
    pub root_motion: Vec<[f64; 4]>,
    pub tracks: Vec<SplineTrack>,
}

impl SplineClip {
    /// The clip's own sample times, one per frame.
    pub fn times(&self) -> Vec<f64> {
        (0..self.num_frames)
            .map(|frame| frame as f64 * self.frame_duration)
            .collect()
    }
}

/// A decoded `hkaSkeleton`: bone names, parent indices and the reference pose.
#[derive(Debug, Clone, Default)]
pub struct RigSkeleton {
    pub name: String,
    pub bones: Vec<String>,
    pub parents: Vec<i32>,
    /// Per bone: local translation and rotation (`x, y, z, w`) at rest.
    pub rest: Vec<([f64; 3], [f64; 4])>,
}

/// The rig skeleton's per-bone rest, as the table a retarget needs.
///
/// The clip's channels are the *skeleton's* local rotations, so an application
/// onto another rig has to divide this rest out first
/// (`turn = pose · rest⁻¹`, or a local `d = rest⁻¹ · pose`); without it the
/// result is the `0159` class of error — a 90° gap that looks like a broken clip
/// and is really two different bases. Name, parent, translation and rotation are
/// enough to compose both sides.
fn skeleton_rest(skeleton: &RigSkeleton, scale: f64) -> Vec<Value> {
    skeleton
        .bones
        .iter()
        .enumerate()
        .map(|(bone, name)| {
            let (translation, rotation) = skeleton.rest[bone];
            // Metres, not Havok units: a consumer that composes this table with a
            // clip whose translations were already written in metres would
            // otherwise build pivots out of two different units — a rest of 22.8
            // next to a clip translation of 0.319 — and every direction measured
            // through a mixed chain comes out wrong.
            let translation = translation.map(|value| value * scale);
            json!({
                "bone": bone,
                "name": name,
                "parent": skeleton.parents[bone],
                "translation": translation,
                "rotation": rotation,
            })
        })
        .collect()
}

/// `run`: decode, bind, write, report.
pub fn run(params: &HavokParams) -> Result<Value, CommandError> {
    if params.scale <= 0.0 {
        return Err(refused(
            "havok_scale",
            format!(
                "scale {} is not a positive Havok-to-metre factor",
                params.scale
            ),
        ));
    }
    let clip = if params.archive.as_os_str().is_empty() && params.file.is_none() {
        // No clip source: inspect the rig alone.
        None
    } else {
        let bytes = source_bytes(
            &params.archive,
            params.member.as_deref(),
            params.file.as_deref(),
        )?;
        let pack = Packfile::parse(&bytes)?;
        Some(SplineClip::decode(&pack)?)
    };

    let skeleton_bytes =
        std::fs::read(&params.skeleton).map_err(|error| errors::io(&params.skeleton, error))?;
    let skeleton_pack = Packfile::parse(&skeleton_bytes)?;
    let skeleton = RigSkeleton::decode(&skeleton_pack)?;

    let Some(clip) = clip else {
        let report = json!({
            "rig_skeleton": {
                "name": skeleton.name,
                "bones": skeleton.bones.len(),
                "bone_names": skeleton.bones,
            },
            "skeleton_rest": skeleton_rest(&skeleton, params.scale),
            "skeleton_rest_units": "metres",
            "scale": params.scale,
        });
        if let Some(path) = params.report.as_deref() {
            let text = serde_json::to_string_pretty(&report)
                .map_err(|error| errors::invalid("havok_report", error.to_string()))?;
            std::fs::write(path, text).map_err(|error| errors::io(path, error))?;
        }
        if params.out.is_some() {
            return Err(refused(
                "havok_clip",
                "a source GLB needs a clip; give `archive` + `member` or `file`".to_owned(),
            ));
        }
        return Ok(report);
    };

    // Track index → rig bone name. A track whose binding index leaves the rig has no
    // name and can never reach a palette: reported, never guessed.
    let track_bones: Vec<Option<usize>> = clip
        .track_to_bone
        .iter()
        .map(|bone| {
            usize::try_from(*bone)
                .ok()
                .filter(|bone| *bone < skeleton.bones.len())
        })
        .collect();

    let (actor, actor_bytes) = match params.actor.as_deref() {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
            let actor = Actor::from_glb(&bytes).map_err(|error| {
                CommandError::new(
                    ErrorKind::InvalidRequest,
                    "havok_actor",
                    format!("{}: {error}", path.display()),
                )
            })?;
            (Some(actor), Some(bytes))
        }
        None => (None, None),
    };
    let palette: Vec<String> = match &actor {
        Some(actor) => actor
            .skin_joints()
            .map_err(|error| {
                CommandError::new(
                    ErrorKind::InvalidRequest,
                    "havok_actor",
                    format!("the actor palette cannot be read: {error}"),
                )
            })?
            .iter()
            .map(|node| actor.node_name(*node).unwrap_or("").to_owned())
            .collect(),
        None => Vec::new(),
    };
    // The same name rule the correspondence layer uses, so this report cannot
    // disagree with `actor adapt` about who maps to whom.
    // One identity per name, decided rather than overwritten: the BodySlide
    // palette carries the `CME` ghost family beside its `NPC` bones at the same
    // rest, and a plain map insert would let whichever slot came **last** win the
    // clip. That is how Mike's arm tracks bound to `CME L UpperArm` while the verb
    // aimed the `NPC` bone, and the pose silently kept our rest on the arm it was
    // supposed to drive. The rule is the correspondence layer's: prefer `NPC` over
    // a `CME` duplicate, then the shorter name, then the earlier slot.
    let mut slot_of_name: HashMap<String, usize> = HashMap::new();
    let mut palette_identity: Vec<Value> = Vec::new();
    for (slot, name) in palette.iter().enumerate() {
        let key = crate::retarget::compact(name);
        match slot_of_name.get(&key).copied() {
            None => {
                slot_of_name.insert(key, slot);
            }
            Some(current) => {
                let ghost = |slot: usize| u8::from(palette[slot].starts_with("CME "));
                let better = (ghost(slot), palette[slot].len(), slot)
                    < (ghost(current), palette[current].len(), current);
                if better {
                    palette_identity.push(json!({
                        "name": key,
                        "chosen": palette[slot],
                        "refused": palette[current],
                        "rule": "prefers the `NPC` family over a ghost `CME` duplicate, then the \
                                 shorter name, then the earlier slot",
                    }));
                    slot_of_name.insert(key, slot);
                } else {
                    palette_identity.push(json!({
                        "name": key,
                        "chosen": palette[current],
                        "refused": palette[slot],
                        "rule": "prefers the `NPC` family over a ghost `CME` duplicate, then the \
                                 shorter name, then the earlier slot",
                    }));
                }
            }
        }
    }

    let mut unbound_tracks: Vec<Value> = Vec::new();
    let mut palette_slots: Vec<Option<usize>> = Vec::with_capacity(clip.tracks.len());
    let mut rows: Vec<Value> = Vec::with_capacity(clip.tracks.len());
    for (track_index, track) in clip.tracks.iter().enumerate() {
        let bone_index = track_bones.get(track_index).copied().flatten();
        let bone_name = bone_index.map(|bone| skeleton.bones[bone].as_str());
        let slot =
            bone_name.and_then(|name| slot_of_name.get(&crate::retarget::compact(name)).copied());
        palette_slots.push(slot);
        let motion = track_motion(track, params.scale);
        if slot.is_none() {
            unbound_tracks.push(json!({
                "track": track_index,
                "bone_index": bone_index,
                "bone": bone_name,
            }));
        }
        rows.push(json!({
            "track": track_index,
            "bone_index": bone_index,
            "bone": bone_name,
            "palette_slot": slot,
            "palette_bone": slot.map(|slot| palette[slot].as_str()),
            "max_rotation_degrees": motion[0],
            "translation_range_m": motion[1],
            "pos_flags": track.pos_flags,
        }));
    }
    let mut driven = vec![false; palette.len()];
    for slot in rows.iter().filter_map(|row| row["palette_slot"].as_u64()) {
        driven[slot as usize] = true;
    }
    let undriven_palette: Vec<&str> = palette
        .iter()
        .enumerate()
        .filter(|(slot, _)| !driven[*slot])
        .map(|(_, name)| name.as_str())
        .collect();

    let mut report = json!({
        "class": clip.class,
        "container_version": clip.version_string,
        "duration_s": clip.duration,
        "frame_duration_s": clip.frame_duration,
        "block_duration_s": clip.block_duration,
        "frames": clip.num_frames,
        "tracks": clip.num_tracks,
        "float_tracks": clip.num_float_tracks,
        "blocks": clip.num_blocks,
        "max_frames_per_block": clip.max_frames_per_block,
        "mask_and_quantization_bytes": clip.mask_and_quant_size,
        "binding_skeleton_name": clip.skeleton_name,
        "binding_map": if clip.identity_binding {
            "identity: empty transformTrackToBoneIndices, tracks in skeleton order"
        } else {
            "explicit transformTrackToBoneIndices"
        },
        "binding_indices_head": clip.track_to_bone.iter().take(32).collect::<Vec<_>>(),
        "binding_words_head": clip.binding_words_head,
        "rig_skeleton": {
            "name": skeleton.name,
            "bones": skeleton.bones.len(),
        },
        "skeleton_rest": skeleton_rest(&skeleton, params.scale),
        "skeleton_rest_units": "metres",
        "blend_hint": clip.blend_hint,
        "root_motion_samples": clip.root_motion.len(),
        "annotations": clip.annotation_tracks.iter()
            .flat_map(|(track, events)| events.iter()
                .map(move |(time, text)| json!({"track": track.as_str(), "time_s": time, "text": text})))
            .collect::<Vec<_>>(),
        "track_bindings": rows,
        "unbound_tracks": unbound_tracks,
        "palette_bones": palette.len(),
        "palette_identity": palette_identity,
        "undriven_palette": undriven_palette,
        "scale": params.scale,
    });

    // The report is the evidence: write it before the GLB attempt, so a refusal
    // still leaves the numbers on disk.
    if let Some(path) = params.report.as_deref() {
        let text = serde_json::to_string_pretty(&report)
            .map_err(|error| errors::invalid("havok_report", error.to_string()))?;
        std::fs::write(path, text).map_err(|error| errors::io(path, error))?;
    }
    if let Some(path) = params.out.as_deref() {
        let clip_name = clip_stem(params);
        let (bytes, written_name) = match (&actor, &actor_bytes) {
            // With an actor, the clip lands in a copy of the actor: mesh,
            // garments and skin survive, so the artifact measures and shows.
            (Some(actor), Some(bytes)) => (
                write_actor_clip(
                    bytes,
                    actor,
                    &clip,
                    &palette_slots,
                    params.scale,
                    &clip_name,
                )?,
                clip_name,
            ),
            // Without one, the rig-only source GLB the adapt path consumes.
            _ => (
                write_source_glb(&skeleton, &clip, &track_bones, params.scale, &clip_name)?,
                clip_name,
            ),
        };
        std::fs::write(path, &bytes).map_err(|error| errors::io(path, error))?;
        report["written_clip"] = json!({
            "path": path.display().to_string(),
            "bytes": bytes.len(),
            "name": written_name,
            "mode": match actor.is_some() {
                true => "actor-bound",
                false => "rig-only source",
            },
        });
        // Rewrite with the GLB facts now that the file exists.
        if let Some(report_path) = params.report.as_deref() {
            let text = serde_json::to_string_pretty(&report)
                .map_err(|error| errors::invalid("havok_report", error.to_string()))?;
            std::fs::write(report_path, text).map_err(|error| errors::io(report_path, error))?;
        }
    }
    Ok(report)
}

/// The written clip's name: the member or file stem, so a decoded walk is
/// `1hm_walkforward` instead of the writer's old `attack` default.
fn clip_stem(params: &HavokParams) -> String {
    let stem = match params.member.as_deref() {
        Some(member) => std::path::Path::new(member)
            .file_stem()
            .and_then(|stem| stem.to_str()),
        None => params
            .file
            .as_deref()
            .and_then(|path| path.file_stem())
            .and_then(|stem| stem.to_str()),
    };
    stem.unwrap_or("havok").to_owned()
}

/// glTF LINEAR rotation keys must share a hemisphere: flip each key that dots
/// negative against its predecessor.
fn hemisphere_fixed(rotations: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let mut hemisphere = [1.0_f64; 4];
    rotations
        .iter()
        .map(|quaternion| {
            let mut q = *quaternion;
            if !q.iter().all(|value| value.is_finite()) {
                q = [0.0, 0.0, 0.0, 1.0];
            }
            if q.iter()
                .zip(hemisphere.iter())
                .map(|(a, b)| a * b)
                .sum::<f64>()
                < 0.0
            {
                q = q.map(|value| -value);
            }
            hemisphere = q;
            q
        })
        .collect()
}

/// Write the decoded clip into a copy of the actor GLB: nodes, mesh, garments
/// and skin are the actor's own, and only the animation block grows. The clip
/// takes the walk slot (index 1), the slot the viewer plays as the walk, and
/// the name is the clip stem. The first track bound to a palette slot wins; a
/// later duplicate is named in the refusal rather than silently overwriting.
fn write_actor_clip(
    actor_bytes: &[u8],
    actor: &Actor,
    clip: &SplineClip,
    palette_slots: &[Option<usize>],
    scale: f64,
    name: &str,
) -> Result<Vec<u8>, CommandError> {
    let invalid = |detail: String| errors::invalid("havok_glb", detail);
    let times = clip.times();
    if times.len() < 2 {
        return Err(invalid(format!(
            "the clip has {} frame(s); a written clip needs at least two",
            times.len()
        )));
    }
    let joints = actor.skin_joints().map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidRequest,
            "havok_actor",
            format!("the actor palette cannot be read: {error}"),
        )
    })?;
    let mut tracks: Vec<Track> = Vec::new();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    for (track_index, slot) in palette_slots.iter().enumerate() {
        let Some(slot) = slot else {
            continue;
        };
        if !seen.insert(*slot) {
            return Err(invalid(format!(
                "track {track_index} binds to palette slot {slot}, which an earlier track \
                 already drives; the decoded clip carries two tracks for one bone"
            )));
        }
        let track = &clip.tracks[track_index];
        let node = joints[*slot];
        tracks.push(Track::Rotation {
            node,
            values: hemisphere_fixed(&track.rotations)
                .iter()
                .map(|quaternion| quaternion.map(|value| value as f32))
                .collect(),
        });
        tracks.push(Track::Translation {
            node,
            values: track
                .translations
                .iter()
                .map(|translation| {
                    [
                        (translation[0] * scale) as f32,
                        (translation[1] * scale) as f32,
                        (translation[2] * scale) as f32,
                    ]
                })
                .collect(),
        });
    }
    if tracks.is_empty() {
        return Err(invalid(
            "no track binds to the actor palette, so the actor-bound GLB would carry an \
             empty clip"
                .to_owned(),
        ));
    }
    let times_f32: Vec<f32> = times.iter().map(|time| *time as f32).collect();
    let mut writer =
        PackWriter::from_glb(actor_bytes).map_err(|error| invalid(error.to_string()))?;
    writer
        .insert_animation(1, name, &times_f32, &tracks)
        .map_err(|error| invalid(error.to_string()))?;
    writer.finish().map_err(|error| invalid(error.to_string()))
}

/// Clip bytes from a BSA member or a loose file.
fn source_bytes(
    archive: &std::path::Path,
    member: Option<&str>,
    file: Option<&std::path::Path>,
) -> Result<Vec<u8>, CommandError> {
    match (archive.as_os_str().is_empty(), member) {
        (false, Some(member)) => {
            let index = sculpto_core::bsa::index_bsa(archive).map_err(errors::bsa)?;
            index.read_member(member).map_err(errors::bsa)
        }
        (false, None) => Err(refused(
            "havok_member",
            format!(
                "{} is an archive; `member` names the clip inside it",
                archive.display()
            ),
        )),
        (true, _) => {
            let path = file.ok_or_else(|| {
                refused(
                    "havok_file",
                    "either `archive` + `member` or a loose `file` is required",
                )
            })?;
            std::fs::read(path).map_err(|error| errors::io(path, error))
        }
    }
}

/// Peak rotation (degrees, first frame vs every other, hemispheres joined) and
/// translation range in metres.
fn track_motion(track: &SplineTrack, scale: f64) -> [f64; 2] {
    // Walk with hemisphere continuity so a decoded quaternion flip between frames
    // does not read as a 178-degree spin.
    let mut joined: Vec<[f64; 4]> = Vec::with_capacity(track.rotations.len());
    for q in &track.rotations {
        let q = match joined.last() {
            Some(previous)
                if previous
                    .iter()
                    .zip(q.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    < 0.0 =>
            {
                q.map(|value| -value)
            }
            _ => *q,
        };
        joined.push(q);
    }
    let Some(first) = joined.first() else {
        return [0.0, 0.0];
    };
    let first_q = DQuat::from_array(*first);
    let max_angle = joined[1..]
        .iter()
        .map(|q| first_q.angle_between(DQuat::from_array(*q)).to_degrees())
        .fold(0.0_f64, f64::max);
    let range = match track.translations.first() {
        Some(t0) => {
            track.translations[1..]
                .iter()
                .map(|t| (DVec3::from_array(*t) - DVec3::from_array(*t0)).length())
                .fold(0.0_f64, f64::max)
                * scale
        }
        None => 0.0,
    };
    [max_angle, range]
}

// ── Packfile container ────────────────────────────────────────────────────────

/// One parsed section header, offsets absolute in the file.
#[derive(Debug, Clone)]
struct Section {
    start: usize,
    local_fixups: usize,
    global_fixups: usize,
    virtual_fixups: usize,
    exports: usize,
    end: usize,
}

/// A parsed `hk_2010.2.0-r1` sectioned packfile.
#[derive(Debug)]
struct Packfile<'a> {
    data: &'a [u8],
    /// Pointer size from the header: 4 (Skyrim clips) or 8 (SE skeleton.hkx).
    ptr: usize,
    version_string: String,
    sections: HashMap<String, Section>,
    /// Data-section-relative source → destination of every local fixup.
    fixups: HashMap<usize, usize>,
    /// Data-section-relative object offsets with their class names.
    objects: Vec<(usize, String)>,
}

impl<'a> Packfile<'a> {
    /// Parse header, sections, fixups and virtual fixups.
    ///
    /// # Errors
    /// A file shorter than the header, a wrong magic or version, a pointer size
    /// that is neither 4 nor 8, or missing `__classnames__` / `__data__`.
    fn parse(bytes: &'a [u8]) -> Result<Self, CommandError> {
        let refuse = |detail: String| errors::invalid("havok_packfile", detail);
        if bytes.len() < 0x40 {
            return Err(refuse(format!(
                "{} bytes is shorter than the 0x40-byte header",
                bytes.len()
            )));
        }
        if bytes[..4] != [0x57, 0xE0, 0xE0, 0x57] {
            return Err(refuse(format!(
                "magic {:02x?} is not the Havok packfile 0x57E0E057",
                &bytes[..4]
            )));
        }
        let version = u32_le(bytes, 0x0C);
        if version != 8 {
            return Err(refuse(format!("file version {version} is not hk_2010's 8")));
        }
        let ptr = usize::from(bytes[0x10]);
        if ptr != 4 && ptr != 8 {
            return Err(refuse(format!(
                "header pointer size {} is neither 4 nor 8",
                bytes[0x10]
            )));
        }
        let version_string = String::from_utf8_lossy(
            bytes[0x28..0x38]
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or(&[]),
        )
        .to_string();
        if !version_string.starts_with("hk_2010") {
            return Err(refuse(format!(
                "version string {version_string:?} is not hk_2010"
            )));
        }
        let mut sections = HashMap::new();
        for index in 0..3_usize {
            let base = 0x40 + index * 0x30;
            let name = String::from_utf8_lossy(
                bytes[base..base + 16]
                    .split(|byte| *byte == 0)
                    .next()
                    .unwrap_or(&[]),
            )
            .to_string();
            let start = u32_le(bytes, base + 0x14) as usize;
            let section = Section {
                start,
                local_fixups: start + u32_le(bytes, base + 0x18) as usize,
                global_fixups: start + u32_le(bytes, base + 0x1C) as usize,
                virtual_fixups: start + u32_le(bytes, base + 0x20) as usize,
                exports: start + u32_le(bytes, base + 0x24) as usize,
                end: start + u32_le(bytes, base + 0x2C) as usize,
            };
            if section.end > bytes.len() {
                return Err(refuse(format!(
                    "section {name:?} ends at {} past the file size {}",
                    section.end,
                    bytes.len()
                )));
            }
            sections.insert(name, section);
        }
        let classnames = sections
            .get("__classnames__")
            .ok_or_else(|| refuse("no __classnames__ section".to_owned()))?;
        let data = sections
            .get("__data__")
            .ok_or_else(|| refuse("no __data__ section".to_owned()))?;
        let mut fixups = HashMap::new();
        let mut at = data.local_fixups;
        while at + 8 <= data.global_fixups {
            let (source, destination) = (u32_le(bytes, at), u32_le(bytes, at + 4));
            if source == u32::MAX {
                break;
            }
            fixups.insert(source as usize, destination as usize);
            at += 8;
        }
        let mut objects = Vec::new();
        let mut at = data.virtual_fixups;
        while at + 12 <= data.exports {
            let source = u32_le(bytes, at);
            let name_offset = u32_le(bytes, at + 8) as usize;
            if source == u32::MAX {
                break;
            }
            let name_at = classnames.start + name_offset;
            let name = bytes
                .get(name_at..)
                .ok_or_else(|| {
                    refuse(format!(
                        "class name offset {name_offset} leaves the classnames section"
                    ))
                })?
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or(&[]);
            objects.push((source as usize, String::from_utf8_lossy(name).to_string()));
            at += 12;
        }
        Ok(Self {
            data: bytes,
            ptr,
            version_string,
            sections,
            fixups,
            objects,
        })
    }

    /// The byte size of an `hkArray` header at this packfile's pointer size.
    fn array_size(&self) -> usize {
        self.ptr + 8
    }

    /// The padded base size of an `hkReferencedObject` at this pointer size.
    fn base_size(&self) -> usize {
        2 * self.ptr
    }

    /// Count field of an `hkArray` header at a data-section-relative offset.
    fn array_count(&self, relative: usize) -> Result<usize, CommandError> {
        let at = self.data_start() + relative + self.ptr;
        u32_at(self.data, at)
            .map(|count| count as usize)
            .ok_or_else(|| errors::invalid("havok_packfile", "array header leaves the file"))
    }

    /// Content offset (data-section-relative) of an `hkArray`, through its fixup.
    fn array_content(&self, relative: usize) -> Result<Option<usize>, CommandError> {
        if self.array_count(relative)? == 0 {
            return Ok(None);
        }
        Ok(self.fixups.get(&relative).copied())
    }

    fn data_start(&self) -> usize {
        self.sections["__data__"].start
    }

    /// A NUL-terminated string reached through the fixup of a pointer field.
    fn fixup_string(&self, relative: usize) -> Option<String> {
        let target = self.fixups.get(&relative).copied()?;
        let at = self.data_start() + target;
        let bytes = self.data.get(at..)?;
        Some(
            String::from_utf8_lossy(bytes.split(|byte| *byte == 0).next().unwrap_or(&[]))
                .to_string(),
        )
    }
}

fn u32_le(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Bounds-checked `u32` read.
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    bytes
        .get(at..end)
        .map(|slice| u32::from_le_bytes(slice.try_into().expect("4 bytes")))
}

/// Bounds-checked `f32` read.
fn f32_at(bytes: &[u8], at: usize) -> Option<f32> {
    u32_at(bytes, at).map(f32::from_bits)
}

/// Bounds-checked run of `N` floats, 4 bytes apart.
fn f32_array<const N: usize>(bytes: &[u8], at: usize) -> Option<[f32; N]> {
    (0..N)
        .map(|axis| f32_at(bytes, at + axis * 4))
        .collect::<Option<Vec<_>>>()
        .map(|values| values.try_into().expect("declared length"))
}

// ── Animation decode ─────────────────────────────────────────────────────────

impl SplineClip {
    /// Decode the animation object of a packfile.
    ///
    /// # Errors
    /// No animation class, an unsupported animation class (`hkaLosslessCompressedAnimation`
    /// and anything unknown are named), no binding, no blocks or no data blob, or a
    /// malformed track stream.
    fn decode(pack: &Packfile<'_>) -> Result<Self, CommandError> {
        let refuse = |code: &'static str, detail: String| {
            CommandError::new(ErrorKind::Unsupported, code, detail)
        };
        let (anim_rel, class) = pack
            .objects
            .iter()
            .find(|(_, class)| {
                matches!(
                    class.as_str(),
                    "hkaSplineCompressedAnimation"
                        | "hkaInterleavedUncompressedAnimation"
                        | "hkaLosslessCompressedAnimation"
                )
            })
            .map(|(rel, class)| (*rel, class.clone()))
            .ok_or_else(|| {
                refuse(
                    "havok_animation",
                    format!(
                        "none of hkaSplineCompressedAnimation, \
                         hkaInterleavedUncompressedAnimation or hkaLosslessCompressedAnimation \
                         is instantiated; classes: {:?}",
                        pack.objects
                            .iter()
                            .map(|(_, class)| class.as_str())
                            .collect::<Vec<_>>()
                    ),
                )
            })?;
        if class != "hkaSplineCompressedAnimation" {
            return Err(refuse(
                "havok_animation",
                format!(
                    "{class} is not spline-compressed; this decoder reads only \
                     hkaSplineCompressedAnimation"
                ),
            ));
        }

        let base = pack.base_size();
        let arr = pack.array_size();
        let data_start = pack.data_start();
        let object_at = data_start + anim_rel;

        let read_u32 = |offset: usize, what: &'static str| {
            u32_at(pack.data, object_at + offset)
                .map(|value| value as usize)
                .ok_or_else(|| {
                    errors::invalid("havok_animation", format!("{what} leaves the file"))
                })
        };
        let read_f32 = |offset: usize, what: &'static str| {
            f32_at(pack.data, object_at + offset)
                .map(f64::from)
                .ok_or_else(|| {
                    errors::invalid("havok_animation", format!("{what} leaves the file"))
                })
        };

        let duration = read_f32(base + 4, "duration")?;
        let num_tracks = read_u32(base + 8, "track count")?;
        let num_float_tracks = read_u32(base + 12, "float track count")?;
        let spline_base = base + 16 + pack.ptr + arr;
        let num_frames = read_u32(spline_base, "frame count")?;
        let num_blocks = read_u32(spline_base + 4, "block count")?;
        let max_frames_per_block = read_u32(spline_base + 8, "max frames per block")?;
        let mask_and_quant_size = read_u32(spline_base + 12, "mask size")?;
        let block_duration = read_f32(spline_base + 16, "block duration")?;
        let frame_duration = read_f32(spline_base + 24, "frame duration")?;
        let block_offsets_at = anim_rel + align_up(spline_base + 28, pack.ptr);
        let block_offsets = read_u32_array(pack, block_offsets_at)?;
        let blob = read_u8_array(pack, block_offsets_at + 4 * arr)?;

        if num_tracks == 0 {
            return Err(refuse(
                "havok_tracks",
                "the animation declares zero transform tracks".to_owned(),
            ));
        }
        if num_frames == 0 || num_blocks == 0 || block_offsets.is_empty() || blob.is_empty() {
            return Err(refuse(
                "havok_frames",
                format!(
                    "degenerate spline payload: {num_frames} frames, {num_blocks} blocks, \
                     {} block offsets, {} data bytes",
                    block_offsets.len(),
                    blob.len()
                ),
            ));
        }

        let mut clip = Self {
            duration,
            frame_duration,
            block_duration,
            num_frames,
            num_tracks,
            num_float_tracks,
            num_blocks,
            max_frames_per_block,
            mask_and_quant_size,
            class,
            version_string: pack.version_string.clone(),
            tracks: vec![SplineTrack::default(); num_tracks],
            ..Self::default()
        };

        // Annotation tracks: vanilla names are empty; the events are the reported facts.
        let ann_tracks_rel = anim_rel + base + 16 + pack.ptr;
        let ann_stride = pack.ptr + arr;
        if let Some(content) = pack.array_content(ann_tracks_rel)? {
            for index in 0..pack.array_count(ann_tracks_rel)? {
                let track_rel = content + index * ann_stride;
                let name = pack.fixup_string(track_rel).unwrap_or_default();
                let events_rel = track_rel + pack.ptr;
                let mut events = Vec::new();
                if let Some(event_content) = pack.array_content(events_rel)? {
                    let event_stride = 2 * pack.ptr;
                    for event in 0..pack.array_count(events_rel)? {
                        let event_rel = event_content + event * event_stride;
                        let time = f32_at(pack.data, data_start + event_rel).unwrap_or_default();
                        let text = pack.fixup_string(event_rel + pack.ptr).unwrap_or_default();
                        events.push((f64::from(time), text));
                    }
                }
                clip.annotation_tracks.push((name, events));
            }
        }

        // Binding: skeleton name, track → bone indices, blend hint.
        let binding_rel = pack
            .objects
            .iter()
            .find(|(_, class)| class == "hkaAnimationBinding")
            .map(|(rel, _)| *rel)
            .ok_or_else(|| {
                refuse(
                    "havok_binding",
                    "no hkaAnimationBinding: the tracks cannot be bound by index".to_owned(),
                )
            })?;
        let bind_base = pack.base_size();
        clip.skeleton_name = pack
            .fixup_string(binding_rel + bind_base)
            .unwrap_or_default();
        // Raw words of the binding object head, reported verbatim: the layout is
        // checked against the real file, not assumed.
        clip.binding_words_head = (0..12)
            .filter_map(|word| u32_at(pack.data, data_start + binding_rel + word * 4))
            .collect();
        let indices_rel = binding_rel + bind_base + 2 * pack.ptr;
        if let Some(content) = pack.array_content(indices_rel)? {
            let count = pack.array_count(indices_rel)?;
            for index in 0..count {
                let at = data_start + content + index * 2;
                let raw = pack.data.get(at..at + 2).ok_or_else(|| {
                    errors::invalid("havok_binding", "binding indices leave the file")
                })?;
                clip.track_to_bone
                    .push(i16::from_le_bytes([raw[0], raw[1]]) as i32);
            }
        }
        if clip.track_to_bone.is_empty() {
            // The Havok convention for an empty `transformTrackToBoneIndices`: the
            // tracks are in the skeleton's own order. Vanilla Skyrim clips bind this
            // way — the skeleton name is the only map — and 0133 recorded exactly
            // that ("tracks bound by index to NPC Root [Root]").
            clip.identity_binding = true;
            clip.track_to_bone = (0..num_tracks as i32).collect();
        }
        clip.blend_hint = u32_at(pack.data, data_start + indices_rel + 2 * arr).unwrap_or_default();

        // Root motion, when the file carries a reference frame (dragon flight only).
        if let Some((rf_rel, _)) = pack
            .objects
            .iter()
            .find(|(_, class)| class == "hkaDefaultAnimatedReferenceFrame")
        {
            let samples_rel = rf_rel + if pack.ptr == 4 { 0x34 } else { 0x38 };
            if let Some(content) = pack.array_content(samples_rel)? {
                for sample in 0..pack.array_count(samples_rel)? {
                    let at = data_start + content + sample * 16;
                    if let Some(values) = f32_array::<4>(pack.data, at) {
                        clip.root_motion.push(values.map(f64::from));
                    }
                }
            }
        }

        decode_blocks(&blob, &mut clip, &block_offsets)?;
        Ok(clip)
    }
}

/// Expand every block's splines into `clip.tracks`.
fn decode_blocks(
    blob: &[u8],
    clip: &mut SplineClip,
    block_offsets: &[u32],
) -> Result<(), CommandError> {
    let refuse = |detail: String| errors::invalid("havok_spline", detail);
    for (block_index, offset) in block_offsets.iter().enumerate() {
        let block_start = *offset as usize;
        if block_start >= blob.len() {
            return Err(refuse(format!(
                "block {block_index} starts at {block_start} past the {}-byte blob",
                blob.len()
            )));
        }
        // Blocks overlap by one frame: block b re-encodes frame b*(mfpb-1), so a later
        // block's first frame is dropped, not appended (the reference reader's rule,
        // measured against multi-block vanilla animations).
        let first_frame = block_index * clip.max_frames_per_block.saturating_sub(1);
        let frames_in_block = clip.max_frames_per_block.min(clip.num_frames - first_frame);
        let skip = usize::from(block_index > 0);
        if frames_in_block == 0 {
            continue;
        }
        let mask_size = if clip.mask_and_quant_size > 0 {
            clip.mask_and_quant_size
        } else {
            align_up(4 * clip.num_tracks, 4)
        };
        let mut masks = Vec::with_capacity(clip.num_tracks);
        for track in 0..clip.num_tracks {
            let at = block_start + track * 4;
            let bytes = blob
                .get(at..at + 4)
                .ok_or_else(|| refuse("track masks leave the block".to_owned()))?;
            masks.push(TrackMask::new(bytes[0], bytes[1], bytes[2], bytes[3]));
        }
        let mut cursor = block_start + mask_size;
        let mapped = |error: String| errors::invalid("havok_spline", error);
        for (track_index, mask) in masks.iter().enumerate() {
            let track = &mut clip.tracks[track_index];
            let translations =
                decode_position(blob, &mut cursor, mask, frames_in_block).map_err(mapped)?;
            let rotations =
                decode_rotation(blob, &mut cursor, mask, frames_in_block).map_err(mapped)?;
            decode_scale(blob, &mut cursor, mask, frames_in_block).map_err(mapped)?;
            cursor = align_up(cursor, 4);
            track.pos_flags = mask.pos_flags;
            track.has_translation = mask.pos_flags != 0;
            track.has_rotation = mask.rot_flags != 0;
            track
                .translations
                .extend(translations.into_iter().skip(skip));
            track.rotations.extend(rotations.into_iter().skip(skip));
        }
    }
    let short: Vec<usize> = clip
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, track)| track.rotations.len() != clip.num_frames)
        .map(|(index, _)| index)
        .collect();
    if !short.is_empty() {
        return Err(refuse(format!(
            "tracks {short:?} decoded to a frame count other than {}",
            clip.num_frames
        )));
    }
    // A declared translation channel that decodes to exact zeros after the first
    // frame is the silent-corruption signature (0152: the vanilla walk's pelvis
    // sat at its rest pose on frame 0 and collapsed into the origin afterwards).
    // Real data may key a static zero, but then every frame is zero *including*
    // the first; first-frame-correct and zero-afterwards is a decoder fault or a
    // corrupt block, and either way refusing beats printing a collapse.
    let dead: Vec<usize> = clip
        .tracks
        .iter()
        .enumerate()
        .filter(|(index, track)| {
            track.rotations.len() >= 2
                && track.translations.len() == track.rotations.len()
                && track.translations[1..]
                    .iter()
                    .all(|t| t == &[0.0, 0.0, 0.0])
                && track.translations[0] != [0.0, 0.0, 0.0]
                && masks_at(blob, clip, block_offsets, *index)
                    .is_some_and(|mask| mask.pos_flags != 0)
        })
        .map(|(index, _)| index)
        .collect();
    if !dead.is_empty() {
        return Err(refuse(format!(
            "translation tracks {dead:?} decode to exact zeros after frame 0 while their \
             masks declare data: the block expansion is misread or the clip is corrupt"
        )));
    }
    Ok(())
}

/// The position mask of one track in the first block, for the zero-track gate.
fn masks_at(
    blob: &[u8],
    clip: &SplineClip,
    block_offsets: &[u32],
    track_index: usize,
) -> Option<TrackMask> {
    let block_start = *block_offsets.first()? as usize;
    let mask_size = if clip.mask_and_quant_size > 0 {
        clip.mask_and_quant_size
    } else {
        align_up(4 * clip.num_tracks, 4)
    };
    let at = block_start + track_index * 4;
    let bytes = blob.get(at..at + 4)?;
    (at < block_start + mask_size).then(|| TrackMask::new(bytes[0], bytes[1], bytes[2], bytes[3]))
}

/// One axis' storage class with its static value or quantization range.
#[derive(Debug, Clone, Copy)]
struct AxisChannel {
    kind: ChannelType,
    /// Static value, or `min` of the quantization range; `max` rides along.
    min: f64,
    max: f64,
}

/// The per-axis storage classes, read in axis order (spline min/max, static values).
fn read_axis_channels(
    blob: &[u8],
    cursor: &mut usize,
    kinds: [ChannelType; 3],
    identity: f64,
) -> Result<[AxisChannel; 3], String> {
    let mut channels = [AxisChannel::identity(identity); 3];
    for (axis, channel) in channels.iter_mut().enumerate() {
        channel.kind = kinds[axis];
        match kinds[axis] {
            ChannelType::Spline => {
                let min = read_scalar(blob, cursor)?;
                let max = read_scalar(blob, cursor)?;
                // A declared spline with a degenerate range cannot carry motion:
                // every quantized control point decodes to the same value. That is
                // the signature of misaligned reads (it silently produced exact
                // zeros across the vanilla walk), so it is a refusal, not data.
                if min == max {
                    return Err(format!(
                        "spline axis {axis} declares a degenerate quantization range \
                         (min = max = {min}); the block is misread or corrupt"
                    ));
                }
                channel.min = min;
                channel.max = max;
            }
            ChannelType::Static => {
                let value = read_scalar(blob, cursor)?;
                channel.min = value;
                channel.max = value;
            }
            ChannelType::Identity => {}
        }
    }
    Ok(channels)
}

/// Decode one track's translation channel into `frames` values.
fn decode_position(
    blob: &[u8],
    cursor: &mut usize,
    mask: &TrackMask,
    frames: usize,
) -> Result<Vec<[f64; 3]>, String> {
    let kinds = [mask.pos_type(0), mask.pos_type(1), mask.pos_type(2)];
    if kinds.contains(&ChannelType::Spline) {
        let knots = read_spline_header(blob, cursor)?;
        // The knot vector is byte-sized and ends anywhere: the reference reader
        // aligns to 4 before the min/max floats. Skipping this read every min/max
        // (and therefore every control point) up to 3 bytes off — the vanilla
        // walk decoded as exact zeros because the misaligned pair landed on
        // padding.
        *cursor = align_up(*cursor, 4);
        let channels = read_axis_channels(blob, cursor, kinds, 0.0)?;
        let mut points = Vec::with_capacity(knots.num_points);
        for _ in 0..knots.num_points {
            let mut point = [0.0; 3];
            for (axis, channel) in channels.iter().enumerate() {
                point[axis] = match channel.kind {
                    ChannelType::Spline => {
                        let bits = if mask.pos_quant == 0 { 8 } else { 16 };
                        read_quant_scalar(blob, cursor, channel.min, channel.max, bits)?
                    }
                    _ => channel.static_or_identity(channel.min),
                };
            }
            points.push(point);
        }
        *cursor = align_up(*cursor, 4);
        Ok(evaluate_spline(&knots, &points, frames))
    } else {
        let channels = read_axis_channels(blob, cursor, kinds, 0.0)?;
        let value =
            std::array::from_fn(|axis| channels[axis].static_or_identity(channels[axis].min));
        Ok(vec![value; frames])
    }
}

/// Decode one track's rotation channel into `frames` quaternions (`x, y, z, w`).
fn decode_rotation(
    blob: &[u8],
    cursor: &mut usize,
    mask: &TrackMask,
    frames: usize,
) -> Result<Vec<[f64; 4]>, String> {
    match mask.rot_type() {
        ChannelType::Identity => Ok(vec![[0.0, 0.0, 0.0, 1.0]; frames]),
        ChannelType::Static => {
            *cursor = align_up(*cursor, mask.rot_align());
            let q = read_quat(blob, cursor, mask.rot_quant)?;
            Ok(vec![q; frames])
        }
        ChannelType::Spline => {
            let knots = read_spline_header(blob, cursor)?;
            *cursor = align_up(*cursor, mask.rot_align());
            let mut points: Vec<[f64; 4]> = Vec::with_capacity(knots.num_points);
            for _ in 0..knots.num_points {
                let q = read_quat(blob, cursor, mask.rot_quant)?;
                // Keep neighbouring control points in one hemisphere so the spline
                // between them interpolates the short arc.
                let continuous = match points.last() {
                    Some(previous)
                        if previous
                            .iter()
                            .zip(q.iter())
                            .map(|(a, b)| a * b)
                            .sum::<f64>()
                            < 0.0 =>
                    {
                        q.map(|value| -value)
                    }
                    _ => q,
                };
                points.push(continuous);
            }
            let interpolated = evaluate_spline(&knots, &points, frames);
            Ok(interpolated.iter().map(|q| normalize_quat(*q)).collect())
        }
    }
}

/// Decode one track's scale channel into `frames` values.
fn decode_scale(
    blob: &[u8],
    cursor: &mut usize,
    mask: &TrackMask,
    frames: usize,
) -> Result<Vec<[f64; 3]>, String> {
    let kinds = [mask.scale_type(0), mask.scale_type(1), mask.scale_type(2)];
    if kinds.contains(&ChannelType::Spline) {
        let knots = read_spline_header(blob, cursor)?;
        // Same alignment as the position spline: floats start on a 4-byte boundary
        // after the byte-sized knot vector.
        *cursor = align_up(*cursor, 4);
        let channels = read_axis_channels(blob, cursor, kinds, 1.0)?;
        let mut points = Vec::with_capacity(knots.num_points);
        for _ in 0..knots.num_points {
            let mut point = [1.0; 3];
            for (axis, channel) in channels.iter().enumerate() {
                point[axis] = match channel.kind {
                    ChannelType::Spline => {
                        let bits = if mask.scale_quant == 0 { 8 } else { 16 };
                        read_quant_scalar(blob, cursor, channel.min, channel.max, bits)?
                    }
                    _ => channel.static_or_identity(channel.min),
                };
            }
            points.push(point);
        }
        *cursor = align_up(*cursor, 4);
        Ok(evaluate_spline(&knots, &points, frames))
    } else {
        let channels = read_axis_channels(blob, cursor, kinds, 1.0)?;
        let value =
            std::array::from_fn(|axis| channels[axis].static_or_identity(channels[axis].min));
        Ok(vec![value; frames])
    }
}

fn read_scalar(blob: &[u8], cursor: &mut usize) -> Result<f64, String> {
    let value = f32_at(blob, *cursor).ok_or_else(|| "float leaves the block".to_owned())?;
    *cursor += 4;
    Ok(f64::from(value))
}

fn read_quant_scalar(
    blob: &[u8],
    cursor: &mut usize,
    min: f64,
    max: f64,
    bits: u8,
) -> Result<f64, String> {
    if bits == 8 {
        let raw = *blob
            .get(*cursor)
            .ok_or_else(|| "8-bit sample leaves the block".to_owned())?;
        *cursor += 1;
        Ok(min + (max - min) * (f64::from(raw) / 255.0))
    } else {
        let raw = blob
            .get(*cursor..*cursor + 2)
            .map(|slice| u16::from_le_bytes(slice.try_into().expect("2 bytes")))
            .ok_or_else(|| "16-bit sample leaves the block".to_owned())?;
        *cursor += 2;
        Ok(min + (max - min) * (f64::from(raw) / 65535.0))
    }
}

/// Parsed spline header: control-point count, degree and the knot vector.
#[derive(Debug, Clone)]
struct SplineHeader {
    num_points: usize,
    degree: usize,
    knots: Vec<f64>,
}

/// `numItems u16, degree u8, knots[numItems + degree + 2] u8`.
fn read_spline_header(blob: &[u8], cursor: &mut usize) -> Result<SplineHeader, String> {
    let bytes = blob
        .get(*cursor..*cursor + 3)
        .ok_or_else(|| "spline header leaves the block".to_owned())?;
    let num_points = usize::from(u16::from_le_bytes([bytes[0], bytes[1]])) + 1;
    let degree = usize::from(bytes[2]);
    *cursor += 3;
    let num_knots = num_points + degree + 1;
    let knots = blob
        .get(*cursor..*cursor + num_knots)
        .ok_or_else(|| "knot vector leaves the block".to_owned())?
        .iter()
        .map(|knot| f64::from(*knot))
        .collect::<Vec<_>>();
    *cursor += num_knots;
    Ok(SplineHeader {
        num_points,
        degree,
        knots,
    })
}

/// Byte alignment a rotation of this quantization code needs (`0=32-bit, 1=40, 2=48, 5=f32`).
const QUAT_ALIGN: [usize; 6] = [4, 1, 2, 1, 2, 4];

/// One quantized quaternion, `x, y, z, w`, normalized.
fn read_quat(blob: &[u8], cursor: &mut usize, format: usize) -> Result<[f64; 4], String> {
    let q = match format {
        0 => {
            let raw = blob
                .get(*cursor..*cursor + 4)
                .map(|slice| u32::from_le_bytes(slice.try_into().expect("4 bytes")))
                .ok_or_else(|| "32-bit quaternion leaves the block".to_owned())?;
            *cursor += 4;
            quat32(raw)
        }
        1 => {
            let raw = blob
                .get(*cursor..*cursor + 5)
                .map(|slice| {
                    let mut bytes = [0u8; 8];
                    bytes[..5].copy_from_slice(slice);
                    u64::from_le_bytes(bytes)
                })
                .ok_or_else(|| "40-bit quaternion leaves the block".to_owned())?;
            *cursor += 5;
            quat40(raw)
        }
        2 => {
            let raw = blob
                .get(*cursor..*cursor + 6)
                .map(|slice| {
                    let mut bytes = [0u8; 8];
                    bytes[..6].copy_from_slice(slice);
                    u64::from_le_bytes(bytes)
                })
                .ok_or_else(|| "48-bit quaternion leaves the block".to_owned())?;
            *cursor += 6;
            quat48(raw)
        }
        5 => {
            let values = f32_array::<4>(blob, *cursor)
                .ok_or_else(|| "uncompressed quaternion leaves the block".to_owned())?;
            *cursor += 16;
            values.map(f64::from)
        }
        other => return Err(format!("unsupported rotation quantization format {other}")),
    };
    Ok(normalize_quat(q))
}

/// 32-bit quantization: a sphere parameterization `(w, phi, theta)` with per-component
/// sign bits 28..31.
fn quat32(raw: u32) -> [f64; 4] {
    let w = 1.0 - (f64::from((raw >> 18) & 0x3FF) / 1023.0).powi(2);
    let phi_theta = f64::from(raw & 0x3_FFFF);
    let mut phi = phi_theta.sqrt().floor();
    let mut theta = 0.0;
    if phi > 0.0 {
        theta = std::f64::consts::FRAC_PI_4 * (phi_theta - phi * phi) / phi;
        phi *= std::f64::consts::FRAC_PI_2 / 511.0;
    }
    let magnitude = (1.0 - w * w).max(0.0).sqrt();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let (sin_theta, cos_theta) = theta.sin_cos();
    let mut result = [
        sin_phi * cos_theta * magnitude,
        sin_phi * sin_theta * magnitude,
        cos_phi * magnitude,
        w,
    ];
    for (index, mask) in [0x1000_0000u32, 0x2000_0000, 0x4000_0000, 0x8000_0000]
        .into_iter()
        .enumerate()
    {
        if raw & mask != 0 {
            result[index] = -result[index];
        }
    }
    result
}

/// 40-bit three-component quantization: 3 × 12 bits + shift + sign.
fn quat40(raw: u64) -> [f64; 4] {
    const FRACTION: f64 = 0.000_345_436;
    let components = [
        f64::from((raw & 0xFFF) as u16),
        f64::from(((raw >> 12) & 0xFFF) as u16),
        f64::from(((raw >> 24) & 0xFFF) as u16),
    ]
    .map(|value| (value - 2049.0) * FRACTION);
    let mut w = (1.0 - components.iter().map(|v| v * v).sum::<f64>().min(1.0)).sqrt();
    if (raw >> 38) & 1 != 0 {
        w = -w;
    }
    insert_fourth(components, w, (raw >> 36) & 3)
}

/// 48-bit three-component quantization: 3 × 15 bits + shift + sign.
fn quat48(raw: u64) -> [f64; 4] {
    const FRACTION: f64 = 0.000_043_161;
    let x_raw = (raw & 0xFFFF) as u16;
    let y_raw = ((raw >> 16) & 0xFFFF) as u16;
    let z_raw = ((raw >> 32) & 0xFFFF) as u16;
    let shift = u64::from((y_raw >> 14) & 2) | (u64::from(x_raw >> 15) & 1);
    let sign = (z_raw >> 15) & 1 == 1;
    let components = [x_raw, y_raw, z_raw]
        .map(|value| f64::from(value & 0x7FFF))
        .map(|value| (value - 16_383.0) * FRACTION);
    let mut w = (1.0 - components.iter().map(|v| v * v).sum::<f64>().min(1.0)).sqrt();
    if sign {
        w = -w;
    }
    insert_fourth(components, w, shift)
}

/// Place the reconstructed fourth component at `shift` (`0=x, 1=y, 2=z, 3=w`).
fn insert_fourth(components: [f64; 3], fourth: f64, shift: u64) -> [f64; 4] {
    match shift {
        0 => [fourth, components[0], components[1], components[2]],
        1 => [components[0], fourth, components[1], components[2]],
        2 => [components[0], components[1], fourth, components[2]],
        _ => [components[0], components[1], components[2], fourth],
    }
}

fn normalize_quat(q: [f64; 4]) -> [f64; 4] {
    let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    q.map(|v| v / length)
}

/// Componentwise weighted-spline arithmetic over the three point shapes.
trait Weighted: Copy + Default {
    fn scaled(self, weight: f64) -> Self;
    fn plus(self, other: Self) -> Self;
}

impl Weighted for f64 {
    fn scaled(self, weight: f64) -> Self {
        self * weight
    }
    fn plus(self, other: Self) -> Self {
        self + other
    }
}

impl Weighted for [f64; 3] {
    fn scaled(self, weight: f64) -> Self {
        self.map(|value| value * weight)
    }
    fn plus(self, other: Self) -> Self {
        std::array::from_fn(|axis| self[axis] + other[axis])
    }
}

impl Weighted for [f64; 4] {
    fn scaled(self, weight: f64) -> Self {
        self.map(|value| value * weight)
    }
    fn plus(self, other: Self) -> Self {
        std::array::from_fn(|axis| self[axis] + other[axis])
    }
}

/// Evaluate the spline at `frames` consecutive integer knots, one value per frame.
///
/// The knot vector is in frame units, so frame f evaluates at `t = f`.
fn evaluate_spline<T: Weighted>(header: &SplineHeader, points: &[T], frames: usize) -> Vec<T> {
    (0..frames)
        .map(|frame| {
            let t = frame as f64;
            let span = knot_span(header.degree, t, header.num_points, &header.knots);
            let weights = de_boor_weights(header.degree, span, t, &header.knots);
            blend(&weights, span, points)
        })
        .collect()
}

/// The knot span de Boor's recursion starts from.
fn knot_span(degree: usize, t: f64, num_points: usize, knots: &[f64]) -> usize {
    if num_points == 0 || knots.is_empty() {
        return 0;
    }
    if t >= knots[num_points.min(knots.len() - 1)] {
        return num_points.saturating_sub(1);
    }
    let (mut low, mut high) = (degree, num_points);
    let mut mid = (low + high) / 2;
    for _ in 0..100 {
        if mid >= knots.len() {
            break;
        }
        if t < knots[mid] {
            high = mid;
        } else if mid + 1 < knots.len() && t >= knots[mid + 1] {
            low = mid;
        } else {
            break;
        }
        if low >= high {
            break;
        }
        mid = (low + high) / 2;
    }
    mid.min(num_points.saturating_sub(1))
}

/// De Boor's basis weights over `degree + 1` control points at `span`.
fn de_boor_weights(degree: usize, span: usize, t: f64, knots: &[f64]) -> Vec<f64> {
    let mut basis = vec![0.0; degree + 1];
    basis[0] = 1.0;
    for level in 1..=degree {
        for position in (0..level).rev() {
            let left = knots.get(span + level - position).copied().unwrap_or(0.0);
            let right = knots.get(span - position).copied().unwrap_or(0.0);
            let denominator = left - right;
            let alpha = if denominator >= 1e-10 {
                ((t - right) / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // The weight splits: `alpha` moves right, the rest stays.
            let kept = basis[position];
            let moved = kept * alpha;
            basis[position + 1] += kept - moved;
            basis[position] = moved;
        }
    }
    basis
}

/// The weighted sum of `weights[i] * points[span - i]`.
fn blend<T: Weighted>(weights: &[f64], span: usize, points: &[T]) -> T {
    let mut result = T::default();
    for (offset, weight) in weights.iter().enumerate() {
        if *weight == 0.0 || span < offset {
            continue;
        }
        if let Some(point) = points.get(span - offset) {
            result = result.plus(point.scaled(*weight));
        }
    }
    result
}

/// `hkArray<u32>` content through its fixup.
fn read_u32_array(pack: &Packfile<'_>, relative: usize) -> Result<Vec<u32>, CommandError> {
    let Some(content) = pack.array_content(relative)? else {
        return Ok(Vec::new());
    };
    let count = pack.array_count(relative)?;
    let at = pack.data_start() + content;
    (0..count)
        .map(|index| {
            u32_at(pack.data, at + index * 4)
                .ok_or_else(|| errors::invalid("havok_animation", "array content leaves the file"))
        })
        .collect()
}

/// `hkArray<u8>` content through its fixup.
fn read_u8_array(pack: &Packfile<'_>, relative: usize) -> Result<Vec<u8>, CommandError> {
    let Some(content) = pack.array_content(relative)? else {
        return Ok(Vec::new());
    };
    let count = pack.array_count(relative)?;
    let at = pack.data_start() + content;
    pack.data
        .get(at..at + count)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| errors::invalid("havok_animation", "data blob leaves the file"))
}

fn align_up(value: usize, alignment: usize) -> usize {
    if alignment <= 1 {
        return value;
    }
    let remainder = value % alignment;
    if remainder == 0 {
        value
    } else {
        value + alignment - remainder
    }
}

// ── Skeleton decode ──────────────────────────────────────────────────────────

impl RigSkeleton {
    /// Decode the first `hkaSkeleton` of a packfile.
    ///
    /// # Errors
    /// No `hkaSkeleton`, or truncated name/parent/pose arrays.
    fn decode(pack: &Packfile<'_>) -> Result<Self, CommandError> {
        let rel = pack
            .objects
            .iter()
            .find(|(_, class)| class == "hkaSkeleton")
            .map(|(rel, _)| *rel)
            .ok_or_else(|| {
                CommandError::new(
                    ErrorKind::Unsupported,
                    "havok_skeleton",
                    format!(
                        "no hkaSkeleton in the rig; classes: {:?}",
                        pack.objects
                            .iter()
                            .map(|(_, class)| class.as_str())
                            .collect::<Vec<_>>()
                    ),
                )
            })?;
        let base = pack.base_size();
        let arr = pack.array_size();
        let data_start = pack.data_start();
        let mut skeleton = Self {
            name: pack.fixup_string(rel + base).unwrap_or_default(),
            ..Self::default()
        };

        let parents_rel = rel + base + pack.ptr;
        let bones_rel = parents_rel + arr;
        let pose_rel = bones_rel + arr;

        if let Some(content) = pack.array_content(parents_rel)? {
            let count = pack.array_count(parents_rel)?;
            for index in 0..count {
                let at = data_start + content + index * 2;
                let raw = pack.data.get(at..at + 2).ok_or_else(|| {
                    errors::invalid("havok_skeleton", "parent indices leave the file")
                })?;
                skeleton
                    .parents
                    .push(i16::from_le_bytes([raw[0], raw[1]]) as i32);
            }
        }

        // `hkaBone` is a name pointer + `lockTranslation`, aligned to the pointer size.
        let bone_stride = if pack.ptr == 4 { pack.ptr + 4 } else { 16 };
        if let Some(content) = pack.array_content(bones_rel)? {
            let count = pack.array_count(bones_rel)?;
            for index in 0..count {
                let bone_rel = content + index * bone_stride;
                skeleton
                    .bones
                    .push(pack.fixup_string(bone_rel).unwrap_or_default());
            }
        }

        // `hkQsTransform` is 0x30 bytes at any pointer size.
        if let Some(content) = pack.array_content(pose_rel)? {
            let count = pack.array_count(pose_rel)?;
            for index in 0..count {
                let at = data_start + content + index * 0x30;
                match (
                    f32_array::<3>(pack.data, at),
                    f32_array::<4>(pack.data, at + 0x10),
                ) {
                    (Some(translation), Some(rotation)) => skeleton
                        .rest
                        .push((translation.map(f64::from), rotation.map(f64::from))),
                    _ => {
                        return Err(errors::invalid(
                            "havok_skeleton",
                            "reference pose leaves the file",
                        ));
                    }
                }
            }
        }
        if skeleton.bones.is_empty() || skeleton.rest.len() != skeleton.bones.len() {
            return Err(errors::invalid(
                "havok_skeleton",
                format!(
                    "the rig has {} bone names and {} reference poses",
                    skeleton.bones.len(),
                    skeleton.rest.len()
                ),
            ));
        }
        Ok(skeleton)
    }
}

// ── Track masks ──────────────────────────────────────────────────────────────

/// How one channel is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ChannelType {
    /// No data stored: identity (translation 0, rotation 0/0/0/1, scale 1).
    #[default]
    Identity,
    /// One stored value, constant across the block.
    Static,
    /// A B-spline with quantized control points.
    Spline,
}

impl AxisChannel {
    fn identity(default: f64) -> Self {
        Self {
            kind: ChannelType::Identity,
            min: default,
            max: default,
        }
    }

    /// The stored static value, or the axis' identity when nothing is stored.
    fn static_or_identity(&self, fallback: f64) -> f64 {
        match self.kind {
            ChannelType::Static => self.min,
            _ => fallback,
        }
    }
}

/// The four mask bytes that describe one track's channels.
#[derive(Debug, Clone, Copy, Default)]
struct TrackMask {
    pos_quant: usize,
    rot_quant: usize,
    scale_quant: usize,
    pos_flags: u8,
    rot_flags: u8,
    scale_flags: u8,
}

impl TrackMask {
    fn new(b0: u8, b1: u8, b2: u8, b3: u8) -> Self {
        Self {
            pos_quant: usize::from(b0 & 0x03),
            rot_quant: usize::from((b0 >> 2) & 0x0F),
            scale_quant: usize::from(b0 >> 6),
            pos_flags: b1,
            rot_flags: b2,
            scale_flags: b3,
        }
    }

    fn pos_type(&self, axis: usize) -> ChannelType {
        axis_type(axis, self.pos_flags)
    }

    fn scale_type(&self, axis: usize) -> ChannelType {
        axis_type(axis, self.scale_flags)
    }

    /// The rotation is one channel: low nibble static, high nibble spline.
    fn rot_type(&self) -> ChannelType {
        if self.rot_flags >> 4 != 0 {
            ChannelType::Spline
        } else if self.rot_flags & 0x0F != 0 {
            ChannelType::Static
        } else {
            ChannelType::Identity
        }
    }

    /// Byte alignment a rotation of this quantization code needs.
    fn rot_align(&self) -> usize {
        QUAT_ALIGN.get(self.rot_quant).copied().unwrap_or(4)
    }
}

/// Per-axis layout: bits 0..3 static per axis, bits 4..7 spline per axis.
fn axis_type(axis: usize, flags: u8) -> ChannelType {
    if (flags >> (axis + 4)) & 1 != 0 {
        ChannelType::Spline
    } else if (flags >> axis) & 1 != 0 {
        ChannelType::Static
    } else {
        ChannelType::Identity
    }
}

// ── Source GLB ───────────────────────────────────────────────────────────────

/// Write the decoded clip as a minimal source GLB: rig nodes at their reference pose
/// plus one LINEAR animation with dense per-frame keys.
///
/// Translations are scaled to metres; consecutive rotation keys stay in one hemisphere
/// so LINEAR interpolation takes the short arc. The layout is exactly what
/// `sculpto_mesh::actor::Actor::from_glb` validates: one embedded BIN buffer, TRS-only
/// nodes, one shared time track.
fn write_source_glb(
    skeleton: &RigSkeleton,
    clip: &SplineClip,
    track_bones: &[Option<usize>],
    scale: f64,
    name: &str,
) -> Result<Vec<u8>, CommandError> {
    let invalid = |detail: String| errors::invalid("havok_glb", detail);
    let times = clip.times();
    if times.len() < 2 {
        return Err(invalid(format!(
            "the clip has {} frame(s); a written clip needs at least two",
            times.len()
        )));
    }
    // Node per bone, parents from the rig, scene roots = parentless bones.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); skeleton.bones.len()];
    let mut roots = Vec::new();
    for (bone, parent) in skeleton.parents.iter().enumerate() {
        match usize::try_from(*parent)
            .ok()
            .filter(|parent| *parent < skeleton.bones.len())
        {
            Some(parent) => children[parent].push(bone),
            None => roots.push(bone),
        }
    }
    if roots.is_empty() {
        return Err(invalid("the rig has no root bone".to_owned()));
    }
    let mut buffer: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut push_view = |data: &[u8], kind: &str, count: usize, min_max: Option<(f64, f64)>| {
        let offset = buffer.len();
        buffer.extend_from_slice(data);
        let view_index = views.len();
        views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": data.len(),
        }));
        let mut accessor = json!({
            "bufferView": view_index,
            "componentType": 5126,
            "count": count,
            "type": kind,
        });
        if let Some((min, max)) = min_max {
            accessor["min"] = json!([min]);
            accessor["max"] = json!([max]);
        }
        let accessor_index = accessors.len();
        accessors.push(accessor);
        accessor_index
    };
    let pack_f32 = |values: &[f32]| -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    };
    let time_values: Vec<f32> = times.iter().map(|t| *t as f32).collect();
    let times_accessor = push_view(
        &pack_f32(&time_values),
        "SCALAR",
        times.len(),
        Some((times[0], times[times.len() - 1])),
    );

    let mut nodes: Vec<Value> = Vec::with_capacity(skeleton.bones.len());
    let mut channels: Vec<Value> = Vec::new();
    let mut samplers: Vec<Value> = Vec::new();
    for (bone, name) in skeleton.bones.iter().enumerate() {
        let (translation, rotation) = skeleton
            .rest
            .get(bone)
            .unwrap_or(&([0.0; 3], [0.0, 0.0, 0.0, 1.0]));
        let mut node = json!({
            "name": name,
            "translation": translation.iter().map(|v| v * scale).collect::<Vec<_>>(),
            "rotation": rotation,
        });
        if !children[bone].is_empty() {
            node["children"] = json!(children[bone]);
        }
        nodes.push(node);
    }
    for (track_index, track) in clip.tracks.iter().enumerate() {
        let Some(bone) = track_bones.get(track_index).copied().flatten() else {
            continue;
        };
        // A channel the clip does not declare must not be written: a zero
        // translation channel would override the node's rest every frame and
        // collapse the skeleton (the 0152 walk collapse), and an identity
        // rotation channel would pin the bone against its rest orientation.
        if track.has_rotation {
            let rotations = hemisphere_fixed(&track.rotations);
            let rotations: Vec<f32> = rotations
                .iter()
                .flat_map(|q| q.map(|value| value as f32))
                .collect();
            let rotation_accessor =
                push_view(&pack_f32(&rotations), "VEC4", track.rotations.len(), None);
            samplers.push(
                json!({"input": times_accessor, "output": rotation_accessor, "interpolation": "LINEAR"}),
            );
            channels.push(
                json!({"sampler": samplers.len() - 1, "target": {"node": bone, "path": "rotation"}}),
            );
        }
        if track.has_translation {
            let translations: Vec<f32> = track
                .translations
                .iter()
                .flat_map(|t| t.iter().map(|value| (*value * scale) as f32))
                .collect();
            let translation_accessor = push_view(
                &pack_f32(&translations),
                "VEC3",
                track.translations.len(),
                None,
            );
            samplers.push(
                json!({"input": times_accessor, "output": translation_accessor, "interpolation": "LINEAR"}),
            );
            channels.push(
                json!({"sampler": samplers.len() - 1, "target": {"node": bone, "path": "translation"}}),
            );
        }
    }
    if channels.is_empty() {
        return Err(invalid(
            "no track binds to a rig bone, so the source GLB would carry an empty clip".to_owned(),
        ));
    }

    let gltf = json!({
        "asset": {"version": "2.0", "generator": "sculpto actor havok"},
        "scene": 0,
        "scenes": [{"nodes": roots}],
        "nodes": nodes,
        "animations": [{
            "name": name,
            "samplers": samplers,
            "channels": channels,
        }],
        "buffers": [{"byteLength": buffer.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut json_bytes = serde_json::to_vec(&gltf).map_err(|error| invalid(error.to_string()))?;
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while !buffer.len().is_multiple_of(4) {
        buffer.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + buffer.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(buffer.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&buffer);
    Ok(out)
}

#[cfg(test)]
mod tests;
