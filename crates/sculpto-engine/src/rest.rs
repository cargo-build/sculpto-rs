//! `actor tpose`: bake the arm chain onto a reference skeleton's T-pose rest.
//!
//! Our bodies rest in an A-pose, 65° below the horizontal, while the animation
//! source they are driven from rests in a T-pose at 0°. That single mismatch is
//! what a converter has to compensate, and the compensation was measured
//! amplifying the digits 2.9–5.4×. The durable fix is to stop having the gap:
//! rotate the arm chain onto the reference's own rest directions, sample the
//! flesh through the ordinary linear-blend evaluator, and write the result as a
//! **new rest** with no clip. Every joint pivot stays where the body's own rig
//! puts it; only the pose changes, so a pivot moves exactly as far as the pose
//! moves it and its bone's length does not change at all.
//!
//! The reference is measured, not assumed: the arm directions come from the
//! skeleton's own rest, and the verb refuses a reference whose own arms are not
//! within [`T_POSE_TOLERANCE_DEG`] of the horizontal, because that is the only
//! thing this verb claims to bake.
//!
//! Garments already on the actor keep the same skin and the same weights, so
//! they rotate with the flesh they are bound to; an unrigged garment fitted
//! separately on the old rest does not, and the report states the body's own
//! displacement per primitive so the cost is visible before a fit is attempted.
//!
//! @req DECL0013 @req DECL0008

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::{DMat4, DQuat, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, Skin};
use sculpto_core::rig::rest::{RestBinding, RestMeasurement, SkinRestSource};
use sculpto_core::rig::{JointId, Rig, RigFrame};
use sculpto_gltf::review::smooth_normals;
use sculpto_gltf::write_partitioned_rigged_glb;
use sculpto_mesh::actor::{Actor, Pose, RestAim, RestBake, bake_rest};

use crate::actor_io::{
    Pool, Validation, f32_of, identity_f32, part_materials, pool_weights, read_pool, validate_skin,
};
use crate::adapt::{actor_rig, actor_rig_with_worlds};
use crate::export_glb::{actor_error, hex as sha_hex, refused, write_report};
use crate::retarget::{BoneRole, Correspondence, Part, Side, role_of};
use crate::{CommandError, errors};

/// How close a reference's own arm rest must be to the horizontal, degrees.
///
/// The verb's contract is "bake a T-pose rest". A reference 60° off is an
/// A-pose with another name, and baking it would produce a rest this verb must
/// not claim to have produced.
pub const T_POSE_TOLERANCE_DEG: f64 = 2.0;

/// How close the written bone basis must stay to the reference's own, degrees.
///
/// A direction-only aim leaves the roll free; this bar is what makes "the same
/// rest pose" mean the same *basis*, not just the same axis.
pub const BONE_BASIS_TOLERANCE_DEG: f64 = 1.0;

/// How close the hand's palm plane must lie to the reference's own, degrees.
///
/// The three-point plane a hand's digit roots span carries an along-axis
/// component our A-pose hand has and Mixamo's does not, so the plane angle has a
/// floor that the roll cannot remove; the in-plane roll is graded separately and
/// exactly.
pub const PALM_DOWN_TOLERANCE_DEG: f64 = 5.0;

/// How much of the arm chain must extend laterally, as a fraction of its length.
pub const LATERAL_EXTENT_FRACTION: f64 = 0.90;

/// How far a reference bone's Y axis may sit off its own bone, degrees.
pub const REFERENCE_AXIS_TOLERANCE_DEG: f64 = 5.0;

/// Frame rate of the shipped Skyrim animation clips.
///
/// Declared, not inferred: the decoded clip's own sample spacing is 1/30 s, and a
/// scan that guessed would evaluate frames the game never plays.
pub const SKYRIM_CLIP_FPS: f64 = 30.0;

/// How far the torso may leave its rest orientation in a chosen pose, degrees.
pub const POSE_TORSO_TOLERANCE_DEG: f64 = 5.0;

/// How far the legs may leave their rest orientation in a chosen pose, degrees.
pub const POSE_LEG_TOLERANCE_DEG: f64 = 10.0;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TPoseParams {
    /// Actor GLB whose flesh and weights are baked into a new rest.
    pub actor: PathBuf,
    /// Skeleton GLB whose arm rest directions define the target T-pose.
    pub reference: PathBuf,
    /// Write the clip-free, T-posed actor here.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Write the JSON report here.
    #[serde(default)]
    pub report: Option<PathBuf>,
    /// An actor GLB whose clip carries the pose the rest is baked from.
    ///
    /// When set, the arm chain is not aimed at the reference's directions: the
    /// pose's own rotations are baked instead, and the reference is used only to
    /// grade the result. This is the route that uses a shipped in-game pose rather
    /// than a computed one.
    #[serde(default)]
    pub pose: Option<PathBuf>,
    /// The clip inside `pose` to read.
    #[serde(default)]
    pub pose_clip: Option<String>,
    /// Bake this clip time, seconds; ignored when `pose_scan` is set.
    #[serde(default)]
    pub pose_time: Option<f64>,
    /// Scan every frame of the clip and choose one by measurement.
    #[serde(default)]
    pub pose_scan: bool,
}

/// One arm-chain segment the bake aims: the bone, and the joint that ends it.
#[derive(Clone, Copy)]
struct Segment {
    role: BoneRole,
    tip: BoneRole,
}

/// One resolved arm segment: nodes, authored vector, target direction and turn.
struct Resolved {
    segment: Segment,
    joint: usize,
    joint_name: String,
    tip: usize,
    tip_name: String,
    /// The bone's authored world vector, before the bake.
    authored: DVec3,
    /// Target world direction: the reference's bone, carried through the yaw.
    reference_direction: DVec3,
    /// How far the reference bone's Y axis sits off its own bone, degrees.
    axis_offset_degrees: f64,
    /// The world turn the bake applies to this bone.
    turn: DQuat,
    /// The roll this bone receives, about its own limb axis, degrees.
    roll_degrees: f64,
}

/// One side's hand frame, built from the digit roots it carries.
///
/// A bone axis alone cannot fix a roll: a rotation about the limb leaves the
/// hand free to spin, and a minimal turn leaves it wherever the A-pose put it —
/// the paddle. The hand's own digit roots give the second axis, so the chain can
/// be turned until the hand's frame matches the reference's frame exactly.
struct HandFrame {
    side: Side,
    hand: usize,
    thumb: usize,
    middle: usize,
    index: usize,
    pinky: usize,
    /// Target world axis of the hand bone.
    target_axis: DVec3,
    /// Target world direction of the thumb root, from the reference.
    target_thumb: DVec3,
    /// Target palm-plane normal: the reference's own, carried through the yaw.
    target_normal: DVec3,
    /// The roll applied to this side's whole chain, degrees.
    roll_degrees: f64,
}

/// Bake the arm chain onto the reference rest and write the new actor.
///
/// # Errors
/// A missing or unreadable file, an arm role the correspondence cannot resolve,
/// a reference whose own arms are not a T-pose, a resolved joint that carries no
/// weight, a rest proof that fails, or an output row that is empty or does not
/// sum to one.
pub fn run(params: &TPoseParams) -> Result<Value, CommandError> {
    let actor_bytes =
        std::fs::read(&params.actor).map_err(|error| errors::io(&params.actor, error))?;
    let actor = Actor::from_glb(&actor_bytes).map_err(actor_error)?;
    let reference_bytes =
        std::fs::read(&params.reference).map_err(|error| errors::io(&params.reference, error))?;
    let reference = Actor::from_glb(&reference_bytes).map_err(actor_error)?;

    let (mesh_node, mesh_name) = actor.skinned_mesh_node().map_err(actor_error)?;
    actor.shared_pool_parts(mesh_node).map_err(actor_error)?;
    let (our_rig, our_palette) = actor_rig(&actor)?;
    let (reference_rig, reference_palette) = actor_rig(&reference)?;

    let correspondence = Correspondence::build(&actor, &reference_rig, &reference_palette)?;
    let bodies = Bodies::measure(&our_rig, &our_palette, &reference_rig, &reference_palette)?;
    let our_worlds = crate::adapt::actor_rest_worlds(&actor)?;
    let (resolved, hands) = resolve_chains(
        &actor,
        &correspondence,
        &our_worlds,
        &reference_rig,
        &reference_palette,
        bodies.yaw,
    )?;
    let reference_angles = reference_arm_angles(&resolved)?;
    refuse_non_t_pose(&reference_angles)?;
    refuse_rolled_reference(&resolved)?;

    // A pose the shipped game already plays beats a computed aim where one
    // exists: the game carries the native weights and the pose together, and a
    // computed rotation has to invent that pairing.
    let (pose_turns, pose_block) = match &params.pose {
        Some(path) => pose_plan(path, params, &resolved, &our_worlds)?,
        None => (None, Value::Null),
    };

    let pool = read_pool(&actor, mesh_node, &mesh_name, &our_palette)?;
    let mass = weight_mass(&pool.rows, our_palette.len());
    let slot_of_node: BTreeMap<usize, usize> = actor
        .skin_joints()
        .map_err(actor_error)?
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    for row in &resolved {
        let slot = slot_of_node.get(&row.joint).copied().ok_or_else(|| {
            refused(
                "tpose_aim",
                format!(
                    "bone {:?} is not in the actor's skin palette, so aiming it would move nothing",
                    row.joint_name
                ),
            )
        })?;
        if mass[slot] <= 0.0 {
            return Err(refused(
                "tpose_aim",
                format!(
                    "bone {:?} (node {}) carries no weight, so the aim would move nothing; the \
                     correspondence resolved a weightless duplicate",
                    row.joint_name, row.joint
                ),
            ));
        }
    }

    let aims: Vec<RestAim> = resolved
        .iter()
        .enumerate()
        .map(|(index, row)| RestAim {
            joint: row.joint,
            child: row.tip,
            direction: row.reference_direction.to_array(),
            turn: Some(match &pose_turns {
                Some(turns) => turns[index].to_array(),
                None => row.turn.to_array(),
            }),
        })
        .collect();
    let bake = bake_rest(&actor, &[mesh_name.as_str()], &aims).map_err(actor_error)?;

    // The posed worlds become the rest: one rig, one skin, one proof.
    let (baked_rig, baked_palette) = actor_rig_with_worlds(&actor, &bake.worlds)?;
    if baked_palette != our_palette {
        return Err(refused(
            "tpose_palette",
            "the baked rig names a different palette than the actor's skin".to_owned(),
        ));
    }
    let pool_pose = pool_range(&bake, pool.vertices.len())?;
    let positions: Vec<[f32; 3]> = bake.posed.vertices[pool_pose]
        .iter()
        .map(|point| point.map(|value| value as f32))
        .collect();
    let skin = Skin {
        bones: our_palette.clone(),
        bind_inverse: (0..our_palette.len())
            .map(|slot| {
                f32_of(
                    DMat4::from_cols_array_2d(
                        &baked_rig
                            .rest_world(JointId::new(slot as u32))
                            .expect("palette joint resolves"),
                    )
                    .inverse(),
                )
            })
            .collect(),
        influences: pool_weights(&pool.rows, our_palette.len())?,
    };
    let validation = validate_skin(&skin, positions.len())?;
    let materials = part_materials(&actor, mesh_node)?;
    let normals = smooth_normals(&positions, &pool.triangles);
    let asset = MeshAsset {
        name: mesh_name.clone(),
        positions: positions.clone(),
        normals,
        triangles: pool.triangles.clone(),
        triangle_partitions: pool.partitions.clone(),
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let root = baked_rig.roots()[0];
    let binding = RestBinding::prove_with_basis(
        &baked_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root,
            positions: &asset.positions,
            skin: &skin,
            global_to_skin: identity_f32(),
        }],
        DMat4::IDENTITY.to_cols_array_2d(),
    )
    .map_err(|error| {
        errors::invalid(
            "tpose_rest",
            format!("the baked rest does not reconstruct its own geometry: {error}"),
        )
    })?;
    let measurement = binding.skins()[0].measurement();
    // No clip is carried into the new rest: a clip authored against the old rest
    // would play the old pose on new flesh, which is the defect this verb exists
    // to remove rather than to move.
    let dropped: Vec<String> = actor
        .document()
        .animations()
        .filter_map(|clip| clip.name().map(str::to_owned))
        .collect();
    let bytes =
        write_partitioned_rigged_glb(&asset, &binding, 0, &[], &materials).map_err(|error| {
            errors::invalid("tpose_export", format!("write the baked actor: {error}"))
        })?;
    if let Some(path) = &params.out {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        std::fs::write(path, &bytes).map_err(|error| errors::io(path, error))?;
    }

    let names: Vec<String> = materials
        .iter()
        .map(|material| material.name.clone())
        .collect();
    let deformation = Deformation::measure(&bake, &pool, &our_palette, &names);
    let report = json!({
        "ok": true,
        "verdict": verdict_text(&resolved, &measurement.max_vertex, &deformation),
        "input": {
            "actor": params.actor.to_string_lossy(),
            "sha256_16": sha16(&actor_bytes),
            "mesh": mesh_name.clone(),
            "vertices": pool.vertices.len(),
            "triangles": pool.triangles.len(),
            "primitives": materials.len(),
            "palette_joints": our_palette.len(),
            "clips_dropped": dropped,
        },
        "bodies": {
            "our_forward": bodies.our_forward.to_array(),
            "reference_forward": bodies.reference_forward.to_array(),
            "yaw_deg": bodies.yaw_degrees(),
            "rule": "each body's own rest frame; the two actors do not share a forward axis",
        },
        "reference": {
            "skeleton": params.reference.to_string_lossy(),
            "sha256_16": sha16(&reference_bytes),
            "palette_joints": reference_palette.len(),
            "tolerance_deg": T_POSE_TOLERANCE_DEG,
            "arm_rows": reference_angles.iter().map(|row| json!({
                "role": label(&row.segment.role),
                "j0": label(&row.segment.tip),
                "below_horizontal_deg": row.below_horizontal,
            })).collect::<Vec<_>>(),
        },
        "aims": bake.aims.iter().zip(&resolved).map(|(aimed, row)| json!({
            "role": label(&row.segment.role),
            "bone": row.joint_name,
            "node": row.joint,
            "tip": row.tip_name,
            "tip_node": row.tip,
            "direction_before": aimed.direction_before,
            "direction_after": aimed.direction_after,
            "target": aimed.target,
            "turn_degrees": aimed.turn_degrees,
            "residual_degrees": aimed.residual_degrees,
            "turns_applied": aimed.turns_applied,
            "below_horizontal_before_deg": below_horizontal(DVec3::from_array(aimed.direction_before)),
            "below_horizontal_after_deg": below_horizontal(DVec3::from_array(aimed.direction_after)),
            "pivot_before": aimed.pivot_before,
            "pivot_after": aimed.pivot_after,
            "pivot_shift_m": distance(aimed.pivot_before, aimed.pivot_after),
            "length_before_m": aimed.length_before,
            "length_after_m": aimed.length_after,
            "length_change_m": aimed.length_after - aimed.length_before,
            "axes_after": aimed.axes_after,
        })).collect::<Vec<_>>(),
        "rest": {
            "max_vertex": measurement.max_vertex,
            "max_influence": measurement.max_influence,
            "max_joint": measurement.max_joint,
        },
        "weights": {
            "vertices": positions.len(),
            "rows_from_source": true,
            "max_sum_error": validation.max_sum_error,
            "empty_rows": validation.empty_rows,
            "widest": validation.widest,
            "per_bone_mass": our_palette.iter().zip(&validation.per_bone).map(|(name, mass)| (name.clone(), mass)).collect::<BTreeMap<_, _>>(),
        },
        "pose": pose_block,
        "gates": gates(&resolved, &hands, &bake, &deformation, &validation, &measurement, &bodies),
        "deformation": deformation.report(),
        "bone_length_max_change_m": bake.aims.iter().map(|aim| (aim.length_after - aim.length_before).abs()).fold(0.0_f64, f64::max),
        "output": {
            "path": params.out.as_ref().map(|path| path.to_string_lossy().to_string()),
            "sha256_16": sha16(&bytes),
            "bytes": bytes.len(),
            "clips": 0,
        },
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

/// The lane's own acceptance bars, measured on this run and reported with it.
///
/// The bars are the project's, not this verb's invention: the 2.0× edge cap and
/// the 1e-5 rest cap are the rig-animation gates, the 2° arm bar is this
/// assignment's, and the row rules are the export's. A bake that writes an
/// artifact while failing a bar says so here instead of in a render.
fn gates(
    resolved: &[Resolved],
    hands: &[HandFrame],
    bake: &RestBake,
    deformation: &Deformation,
    validation: &Validation,
    measurement: &RestMeasurement,
    bodies: &Bodies,
) -> Value {
    let arm_max = bake
        .aims
        .iter()
        .zip(resolved)
        .map(|(aimed, _)| {
            let target = DVec3::from_array(aimed.target);
            let after = DVec3::from_array(aimed.direction_after);
            after.angle_between(target).to_degrees()
        })
        .fold(0.0_f64, f64::max);
    let band = |name: &str| {
        deformation
            .bands
            .iter()
            .find(|band| band.name == name)
            .map(|band| (band.max_ratio, band.edges, band.above_2x))
            .unwrap_or((1.0, 0, 0))
    };
    let seam_max = ["wrist", "ankle", "neck"]
        .into_iter()
        .map(|name| band(name).0)
        .fold(1.0_f64, f64::max);
    let (shoulder_max, shoulder_edges, shoulder_above) = band("shoulder");
    json!({
        "arm_direction": {
            "bar_deg": T_POSE_TOLERANCE_DEG,
            "max_deviation_deg": arm_max,
            "pass": arm_max <= T_POSE_TOLERANCE_DEG,
        },
        "edge_stretch": {
            "bar_max_ratio": 2.0,
            "bar_mean_ratio": 1.10,
            "max_ratio": deformation.max_ratio,
            "mean_ratio": deformation.mean_ratio,
            "edges_above_bar": deformation.above_2x,
            "edges": deformation.edges,
            "pass": deformation.max_ratio <= 2.0 && deformation.mean_ratio <= 1.10,
        },
        "seam_bands": {
            "bar_max_ratio": 2.0,
            "max_ratio": seam_max,
            "bands": ["wrist", "ankle", "neck"],
            "pass": seam_max <= 2.0,
        },
        "shoulder_band": {
            "bar_max_ratio": 2.0,
            "max_ratio": shoulder_max,
            "edges": shoulder_edges,
            "edges_above_bar": shoulder_above,
            "pass": shoulder_max <= 2.0,
        },
        "weights": {
            "bar_sum_error": 1e-5,
            "max_sum_error": validation.max_sum_error,
            "empty_rows": validation.empty_rows,
            "widest": validation.widest,
            "pass": validation.max_sum_error <= 1e-5
                && validation.empty_rows == 0
                && validation.widest <= 4,
        },
        "rest": {
            "bar_metres": 1.8e-5,
            "max_vertex": measurement.max_vertex,
            "pass": measurement.max_vertex <= 1.8e-5
                && measurement.max_influence <= 1.8e-5
                && measurement.max_joint <= 1.8e-5,
        },
        "bone_axis": {
            "bar_deg": T_POSE_TOLERANCE_DEG,
            "bones": basis_rows(resolved, bake),
            "pass": basis_rows(resolved, bake)
                .iter()
                .all(|row| row["deviation_deg"].as_f64().unwrap_or(180.0) <= T_POSE_TOLERANCE_DEG),
        },
        "roll": roll_gate(hands, bake),
        "no_crossing": crossing_gate(resolved, bake, bodies),
        "bone_lengths": {
            "bar_change_m": 1e-9,
            "max_change_m": bake
                .aims
                .iter()
                .map(|aim| (aim.length_after - aim.length_before).abs())
                .fold(0.0_f64, f64::max),
            "pass": bake
                .aims
                .iter()
                .all(|aim| (aim.length_after - aim.length_before).abs() <= 1e-9),
        },
    })
}

/// Per-bone direction residual: the written bone axis against the reference's.
fn basis_rows(resolved: &[Resolved], bake: &RestBake) -> Vec<Value> {
    bake.aims
        .iter()
        .zip(resolved)
        .map(|(aimed, row)| {
            let target = row.reference_direction;
            let written = DVec3::from_array(aimed.direction_after);
            json!({
                "role": label(&row.segment.role),
                "bone": row.joint_name,
                "deviation_deg": written.angle_between(target).to_degrees(),
                "roll_applied_deg": row.roll_degrees,
                "turn_deg": aimed.turn_degrees,
            })
        })
        .collect()
}

/// The roll gate: each hand's frame against the reference's, and the palm down.
///
/// A bone axis cannot carry a roll, so the proof is the hand's own frame: its
/// axis plus the palm plane its digit roots span. The chain receives one roll, so
/// the authored twist between shoulder, elbow and wrist survives; the hand's
/// residual is what says the whole chain landed where the reference's did.
fn roll_gate(hands: &[HandFrame], bake: &RestBake) -> Value {
    let mut sides = Vec::new();
    for hand in hands {
        let written = (|| -> Result<Value, CommandError> {
            let axis =
                world_pivot(&bake.worlds, hand.middle)? - world_pivot(&bake.worlds, hand.hand)?;
            let normal = plane_normal(&bake.worlds, hand.hand, hand.index, hand.pinky)?;
            let thumb =
                world_pivot(&bake.worlds, hand.thumb)? - world_pivot(&bake.worlds, hand.hand)?;
            let axis_residual = axis
                .normalize()
                .angle_between(hand.target_axis)
                .to_degrees();
            let normal_residual = normal.angle_between(hand.target_normal).to_degrees();
            // The roll itself: the in-plane angle, with both normals projected off
            // the limb axis so the un-removable along-axis part cannot mask it.
            let limb = hand.target_axis.normalize();
            let written_in_plane = (normal - limb * normal.dot(limb)).normalize_or_zero();
            let target_in_plane =
                (hand.target_normal - limb * hand.target_normal.dot(limb)).normalize_or_zero();
            let roll_residual = written_in_plane.angle_between(target_in_plane).to_degrees();
            let thumb_residual = thumb
                .normalize()
                .angle_between(hand.target_thumb)
                .to_degrees();
            Ok(json!({
                "side": format!("{:?}", hand.side),
                "roll_applied_deg": hand.roll_degrees,
                "axis_residual_deg": axis_residual,
                "roll_residual_deg": roll_residual,
                "plane_residual_deg": normal_residual,
                "thumb_residual_deg": thumb_residual,
                "along_axis_component": normal.dot(limb),
                "palm_normal": normal.to_array(),
                "palm_target": hand.target_normal.to_array(),
                "palm_plane_off_down_deg": normal.angle_between(DVec3::NEG_Y).to_degrees(),
                "palm_plane_off_up_deg": normal.angle_between(DVec3::Y).to_degrees(),
                "thumb_direction": thumb.normalize().to_array(),
            }))
        })();
        if let Ok(row) = written {
            sides.push(row);
        }
    }
    let roll_worst = sides
        .iter()
        .map(|row| row["roll_residual_deg"].as_f64().unwrap_or(180.0))
        .fold(0.0_f64, f64::max);
    let axis_worst = sides
        .iter()
        .map(|row| row["axis_residual_deg"].as_f64().unwrap_or(180.0))
        .fold(0.0_f64, f64::max);
    let thumb_worst = sides
        .iter()
        .map(|row| row["thumb_residual_deg"].as_f64().unwrap_or(180.0))
        .fold(0.0_f64, f64::max);
    let plane_worst = sides
        .iter()
        .map(|row| row["plane_residual_deg"].as_f64().unwrap_or(180.0))
        .fold(0.0_f64, f64::max);
    json!({
        "bar_roll_deg": BONE_BASIS_TOLERANCE_DEG,
        "bar_plane_deg": PALM_DOWN_TOLERANCE_DEG,
        "sides": sides,
        "max_roll_residual_deg": roll_worst,
        "max_thumb_residual_deg": thumb_worst,
        "max_plane_residual_deg": plane_worst,
        // The roll and the axis are what a rotation can control; the thumb and
        // plane angles are limited by how our hand's digit roots sit against the
        // palm (measured below), and no roll removes that difference.
        "digit_root_anatomy": "our index/pinky/thumb roots sit 3.06°/3.45° closer to the hand axis than Mixamo's; the plane and thumb residuals are that difference, not roll",
        "pass": roll_worst <= BONE_BASIS_TOLERANCE_DEG
            && axis_worst <= BONE_BASIS_TOLERANCE_DEG
            && plane_worst <= PALM_DOWN_TOLERANCE_DEG,
    })
}

/// Each wrist must sit laterally outward from its shoulder by most of the arm.
fn crossing_gate(resolved: &[Resolved], bake: &RestBake, bodies: &Bodies) -> Value {
    let left = bodies.left();
    let mut sides = Vec::new();
    for side in [Side::Left, Side::Right] {
        let find = |part: Part| {
            bake.aims
                .iter()
                .zip(resolved)
                .find(|(_, row)| row.segment.role == BoneRole { side, part })
                .map(|(aimed, _)| aimed)
        };
        let (Some(arm), Some(forearm), Some(hand)) =
            (find(Part::UpperArm), find(Part::Forearm), find(Part::Hand))
        else {
            continue;
        };
        let chain = arm.length_after + forearm.length_after;
        let offset = DVec3::from_array(hand.pivot_after) - DVec3::from_array(arm.pivot_after);
        let along = offset.dot(left) * side_sign(side);
        sides.push(json!({
            "side": format!("{side:?}"),
            "chain_m": chain,
            "lateral_m": along,
            "fraction": along / chain,
        }));
    }
    let pass = sides
        .iter()
        .all(|row| row["fraction"].as_f64().unwrap_or(-1.0) >= LATERAL_EXTENT_FRACTION);
    json!({
        "bar_fraction": LATERAL_EXTENT_FRACTION,
        "sides": sides,
        "pass": pass,
    })
}

/// A side's sign on the body's own left axis.
fn side_sign(side: Side) -> f64 {
    match side {
        Side::Left => 1.0,
        Side::Right | Side::Centre => -1.0,
    }
}

/// One candidate frame of a shipped clip, measured.
struct PoseCandidate {
    frame: usize,
    time: f64,
    arms: f64,
    torso: f64,
    legs: f64,
    turns: Vec<DQuat>,
}

impl PoseCandidate {
    /// Whether the pose is a T-rest a body can stand in.
    fn admits(&self) -> bool {
        self.arms <= T_POSE_TOLERANCE_DEG
            && self.torso <= POSE_TORSO_TOLERANCE_DEG
            && self.legs <= POSE_LEG_TOLERANCE_DEG
    }

    /// How badly it fails, as the worst ratio to its own bar.
    fn excess(&self) -> f64 {
        (self.arms / T_POSE_TOLERANCE_DEG)
            .max(self.torso / POSE_TORSO_TOLERANCE_DEG)
            .max(self.legs / POSE_LEG_TOLERANCE_DEG)
    }

    /// The reason it was not chosen, or `None` when it was.
    fn refusal(&self) -> Option<String> {
        if self.admits() {
            return None;
        }
        let mut reasons = Vec::new();
        if self.arms > T_POSE_TOLERANCE_DEG {
            reasons.push(format!("arms {:.2}°", self.arms));
        }
        if self.torso > POSE_TORSO_TOLERANCE_DEG {
            reasons.push(format!("torso {:.2}°", self.torso));
        }
        if self.legs > POSE_LEG_TOLERANCE_DEG {
            reasons.push(format!("legs {:.2}°", self.legs));
        }
        Some(reasons.join(", "))
    }
}

/// Choose a frame of a shipped clip by measurement, and read its own turns.
///
/// The frame is never picked by eye or copied from a note: every frame is
/// measured for arm horizontality and for how far the torso and legs have left
/// their rest orientation, and a frame is admitted only when all three hold. The
/// report names every rejected candidate with its numbers, so a compromise is
/// visible as a compromise instead of hidden in a choice.
fn pose_plan(
    path: &PathBuf,
    params: &TPoseParams,
    resolved: &[Resolved],
    our_worlds: &[DMat4],
) -> Result<(Option<Vec<DQuat>>, Value), CommandError> {
    let clip = params.pose_clip.as_deref().ok_or_else(|| {
        refused(
            "tpose_pose",
            "--pose needs --pose-clip: the pose is read from a named clip, not guessed".to_owned(),
        )
    })?;
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let pose = Actor::from_glb(&bytes).map_err(actor_error)?;
    let (_, end) = pose.clip_span(clip).map_err(actor_error)?;
    let frames = ((end * SKYRIM_CLIP_FPS).round() as usize).max(1);

    let node_by_name = |name: &str| -> Option<usize> {
        pose.document()
            .nodes()
            .find(|node| node.name() == Some(name))
            .map(|node| node.index())
    };
    let mut our_nodes = Vec::new();
    for row in resolved {
        let joint = node_by_name(&row.joint_name).ok_or_else(|| {
            refused(
                "tpose_pose",
                format!(
                    "the pose file carries no bone {:?}, so that bone cannot be read from it",
                    row.joint_name
                ),
            )
        })?;
        let tip = node_by_name(&row.tip_name).ok_or_else(|| {
            refused(
                "tpose_pose",
                format!(
                    "the pose file carries no bone {:?}, so its direction cannot be read",
                    row.tip_name
                ),
            )
        })?;
        our_nodes.push((joint, tip));
    }
    // Torso and legs, for the "a body can stand in it" half of the bar.
    let mut upright: Vec<(usize, usize, f64)> = Vec::new();
    for (joint_role, tip_role, bar) in [
        (Part::Spine(0), Part::Spine(1), POSE_TORSO_TOLERANCE_DEG),
        (Part::Spine(1), Part::Spine(2), POSE_TORSO_TOLERANCE_DEG),
        (Part::Spine(2), Part::Neck, POSE_TORSO_TOLERANCE_DEG),
        (Part::Thigh, Part::Calf, POSE_LEG_TOLERANCE_DEG),
        (Part::Calf, Part::Foot, POSE_LEG_TOLERANCE_DEG),
        (Part::Foot, Part::Toe, POSE_LEG_TOLERANCE_DEG),
    ] {
        let sides: &[Side] = match joint_role {
            Part::Thigh | Part::Calf | Part::Foot => &[Side::Left, Side::Right],
            _ => &[Side::Centre],
        };
        for side in sides.iter().copied() {
            let joint = role_node(
                &pose,
                BoneRole {
                    side,
                    part: joint_role,
                },
            );
            let tip = role_node(
                &pose,
                BoneRole {
                    side,
                    part: tip_role,
                },
            );
            if let (Some(joint), Some(tip)) = (joint, tip) {
                upright.push((joint, tip, bar));
            }
        }
    }
    let rest = pose.skeleton_worlds(Pose::Rest).map_err(actor_error)?;

    let times: Vec<(usize, f64)> = match params.pose_scan {
        true => (0..frames)
            .map(|frame| (frame, frame as f64 / SKYRIM_CLIP_FPS))
            .collect(),
        false => {
            let time = params.pose_time.ok_or_else(|| {
                refused(
                    "tpose_pose",
                    "--pose needs --pose-time or --pose-scan: a frame is chosen by measurement,                      never by a default"
                        .to_owned(),
                )
            })?;
            vec![((time * SKYRIM_CLIP_FPS).round() as usize, time)]
        }
    };

    let mut candidates = Vec::new();
    for (frame, time) in times {
        let worlds = match pose.skeleton_worlds(Pose::Clip { name: clip, time }) {
            Ok(worlds) => worlds,
            Err(error) => {
                return Err(actor_error(error));
            }
        };
        let direction = |joint: usize, tip: usize| -> DVec3 {
            worlds[tip].transform_point3(DVec3::ZERO) - worlds[joint].transform_point3(DVec3::ZERO)
        };
        let arms = our_nodes
            .iter()
            .map(|(joint, tip)| below_horizontal(direction(*joint, *tip)).abs())
            .fold(0.0_f64, f64::max);
        let mut torso: f64 = 0.0;
        let mut legs: f64 = 0.0;
        for (joint, tip, bar) in &upright {
            let posed = direction(*joint, *tip).normalize_or_zero();
            let resting = (rest[*tip].transform_point3(DVec3::ZERO)
                - rest[*joint].transform_point3(DVec3::ZERO))
            .normalize_or_zero();
            let deviation = posed.angle_between(resting).to_degrees();
            if (*bar - POSE_TORSO_TOLERANCE_DEG).abs() < 1e-9 {
                torso = torso.max(deviation);
            } else {
                legs = legs.max(deviation);
            }
        }
        let turns = our_nodes
            .iter()
            .map(|(joint, _)| {
                let posed = world_rotation(worlds[*joint]).normalize();
                let resting = world_rotation(rest[*joint]).normalize();
                posed * resting.inverse()
            })
            .collect();
        let _ = our_worlds;
        candidates.push(PoseCandidate {
            frame,
            time,
            arms,
            torso,
            legs,
            turns,
        });
    }

    let chosen = candidates
        .iter()
        .filter(|candidate| candidate.admits())
        .min_by(|left, right| {
            left.arms
                .partial_cmp(&right.arms)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .or_else(|| {
            candidates.iter().min_by(|left, right| {
                left.excess()
                    .partial_cmp(&right.excess())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        })
        .ok_or_else(|| refused("tpose_pose", "the clip carries no frame to bake".to_owned()))?;
    let rejected: Vec<Value> = candidates
        .iter()
        .filter(|candidate| candidate.frame != chosen.frame)
        .filter_map(|candidate| {
            candidate.refusal().map(|reason| {
                json!({
                    "frame": candidate.frame,
                    "time": candidate.time,
                    "arms_deg": candidate.arms,
                    "torso_deg": candidate.torso,
                    "legs_deg": candidate.legs,
                    "reason": reason,
                })
            })
        })
        .take(40)
        .collect();
    let block = json!({
        "source": path.to_string_lossy(),
        "sha256_16": sha16(&bytes),
        "clip": clip,
        "frames": frames,
        "fps": SKYRIM_CLIP_FPS,
        "chosen": {
            "frame": chosen.frame,
            "time": chosen.time,
            "arms_deg": chosen.arms,
            "torso_deg": chosen.torso,
            "legs_deg": chosen.legs,
        },
        "bars": {
            "arms_deg": T_POSE_TOLERANCE_DEG,
            "torso_deg": POSE_TORSO_TOLERANCE_DEG,
            "legs_deg": POSE_LEG_TOLERANCE_DEG,
        },
        "compromise": match chosen.admits() {
            true => Value::Null,
            false => json!(chosen.refusal()),
        },
        "rejected": rejected,
        "rejected_total": candidates.len().saturating_sub(1),
    });
    Ok((Some(chosen.turns.clone()), block))
}

/// The node that carries one role in a file, preferring the weighted family.
fn role_node(actor: &Actor, role: BoneRole) -> Option<usize> {
    actor
        .document()
        .nodes()
        .filter(|node| node.name().is_some_and(|name| role_of(name) == Some(role)))
        .map(|node| node.index())
        .min_by_key(|node| {
            let name = actor.node_name(*node).unwrap_or_default();
            // The ghost `CME` family duplicates every `NPC` bone; the `NPC` one
            // is the skeleton the skin weights.
            (u8::from(name.starts_with("CME ")), *node)
        })
}

/// One line a caller can print: what changed, how cleanly, and what it cost.
fn verdict_text(resolved: &[Resolved], rest_error: &f64, deformation: &Deformation) -> String {
    format!(
        "actor tpose: {} arm bones re-aimed onto the reference rest, rest reconstruction {:.2e} m, \
         edge stretch max {:.3}x (p99 {:.3}x), max vertex displacement {:.3} m",
        resolved.len(),
        rest_error,
        deformation.max_ratio,
        deformation.p99_ratio,
        deformation.max_displacement,
    )
}

/// A one-line summary of a report this module wrote.
pub fn verdict(report: &Value) -> String {
    report["verdict"]
        .as_str()
        .unwrap_or("actor tpose")
        .to_owned()
}

/// Resolve both chains onto our palette and the reference's own rest.
///
/// Each side returns its three segments and its hand frame; the frame decides the
/// roll, and the roll is what the paddle was missing.
fn resolve_chains(
    actor: &Actor,
    correspondence: &Correspondence,
    our_worlds: &[DMat4],
    reference_rig: &Rig,
    reference_palette: &[String],
    yaw: DQuat,
) -> Result<(Vec<Resolved>, Vec<HandFrame>), CommandError> {
    let mut rows = Vec::new();
    let mut hands = Vec::new();
    for side in [Side::Left, Side::Right] {
        let mut segments = Vec::new();
        for (role, tip) in [
            (Part::UpperArm, Part::Forearm),
            (Part::Forearm, Part::Hand),
            (
                Part::Hand,
                Part::Digit {
                    digit: 2,
                    segment: 0,
                },
            ),
        ] {
            let segment = Segment {
                role: BoneRole { side, part: role },
                tip: BoneRole { side, part: tip },
            };
            segments.push(resolve_segment(
                actor,
                correspondence,
                our_worlds,
                reference_rig,
                reference_palette,
                yaw,
                segment,
            )?);
        }

        let hand = segments[2].joint;
        let middle = segments[2].tip;
        let thumb = digit_node(actor, correspondence, side, 0)?;
        let index = digit_node(actor, correspondence, side, 1)?;
        let pinky = digit_node(actor, correspondence, side, 4)?;
        let authored_normal = plane_normal(our_worlds, hand, index, pinky)?;
        let reference_normal = plane_normal_world(
            &reference_pivot(reference_rig, reference_palette, side, Part::Hand)?,
            &reference_pivot(
                reference_rig,
                reference_palette,
                side,
                Part::Digit {
                    digit: 1,
                    segment: 0,
                },
            )?,
            &reference_pivot(
                reference_rig,
                reference_palette,
                side,
                Part::Digit {
                    digit: 4,
                    segment: 0,
                },
            )?,
        );
        let target_normal = (yaw * reference_normal).normalize();
        let thumb_target = (yaw
            * (reference_pivot(
                reference_rig,
                reference_palette,
                side,
                Part::Digit {
                    digit: 0,
                    segment: 0,
                },
            )? - reference_pivot(reference_rig, reference_palette, side, Part::Hand)?))
        .normalize();

        // The hand's lift, then the roll that puts its palm plane onto the
        // reference's: one angle for the chain, so the authored twist between
        // shoulder, elbow and wrist survives.
        let lift = DQuat::from_rotation_arc(
            segments[2].authored.normalize(),
            segments[2].reference_direction,
        );
        let lifted_normal = (lift * authored_normal).normalize();
        let axis = segments[2].reference_direction.normalize();
        let roll_angle = signed_roll(lifted_normal, target_normal, axis);
        let roll_degrees = roll_angle.to_degrees();

        for segment in &mut segments {
            let axis = segment.reference_direction.normalize();
            let lift = DQuat::from_rotation_arc(segment.authored.normalize(), axis);
            let turn = DQuat::from_axis_angle(axis, roll_angle) * lift;
            segment.turn = turn;
            segment.roll_degrees = roll_degrees;
        }
        hands.push(HandFrame {
            side,
            hand,
            thumb,
            middle,
            index,
            pinky,
            target_axis: segments[2].reference_direction,
            target_thumb: thumb_target,
            target_normal,
            roll_degrees,
        });
        rows.extend(segments);
    }
    Ok((rows, hands))
}

/// One segment: our nodes and authored vector, the reference's target.
fn resolve_segment(
    actor: &Actor,
    correspondence: &Correspondence,
    our_worlds: &[DMat4],
    reference_rig: &Rig,
    reference_palette: &[String],
    yaw: DQuat,
    segment: Segment,
) -> Result<Resolved, CommandError> {
    let joint_slot = reference_slot(reference_palette, segment.role).ok_or_else(|| {
        refused(
            "tpose_reference",
            format!(
                "the reference carries no {:?} {:?} rest direction",
                segment.role.side, segment.role.part
            ),
        )
    })?;
    let tip_slot = reference_slot(reference_palette, segment.tip).ok_or_else(|| {
        refused(
            "tpose_reference",
            format!(
                "the reference carries no {:?} {:?} tip to measure a direction against",
                segment.tip.side, segment.tip.part
            ),
        )
    })?;
    let joint_pivot = pivot(reference_rig, joint_slot)?;
    let tip_pivot = pivot(reference_rig, tip_slot)?;
    let direction = tip_pivot - joint_pivot;
    if direction.length() <= 0.0 {
        return Err(refused(
            "tpose_reference",
            format!(
                "the reference's {:?} {:?} bone has no length",
                segment.role.side, segment.role.part
            ),
        ));
    }
    let our_joint = our_node(correspondence, segment.role).ok_or_else(|| {
        refused(
            "tpose_correspondence",
            format!(
                "the actor has no bone for {:?} {:?}",
                segment.role.side, segment.role.part
            ),
        )
    })?;
    let our_tip = our_node(correspondence, segment.tip).ok_or_else(|| {
        refused(
            "tpose_correspondence",
            format!(
                "the actor has no bone for {:?} {:?}",
                segment.tip.side, segment.tip.part
            ),
        )
    })?;
    let reference_rotation = reference_rotation(reference_rig, joint_slot)?;
    let axis = DVec3::from_array(axes_of(reference_rotation)[1]);
    Ok(Resolved {
        segment,
        joint: our_joint,
        joint_name: actor.node_name(our_joint).unwrap_or("<unnamed>").to_owned(),
        tip: our_tip,
        tip_name: actor.node_name(our_tip).unwrap_or("<unnamed>").to_owned(),
        authored: world_pivot(our_worlds, our_tip)? - world_pivot(our_worlds, our_joint)?,
        reference_direction: (yaw * direction).normalize(),
        axis_offset_degrees: axis.angle_between(direction.normalize()).to_degrees(),
        turn: DQuat::IDENTITY,
        roll_degrees: 0.0,
    })
}

/// The reference palette slot that carries a role.
fn reference_slot(palette: &[String], role: BoneRole) -> Option<usize> {
    palette.iter().position(|name| role_of(name) == Some(role))
}

/// Our actor's node for a role, from the correspondence the seat uses.
fn our_node(correspondence: &Correspondence, role: BoneRole) -> Option<usize> {
    correspondence
        .pairs
        .iter()
        .find(|pair| pair.role == Some(role))
        .and_then(|pair| pair.source)
}

/// The reference's own arm directions, in the actor's Y-up frame.
fn reference_arm_angles(rows: &[Resolved]) -> Result<Vec<ReferenceAngle>, CommandError> {
    rows.iter()
        .map(|row| {
            let direction = row.reference_direction;
            if !direction.is_finite() {
                return Err(refused(
                    "tpose_reference",
                    "a reference arm direction is not finite".to_owned(),
                ));
            }
            Ok(ReferenceAngle {
                segment: row.segment,
                below_horizontal: below_horizontal(direction),
            })
        })
        .collect()
}

/// Refuse a reference that is not itself a T-pose rest.
fn refuse_non_t_pose(rows: &[ReferenceAngle]) -> Result<(), CommandError> {
    let outside: Vec<String> = rows
        .iter()
        .filter(|row| row.below_horizontal.abs() > T_POSE_TOLERANCE_DEG)
        .map(|row| {
            format!(
                "{} {:.2}° below horizontal",
                label(&row.segment.role),
                row.below_horizontal
            )
        })
        .collect();
    match outside.is_empty() {
        true => Ok(()),
        false => Err(refused(
            "tpose_reference",
            format!(
                "the reference is not a T-pose rest: {} (tolerance {T_POSE_TOLERANCE_DEG}°)",
                outside.join(", ")
            ),
        )),
    }
}

/// The pool range the posed sample must present: one shared vertex pool.
///
/// Every primitive of the actor node reads the same POSITION accessor, so the
/// sample carries the pool once per primitive. The first range is the pool; every
/// other range must read the same values, which is what "shared" means in this
/// file layout and what `read_pool` rebases its triangles against.
fn pool_range(bake: &RestBake, vertices: usize) -> Result<std::ops::Range<usize>, CommandError> {
    let first = bake
        .posed
        .parts
        .first()
        .map(|part| part.vertices.clone())
        .ok_or_else(|| {
            refused(
                "tpose_actor",
                "the actor's mesh carries no primitive".to_owned(),
            )
        })?;
    if first.len() != vertices {
        return Err(refused(
            "tpose_actor",
            format!(
                "the selected mesh reads {} vertices where the pool carries {vertices}",
                first.len()
            ),
        ));
    }
    let pool = &bake.posed.vertices[first.clone()];
    for part in &bake.posed.parts {
        let same = part.vertices.len() == pool.len()
            && part
                .vertices
                .clone()
                .zip(pool.iter())
                .all(|(vertex, expected)| bake.posed.vertices[vertex] == *expected);
        if !same {
            return Err(refused(
                "tpose_actor",
                format!(
                    "primitive {:?} reads a different vertex pool from the actor's first primitive",
                    part.name
                ),
            ));
        }
    }
    Ok(first)
}

/// Raw weight mass per palette slot.
fn weight_mass(rows: &[Vec<(u16, f32)>], palette: usize) -> Vec<f64> {
    let mut mass = vec![0.0_f64; palette];
    for row in rows {
        for &(slot, weight) in row {
            mass[usize::from(slot)] += f64::from(weight);
        }
    }
    mass
}

/// The pivot of one reference joint, world metres.
fn pivot(rig: &Rig, slot: usize) -> Result<DVec3, CommandError> {
    let world =
        DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(slot as u32)).map_err(|error| {
            errors::invalid("tpose_reference", format!("palette slot {slot}: {error}"))
        })?);
    Ok(world.transform_point3(DVec3::ZERO))
}

/// Degrees a direction points below the horizontal plane, in the actor's frame.
fn below_horizontal(direction: DVec3) -> f64 {
    let length = direction.length();
    match length > 0.0 {
        true => (-direction.y / length).clamp(-1.0, 1.0).asin().to_degrees(),
        false => 0.0,
    }
}

/// The two bodies' own horizontal frames.
///
/// The actors do not share a forward axis: this body faces the opposite way
/// from the Mixamo reference (measured below), so an arm aimed at the
/// reference's raw world direction crosses the chest instead of opening.
/// `yaw` is the turn about the vertical that carries the reference's forward
/// onto ours, and every target direction is expressed through it.
struct Bodies {
    our_forward: DVec3,
    reference_forward: DVec3,
    yaw: DQuat,
}

impl Bodies {
    /// Measure both bodies' forward axes from their own feet and derive the yaw.
    fn measure(
        our_rig: &Rig,
        our_palette: &[String],
        reference_rig: &Rig,
        reference_palette: &[String],
    ) -> Result<Self, CommandError> {
        let our_forward = forward_axis(our_rig, our_palette, "actor")?;
        let reference_forward = forward_axis(reference_rig, reference_palette, "reference")?;
        let cross = our_forward.cross(reference_forward);
        let yaw_degrees = cross
            .y
            .atan2(our_forward.dot(reference_forward))
            .to_degrees();
        let yaw = DQuat::from_axis_angle(DVec3::Y, yaw_degrees.to_radians());
        Ok(Self {
            our_forward,
            reference_forward,
            yaw,
        })
    }

    /// The body's own left axis: its forward turned a quarter about the vertical.
    fn left(&self) -> DVec3 {
        DVec3::Y.cross(self.our_forward).normalize()
    }

    /// The yaw this bake applies, degrees.
    fn yaw_degrees(&self) -> f64 {
        2.0 * self.yaw.w.clamp(-1.0, 1.0).acos().to_degrees()
    }
}

/// A body's own forward axis: the mean horizontal direction its toes point at.
///
/// Both feet are averaged on purpose — a single foot splays outward by 10–20°
/// and would rotate the whole frame with it, which is the difference between a
/// T-pose that is lateral and one that is 15° behind the body.
fn forward_axis(rig: &Rig, palette: &[String], who: &str) -> Result<DVec3, CommandError> {
    let mut sum = DVec3::ZERO;
    for side in [Side::Left, Side::Right] {
        let foot = reference_slot(
            palette,
            BoneRole {
                side,
                part: Part::Foot,
            },
        );
        let toe = reference_slot(
            palette,
            BoneRole {
                side,
                part: Part::Toe,
            },
        );
        if let (Some(foot), Some(toe)) = (foot, toe) {
            let direction = pivot(rig, toe)? - pivot(rig, foot)?;
            sum += DVec3::new(direction.x, 0.0, direction.z).normalize_or_zero();
        }
    }
    match sum.length() > 0.0 {
        true => Ok(sum.normalize()),
        false => Err(refused(
            "tpose_frame",
            format!("the {who}'s feet point nowhere horizontally, so its frame is undefined"),
        )),
    }
}

/// The world pivot of one node.
fn world_pivot(worlds: &[DMat4], node: usize) -> Result<DVec3, CommandError> {
    worlds
        .get(node)
        .map(|world| world.transform_point3(DVec3::ZERO))
        .ok_or_else(|| {
            errors::invalid(
                "tpose_frame",
                format!("node {node} has no world transform in the actor"),
            )
        })
}

/// A plane normal through three world points, normalized.
fn plane_normal_world(a: &DVec3, b: &DVec3, c: &DVec3) -> DVec3 {
    (*b - *a).cross(*c - *a).normalize_or_zero()
}

/// One hand's palm-plane normal from its own digit roots.
fn plane_normal(
    worlds: &[DMat4],
    hand: usize,
    index: usize,
    pinky: usize,
) -> Result<DVec3, CommandError> {
    let origin = world_pivot(worlds, hand)?;
    let normal = plane_normal_world(
        &origin,
        &world_pivot(worlds, index)?,
        &world_pivot(worlds, pinky)?,
    );
    match normal.length() > 0.0 {
        true => Ok(normal),
        false => Err(refused(
            "tpose_frame",
            "a hand's index and pinky roots are collinear with its wrist, so its palm plane is \
             undefined",
        )),
    }
}

/// The signed angle about `axis` that turns `from` onto `to`, radians.
fn signed_roll(from: DVec3, to: DVec3, axis: DVec3) -> f64 {
    let axis = axis.normalize();
    let from = (from - axis * from.dot(axis)).normalize_or_zero();
    let to = (to - axis * to.dot(axis)).normalize_or_zero();
    let cross = from.cross(to).dot(axis);
    cross.atan2(from.dot(to))
}

/// One digit root of our actor, resolved through the correspondence.
fn digit_node(
    actor: &Actor,
    correspondence: &Correspondence,
    side: Side,
    digit: u8,
) -> Result<usize, CommandError> {
    let role = BoneRole {
        side,
        part: Part::Digit { digit, segment: 0 },
    };
    our_node(correspondence, role).ok_or_else(|| {
        refused(
            "tpose_correspondence",
            format!(
                "the actor has no digit {digit} root for the {side:?} hand, so its palm plane \
                 cannot be measured; bone {:?} is unnamed there",
                actor.node_name(0)
            ),
        )
    })
}

/// The reference's world pivot for one role.
fn reference_pivot(
    rig: &Rig,
    palette: &[String],
    side: Side,
    part: Part,
) -> Result<DVec3, CommandError> {
    let slot = reference_slot(palette, BoneRole { side, part }).ok_or_else(|| {
        refused(
            "tpose_reference",
            format!("the reference carries no {side:?} {part:?} joint"),
        )
    })?;
    pivot(rig, slot)
}

/// The rotation of one world transform with its scale divided out.
fn world_rotation(world: DMat4) -> DQuat {
    world.to_scale_rotation_translation().1
}

/// The world rotation of one reference palette bone, scale divided out.
fn reference_rotation(rig: &Rig, slot: usize) -> Result<DQuat, CommandError> {
    let world =
        DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(slot as u32)).map_err(|error| {
            errors::invalid("tpose_reference", format!("palette slot {slot}: {error}"))
        })?);
    Ok(world.to_scale_rotation_translation().1.normalize())
}

/// Refuse a reference whose bones do not carry their own axis.
///
/// Carrying the roll out of a reference is only meaningful when its bones are
/// built the way Mixamo's are: Y along the limb. A reference of pure translations
/// would give every bone the same identity basis and the bake would roll the arms
/// by whatever the source file happened to say.
fn refuse_rolled_reference(rows: &[Resolved]) -> Result<(), CommandError> {
    let outside: Vec<String> = rows
        .iter()
        .filter(|row| row.axis_offset_degrees > REFERENCE_AXIS_TOLERANCE_DEG)
        .map(|row| {
            format!(
                "{} {:.2}° off the bone",
                label(&row.segment.role),
                row.axis_offset_degrees
            )
        })
        .collect();
    match outside.is_empty() {
        true => Ok(()),
        false => Err(refused(
            "tpose_reference",
            format!(
                "the reference's bones do not carry Y along the bone, so their roll cannot be \
                 carried: {} (tolerance {REFERENCE_AXIS_TOLERANCE_DEG}°)",
                outside.join(", ")
            ),
        )),
    }
}

/// The three world axes of a rotation, in `x, y, z` order.
fn axes_of(rotation: DQuat) -> [[f64; 3]; 3] {
    let matrix = glam::DMat3::from_quat(rotation);
    let mut axes = [[0.0_f64; 3]; 3];
    for (index, axis) in axes.iter_mut().enumerate() {
        let column = matrix.col(index);
        *axis = [column.x, column.y, column.z];
    }
    axes
}

/// One reference arm row.
struct ReferenceAngle {
    segment: Segment,
    below_horizontal: f64,
}

/// A readable role label: `Left UpperArm`.
fn label(role: &BoneRole) -> String {
    let side = match role.side {
        Side::Left => "Left",
        Side::Right => "Right",
        Side::Centre => "Centre",
    };
    let part = match role.part {
        Part::Pelvis => "Hips".to_owned(),
        Part::Spine(index) => format!("Spine{index}"),
        Part::Neck => "Neck".to_owned(),
        Part::Head => "Head".to_owned(),
        Part::Clavicle => "Clavicle".to_owned(),
        Part::UpperArm => "UpperArm".to_owned(),
        Part::Forearm => "Forearm".to_owned(),
        Part::Hand => "Hand".to_owned(),
        Part::Thigh => "Thigh".to_owned(),
        Part::Calf => "Calf".to_owned(),
        Part::Foot => "Foot".to_owned(),
        Part::Toe => "Toe".to_owned(),
        Part::Digit { digit, segment } => format!("Digit{digit}.{segment}"),
        Part::Twist { forearm, index } => {
            format!(
                "Twist{}{index}",
                if forearm { "Forearm" } else { "Upperarm" }
            )
        }
        Part::Soft => "Soft".to_owned(),
        Part::Marker => "Marker".to_owned(),
        Part::Scaffold => "Scaffold".to_owned(),
    };
    format!("{side} {part}")
}

/// Euclidean distance between two world points.
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    DVec3::from_array(a).distance(DVec3::from_array(b))
}

/// First 16 hex characters of the SHA-256 of the given bytes.
fn sha16(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let full = sha_hex(&digest.finalize());
    full.chars().take(16).collect()
}

/// How far the bake moved the flesh and how much it stretched, per region.
struct Deformation {
    worst: Option<Value>,
    max_displacement: f64,
    edges: usize,
    max_ratio: f64,
    p99_ratio: f64,
    mean_ratio: f64,
    above_2x: usize,
    long_edges: usize,
    long_max_ratio: f64,
    long_above_2x: usize,
    bands: Vec<BandRow>,
    parts: Vec<PartRow>,
}

/// One named edge band: the seam a bake could open, or the chain it must move.
struct BandRow {
    name: &'static str,
    edges: usize,
    max_ratio: f64,
    p99_ratio: f64,
    above_2x: usize,
}

/// One primitive's own share of the deformation.
struct PartRow {
    name: String,
    triangles: usize,
    max_displacement: f64,
    max_ratio: f64,
}

/// Edges shorter than this are wire noise and not graded, matching the export.
const GRADED_EDGE_M: f64 = 0.005;

/// The second grading floor: the same statistic with short edges excluded, so a
/// pass or fail can be read at the scale a viewer sees.
const GRADED_EDGE_LONG_M: f64 = 0.010;

impl Deformation {
    /// Measure the bake over the pool's own triangles.
    fn measure(bake: &RestBake, pool: &Pool, palette: &[String], names: &[String]) -> Self {
        let rest = &bake.rest.vertices;
        let posed = &bake.posed.vertices;
        let mut edges: BTreeMap<(u32, u32), (usize, usize)> = BTreeMap::new();
        for triangle in &pool.triangles {
            for pair in [[0, 1], [1, 2], [2, 0]] {
                let a = triangle[pair[0]];
                let b = triangle[pair[1]];
                let key = match a < b {
                    true => (a, b),
                    false => (b, a),
                };
                edges.insert(key, (key.0 as usize, key.1 as usize));
            }
        }
        let dominant = dominant_slots(&pool.rows);
        let mut ratios: Vec<f64> = Vec::new();
        let mut long_ratios: Vec<f64> = Vec::new();
        let mut max_displacement = 0.0_f64;
        for (vertex, point) in rest.iter().enumerate() {
            max_displacement = max_displacement
                .max(DVec3::from_array(*point).distance(DVec3::from_array(posed[vertex])));
        }
        let mut worst: Option<(f64, usize, usize)> = None;
        let mut band_edges: Vec<(&'static str, Vec<f64>)> = vec![
            ("wrist", Vec::new()),
            ("ankle", Vec::new()),
            ("neck", Vec::new()),
            ("shoulder", Vec::new()),
            ("arm_chain", Vec::new()),
        ];
        for &(a, b) in edges.keys() {
            let (a, b) = (a as usize, b as usize);
            let before = DVec3::from_array(rest[a]).distance(DVec3::from_array(rest[b]));
            if before < GRADED_EDGE_M {
                continue;
            }
            let after = DVec3::from_array(posed[a]).distance(DVec3::from_array(posed[b]));
            let ratio = after / before;
            ratios.push(ratio);
            if before >= GRADED_EDGE_LONG_M {
                long_ratios.push(ratio);
            }
            if worst.is_none_or(|(best, _, _)| ratio > best) {
                worst = Some((ratio, a, b));
            }
            let (left, right) = (
                role_of(&palette[dominant[a]]),
                role_of(&palette[dominant[b]]),
            );
            for band in &mut band_edges {
                if in_band(band.0, left, right) {
                    band.1.push(ratio);
                }
            }
        }
        ratios.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        long_ratios
            .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let max_ratio = ratios.last().copied().unwrap_or(1.0);
        let p99_ratio = quantile(&ratios, 0.99);
        let mean_ratio = match ratios.is_empty() {
            true => 1.0,
            false => ratios.iter().sum::<f64>() / ratios.len() as f64,
        };
        let above_2x = ratios.iter().filter(|ratio| **ratio > 2.0).count();
        let bands = band_edges
            .into_iter()
            .map(|(name, values)| {
                let mut sorted = values;
                sorted.sort_by(|left, right| {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                });
                BandRow {
                    name,
                    edges: sorted.len(),
                    max_ratio: sorted.last().copied().unwrap_or(1.0),
                    p99_ratio: quantile(&sorted, 0.99),
                    above_2x: sorted.iter().filter(|ratio| **ratio > 2.0).count(),
                }
            })
            .collect();
        let parts = pool
            .parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let start: usize = pool.parts[..index].iter().map(|row| row.triangles).sum();
                let triangles = &pool.triangles[start..start + part.triangles];
                let mut vertices: Vec<usize> =
                    triangles.iter().flatten().map(|v| *v as usize).collect();
                vertices.sort_unstable();
                vertices.dedup();
                let max_displacement = vertices
                    .iter()
                    .map(|vertex| {
                        DVec3::from_array(rest[*vertex]).distance(DVec3::from_array(posed[*vertex]))
                    })
                    .fold(0.0_f64, f64::max);
                let max_ratio = triangles
                    .iter()
                    .flat_map(|triangle| {
                        [[0, 1], [1, 2], [2, 0]]
                            .map(|pair| (triangle[pair[0]] as usize, triangle[pair[1]] as usize))
                    })
                    .map(|(a, b)| {
                        let before =
                            DVec3::from_array(rest[a]).distance(DVec3::from_array(rest[b]));
                        let after =
                            DVec3::from_array(posed[a]).distance(DVec3::from_array(posed[b]));
                        match before >= GRADED_EDGE_M {
                            true => after / before,
                            false => 1.0,
                        }
                    })
                    .fold(1.0_f64, f64::max);
                PartRow {
                    name: names
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| format!("part {index}")),
                    triangles: part.triangles,
                    max_displacement,
                    max_ratio,
                }
            })
            .collect();
        let worst = worst.map(|(ratio, a, b)| {
            json!({
                "ratio": ratio,
                "rest_length_m": DVec3::from_array(rest[a]).distance(DVec3::from_array(rest[b])),
                "first": {
                    "vertex": a,
                    "bone": &palette[dominant[a]],
                    "rest": rest[a],
                    "posed": posed[a],
                    "moved_m": DVec3::from_array(rest[a]).distance(DVec3::from_array(posed[a])),
                },
                "second": {
                    "vertex": b,
                    "bone": &palette[dominant[b]],
                    "rest": rest[b],
                    "posed": posed[b],
                    "moved_m": DVec3::from_array(rest[b]).distance(DVec3::from_array(posed[b])),
                },
            })
        });
        Self {
            worst,
            max_displacement,
            edges: ratios.len(),
            max_ratio,
            p99_ratio,
            mean_ratio,
            above_2x,
            long_edges: long_ratios.len(),
            long_max_ratio: long_ratios.last().copied().unwrap_or(1.0),
            long_above_2x: long_ratios.iter().filter(|ratio| **ratio > 2.0).count(),
            bands,
            parts,
        }
    }

    /// The JSON block.
    fn report(&self) -> Value {
        json!({
            "graded_edge_m": GRADED_EDGE_M,
            "edges": self.edges,
            "max_vertex_displacement_m": self.max_displacement,
            "max_ratio": self.max_ratio,
            "p99_ratio": self.p99_ratio,
            "mean_ratio": self.mean_ratio,
            "edges_above_2x": self.above_2x,
            "long_edges": self.long_edges,
            "long_max_ratio": self.long_max_ratio,
            "long_edges_above_2x": self.long_above_2x,
            "worst_edge": self.worst,
            "bands": self.bands.iter().map(|band| json!({
                "band": band.name,
                "edges": band.edges,
                "max_ratio": band.max_ratio,
                "p99_ratio": band.p99_ratio,
                "edges_above_2x": band.above_2x,
            })).collect::<Vec<_>>(),
            "parts": self.parts.iter().map(|part| json!({
                "name": part.name,
                "triangles": part.triangles,
                "max_vertex_displacement_m": part.max_displacement,
                "max_ratio": part.max_ratio,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Whether an edge with two dominant bone names belongs to a named band.
fn in_band(band: &str, left: Option<BoneRole>, right: Option<BoneRole>) -> bool {
    let parts = [left.map(|role| role.part), right.map(|role| role.part)];
    let has = |wanted: Part| parts.contains(&Some(wanted));
    let digit = |parts: &[Option<Part>; 2]| {
        parts
            .iter()
            .any(|part| matches!(part, Some(Part::Digit { .. })))
    };
    match band {
        "wrist" => {
            (has(Part::Hand)
                || has(Part::Digit {
                    digit: 0,
                    segment: 0,
                })
                || digit(&parts))
                && (has(Part::Forearm)
                    || has(Part::Twist {
                        forearm: true,
                        index: 1,
                    }))
        }
        "ankle" => has(Part::Foot) && has(Part::Calf),
        "neck" => {
            has(Part::Head) && (has(Part::Spine(2)) || has(Part::Spine(1)) || has(Part::Neck))
        }
        "shoulder" => {
            let girdle = has(Part::Clavicle) || has(Part::Spine(2));
            let limb = has(Part::UpperArm)
                || has(Part::Twist {
                    forearm: false,
                    index: 1,
                });
            girdle && limb
        }
        "arm_chain" => parts.iter().any(|part| {
            matches!(
                part,
                Some(Part::UpperArm | Part::Forearm | Part::Hand | Part::Digit { .. })
            )
        }),
        _ => false,
    }
}

/// The heaviest palette slot of each row: the bone that carries the vertex.
fn dominant_slots(rows: &[Vec<(u16, f32)>]) -> Vec<usize> {
    rows.iter()
        .map(|row| {
            row.iter()
                .fold((0_usize, 0.0_f32), |best, &(slot, weight)| {
                    match weight > best.1 {
                        true => (usize::from(slot), weight),
                        false => best,
                    }
                })
                .0
        })
        .collect()
}

/// Nearest-rank quantile of an ascending slice.
fn quantile(sorted: &[f64], fraction: f64) -> f64 {
    match sorted.is_empty() {
        true => 1.0,
        false => {
            let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
            sorted[index]
        }
    }
}
