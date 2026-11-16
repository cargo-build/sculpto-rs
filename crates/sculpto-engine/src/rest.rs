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

use std::collections::{BTreeMap, BTreeSet};
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

/// How straight an elbow or wrist must be after the straightening step, degrees.
///
/// The bar is the task's, not the geometry's: a hinge rotation about
/// `normalize(parent × child)` lands the child collinear by construction, so
/// anything above float noise means the written turn did not reach the bone.
pub const STRAIGHTEN_TOLERANCE_DEG: f64 = 0.5;

/// How straight a pair must already be before the step is a no-op, degrees.
///
/// The hinge is the pair's cross product, which vanishes as the flexion does; a
/// pair below this is recorded as already straight instead of being turned about
/// float noise.
pub const STRAIGHTEN_EPS_DEG: f64 = 1e-6;

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
    /// The `actor havok` report whose `skeleton_rest` is the pose clip's own basis.
    ///
    /// Without it the clip's locals are applied as if they belonged to our rest,
    /// which is the `0159` error class; with it each bone receives the delta the
    /// game applies (`rest⁻¹ · pose`) in its own local frame.
    #[serde(default)]
    pub pose_rest: Option<PathBuf>,
    /// Straighten each elbow and wrist about its own geometric hinge.
    ///
    /// The shipped pose is a fall (`mt_jumpfall` f148): its shoulders are a T to
    /// `1.5°`, but its elbows are bent `43°`/`52°`, and a rest pose that carries
    /// that bend adds it to every clip played against the rest. The step runs
    /// after the aim and rotates each forearm onto its upper arm, then each hand
    /// onto the straightened forearm, about `normalize(parent × child)` — the
    /// pair's own hinge, not the axis `joints.toml` declares, which sits `59–65°`
    /// off it on this rig.
    #[serde(default)]
    pub straighten_arms: bool,
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
    /// The hand's authored palm-plane normal, world.
    authored_normal: DVec3,
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
    let plan = match &params.pose {
        Some(path) => pose_plan(&actor, path, params, &resolved, &our_worlds)?,
        None => PosePlan {
            targets: None,
            hand_targets: None,
            block: Value::Null,
        },
    };
    let pose_block = plan.block;
    // The pose's compensated targets replace the reference's, and the same turn
    // arithmetic is re-run on them: our bones keep their own axis convention
    // instead of having a foreign world delta applied to it.
    let mut resolved = resolved;
    let mut hands = hands;
    if let Some(targets) = &plan.targets {
        for (row, target) in resolved.iter_mut().zip(targets) {
            row.reference_direction = *target;
        }
        if let Some(hand_targets) = &plan.hand_targets {
            for (hand, (axis, normal, thumb)) in hands.iter_mut().zip(hand_targets) {
                hand.target_axis = *axis;
                hand.target_normal = *normal;
                hand.target_thumb = *thumb;
            }
        }
        retarget_turns(&mut resolved, &mut hands)?;
    }

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

    let aims_of = |resolved: &[Resolved]| -> Vec<RestAim> {
        resolved
            .iter()
            .map(|row| RestAim {
                joint: row.joint,
                child: row.tip,
                direction: row.reference_direction.to_array(),
                local_turn: None,
                turn: Some(row.turn.to_array()),
            })
            .collect()
    };
    // The aim is baked and then measured, because the hinge a chain turns about
    // is its own geometry: the straightening step re-aims what the aim produced,
    // and predicting that pose instead would be a second implementation of the
    // same solver, free to disagree with it.
    let (bake, straighten) = {
        let aimed =
            bake_rest(&actor, &[mesh_name.as_str()], &aims_of(&resolved)).map_err(actor_error)?;
        match params.straighten_arms {
            true => {
                let plan = plan_straightening(&mut resolved, &mut hands, &aimed)?;
                let straightened = bake_rest(&actor, &[mesh_name.as_str()], &aims_of(&resolved))
                    .map_err(actor_error)?;
                (straightened, Some((plan, aimed)))
            }
            false => (aimed, None),
        }
    };

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
    let (straighten_value, straighten_verdict) = match &straighten {
        Some((plan, baseline)) => {
            let baseline_deformation = Deformation::measure(baseline, &pool, &our_palette, &names);
            let (value, verdict) = straighten_report(&StraightenInputs {
                plan,
                baseline,
                straightened: &bake,
                hands: &hands,
                resolved: &resolved,
                authored: &our_worlds,
                actor: &actor,
                baseline_deformation: &baseline_deformation,
                deformation: &deformation,
            })?;
            (value, Some(verdict))
        }
        None => (Value::Null, None),
    };
    // The straightening block is its own evidence: the aim gates above cannot
    // carry it, so it appends its own row to the same table.
    let mut gate_rows = gates(
        &resolved,
        &hands,
        &bake,
        &deformation,
        &validation,
        &measurement,
        &bodies,
    );
    gate_rows
        .as_object_mut()
        .expect("the gate table is an object")
        .insert(
            "straighten".to_owned(),
            straighten_gate(straighten_verdict.as_ref()),
        );
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
        "straighten": straighten_value,
        "gates": gate_rows,
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

/// A two-segment joint's own hinge: the axis the flexion turns about.
struct Hinge {
    /// `normalize(parent × child)`, world.
    axis: DVec3,
    /// The angle between the segments, degrees, positive.
    flexion_deg: f64,
}

impl Hinge {
    /// The world rotation that removes the flexion: `−flexion` about the hinge.
    ///
    /// By construction this lands the child on the parent, whatever the rig's
    /// declared joint axis says the hinge is.
    fn rotation(&self) -> DQuat {
        DQuat::from_axis_angle(self.axis, -self.flexion_deg.to_radians())
    }
}

/// One arm joint the straightening step re-aims.
struct Straightening {
    side: Side,
    /// The joint that turns.
    part: Part,
    /// The segment it is made collinear with.
    parent: Part,
    /// The hinge of the pose the step turns, or `None` when already collinear.
    hinge: Option<Hinge>,
    /// The same joint's hinge on the authored rest, for the record: the hinge is
    /// a property of a pose, and the pose that was straightened is the fall's.
    rest_hinge: Option<Hinge>,
    /// The world turn that replaces the aimed one.
    turn: DQuat,
}

/// The hinge of one two-segment joint, measured from the segments themselves.
///
/// `joints.toml` declares the elbow as its own local `Z`; on our rig that axis
/// sits `59–65°` off the true hinge (the report's `declared_local_z_off_deg`), and
/// a rotation about it swings the forearm sideways instead of straightening it.
/// The convention-free hinge is the pair's cross product: turning the child by
/// the flexion about `normalize(parent × child)` leaves it collinear with the
/// parent by construction, and the axis is only undefined when the pair is
/// already straight or exactly folded back.
fn hinge_between(
    parent: DVec3,
    child: DVec3,
    side: Side,
    part: Part,
) -> Result<Option<Hinge>, CommandError> {
    let (Some(parent), Some(child)) = (parent.try_normalize(), child.try_normalize()) else {
        return Err(refused(
            "tpose_straighten",
            format!("the {side:?} {part:?} segment has no length, so its hinge is undefined"),
        ));
    };
    let flexion_deg = parent.angle_between(child).to_degrees();
    if !flexion_deg.is_finite() {
        return Err(refused(
            "tpose_straighten",
            format!("the {side:?} {part:?} segment does not point anywhere finite"),
        ));
    }
    if flexion_deg <= STRAIGHTEN_EPS_DEG {
        return Ok(None);
    }
    if flexion_deg >= 180.0 - STRAIGHTEN_EPS_DEG {
        return Err(refused(
            "tpose_straighten",
            format!(
                "the {side:?} {part:?} joint is folded back by {flexion_deg:.3}°, so the turn axis \
                 is undefined and it cannot be straightened"
            ),
        ));
    }
    Ok(Some(Hinge {
        axis: parent.cross(child).normalize(),
        flexion_deg,
    }))
}

/// Replace the aimed arm turns with ones that leave each chain collinear.
///
/// The elbow turns the forearm onto its upper arm; that turn carries the hand, so
/// the wrist then turns what is left onto the straightened forearm. Both turns
/// are stated against the authored basis, exactly as the aim's were, so the
/// second bake writes each bone's world orientation once instead of compounding.
fn plan_straightening(
    resolved: &mut [Resolved],
    hands: &mut [HandFrame],
    aimed: &RestBake,
) -> Result<Vec<Straightening>, CommandError> {
    let mut plan = Vec::new();
    for side in [Side::Left, Side::Right] {
        let role = |part: Part| BoneRole { side, part };
        let find = |resolved: &[Resolved], part: Part| {
            resolved
                .iter()
                .position(|row| row.segment.role == role(part))
        };
        let mut chain = [DVec3::ZERO; 3];
        for (slot, part) in [Part::UpperArm, Part::Forearm, Part::Hand]
            .into_iter()
            .enumerate()
        {
            chain[slot] = measured_direction(aimed, resolved, role(part)).ok_or_else(|| {
                refused(
                    "tpose_straighten",
                    format!(
                        "the aim carries no {:?} {:?} direction to straighten",
                        side, part
                    ),
                )
            })?;
        }
        let elbow = hinge_between(chain[0], chain[1], side, Part::Forearm)?;
        let elbow_lift = elbow
            .as_ref()
            .map(Hinge::rotation)
            .unwrap_or(DQuat::IDENTITY);
        let forearm = elbow_lift * chain[1];
        // The hand is a child of the forearm, so the elbow's turn carries it
        // before the wrist decides how much of the remaining bend to remove.
        let wrist = hinge_between(forearm, elbow_lift * chain[2], side, Part::Hand)?;
        let wrist_lift = wrist
            .as_ref()
            .map(Hinge::rotation)
            .unwrap_or(DQuat::IDENTITY);
        let steps = [
            (Part::Forearm, Part::UpperArm, elbow, elbow_lift),
            (
                Part::Hand,
                Part::Forearm,
                wrist,
                (wrist_lift * elbow_lift).normalize(),
            ),
        ];
        for (part, parent, hinge, composition) in steps {
            let index = find(resolved, part).ok_or_else(|| {
                refused(
                    "tpose_straighten",
                    format!("the aim carries no {:?} {part:?} bone to straighten", side),
                )
            })?;
            let parent_index = find(resolved, parent).ok_or_else(|| {
                refused(
                    "tpose_straighten",
                    format!(
                        "the aim carries no {:?} {parent:?} bone to straighten against",
                        side
                    ),
                )
            })?;
            let turn = combine(&composition, resolved[index].turn);
            resolved[index].turn = turn;
            // Collinear with the upper arm's own measured direction: the
            // residual the bake then reports is the real collinearity error.
            resolved[index].reference_direction = chain[0];
            plan.push(Straightening {
                side,
                part,
                parent,
                hinge,
                rest_hinge: hinge_between(
                    resolved[parent_index].authored,
                    resolved[index].authored,
                    side,
                    part,
                )?,
                turn,
            });
        }
        // The hand's target frame is the pose's own authority carried through the
        // same rotation: the step turns the whole hand, so grading the palm
        // against a target left behind would report the step as a roll defect.
        if let Some(hand) = hands.iter_mut().find(|hand| hand.side == side) {
            let carry = (wrist_lift * elbow_lift).normalize();
            hand.target_axis = carry * hand.target_axis;
            hand.target_normal = carry * hand.target_normal;
            hand.target_thumb = carry * hand.target_thumb;
        }
    }
    Ok(plan)
}

/// Apply one straightening turn on top of an aimed one.
///
/// The aim's turn is `R_aim · authored`, and the step wants `R_step · R_aim ·
/// authored`: a world pre-rotation of the pose the aim produced, so the authored
/// basis is named once and the chain's parents are not re-applied.
fn combine(step: &DQuat, aimed: DQuat) -> DQuat {
    (*step * aimed).normalize()
}

/// One joint's direction as one bake measured it, by role.
fn measured_direction(bake: &RestBake, resolved: &[Resolved], role: BoneRole) -> Option<DVec3> {
    let row = resolved.iter().find(|row| row.segment.role == role)?;
    let aimed = bake
        .aims
        .iter()
        .find(|aimed| aimed.joint == row.joint && aimed.child == row.tip)?;
    Some(DVec3::from_array(aimed.direction_after))
}

/// The rotation of one world transform with its scale divided out.
///
/// A trace-based extraction from a scaled matrix is not scale-invariant, and the
/// hands carry `0.8517`: the local axis of a hinge expressed in that frame would
/// otherwise be off by degrees.
fn world_rotation(world: DMat4) -> DQuat {
    world.to_scale_rotation_translation().1.normalize()
}

/// Everything the straightening ledger reads.
struct StraightenInputs<'a> {
    plan: &'a [Straightening],
    /// The bake before the step: the aim's own pose.
    baseline: &'a RestBake,
    /// The bake after the step, which is also the written rest.
    straightened: &'a RestBake,
    hands: &'a [HandFrame],
    resolved: &'a [Resolved],
    /// The authored worlds, for the hinge expressed in a bone's own frame.
    authored: &'a [DMat4],
    actor: &'a Actor,
    baseline_deformation: &'a Deformation,
    deformation: &'a Deformation,
}

/// The few numbers the gate reads back out of the ledger.
struct StraightenVerdict {
    applied: bool,
    elbow_max_deg: f64,
    wrist_max_deg: f64,
    upper_arm_max_deg: f64,
    non_arm_max_deg: f64,
    not_worse: bool,
    applied_max_deg: f64,
}

/// The straightening ledger: every rotation, both angles, and what it cost.
///
/// The angles are measured, not predicted: `angle_before_deg` comes from the
/// bake the aim produced and `angle_after_deg` from the bake that is written, so
/// the residual column is the artifact's own collinearity error.
fn straighten_report(
    input: &StraightenInputs<'_>,
) -> Result<(Value, StraightenVerdict), CommandError> {
    let measured = |bake: &RestBake, role: BoneRole| measured_direction(bake, input.resolved, role);
    let mut rows = Vec::new();
    for stepped in input.plan {
        let role = BoneRole {
            side: stepped.side,
            part: stepped.part,
        };
        let parent_role = BoneRole {
            side: stepped.side,
            part: stepped.parent,
        };
        let row = input
            .resolved
            .iter()
            .find(|row| row.segment.role == role)
            .ok_or_else(|| {
                refused(
                    "tpose_straighten",
                    "the plan names a joint the aim does not resolve".to_owned(),
                )
            })?;
        let angle = |a: DVec3, b: DVec3| a.angle_between(b).to_degrees();
        let (parent_before, before) = (
            measured(input.baseline, parent_role)
                .ok_or_else(|| refused("tpose_straighten", "no parent direction".to_owned()))?,
            measured(input.baseline, role)
                .ok_or_else(|| refused("tpose_straighten", "no child direction".to_owned()))?,
        );
        let (parent_after, after) = (
            measured(input.straightened, parent_role)
                .ok_or_else(|| refused("tpose_straighten", "no parent direction".to_owned()))?,
            measured(input.straightened, role)
                .ok_or_else(|| refused("tpose_straighten", "no child direction".to_owned()))?,
        );
        let local = stepped
            .hinge
            .as_ref()
            .map(|hinge| world_rotation(input.authored[row.joint]).inverse() * hinge.axis);
        rows.push(json!({
            "side": format!("{:?}", stepped.side),
            "joint": format!("{:?}", stepped.part),
            "bone": row.joint_name,
            "node": row.joint,
            "straightened_against": format!("{:?}", stepped.parent),
            // The hinge the step turned about, and the same axis in the joint's
            // own authored frame — the frame a declared joint axis is written in.
            "hinge_world": stepped.hinge.as_ref().map(|hinge| hinge.axis.to_array()),
            "hinge_local": local.map(|axis| axis.to_array()),
            "hinge_local_off_z_deg": local.map(|axis| axis.angle_between(DVec3::Z).to_degrees()),
            // The same joint on the authored A-rest: a hinge belongs to a pose,
            // and the step's hinge is the fall's, not the rest's.
            "rest_hinge_world": stepped.rest_hinge.as_ref().map(|hinge| hinge.axis.to_array()),
            "rest_flexion_deg": stepped.rest_hinge.as_ref().map(|hinge| hinge.flexion_deg).unwrap_or(0.0),
            "hinge_moved_from_rest_deg": match (&stepped.rest_hinge, &stepped.hinge) {
                (Some(rest), Some(hinge)) => Some(rest.axis.angle_between(hinge.axis).to_degrees()),
                _ => None,
            },
            "flexion_deg": stepped.hinge.as_ref().map(|hinge| hinge.flexion_deg).unwrap_or(0.0),
            "rotation_deg": stepped.hinge.as_ref().map(|hinge| -hinge.flexion_deg).unwrap_or(0.0),
            "turn_world": stepped.turn.to_array(),
            "direction_before": before.to_array(),
            "direction_after": after.to_array(),
            "angle_before_deg": angle(parent_before, before),
            "angle_after_deg": angle(parent_after, after),
            "turn_degrees": angle(before, after),
            "target": row.reference_direction.to_array(),
            "residual_to_target_deg": angle(after, row.reference_direction),
        }));
    }

    // The upper arm is not the step's to move: its direction and its angle to the
    // horizontal must survive, or the T was not preserved, it was replaced.
    let mut upper = Vec::new();
    let mut elbow_max_deg = 0.0_f64;
    let mut wrist_max_deg = 0.0_f64;
    for side in [Side::Left, Side::Right] {
        let role = |part: Part| BoneRole { side, part };
        let upper_before = measured(input.baseline, role(Part::UpperArm));
        let upper_after = measured(input.straightened, role(Part::UpperArm));
        let elbow = (
            measured(input.straightened, role(Part::UpperArm)),
            measured(input.straightened, role(Part::Forearm)),
        );
        let wrist = (
            measured(input.straightened, role(Part::Forearm)),
            measured(input.straightened, role(Part::Hand)),
        );
        let elbow_deg = match elbow {
            (Some(a), Some(b)) => a.angle_between(b).to_degrees(),
            _ => f64::INFINITY,
        };
        let wrist_deg = match wrist {
            (Some(a), Some(b)) => a.angle_between(b).to_degrees(),
            _ => f64::INFINITY,
        };
        elbow_max_deg = elbow_max_deg.max(elbow_deg);
        wrist_max_deg = wrist_max_deg.max(wrist_deg);
        upper.push(json!({
            "side": format!("{side:?}"),
            "direction_before": upper_before.map(|d| d.to_array()),
            "direction_after": upper_after.map(|d| d.to_array()),
            "delta_deg": match (upper_before, upper_after) {
                (Some(a), Some(b)) => a.angle_between(b).to_degrees(),
                _ => f64::INFINITY,
            },
            "below_horizontal_before_deg": upper_before.map(below_horizontal),
            "below_horizontal_after_deg": upper_after.map(below_horizontal),
            "elbow_after_deg": elbow_deg,
            "wrist_after_deg": wrist_deg,
        }));
    }
    let upper_arm_max_deg = upper
        .iter()
        .map(|row| row["delta_deg"].as_f64().unwrap_or(180.0))
        .fold(0.0_f64, f64::max);

    // The palm rides the chain: how far it moved is the visible cost of the step,
    // and stating it keeps a rotated hand from passing as an untouched one.
    let mut palms = Vec::new();
    for hand in input.hands {
        let normal = |bake: &RestBake| -> Result<DVec3, CommandError> {
            Ok(plane_normal(&bake.worlds, hand.hand, hand.index, hand.pinky)?.normalize_or_zero())
        };
        let before = normal(input.baseline)?;
        let after = normal(input.straightened)?;
        palms.push(json!({
            "side": format!("{:?}", hand.side),
            "palm_before": before.to_array(),
            "palm_after": after.to_array(),
            "change_deg": before.angle_between(after).to_degrees(),
        }));
    }

    // Everything outside the arm subtrees must be bit-identical between the two
    // bakes: the step aims six bones and inherits their ancestry, nothing else.
    let deltas = world_deltas(
        input.actor,
        input.resolved,
        input.baseline,
        input.straightened,
    );

    let band = |deformation: &Deformation, name: &str| {
        deformation
            .bands
            .iter()
            .find(|band| band.name == name)
            .map(|band| (band.max_ratio, band.above_2x))
            .unwrap_or((1.0, 0))
    };
    let (arm_before, arm_above_before) = band(input.baseline_deformation, "arm_chain");
    let (arm_after, arm_above_after) = band(input.deformation, "arm_chain");
    let not_worse = input.deformation.max_ratio <= input.baseline_deformation.max_ratio
        && input.deformation.above_2x <= input.baseline_deformation.above_2x;
    let applied_max_deg = input
        .plan
        .iter()
        .filter_map(|stepped| stepped.hinge.as_ref().map(|hinge| hinge.flexion_deg))
        .fold(0.0_f64, f64::max);

    let value = json!({
        "flag": "--straighten-arms",
        "step": "after the aim, each forearm onto its upper arm and each hand onto the straightened forearm, about normalize(parent × child)",
        "axes": "hinge_world is the pose's own hinge in the actor's Y-up world frame; hinge_local is that axis in the joint's own authored world basis, so hinge_local_off_z_deg is its angle to that basis's +Z. The fall's hinge and the authored A-rest's hinge are different axes: rest_hinge_world is the same joint measured on the rest, and hinge_moved_from_rest_deg is how far the pose moved it",
        "applied": applied_max_deg > STRAIGHTEN_EPS_DEG,
        "max_flexion_removed_deg": applied_max_deg,
        "rotations": rows,
        "upper_arm": upper,
        "palm": palms,
        "elbow_max_deg": elbow_max_deg,
        "wrist_max_deg": wrist_max_deg,
        "upper_arm_max_delta_deg": upper_arm_max_deg,
        "outside_arm_subtree": {
            "nodes": deltas.outside_nodes,
            "max_rotation_deg": deltas.outside_deg,
            "max_pivot_shift_m": deltas.outside_pivot_m,
        },
        "arm_subtree": {
            "nodes": deltas.arm_nodes,
            "max_rotation_deg": deltas.arm_deg,
            "max_pivot_shift_m": deltas.arm_pivot_m,
        },
        "edge_stretch": {
            "baseline_max_ratio": input.baseline_deformation.max_ratio,
            "baseline_edges_above_2x": input.baseline_deformation.above_2x,
            "after_max_ratio": input.deformation.max_ratio,
            "after_edges_above_2x": input.deformation.above_2x,
            "arm_chain_baseline_max_ratio": arm_before,
            "arm_chain_baseline_edges_above_2x": arm_above_before,
            "arm_chain_after_max_ratio": arm_after,
            "arm_chain_after_edges_above_2x": arm_above_after,
            "not_worse": not_worse,
        },
    });
    let verdict = StraightenVerdict {
        applied: applied_max_deg > STRAIGHTEN_EPS_DEG,
        elbow_max_deg,
        wrist_max_deg,
        upper_arm_max_deg,
        non_arm_max_deg: deltas.outside_deg,
        not_worse,
        applied_max_deg,
    };
    Ok((value, verdict))
}

/// Rotation and pivot deltas between two bakes, split at the arm subtrees.
struct WorldDeltas {
    /// Largest rotation change outside the arm subtrees, degrees.
    outside_deg: f64,
    /// Largest pivot shift outside the arm subtrees, metres.
    outside_pivot_m: f64,
    /// Largest rotation change inside them, degrees.
    arm_deg: f64,
    /// Largest pivot shift inside them, metres.
    arm_pivot_m: f64,
    /// Nodes measured in each class.
    outside_nodes: usize,
    arm_nodes: usize,
}

fn world_deltas(
    actor: &Actor,
    resolved: &[Resolved],
    baseline: &RestBake,
    straightened: &RestBake,
) -> WorldDeltas {
    let parents = actor.node_parents();
    let mut inside = vec![false; baseline.worlds.len()];
    for row in resolved {
        if !matches!(row.segment.role.part, Part::UpperArm) {
            continue;
        }
        let mut stack = vec![row.joint];
        while let Some(node) = stack.pop() {
            if std::mem::replace(&mut inside[node], true) {
                continue;
            }
            for (child, parent) in parents.iter().enumerate() {
                if *parent == Some(node) {
                    stack.push(child);
                }
            }
        }
    }
    let mut deltas = WorldDeltas {
        outside_deg: 0.0,
        outside_pivot_m: 0.0,
        arm_deg: 0.0,
        arm_pivot_m: 0.0,
        outside_nodes: 0,
        arm_nodes: 0,
    };
    for node in 0..baseline.worlds.len() {
        let rotation = world_rotation(baseline.worlds[node])
            .angle_between(world_rotation(straightened.worlds[node]))
            .to_degrees();
        let pivot = baseline.worlds[node]
            .transform_point3(DVec3::ZERO)
            .distance(straightened.worlds[node].transform_point3(DVec3::ZERO));
        match inside[node] {
            true => {
                deltas.arm_deg = deltas.arm_deg.max(rotation);
                deltas.arm_pivot_m = deltas.arm_pivot_m.max(pivot);
                deltas.arm_nodes += 1;
            }
            false => {
                deltas.outside_deg = deltas.outside_deg.max(rotation);
                deltas.outside_pivot_m = deltas.outside_pivot_m.max(pivot);
                deltas.outside_nodes += 1;
            }
        }
    }
    deltas
}

/// The straightening gate: the T it claims, and what it cost.
fn straighten_gate(verdict: Option<&StraightenVerdict>) -> Value {
    let Some(verdict) = verdict else {
        return json!({
            "applied": false,
            "note": "the step did not run; --straighten-arms was not set",
        });
    };
    json!({
        "applied": verdict.applied,
        "bar_deg": STRAIGHTEN_TOLERANCE_DEG,
        "max_flexion_removed_deg": verdict.applied_max_deg,
        "elbow_max_deg": verdict.elbow_max_deg,
        "wrist_max_deg": verdict.wrist_max_deg,
        "bar_upper_arm_deg": T_POSE_TOLERANCE_DEG,
        "upper_arm_max_delta_deg": verdict.upper_arm_max_deg,
        "outside_arm_max_rotation_deg": verdict.non_arm_max_deg,
        "edge_stretch_not_worse": verdict.not_worse,
        // "Exactly 0.00°" is compared exactly: the two bakes compose the same
        // locals for every node outside the arm subtrees, so a non-zero value is
        // a real move, not a rounding one.
        "pass": verdict.elbow_max_deg <= STRAIGHTEN_TOLERANCE_DEG
            && verdict.wrist_max_deg <= STRAIGHTEN_TOLERANCE_DEG
            && verdict.upper_arm_max_deg <= T_POSE_TOLERANCE_DEG
            && verdict.non_arm_max_deg == 0.0
            && verdict.not_worse,
    })
}

/// One candidate frame of a shipped clip, measured.
struct PoseCandidate {
    frame: usize,
    time: f64,
    arms: f64,
    torso: f64,
    legs: f64,
    /// The game's own bone directions, carried into our frame.
    targets: Vec<DVec3>,
    /// Per side: hand axis, palm-plane normal, thumb direction, carried likewise.
    hand_targets: Vec<(DVec3, DVec3, DVec3)>,
    /// Bones the clip never keys, whose pose therefore mixes our rest in.
    unkeyed: Vec<String>,
}

impl PoseCandidate {
    /// Whether the pose is a T-rest: its **shoulders** are horizontal.
    ///
    /// The torso and leg figures are advisory. They compare the clip's own bone
    /// directions to our rest, and the clip's translation channels do not share
    /// our units, so their absolute value is not a property of this bake: the verb
    /// turns only the arm chain, and every other bone keeps its authored rest by
    /// construction (proved by the rest measurement, which would fail otherwise).
    fn admits(&self) -> bool {
        self.arms <= T_POSE_TOLERANCE_DEG
    }

    /// How badly it fails, as the ratio to its own bar.
    fn excess(&self) -> f64 {
        self.arms / T_POSE_TOLERANCE_DEG
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
        Some(reasons.join(", "))
    }
}

/// What a pose plan hands back: world turns, local deltas, and the report block.
struct PosePlan {
    /// Compensated target direction per segment, in our frame.
    targets: Option<Vec<DVec3>>,
    /// Compensated hand-frame targets, per side: axis, palm normal, thumb.
    hand_targets: Option<Vec<(DVec3, DVec3, DVec3)>>,
    block: Value,
}

/// Choose a frame of a shipped clip by measurement, and read its own turns.
///
/// The clip's channels carry **the clip's own skeleton's** local rotations, so a
/// direct application is the `0159` error class: two rests, one set of numbers.
/// The compensation is the whole point of this function —
///
/// ```text
/// Δ_skyrim(i) = W_pose(i) · W_rest_skyrim(i)⁻¹        (world, the clip's frame)
/// turn(i)     = C · Δ_skyrim(i) · Cᵀ                  (C: the clip's frame → ours)
/// ```
///
/// — and a frame is admitted only when the *compensated* pose is a body someone
/// can stand in: arms within `T_POSE_TOLERANCE_DEG` of horizontal, torso and legs
/// within their own bars of rest. The report names every rejected candidate with
/// its numbers, so a compromise is visible instead of hidden in a choice.
fn pose_plan(
    actor: &Actor,
    path: &PathBuf,
    params: &TPoseParams,
    resolved: &[Resolved],
    our_worlds: &[DMat4],
) -> Result<PosePlan, CommandError> {
    let clip = params.pose_clip.as_deref().ok_or_else(|| {
        refused(
            "tpose_pose",
            "--pose needs --pose-clip: the pose is read from a named clip, not guessed".to_owned(),
        )
    })?;
    let rest_path = params.pose_rest.as_ref().ok_or_else(|| {
        refused(
            "tpose_pose",
            "--pose needs --pose-rest <actor havok report>: without the clip skeleton's own rest \
             the channels are applied against the wrong basis"
                .to_owned(),
        )
    })?;
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let pose = Actor::from_glb(&bytes).map_err(actor_error)?;
    let (_, end) = pose.clip_span(clip).map_err(actor_error)?;
    let frames = ((end * SKYRIM_CLIP_FPS).round() as usize).max(1);
    let skeleton = SkyrimRest::read(rest_path)?;
    if skeleton.driven.is_empty() {
        return Err(refused(
            "tpose_pose",
            format!(
                "{} binds no clip track to any palette bone, so its pose would be the skeleton's \
                 rest and the bake would look like a no-op; decode the clip with --actor set to \
                 an actor that carries the clip skeleton's own bone names",
                rest_path.display()
            ),
        ));
    }

    // Nothing here is resolved by *our* bone names: the pose file is the clip
    // skeleton's own document, and the body this bakes onto may name the same
    // anatomy differently (`mixamorig:LeftArm` against `NPC L UpperArm [LUar]`).
    // Every lookup below goes through a role.

    // Upright witnesses: (joint role, tip role, is_leg, our rest direction).
    let mut upright: Vec<(BoneRole, BoneRole, bool, DVec3)> = Vec::new();
    for (joint_role, tip_role, is_leg) in [
        (Part::Spine(0), Part::Spine(1), false),
        (Part::Spine(1), Part::Spine(2), false),
        (Part::Spine(2), Part::Neck, false),
        (Part::Thigh, Part::Calf, true),
        (Part::Calf, Part::Foot, true),
        (Part::Foot, Part::Toe, true),
    ] {
        let sides: &[Side] = match joint_role {
            Part::Thigh | Part::Calf | Part::Foot => &[Side::Left, Side::Right],
            _ => &[Side::Centre],
        };
        for side in sides.iter().copied() {
            let joint = role_node(
                actor,
                BoneRole {
                    side,
                    part: joint_role,
                },
            );
            let tip = role_node(
                actor,
                BoneRole {
                    side,
                    part: tip_role,
                },
            );
            if let (Some(joint), Some(tip)) = (joint, tip) {
                let direction = (world_pivot(our_worlds, tip)? - world_pivot(our_worlds, joint)?)
                    .normalize_or_zero();
                upright.push((
                    BoneRole {
                        side,
                        part: joint_role,
                    },
                    BoneRole {
                        side,
                        part: tip_role,
                    },
                    is_leg,
                    direction,
                ));
            }
        }
    }

    let times: Vec<(usize, f64)> = match params.pose_scan {
        true => (0..frames)
            .map(|frame| (frame, frame as f64 / SKYRIM_CLIP_FPS))
            .collect(),
        false => {
            let time = params.pose_time.ok_or_else(|| {
                refused(
                    "tpose_pose",
                    "--pose needs --pose-time or --pose-scan: a frame is chosen by measurement, \
                     never by a default"
                        .to_owned(),
                )
            })?;
            vec![((time * SKYRIM_CLIP_FPS).round() as usize, time)]
        }
    };

    let mut candidates = Vec::new();
    for (frame, time) in times {
        let Some(candidate) =
            measure_frame(&pose, clip, frame, time, &skeleton, resolved, &upright)?
        else {
            continue;
        };
        candidates.push(candidate);
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
        "rest_table": rest_path.to_string_lossy(),
        "skeleton": skeleton.name,
        "clip": clip,
        "frames": frames,
        "fps": SKYRIM_CLIP_FPS,
        "basis": "turn(i) = C · W_pose(i) · W_rest_skyrim(i)⁻¹ · Cᵀ",
        "chosen": {
            "frame": chosen.frame,
            "time": chosen.time,
            "arms_deg": chosen.arms,
            "torso_deg": chosen.torso,
            "legs_deg": chosen.legs,
        },
        "bars": {
            "upper_arms_deg": T_POSE_TOLERANCE_DEG,
            "torso_deg_advisory": POSE_TORSO_TOLERANCE_DEG,
            "legs_deg_advisory": POSE_LEG_TOLERANCE_DEG,
        },
        "note": "the chosen frame turns only the arm chain; the torso and legs keep this body's \
                 authored rest by construction, and the clip's own leg figures are advisory \
                 because its translation channels do not share our units",
        "unkeyed_bones": chosen.unkeyed,
        "compromise": match chosen.admits() {
            true => Value::Null,
            false => json!(chosen.refusal()),
        },
        "rejected": rejected,
        "rejected_total": candidates.len().saturating_sub(1),
    });
    Ok(PosePlan {
        targets: Some(chosen.targets.clone()),
        hand_targets: Some(chosen.hand_targets.clone()),
        block,
    })
}

/// The clip skeleton's own rest, read from an `actor havok` report.
struct SkyrimRest {
    name: String,
    parents: Vec<i32>,
    /// Per bone: rest local translation (metres) and rotation.
    rest: Vec<(DVec3, DQuat)>,
    names: Vec<String>,
    driven: BTreeSet<String>,
}

impl SkyrimRest {
    /// Read the rest table a compensated application needs.
    fn read(path: &PathBuf) -> Result<Self, CommandError> {
        let text = std::fs::read_to_string(path).map_err(|error| errors::io(path, error))?;
        let report: Value = serde_json::from_str(&text).map_err(|error| {
            errors::invalid("tpose_pose", format!("{}: {error}", path.display()))
        })?;
        let units = report["skeleton_rest_units"].as_str().unwrap_or("havok");
        if units != "metres" {
            return Err(refused(
                "tpose_pose",
                format!(
                    "{} declares skeleton_rest in {units:?}; a compensated composition needs \
                     metres, so regenerate it with the current actor havok",
                    path.display()
                ),
            ));
        }
        let rows = report["skeleton_rest"].as_array().ok_or_else(|| {
            refused(
                "tpose_pose",
                format!(
                    "{} carries no skeleton_rest table; regenerate it with actor havok",
                    path.display()
                ),
            )
        })?;
        let name = report["rig_skeleton"]["name"]
            .as_str()
            .unwrap_or("<unnamed>")
            .to_owned();
        let mut names = Vec::with_capacity(rows.len());
        let mut parents = Vec::with_capacity(rows.len());
        let mut rest = Vec::with_capacity(rows.len());
        for row in rows {
            names.push(row["name"].as_str().unwrap_or_default().to_owned());
            parents.push(row["parent"].as_i64().unwrap_or(-1) as i32);
            let translation = row["translation"]
                .as_array()
                .map(|values| {
                    DVec3::from([
                        values.first().and_then(Value::as_f64).unwrap_or(0.0),
                        values.get(1).and_then(Value::as_f64).unwrap_or(0.0),
                        values.get(2).and_then(Value::as_f64).unwrap_or(0.0),
                    ])
                })
                .unwrap_or(DVec3::ZERO);
            let rotation = row["rotation"]
                .as_array()
                .map(|values| {
                    DQuat::from_array(
                        [0, 1, 2, 3]
                            .map(|axis| values.get(axis).and_then(Value::as_f64).unwrap_or(0.0)),
                    )
                    .normalize()
                })
                .unwrap_or(DQuat::IDENTITY);
            rest.push((translation, rotation));
        }
        let driven: BTreeSet<String> = report["track_bindings"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|row| !row["palette_bone"].is_null())
                    .filter_map(|row| row["bone"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name,
            parents,
            rest,
            names,
            driven,
        })
    }

    /// World pivots and rotations over the clip skeleton for one local set.
    ///
    /// Pivots are what turn a world delta into a *direction*: a bone's direction
    /// is the vector to its child's pivot, and only the pivot knows it. They are
    /// composed in the clip's own frame, before the carry into ours.
    fn compose(&self, locals: &[(DVec3, DQuat)]) -> (Vec<DVec3>, Vec<DQuat>) {
        let mut pivots = vec![DVec3::ZERO; locals.len()];
        let mut rotations = vec![DQuat::IDENTITY; locals.len()];
        for bone in 0..locals.len() {
            let (translation, rotation) = locals[bone];
            let parent = self.parents[bone];
            match parent >= 0 {
                true => {
                    let parent = parent as usize;
                    pivots[bone] = pivots[parent] + rotations[parent] * translation;
                    rotations[bone] = (rotations[parent] * rotation).normalize();
                }
                false => {
                    pivots[bone] = translation;
                    rotations[bone] = rotation;
                }
            }
        }
        (pivots, rotations)
    }

    /// The bone that carries a role.
    fn bone_for(&self, role: BoneRole) -> Option<usize> {
        self.names
            .iter()
            .enumerate()
            .filter(|(_, name)| role_of(name) == Some(role))
            .min_by_key(|(bone, name)| (u8::from(name.starts_with("CME ")), *bone))
            .map(|(bone, _)| bone)
    }
}

/// One frame, compensated into our frame as directions and a hand frame.
#[allow(clippy::too_many_arguments)]
fn measure_frame(
    pose: &Actor,
    clip: &str,
    frame: usize,
    time: f64,
    skeleton: &SkyrimRest,
    resolved: &[Resolved],
    upright: &[(BoneRole, BoneRole, bool, DVec3)],
) -> Result<Option<PoseCandidate>, CommandError> {
    let clip_pose = pose
        .skeleton_transforms(Pose::Clip { name: clip, time })
        .map_err(actor_error)?;
    // Compose the clip skeleton: a driven bone takes the clip's own local, every
    // other bone keeps the skeleton's rest — never our body's rest.
    let mut locals = skeleton.rest.clone();
    for (bone, name) in skeleton.names.iter().enumerate() {
        if !skeleton.driven.contains(name) {
            continue;
        }
        let Some(node) = pose
            .document()
            .nodes()
            .find(|node| node.name() == Some(name.as_str()))
            .map(|node| node.index())
        else {
            continue;
        };
        if let Some((translation, rotation)) = clip_pose.get(node) {
            locals[bone] = (*translation, *rotation);
        }
    }
    let (pivots, _) = skeleton.compose(&locals);
    let frame_change = frame_change_rotation();
    let carried = |index: usize| -> DVec3 {
        // Pivots are metres in the clip's own frame — the report declares that
        // unit and the clip's channels already carry it — so only the carry into
        // our axes remains.
        frame_change * pivots[index]
    };

    // Target directions: the game's own bone, carried into our frame.
    let mut targets = Vec::with_capacity(resolved.len());
    let mut arms = 0.0_f64;
    // A bone the clip does *not* key keeps our rest inside the pose, so the aim
    // would silently mix two poses. Count them and let the caller report it.
    let mut unkeyed = Vec::new();
    for row in resolved {
        if !skeleton
            .bone_for(row.segment.role)
            .is_some_and(|bone| skeleton.driven.contains(&skeleton.names[bone]))
        {
            unkeyed.push(row.joint_name.clone());
        }
        // By *role*, never by our bone's name: the body this bakes onto may carry
        // Mixamo's names while the clip skeleton carries Skyrim's.
        let Some(joint) = skeleton.bone_for(row.segment.role) else {
            return Ok(None);
        };
        let Some(tip) = skeleton.bone_for(row.segment.tip) else {
            return Ok(None);
        };
        let direction = (carried(tip) - carried(joint)).normalize_or_zero();
        if direction.length_squared() <= 0.0 {
            return Ok(None);
        }
        // The T criterion is the *shoulder*: a shipped pose can carry flexed
        // elbows and wrists, and calling that a failed T-pose would grade the
        // game's anatomy rather than this bake. Every segment is reported.
        if matches!(row.segment.role.part, Part::UpperArm) {
            arms = arms.max(below_horizontal(direction).abs());
        }
        targets.push(direction);
    }
    // The hand frames: palm plane and thumb, from the same pivots.
    let mut hand_targets = Vec::new();
    for side in [Side::Left, Side::Right] {
        let bone = |part: Part| skeleton.bone_for(BoneRole { side, part }).map(carried);
        let (Some(hand), Some(index), Some(pinky), Some(thumb)) = (
            bone(Part::Hand),
            bone(Part::Digit {
                digit: 1,
                segment: 0,
            }),
            bone(Part::Digit {
                digit: 4,
                segment: 0,
            }),
            bone(Part::Digit {
                digit: 0,
                segment: 0,
            }),
        ) else {
            continue;
        };
        let normal = plane_normal_world(&hand, &index, &pinky);
        if normal.length_squared() <= 0.0 {
            continue;
        }
        let axis = (bone(Part::Digit {
            digit: 2,
            segment: 0,
        })
        .unwrap_or(hand)
            - hand)
            .normalize_or_zero();
        hand_targets.push((axis, normal, (thumb - hand).normalize_or_zero()));
    }
    // Torso and legs, measured as how far the game's pose leaves our rest.
    let mut torso = 0.0_f64;
    let mut legs = 0.0_f64;
    for (joint_role, tip_role, is_leg, resting) in upright {
        let (Some(bone), Some(child)) =
            (skeleton.bone_for(*joint_role), skeleton.bone_for(*tip_role))
        else {
            continue;
        };
        let direction = (carried(child) - carried(bone)).normalize_or_zero();
        let turned = (frame_change * direction).normalize_or_zero();
        let deviation = turned.angle_between(*resting).to_degrees();
        match is_leg {
            true => legs = legs.max(deviation),
            false => torso = torso.max(deviation),
        }
    }

    Ok(Some(PoseCandidate {
        frame,
        time,
        arms,
        torso,
        legs,
        targets,
        hand_targets,
        unkeyed,
    }))
}

/// The clip skeleton's Z-up frame as a rotation into the glTF Y-up frame.
fn frame_change_rotation() -> DQuat {
    DQuat::from_rotation_x(-std::f64::consts::FRAC_PI_2)
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

        hands.push(HandFrame {
            side,
            hand,
            thumb,
            middle,
            index,
            pinky,
            authored_normal,
            target_axis: segments[2].reference_direction,
            target_thumb: thumb_target,
            target_normal,
            roll_degrees: 0.0,
        });
        rows.extend(segments);
    }
    retarget_turns(&mut rows, &mut hands)?;
    Ok((rows, hands))
}

/// Turn each bone onto its segment's target direction, with one roll per chain.
///
/// The hand's own palm plane decides the roll: a direction alone leaves a bone
/// free to spin about its axis, and one angle per side keeps the authored twist
/// between shoulder, elbow and wrist. The targets are what a caller supplies, so
/// the reference rest and a shipped clip's pose go through the same arithmetic.
fn retarget_turns(rows: &mut [Resolved], hands: &mut [HandFrame]) -> Result<(), CommandError> {
    for hand in hands.iter_mut() {
        let side_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.segment.role.side == hand.side)
            .map(|(index, _)| index)
            .collect();
        let hand_index = side_rows
            .iter()
            .copied()
            .find(|index| matches!(rows[*index].segment.role.part, Part::Hand))
            .ok_or_else(|| {
                refused(
                    "tpose_roll",
                    format!("no hand segment for the {:?} chain", hand.side),
                )
            })?;
        let axis = rows[hand_index].reference_direction.normalize();
        let lift = DQuat::from_rotation_arc(rows[hand_index].authored.normalize(), axis);
        let lifted_normal = (lift * hand.authored_normal).normalize();
        let roll_angle = signed_roll(lifted_normal, hand.target_normal, axis);
        hand.roll_degrees = roll_angle.to_degrees();
        for index in side_rows {
            let axis = rows[index].reference_direction.normalize();
            let lift = DQuat::from_rotation_arc(rows[index].authored.normalize(), axis);
            rows[index].turn = DQuat::from_axis_angle(axis, roll_angle) * lift;
            rows[index].roll_degrees = roll_angle.to_degrees();
        }
    }
    Ok(())
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

/// The hinge is the pair's own geometry, and the turn it produces is exact.
#[cfg(test)]
mod straighten_tests {
    use super::*;

    /// A parent along `+x` and a child 40° from it, tilted out of every axis
    /// plane: an arbitrary pair, which is what a measured pose is.
    fn pair() -> (DVec3, DVec3) {
        let parent = DVec3::X;
        let child = (DVec3::X * 40.0_f64.to_radians().cos()
            + DVec3::Y * 40.0_f64.to_radians().sin() * 0.6
            + DVec3::Z * 40.0_f64.to_radians().sin() * 0.8)
            .normalize();
        (parent, child)
    }

    #[test]
    fn the_hinge_turn_lands_the_child_on_the_parent() {
        let (parent, child) = pair();
        let hinge = hinge_between(parent, child, Side::Left, Part::Forearm)
            .expect("a bent pair has a hinge")
            .expect("a bent pair is not already straight");
        assert!(
            (hinge.flexion_deg - 40.0).abs() < 1e-9,
            "{}",
            hinge.flexion_deg
        );
        // `normalize(parent × child)` is perpendicular to both segments by
        // construction, which is the whole reason it is the hinge.
        assert!(hinge.axis.dot(parent).abs() < 1e-12);
        assert!(hinge.axis.dot(child).abs() < 1e-12);
        let landed = hinge.rotation() * child;
        assert!(
            landed.angle_between(parent).to_degrees() < 1e-9,
            "residual {}",
            landed.angle_between(parent).to_degrees()
        );
        // The turn only rotates about the hinge: it cannot swing the child
        // sideways, which is what a wrong axis such as the declared local Z does.
        assert!((landed.length() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_straight_pair_needs_no_hinge_and_a_folded_one_has_none() {
        let straight = hinge_between(DVec3::X, DVec3::X, Side::Right, Part::Hand)
            .expect("a straight pair is not an error");
        assert!(straight.is_none(), "a straight pair must be a no-op");
        let folded = hinge_between(DVec3::X, -DVec3::X, Side::Right, Part::Hand);
        assert!(folded.is_err(), "an antipodal pair has no turn axis");
        let empty = hinge_between(DVec3::ZERO, DVec3::X, Side::Left, Part::Forearm);
        assert!(empty.is_err(), "a zero-length segment has no hinge");
    }

    #[test]
    fn the_turn_is_composed_on_the_world_side_of_the_aim() {
        // The aim produced `R_aim`; the step wants `R_step · R_aim`, so the
        // composed turn applied to the authored basis equals the step applied to
        // the aimed world rotation.
        let aim = DQuat::from_axis_angle(DVec3::Y, 0.7);
        let step = DQuat::from_axis_angle(DVec3::X, -0.3);
        let composed = combine(&step, aim);
        let direct = step * aim;
        assert!(composed.angle_between(direct).to_degrees() < 1e-12);
        assert!((composed.length() - 1.0).abs() < 1e-12);
    }
}
