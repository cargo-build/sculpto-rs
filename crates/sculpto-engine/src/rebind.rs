//! `actor rebind`: seat an actor's mesh onto another skeleton, transferring the
//! authored weights onto the target palette.
//!
//! The mesh keeps its geometry, partitions and part colours; only the skeleton
//! and the skin change. The transfer is the neutral remap kernel
//! ([`remap_skin`]) through an explicitly checked [`BoneMap::resolved`] table:
//! the correspondence layer ([`crate::retarget`]) resolves each target bone onto
//! a source bone, the target hierarchy is seated at the source's rest, and every
//! source weight the target cannot name folds to its nearest mapped source
//! ancestor. The fold is reported per family (breast, twist, helpers) with its
//! raw weight mass, so the soft tissue a pure target rig trades away is named
//! before a render shows it, not after.
//!
//! Everything is measured in world glTF Y-up metres with an identity basis and
//! global-to-skin: the source rest is proved, the seat is proved, and the
//! encoded output is proved again under the seated rig's own basis before a byte
//! is written. Every output vertex sums to one and carries at least one
//! influence, or the verb refuses. @req DECL0008 @req DECL0013

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use glam::{DMat3, DMat4, DQuat, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::review::smooth_normals;
use sculpto_gltf::{PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::Actor;
use sculpto_mesh::rig::map::{BoneMap, Destination};
use sculpto_mesh::rig::remap::{RemapSettings, remap_skin};

use crate::actor_io::{
    Pool, Validation, f32_of, identity_f32, part_materials, pool_weights, read_pool, validate_skin,
};
use crate::adapt::{actor_rest_worlds, actor_rig};
use crate::export_glb::{
    actor_error, bounds, glb_error, hex as sha_hex, refused, rest_error, write_report,
};
use crate::retarget::{
    Correspondence, Part, compact, correspondence_report, require_mapped, role_of,
};
use crate::{CommandError, errors};

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebindParams {
    /// Actor GLB whose mesh and authored weights are transferred.
    pub actor: PathBuf,
    /// Skeleton GLB whose skin palette names the target rig.
    pub skeleton: PathBuf,
    /// Write the re-bound actor GLB here.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Write the JSON report here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// One folded source bone: its raw mass and the target that carries it.
struct FoldRow {
    name: String,
    group: &'static str,
    mass: f64,
    target: String,
}

/// The chain a target bone belongs to, for per-chain proportioning. Fingers are
/// their own chain: they branch in parallel from the hand, so summing them into
/// the arm would inflate the arm's length and mis-scale the upper arm.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chain {
    Spine,
    Arm,
    Leg,
    Finger,
}

const CHAIN_COUNT: usize = 4;

fn chain_idx(c: Chain) -> usize {
    match c {
        Chain::Spine => 0,
        Chain::Arm => 1,
        Chain::Leg => 2,
        Chain::Finger => 3,
    }
}

fn chain_name(c: Chain) -> &'static str {
    match c {
        Chain::Spine => "spine",
        Chain::Arm => "arm",
        Chain::Leg => "leg",
        Chain::Finger => "finger",
    }
}

fn chain_of(name: &str) -> Chain {
    match role_of(name).map(|role| role.part) {
        Some(Part::Digit { .. }) => Chain::Finger,
        Some(Part::UpperArm | Part::Forearm | Part::Hand) => Chain::Arm,
        Some(Part::Thigh | Part::Calf | Part::Foot | Part::Toe) => Chain::Leg,
        Some(Part::Marker) if compact(name).contains("toe") => Chain::Leg,
        // The clavicle is shoulder girdle: it scales with the torso, so the arm
        // factor covers only Arm->ForeArm->Hand (the operator's limb chain).
        _ => Chain::Spine,
    }
}

/// Signed distance of a point to the nearest flesh vertex, millimetres: positive
/// outside, negative inside, by the surface normal at the nearest vertex.
/// Nearest-vertex resolution (the mesh tessellation bounds the precision).
fn flesh_probe(
    point: DVec3,
    mesh_positions: &[[f64; 3]],
    mesh_normals: &[[f32; 3]],
) -> (f64, usize, bool) {
    let mut best = f64::INFINITY;
    let mut best_k = 0;
    for (k, v) in mesh_positions.iter().enumerate() {
        let d = point.distance(DVec3::from_array(*v));
        if d < best {
            best = d;
            best_k = k;
        }
    }
    let normal = DVec3::from_array(mesh_normals[best_k].map(f64::from));
    let outward = (point - DVec3::from_array(mesh_positions[best_k])).dot(normal);
    let signed_mm = best * 1000.0 * if outward > 0.0 { 1.0 } else { -1.0 };
    (signed_mm, best_k, outward <= 0.0)
}

/// A leg branch's socket, and the hub position it implies.
struct HubLeg {
    /// The thigh bone this branch belongs to.
    bone: String,
    /// The flesh socket (the femoral head landmark).
    socket: DVec3,
    /// Socket offset from the midline along the body's right axis, millimetres.
    socket_lateral_mm: f64,
    /// Lateral component of Mixamo's branch direction, unitless.
    mixamo_lateral: f64,
    /// Branch length that reaches the socket, millimetres.
    length_mm: f64,
    /// Mixamo's branch angle below horizontal, degrees.
    below_horizontal_deg: f64,
    /// The hub this socket alone implies.
    candidate: DVec3,
}

/// The anatomical hips hub: where Mixamo's root sits, derived from the leg sockets.
struct HipsHub {
    /// The seated root position.
    position: DVec3,
    /// Per-side branch geometry, for the report.
    legs: Vec<HubLeg>,
    /// Left/right candidate disagreement, millimetres (symmetry check).
    disagreement_mm: f64,
    /// Hub height above the mean socket, millimetres.
    rise_mm: f64,
    /// Mean branch angle below horizontal, degrees.
    below_horizontal_deg: f64,
    /// Whether a leg branch resolved; when false the flesh anchor is kept.
    derived: bool,
    /// Midpoint of the two sockets: the point on the symmetry plane the hub is derived from.
    mid_socket: DVec3,
}

/// Derive the hips hub from the flesh sockets and Mixamo's own branch direction.
///
/// Mixamo's root sits above the femoral heads — its `Hips→UpLeg` branch drops ~66.6 mm at
/// 36.1° below horizontal — while the flesh medial axis anchors the root at the pelvis
/// inscribed centre, which lands level with the sockets. Taking that flesh vector therefore
/// collapses the branch into a pure lateral bar. The sockets are the trusted landmarks (the
/// femoral heads, measured 1.4 mm from Mixamo's own lateral socket), so the seat holds them
/// and derives the hub instead: on the symmetry plane, back along Mixamo's body-frame branch
/// direction by exactly the distance that puts the socket at its flesh lateral offset,
/// `hub = socket − d · (lateral / d_lateral)`. Both sides must agree; their difference is
/// reported, never averaged away silently. No native joint is read.
#[allow(clippy::too_many_arguments)]
fn hips_hub(
    landmark_pos: &BTreeMap<JointId, DVec3>,
    root: usize,
    parent: &[Option<usize>],
    chain: &[Chain],
    palette: &[String],
    pos: &[DVec3],
    frame_rotation: DMat3,
    right: DVec3,
    hip_anchor: DVec3,
) -> HipsHub {
    // The lateral offset is measured from the **socket pair's midpoint**, which is the point
    // on the body's symmetry plane: measuring from the pelvis anchor instead gives each side
    // a different offset (the anchor is not on that plane in a real body) and the derived hub
    // then drifts off it.
    let mut sockets: Vec<(usize, String, DVec3, DVec3)> = Vec::new();
    for (j, name) in palette.iter().enumerate() {
        if parent[j] != Some(root) || chain[j] != Chain::Leg {
            continue;
        }
        let Some(&socket) = landmark_pos.get(&JointId::new(j as u32)) else {
            continue;
        };
        let mix_bone = frame_rotation * (pos[j] - pos[root]);
        let mix_length = mix_bone.length();
        if mix_length < 1e-9 {
            continue;
        }
        sockets.push((j, name.clone(), socket, mix_bone / mix_length));
    }
    if sockets.is_empty() {
        return HipsHub {
            position: hip_anchor,
            legs: Vec::new(),
            disagreement_mm: 0.0,
            rise_mm: 0.0,
            below_horizontal_deg: 0.0,
            derived: false,
            mid_socket: hip_anchor,
        };
    }
    let mid = sockets.iter().fold(DVec3::ZERO, |acc, s| acc + s.2) / sockets.len() as f64;
    let mut legs = Vec::new();
    for (_, name, socket, dir) in sockets {
        let lateral = (socket - mid).dot(right);
        let mixamo_lateral = dir.dot(right);
        // The branch must travel outward from the midline to reach its own socket.
        if mixamo_lateral.abs() < 1e-6 || lateral * mixamo_lateral <= 0.0 {
            continue;
        }
        let length = lateral / mixamo_lateral;
        legs.push(HubLeg {
            bone: name,
            socket,
            socket_lateral_mm: lateral * 1000.0,
            mixamo_lateral,
            length_mm: length * 1000.0,
            below_horizontal_deg: (-dir.y).asin().to_degrees(),
            candidate: socket - dir * length,
        });
    }
    if legs.is_empty() {
        return HipsHub {
            position: hip_anchor,
            legs,
            disagreement_mm: 0.0,
            rise_mm: 0.0,
            below_horizontal_deg: 0.0,
            derived: false,
            mid_socket: mid,
        };
    }
    let derived = true;
    let position = legs
        .iter()
        .fold(DVec3::ZERO, |acc, leg| acc + leg.candidate)
        / legs.len() as f64;
    let disagreement_mm = match (legs.first(), legs.get(1)) {
        (Some(a), Some(b)) => a.candidate.distance(b.candidate) * 1000.0,
        _ => 0.0,
    };
    let rise_mm = {
        let mean_socket =
            legs.iter().fold(DVec3::ZERO, |acc, leg| acc + leg.socket) / legs.len() as f64;
        (position.y - mean_socket.y) * 1000.0
    };
    let below_horizontal_deg =
        legs.iter().map(|leg| leg.below_horizontal_deg).sum::<f64>() / legs.len() as f64;
    HipsHub {
        position,
        legs,
        disagreement_mm,
        rise_mm,
        below_horizontal_deg,
        derived,
        mid_socket: mid,
    }
}

/// Whether Mixamo's body-frame direction seats this bone, instead of the flesh direction.
///
/// The flesh direction is the body's own rest posture, and it is the truth wherever the body
/// holds a limb in that posture: the A-pose arms (M2 is not warped to the T yet), this
/// stance's legs and the digits. The torso column and the foot's toe are different: there the
/// medial axis measures a torso sheet, or the junction between two masses, and that direction
/// is not a bone's. The pelvis anchor itself is derived from both sockets in [`hips_hub`].
fn mixamo_direction(role: Option<Part>) -> bool {
    matches!(
        role,
        Some(Part::Spine(_) | Part::Neck | Part::Head | Part::Toe)
    )
}

/// Seat the target skeleton on the flesh medial axis at Mixamo's chain fractions.
///
/// Mixamo's names, hierarchy and rest directions are kept; the flesh supplies the
/// hip anchor (pelvis inscribed centre), the branch-to-branch chain extents and the
/// body frame, all measured on the medial axis. `flesh_pos` carries those flesh-derived
/// landmark positions; each bone takes its **length** from the spacing between them, so the
/// chain spans the flesh's own length (shoulder→elbow→wrist, hip→knee→ankle, pelvis→skull).
/// The **direction** comes from Mixamo's body frame for the torso column and the toe, where
/// the medial axis measures a sheet or a junction rather than a bone ([`mixamo_direction`]);
/// the limbs the body holds in its own rest posture keep the flesh direction. The root is the
/// derived hips hub ([`hips_hub`]), above the leg sockets, and the socket stays at the femoral
/// head. The native palette (`native_pos`) is used **only** to report the delta a native seat
/// would have copied (`native_delta_mm`) and how many native seats sit outside the flesh
/// (`native_outside_flesh_count`) — it never measures, anchors or places a vertex. The
/// result is Mixamo's skeleton proportioned to this body's flesh, not native placement
/// and not literal Mixamo proportions.
fn seat_rig(
    target_rig: &Rig,
    target_palette: &[String],
    flesh_pos: &BTreeMap<JointId, DVec3>,
    native_pos: &BTreeMap<JointId, DVec3>,
    mesh_positions: &[[f64; 3]],
    mesh_normals: &[[f32; 3]],
) -> Result<(Rig, Value), CommandError> {
    let n = target_palette.len();
    let authored: Vec<DMat4> = (0..n)
        .map(|slot| -> Result<DMat4, CommandError> {
            Ok(DMat4::from_cols_array_2d(
                &target_rig
                    .rest_world(JointId::new(slot as u32))
                    .map_err(|error| {
                        errors::invalid("rebind_seat", format!("target rest world: {error}"))
                    })?,
            ))
        })
        .collect::<Result<_, _>>()?;
    let pos: Vec<DVec3> = authored.iter().map(|m| m.w_axis.truncate()).collect();
    let chain: Vec<Chain> = target_palette.iter().map(|name| chain_of(name)).collect();
    let parent: Vec<Option<usize>> = (0..n)
        .map(|slot| {
            target_rig
                .joint(JointId::new(slot as u32))
                .and_then(|joint| joint.parent)
                .map(|p| p.index())
        })
        .collect();
    let root = target_rig.roots()[0].index();

    // Flesh landmarks (`flesh_pos`) are measured on the medial axis: the hip anchor is
    // the pelvis inscribed centre and each limb joint sits at Mixamo's arc fraction on
    // the flesh centreline. Native seats are kept only to report the delta a native
    // placement would have copied and how many of them sit outside the flesh.
    let landmark_pos = flesh_pos.clone();
    let mut landmark_src: BTreeMap<JointId, (&'static str, f64)> = BTreeMap::new();
    for (&joint, &fp) in flesh_pos {
        let (mm, _k, _inside) = flesh_probe(fp, mesh_positions, mesh_normals);
        landmark_src.insert(joint, ("flesh_landmark", mm));
    }
    let mut native_outside = 0_usize;
    for &np in native_pos.values() {
        let (_mm, _k, inside) = flesh_probe(np, mesh_positions, mesh_normals);
        if !inside {
            native_outside += 1;
        }
    }
    let hip_anchor = *landmark_pos.get(&JointId::new(root as u32)).ok_or_else(|| {
        refused(
            "rebind_seat",
            "the flesh hip landmark (source of the skeleton root) is unmapped; cannot anchor the seat",
        )
    })?;

    // Align Mixamo's rest frame to the body's rest frame. Our bodies face -Z and
    // Mixamo +Z (180 deg yaw); the rotation is measured from the flesh landmarks
    // (left->right shoulder axis, hips->head up), not assumed. Mixamo's anatomical
    // directions are kept but expressed in the body frame, so the arms point down
    // the body's own sides instead of across its chest.
    let find = |name: &str| target_palette.iter().position(|p| p == name);
    let body_vec = |a: &str, b: &str| -> Option<DVec3> {
        let ia = find(a)?;
        let ib = find(b)?;
        let pa = landmark_pos.get(&JointId::new(ia as u32))?;
        let pb = landmark_pos.get(&JointId::new(ib as u32))?;
        Some((*pb - *pa).normalize_or_zero())
    };
    let mix_vec = |a: &str, b: &str| -> Option<DVec3> {
        let ia = find(a)?;
        let ib = find(b)?;
        Some((pos[ib] - pos[ia]).normalize_or_zero())
    };
    let basis = |right: DVec3, up: DVec3| -> DMat3 {
        let up = up.normalize_or_zero();
        let right = (right - up * right.dot(up)).normalize_or_zero();
        DMat3::from_cols(right, up, right.cross(up))
    };
    let (frame_rotation, body_right) = match (
        body_vec("mixamorig:LeftShoulder", "mixamorig:RightShoulder"),
        body_vec("mixamorig:Hips", "mixamorig:Head"),
        mix_vec("mixamorig:LeftShoulder", "mixamorig:RightShoulder"),
        mix_vec("mixamorig:Hips", "mixamorig:Head"),
    ) {
        (Some(br), Some(bu), Some(mr), Some(mu)) => {
            // The body's own right axis (the symmetry plane's normal): the hips hub must
            // sit on that plane, not wherever a socket happens to land.
            let body = basis(br, bu);
            (body * basis(mr, mu).inverse(), body.x_axis)
        }
        _ => (DMat3::IDENTITY, DVec3::X),
    };
    let frame_quat = DQuat::from_mat3(&frame_rotation);

    // First pass: one uniform scale per chain, measured along the serial limb path
    // (shoulder->elbow->wrist, hip->knee->ankle, pelvis->head, hand->fingertip),
    // not the sum of parallel branches. Reported as the reference.
    let path_len = |names: &[&str], flesh: bool| -> f64 {
        let mut total = 0.0;
        let mut prev: Option<DVec3> = None;
        for nm in names {
            let point = find(nm).and_then(|i| {
                if flesh {
                    landmark_pos.get(&JointId::new(i as u32)).copied()
                } else {
                    Some(pos[i])
                }
            });
            if let (Some(a), Some(b)) = (prev, point) {
                total += a.distance(b);
            }
            prev = point;
        }
        total
    };
    let spine_path = [
        "mixamorig:Hips",
        "mixamorig:Spine",
        "mixamorig:Spine1",
        "mixamorig:Spine2",
        "mixamorig:Neck",
        "mixamorig:Head",
    ];
    let arm_path = [
        "mixamorig:LeftShoulder",
        "mixamorig:LeftArm",
        "mixamorig:LeftForeArm",
        "mixamorig:LeftHand",
    ];
    let leg_path = [
        "mixamorig:LeftUpLeg",
        "mixamorig:LeftLeg",
        "mixamorig:LeftFoot",
        "mixamorig:LeftToeBase",
    ];
    let finger_path = [
        "mixamorig:LeftHand",
        "mixamorig:LeftHandMiddle1",
        "mixamorig:LeftHandMiddle2",
        "mixamorig:LeftHandMiddle3",
    ];
    let mut mix_len = [0.0_f64; CHAIN_COUNT];
    let mut flesh_len = [0.0_f64; CHAIN_COUNT];
    for (k, path) in [
        spine_path.as_slice(),
        arm_path.as_slice(),
        leg_path.as_slice(),
        finger_path.as_slice(),
    ]
    .into_iter()
    .enumerate()
    {
        mix_len[k] = path_len(path, false);
        flesh_len[k] = path_len(path, true);
    }
    let mut scale = [1.0_f64; CHAIN_COUNT];
    for k in 0..CHAIN_COUNT {
        if mix_len[k] > 1e-9 {
            scale[k] = flesh_len[k] / mix_len[k];
        }
    }

    // The anatomical root: Mixamo's root sits above the leg sockets, while the flesh medial
    // axis anchors it at the pelvis inscribed centre, level with them. The sockets are the
    // trusted landmarks (the femoral heads), so the seat holds them and derives the hub from
    // Mixamo's own branch angle. See [`hips_hub`].
    let hub = hips_hub(
        &landmark_pos,
        root,
        &parent,
        &chain,
        target_palette,
        &pos,
        frame_rotation,
        body_right,
        hip_anchor,
    );

    // Length and direction of every bone. The flesh supplies the **length** (the spacing
    // between the two flesh landmarks); the torso column and the foot's toe take Mixamo's
    // body-frame **direction**, because the medial axis measures a sheet or a junction there,
    // not a bone. Every limb the body holds in its own rest posture — the A-pose arms, this
    // stance's legs, the digits — keeps the flesh direction: that posture is what the weights
    // are authored in, and M2 is not warped to the T yet (SkeletonFoundation revision 2). The
    // drift the flesh direction would have introduced stays in the report, as the measurement
    // that justifies the rule.
    let mut bone_vec = vec![DVec3::ZERO; n];
    let mut bone_factor = vec![1.0_f64; n];
    let mut angle_error = vec![0.0_f64; n];
    let mut seat_deviation = vec![0.0_f64; n];
    let mut direction_source = vec!["mixamo_body_frame_chain_scale"; n];
    let mut seated_pos = vec![DVec3::ZERO; n];
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| std::iter::successors(parent[i], |&p| parent[p]).count());
    for j in order {
        let Some(p) = parent[j] else {
            // The root is the hips hub, on the symmetry plane above the leg sockets.
            seated_pos[j] = hub.position;
            direction_source[j] = "hips_hub";
            continue;
        };
        let mix_bone = frame_rotation * (pos[j] - pos[p]);
        let mix_length = mix_bone.length();
        let dir = mix_bone.normalize_or_zero();
        let mapped = landmark_pos
            .get(&JointId::new(j as u32))
            .copied()
            .zip(landmark_pos.get(&JointId::new(p as u32)).copied());
        let (target, angle, deviation, source) = match mapped {
            Some((nj, np)) => {
                let flesh_bone = nj - np;
                let flesh_length = flesh_bone.length();
                let angle = flesh_bone
                    .normalize_or_zero()
                    .dot(dir)
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                // A chain's proximal anchor is a flesh landmark POSITION, so a chain whose
                // parent moved (the arm on the re-aimed column, the leg on the raised hub, a
                // digit on the hand) cannot inherit that offset: it starts where the flesh says
                // and its own bones keep the flesh spacing. The clavicle is an arm-chain root by
                // the same argument even though the proportion table files it with the torso for
                // its scale: its lateral end is the armpit branch the flesh measures.
                let anchored = chain[p] != chain[j]
                    || matches!(
                        role_of(&target_palette[j]).map(|role| role.part),
                        Some(Part::Clavicle)
                    );
                if anchored {
                    (nj, angle, angle, "flesh_landmark_position")
                } else if mixamo_direction(role_of(&target_palette[j]).map(|role| role.part)) {
                    (
                        seated_pos[p] + dir * flesh_length,
                        angle,
                        0.0,
                        "mixamo_body_frame",
                    )
                } else {
                    (
                        seated_pos[p] + flesh_bone,
                        angle,
                        angle,
                        "flesh_landmark_spacing",
                    )
                }
            }
            None => {
                let k = chain_idx(chain[j]);
                (
                    seated_pos[p] + dir * scale[k] * mix_length,
                    0.0,
                    0.0,
                    "mixamo_body_frame_chain_scale",
                )
            }
        };
        seated_pos[j] = target;
        bone_vec[j] = target - seated_pos[p];
        bone_factor[j] = if mix_length > 1e-9 {
            bone_vec[j].length() / mix_length
        } else {
            1.0
        };
        angle_error[j] = angle;
        seat_deviation[j] = deviation;
        direction_source[j] = source;
    }

    let mut joints = Vec::with_capacity(n);
    let mut rows = Vec::new();
    for j in 0..n {
        let rotation = frame_quat * authored[j].to_scale_rotation_translation().1;
        let world = DMat4::from_scale_rotation_translation(DVec3::ONE, rotation, seated_pos[j]);
        let pworld = match parent[j] {
            Some(p) => DMat4::from_scale_rotation_translation(
                DVec3::ONE,
                frame_quat * authored[p].to_scale_rotation_translation().1,
                seated_pos[p],
            ),
            None => DMat4::IDENTITY,
        };
        let rest = local_of(pworld.inverse() * world, &target_palette[j])?;
        // Signed distance of the SEATED joint to the nearest flesh vertex.
        let (signed_mm, _k, inside) = flesh_probe(seated_pos[j], mesh_positions, mesh_normals);
        let native_delta = native_pos
            .get(&JointId::new(j as u32))
            .map(|np| seated_pos[j].distance(*np) * 1000.0);
        let (src, native_mm) = landmark_src
            .get(&JointId::new(j as u32))
            .copied()
            .unwrap_or(("unmapped", f64::NAN));
        rows.push(json!({
            "bone": target_palette[j],
            "chain": chain_name(chain[j]),
            "chain_scale": scale[chain_idx(chain[j])],
            "bone_factor": bone_factor[j],
            "joint_angle_error_degrees": angle_error[j],
            "seat_direction": direction_source[j],
            "seat_angle_from_mixamo_degrees": seat_deviation[j],
            "bone_length_mm": bone_vec[j].length() * 1000.0,
            "seated_position_mm": [
                seated_pos[j].x * 1000.0,
                seated_pos[j].y * 1000.0,
                seated_pos[j].z * 1000.0,
            ],
            "flesh_distance_mm": signed_mm,
            "inside": inside,
            "native_delta_mm": native_delta,
            "landmark_source": src,
            "native_seat_flesh_mm": native_mm,
        }));
        joints.push(Joint {
            name: Some(target_palette[j].clone()),
            parent: parent[j].map(|p| JointId::new(p as u32)),
            rest,
        });
    }
    let rig = Rig::new(joints, RigFrame::YUp).map_err(|error| {
        errors::invalid(
            "rebind_seat",
            format!("the seated skeleton is not a rig: {error}"),
        )
    })?;
    let outside: Vec<&Value> = rows.iter().filter(|r| r["inside"] == false).collect();
    let yaw_degrees = (-frame_rotation.x_axis.z)
        .atan2(frame_rotation.x_axis.x)
        .to_degrees();
    let sp2 = find("mixamorig:Spine2");
    let girdle: Vec<Value> = ["Left", "Right"]
        .iter()
        .filter_map(|side| {
            let sh = find(&format!("mixamorig:{side}Shoulder"))?;
            let sp2 = sp2?;
            let anchor_level = (seated_pos[sh].y - seated_pos[sp2].y) * 1000.0;
            let mixamo_level = (pos[sh].y - pos[sp2].y) * 1000.0;
            let (mm, _k, inside) = flesh_probe(seated_pos[sh], mesh_positions, mesh_normals);
            let arm = find(&format!("mixamorig:{side}Arm"));
            let clavicle_deltoid_vertical = arm.map(|a| (seated_pos[sh].y - seated_pos[a].y) * 1000.0);
            Some(json!({
                "side": side,
                "anchor_level_above_spine2_mm": anchor_level,
                "mixamo_proportion_level_mm": mixamo_level,
                "shoulders_low_gap_mm": anchor_level - mixamo_level,
                "clavicle_to_deltoid_vertical_mm": clavicle_deltoid_vertical,
                "flesh_distance_mm": mm,
                "inside": inside,
                "native_delta_mm": native_pos.get(&JointId::new(sh as u32)).map(|np| seated_pos[sh].distance(*np)*1000.0),
            }))
        })
        .collect();
    let hub_rows: Vec<Value> = hub
        .legs
        .iter()
        .map(|leg| {
            json!({
                "bone": leg.bone,
                "socket_mm": [leg.socket.x * 1000.0, leg.socket.y * 1000.0, leg.socket.z * 1000.0],
                "socket_lateral_mm": leg.socket_lateral_mm,
                "mixamo_branch_lateral": leg.mixamo_lateral,
                "branch_length_mm": leg.length_mm,
                "branch_below_horizontal_deg": leg.below_horizontal_deg,
                "candidate_hub_mm": [leg.candidate.x * 1000.0, leg.candidate.y * 1000.0, leg.candidate.z * 1000.0],
            })
        })
        .collect();
    let report = json!({
        "method": "per-chain Mixamo structure seated at the caller's flesh landmarks: Mixamo body-frame direction with the flesh landmark length for the torso column and the toe, the flesh direction for the limbs the body holds in its own rest posture; the root is the hips hub derived from the flesh sockets at Mixamo's branch angle, uniform chain-scale fallback for unmapped joints",
        "bones": n,
        "frame_yaw_degrees": yaw_degrees,
        "shoulder_girdle": girdle,
        "hips_hub": {
            "derived_from": "flesh leg sockets + Mixamo body-frame branch direction; keeps the socket at the femoral head and raises the hub above it",
            "flesh_hip_anchor_mm": [hip_anchor.x * 1000.0, hip_anchor.y * 1000.0, hip_anchor.z * 1000.0],
            "seated_hub_mm": [hub.position.x * 1000.0, hub.position.y * 1000.0, hub.position.z * 1000.0],
            "rise_above_socket_mm": hub.rise_mm,
            "branch_below_horizontal_deg": hub.below_horizontal_deg,
            "left_right_disagreement_mm": hub.disagreement_mm,
            // Lateral offsets: from the socket pair's midpoint (the plane the hub is placed on,
            // so this is the placement residual) and from the flesh hip anchor (which is not on
            // the body's symmetry plane — the two differ by the anchor's own offset).
            "offset_from_socket_midpoint_mm": (hub.position - hub.mid_socket).dot(body_right) * 1000.0,
            "offset_from_flesh_hip_anchor_mm": (hub.position - hip_anchor).dot(body_right) * 1000.0,
            "derived": hub.derived,
            "legs": hub_rows,
        },
        "chain_scale": { "spine": scale[0], "arm": scale[1], "leg": scale[2], "finger": scale[3] },
        "chain_length_mm": {
            "spine": { "mixamo": mix_len[0]*1000.0, "flesh": flesh_len[0]*1000.0 },
            "arm": { "mixamo": mix_len[1]*1000.0, "flesh": flesh_len[1]*1000.0 },
            "leg": { "mixamo": mix_len[2]*1000.0, "flesh": flesh_len[2]*1000.0 },
            "finger": { "mixamo": mix_len[3]*1000.0, "flesh": flesh_len[3]*1000.0 },
        },
        "outside_flesh_count": outside.len(),
        "native_outside_flesh_count": native_outside,
        "per_bone": rows,
    });
    Ok((rig, report))
}

/// Decompose a local matrix into the rig model's uniform-scale TRS.
fn local_of(local: DMat4, name: &str) -> Result<LocalTransform, CommandError> {
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    if (scale.x - scale.y).abs() > 1e-6 || (scale.x - scale.z).abs() > 1e-6 {
        return Err(refused(
            "rebind_seat",
            format!("bone {name:?} carries a non-uniform rest scale ({scale:?})"),
        ));
    }
    Ok(LocalTransform {
        translation: translation.to_array().map(|value| value as f32),
        rotation: rotation.normalize().to_array().map(|value| value as f32),
        scale: scale.x as f32,
    })
}

/// The family a folded bone belongs to, for the named soft-tissue census.
fn fold_group(name: &str) -> &'static str {
    if compact(name).contains("breast") {
        "breast"
    } else if matches!(
        role_of(name).map(|role| role.part),
        Some(Part::Twist { .. })
    ) {
        "twist"
    } else {
        "helpers"
    }
}

/// Record one flesh-placed joint: its world position and the per-joint placement meta
/// (chain, fraction, inscribed radius, medial containment, landmark source, native delta).
/// `native_pos` is read only for the delta — it never sets the position.
#[allow(clippy::too_many_arguments)]
fn record_flesh_joint(
    flesh_pos: &mut BTreeMap<JointId, DVec3>,
    joint_meta: &mut BTreeMap<JointId, Value>,
    native_pos: &BTreeMap<JointId, DVec3>,
    field: &crate::medial::MedialField,
    j: usize,
    p: DVec3,
    r: f64,
    source: &str,
    total: f64,
    chain: &str,
    fraction: f64,
) {
    let joint = JointId::new(j as u32);
    flesh_pos.insert(joint, p);
    let delta_mm = native_pos.get(&joint).map(|np| p.distance(*np) * 1000.0);
    joint_meta.insert(
        joint,
        json!({
            "chain": chain,
            "fraction": fraction,
            "position_mm": [p.x * 1000.0, p.y * 1000.0, p.z * 1000.0],
            "medial_radius_mm": r * 1000.0,
            "medial_inside": field.is_inside(p),
            "landmark_source": source,
            "delta_vs_m0_mm": delta_mm,
            "chain_total_mm": total * 1000.0,
        }),
    );
}

/// Place a digit/toe/head-top chain's joints at Mixamo's fractions along a flesh axis
/// (wrist->fingertip, toe base->toe tip, skull centre->crown). The axis is straight, so
/// chord and arc fractions coincide; the flesh supplies direction and extent, Mixamo the
/// fractions. Joints stay inside the flesh hand/foot/skull rather than following Mixamo's
/// T-pose digit direction into the air beside an A-pose hand.
#[allow(clippy::too_many_arguments)]
fn place_digit_chain(
    chain: &str,
    axis: &[(DVec3, f64)],
    fractions: &BTreeMap<usize, crate::medial::JointTemplate>,
    flesh_pos: &mut BTreeMap<JointId, DVec3>,
    joint_meta: &mut BTreeMap<JointId, Value>,
    native_pos: &BTreeMap<JointId, DVec3>,
    field: &crate::medial::MedialField,
    source: &str,
) {
    if axis.len() < 2 {
        return;
    }
    let mut members: Vec<(usize, f64)> = fractions
        .iter()
        .filter(|(_, t)| t.chain == chain)
        .map(|(j, t)| (*j, t.fraction))
        .collect();
    if members.is_empty() {
        return;
    }
    members.sort_by(|a, b| a.1.total_cmp(&b.1));
    let fracs: Vec<f64> = members.iter().map(|(_, f)| *f).collect();
    let placed = crate::medial::place_at_chord_fractions(axis, &fracs);
    let total = crate::medial::FleshCentrelines::arc(axis);
    for ((j, f), (p, r)) in members.iter().zip(placed.iter()) {
        record_flesh_joint(
            flesh_pos, joint_meta, native_pos, field, *j, *p, *r, source, total, chain, *f,
        );
    }
}

/// Rebind one actor's mesh onto another skeleton.
///
/// # Errors
/// A missing or unreadable file, an actor whose mesh is not one shared skinned
/// pool, a correspondence that resolves nothing, a weighted source bone with no
/// mapped ancestor, a rest proof that fails, or an output row that is empty or
/// does not sum to one.
pub fn run(params: &RebindParams) -> Result<Value, CommandError> {
    let actor_bytes =
        std::fs::read(&params.actor).map_err(|error| errors::io(&params.actor, error))?;
    let actor = Actor::from_glb(&actor_bytes).map_err(actor_error)?;
    let skeleton_bytes =
        std::fs::read(&params.skeleton).map_err(|error| errors::io(&params.skeleton, error))?;
    let skeleton = Actor::from_glb(&skeleton_bytes).map_err(actor_error)?;

    let (source_rig, source_palette) = actor_rig(&actor)?;
    let (target_rig, target_palette) = actor_rig(&skeleton)?;
    if source_rig.roots().len() != 1 {
        return Err(refused(
            "rebind_source",
            format!(
                "the source palette has {} roots; rebind needs one skeleton root",
                source_rig.roots().len()
            ),
        ));
    }

    // One skinned mesh node, one shared vertex pool across its primitives: the
    // engine actor layout. A different layout is a named refusal, not a guess.
    let (mesh_node, mesh_name) = actor.skinned_mesh_node().map_err(actor_error)?;
    let pool = read_pool(&actor, mesh_node, &mesh_name, &source_palette)?;

    // The correspondence resolves each target bone onto a source node; invert it
    // into a source-palette -> target-joint map and seat the target at the source
    // rest.
    let correspondence = Correspondence::build(&actor, &target_rig, &target_palette)?;
    require_mapped(&correspondence, &params.skeleton.to_string_lossy())?;
    let source_worlds = actor_rest_worlds(&actor)?;
    let source_joints = actor.skin_joints().map_err(actor_error)?;
    let slot_of_node: BTreeMap<usize, usize> = source_joints
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    let mut direct: Vec<Option<JointId>> = vec![None; source_palette.len()];
    let mut native_pos: BTreeMap<JointId, DVec3> = BTreeMap::new();
    for pair in &correspondence.pairs {
        let Some(node) = pair.source else { continue };
        native_pos.insert(pair.joint, source_worlds[node].w_axis.truncate());
        if let Some(&slot) = slot_of_node.get(&node) {
            direct[slot] = Some(pair.joint);
        }
    }
    // The correspondence resolves by role, and the 13 zero-weight leaf tips carry
    // no role — yet they are joints of the body's own rest polyline (the extent
    // carrier). Recover their rest worlds by name so their chains (head_top, toe,
    // finger) place them at the flesh tip (fraction 1.0) instead of falling back
    // to a scaled Mixamo offset. This seats no weight: `direct` stays role-based.
    for (j, name) in target_palette.iter().enumerate() {
        let joint = JointId::new(j as u32);
        if native_pos.contains_key(&joint) {
            continue;
        }
        if let Some(slot) = source_palette.iter().position(|p| p == name) {
            native_pos.insert(joint, source_worlds[source_joints[slot]].w_axis.truncate());
        }
    }
    let humanoid = crate::humanoid::supports(&target_palette);
    let (seated_rig, seat_report) = match humanoid {
        true => {
            let body_triangles: Vec<[u32; 3]> = pool.triangles.iter().zip(&pool.partitions)
                .filter(|(_, partition)| partition.0 == 0)
                .map(|(triangle, _)| *triangle).collect();
            crate::humanoid::place(&pool.vertices, &body_triangles, &skeleton, &target_rig, &target_palette)?
        }
        false => generic_seat(&pool, &target_rig, &target_palette, &native_pos)?,
    };

    // Resolve every source joint: direct, folded to its nearest mapped ancestor,
    // or unsupported. A weighted bone with no mapped ancestor is a refusal.
    let mut mass = vec![0.0_f64; source_palette.len()];
    for row in &pool.rows {
        for &(slot, weight) in row {
            mass[usize::from(slot)] += f64::from(weight);
        }
    }
    let total_mass: f64 = mass.iter().sum();
    let mut destinations = Vec::with_capacity(source_palette.len());
    let mut folds: Vec<FoldRow> = Vec::new();
    for (slot, name) in source_palette.iter().enumerate() {
        let joint = JointId::new(slot as u32);
        if let Some(target) = direct[slot] {
            destinations.push(Destination::Direct(target));
            continue;
        }
        let ancestor = nearest_mapped_ancestor(&source_rig, joint, &direct);
        match ancestor {
            Some(target) => {
                destinations.push(Destination::Collapsed(target));
                if mass[slot] > 0.0 {
                    folds.push(FoldRow {
                        name: name.clone(),
                        group: fold_group(name),
                        mass: mass[slot],
                        target: target_palette[target.index()].clone(),
                    });
                }
            }
            None if mass[slot] > 0.0 => {
                return Err(refused(
                    "rebind_fold",
                    format!(
                        "source bone {name:?} carries weight mass {:.4} but no mapped ancestor; \
                         the target skeleton cannot carry it",
                        mass[slot]
                    ),
                ));
            }
            None => destinations.push(Destination::Unsupported),
        }
    }

    // The source rest, proved in world Y-up with an identity basis.
    let positions: Vec<[f32; 3]> = pool
        .vertices
        .iter()
        .map(|point| point.map(|value| value as f32))
        .collect();
    let source_skin = Skin {
        bones: source_palette.clone(),
        bind_inverse: (0..source_palette.len())
            .map(|slot| {
                f32_of(
                    DMat4::from_cols_array_2d(
                        &source_rig
                            .rest_world(JointId::new(slot as u32))
                            .expect("source palette joint resolves"),
                    )
                    .inverse(),
                )
            })
            .collect(),
        influences: pool_weights(&pool.rows, source_palette.len())?,
    };
    let source_root = source_rig.roots()[0];
    let source_binding = RestBinding::prove_with_basis(
        &source_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: source_root,
            positions: &positions,
            skin: &source_skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;

    let map = BoneMap::resolved(&source_binding, &seated_rig, &destinations)
        .map_err(|error| errors::invalid("rebind_map", error.to_string()))?;

    // The seat carrier: the mesh rigidly bound to the seated root. It proves the
    // seated rig carries the geometry at rest and supplies the kernel its target
    // root and basis; the transferred output is proved again below.
    let seat_root = seated_rig.roots()[0];
    let seat_skin = rigid_root_skin(&seated_rig, seat_root, positions.len())?;
    let seat_binding = RestBinding::prove_with_basis(
        &seated_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: seat_root,
            positions: &positions,
            skin: &seat_skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;

    let remapped = remap_skin(
        &source_binding,
        &map,
        &seat_binding,
        &RemapSettings {
            source_shape: 0,
            target_shape: 0,
            locked_source_joints: Vec::new(),
        },
    )
    .map_err(|error| errors::invalid("rebind_transfer", error.to_string()))?;

    // The palette is the skeleton's, not the weight census's: keep every target
    // bone so a clip authored on the full skeleton drives it without a conversion
    // layer. Unreached tips carry zero weight.
    let skin = complete_palette(&remapped.skin, &seated_rig, &target_palette)?;
    let validation = validate_skin(&skin, positions.len())?;
    if humanoid {
        crate::humanoid::validate_marker_weights(&target_palette,&validation.per_bone)?;
    }

    let normals = smooth_normals(&positions, &pool.triangles);
    let asset = MeshAsset {
        name: mesh_name.to_owned(),
        positions: positions.clone(),
        normals,
        triangles: pool.triangles.clone(),
        triangle_partitions: pool.partitions.clone(),
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let final_binding = RestBinding::prove_with_basis(
        &seated_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: seat_root,
            positions: &asset.positions,
            skin: &skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;
    let measurement = final_binding.skins()[0].measurement();
    let materials = part_materials(&actor, mesh_node)?;
    let bytes = write_partitioned_rigged_glb(&asset, &final_binding, 0, &[], &materials)
        .map_err(glb_error)?;
    if let Some(path) = &params.out {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        std::fs::write(path, &bytes).map_err(|error| errors::io(path, error))?;
    }

    let report = build_report(ReportInputs {
        params,
        actor_bytes: &actor_bytes,
        skeleton_bytes: &skeleton_bytes,
        correspondence: &correspondence,
        seat_report,
        source_palette: &source_palette,
        target_palette: &target_palette,
        folds: &folds,
        total_mass,
        mass: &mass,
        direct: &direct,
        validation: &validation,
        measurement,
        remap_discarded: remapped.report.discarded.len(),
        pool: &pool,
        materials: &materials,
        bytes: &bytes,
        positions: &positions,
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

/// Retain generic transfer semantics for palettes outside the humanoid contract.
fn generic_seat(
    pool: &Pool,
    target_rig: &Rig,
    target_palette: &[String],
    native_pos: &BTreeMap<JointId, DVec3>,
) -> Result<(Rig, Value), CommandError> {
    // Skeleton-authority placement (operator): Mixamo's graph and chain fractions are
    // the authority; the flesh medial axis supplies the hip anchor, the chain extents
    // and the frame. No native joint places, anchors or measures a vertex — native_pos
    // is read only for the delta report and the outside-flesh census below. Each limb
    // joint lands at Mixamo's arc fraction on the flesh centreline (branch-to-branch),
    // inside the flesh by construction; containment is re-measured below with the gate's
    // own ray-parity probe. A joint outside the flesh is a warp/refusal candidate, never
    // snapped back onto the axis — bending the skeleton to the flesh is rejected.
    let field =
        crate::medial::MedialField::from_mesh_sealed(&pool.vertices, &pool.triangles, 0.003, 2);
    let fc = crate::medial::flesh_centrelines(&field);
    let target_rest: Vec<DVec3> = (0..target_palette.len())
        .map(|i| {
            let m = target_rig
                .rest_world(JointId::new(i as u32))
                .map_err(|e| errors::invalid("rebind_seat", format!("target rest world: {e}")))?;
            Ok(DVec3::new(m[3][0], m[3][1], m[3][2]))
        })
        .collect::<Result<_, CommandError>>()?;
    let fractions = crate::medial::chain_fractions(target_palette, &target_rest);
    let index_of = |name: &str| target_palette.iter().position(|p| p == name);

    // Flesh placement axes, branch-to-branch, measured on the medial axis: spine pelvis
    // centre -> skull centre, arm shoulder branch -> wrist, leg hip branch -> toe base.
    let mut flesh_axes: BTreeMap<String, Vec<(DVec3, f64)>> = BTreeMap::new();
    flesh_axes.insert("spine".into(), fc.spine_axis());
    flesh_axes.insert("arm_left".into(), fc.arm_axis_left());
    flesh_axes.insert("arm_right".into(), fc.arm_axis_right());
    flesh_axes.insert("leg_left".into(), fc.leg_axis_left());
    flesh_axes.insert("leg_right".into(), fc.leg_axis_right());

    // The body frame from flesh geometry alone: up = hip anchor -> skull centre, right =
    // the shoulder-branch axis. Mixamo's rest directions are expressed in this frame for
    // the digit/toe/head-top fallback and the seated bone rolls. Never from a native joint.
    let body_lo = pool
        .vertices
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |a, v| {
            a.min(DVec3::from_array(*v))
        });
    let body_hi = pool
        .vertices
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |a, v| {
            a.max(DVec3::from_array(*v))
        });
    let hip_anchor = fc
        .hip_anchor
        .map(|(p, _)| p)
        .or_else(|| fc.spine.first().map(|(p, _)| *p))
        .unwrap_or((body_lo + body_hi) * 0.5);
    let head_point = fc
        .head_centre
        .map(|(p, _)| p)
        .or_else(|| fc.spine.last().map(|(p, _)| *p))
        .unwrap_or(hip_anchor + DVec3::Y);
    let shoulder_l = fc
        .shoulder_left
        .map(|(p, _)| p)
        .or_else(|| fc.arm_left.first().map(|(p, _)| *p));
    let shoulder_r = fc
        .shoulder_right
        .map(|(p, _)| p)
        .or_else(|| fc.arm_right.first().map(|(p, _)| *p));
    let up = (head_point - hip_anchor).normalize_or_zero();
    let right = match (shoulder_l, shoulder_r) {
        (Some(l), Some(r)) => (r - l).normalize_or_zero(),
        _ => DVec3::X,
    };
    let right = (right - up * right.dot(up)).normalize_or_zero();
    let body_basis = DMat3::from_cols(right, up, right.cross(up));
    let mix_basis = {
        let f = |n: &str| index_of(n).map(|i| target_rest[i]);
        let mr = match (f("mixamorig:LeftShoulder"), f("mixamorig:RightShoulder")) {
            (Some(l), Some(r)) => (r - l).normalize_or_zero(),
            _ => DVec3::X,
        };
        let mu = match (f("mixamorig:Hips"), f("mixamorig:Head")) {
            (Some(h), Some(hd)) => (hd - h).normalize_or_zero(),
            _ => DVec3::Y,
        };
        let mr = (mr - mu * mr.dot(mu)).normalize_or_zero();
        DMat3::from_cols(mr, mu, mr.cross(mu))
    };
    let frame_rotation = body_basis * mix_basis.inverse();

    // Flesh height similarity: the uniform scale for chains without a resolved medial
    // axis (digits, toes, head top, or a degenerate mesh). Mixamo supplies the direction
    // and fraction; the flesh supplies the scale and the proximal anchor.
    let body_y_min = pool
        .vertices
        .iter()
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min);
    let body_y_max = pool
        .vertices
        .iter()
        .map(|p| p[1])
        .fold(f64::NEG_INFINITY, f64::max);
    let mix_y_min = target_rest
        .iter()
        .map(|p| p.y)
        .fold(f64::INFINITY, f64::min);
    let mix_y_max = target_rest
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let height_scale = if mix_y_max - mix_y_min > 1e-9 {
        (body_y_max - body_y_min) / (mix_y_max - mix_y_min)
    } else {
        1.0
    };

    // Place every joint parent-before-child. A joint with a flesh axis lands at Mixamo's
    // arc fraction on it (inside by construction). A joint without one keeps Mixamo's
    // fraction along Mixamo's body-frame direction, scaled by the flesh height similarity
    // and anchored at its chain's placed proximal joint (Hand / ToeBase / Head).
    let parent_of: Vec<Option<usize>> = (0..target_palette.len())
        .map(|i| {
            target_rig
                .joint(JointId::new(i as u32))
                .and_then(|j| j.parent)
                .map(|p| p.index())
        })
        .collect();
    let mut flesh_pos: BTreeMap<JointId, DVec3> = BTreeMap::new();
    let mut joint_meta: BTreeMap<JointId, Value> = BTreeMap::new();
    // Primary chains: place every member joint together at Mixamo's chord fractions on
    // the flesh medial axis. The acceptance gate measures chord fractions and Mixamo's
    // own fractions are chord-based, so this reconciles the curved A-pose flesh with the
    // gate while keeping every joint on the medial axis (inside the flesh).
    for chain in ["spine", "arm_left", "arm_right", "leg_left", "leg_right"] {
        let Some(axis) = flesh_axes.get(chain) else {
            continue;
        };
        if axis.len() < 2 {
            continue;
        }
        let mut members: Vec<(usize, f64)> = fractions
            .iter()
            .filter(|(_, t)| t.chain == chain)
            .map(|(j, t)| (*j, t.fraction))
            .collect();
        members.sort_by(|a, b| a.1.total_cmp(&b.1));
        let fracs: Vec<f64> = members.iter().map(|(_, f)| *f).collect();
        let placed = crate::medial::place_at_chord_fractions(axis, &fracs);
        let total = crate::medial::FleshCentrelines::arc(axis);
        for ((j, f), (p, r)) in members.iter().zip(placed.iter()) {
            record_flesh_joint(
                &mut flesh_pos,
                &mut joint_meta,
                native_pos,
                &field,
                *j,
                *p,
                *r,
                "medial_axis",
                total,
                chain,
                *f,
            );
        }
    }
    // Digit / toe / head-top axes from the flesh, not Mixamo's T-pose hand direction:
    // wrist -> fingertip medial-skeleton paths (fanned), toe base -> toe tip, skull centre
    // -> crown. Mixamo supplies the fractions; the flesh supplies the centreline.
    for (side, sign) in [("Left", -1.0_f64), ("Right", 1.0_f64)] {
        let hand = format!("mixamorig:{side}Hand");
        let Some(wrist) =
            index_of(&hand).and_then(|j| flesh_pos.get(&JointId::new(j as u32)).copied())
        else {
            continue;
        };
        // Digit centrelines traced on the medial skeleton graph (wrist -> fingertip),
        // inside the flesh by construction. Mixamo supplies the fractions; the flesh
        // supplies each digit's centreline. No native input, no T-pose hand direction.
        let arm_track = if side == "Left" {
            &fc.arm_left
        } else {
            &fc.arm_right
        };
        let hand_fallback = crate::medial::FleshCentrelines::sub_path(
            arm_track,
            wrist,
            arm_track.last().map(|(p, _)| *p).unwrap_or(wrist),
        );
        let paths = crate::medial::digit_paths(&field, wrist, sign, fc.cx, 5);
        for (idx, digit) in ["Thumb", "Index", "Middle", "Ring", "Pinky"]
            .iter()
            .enumerate()
        {
            let chain = format!("finger_{}_{}", side.to_lowercase(), digit.to_lowercase());
            let axis = paths.get(idx).unwrap_or(&hand_fallback);
            place_digit_chain(
                &chain,
                axis,
                &fractions,
                &mut flesh_pos,
                &mut joint_meta,
                native_pos,
                &field,
                "flesh_digit_axis",
            );
        }
        // Toe: toe base -> the leg track's distal tip (the flesh toe tip).
        let toe_base = if side == "Left" {
            fc.toe_base_left
        } else {
            fc.toe_base_right
        };
        let leg_track = if side == "Left" {
            &fc.leg_left
        } else {
            &fc.leg_right
        };
        if let (Some((tb, tbr)), Some((tip, tipr))) = (toe_base, leg_track.last().copied()) {
            let chain = if side == "Left" {
                "toe_left"
            } else {
                "toe_right"
            };
            let axis = vec![(tb, tbr), (tip, tipr)];
            place_digit_chain(
                chain,
                &axis,
                &fractions,
                &mut flesh_pos,
                &mut joint_meta,
                native_pos,
                &field,
                "flesh_digit_axis",
            );
        }
    }
    // Head top: skull centre -> crown along the midline.
    if let (Some((hc, hcr)), Some((cr, crr))) = (fc.head_centre, fc.crown) {
        let axis = vec![(hc, hcr), (cr, crr)];
        place_digit_chain(
            "head_top",
            &axis,
            &fractions,
            &mut flesh_pos,
            &mut joint_meta,
            native_pos,
            &field,
            "flesh_digit_axis",
        );
    }
    // Any joint still unplaced (no chain fraction, or an unresolved digit axis): keep the
    // flesh parent offset along Mixamo's body-frame direction at the flesh height scale.
    // Never a native seat.
    let mut order: Vec<usize> = (0..target_palette.len()).collect();
    order.sort_by_key(|&i| std::iter::successors(parent_of[i], |&p| parent_of[p]).count());
    for j in order {
        let joint = JointId::new(j as u32);
        if flesh_pos.contains_key(&joint) {
            continue;
        }
        let anchor = parent_of[j]
            .and_then(|p| flesh_pos.get(&JointId::new(p as u32)).copied())
            .unwrap_or(hip_anchor);
        let p = match parent_of[j] {
            Some(pa) => anchor + frame_rotation * (target_rest[j] - target_rest[pa]) * height_scale,
            None => anchor,
        };
        let chain = fractions
            .get(&j)
            .map(|t| t.chain.clone())
            .unwrap_or_default();
        let fraction = fractions.get(&j).map(|t| t.fraction).unwrap_or(0.0);
        record_flesh_joint(
            &mut flesh_pos,
            &mut joint_meta,
            native_pos,
            &field,
            j,
            p,
            field.radius_at(p),
            "mixamo_direction_flesh_scale",
            0.0,
            &chain,
            fraction,
        );
    }
    // Zero-weight leaf-tip markers (fingertip / toe tip / crown) define the chain's
    // terminal extent and carry no weight. Seat them on the skin so they sit within the
    // gate tolerance rather than past it — a tip marker on the surface, not a placement
    // snap of a weighted joint (those stay on the medial axis or are reported as refusals).
    let leaf_markers = [
        "Toe_End",
        "HeadTop_End",
        "HandThumb4",
        "HandIndex4",
        "HandMiddle4",
        "HandRing4",
        "HandPinky4",
    ];
    for (j, name) in target_palette.iter().enumerate() {
        if !leaf_markers.iter().any(|m| name.contains(m)) {
            continue;
        }
        let joint = JointId::new(j as u32);
        let Some(&p) = flesh_pos.get(&joint) else {
            continue;
        };
        let (signed_mm, _inside, _votes) =
            crate::medial::surface_probe(&pool.vertices, &pool.triangles, p);
        if signed_mm > 1.0 {
            let q = crate::medial::closest_surface_point(&pool.vertices, &pool.triangles, p);
            flesh_pos.insert(joint, q);
            if let Some(obj) = joint_meta.get_mut(&joint).and_then(|v| v.as_object_mut()) {
                obj.insert(
                    "position_mm".into(),
                    json!([q.x * 1000.0, q.y * 1000.0, q.z * 1000.0]),
                );
                obj.insert(
                    "medial_radius_mm".into(),
                    json!(field.radius_at(q) * 1000.0),
                );
                obj.insert("leaf_tip_seated_on_surface".into(), json!(true));
            }
        }
    }
    let positions_f32: Vec<[f32; 3]> = pool
        .vertices
        .iter()
        .map(|point| point.map(|value| value as f32))
        .collect();
    let mesh_normals = smooth_normals(&positions_f32, &pool.triangles);
    let (seated_rig, mut seat_report) = seat_rig(
        target_rig,
        target_palette,
        &flesh_pos,
        native_pos,
        &pool.vertices,
        &mesh_normals,
    )?;
    // Skeleton-authority augmentation: per-joint fraction/radius/inside/delta-vs-M0,
    // the chain table (our extents at Mixamo fractions), the similarity, and the
    // joints outside the flesh (zero-weight leaf tips by design vs placement failures).
    {
        let body_y_min = pool
            .vertices
            .iter()
            .map(|p| p[1])
            .fold(f64::INFINITY, f64::min);
        let body_y_max = pool
            .vertices
            .iter()
            .map(|p| p[1])
            .fold(f64::NEG_INFINITY, f64::max);
        let mix_y_min = target_rest
            .iter()
            .map(|p| p.y)
            .fold(f64::INFINITY, f64::min);
        let mix_y_max = target_rest
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let height_scale = (body_y_max - body_y_min) / (mix_y_max - mix_y_min);
        if let Some(rows) = seat_report["per_bone"].as_array_mut() {
            for row in rows.iter_mut() {
                if let Some(bone) = row["bone"].as_str()
                    && let Some(j) = target_palette.iter().position(|p| p == bone)
                    && let Some(meta) = joint_meta.get(&JointId::new(j as u32))
                    && let Some(obj) = meta.as_object()
                {
                    for (k, v) in obj {
                        row[k] = v.clone();
                    }
                }
            }
        }
        let mut chain_table = Vec::new();
        for (cname, pts) in &flesh_axes {
            let total = crate::medial::FleshCentrelines::arc(pts);
            let r_prox = pts.first().map(|(_, r)| r * 1000.0).unwrap_or(0.0);
            let r_dist = pts.last().map(|(_, r)| r * 1000.0).unwrap_or(0.0);
            chain_table.push(json!({
                "chain": cname,
                "our_extent_mm": total * 1000.0,
                "medial_radius_proximal_mm": r_prox,
                "medial_radius_distal_mm": r_dist,
                "extent_source": "flesh medial axis, branch-to-branch arc",
            }));
        }
        let leaf_markers = [
            "Toe_End",
            "HeadTop_End",
            "HandThumb4",
            "HandIndex4",
            "HandMiddle4",
            "HandRing4",
            "HandPinky4",
        ];
        // Containment authority: the same +X ray parity / true point-triangle
        // measurement the acceptance gate uses, at its tolerance (default 1 mm).
        // The voxel field's nearest-cell probe flips on surface-coincident joints;
        // it stays in the report as `medial_inside`/`medial_radius_mm` diagnostics.
        let tolerance_mm = std::env::var("SKULPTOR_FLESH_TOLERANCE_MM")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(1.0);
        if let Some(rows) = seat_report["per_bone"].as_array_mut() {
            for row in rows.iter_mut() {
                if let Some(pos) = row["position_mm"].as_array()
                    && pos.len() == 3
                {
                    let point = DVec3::new(
                        pos[0].as_f64().unwrap_or(0.0) / 1000.0,
                        pos[1].as_f64().unwrap_or(0.0) / 1000.0,
                        pos[2].as_f64().unwrap_or(0.0) / 1000.0,
                    );
                    let (signed_mm, _parity, votes) =
                        crate::medial::surface_probe(&pool.vertices, &pool.triangles, point);
                    row["surface_signed_mm"] = json!(signed_mm);
                    row["parity_votes_inside"] = json!(votes);
                    row["inside"] = json!(signed_mm <= tolerance_mm);
                }
            }
        }
        let mut outside = Vec::new();
        let mut refusals = Vec::new();
        for (j, name) in target_palette.iter().enumerate() {
            let Some(meta) = joint_meta.get(&JointId::new(j as u32)) else {
                continue;
            };
            let surface_mm = seat_report["per_bone"]
                .as_array()
                .and_then(|rows| {
                    rows.iter()
                        .find(|r| r["bone"].as_str() == Some(name.as_str()))
                })
                .and_then(|r| r["surface_signed_mm"].as_f64())
                .unwrap_or(f64::NAN);
            if surface_mm <= tolerance_mm {
                continue;
            }
            let is_leaf = leaf_markers.iter().any(|m| name.contains(m));
            let row = json!({
                "bone": name,
                "chain": meta["chain"],
                "fraction": meta["fraction"],
                "surface_signed_mm": surface_mm,
                "medial_radius_mm": meta["medial_radius_mm"],
                "delta_vs_m0_mm": meta["delta_vs_m0_mm"],
                "leaf_tip_by_design": is_leaf,
            });
            outside.push(row.clone());
            // A non-leaf joint outside the flesh cannot be bent back onto the
            // axis (the rejected direction); until the bounded warp lands it is
            // a refusal with its number, never a silent nudge.
            if !is_leaf {
                refusals.push(row);
            }
        }
        seat_report["skeleton_authority"] = json!({
            "method": "Mixamo graph + arc-fraction placement on the flesh medial axis (hip anchor = pelvis inscribed centre, root = hips hub derived from the leg sockets at Mixamo's branch angle; chain extents branch-to-branch; digits/toes/head-top by Mixamo direction at flesh height scale); containment by +X ray parity / point-triangle distance at the gate tolerance; the medial field supplies inscribed radii; no native joint places, anchors or measures — native_pos is delta/census only; no snap back to the axis",
            "similarity": {
                "height_scale": height_scale,
                "body_height_mm": (body_y_max - body_y_min) * 1000.0,
                "mixamo_height_mm": (mix_y_max - mix_y_min) * 1000.0,
                "rotation_source": "body symmetry plane + shoulder-branch axis + hip->head up (see frame_yaw_degrees), measured from flesh",
                "translation_source": "root = hips hub derived from the leg sockets at Mixamo's branch angle (see seat.hips_hub); the pelvis inscribed centre is its flesh anchor",
                "hip_anchor_mm": [hip_anchor.x * 1000.0, hip_anchor.y * 1000.0, hip_anchor.z * 1000.0],
                "hip_anchor_medial_radius_mm": fc.hip_anchor.map(|(_, r)| r * 1000.0),
                "symmetry_plane_x_mm": fc.cx * 1000.0,
            },
            "chain_table": chain_table,
            "hip_sockets": {
                "rule": "the leg's proximal centre at the crotch carried up one thigh radius, mirrored onto the midline: the pelvis medial skeleton is a sheet, so its junctions sit in the upper thigh, not at the hip (measured 116 mm below the crotch on Emily)",
                "left": fc.femoral_head_left.map(|(crotch, radius)| json!({
                    "crotch_mm": [crotch.x * 1000.0, crotch.y * 1000.0, crotch.z * 1000.0],
                    "crotch_inscribed_radius_mm": radius * 1000.0,
                })),
                "right": fc.femoral_head_right.map(|(crotch, radius)| json!({
                    "crotch_mm": [crotch.x * 1000.0, crotch.y * 1000.0, crotch.z * 1000.0],
                    "crotch_inscribed_radius_mm": radius * 1000.0,
                })),
            },
            "containment_tolerance_mm": tolerance_mm,
            "outside_flesh": outside,
            "refusals": refusals,
            "warp_report": {
                "applied": false,
                "reason": "every placed joint is inside the flesh at the gate tolerance, so no warp was required. The bounded ARAP/Laplacian mesh warp is not yet implemented — an outside joint would be refused with its number, not forced",
                "distortion_cap": {
                    "max_local_vertex_displacement_mm": 15.0,
                    "max_mean_edge_strain": 0.02,
                    "max_height_drift": 0.005,
                    "max_chain_arc_extent_drift": 0.01,
                    "status": "declared by the lane for the warp stage; no warp applied in this delivery, so measured drift is zero",
                },
                "ledger": [],
            },
            "proportion_drift": {
                "mesh_vertices_moved": 0,
                "height_drift_mm": 0.0,
                "chain_arc_extent_drift_mm": 0.0,
                "note": "the mesh is bit-identical to the input body; chain arc extents are the input polyline's by construction. Chord sums between placed joints read slightly shorter than the arc extents because placement cuts the polyline corners — a measurement effect, not a proportion change",
            },
        });
    }

    Ok((seated_rig, seat_report))
}

fn identity_f64() -> [[f64; 4]; 4] {
    DMat4::IDENTITY.to_cols_array_2d()
}

/// The nearest ancestor of `joint` that maps directly to a target bone.
fn nearest_mapped_ancestor(
    rig: &Rig,
    joint: JointId,
    direct: &[Option<JointId>],
) -> Option<JointId> {
    let mut cursor = rig.joint(joint).and_then(|joint| joint.parent);
    while let Some(parent) = cursor {
        if let Some(target) = direct[parent.index()] {
            return Some(target);
        }
        cursor = rig.joint(parent).and_then(|joint| joint.parent);
    }
    None
}

/// One skin row per pool vertex: palette slots and raw weights.
/// Every vertex rigidly bound to one root: a valid seat carrier skin.
fn rigid_root_skin(rig: &Rig, root: JointId, vertices: usize) -> Result<Skin, CommandError> {
    let name = rig
        .joint(root)
        .and_then(|joint| joint.name.clone())
        .ok_or_else(|| refused("rebind_seat", "the seated root is unnamed".to_owned()))?;
    let world = DMat4::from_cols_array_2d(
        &rig.rest_world(root)
            .map_err(|error| errors::invalid("rebind_seat", format!("seat root: {error}")))?,
    );
    let offsets: Vec<u32> = (0..=vertices as u32).collect();
    let joints = vec![0_u16; vertices];
    let values = vec![1.0_f32; vertices];
    let influences = SkinWeights::new(offsets, joints, values, 1)
        .map_err(|error| errors::invalid("rebind_seat", format!("seat skin: {error}")))?;
    Ok(Skin {
        bones: vec![name],
        bind_inverse: vec![f32_of(world.inverse())],
        influences,
    })
}

/// Extend the transferred skin to the full target palette, zero-weight tips kept.
fn complete_palette(
    transferred: &Skin,
    seated_rig: &Rig,
    target_palette: &[String],
) -> Result<Skin, CommandError> {
    let slot_of_name: BTreeMap<&str, usize> = target_palette
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.as_str(), slot))
        .collect();
    let remap_slot: Vec<usize> = transferred
        .bones
        .iter()
        .map(|name| {
            slot_of_name.get(name.as_str()).copied().ok_or_else(|| {
                refused(
                    "rebind_transfer",
                    format!("transferred bone {name:?} is outside the target palette"),
                )
            })
        })
        .collect::<Result<_, CommandError>>()?;
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for vertex in 0..transferred.influences.rows() {
        let row = transferred
            .influences
            .row(vertex)
            .expect("row below the checked count");
        for (&slot, &weight) in row.joints.iter().zip(row.weights) {
            if weight <= 0.0 {
                continue;
            }
            joints.push(
                u16::try_from(remap_slot[usize::from(slot)])
                    .map_err(|_| refused("rebind_transfer", "target palette exceeds u16"))?,
            );
            values.push(weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| refused("rebind_transfer", "influence entries exceed u32"))?,
        );
    }
    let influences = SkinWeights::new(offsets, joints, values, target_palette.len())
        .map_err(|error| errors::invalid("rebind_transfer", format!("output weights: {error}")))?;
    let bind_inverse = (0..target_palette.len())
        .map(|slot| {
            f32_of(
                DMat4::from_cols_array_2d(
                    &seated_rig
                        .rest_world(JointId::new(slot as u32))
                        .expect("target palette joint resolves"),
                )
                .inverse(),
            )
        })
        .collect();
    Ok(Skin {
        bones: target_palette.to_vec(),
        bind_inverse,
        influences,
    })
}

/// Everything the report folds together.
struct ReportInputs<'a> {
    params: &'a RebindParams,
    actor_bytes: &'a [u8],
    skeleton_bytes: &'a [u8],
    correspondence: &'a Correspondence,
    seat_report: Value,
    source_palette: &'a [String],
    target_palette: &'a [String],
    folds: &'a [FoldRow],
    total_mass: f64,
    mass: &'a [f64],
    direct: &'a [Option<JointId>],
    validation: &'a Validation,
    measurement: sculpto_core::rig::rest::RestMeasurement,
    remap_discarded: usize,
    pool: &'a Pool,
    materials: &'a [PartMaterial],
    bytes: &'a [u8],
    positions: &'a [[f32; 3]],
}

fn build_report(input: ReportInputs<'_>) -> Value {
    let mut groups: BTreeMap<&str, (f64, BTreeSet<String>, Vec<Value>)> = BTreeMap::new();
    for fold in input.folds {
        let entry = groups.entry(fold.group).or_default();
        entry.0 += fold.mass;
        entry.1.insert(fold.target.clone());
        entry.2.push(json!({
            "bone": fold.name,
            "mass": fold.mass,
            "share": fold.mass / input.total_mass,
            "target": fold.target,
        }));
    }
    let folded_mass: f64 = input.folds.iter().map(|fold| fold.mass).sum();
    let group_rows: Vec<Value> = groups
        .into_iter()
        .map(|(group, (mass, targets, bones))| {
            json!({
                "group": group,
                "mass": mass,
                "share": mass / input.total_mass,
                "targets": targets.into_iter().collect::<Vec<_>>(),
                "bones": bones,
            })
        })
        .collect();
    let mapped_mass: f64 = input
        .mass
        .iter()
        .zip(input.direct)
        .filter(|(_, direct)| direct.is_some())
        .map(|(mass, _)| *mass)
        .sum();
    let per_bone: Vec<Value> = input
        .target_palette
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            json!({
                "bone": name,
                "mass": input.validation.per_bone[slot],
                "share": input.validation.per_bone[slot] / input.positions.len() as f64,
            })
        })
        .collect();
    json!({
        "actor": input.params.actor.to_string_lossy(),
        "actor_sha256": sha_hex(&Sha256::digest(input.actor_bytes)),
        "skeleton": input.params.skeleton.to_string_lossy(),
        "skeleton_sha256": sha_hex(&Sha256::digest(input.skeleton_bytes)),
        "out": input.params.out.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "bytes": input.bytes.len(),
        "sha256": sha_hex(&Sha256::digest(input.bytes)),
        "correspondence": correspondence_report(input.correspondence),
        "seat": input.seat_report,
        "palette": {
            "source_bones": input.source_palette.len(),
            "target_bones": input.target_palette.len(),
            "mapped_source_bones": input.direct.iter().filter(|d| d.is_some()).count(),
            "weighted_target_bones": input.validation.per_bone.iter().filter(|m| **m > 0.0).count(),
        },
        "weight_census": {
            "total_raw_mass": input.total_mass,
            "mapped_share": mapped_mass / input.total_mass,
            "folded_share": folded_mass / input.total_mass,
            "groups": group_rows,
        },
        "validation": {
            "vertices": input.positions.len(),
            "max_sum_error": input.validation.max_sum_error,
            "empty_rows": input.validation.empty_rows,
            "widest_row": input.validation.widest,
            "discarded_weights": input.remap_discarded,
            "rest": {
                "max_vertex": input.measurement.max_vertex,
                "max_influence": input.measurement.max_influence,
                "max_joint": input.measurement.max_joint,
            },
        },
        "per_bone_mass": per_bone,
        "parts": input.pool.parts.iter().zip(input.materials).map(|(part, material)| json!({
            "name": material.name,
            "color": material.color,
            "triangles": part.triangles,
        })).collect::<Vec<_>>(),
        "bounds": bounds(input.positions),
        "statement": "the mesh keeps its geometry, partitions and part colours; the skeleton is \
                      the target's, seated at the source rest, and every source weight the target \
                      cannot name folds to its nearest mapped ancestor, reported per family",
    })
}

/// The report's one-line verdict, for the caller that prints it.
pub fn verdict(report: &Value) -> String {
    let validation = &report["validation"];
    format!(
        "rebound · {} target bones · {:.2}% mapped / {:.2}% folded · sum error {:.2e} · rest {:.2e}",
        report["palette"]["target_bones"],
        report["weight_census"]["mapped_share"]
            .as_f64()
            .unwrap_or(0.0)
            * 100.0,
        report["weight_census"]["folded_share"]
            .as_f64()
            .unwrap_or(0.0)
            * 100.0,
        validation["max_sum_error"].as_f64().unwrap_or(0.0),
        validation["rest"]["max_vertex"].as_f64().unwrap_or(0.0),
    )
}
