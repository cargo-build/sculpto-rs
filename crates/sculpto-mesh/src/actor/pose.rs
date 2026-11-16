//! Explicit joint-angle probes on top of rest, with a world-space displacement
//! report. @req DECL0013
//!
//! A probe rotates named joints about a **world** axis through their own pivot
//! and leaves every other joint at rest. The axis letter names the world axis,
//! not the joint's local one: the pelvis's rest world is a −90° X rotation, so
//! its local `y` is the world's horizontal `−z` and a "yaw" written as a local
//! `y` would roll the figure. Reading the letter as the world axis makes
//! `--axis y` mean the vertical everywhere, which is what a yaw probe wants.
//! The local delta the probe writes is reported next to it, so both frames are
//! visible. The same evaluator that plays a clip composes the probe, so the
//! reported displacement is exactly what a written probe clip reproduces.

use glam::{DQuat, DVec3};
use sculpto_gltf::pack::{PackWriter, Track};

use super::{Actor, ActorError, Transform, animation, geometry, invalid, packing::packed, rig};

/// The world axis a probe rotates about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    /// The unit basis vector of this axis in world space.
    pub fn vector(self) -> DVec3 {
        match self {
            Self::X => DVec3::X,
            Self::Y => DVec3::Y,
            Self::Z => DVec3::Z,
        }
    }

    /// Parse a CLI axis letter; anything else is a named refusal.
    pub fn parse(value: &str) -> Result<Self, ActorError> {
        match value {
            "x" | "X" => Ok(Self::X),
            "y" | "Y" => Ok(Self::Y),
            "z" | "Z" => Ok(Self::Z),
            _ => Err(invalid(
                "pose axis",
                format!("expected x, y or z, found {value:?}"),
            )),
        }
    }
}

/// One requested probe: a joint name, a world axis and an angle in degrees.
#[derive(Clone, Copy, Debug)]
pub struct JointPose<'name> {
    pub name: &'name str,
    pub axis: Axis,
    pub angle_deg: f64,
}

/// A resolved probe joint with its rest and posed local rotations.
#[derive(Clone, Debug)]
pub struct PosedJoint {
    pub name: String,
    pub node: usize,
    pub axis: Axis,
    pub angle_deg: f64,
    /// The requested world axis.
    pub axis_world: DVec3,
    /// The same axis in the joint's local frame, as written to the clip.
    pub axis_local: DVec3,
    /// The applied local-frame rotation (the delta on top of rest).
    pub delta: DQuat,
    pub rest_rotation: DQuat,
    /// `rest_rotation * delta`: the local rotation a probe clip writes.
    pub posed_rotation: DQuat,
    /// The joint's rest world position (its pivot).
    pub world_pivot: DVec3,
}

/// One probe joint's measured effect on the sampled geometry.
#[derive(Clone, Debug)]
pub struct JointProbe {
    pub name: String,
    pub node: usize,
    pub axis: Axis,
    pub angle_deg: f64,
    pub axis_world: [f64; 3],
    pub axis_local: [f64; 3],
    pub delta_quaternion: [f64; 4],
    pub rest_rotation: [f64; 4],
    pub posed_rotation: [f64; 4],
    pub world_pivot: [f64; 3],
    /// Vertices weighted to this joint's subtree that moved more than [`EPS`].
    pub moved_vertices: usize,
    /// Farthest weighted-vertex displacement, in actor units (metres).
    pub max_displacement: f64,
    pub farthest_vertex: usize,
    pub farthest_rest: [f64; 3],
    pub farthest_posed: [f64; 3],
}

/// Resolved probes plus their measured displacement report.
#[derive(Clone, Debug)]
pub struct PoseOutcome {
    pub joints: Vec<PosedJoint>,
    pub probes: Vec<JointProbe>,
    pub vertex_count: usize,
}

/// Displacements below this are treated as unmoved (f32 wire noise).
const EPS: f64 = 1e-6;

/// Resolve the probes, compose rest and posed worlds, and measure how far each
/// joint's weighted geometry moves.
///
/// # Errors
/// A non-unique or missing joint name, a static-matrix joint (no local TRS to
/// rotate), a non-finite angle, or any sampling refusal of the selected parts.
pub fn probe(
    actor: &Actor,
    parts: &[&str],
    joints: &[JointPose<'_>],
) -> Result<PoseOutcome, ActorError> {
    if joints.is_empty() {
        return Err(invalid("actor pose", "at least one --joint is required"));
    }
    let names: Vec<&str> = joints.iter().map(|joint| joint.name).collect();
    let resolved = rig::resolve(actor, &names)?;
    let selected = geometry::select(&actor.document, parts)?;
    let rest_worlds = animation::compose_worlds(actor, &actor.transforms)?;

    let mut posed_transforms = actor.transforms.clone();
    let mut posed = Vec::with_capacity(joints.len());
    for (request, resolved) in joints.iter().zip(resolved) {
        if !request.angle_deg.is_finite() {
            return Err(invalid(
                format!("joint {:?}", request.name),
                "angle must be finite",
            ));
        }
        let Transform::Trs {
            translation,
            rotation,
            scale,
        } = &actor.transforms[resolved.node]
        else {
            return Err(invalid(
                format!("joint {:?}", request.name),
                "joint is a static matrix; it has no local rotation to probe",
            ));
        };
        // A world axis becomes a local delta through the joint's rest world, so
        // `posed_world = axis_rotation · rest_world` exactly: the probe turns the
        // joint (and its subtree) about the requested world axis through its pivot.
        let rest_world_rotation = DQuat::from_mat4(&rest_worlds[resolved.node]);
        let axis_local = (rest_world_rotation.inverse() * request.axis.vector()).normalize();
        let delta = DQuat::from_axis_angle(axis_local, request.angle_deg.to_radians());
        let posed_rotation = (*rotation * delta).normalize();
        posed_transforms[resolved.node] = Transform::Trs {
            translation: *translation,
            rotation: posed_rotation,
            scale: *scale,
        };
        posed.push(PosedJoint {
            name: resolved.name,
            node: resolved.node,
            axis: request.axis,
            angle_deg: request.angle_deg,
            axis_world: request.axis.vector(),
            axis_local,
            delta,
            rest_rotation: *rotation,
            posed_rotation,
            world_pivot: rest_worlds[resolved.node].transform_point3(DVec3::ZERO),
        });
    }

    let posed_worlds = animation::compose_worlds(actor, &posed_transforms)?;
    let rest_sample = geometry::sample(actor, &selected, &rest_worlds)?;
    let posed_sample = geometry::sample(actor, &selected, &posed_worlds)?;
    let influences = influence_nodes(actor, &rest_sample)?;
    let children = children_of(actor);

    let probes = posed
        .iter()
        .map(|joint| {
            let subtree = subtree(&children, joint.node);
            let mut farthest = (0.0_f64, 0usize);
            let mut moved = 0usize;
            for (vertex, nodes) in influences.iter().enumerate() {
                if !nodes.iter().any(|node| subtree[*node]) {
                    continue;
                }
                let rest = DVec3::from_array(rest_sample.vertices[vertex]);
                let posed = DVec3::from_array(posed_sample.vertices[vertex]);
                let distance = rest.distance(posed);
                if distance > EPS {
                    moved += 1;
                }
                if distance > farthest.0 {
                    farthest = (distance, vertex);
                }
            }
            let (max_displacement, farthest_vertex) = farthest;
            JointProbe {
                name: joint.name.clone(),
                node: joint.node,
                axis: joint.axis,
                angle_deg: joint.angle_deg,
                axis_world: joint.axis_world.to_array(),
                axis_local: joint.axis_local.to_array(),
                delta_quaternion: joint.delta.to_array(),
                rest_rotation: joint.rest_rotation.to_array(),
                posed_rotation: joint.posed_rotation.to_array(),
                world_pivot: joint.world_pivot.to_array(),
                moved_vertices: moved,
                max_displacement,
                farthest_vertex,
                farthest_rest: rest_sample.vertices[farthest_vertex],
                farthest_posed: posed_sample.vertices[farthest_vertex],
            }
        })
        .collect();

    Ok(PoseOutcome {
        joints: posed,
        probes,
        vertex_count: rest_sample.vertices.len(),
    })
}

/// Write the probe as a constant clip inserted at the walk slot (index 1),
/// preserving the idle clip and shifting later clips back. Returns the GLB and
/// the inserted clip index. @req DECL0013
///
/// # Errors
/// Any container refusal of the append-only packer.
pub fn write_probe_clip(
    base: &[u8],
    outcome: &PoseOutcome,
    clip: &str,
) -> Result<(Vec<u8>, usize), ActorError> {
    let tracks: Vec<Track> = outcome
        .joints
        .iter()
        .map(|joint| Track::Rotation {
            node: joint.node,
            values: vec![joint.posed_rotation.to_array().map(|v| v as f32); 2],
        })
        .collect();
    let mut writer = PackWriter::from_glb(base).map_err(packed)?;
    let index = writer
        .insert_animation(1, clip, &[0.0, 1.0], &tracks)
        .map_err(packed)?;
    Ok((writer.finish().map_err(packed)?, index))
}

/// One child list per node, indexed by node index.
fn children_of(actor: &Actor) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); actor.parents.len()];
    actor.parents.iter().enumerate().for_each(|(node, parent)| {
        if let Some(parent) = parent {
            children[*parent].push(node);
        }
    });
    children
}

/// A boolean membership table for a joint and all of its descendants.
fn subtree(children: &[Vec<usize>], root: usize) -> Vec<bool> {
    let mut member = vec![false; children.len()];
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if member[node] {
            continue;
        }
        member[node] = true;
        stack.extend(children[node].iter().copied());
    }
    member
}

/// Per-vertex influence node indices (weight > 0), aligned to the sample.
fn influence_nodes(actor: &Actor, sample: &super::Sample) -> Result<Vec<Vec<usize>>, ActorError> {
    let mut rows = vec![Vec::new(); sample.vertices.len()];
    for part in &sample.parts {
        let context = format!(
            "part {:?} node {} mesh {} primitive {}",
            part.name, part.node, part.mesh, part.primitive
        );
        let node = actor
            .document
            .nodes()
            .nth(part.node)
            .ok_or_else(|| invalid(format!("node {}", part.node), "missing selected node"))?;
        let skin = node
            .skin()
            .ok_or_else(|| super::unsupported(&context, "unskinned mesh"))?;
        let joints: Vec<usize> = skin.joints().map(|joint| joint.index()).collect();
        let primitive = node
            .mesh()
            .and_then(|mesh| mesh.primitives().nth(part.primitive))
            .ok_or_else(|| invalid(&context, "missing selected primitive"))?;
        let skinning = geometry::skin_rows(
            actor,
            &primitive,
            joints.len(),
            part.vertices.len(),
            &context,
        )?;
        for (offset, row) in skinning.into_iter().enumerate() {
            let vertex = part.vertices.start + offset;
            rows[vertex] = row
                .joints
                .into_iter()
                .zip(row.weights)
                .filter(|(_, weight)| *weight > 0.0)
                .map(|(joint, _)| joints[usize::from(joint)])
                .collect();
        }
    }
    Ok(rows)
}
