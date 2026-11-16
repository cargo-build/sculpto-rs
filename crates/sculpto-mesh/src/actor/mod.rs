//! Explicit actor selection and f64 world-space glTF motion. @req DECL0013

mod animation;
mod clips;
mod decode;
mod fit;
mod gait;
mod geometry;
mod packing;
mod pose;
mod rest;
mod rig;
mod shoulder;
mod stress;
mod torso;

pub use clips::{LocalRest, SourceChannel, SourceClip, SourceProperty, SourceValues};
pub use fit::{
    ChainFit, FitOutcome, FittedFrame, Flexion, JointFit, LengthMode, UnfittedChain, extent, fit,
    write_fit, write_frames,
};
pub use gait::{GaitOutcome, GaitParameters, JointRange, gait, write_gait_clip};
pub use packing::{ClothPacking, PackedCloth, pack_cloth};
pub use pose::{Axis, JointPose, JointProbe, PoseOutcome, PosedJoint, probe, write_probe_clip};
pub use rest::{AimedJoint, RestAim, RestBake, bake_rest};
pub use shoulder::{SEAT_CHAIN, SeatBone, SeatJoint, SeatOutcome, SeatWall, shoulder_seat};
pub use stress::{
    FrameRow, GarmentRow, PartAddress, StressError, StressPlan, StressPose, StressReport, stress,
};
pub use torso::{
    AnatomyLevel, BodyFrame, Decision, LATERAL_TOLERANCE, LimbProbe, PartSelector, Section,
    SectionLoop, TORSO_CHAIN, TorsoBone, TorsoJoint, TorsoOutcome, torso_fit,
};

use std::fmt;
use std::ops::Range;

use glam::{DMat4, DQuat, DVec3};

/// Asset defaults and animation samples are distinct poses. @req DECL0013
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Pose<'clip> {
    Rest,
    Clip { name: &'clip str, time: f64 },
}

/// Source identity and half-open ranges in the combined sample. @req DECL0013
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Part {
    pub name: String,
    pub node: usize,
    pub mesh: usize,
    pub primitive: usize,
    pub vertices: Range<usize>,
    pub triangles: Range<usize>,
}

/// Ordered primitive geometry in world-space glTF Y-up units. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Sample {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[usize; 3]>,
    pub parts: Vec<Part>,
}

/// A retained rig node keeps its original identity and pose world transform.
/// @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RigNode {
    pub node: usize,
    pub name: Option<String>,
    pub parent: Option<usize>,
    pub world: [[f64; 4]; 4],
    pub role: RigRole,
}

/// Connectors exist only to keep requested joints one connected graph.
/// @req DECL0013
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigRole {
    Requested,
    Connector,
}

/// One requested joint name bound to its retained rig position. @req DECL0013
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestedJoint {
    pub name: String,
    pub node: usize,
    pub rig: usize,
}

/// Sparse retained-rig binding for one sampled vertex. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Influence {
    pub rig: usize,
    pub weight: f64,
}

/// One ambiguous joint name's candidate node, with the evidence the
/// resolution rule reads. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct JointCandidate {
    /// The candidate's graph node index.
    pub node: usize,
    /// Whether the selected parts' skins reference this node.
    pub is_skin_joint: bool,
    /// Ancestors between the node and its graph root, exclusive.
    pub parent_depth: usize,
}

/// An ambiguous joint name the skin resolved, kept so a report can say which
/// node won and why. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct JointDisambiguation {
    /// The requested name; it stays a label, the node is the address.
    pub name: String,
    /// The skin-referenced node the resolution bound.
    pub chosen: JointCandidate,
    /// The other matches the rule did not choose.
    pub refused: Vec<JointCandidate>,
}

/// Sampled geometry plus its reduced rig correspondence. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RigSample {
    pub sample: Sample,
    pub rig: Vec<RigNode>,
    pub requested: Vec<RequestedJoint>,
    pub influences: Vec<Vec<Influence>>,
    /// Names shared by several nodes that the skin tie-break resolved.
    pub disambiguated: Vec<JointDisambiguation>,
}

/// One primitive's part material: its name and base colour. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
pub struct PartColor {
    /// glTF material name, usually the part name.
    pub name: String,
    /// The material's `baseColorFactor`.
    pub color: [f32; 4],
}

/// Classified input failures retain their source location. @req DECL0013
#[derive(Debug)]
#[non_exhaustive]
pub enum ActorError {
    Decode(gltf::Error),
    Invalid {
        context: String,
        reason: String,
    },
    Unsupported {
        context: String,
        feature: String,
    },
    Selection {
        name: String,
        matches: usize,
    },
    DuplicatePart {
        name: String,
    },
    JointSelection {
        name: String,
        matches: usize,
        candidates: Vec<JointCandidate>,
    },
    DuplicateJoint {
        name: String,
    },
    DisconnectedRig,
    Clip {
        name: String,
        matches: usize,
    },
    Time {
        time: f64,
        start: f64,
        end: f64,
    },
}

impl fmt::Display for ActorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "invalid actor GLB: {error}"),
            Self::Invalid { context, reason } => write!(f, "{context}: {reason}"),
            Self::Unsupported { context, feature } => {
                write!(f, "{context}: unsupported {feature}")
            }
            Self::Selection { name, matches } => {
                write!(
                    f,
                    "actor part {name:?}: expected one mesh node, found {matches}"
                )
            }
            Self::DuplicatePart { name } => write!(f, "duplicate actor part {name:?}"),
            Self::JointSelection {
                name,
                matches,
                candidates,
            } => {
                write!(
                    f,
                    "actor joint {name:?}: expected one node, found {matches}"
                )?;
                if candidates.is_empty() {
                    return Ok(());
                }
                let evidence = candidates
                    .iter()
                    .map(|candidate| {
                        let skin = match candidate.is_skin_joint {
                            true => "skin joint",
                            false => "no skin reference",
                        };
                        format!(
                            "node {} ({skin}, parent depth {})",
                            candidate.node, candidate.parent_depth
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "; candidates: {evidence}")
            }
            Self::DuplicateJoint { name } => write!(f, "duplicate actor joint {name:?}"),
            Self::DisconnectedRig => {
                write!(f, "requested joints share no common ancestor node")
            }
            Self::Clip { name, matches } => {
                write!(f, "actor clip {name:?}: expected one clip, found {matches}")
            }
            Self::Time { time, start, end } => {
                write!(
                    f,
                    "actor time {time}: expected finite seconds in [{start}, {end}]"
                )
            }
        }
    }
}

impl std::error::Error for ActorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

pub(super) fn invalid(context: impl Into<String>, reason: impl Into<String>) -> ActorError {
    ActorError::Invalid {
        context: context.into(),
        reason: reason.into(),
    }
}

pub(super) fn unsupported(context: impl Into<String>, feature: impl Into<String>) -> ActorError {
    ActorError::Unsupported {
        context: context.into(),
        feature: feature.into(),
    }
}

#[derive(Clone, Debug)]
pub(super) enum Transform {
    Matrix(DMat4),
    Trs {
        translation: DVec3,
        rotation: DQuat,
        scale: DVec3,
    },
}

impl Transform {
    pub(super) fn matrix(&self) -> DMat4 {
        match self {
            Self::Matrix(matrix) => *matrix,
            Self::Trs {
                translation,
                rotation,
                scale,
            } => DMat4::from_scale_rotation_translation(*scale, *rotation, *translation),
        }
    }
}

/// Validated embedded GLB; no file or image I/O occurs here. @req DECL0013
#[derive(Debug)]
pub struct Actor {
    document: gltf::Document,
    blob: Vec<u8>,
    transforms: Vec<Transform>,
    parents: Vec<Option<usize>>,
    order: Vec<usize>,
}

impl Actor {
    /// Decode framing, references and hierarchy before accessor reads. @req DECL0013
    pub fn from_glb(bytes: &[u8]) -> Result<Self, ActorError> {
        decode::read(bytes)
    }

    /// Preserve selection/primitive order without welding or axis conversion. @req DECL0013
    pub fn sample(&self, parts: &[&str], pose: Pose<'_>) -> Result<Sample, ActorError> {
        let nodes = geometry::select(&self.document, parts)?;
        let worlds = animation::worlds(self, &nodes, pose)?;
        geometry::sample(self, &nodes, &worlds)
    }

    /// Authored time span of one uniquely named clip. @req DECL0013
    ///
    /// # Errors
    /// The typed clip refusal ([`ActorError::Clip`]) when the name matches
    /// zero or several clips.
    pub fn clip_span(&self, name: &str) -> Result<(f64, f64), ActorError> {
        animation::span(self, name)
    }

    /// Every node's world transform at one pose, without sampling geometry.
    ///
    /// A caller that needs the *skeleton* at a clip time — choosing a rest frame,
    /// comparing joint directions — would otherwise pay for a full skinned sample
    /// of the mesh on every candidate. The composition is the same owner the
    /// vertex path uses, so the two cannot disagree.
    ///
    /// # Errors
    /// A clip name that matches zero or several clips, an unreadable channel, or a
    /// sampled time outside the clip's authored span.
    pub fn skeleton_worlds(&self, pose: Pose<'_>) -> Result<Vec<DMat4>, ActorError> {
        animation::worlds(self, &[], pose)
    }

    /// Every node's local rotation at one pose, without composing the hierarchy.
    ///
    /// The frame a retarget's local delta lives in: a clip written against one
    /// skeleton's rest is turned into a delta by dividing that rest out here, not
    /// by comparing world matrices two different bases built.
    ///
    /// # Errors
    /// The same clip refusals as [`Actor::skeleton_worlds`].
    pub fn skeleton_locals(&self, pose: Pose<'_>) -> Result<Vec<DQuat>, ActorError> {
        animation::locals(self, &[], pose)
    }

    /// Every node's local translation and rotation at one pose.
    ///
    /// The pair a compensated application needs: the rotation for the bone's
    /// frame, the translation for its child's pivot, so a world delta can be
    /// turned into a direction instead of rotating whatever axis this rig's bones
    /// happen to use.
    ///
    /// # Errors
    /// The same clip refusals as [`Actor::skeleton_worlds`].
    pub fn skeleton_transforms(&self, pose: Pose<'_>) -> Result<Vec<(DVec3, DQuat)>, ActorError> {
        animation::local_poses(self, &[], pose)
    }

    /// Reduced skin correspondence for the same explicit parts and pose.
    /// @req DECL0013
    pub fn rig_sample(
        &self,
        parts: &[&str],
        pose: Pose<'_>,
        joints: &[&str],
    ) -> Result<RigSample, ActorError> {
        rig::sample(self, parts, pose, joints)
    }

    /// The node's parent, for verbs that report a joint's parent link. @req DECL0013
    pub(super) fn parent_of(&self, node: usize) -> Option<usize> {
        self.parents.get(node).copied().flatten()
    }

    /// The validated glTF document, for read-only inspection by other verbs
    /// (primitive layout, materials). No I/O occurs here; [`Actor::from_glb`]
    /// already validated framing, references and hierarchy. @req DECL0013
    pub fn document(&self) -> &gltf::Document {
        &self.document
    }

    /// The unique skinned mesh node and its name, for a weight transfer.
    ///
    /// # Errors
    /// Refuses zero or several skinned mesh nodes, or an unnamed one.
    /// @req DECL0013
    pub fn skinned_mesh_node(&self) -> Result<(usize, String), ActorError> {
        let mut found = self
            .document
            .nodes()
            .filter(|node| node.mesh().is_some() && node.skin().is_some())
            .map(|node| (node.index(), node.name().map(str::to_owned)));
        let Some((node, name)) = found.next() else {
            return Err(invalid(
                "actor skin",
                "the actor carries no skinned mesh node",
            ));
        };
        if found.next().is_some() {
            return Err(invalid(
                "actor skin",
                "the actor carries several skinned mesh nodes; a transfer seats one",
            ));
        }
        let name = name
            .ok_or_else(|| invalid(format!("node {node}"), "the skinned mesh node is unnamed"))?;
        Ok((node, name))
    }

    /// Validate the shared-pool actor layout: every primitive shares one
    /// POSITION/JOINTS_0/WEIGHTS_0 arena, is triangles, and carries no morphs.
    /// Returns the primitive count. @req DECL0013
    ///
    /// # Errors
    /// Refuses an empty mesh, a primitive that does not share the pool, a
    /// non-triangle mode and morph geometry.
    pub fn shared_pool_parts(&self, node: usize) -> Result<usize, ActorError> {
        use gltf::mesh::{Mode, Semantic};
        let mesh = self
            .document
            .nodes()
            .nth(node)
            .and_then(|node| node.mesh())
            .ok_or_else(|| invalid(format!("node {node}"), "missing selected mesh"))?;
        let primitives: Vec<_> = mesh.primitives().collect();
        if primitives.is_empty() {
            return Err(invalid(
                format!("node {node}"),
                "the mesh has no primitives",
            ));
        }
        let first = &primitives[0];
        let index_of = |primitive: &gltf::Primitive<'_>, semantic: &Semantic| {
            primitive.get(semantic).map(|accessor| accessor.index())
        };
        for primitive in &primitives {
            let context = format!("node {node} primitive {}", primitive.index());
            for semantic in [
                Semantic::Positions,
                Semantic::Joints(0),
                Semantic::Weights(0),
            ] {
                if index_of(primitive, &semantic) != index_of(first, &semantic) {
                    return Err(invalid(
                        &context,
                        "the primitives do not share one POSITION/JOINTS_0/WEIGHTS_0 vertex pool",
                    ));
                }
            }
            if primitive.mode() != Mode::Triangles {
                return Err(unsupported(
                    &context,
                    format!("mode {:?}", primitive.mode()),
                ));
            }
            if primitive.morph_targets().len() > 0 {
                return Err(unsupported(&context, "morph geometry"));
            }
        }
        Ok(primitives.len())
    }

    /// One part colour per primitive of a mesh node. @req DECL0013
    ///
    /// # Errors
    /// Refuses a textured material: the colour-only part route would drop it.
    pub fn part_colors(&self, node: usize) -> Result<Vec<PartColor>, ActorError> {
        let mesh = self
            .document
            .nodes()
            .nth(node)
            .and_then(|node| node.mesh())
            .ok_or_else(|| invalid(format!("node {node}"), "missing selected mesh"))?;
        let mut colors = Vec::new();
        for primitive in mesh.primitives() {
            let material = primitive.material();
            let pbr = material.pbr_metallic_roughness();
            if pbr.base_color_texture().is_some()
                || pbr.metallic_roughness_texture().is_some()
                || material.normal_texture().is_some()
                || material.occlusion_texture().is_some()
                || material.emissive_texture().is_some()
            {
                return Err(unsupported(
                    format!("node {node} primitive {}", primitive.index()),
                    "a textured material; the part route writes colour only",
                ));
            }
            colors.push(PartColor {
                name: material.name().unwrap_or("part").to_owned(),
                color: pbr.base_color_factor(),
            });
        }
        Ok(colors)
    }

    fn buffer(&self, buffer: gltf::Buffer<'_>) -> Option<&[u8]> {
        match buffer.source() {
            gltf::buffer::Source::Bin if buffer.index() == 0 => Some(&self.blob),
            _ => None,
        }
    }
}
