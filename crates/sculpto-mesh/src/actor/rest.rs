//! Bake a rest pose: rotate declared bone directions onto a target rest and
//! return the flesh that follows them as the new rest. @req DECL0013
//!
//! A rest pose is not decoration. Every clip is expressed relative to it, so a
//! skeleton whose arms rest 65° below the animation source's horizontal makes a
//! converter compensate a gap the pair should never have had. This owner applies
//! the aims to the authored locals, samples the flesh through the ordinary
//! linear-blend evaluator and hands back the posed worlds: written as a rest,
//! they turn the same pose into the file's basis with no clip and no offset left
//! to correct. Joint pivots are never re-placed — only each aimed bone's own
//! rotation changes, so pivots move exactly as far as the pose moves them.

use glam::{DMat4, DQuat, DVec3};

use super::{Actor, ActorError, Sample, Transform, animation, geometry, invalid};

/// One bone re-aimed by the bake.
///
/// `joint` is the bone, `child` the joint that ends it, so the measured
/// direction is `child − joint`; `direction` is the world direction the bone
/// must point at in the new rest.
#[derive(Clone, Copy, Debug)]
pub struct RestAim {
    /// Node whose bone direction changes.
    pub joint: usize,
    /// Joint that ends the bone.
    pub child: usize,
    /// Target world direction, in the actor's own frame.
    pub direction: [f64; 3],
    /// The world turn to apply instead of the minimal one, when the caller also
    /// controls the roll. It is stated against the bone's authored world
    /// rotation, so a chain's child turns do not accumulate the parents'.
    ///
    /// A direction alone leaves the bone free to spin about its own axis, and a
    /// minimal turn puts that roll wherever the geometry happens to land — for an
    /// arm raised from an A-pose, a paddle. A caller that has a reference basis
    /// passes the whole turn here and the bone's orientation is set exactly.
    pub turn: Option<[f64; 4]>,
    /// A local rotation delta to apply in the bone's own frame.
    ///
    /// The retarget-correct input for a clip authored against another skeleton's
    /// rest: `d = rest⁻¹ · pose` is frame-free, where a world turn would mix two
    /// bases. Applied as `new_local = authored_local · d`.
    pub local_turn: Option<[f64; 4]>,
}

/// One applied aim, measured before and after.
#[derive(Clone, Debug)]
pub struct AimedJoint {
    /// Bone that was turned.
    pub joint: usize,
    /// Joint that ends the bone.
    pub child: usize,
    /// Unit bone direction before the bake.
    pub direction_before: [f64; 3],
    /// Unit bone direction after the bake.
    pub direction_after: [f64; 3],
    /// Unit target direction.
    pub target: [f64; 3],
    /// Rotation the aim applied at this joint, degrees.
    pub turn_degrees: f64,
    /// Angle left between the baked direction and the target, degrees.
    pub residual_degrees: f64,
    /// Corrective turns the aim needed to reach the target.
    pub turns_applied: usize,
    /// The bone's pivot before the bake, world metres.
    pub pivot_before: [f64; 3],
    /// The bone's pivot after the bake, world metres.
    pub pivot_after: [f64; 3],
    /// Bone length before the bake, metres.
    pub length_before: f64,
    /// Bone length after the bake, metres.
    pub length_after: f64,
    /// The bone's world basis after the bake, as `[x, y, z]` world axes.
    pub axes_after: [[f64; 3]; 3],
}

/// The baked rest: the authored pose, the posed flesh, and the worlds that make
/// the posed flesh a rest.
pub struct RestBake {
    /// The selected parts at their authored rest.
    pub rest: Sample,
    /// The same parts after every aim was applied.
    pub posed: Sample,
    /// One row per aim, in application order.
    pub aims: Vec<AimedJoint>,
    /// Posed world transform per document node: the new rest.
    pub worlds: Vec<DMat4>,
}

/// Rotate every aim's bone onto its target direction and sample the result.
///
/// Aims are applied in the order given, so a chain is spelled root first: a
/// parent's turn carries its children, and the child's own aim then corrects
/// only the residual its parent left.
///
/// # Errors
/// An empty aim set, a node index outside the document, a static-matrix joint
/// with no local rotation to turn, a non-finite or antipodal target, a
/// degenerate bone, or any sampling refusal of the selected parts.
pub fn bake_rest(actor: &Actor, parts: &[&str], aims: &[RestAim]) -> Result<RestBake, ActorError> {
    if aims.is_empty() {
        return Err(invalid(
            "rest bake",
            "at least one aim is required; a bake with no aim would claim a rest change it did not make",
        ));
    }
    let nodes = geometry::select(&actor.document, parts)?;
    let mut locals = actor.transforms.clone();
    let mut worlds = animation::compose_worlds(actor, &locals)?;
    let authored = worlds.clone();
    let rest = geometry::sample(actor, &nodes, &worlds)?;
    let mut rows = Vec::with_capacity(aims.len());
    for aim in aims {
        if aim.joint >= worlds.len() || aim.child >= worlds.len() {
            return Err(invalid(
                "rest aim",
                format!(
                    "node index out of range: joint {} child {} of {} nodes",
                    aim.joint,
                    aim.child,
                    worlds.len()
                ),
            ));
        }
        let target = DVec3::from_array(aim.direction);
        if !target.is_finite() || target.length_squared() <= 0.0 {
            return Err(invalid(
                format!("rest aim on node {}", aim.joint),
                "the target direction must be finite and non-zero",
            ));
        }
        let target = target.normalize();
        let pivot_authored = authored[aim.joint].transform_point3(DVec3::ZERO);
        let child_authored = authored[aim.child].transform_point3(DVec3::ZERO);
        let offset_authored = child_authored - pivot_authored;
        let length_before = offset_authored.length();
        if length_before <= 0.0 {
            return Err(invalid(
                format!("rest aim on node {}", aim.joint),
                "the bone has no length, so it has no direction to aim",
            ));
        }
        let before = offset_authored / length_before;
        if before.dot(target) < -0.999_999 {
            return Err(invalid(
                format!("rest aim on node {}", aim.joint),
                "the target is antipodal to the bone's own direction, so the turn axis is undefined",
            ));
        }
        let Transform::Trs {
            translation, scale, ..
        } = actor.transforms[aim.joint]
        else {
            return Err(invalid(
                format!("rest aim on node {}", aim.joint),
                "joint is a static matrix; it has no local rotation to bake",
            ));
        };
        // One world turn is enough when the joint's world carries a uniform scale,
        // and a non-uniform one leaves a few degrees: iterate the residual so the
        // written direction reaches the target in either case.
        let mut residual = before.angle_between(target).to_degrees();
        let mut turns_applied = 0;
        if let Some(local_turn) = aim.local_turn {
            let shaft = DQuat::from_array(local_turn).normalize();
            let parent_world_rotation = match actor.parents[aim.joint] {
                Some(parent) => world_rotation(worlds[parent]),
                None => DQuat::IDENTITY,
            };
            let own_world = world_rotation(worlds[aim.joint]);
            locals[aim.joint] = Transform::Trs {
                translation,
                rotation: (parent_world_rotation.inverse() * (own_world * shaft)).normalize(),
                scale,
            };
            worlds = animation::compose_worlds(actor, &locals)?;
            turns_applied = 1;
            let pivot = worlds[aim.joint].transform_point3(DVec3::ZERO);
            let child = worlds[aim.child].transform_point3(DVec3::ZERO);
            let offset = child - pivot;
            residual = match offset.length() > 0.0 {
                true => (offset / offset.length())
                    .angle_between(target)
                    .to_degrees(),
                false => f64::INFINITY,
            };
        } else if let Some(turn) = aim.turn {
            let parent_world_rotation = match actor.parents[aim.joint] {
                Some(parent) => world_rotation(worlds[parent]),
                None => DQuat::IDENTITY,
            };
            let turn = DQuat::from_array(turn).normalize();
            // The turn is stated against the **authored** world rotation, not the
            // current one: a parent's turn has already carried this bone, and
            // composing on top of that would apply the child's turn twice.
            let local =
                parent_world_rotation.inverse() * (turn * world_rotation(authored[aim.joint]));
            locals[aim.joint] = Transform::Trs {
                translation,
                rotation: local.normalize(),
                scale,
            };
            worlds = animation::compose_worlds(actor, &locals)?;
            turns_applied = 1;
            let pivot = worlds[aim.joint].transform_point3(DVec3::ZERO);
            let child = worlds[aim.child].transform_point3(DVec3::ZERO);
            let offset = child - pivot;
            residual = match offset.length() > 0.0 {
                true => (offset / offset.length())
                    .angle_between(target)
                    .to_degrees(),
                false => f64::INFINITY,
            };
        }
        for _ in 0..MAX_AIM_ITERATIONS {
            if aim.turn.is_some() || aim.local_turn.is_some() || residual <= AIM_CONVERGENCE_DEG {
                break;
            }
            let pivot = worlds[aim.joint].transform_point3(DVec3::ZERO);
            let child = worlds[aim.child].transform_point3(DVec3::ZERO);
            let offset = child - pivot;
            let length = offset.length();
            if length <= 0.0 {
                return Err(invalid(
                    format!("rest aim on node {}", aim.joint),
                    "the bone has no length after the turn",
                ));
            }
            let turn = DQuat::from_rotation_arc(offset / length, target);
            // The world turn becomes a local rotation through the joint's own
            // world rotation, so the bone's new world rotation is `turn · rest`
            // and its local translation (the pivot offset inside the parent) is
            // untouched: a pivot is never re-placed, only carried by the pose.
            let parent_world_rotation = match actor.parents[aim.joint] {
                Some(parent) => world_rotation(worlds[parent]),
                None => DQuat::IDENTITY,
            };
            let joint_world_rotation = world_rotation(worlds[aim.joint]);
            let local = parent_world_rotation.inverse() * (turn * joint_world_rotation);
            locals[aim.joint] = Transform::Trs {
                translation,
                rotation: local.normalize(),
                scale,
            };
            worlds = animation::compose_worlds(actor, &locals)?;
            turns_applied += 1;
            let pivot = worlds[aim.joint].transform_point3(DVec3::ZERO);
            let child = worlds[aim.child].transform_point3(DVec3::ZERO);
            let offset = child - pivot;
            residual = match offset.length() > 0.0 {
                true => (offset / offset.length())
                    .angle_between(target)
                    .to_degrees(),
                false => f64::INFINITY,
            };
        }
        let pivot_after = worlds[aim.joint].transform_point3(DVec3::ZERO);
        let child_after = worlds[aim.child].transform_point3(DVec3::ZERO);
        let after_offset = child_after - pivot_after;
        let length_after = after_offset.length();
        let after = match length_after > 0.0 {
            true => after_offset / length_after,
            false => DVec3::ZERO,
        };
        rows.push(AimedJoint {
            joint: aim.joint,
            child: aim.child,
            direction_before: before.to_array(),
            direction_after: after.to_array(),
            target: target.to_array(),
            turn_degrees: angle_degrees(DQuat::from_rotation_arc(before, after)),
            residual_degrees: residual,
            turns_applied,
            pivot_before: pivot_authored.to_array(),
            pivot_after: pivot_after.to_array(),
            length_before,
            length_after,
            axes_after: axes_of(world_rotation(worlds[aim.joint])),
        });
    }
    let posed = geometry::sample(actor, &nodes, &worlds)?;
    Ok(RestBake {
        rest,
        posed,
        aims: rows,
        worlds,
    })
}

/// The three world axes of a rotation, in `x, y, z` order.
fn axes_of(rotation: DQuat) -> [[f64; 3]; 3] {
    let matrix = glam::DMat3::from_quat(rotation);
    let column = |index: usize| {
        let column = matrix.col(index);
        [column.x, column.y, column.z]
    };
    [column(0), column(1), column(2)]
}

/// The rotation of one world transform with its scale divided out.
///
/// A trace-based quaternion extraction from a scaled matrix is not
/// scale-invariant, and a bone that carries one (SSE's hands carry 0.8517)
/// would otherwise turn to a rotation that misses the target by degrees and
/// never converge.
fn world_rotation(world: DMat4) -> DQuat {
    let (_, rotation, _) = world.to_scale_rotation_translation();
    rotation.normalize()
}

/// Upper bound on corrective turns per aim: the residual decays geometrically,
/// and a joint that has not converged by then is reported, not spun forever.
const MAX_AIM_ITERATIONS: usize = 12;

/// A residual at or below this is the float noise of the composition, degrees.
const AIM_CONVERGENCE_DEG: f64 = 1e-9;

/// Rotation magnitude of a quaternion, degrees.
fn angle_degrees(value: DQuat) -> f64 {
    (2.0 * value.w.abs().clamp(-1.0, 1.0).acos()).to_degrees()
}
