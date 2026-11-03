//! Explicit actor selection and f64 world-space glTF motion. @req DECL0013

mod animation;
mod decode;
mod geometry;
mod packing;
mod rig;

pub use packing::{ClothPacking, PackedCloth, pack_cloth};

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

/// Sampled geometry plus its reduced rig correspondence. @req DECL0013
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RigSample {
    pub sample: Sample,
    pub rig: Vec<RigNode>,
    pub requested: Vec<RequestedJoint>,
    pub influences: Vec<Vec<Influence>>,
}

/// Classified input failures retain their source location. @req DECL0013
#[derive(Debug)]
#[non_exhaustive]
pub enum ActorError {
    Decode(gltf::Error),
    Invalid { context: String, reason: String },
    Unsupported { context: String, feature: String },
    Selection { name: String, matches: usize },
    DuplicatePart { name: String },
    JointSelection { name: String, matches: usize },
    DuplicateJoint { name: String },
    DisconnectedRig,
    Clip { name: String, matches: usize },
    Time { time: f64, start: f64, end: f64 },
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
            Self::JointSelection { name, matches } => {
                write!(
                    f,
                    "actor joint {name:?}: expected one node, found {matches}"
                )
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

    fn buffer(&self, buffer: gltf::Buffer<'_>) -> Option<&[u8]> {
        match buffer.source() {
            gltf::buffer::Source::Bin if buffer.index() == 0 => Some(&self.blob),
            _ => None,
        }
    }
}
