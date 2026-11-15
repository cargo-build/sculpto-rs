//! A parametric, symmetric walk cycle generated on the actor's own rig — no
//! borrowed clip. @req DECL0013
//!
//! Left and right sides run the same waveforms half a cycle apart, so foot lift
//! and arm swing are symmetric by construction. The cycle is written as node
//! local TRS tracks and measured through the same forward kinematics that plays
//! a clip, so the reported ranges are what the exported gait actually does.
//!
//! The gait targets the standard NPC palette (the rig the engine binds to). A
//! missing role joint is a named refusal, not a silently skipped limb.

use glam::{DQuat, DVec3};
use sculpto_gltf::pack::{PackWriter, Track};

use super::{Actor, ActorError, Transform, animation, invalid, packing::packed, rig};

/// Tunable walk parameters; amplitudes in degrees, bob in metres.
#[derive(Clone, Copy, Debug)]
pub struct GaitParameters {
    /// Duration of one full cycle (two steps), seconds.
    pub cycle_seconds: f64,
    /// Keyframes including the closing duplicate (first == last for a loop).
    pub samples: usize,
    pub hip_amplitude_deg: f64,
    pub knee_amplitude_deg: f64,
    pub ankle_amplitude_deg: f64,
    pub toe_amplitude_deg: f64,
    pub arm_amplitude_deg: f64,
    pub elbow_amplitude_deg: f64,
    /// Pelvis vertical travel, metres (peak-to-peak).
    pub pelvis_bob: f64,
    pub pelvis_rotation_deg: f64,
    /// Cycle-fraction shift of the knee flexion peak.
    pub knee_phase: f64,
}

impl Default for GaitParameters {
    fn default() -> Self {
        Self {
            cycle_seconds: 1.0,
            samples: 30,
            hip_amplitude_deg: 25.0,
            knee_amplitude_deg: 45.0,
            ankle_amplitude_deg: 15.0,
            toe_amplitude_deg: 20.0,
            arm_amplitude_deg: 20.0,
            elbow_amplitude_deg: 10.0,
            pelvis_bob: 0.03,
            pelvis_rotation_deg: 5.0,
            knee_phase: 0.0,
        }
    }
}

/// Per-joint driven angle range over the cycle, degrees.
#[derive(Clone, Debug)]
pub struct JointRange {
    pub name: String,
    pub node: usize,
    pub role: &'static str,
    pub min_deg: f64,
    pub max_deg: f64,
}

/// World-space symmetry measures read back from the generated cycle.
#[derive(Clone, Debug)]
pub struct GaitMeasures {
    pub pelvis_vertical_range: f64,
    pub foot_lift_left: f64,
    pub foot_lift_right: f64,
    /// Peak-to-peak arm swing: largest angle between the upper-arm world
    /// direction at any two cycle samples, degrees.
    pub arm_swing_left_deg: f64,
    pub arm_swing_right_deg: f64,
}

/// Generated tracks plus the measured report.
#[derive(Clone, Debug)]
pub struct GaitOutcome {
    /// Shared keyframe times, seconds.
    pub times: Vec<f64>,
    /// Node-local rotation tracks: `(node, name, role, per-sample quaternion)`.
    pub rotations: Vec<(usize, String, &'static str, Vec<DQuat>)>,
    /// Node-local translation tracks: `(node, name, role, per-sample vector)`.
    pub translations: Vec<(usize, String, &'static str, Vec<DVec3>)>,
    pub ranges: Vec<JointRange>,
    pub measures: GaitMeasures,
}

/// One driven joint: its rest local transform and the axis its waveform uses.
struct Drive {
    node: usize,
    name: String,
    role: &'static str,
    axis: DVec3,
    rest_rotation: DQuat,
    /// Phase offset in cycles (right side = 0.5).
    offset: f64,
}

const TAU: f64 = std::f64::consts::TAU;

/// Resolve the standard palette, generate the cycle and measure it.
///
/// # Errors
/// A missing or non-unique role joint, a static-matrix role joint, or invalid
/// parameters (non-finite, fewer than two samples, non-positive cycle).
pub fn gait(actor: &Actor, params: &GaitParameters) -> Result<GaitOutcome, ActorError> {
    validate(params)?;
    let rest_worlds = animation::compose_worlds(actor, &actor.transforms)?;

    // (role, exact node name, side offset). Left leads, right is half a cycle back.
    let spec: &[(&'static str, &str, f64)] = &[
        ("hip_left", "NPC L Thigh [LThg]", 0.0),
        ("hip_right", "NPC R Thigh [RThg]", 0.5),
        ("knee_left", "NPC L Calf [LClf]", 0.0),
        ("knee_right", "NPC R Calf [RClf]", 0.5),
        ("ankle_left", "NPC L Foot [Lft ]", 0.0),
        ("ankle_right", "NPC R Foot [Rft ]", 0.5),
        ("toe_left", "NPC L Toe0 [LToe]", 0.0),
        ("toe_right", "NPC R Toe0 [RToe]", 0.5),
        ("shoulder_left", "NPC L UpperArm [LUar]", 0.0),
        ("shoulder_right", "NPC R UpperArm [RUar]", 0.5),
        ("elbow_left", "NPC L Forearm [LLar]", 0.0),
        ("elbow_right", "NPC R Forearm [RLar]", 0.5),
    ];
    let names: Vec<&str> = spec.iter().map(|(_, name, _)| *name).collect();
    let resolved = rig::resolve(actor, &names)?;
    let pelvis = rig::resolve(actor, &["NPC Pelvis [Pelv]"])?;
    let pelvis_node = pelvis[0].node;

    let mut drives = Vec::new();
    for ((role, _, offset), resolved) in spec.iter().zip(resolved) {
        let Transform::Trs { rotation, .. } = &actor.transforms[resolved.node] else {
            return Err(invalid(
                format!("gait joint {:?}", resolved.name),
                "static matrix node cannot be driven",
            ));
        };
        drives.push(Drive {
            node: resolved.node,
            name: resolved.name,
            role,
            axis: DVec3::X,
            rest_rotation: *rotation,
            offset: *offset,
        });
    }
    let Transform::Trs {
        translation: pelvis_translation,
        rotation: pelvis_rotation,
        ..
    } = &actor.transforms[pelvis_node]
    else {
        return Err(invalid(
            "gait pelvis",
            "static matrix node cannot be driven",
        ));
    };
    // World-vertical in the pelvis's parent frame, so the bob is truly vertical
    // regardless of the pelvis's own rest orientation.
    let up_local = match actor.parents[pelvis_node] {
        Some(parent) => rest_worlds[parent].inverse().transform_vector3(DVec3::Y),
        None => DVec3::Y,
    };
    // Pelvis yaw is about the local axis that maps to world-up.
    let pelvis_yaw_axis = rest_worlds[pelvis_node]
        .inverse()
        .transform_vector3(DVec3::Y)
        .normalize();

    let samples = params.samples;
    let times: Vec<f64> = (0..samples)
        .map(|i| params.cycle_seconds * i as f64 / (samples - 1) as f64)
        .collect();
    let phases: Vec<f64> = (0..samples)
        .map(|i| i as f64 / (samples - 1) as f64)
        .collect();

    let angle_of = |role: &str, p: f64| -> f64 {
        let wave = (TAU * p).sin();
        let raised = 0.5 - 0.5 * (TAU * p).cos();
        match role {
            "hip_left" | "hip_right" => params.hip_amplitude_deg * wave,
            "knee_left" | "knee_right" => {
                params.knee_amplitude_deg * (0.5 - 0.5 * (TAU * (p - params.knee_phase)).cos())
            }
            "ankle_left" | "ankle_right" => params.ankle_amplitude_deg * wave,
            "toe_left" | "toe_right" => params.toe_amplitude_deg * wave.max(0.0),
            "shoulder_left" | "shoulder_right" => -params.arm_amplitude_deg * wave,
            "elbow_left" | "elbow_right" => params.elbow_amplitude_deg * raised,
            _ => 0.0,
        }
    };

    // Per-sample driven angles, then quaternion/translation tracks.
    let mut angles: Vec<Vec<f64>> = vec![Vec::with_capacity(samples); drives.len()];
    let mut rotations: Vec<(usize, String, &'static str, Vec<DQuat>)> = Vec::new();
    for (index, drive) in drives.iter().enumerate() {
        let mut quats = Vec::with_capacity(samples);
        for &phase in &phases {
            let p = (phase + drive.offset).rem_euclid(1.0);
            let angle = angle_of(drive.role, p);
            angles[index].push(angle);
            let delta = DQuat::from_axis_angle(drive.axis, angle.to_radians());
            quats.push((drive.rest_rotation * delta).normalize());
        }
        rotations.push((drive.node, drive.name.clone(), drive.role, quats));
    }
    // Pelvis yaw track.
    let mut pelvis_quats = Vec::with_capacity(samples);
    let mut pelvis_yaw_angles = Vec::with_capacity(samples);
    for &phase in &phases {
        let angle = params.pelvis_rotation_deg * (TAU * phase).sin();
        pelvis_yaw_angles.push(angle);
        let delta = DQuat::from_axis_angle(pelvis_yaw_axis, angle.to_radians());
        pelvis_quats.push((*pelvis_rotation * delta).normalize());
    }
    rotations.push((
        pelvis_node,
        "NPC Pelvis [Pelv]".to_owned(),
        "pelvis_yaw",
        pelvis_quats,
    ));
    // Pelvis bob translation track.
    let mut bob_heights = Vec::with_capacity(samples);
    let mut pelvis_trans = Vec::with_capacity(samples);
    for &phase in &phases {
        let bob = params.pelvis_bob * (0.5 - 0.5 * (2.0 * TAU * phase).cos());
        bob_heights.push(bob);
        pelvis_trans.push(*pelvis_translation + up_local * bob);
    }
    let translations = vec![(
        pelvis_node,
        "NPC Pelvis [Pelv]".to_owned(),
        "pelvis_bob",
        pelvis_trans,
    )];

    let ranges = drives
        .iter()
        .enumerate()
        .map(|(index, drive)| JointRange {
            name: drive.name.clone(),
            node: drive.node,
            role: drive.role,
            min_deg: angles[index].iter().cloned().fold(f64::INFINITY, f64::min),
            max_deg: angles[index]
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max),
        })
        .chain(std::iter::once(JointRange {
            name: "NPC Pelvis [Pelv]".to_owned(),
            node: pelvis_node,
            role: "pelvis_yaw",
            min_deg: pelvis_yaw_angles
                .iter()
                .cloned()
                .fold(f64::INFINITY, f64::min),
            max_deg: pelvis_yaw_angles
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max),
        }))
        .collect();

    let measures = measure(actor, &times, &rotations, &translations, pelvis_node)?;

    Ok(GaitOutcome {
        times,
        rotations,
        translations,
        ranges,
        measures,
    })
}

/// Write the generated cycle as a clip inserted at the walk slot (index 1),
/// preserving the idle clip and shifting later clips back. Returns the GLB and
/// the inserted clip index. @req DECL0013
///
/// # Errors
/// Any container refusal of the append-only packer.
pub fn write_gait_clip(
    base: &[u8],
    outcome: &GaitOutcome,
    clip: &str,
) -> Result<(Vec<u8>, usize), ActorError> {
    let times: Vec<f32> = outcome.times.iter().map(|time| *time as f32).collect();
    let mut tracks: Vec<Track> = outcome
        .rotations
        .iter()
        .map(|(node, _, _, quats)| Track::Rotation {
            node: *node,
            values: quats
                .iter()
                .map(|quat| quat.to_array().map(|v| v as f32))
                .collect(),
        })
        .collect();
    tracks.extend(outcome.translations.iter().map(|(node, _, _, vectors)| {
        Track::Translation {
            node: *node,
            values: vectors
                .iter()
                .map(|vector| vector.to_array().map(|v| v as f32))
                .collect(),
        }
    }));
    let mut writer = PackWriter::from_glb(base).map_err(packed)?;
    let index = writer
        .insert_animation(1, clip, &times, &tracks)
        .map_err(packed)?;
    Ok((writer.finish().map_err(packed)?, index))
}

fn validate(params: &GaitParameters) -> Result<(), ActorError> {
    let finite = [
        params.cycle_seconds,
        params.hip_amplitude_deg,
        params.knee_amplitude_deg,
        params.ankle_amplitude_deg,
        params.toe_amplitude_deg,
        params.arm_amplitude_deg,
        params.elbow_amplitude_deg,
        params.pelvis_bob,
        params.pelvis_rotation_deg,
        params.knee_phase,
    ]
    .into_iter()
    .all(|value| value.is_finite());
    if !finite || params.cycle_seconds <= 0.0 || params.samples < 2 {
        return Err(invalid(
            "gait parameters",
            "need finite values, a positive cycle and at least two samples",
        ));
    }
    Ok(())
}

/// Read the generated cycle back through forward kinematics and measure the
/// world-space symmetry the acceptance gates name.
fn measure(
    actor: &Actor,
    times: &[f64],
    rotations: &[(usize, String, &'static str, Vec<DQuat>)],
    translations: &[(usize, String, &'static str, Vec<DVec3>)],
    pelvis_node: usize,
) -> Result<GaitMeasures, ActorError> {
    let find =
        |name: &str| -> Result<usize, ActorError> { Ok(rig::resolve(actor, &[name])?[0].node) };
    let foot_left = find("NPC L Foot [Lft ]")?;
    let foot_right = find("NPC R Foot [Rft ]")?;
    let arm_left = find("NPC L UpperArm [LUar]")?;
    let arm_right = find("NPC R UpperArm [RUar]")?;
    let elbow_left = find("NPC L Forearm [LLar]")?;
    let elbow_right = find("NPC R Forearm [RLar]")?;

    let mut pelvis_y = Vec::with_capacity(times.len());
    let mut foot_y = [
        Vec::with_capacity(times.len()),
        Vec::with_capacity(times.len()),
    ];
    let mut arm_dir = [
        Vec::with_capacity(times.len()),
        Vec::with_capacity(times.len()),
    ];
    for sample in 0..times.len() {
        let mut transforms = actor.transforms.clone();
        for (node, _, _, quats) in rotations {
            if let Transform::Trs { rotation, .. } = &mut transforms[*node] {
                *rotation = quats[sample];
            }
        }
        for (node, _, _, vectors) in translations {
            if let Transform::Trs { translation, .. } = &mut transforms[*node] {
                *translation = vectors[sample];
            }
        }
        let worlds = animation::compose_worlds(actor, &transforms)?;
        pelvis_y.push(worlds[pelvis_node].transform_point3(DVec3::ZERO).y);
        foot_y[0].push(worlds[foot_left].transform_point3(DVec3::ZERO).y);
        foot_y[1].push(worlds[foot_right].transform_point3(DVec3::ZERO).y);
        arm_dir[0].push(arm_direction(&worlds, arm_left, elbow_left));
        arm_dir[1].push(arm_direction(&worlds, arm_right, elbow_right));
    }
    let range = |values: &[f64]| {
        values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - values.iter().cloned().fold(f64::INFINITY, f64::min)
    };
    Ok(GaitMeasures {
        pelvis_vertical_range: range(&pelvis_y),
        foot_lift_left: range(&foot_y[0]),
        foot_lift_right: range(&foot_y[1]),
        arm_swing_left_deg: swing_peak_to_peak(&arm_dir[0]),
        arm_swing_right_deg: swing_peak_to_peak(&arm_dir[1]),
    })
}

/// Unit world direction of the upper arm (shoulder toward elbow).
fn arm_direction(worlds: &[glam::DMat4], arm: usize, elbow: usize) -> DVec3 {
    let shoulder = worlds[arm].transform_point3(DVec3::ZERO);
    let elbow = worlds[elbow].transform_point3(DVec3::ZERO);
    (elbow - shoulder).normalize_or_zero()
}

/// Peak-to-peak arm swing: the largest angle between the upper-arm direction at
/// any two samples of the cycle. Frame-independent, so it reads the same whether
/// the arm swings in flexion or abduction.
fn swing_peak_to_peak(directions: &[DVec3]) -> f64 {
    let mut max = 0.0_f64;
    for (i, a) in directions.iter().enumerate() {
        for b in &directions[i + 1..] {
            let angle = a.dot(*b).clamp(-1.0, 1.0).acos().to_degrees();
            if angle > max {
                max = angle;
            }
        }
    }
    max
}
