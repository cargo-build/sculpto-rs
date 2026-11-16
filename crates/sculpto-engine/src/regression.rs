//! `rig.regression`: the four-way default-vs-adapted rig gate.
//!
//! The operator judged the four side-by-side viewer windows (Emily and Mike,
//! each with its own walk and with the adapted Mixamo walk) a good regression
//! test; the windows were ad hoc, so this verb replaces them with numbers. For
//! every declared actor it measures a default artifact (the actor's own walk)
//! and an adapted artifact (a converted clip) with the same instruments the
//! `0134` conversion gate runs, then decides on the *difference*: a
//! body-owned defect shows in both rows and cannot masquerade as a
//! conversion regression.
//!
//! Rows, all measured per artifact and clip:
//!
//! - **spine straightness** — per-segment lateral and sagittal tilt of the
//!   `Pelvis/Spine/Spine1/Spine2/Neck` chain, resolved by role rather than by
//!   name, and the peak cumulative lateral deviation of the chain top in mm.
//!   Positions live in the pelvis's yaw-only frame, so the walk's normal
//!   carriage folds out and only the spine's own bending stays.
//! - **hand-to-hip clearance** — the smallest distance from a hand or finger
//!   joint to the pelvis point or a hip/thigh segment, with the frame.
//! - **stance median and max** — the `0134` toe-slide form.
//! - **rest reconstruction** — the default artifact's rest must be the
//!   adapted artifact's rest, joint for joint.
//! - **role coverage** — the roles each clip drives, read from the file's own
//!   channels; the conversion may not lose a role the own clip drives.
//! - **edge stretch** — the rest-length classes; the gate is the relative one.
//! - **clothing stress** — the per-frame dressed clearance on the adapted
//!   artifact: the adapted clip against the actor's own anchor clip on the
//!   same mesh, plus declared context clips.
//!
//! A regression is a non-zero exit and a named row, not a note.
//!
//! @req BODY-01

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use glam::{DMat4, DQuat, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::{Value, json};

use sculpto_mesh::actor::{Actor, JointCandidate, Pose, StressPlan, stress};

pub(crate) use crate::adapt::source_clip_name;
use crate::export_actor::CLIP_ROLES;
use crate::measures::{
    RELATIVE_STRETCH, STANCE_MEDIAN_BAR, ShoulderLevel, edge_stretch, measurable_parts,
    palette_names, palette_reconstruction, shoulder_level_from_offsets, toe_slide,
};
use crate::retarget::{BoneRole, Part, Side, coverage_token, palette_role, role_of};
use crate::{CommandError, ErrorKind, errors};

/// Lateral mm the adapted chain may drift beyond `1.10 ×` the own walk's drift.
///
/// Sized to the measured bracket, not silently lowered: a healthy, aimed
/// transfer drives the spine and sways the chain top 27.6 mm at peak
/// (~10.8 mm abs-mean, `0146`'s independent measure) while the pre-fix
/// zigzag defect measured 138-163 mm — a five-fold separation. The old
/// 5 mm floor refused the post-fix reference itself, which is the
/// badly-posed case; if a tighter bar is wanted it belongs to the
/// coordinator, not to a silent constant.
pub const SPINE_DEVIATION_FLOOR_MM: f64 = 50.0;
/// Lateral degrees per segment the adapted chain may exceed `1.10 ×` default by.
///
/// Same measured bracket as the deviation floor: the healthy adapted walk
/// tilts a spine segment 5.1° at peak; the pre-fix defect read ≥24° once the
/// sliver artefact is removed. The old 2° floor refused the post-fix
/// reference itself.
pub const SPINE_TILT_FLOOR_DEG: f64 = 10.0;
/// Hand/finger-joint distance to a hip/thigh target below which the joint sits
/// inside the target's flesh envelope and the row is a crossing, not a style.
///
/// The joint-to-segment distance measures to the bone axis, and the axis runs
/// inside the flesh: Mike's hand joint sits ~25 mm under his own hand surface
/// (the `0134` joint-to-mesh inventory) and the thigh is fatter than the hand,
/// so a hand joint within 25 mm of the thigh axis is inside the thigh by any
/// reasonable envelope. The pre-fix adapted row measured 14.6 mm — inside;
/// the post-fix row measures ~125 mm — outside.
pub const CLEARANCE_CROSSING_FLOOR_MM: f64 = 25.0;
/// The proportional narrowing of the own walk's corridor reported as advisory.
///
/// Advisory, not a gate: the own walk here is the procedural gait, whose arms
/// swing wide, while the Mixamo reference carries its arms close — on its own
/// body the reference walk's arm↔torso gap reaches 0.24 mm (`012D`). A
/// relative bar against the gait therefore refuses a faithful transfer of the
/// reference carriage, which is the reference itself, so the narrowing is
/// reported with the delta instead of failing the row.
pub const CLEARANCE_MARGIN: f64 = 0.25;
/// Admitted adapted stretch as a multiple of the own clip's stretch at the
/// same witnesses: a body-owned tear (Mike's crotch) shows in both rows.
pub const CLOTHING_RELATIVE: f64 = 1.10;
/// Admitted adapted shoulder radius from the spine top, as a multiple of the
/// own walk's — which for a procedural gait with no clavicle channel is the
/// body's rest level.
///
/// The lever defect (`0154`) put the shoulder at 355.4 mm against the body's
/// own 280.7 mm on the adapted walk (1.266×) while the source, measured from
/// its own spine top, reads 207.6 mm with a 5.9 % radial excursion. A 10 % bar
/// separates a converted walk from a re-seated shoulder by more than two
/// times the defect, and leaves room for a genuinely different pelvis/spine
/// proportion.
pub const SHOULDER_RADIUS_RATIO: f64 = 1.10;
/// Admitted adapted shoulder height above the spine top, mm from the own
/// walk's. The same defect raised it 38.9 mm (252.7 against 213.8); 25 mm is
/// smaller than the defect and larger than any measured conversion drift.
pub const SHOULDER_HEIGHT_MM: f64 = 25.0;

/// The chain roles whose absence makes a baseline unanimated: spine and neck.
///
/// The head is deliberately absent: today's gait drives a ±5° head
/// counter-yaw while the spine segments stay at rest, so a head channel does
/// not make a baseline animated where the chain is concerned.
pub const SPINE_CHAIN_ROLES: [&str; 2] = ["spine", "neck"];
/// Human-scale sanity bound for a reference's peak chain deviation: a real
/// walk bends a spine millimetres to centimetres; beyond this the figure
/// collapsed and the numbers are debris, not measurements.
pub const REFERENCE_SANITY_MM: f64 = 5000.0;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegressionParams {
    /// One entry per actor: the default artifact, the adapted artifact and the
    /// clips each carries.
    pub actors: Vec<RegressionActor>,
    /// Clip frames the motion measures sample, endpoints included.
    #[serde(default = "default_frames")]
    pub frames: usize,
    /// Accepted graded visible clothing depth, GLB units (metres).
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// Also write the report JSON here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// One actor under regression.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegressionActor {
    /// Actor name the rows and failures carry.
    pub name: String,
    /// Artifact carrying the actor's own walk.
    pub default: PathBuf,
    /// Artifact carrying the adapted (converted) clip.
    pub adapted: PathBuf,
    /// The own-walk clip the default metrics measure.
    pub default_clip: String,
    /// The converted clip the adapted metrics measure.
    pub adapted_clip: String,
    /// The own-walk clip the clothing anchor row measures on the adapted
    /// artifact; defaults to `default_clip`.
    #[serde(default)]
    pub stress_anchor_clip: Option<String>,
    /// Extra context clips measured on the adapted artifact, report-only.
    #[serde(default)]
    pub stress_clips: Vec<String>,
    /// Body part address for the clothing rows (`Name` or `Name#i`).
    pub body: String,
    /// Garment part addresses graded against the body every frame.
    #[serde(default)]
    pub garments: Vec<String>,
    /// The conversion's source GLB; when declared, the motion row grows the
    /// role-matched travel amplification (adapted vs source per role) and the
    /// `mixamo-source` reference block (the same rows, on the source's own rig).
    #[serde(default)]
    pub source: Option<PathBuf>,
    /// The source clip name; with the file carrying exactly one, optional.
    #[serde(default)]
    pub source_clip: Option<String>,
    /// The native reference: a decoded Skyrim walk bound onto this body
    /// (`actor havok --actor …`), measured with the same rows, report-only.
    #[serde(default)]
    pub native: Option<PathBuf>,
    /// The native clip name as `actor havok` wrote it (the member stem).
    #[serde(default)]
    pub native_clip: Option<String>,
}

fn default_frames() -> usize {
    16
}

fn default_tolerance() -> f64 {
    0.0021
}

/// The command input schema.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(RegressionParams))
}

// ---------------------------------------------------------------------------
// typed metric cores; the JSON shapes mirror these
// ---------------------------------------------------------------------------

/// One spine segment's peak tilt over a clip.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SegmentTilt {
    /// `parent→child` chain names.
    pub segment: String,
    /// Peak angle between the posed and rest segment, sideways, degrees.
    pub peak_lateral_deg: f64,
    /// Clip time of that peak, seconds.
    pub peak_lateral_time: f64,
    /// Peak angle fore/aft, degrees.
    pub peak_sagittal_deg: f64,
    /// Clip time of that peak, seconds.
    pub peak_sagittal_time: f64,
}
/// Spine straightness of one clip: the defect the adapted walk carries today.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpineMetrics {
    /// Per-segment peaks, pelvis to neck.
    pub segments: Vec<SegmentTilt>,
    /// Peak lateral offset of the chain top from its rest position, mm — the
    /// cumulative lateral deviation.
    pub peak_lateral_deviation_mm: f64,
    /// Clip time of that peak, seconds.
    pub deviation_time: f64,
    /// The chain joint carrying the worst own lateral offset.
    pub worst_deviation_joint: String,
    /// Peak own lateral offset of that joint, mm.
    pub worst_deviation_mm: f64,
    /// Peak lateral tilt over the spine segments, degrees; the connector into
    /// the spine is reported per segment but never gates or peaks here.
    pub peak_lateral_tilt_deg: f64,
    /// Clip time of the peak lateral tilt, seconds.
    pub peak_lateral_time: f64,
    /// Peak sagittal tilt over the spine segments, degrees.
    pub peak_sagittal_tilt_deg: f64,
    /// Clip time of the peak sagittal tilt, seconds.
    pub peak_sagittal_time: f64,
    /// Frames sampled.
    pub frames: usize,
}

/// The narrowest hand-to-hip pass of one clip, with its witness.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ClearanceMetrics {
    /// Smallest joint-to-target distance over the clip, mm.
    pub min_mm: f64,
    /// Clip time of the minimum, seconds.
    pub time: f64,
    /// Frame index of the minimum, endpoints included.
    pub frame: usize,
    /// The hand or finger joint that reached it.
    pub joint: String,
    /// The pelvis/thigh target it approached.
    pub target: String,
    /// Frames sampled.
    pub frames: usize,
}

/// One bone's angular travel in one clip: the largest rotation between any
/// two samples of the bone's rotation channel, degrees.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MotionRange {
    /// The bone name as the clip addresses it.
    pub bone: String,
    /// The bone's standard role label (`L hand`, `spine1`, `pelvis`).
    pub role: String,
    /// Peak-to-peak rotation of the channel, degrees.
    pub range_deg: f64,
}

/// Per-clip motion inventory: how many bones the clip drives and how far the
/// loudest move. This is what separates an animated baseline from an
/// unanimated one — a clip can carry a skeleton and animate no spine or neck.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MotionMetrics {
    /// Distinct bones with a rotation channel.
    pub driven_bones: usize,
    /// Driven bones whose travel reaches one degree.
    pub moving_bones: usize,
    /// Every driven bone with its travel, loudest first.
    pub rows: Vec<MotionRange>,
}

impl MotionMetrics {
    /// The loudest `n` movers, for tables.
    pub fn loudest(&self, n: usize) -> &[MotionRange] {
        &self.rows[..n.min(self.rows.len())]
    }
}

/// One matched source↔adapted travel pair: the amplification the conversion
/// applies to one role. The finger fan lives here — our walk moves fingers
/// 28–34° where the source moves them 18°.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AmplificationRow {
    /// The matched role label.
    pub role: String,
    /// The source clip's travel for the role, degrees.
    pub source_deg: f64,
    /// The adapted clip's travel for the role, degrees.
    pub adapted_deg: f64,
    /// `adapted / source`; a source role that does not move yields no row.
    pub ratio: f64,
}

/// The roles one clip drives, read from the file's own channels.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CoverageMetrics {
    /// Standard roles the clip drives, [`CLIP_ROLES`] order.
    pub driven_roles: Vec<&'static str>,
    /// Standard roles the palette carries bones for.
    pub palette_roles: Vec<&'static str>,
    /// Palette roles the clip leaves at rest.
    pub undriven_roles: Vec<&'static str>,
    /// Distinct bones the clip drives.
    pub driven_bones: usize,
}

/// One typed gate verdict; the report carries these verbatim.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GateVerdict {
    /// Stable gate name the failure row carries.
    pub gate: &'static str,
    /// Row the verdict is about, e.g. `emily/adapted`.
    pub row: String,
    /// Whether the gate passes.
    pub pass: bool,
    /// The measurement behind the verdict, with both numbers.
    pub detail: String,
}

// ---------------------------------------------------------------------------
// pure metric math
// ---------------------------------------------------------------------------

/// Point in the chain frame joint's local space, metres.
///
/// The chain is measured where the clip writes its spine rotations: the spine
/// root's parent joint's local frame. On today's palettes that parent is the
/// `COM` mirror; after the pelvis re-parent it is the pelvis — either way the
/// body's carriage (yaw, pitch, roll, bob) moves the frame and the chain with
/// it and folds out, so only the spine's own bending stays. A chain rigid to
/// its parent measures zero under any motion of the parent.
pub fn chain_local(frame: DMat4, point: DVec3) -> DVec3 {
    frame.inverse().transform_point3(point)
}

/// The rest frame joint's rotation as an orthonormal basis.
///
/// Folding into the frame's local space alone is not enough: the frame joint's
/// own rest convention runs the chain along a local axis, not along world up —
/// on these palettes `COM`-local carries the spine along +Z — so a
/// world-lateral projection of a 122 mm segment is a 5 mm sliver and an 11 mm
/// sway tilts that sliver by ~118°. Re-expressing the folded chain in the rest
/// frame's world-aligned axes restores segment-length projections: lateral is
/// world X, sagittal world Z, and the angles scale with the real bend.
///
/// The basis is built from the normalized first two columns with the third
/// re-crossed, so a mirrored rest would still produce a proper rotation rather
/// than flipping the metric's handedness silently.
pub fn rest_frame_axes(frame_world_rest: DMat4) -> DMat4 {
    let x = frame_world_rest.x_axis.truncate().normalize();
    let y = frame_world_rest.y_axis.truncate().normalize();
    let z = x.cross(y);
    if x.length() < 1e-9 || y.length() < 1e-9 || z.length() < 1e-9 {
        // A degenerate rest frame cannot define axes; the identity keeps the
        // caller measurable and the report already carries the frame joint.
        return DMat4::IDENTITY;
    }
    DMat4::from_cols(
        x.extend(0.0),
        y.extend(0.0),
        z.extend(0.0),
        DVec3::ZERO.extend(1.0),
    )
}

/// The folded chain point re-expressed in the rest frame's world-aligned axes.
pub fn chain_aligned(rest_axes: DMat4, local: DVec3) -> DVec3 {
    rest_axes.transform_point3(local)
}

fn projected_angle_deg(first: [f64; 2], second: [f64; 2]) -> f64 {
    let length = first.iter().map(|v| v * v).sum::<f64>().sqrt()
        * second.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length < 1e-12 {
        return 0.0;
    }
    let dot = (first[0] * second[0] + first[1] * second[1]) / length;
    dot.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Spine straightness from the chain's folded, world-aligned positions.
///
/// `rest` and every frame's chain carry the five joints in one order, pelvis
/// first, folded into the frame joint and re-expressed in the rest frame's
/// world-aligned axes ([`chain_aligned`]): the rest chain stands along world
/// up, lateral is world X, sagittal world Z.
pub fn spine_metrics(rest: &[DVec3], frames: &[(f64, Vec<DVec3>)]) -> SpineMetrics {
    let names = ["Pelvis", "Spine", "Spine1", "Spine2", "Neck"];
    let rest_mm: Vec<[f64; 3]> = rest
        .iter()
        .map(|p| [p.x * 1000.0, p.y * 1000.0, p.z * 1000.0])
        .collect();
    let mut segments: Vec<SegmentTilt> = (0..rest.len().saturating_sub(1))
        .map(|index| SegmentTilt {
            segment: format!("{}→{}", names[index], names[index + 1]),
            peak_lateral_deg: 0.0,
            peak_lateral_time: 0.0,
            peak_sagittal_deg: 0.0,
            peak_sagittal_time: 0.0,
        })
        .collect();
    let mut peak_lateral_tilt = 0.0_f64;
    let mut peak_lateral_time = 0.0_f64;
    let mut peak_sagittal_tilt = 0.0_f64;
    let mut peak_sagittal_time = 0.0_f64;
    let mut peak_deviation = 0.0_f64;
    let mut deviation_time = 0.0_f64;
    let mut worst_joint = String::new();
    let mut worst_joint_mm = 0.0_f64;
    for (time, chain) in frames {
        let posed_mm: Vec<[f64; 3]> = chain
            .iter()
            .map(|p| [p.x * 1000.0, p.y * 1000.0, p.z * 1000.0])
            .collect();
        for index in 1..posed_mm.len() {
            let delta = [
                posed_mm[index][0] - rest_mm[index][0],
                posed_mm[index][1] - rest_mm[index][1],
                posed_mm[index][2] - rest_mm[index][2],
            ];
            if delta[0].abs() > worst_joint_mm {
                worst_joint_mm = delta[0].abs();
                worst_joint = names[index].to_owned();
            }
            let is_top = index == posed_mm.len() - 1;
            if is_top && delta[0].abs() > peak_deviation {
                peak_deviation = delta[0].abs();
                deviation_time = *time;
            }
            if let Some(segment) = segments.get_mut(index - 1) {
                let rest_segment = [
                    rest_mm[index][0] - rest_mm[index - 1][0],
                    rest_mm[index][1] - rest_mm[index - 1][1],
                ];
                let posed_segment = [
                    posed_mm[index][0] - posed_mm[index - 1][0],
                    posed_mm[index][1] - posed_mm[index - 1][1],
                ];
                let lateral = projected_angle_deg(posed_segment, rest_segment);
                if lateral > segment.peak_lateral_deg {
                    segment.peak_lateral_deg = lateral;
                    segment.peak_lateral_time = *time;
                }
                // Segment 0 is the connector into the spine (Pelvis→Spine); on
                // a palette where the spine hangs beside the pelvis it carries
                // the pelvis's own carriage, so the gate and the reported
                // peaks read the spine segments only, while the connector
                // stays listed below.
                if index > 1 && lateral > peak_lateral_tilt {
                    peak_lateral_tilt = lateral;
                    peak_lateral_time = *time;
                }
                let rest_sagittal = [
                    rest_mm[index][2] - rest_mm[index - 1][2],
                    rest_mm[index][1] - rest_mm[index - 1][1],
                ];
                let posed_sagittal = [
                    posed_mm[index][2] - posed_mm[index - 1][2],
                    posed_mm[index][1] - posed_mm[index - 1][1],
                ];
                let sagittal = projected_angle_deg(posed_sagittal, rest_sagittal);
                if sagittal > segment.peak_sagittal_deg {
                    segment.peak_sagittal_deg = sagittal;
                    segment.peak_sagittal_time = *time;
                }
                if index > 1 && sagittal > peak_sagittal_tilt {
                    peak_sagittal_tilt = sagittal;
                    peak_sagittal_time = *time;
                }
            }
        }
    }
    SpineMetrics {
        segments,
        peak_lateral_deviation_mm: peak_deviation,
        deviation_time,
        worst_deviation_joint: if worst_joint.is_empty() {
            "Neck".to_owned()
        } else {
            worst_joint
        },
        worst_deviation_mm: worst_joint_mm,
        peak_lateral_tilt_deg: peak_lateral_tilt,
        peak_lateral_time,
        peak_sagittal_tilt_deg: peak_sagittal_tilt,
        peak_sagittal_time,
        frames: frames.len(),
    }
}

/// Distance from a point to a segment, metres; a point target is `a == b`.
pub fn point_segment_distance(point: DVec3, a: DVec3, b: DVec3) -> f64 {
    let edge = b - a;
    let span = edge.length_squared();
    if span < 1e-18 {
        return point.distance(a);
    }
    let t = ((point - a).dot(edge) / span).clamp(0.0, 1.0);
    point.distance(a + edge * t)
}

/// One clearance frame: hand/finger joints against pelvis/thigh targets.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearanceFrame {
    /// Clip time, seconds.
    pub time: f64,
    /// `(joint name, world position)` for every hand and finger joint.
    pub joints: Vec<(String, DVec3)>,
    /// `(target name, segment start, segment end)`; a point target repeats the
    /// position.
    pub targets: Vec<(String, DVec3, DVec3)>,
}

/// Hand-to-hip clearance over a clip: the smallest pass and its witness.
pub fn clearance_metrics(frames: &[ClearanceFrame]) -> ClearanceMetrics {
    let mut best: Option<ClearanceMetrics> = None;
    for (index, frame) in frames.iter().enumerate() {
        for (joint, point) in &frame.joints {
            for (target, a, b) in &frame.targets {
                let distance = point_segment_distance(*point, *a, *b) * 1000.0;
                let better = match &best {
                    None => true,
                    Some(current) => distance < current.min_mm,
                };
                if better {
                    best = Some(ClearanceMetrics {
                        min_mm: distance,
                        time: frame.time,
                        frame: index,
                        joint: joint.clone(),
                        target: target.clone(),
                        frames: frames.len(),
                    });
                }
            }
        }
    }
    best.unwrap_or(ClearanceMetrics {
        min_mm: f64::INFINITY,
        time: 0.0,
        frame: 0,
        joint: String::new(),
        target: String::new(),
        frames: frames.len(),
    })
}

/// The standard roles one set of palette bone names answers.
fn palette_role_tokens(names: &[String]) -> Vec<&'static str> {
    let mut tokens: Vec<&'static str> = names
        .iter()
        .filter_map(|name| palette_role(name))
        .filter_map(|role| coverage_token(role.part))
        .collect();
    tokens.sort_unstable();
    tokens.dedup();
    CLIP_ROLES
        .iter()
        .filter(|role| tokens.contains(role))
        .copied()
        .collect()
}

/// The roles one clip drives, read from the artifact's own channels — not from
/// a conversion plan, so the same measure answers a written file and a
/// hand-authored clip.
///
/// # Errors
/// A missing or ambiguous clip.
pub fn clip_role_coverage(
    actor: &Actor,
    clip: &str,
    palette: &[String],
) -> Result<CoverageMetrics, CommandError> {
    let source = actor
        .clip_shared(clip)
        .map_err(crate::measures::actor_error)?;
    let mut driven: BTreeMap<&'static str, ()> = BTreeMap::new();
    let mut bones: BTreeMap<String, ()> = BTreeMap::new();
    for channel in &source.channels {
        // Scale carries no pose.
        if matches!(
            channel.property,
            sculpto_mesh::actor::SourceProperty::Translation
                | sculpto_mesh::actor::SourceProperty::Rotation
        ) && let Some(name) = &channel.node_name
        {
            bones.insert(name.clone(), ());
            if let Some(role) = palette_role(name)
                && let Some(token) = coverage_token(role.part)
            {
                driven.insert(token, ());
            }
        }
    }
    let palette_roles = palette_role_tokens(palette);
    let undriven: Vec<&'static str> = palette_roles
        .iter()
        .filter(|role| !driven.contains_key(**role))
        .copied()
        .collect();
    Ok(CoverageMetrics {
        driven_roles: CLIP_ROLES
            .iter()
            .filter(|role| driven.contains_key(**role))
            .copied()
            .collect(),
        palette_roles,
        undriven_roles: undriven,
        driven_bones: bones.len(),
    })
}

/// Largest rotation between any two samples of one rotation channel, degrees.
///
/// Constant channels measure 0.0; the shortest-path angle keeps the result in
/// [0°, 180°] even across a wrap.
pub fn rotation_range_deg(values: &[[f64; 4]]) -> f64 {
    let quats: Vec<DQuat> = values
        .iter()
        .filter(|value| value.iter().all(|component| component.is_finite()))
        .map(|value| DQuat::from_array(*value).normalize())
        .collect();
    let mut max = 0.0_f64;
    for (index, first) in quats.iter().enumerate() {
        for second in &quats[index + 1..] {
            let delta = first.inverse() * *second;
            let angle = 2.0 * delta.w.clamp(-1.0, 1.0).abs().acos();
            max = max.max(angle.to_degrees());
        }
    }
    max
}

/// Per-bone angular travel of one clip, classified by `classify` (the palette
/// classifier for our clips, the source-name classifier for foreign ones).
///
/// # Errors
/// A missing or ambiguous clip.
pub fn motion_ranges(
    actor: &Actor,
    clip: &str,
    classify: fn(&str) -> Option<BoneRole>,
) -> Result<MotionMetrics, CommandError> {
    let source = actor
        .clip_shared(clip)
        .map_err(crate::measures::actor_error)?;
    // One range per bone: the loudest of its rotation channels. Translation
    // channels carry no angle and are not part of this row.
    let mut per_bone: BTreeMap<String, f64> = BTreeMap::new();
    for channel in &source.channels {
        if !matches!(
            channel.property,
            sculpto_mesh::actor::SourceProperty::Rotation
        ) {
            continue;
        }
        let sculpto_mesh::actor::SourceValues::Rotation(values) = &channel.values else {
            continue;
        };
        let Some(name) = &channel.node_name else {
            continue;
        };
        let range = rotation_range_deg(values);
        per_bone
            .entry(name.clone())
            .and_modify(|current| *current = current.max(range))
            .or_insert(range);
    }
    let mut rows: Vec<MotionRange> = per_bone
        .into_iter()
        .map(|(bone, range_deg)| MotionRange {
            role: classify(&bone).map(|role| role.label()).unwrap_or_default(),
            bone,
            range_deg,
        })
        .collect();
    rows.sort_unstable_by(|a, b| b.range_deg.total_cmp(&a.range_deg));
    let moving = rows.iter().filter(|row| row.range_deg >= 1.0).count();
    Ok(MotionMetrics {
        driven_bones: rows.len(),
        moving_bones: moving,
        rows,
    })
}

// ---------------------------------------------------------------------------
// gates
// ---------------------------------------------------------------------------

/// Spine straightness: the adapted chain may not bend beyond the measured
/// bracket. `default_drives_spine_chain` decides the form: against a clip that
/// actually animates the spine the bound is relative to it; against an
/// unanimated own walk there is no reference and the bracket floors apply to
/// the adapted row absolutely.
pub fn spine_gate(
    default: &SpineMetrics,
    adapted: &SpineMetrics,
    row: &str,
    default_drives_spine_chain: bool,
) -> GateVerdict {
    let deviation_bound = match default_drives_spine_chain {
        true => SPINE_DEVIATION_FLOOR_MM.max(1.10 * default.peak_lateral_deviation_mm),
        false => SPINE_DEVIATION_FLOOR_MM,
    };
    let tilt_bound = match default_drives_spine_chain {
        true => SPINE_TILT_FLOOR_DEG.max(1.10 * default.peak_lateral_tilt_deg),
        false => SPINE_TILT_FLOOR_DEG,
    };
    let pass = adapted.peak_lateral_deviation_mm <= deviation_bound
        && adapted.peak_lateral_tilt_deg <= tilt_bound;
    GateVerdict {
        gate: "spine_lateral",
        row: row.to_owned(),
        pass,
        detail: format!(
            "peak lateral deviation {:.1} mm (bound {:.1}, default {}), peak lateral tilt \
             {:.2}° at t={:.3} (bound {:.2}, default {:.2}), sagittal {:.2}° (reported, not gated){}",
            adapted.peak_lateral_deviation_mm,
            deviation_bound,
            match default_drives_spine_chain {
                true => format!("{:.1}", default.peak_lateral_deviation_mm),
                false => "unanimated — the own clip animates no spine or neck bone, so the \
                          bound is the absolute bracket (healthy 27.6 mm / 5.1° versus defect \
                          138 mm / ≥24°)"
                    .to_owned(),
            },
            adapted.peak_lateral_tilt_deg,
            adapted.peak_lateral_time,
            tilt_bound,
            default.peak_lateral_tilt_deg,
            adapted.peak_sagittal_tilt_deg,
            match default_drives_spine_chain {
                true => "",
                false => "; no reference clip animates the spine chain on our side yet",
            },
        ),
    }
}

/// Hand-to-hip clearance: the gate refuses a **crossing** — the adapted walk
/// driving a hand or finger joint inside the hip/thigh flesh envelope — and
/// reports the corridor narrowing as advisory.
///
/// The narrowing alone cannot gate: the own walk here is the procedural gait,
/// whose arms swing wide, while the Mixamo reference carries its arms close —
/// on its own body the reference walk's arm↔torso gap reaches 0.24 mm
/// (`012D`). A relative bar against the gait therefore refuses a faithful
/// transfer of the reference carriage, which is the reference itself; the
/// measured history makes the difference visible (the pre-fix adapted row
/// jammed to 14.6 mm — inside the envelope, a real crossing; the post-fix row
/// rests at ~125 mm — outside it, merely a closer carriage). When the own clip
/// already crosses, the gate is vacuous and says so.
pub fn clearance_gate(
    default: &ClearanceMetrics,
    adapted: &ClearanceMetrics,
    row: &str,
) -> GateVerdict {
    let crossing = adapted.min_mm < CLEARANCE_CROSSING_FLOOR_MM;
    let vacuous = default.min_mm < CLEARANCE_CROSSING_FLOOR_MM;
    let narrowing = 1.0 - (adapted.min_mm / default.min_mm.max(f64::MIN_POSITIVE));
    let narrowing_advisory = !vacuous && narrowing > CLEARANCE_MARGIN && !crossing;
    GateVerdict {
        gate: "clearance",
        row: row.to_owned(),
        // A body that crosses on its own material cannot blame the conversion:
        // the vacuous case passes with the note, not with a silence.
        pass: !crossing || vacuous,
        detail: format!(
            "min {:.2} mm at t={:.3} ({}↔{}, frame {}) against default {:.2} mm: {}, {}{}",
            adapted.min_mm,
            adapted.time,
            adapted.joint,
            adapted.target,
            adapted.frame,
            default.min_mm,
            match crossing {
                true => format!(
                    "CROSSING the {:.0} mm flesh-envelope floor",
                    CLEARANCE_CROSSING_FLOOR_MM
                ),
                false => format!(
                    "outside the {:.0} mm crossing floor",
                    CLEARANCE_CROSSING_FLOOR_MM
                ),
            },
            match narrowing_advisory {
                true => format!(
                    "corridor narrowed {:.0}% (advisory: authored carriage differs between clips, see 012D)",
                    narrowing * 100.0
                ),
                false => "corridor narrowing within the advisory margin".to_owned(),
            },
            match vacuous {
                true => "; the own clip also crosses, so the crossing read is vacuous here",
                false => "",
            },
        ),
    }
}

/// The girdle gate: the adapted shoulder's *level* against the own walk's,
/// which for a procedural gait without a clavicle channel is the body's rest.
///
/// Swing is not gated: the reference's own shoulders move back and forth
/// (`0154` measured 5.9 % of the radius from its own spine top), and the
/// operator was explicit that the swing is natural. What must not move is the
/// mean: the lever defect put the mean 74.7 mm further out than the body's own
/// seat, which is the frame over the chest.
pub fn shoulder_level_gate(
    default: &ShoulderLevel,
    adapted: &ShoulderLevel,
    row: &str,
) -> GateVerdict {
    let ratio = adapted.mean_radius_mm / default.mean_radius_mm.max(f64::MIN_POSITIVE);
    let height_delta = adapted.mean_height_mm - default.mean_height_mm;
    GateVerdict {
        gate: "shoulder_level",
        row: row.to_owned(),
        pass: ratio <= SHOULDER_RADIUS_RATIO && height_delta.abs() <= SHOULDER_HEIGHT_MM,
        detail: format!(
            "adapted {} mean radius {:.1} mm ({:.3}× own {:.1}, bar {:.2}×), mean height {:+.1} mm \
             above {} (own {:+.1}, delta {:+.1}, bar ±{:.0}); radial excursion {:.1} mm ({:.1}% of \
             radius), height excursion {:.1} mm",
            adapted.bone,
            adapted.mean_radius_mm,
            ratio,
            default.mean_radius_mm,
            SHOULDER_RADIUS_RATIO,
            adapted.mean_height_mm,
            default.spine,
            default.mean_height_mm,
            height_delta,
            SHOULDER_HEIGHT_MM,
            adapted.radius_excursion_mm,
            adapted.radius_excursion_fraction * 100.0,
            adapted.height_excursion_mm,
        ),
    }
}

/// Edge stretch, relative: the adapted clip may not stretch the body more than
/// `1.10 ×` the own clip's stretch at the same witnesses. The absolutes stay
/// in the report; they are the body's property.
pub fn stretch_gate(default_five_mm: f64, adapted_five_mm: f64, row: &str) -> GateVerdict {
    let bound = RELATIVE_STRETCH * default_five_mm;
    let pass = if default_five_mm <= 0.0 {
        adapted_five_mm <= 0.0
    } else {
        adapted_five_mm <= bound
    };
    GateVerdict {
        gate: "edge_stretch_relative",
        row: row.to_owned(),
        pass,
        detail: format!(
            "adapted {:.2}x at ≥5 mm (bound {:.2}x, default {:.2}x){}",
            adapted_five_mm,
            bound,
            default_five_mm,
            match default_five_mm <= 0.0 {
                true => "; the own clip stretches nothing, so any conversion stretch fails here",
                false => "",
            },
        ),
    }
}

/// Clothing stress: when the own anchor clip passes the tolerance on the same
/// mesh, the adapted clip must too (an absolute regression); when the anchor
/// already fails, the verdict is the relative delta, never the absolute number.
/// Depths and the tolerance arrive in the same units (mm).
pub fn clothing_gate(
    tolerance_mm: f64,
    anchor_depth_mm: f64,
    adapted_depth_mm: f64,
    anchor_clip: &str,
    row: &str,
) -> GateVerdict {
    let anchor_pass = anchor_depth_mm <= tolerance_mm;
    let bound = match anchor_pass {
        true => tolerance_mm,
        false => CLOTHING_RELATIVE * anchor_depth_mm,
    };
    GateVerdict {
        gate: "clothing_stress",
        row: row.to_owned(),
        pass: adapted_depth_mm <= bound,
        detail: format!(
            "adapted {:.3} mm against anchor {anchor_clip:?} {:.3} mm ({}), bound {:.3} mm",
            adapted_depth_mm,
            anchor_depth_mm,
            match anchor_pass {
                true => "passing, so the adapted row must pass the tolerance too",
                false => "already failing, delta is the verdict",
            },
            bound,
        ),
    }
}

// ---------------------------------------------------------------------------
// sampling plumbing
// ---------------------------------------------------------------------------

/// One chain role the spine row needs.
struct ChainRole {
    side: Side,
    matches: fn(&Part) -> bool,
    label: &'static str,
}

/// The chain roles the spine row needs, in chain order.
const CHAIN_ORDER: [ChainRole; 5] = [
    ChainRole {
        side: Side::Centre,
        matches: |part| matches!(part, Part::Pelvis),
        label: "Pelvis",
    },
    ChainRole {
        side: Side::Centre,
        matches: |part| matches!(part, Part::Spine(0)),
        label: "Spine",
    },
    ChainRole {
        side: Side::Centre,
        matches: |part| matches!(part, Part::Spine(1)),
        label: "Spine1",
    },
    ChainRole {
        side: Side::Centre,
        matches: |part| matches!(part, Part::Spine(2)),
        label: "Spine2",
    },
    ChainRole {
        side: Side::Centre,
        matches: |part| matches!(part, Part::Neck),
        label: "Neck",
    },
];

/// Resolve the spine chain, hand/finger joints and hip/thigh targets on one
/// palette, by role. Refuses with the missing role named.
fn resolve_joints(palette: &[String]) -> Result<ResolvedJoints, CommandError> {
    let mut resolved = ResolvedJoints::default();
    for role in CHAIN_ORDER {
        let name = palette
            .iter()
            .find(|name| {
                palette_role(name)
                    .is_some_and(|found| found.side == role.side && (role.matches)(&found.part))
            })
            .ok_or_else(|| {
                errors::invalid(
                    "rig_regression_chain",
                    format!(
                        "the palette carries no {} joint, so the spine row has no chain",
                        role.label
                    ),
                )
            })?;
        resolved.chain.push(name.clone());
    }
    for (name, role) in palette
        .iter()
        .filter_map(|name| palette_role(name).map(|role| (name, role)))
    {
        match role.part {
            Part::Hand => resolved.hands.push(name.clone()),
            Part::Digit { .. } => resolved.digits.push(name.clone()),
            Part::Thigh => resolved.thighs.push(name.clone()),
            Part::Calf => resolved.calves.push(name.clone()),
            _ => {}
        }
    }
    if resolved.hands.is_empty() {
        return Err(errors::invalid(
            "rig_regression_chain",
            "the palette carries no hand joint, so the clearance row has no hand",
        ));
    }
    if resolved.thighs.len() < 2 || resolved.calves.len() < 2 {
        return Err(errors::invalid(
            "rig_regression_chain",
            "the palette carries no thigh/calf pair, so the clearance row has no hip target",
        ));
    }
    let pelvis = resolved
        .chain
        .first()
        .cloned()
        .ok_or_else(|| errors::invalid("rig_regression_chain", "empty chain"))?;
    resolved.pelvis = pelvis;
    resolved.spine_top = resolved
        .chain
        .get(3)
        .cloned()
        .ok_or_else(|| errors::invalid("rig_regression_chain", "the chain has no Spine2"))?;
    let upper_arms: Vec<(&String, Side)> = palette
        .iter()
        .filter_map(|name| {
            palette_role(name)
                .filter(|role| matches!(role.part, Part::UpperArm))
                .map(|role| (name, role.side))
        })
        .collect();
    resolved.shoulder = upper_arms
        .iter()
        .find(|(_, side)| *side == Side::Left)
        .or_else(|| upper_arms.first())
        .map(|(name, _)| (*name).clone());
    Ok(resolved)
}

/// The palette joints the motion rows sample, by role.
#[derive(Default, Debug, Clone)]
struct ResolvedJoints {
    pelvis: String,
    /// Pelvis → Neck, five joints on these palettes.
    chain: Vec<String>,
    /// The top of the spine chain (`Spine2` on these palettes).
    spine_top: String,
    /// One shoulder the girdle row measures: the first `UpperArm` by side,
    /// left preferred so the row is deterministic. Tool chains and test
    /// fixtures without arms have none, and their girdle row is absent, not
    /// vacuously passing.
    shoulder: Option<String>,
    hands: Vec<String>,
    digits: Vec<String>,
    thighs: Vec<String>,
    calves: Vec<String>,
}

/// World matrices of the wanted joints at evenly spaced clip times.
///
/// The whole palette is requested per frame — a retained rig keeps the joints a
/// requested bone's vertices bind to — so a joint outside the wanted list is
/// still retained when the skin needs it.
type JointTracks = Vec<(f64, BTreeMap<String, DMat4>)>;

fn joint_tracks(
    actor: &Actor,
    clip: &str,
    frames: usize,
    wanted: &[String],
    palette: &[String],
) -> Result<JointTracks, CommandError> {
    let parts = measurable_parts(actor);
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let (start, end) = actor
        .clip_span(clip)
        .map_err(crate::measures::actor_error)?;
    let count = frames.max(2);
    let mut tracks = Vec::with_capacity(count);
    for index in 0..count {
        let time = start + (end - start) * index as f64 / (count - 1) as f64;
        let sample = actor
            .rig_sample(
                &parts,
                Pose::Clip { name: clip, time },
                &palette.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .map_err(crate::measures::actor_error)?;
        let mut worlds = BTreeMap::new();
        for name in wanted {
            let node = sample
                .rig
                .iter()
                .find(|node| node.name.as_deref() == Some(name.as_str()))
                .ok_or_else(|| {
                    errors::invalid(
                        "rig_regression_chain",
                        format!("the rig sample does not carry joint {name:?}"),
                    )
                })?;
            worlds.insert(name.clone(), DMat4::from_cols_array_2d(&node.world));
        }
        tracks.push((time, worlds));
    }
    Ok(tracks)
}

/// Rest state of the chain: the frame joint (the spine root's parent, so the
/// chain is measured where its rotations land) and the chain's rest world
/// positions.
fn rest_pose(
    actor: &Actor,
    chain: &[String],
    palette: &[String],
) -> Result<(String, DMat4, Vec<DVec3>), CommandError> {
    let parts = measurable_parts(actor);
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let sample = actor
        .rig_sample(
            &parts,
            Pose::Rest,
            &palette.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .map_err(crate::measures::actor_error)?;
    let spine = chain
        .get(1)
        .ok_or_else(|| errors::invalid("rig_regression_chain", "the chain has no spine root"))?;
    let spine_position = sample
        .rig
        .iter()
        .position(|node| node.name.as_deref() == Some(spine.as_str()))
        .ok_or_else(|| {
            errors::invalid(
                "rig_regression_chain",
                format!("the rest rig does not carry joint {spine:?}"),
            )
        })?;
    // `RigNode.parent` is the retained rig's own index: the nearest retained
    // ancestor, which with the whole palette requested is the joint the spine
    // hangs from in the file.
    let parent_position = sample.rig[spine_position].parent.ok_or_else(|| {
        errors::invalid(
            "rig_regression_chain",
            format!(
                "joint {spine:?} has no retained parent, so there is no frame to measure it in"
            ),
        )
    })?;
    let frame_node = sample.rig.get(parent_position).ok_or_else(|| {
        errors::invalid(
            "rig_regression_chain",
            format!(
                "joint {spine:?}'s rig parent index {parent_position} is outside the retained rig"
            ),
        )
    })?;
    let frame_name = frame_node.name.clone().ok_or_else(|| {
        errors::invalid(
            "rig_regression_chain",
            format!(
                "joint {spine:?}'s parent node {} is unnamed",
                frame_node.node
            ),
        )
    })?;
    let mut positions = Vec::with_capacity(chain.len());
    for name in chain {
        let node = sample
            .rig
            .iter()
            .find(|node| node.name.as_deref() == Some(name.as_str()))
            .ok_or_else(|| {
                errors::invalid(
                    "rig_regression_chain",
                    format!("the rest rig does not carry joint {name:?}"),
                )
            })?;
        positions.push(DMat4::from_cols_array_2d(&node.world).w_axis.truncate());
    }
    Ok((
        frame_name,
        DMat4::from_cols_array_2d(&frame_node.world),
        positions,
    ))
}

/// One motion measurement on one artifact and clip: spine chain plus
/// hand-to-hip clearance from one shared joint pass.
struct MotionRows {
    /// The joint whose local frame the chain was measured in.
    frame_joint: String,
    spine: SpineMetrics,
    clearance: ClearanceMetrics,
    /// Shoulder level against the spine top: the girdle row, absent on a
    /// palette that carries no arm.
    shoulder: Option<ShoulderLevel>,
}

fn measure_motion(
    actor: &Actor,
    clip: &str,
    frames: usize,
    joints: &ResolvedJoints,
    palette: &[String],
) -> Result<MotionRows, CommandError> {
    let mut wanted: Vec<String> = joints.chain.clone();
    for list in [
        &joints.hands,
        &joints.digits,
        &joints.thighs,
        &joints.calves,
    ] {
        wanted.extend(list.iter().cloned());
    }
    if let Some(shoulder) = &joints.shoulder {
        wanted.push(shoulder.clone());
    }
    wanted.sort_unstable();
    wanted.dedup();
    let (frame_joint, frame_world_rest, rest_positions) = rest_pose(actor, &joints.chain, palette)?;
    // Fold into the frame joint, then re-express in the rest frame's
    // world-aligned axes: the fold kills the carriage, the axes keep the
    // lateral/sagittal projections the length of the segment instead of the
    // millimetre-scale sliver a rotated rest convention leaves.
    let rest_axes = rest_frame_axes(frame_world_rest);
    let rest_folded: Vec<DVec3> = rest_positions
        .iter()
        .map(|point| chain_aligned(rest_axes, chain_local(frame_world_rest, *point)))
        .collect();
    // The body's own up axis: the rest chain, pelvis to neck. Reading it from
    // the body rather than from a world axis keeps the girdle row honest on a
    // file whose world axes are the rig's convention (these palettes are Z-up).
    let up = match (rest_positions.first(), rest_positions.last()) {
        (Some(pelvis), Some(neck)) => (*neck - *pelvis).normalize_or_zero(),
        _ => DVec3::Z,
    };
    let mut sampled = wanted;
    sampled.push(frame_joint.clone());
    sampled.sort_unstable();
    sampled.dedup();
    let tracks = joint_tracks(actor, clip, frames, &sampled, palette)?;
    let mut chain_frames = Vec::with_capacity(tracks.len());
    let mut clearance_frames = Vec::with_capacity(tracks.len());
    let mut shoulder_offsets: Vec<DVec3> = Vec::with_capacity(tracks.len());
    for (time, worlds) in &tracks {
        let frame_world = worlds.get(&frame_joint).ok_or_else(|| {
            errors::invalid(
                "rig_regression_chain",
                format!("frame joint {frame_joint:?} missing from a frame"),
            )
        })?;
        let chain: Vec<DVec3> = joints
            .chain
            .iter()
            .map(|name| {
                worlds
                    .get(name)
                    .map(|world| world.w_axis.truncate())
                    .ok_or_else(|| {
                        errors::invalid(
                            "rig_regression_chain",
                            format!("{name:?} missing from a frame"),
                        )
                    })
            })
            .collect::<Result<_, _>>()?;
        chain_frames.push((
            *time,
            chain
                .iter()
                .map(|p| chain_aligned(rest_axes, chain_local(*frame_world, *p)))
                .collect(),
        ));
        let pelvis_world = worlds.get(&joints.pelvis).ok_or_else(|| {
            errors::invalid(
                "rig_regression_chain",
                format!("{:?} missing from a frame", joints.pelvis),
            )
        })?;
        let mut frame = ClearanceFrame {
            time: *time,
            joints: Vec::new(),
            targets: Vec::new(),
        };
        for list in [&joints.hands, &joints.digits] {
            for name in list.iter() {
                if let Some(world) = worlds.get(name) {
                    frame.joints.push((name.clone(), world.w_axis.truncate()));
                }
            }
        }
        let pelvis_point = pelvis_world.w_axis.truncate();
        frame
            .targets
            .push(("pelvis".to_owned(), pelvis_point, pelvis_point));
        let thighs: Vec<DVec3> = joints
            .thighs
            .iter()
            .map(|name| {
                worlds
                    .get(name)
                    .map(|world| world.w_axis.truncate())
                    .ok_or_else(|| {
                        errors::invalid(
                            "rig_regression_chain",
                            format!("{name:?} missing from a frame"),
                        )
                    })
            })
            .collect::<Result<_, _>>()?;
        let calves: Vec<DVec3> = joints
            .calves
            .iter()
            .map(|name| {
                worlds
                    .get(name)
                    .map(|world| world.w_axis.truncate())
                    .ok_or_else(|| {
                        errors::invalid(
                            "rig_regression_chain",
                            format!("{name:?} missing from a frame"),
                        )
                    })
            })
            .collect::<Result<_, _>>()?;
        let sides = ["L", "R"];
        for (index, side) in sides.iter().enumerate() {
            if let (Some(thigh), Some(calf)) = (thighs.get(index), calves.get(index)) {
                frame
                    .targets
                    .push((format!("{side} hip"), pelvis_point, *thigh));
                frame.targets.push((format!("{side} thigh"), *thigh, *calf));
            }
        }
        clearance_frames.push(frame);
        if let Some(shoulder) = &joints.shoulder {
            let shoulder_world = worlds.get(shoulder).ok_or_else(|| {
                errors::invalid(
                    "rig_regression_chain",
                    format!("{shoulder:?} missing from a frame"),
                )
            })?;
            let spine_world = worlds.get(&joints.spine_top).ok_or_else(|| {
                errors::invalid(
                    "rig_regression_chain",
                    format!("{:?} missing from a frame", joints.spine_top),
                )
            })?;
            shoulder_offsets.push(shoulder_world.w_axis.truncate() - spine_world.w_axis.truncate());
        }
    }
    Ok(MotionRows {
        frame_joint,
        spine: spine_metrics(&rest_folded, &chain_frames),
        clearance: clearance_metrics(&clearance_frames),
        shoulder: joints.shoulder.as_ref().map(|shoulder| {
            shoulder_level_from_offsets(shoulder, &joints.spine_top, up, &shoulder_offsets)
        }),
    })
}

/// One clothing stress row: the worst frame of one clip on one artifact.
#[derive(Clone, Debug)]
struct StressRow {
    clip: String,
    worst_mm: f64,
    worst_part: String,
    frame: usize,
    time: Option<f64>,
    within_tolerance: bool,
}

fn stress_row(
    actor: &Actor,
    clip: &str,
    frames: usize,
    tolerance: f64,
    body: &str,
    garments: &[String],
) -> Result<StressRow, CommandError> {
    let plan = StressPlan {
        body,
        garments: garments.iter().map(String::as_str).collect(),
        clip: Some(clip),
        frames,
        tolerance: tolerance as f32,
    };
    let report = stress(actor, &plan).map_err(|error| {
        errors::invalid("rig_regression_stress", format!("clip {clip:?}: {error}"))
    })?;
    let mut worst: Option<(f32, String, usize, Option<f64>)> = None;
    for row in &report.frames {
        for garment in &row.garments {
            let better = match &worst {
                None => true,
                Some((depth, _, _, _)) => garment.visible_max > *depth,
            };
            if better {
                worst = Some((
                    garment.visible_max,
                    garment.part.clone(),
                    row.index,
                    row.time_seconds,
                ));
            }
        }
    }
    let (depth, part, frame, time) = worst.ok_or_else(|| {
        errors::invalid(
            "rig_regression_stress",
            format!("clip {clip:?}: the stress harness returned no rows"),
        )
    })?;
    Ok(StressRow {
        clip: clip.to_owned(),
        worst_mm: depth as f64 * 1000.0,
        worst_part: part,
        frame,
        time,
        within_tolerance: depth as f64 <= tolerance,
    })
}

// ---------------------------------------------------------------------------
// the verb
// ---------------------------------------------------------------------------

/// One actor's full measurement, default and adapted, with its gates.
struct ActorReport {
    value: Value,
    gates: Vec<GateVerdict>,
}

fn measure_actor(
    params: &RegressionActor,
    frames: usize,
    tolerance: f64,
) -> Result<ActorReport, CommandError> {
    let default_bytes =
        std::fs::read(&params.default).map_err(|error| errors::io(&params.default, error))?;
    let adapted_bytes =
        std::fs::read(&params.adapted).map_err(|error| errors::io(&params.adapted, error))?;
    let default_actor =
        Actor::from_glb(&default_bytes).map_err(|error| actor_read(&params.default, error))?;
    let adapted_actor =
        Actor::from_glb(&adapted_bytes).map_err(|error| actor_read(&params.adapted, error))?;
    let default_palette = palette_names(&default_actor)?;
    let adapted_palette = palette_names(&adapted_actor)?;
    let default_joints = resolve_joints(&default_palette)?;
    let adapted_joints = resolve_joints(&adapted_palette)?;

    // Motion rows: the own walk on the default artifact, the converted clip on
    // the adapted artifact.
    let default_motion = measure_motion(
        &default_actor,
        &params.default_clip,
        frames,
        &default_joints,
        &default_palette,
    )?;
    let adapted_motion = measure_motion(
        &adapted_actor,
        &params.adapted_clip,
        frames,
        &adapted_joints,
        &adapted_palette,
    )?;

    // Stance on both sides; the toes come from the palette's own roles.
    let toes_of = |palette: &[String]| -> Result<Vec<String>, CommandError> {
        let toes: Vec<String> = palette
            .iter()
            .filter(|name| {
                palette_role(name).is_some_and(|role: BoneRole| matches!(role.part, Part::Toe))
            })
            .cloned()
            .collect();
        if toes.is_empty() {
            return Err(errors::invalid(
                "rig_regression_chain",
                "the palette carries no toe joint, so the stance row has no foot",
            ));
        }
        Ok(toes)
    };
    let default_toes = toes_of(&default_palette)?;
    let adapted_toes = toes_of(&adapted_palette)?;
    let default_stance = toe_slide(
        &default_actor,
        &params.default_clip,
        &default_palette,
        &default_toes,
        frames,
    )?;
    let adapted_stance = toe_slide(
        &adapted_actor,
        &params.adapted_clip,
        &adapted_palette,
        &adapted_toes,
        frames,
    )?;

    // Stretch on both sides.
    let default_stretch = edge_stretch(&default_actor, &params.default_clip, frames)?;
    let adapted_stretch = edge_stretch(&adapted_actor, &params.adapted_clip, frames)?;

    // Coverage on both sides.
    let default_coverage =
        clip_role_coverage(&default_actor, &params.default_clip, &default_palette)?;
    let adapted_coverage =
        clip_role_coverage(&adapted_actor, &params.adapted_clip, &adapted_palette)?;

    // Motion inventory on both sides: how many bones each clip drives and how
    // far. An own walk that animates no spine or neck bone is an unanimated
    // baseline, not a healthy one, and the spine gate must say so.
    let default_motion_rows = motion_ranges(&default_actor, &params.default_clip, palette_role)?;
    let adapted_motion_rows = motion_ranges(&adapted_actor, &params.adapted_clip, palette_role)?;
    let default_drives_spine_chain = default_coverage
        .driven_roles
        .iter()
        .any(|role| SPINE_CHAIN_ROLES.contains(role));

    // Travel amplification against the conversion's source, matched by role:
    // both sides classify their own bone names, and the standard role labels
    // are the join key (source `mixamorig:LeftHand` and palette
    // `NPC L Hand [LHnd]` are both `L hand`).
    let mut amplification: Vec<AmplificationRow> = Vec::new();
    if let Some(source_path) = &params.source {
        let source_bytes =
            std::fs::read(source_path).map_err(|error| errors::io(source_path, error))?;
        let source_actor =
            Actor::from_glb(&source_bytes).map_err(|error| actor_read(source_path, error))?;
        let declared = source_clip_name(&source_actor, params.source_clip.as_deref())?;
        let source_motion = motion_ranges(&source_actor, &declared, role_of)?;
        let adapted_by_role: BTreeMap<&str, f64> = adapted_motion_rows
            .rows
            .iter()
            .filter(|row| !row.role.is_empty())
            .map(|row| (row.role.as_str(), row.range_deg))
            .collect();
        for row in &source_motion.rows {
            let Some(adapted_deg) = adapted_by_role.get(row.role.as_str()) else {
                continue;
            };
            if row.range_deg < 1.0 {
                continue;
            }
            amplification.push(AmplificationRow {
                role: row.role.clone(),
                source_deg: row.range_deg,
                adapted_deg: *adapted_deg,
                ratio: adapted_deg / row.range_deg,
            });
        }
        amplification.sort_unstable_by(|a, b| b.ratio.total_cmp(&a.ratio));
    }

    // Rest reconstruction: the adapted artifact's rest must be the default's.
    let reconstruction = palette_reconstruction(&default_actor, &adapted_actor)?;

    // Clothing rows live on the adapted artifact: adapt preserves the mesh, so
    // the own anchor and the converted clip grade on byte-identical geometry.
    let anchor_clip = params
        .stress_anchor_clip
        .clone()
        .unwrap_or_else(|| params.default_clip.clone());
    let mut clothing_rows = Vec::new();
    let mut adapted_clothing: Option<StressRow> = None;
    let mut anchor_clothing: Option<StressRow> = None;
    if !params.garments.is_empty() {
        for clip in [&params.adapted_clip, &anchor_clip] {
            if clothing_rows
                .iter()
                .any(|row: &StressRow| row.clip == *clip)
            {
                continue;
            }
            let row = stress_row(
                &adapted_actor,
                clip,
                frames,
                tolerance,
                &params.body,
                &params.garments,
            )?;
            if *clip == params.adapted_clip {
                adapted_clothing = Some(row.clone());
            }
            if *clip == anchor_clip {
                anchor_clothing = Some(row.clone());
            }
            clothing_rows.push(row);
        }
        for clip in &params.stress_clips {
            if clothing_rows
                .iter()
                .any(|row: &StressRow| row.clip == *clip)
            {
                continue;
            }
            clothing_rows.push(stress_row(
                &adapted_actor,
                clip,
                frames,
                tolerance,
                &params.body,
                &params.garments,
            )?);
        }
    }

    // Gates: the difference decides, so a body-owned defect shows in both rows.
    let adapted_row = format!("{}/adapted", params.name);
    let mut gates = Vec::new();
    gates.push(spine_gate(
        &default_motion.spine,
        &adapted_motion.spine,
        &adapted_row,
        default_drives_spine_chain,
    ));
    if let (Some(default_shoulder), Some(adapted_shoulder)) =
        (&default_motion.shoulder, &adapted_motion.shoulder)
    {
        gates.push(shoulder_level_gate(
            default_shoulder,
            adapted_shoulder,
            &adapted_row,
        ));
    }
    gates.push(clearance_gate(
        &default_motion.clearance,
        &adapted_motion.clearance,
        &adapted_row,
    ));
    gates.push(GateVerdict {
        gate: "stance_median",
        row: adapted_row.clone(),
        pass: adapted_stance["pass"].as_bool() == Some(true),
        detail: format!(
            "adapted median {:.3} m/s (max {:.3}) against bar {STANCE_MEDIAN_BAR:.3}; default \
             median {:.3} (max {:.3})",
            adapted_stance["worst_median_speed"].as_f64().unwrap_or(0.0),
            adapted_stance["max_horizontal_speed"]
                .as_f64()
                .unwrap_or(0.0),
            default_stance["worst_median_speed"].as_f64().unwrap_or(0.0),
            default_stance["max_horizontal_speed"]
                .as_f64()
                .unwrap_or(0.0),
        ),
    });
    gates.push(GateVerdict {
        gate: "rest_reconstruction",
        row: params.name.clone(),
        pass: reconstruction["pass"].as_bool() == Some(true),
        detail: format!(
            "max joint rest delta {:.2e} over extent {:.3} (bound 1e-5·extent), names identical: {}",
            reconstruction["max_rest_delta"].as_f64().unwrap_or(0.0),
            reconstruction["extent"].as_f64().unwrap_or(0.0),
            reconstruction["names_identical"].as_bool().unwrap_or(false),
        ),
    });
    {
        let lost: Vec<&str> = default_coverage
            .driven_roles
            .iter()
            .filter(|role| !adapted_coverage.driven_roles.contains(role))
            .copied()
            .collect();
        gates.push(GateVerdict {
            gate: "role_coverage",
            row: adapted_row.clone(),
            pass: lost.is_empty(),
            detail: format!(
                "adapted drives {} roles ({}) against default {} ({}); lost: {}",
                adapted_coverage.driven_roles.len(),
                adapted_coverage.driven_roles.join(", "),
                default_coverage.driven_roles.len(),
                default_coverage.driven_roles.join(", "),
                match lost.is_empty() {
                    true => "none".to_owned(),
                    false => lost.join(", "),
                },
            ),
        });
    }
    gates.push(stretch_gate(
        default_stretch["max_ratio_edges_at_least_5mm"]
            .as_f64()
            .unwrap_or(0.0),
        adapted_stretch["max_ratio_edges_at_least_5mm"]
            .as_f64()
            .unwrap_or(0.0),
        &adapted_row,
    ));
    if let (Some(anchor), Some(adapted)) = (&anchor_clothing, &adapted_clothing) {
        gates.push(clothing_gate(
            tolerance * 1000.0,
            anchor.worst_mm,
            adapted.worst_mm,
            &anchor.clip,
            &adapted_row,
        ));
    }

    let motion_value = |side: &str,
                        clip: &str,
                        rows: &MotionRows,
                        motion: &MotionMetrics,
                        stance: &Value,
                        stretch: &Value,
                        coverage: &CoverageMetrics| {
        let name = &params.name;
        json!({
            "row": format!("{name}/{side}"),
            "artifact": match side {
                "default" => params.default.to_string_lossy(),
                _ => params.adapted.to_string_lossy(),
            },
            "clip": clip,
            "spine_frame_joint": rows.frame_joint,
            "spine": rows.spine,
            "shoulder": rows.shoulder,
            "clearance": rows.clearance,
            "stance": stance,
            "edge_stretch": stretch,
            "coverage": coverage,
            "motion": motion,
            "drives_spine_chain": match side {
                "default" => default_drives_spine_chain,
                _ => adapted_coverage
                    .driven_roles
                    .iter()
                    .any(|role| SPINE_CHAIN_ROLES.contains(role)),
            },
        })
    };
    let value = json!({
        "name": params.name,
        "default": motion_value(
            "default",
            &params.default_clip,
            &default_motion,
            &default_motion_rows,
            &default_stance,
            &default_stretch,
            &default_coverage,
        ),
        "adapted": motion_value(
            "adapted",
            &params.adapted_clip,
            &adapted_motion,
            &adapted_motion_rows,
            &adapted_stance,
            &adapted_stretch,
            &adapted_coverage,
        ),
        "amplification": match amplification.is_empty() {
            true => json!(null),
            false => json!({
                "source": params.source.as_ref().map(|path| path.to_string_lossy()),
                "rows": amplification,
            }),
        },
        "source_reference": match &params.source {
            Some(path) => reference_block(
                "mixamo-source",
                path,
                params.source_clip.as_deref(),
                frames,
            ),
            None => json!({"status": "not declared"}),
        },
        "native_reference": match &params.native {
            Some(path) => reference_block("native", path, params.native_clip.as_deref(), frames),
            None => json!({"status": "not declared"}),
        },
        "clothing": {
            "artifact": params.adapted.to_string_lossy(),
            "anchor_clip": anchor_clip,
            "tolerance_mm": tolerance * 1000.0,
            "rows": clothing_rows.iter().map(|row| json!({
                "clip": row.clip,
                "worst_visible_mm": row.worst_mm,
                "worst_part": row.worst_part,
                "frame": row.frame,
                "time": row.time,
                "within_tolerance": row.within_tolerance,
            })).collect::<Vec<_>>(),
        },
        "rest_reconstruction": reconstruction,
        "gates": gates,
    });
    Ok(ActorReport { value, gates })
}

/// One reference measurement: the same instruments the judged rows run, on
/// any (artifact, clip) pair, report-only — no gate derives from it. A
/// missing artifact or unmeasurable pair is reported as `unavailable` with
/// the reason; a reference is never faked.
fn reference_block(
    label: &str,
    artifact: &Path,
    declared_clip: Option<&str>,
    frames: usize,
) -> Value {
    let inner = measure_reference_inner(artifact, declared_clip, frames);
    let clip = declared_clip.unwrap_or("(the file's only clip)").to_owned();
    let block = match inner {
        Ok(block) => block,
        Err(error) => {
            return json!({
                "label": label,
                "artifact": artifact.to_string_lossy(),
                "clip": clip,
                "status": "unavailable",
                "reason": error.to_string(),
            });
        }
    };
    // A collapsed figure is not a measurement: peak chain deviations beyond a
    // human-scale bound mean the artifact's pose is debris (the first native
    // decode produced exact-zero translations from frame 1, collapsing the
    // skeleton into the origin). Report the collapse, never the numbers.
    let deviation = block["spine"]["peak_lateral_deviation_mm"]
        .as_f64()
        .unwrap_or(0.0);
    let stretch = block["edge_stretch"]["max_ratio"].as_f64().unwrap_or(0.0);
    if !deviation.is_finite() || deviation > REFERENCE_SANITY_MM || !stretch.is_finite() {
        return json!({
            "label": label,
            "artifact": artifact.to_string_lossy(),
            "clip": block["clip"],
            "status": "degenerate",
            "reason": format!(
                "the posed figure is not human-scale (peak chain deviation {:.3e} mm, worst \
                 edge ratio {:.3e}); the clip's pose collapses the skeleton — suspected decode \
                 defect, see the havok report's binding rows",
                deviation, stretch
            ),
        });
    }
    json!({
        "label": label,
        "artifact": artifact.to_string_lossy(),
        "clip": block["clip"],
        "status": "measured",
        "joint_resolution": block["joint_resolution"],
        "spine": block["spine"],
        "spine_frame_joint": block["spine_frame_joint"],
        "shoulder": block["shoulder"],
        "clearance": block["clearance"],
        "stance": block["stance"],
        "edge_stretch": block["edge_stretch"],
        "coverage": block["coverage"],
        "motion": block["motion"],
        "drives_spine_chain": block["drives_spine_chain"],
    })
}

fn measure_reference_inner(
    artifact: &Path,
    declared_clip: Option<&str>,
    frames: usize,
) -> Result<Value, CommandError> {
    let bytes = std::fs::read(artifact).map_err(|error| errors::io(artifact, error))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| actor_read(artifact, error))?;
    let palette = palette_names(&actor)?;
    let clip = match declared_clip {
        Some(declared) => declared.to_owned(),
        None => source_clip_name(&actor, None)?,
    };
    let joints = resolve_joints(&palette)?;
    let resolution = joint_resolution(&actor, &palette)?;
    let motion = measure_motion(&actor, &clip, frames, &joints, &palette)?;
    let toes: Vec<String> = palette
        .iter()
        .filter(|name| palette_role(name).is_some_and(|role| matches!(role.part, Part::Toe)))
        .cloned()
        .collect();
    let stance = match toes.is_empty() {
        true => json!({"pass": false, "reason": "the palette carries no toe joint"}),
        false => toe_slide(&actor, &clip, &palette, &toes, frames)?,
    };
    let stretch = edge_stretch(&actor, &clip, frames)?;
    let coverage = clip_role_coverage(&actor, &clip, &palette)?;
    let motion_rows = motion_ranges(&actor, &clip, palette_role)?;
    Ok(json!({
        "clip": clip,
        "joint_resolution": resolution,
        "spine": motion.spine,
        "spine_frame_joint": motion.frame_joint,
        "shoulder": motion.shoulder,
        "clearance": motion.clearance,
        "stance": stance,
        "edge_stretch": stretch,
        "coverage": coverage,
        "motion": motion_rows,
        "drives_spine_chain": coverage
            .driven_roles
            .iter()
            .any(|role| SPINE_CHAIN_ROLES.contains(role)),
    }))
}

fn actor_read(path: &Path, error: sculpto_mesh::actor::ActorError) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidRequest,
        "rig_regression_glb_invalid",
        format!("{}: {error}", path.display()),
    )
}

/// Which nodes the palette's names actually bound to, measured once at rest
/// with the whole palette requested. A file that ships duplicate rig copies
/// reports each tie-break here: the chosen node with its skin evidence and
/// the matches it beat. A genuinely ambiguous file refuses instead and the
/// reference block reports `unavailable` with the candidates named.
fn joint_resolution(actor: &Actor, palette: &[String]) -> Result<Value, CommandError> {
    let parts = measurable_parts(actor);
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let sample = actor
        .rig_sample(
            &parts,
            Pose::Rest,
            &palette.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .map_err(crate::measures::actor_error)?;
    let candidate = |joint: &JointCandidate| {
        json!({
            "node": joint.node,
            "is_skin_joint": joint.is_skin_joint,
            "parent_depth": joint.parent_depth,
        })
    };
    Ok(json!({
        "palette_joints": palette.len(),
        "disambiguated": sample
            .disambiguated
            .iter()
            .map(|row| {
                json!({
                    "name": row.name,
                    "chosen": candidate(&row.chosen),
                    "refused": row.refused.iter().map(candidate).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
    }))
}

/// Run the regression over every declared actor.
///
/// # Errors
/// Unreadable artifacts, a palette without the roles the rows need, a missing
/// clip and a stress plan the harness refuses.
pub fn run(input: &Value) -> Result<Value, CommandError> {
    let params: RegressionParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "rig_regression_input",
            error.to_string(),
        )
    })?;
    run_params(&params)
}

/// The typed entry, shared by the registry and the argv surface.
///
/// # Errors
/// As [`run`].
pub fn run_params(params: &RegressionParams) -> Result<Value, CommandError> {
    if params.actors.is_empty() {
        return Err(errors::invalid("actors", "at least one actor is required"));
    }
    if !params.tolerance.is_finite() || params.tolerance < 0.0 {
        return Err(errors::invalid(
            "tolerance",
            format!("{} is not a finite non-negative depth", params.tolerance),
        ));
    }
    let mut actors = Vec::with_capacity(params.actors.len());
    let mut gates = Vec::new();
    for actor in &params.actors {
        let report = measure_actor(actor, params.frames, params.tolerance)?;
        gates.extend(report.gates.iter().cloned());
        actors.push(report.value);
    }
    let mut failed: Vec<String> = gates
        .iter()
        .filter(|gate| !gate.pass)
        .map(|gate| format!("{}:{}", gate.row, gate.gate))
        .collect();
    failed.sort();
    failed.dedup();
    let report = json!({
        "frames": params.frames,
        "tolerance_mm": params.tolerance * 1000.0,
        "bars": {
            "spine_deviation_floor_mm": SPINE_DEVIATION_FLOOR_MM,
            "spine_tilt_floor_deg": SPINE_TILT_FLOOR_DEG,
            "clearance_crossing_floor_mm": CLEARANCE_CROSSING_FLOOR_MM,
            "clearance_margin": CLEARANCE_MARGIN,
            "stance_median_bar_mps": STANCE_MEDIAN_BAR,
            "stretch_relative_bar": RELATIVE_STRETCH,
            "clothing_relative_bar": CLOTHING_RELATIVE,
        },
        "actors": actors,
        "gates": gates,
        "failed_rows": failed,
        "pass": failed.is_empty(),
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

/// The one-line-per-row table the argv surface prints.
pub fn table_lines(report: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for actor in report["actors"].as_array().unwrap_or(&Vec::new()) {
        for side in ["default", "adapted"] {
            let row = &actor[side];
            let spine = &row["spine"];
            let clearance = &row["clearance"];
            let stance = &row["stance"];
            let stretch = &row["edge_stretch"];
            // A clip that animates no spine or neck bone cannot carry a spine
            // reading: the numbers would be rest values printed as if they
            // were measured.
            let drives_spine_chain = row["drives_spine_chain"].as_bool().unwrap_or(true);
            let spine_text = match drives_spine_chain {
                true => format!(
                    "spine lat {:.2}° sag {:.2}° dev {:.1} mm",
                    spine["peak_lateral_tilt_deg"].as_f64().unwrap_or(0.0),
                    spine["peak_sagittal_tilt_deg"].as_f64().unwrap_or(0.0),
                    spine["peak_lateral_deviation_mm"].as_f64().unwrap_or(0.0),
                ),
                false => "spine unanimated — clip animates no spine or neck".to_owned(),
            };
            lines.push(format!(
                "{} {} clip={:?} | {} | clearance {:.2} mm @t={:.3} ({}↔{}) | stance med {:.3} \
                 max {:.3} m/s | stretch ≥5 mm {:.2}x | roles {}",
                actor["name"].as_str().unwrap_or("?"),
                side,
                row["clip"].as_str().unwrap_or("?"),
                spine_text,
                clearance["min_mm"].as_f64().unwrap_or(0.0),
                clearance["time"].as_f64().unwrap_or(0.0),
                clearance["joint"].as_str().unwrap_or("?"),
                clearance["target"].as_str().unwrap_or("?"),
                stance["worst_median_speed"].as_f64().unwrap_or(0.0),
                stance["max_horizontal_speed"].as_f64().unwrap_or(0.0),
                stretch["max_ratio_edges_at_least_5mm"]
                    .as_f64()
                    .unwrap_or(0.0),
                row["coverage"]["driven_roles"]
                    .as_array()
                    .map(|roles| roles.len())
                    .unwrap_or(0),
            ));
        }
        // The motion inventory, plus the amplification when a source is
        // declared: how many bones each clip drives, how far the loudest
        // move, and where the conversion amplifies travel.
        let motion = |side: &str, n: usize| -> String {
            actor[side]["motion"]["rows"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .take(n)
                        .filter_map(|row| {
                            let degrees = row["range_deg"].as_f64()?;
                            (degrees >= 1.0).then(|| {
                                format!(
                                    "{} {}°",
                                    row["role"].as_str().unwrap_or("?"),
                                    degrees.round() as i64
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        let default_driven = actor["default"]["motion"]["driven_bones"]
            .as_u64()
            .unwrap_or(0);
        let adapted_driven = actor["adapted"]["motion"]["driven_bones"]
            .as_u64()
            .unwrap_or(0);
        let mut motion_line = format!(
            "{} motion: default clip drives {} bones (top: {}) | adapted drives {} (top: {})",
            actor["name"].as_str().unwrap_or("?"),
            default_driven,
            motion("default", 3),
            adapted_driven,
            motion("adapted", 4),
        );
        if let Some(rows) = actor["amplification"]["rows"].as_array() {
            let top: Vec<String> = rows
                .iter()
                .take(3)
                .filter_map(|row| {
                    Some(format!(
                        "{} {}° vs {}° = {:.2}x",
                        row["role"].as_str()?,
                        row["adapted_deg"].as_f64()?.round() as i64,
                        row["source_deg"].as_f64()?.round() as i64,
                        row["ratio"].as_f64()?,
                    ))
                })
                .collect();
            if !top.is_empty() {
                motion_line.push_str(&format!(" | amplification vs source: {}", top.join(", ")));
            }
        }
        lines.push(motion_line);
        // The three references, each on one line: mixamo (its own rig),
        // synthesis (our default row, above — labelled here for the
        // three-way read), native (the decoded Skyrim walk on this body).
        for (label, key) in [
            ("reference mixamo", "source_reference"),
            ("reference native", "native_reference"),
        ] {
            let block = &actor[key];
            match block["status"].as_str() {
                Some("measured") => {
                    let spine = &block["spine"];
                    let clearance = &block["clearance"];
                    lines.push(format!(
                        "{} {} reference clip={:?} | spine lat {:.2}° sag {:.2}° dev {:.1} mm | \
                         clearance {:.2} mm @t={:.3} | stance med {:.3} max {:.3} m/s | \
                         stretch ≥5 mm {:.2}x | roles {} (driven {})",
                        actor["name"].as_str().unwrap_or("?"),
                        label,
                        block["clip"].as_str().unwrap_or("?"),
                        spine["peak_lateral_tilt_deg"].as_f64().unwrap_or(0.0),
                        spine["peak_sagittal_tilt_deg"].as_f64().unwrap_or(0.0),
                        spine["peak_lateral_deviation_mm"].as_f64().unwrap_or(0.0),
                        clearance["min_mm"].as_f64().unwrap_or(0.0),
                        clearance["time"].as_f64().unwrap_or(0.0),
                        block["stance"]["worst_median_speed"]
                            .as_f64()
                            .unwrap_or(0.0),
                        block["stance"]["max_horizontal_speed"]
                            .as_f64()
                            .unwrap_or(0.0),
                        block["edge_stretch"]["max_ratio_edges_at_least_5mm"]
                            .as_f64()
                            .unwrap_or(0.0),
                        block["coverage"]["driven_roles"]
                            .as_array()
                            .map(|roles| roles.len())
                            .unwrap_or(0),
                        block["motion"]["driven_bones"].as_u64().unwrap_or(0),
                    ));
                }
                Some(status) => lines.push(format!(
                    "{} {} reference: {status} — {}",
                    actor["name"].as_str().unwrap_or("?"),
                    label,
                    block["reason"]
                        .as_str()
                        .unwrap_or(block["status"].as_str().unwrap_or("?")),
                )),
                _ => {}
            }
        }
        if actor["default"]["drives_spine_chain"].as_bool() == Some(false) {
            lines.push(format!(
                "NOTE: the own walk ({} bones driven) animates no spine or neck channel, so \
                 `default` is an unanimated baseline, not a spine reference — the three-way \
                 comparison has no clip that animates the spine chain on our side until a real \
                 reference walk exists (the Mixamo source played through our converter is not \
                 independent; a native Skyrim walk from the catalogue might be, since `.hkx` \
                 binds by skeleton order and needs no conversion).",
                default_driven,
            ));
        }
        let clothing = &actor["clothing"];
        let rows: Vec<String> = clothing["rows"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|row| {
                format!(
                    "{:?} {:.3} mm on {:?} (f{}){}",
                    row["clip"].as_str().unwrap_or("?"),
                    row["worst_visible_mm"].as_f64().unwrap_or(0.0),
                    row["worst_part"].as_str().unwrap_or("?"),
                    row["frame"].as_u64().unwrap_or(0),
                    match row["within_tolerance"].as_bool() {
                        Some(true) => " pass",
                        Some(false) => " FAIL",
                        None => "",
                    },
                )
            })
            .collect();
        lines.push(format!(
            "{} clothing on adapted artifact (tolerance {:.1} mm): {}",
            actor["name"].as_str().unwrap_or("?"),
            clothing["tolerance_mm"].as_f64().unwrap_or(0.0),
            rows.join(" | "),
        ));
    }
    for gate in report["gates"].as_array().unwrap_or(&Vec::new()) {
        if gate["pass"].as_bool() == Some(false) {
            lines.push(format!(
                "FAIL {}:{} — {}",
                gate["row"].as_str().unwrap_or("?"),
                gate["gate"].as_str().unwrap_or("?"),
                gate["detail"].as_str().unwrap_or(""),
            ));
        }
    }
    lines.push(match report["failed_rows"].as_array() {
        Some(rows) if rows.is_empty() => "rig regression: PASS".to_owned(),
        Some(rows) => format!(
            "rig regression: FAIL ({}): {}",
            rows.len(),
            rows.iter()
                .filter_map(|row| row.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        None => "rig regression: report has no verdict".to_owned(),
    });
    lines
}

fn write_report(path: &Path, report: &Value) -> Result<(), CommandError> {
    let text = serde_json::to_string_pretty(report).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidRequest,
            "rig_regression_report",
            format!("cannot serialize the report: {error}"),
        )
    })?;
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
    }
    std::fs::write(path, format!("{text}\n")).map_err(|error| errors::io(path, error))
}
