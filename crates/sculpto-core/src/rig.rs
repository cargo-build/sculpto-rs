//! Neutral joint graph and static rest transforms for a supported body rig.
//!
//! A NIF skin palette is a subset of a skeleton: its 22/24 names and their order
//! do not carry the hierarchy, the roots or the rest pose. This module owns that
//! one identity — [`Rig`] holds every joint, [`JointId`] names a joint inside it,
//! and the validated static rest-world cache answers import/bind proofs without a
//! second pose evaluator.
//!
//! Frame and units are declared ([`RigFrame`]) and never inferred or converted
//! here. Palette-to-joint references and per-shape binds belong to the skin model,
//! and dynamic posing belongs to the shared evaluator. Wire limits such as the
//! palette size or a bone-name length stay in the format writers.
//!
//! @req DECL0002

use std::fmt;

use glam::{DMat4, DQuat, DVec3};

pub mod rest;

/// Squared-length tolerance for a unit rotation quaternion.
///
/// Source rotations arrive as `f32` matrices whose quaternion is unit to about
/// `1e-6`; `1e-4` refuses a rotation that has scale baked into it while accepting
/// the wire's own rounding. A zero quaternion is far outside it.
const ROTATION_LENGTH_SQUARED_TOLERANCE: f64 = 1e-4;

/// Dense joint identity inside one rig.
///
/// It is deliberately numeric: a palette slot and a native node index are
/// different spaces with the same shape, and only this type crosses the model
/// boundary. The value is only meaningful together with the [`Rig`] it came from;
/// [`Rig::joint`], [`Rig::rest_world`] and [`Rig::is_ancestor`] refuse an id the
/// rig does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JointId(u32);

impl JointId {
    /// Name a joint by its position in the rig.
    ///
    /// Infallible so a caller can spell an id before it owns the graph; every rig
    /// operation validates the id against that rig's bounds.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Position of the joint inside its rig.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for JointId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}

/// Coordinate frame the rig's rest transforms are expressed in.
///
/// Declared, never inferred: a native body NIF is source Z-up and a glTF document
/// is Y-up, so a rig that guessed would silently rotate a skeleton. No code here
/// converts between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigFrame {
    /// Gamebryo/Skyrim source axes, as `NifShape::positions` already use.
    SourceZUp,
    /// glTF renderer axes.
    YUp,
}

/// One joint's local rest transform: rotation, translation, uniform scale.
///
/// Composition is the column-vector `T * R * S`, the rule a native `NiTransform`
/// and a glTF node TRS share. Lateral (non-uniform) scale and shear have no
/// representation here, so a source that carries them is refused instead of
/// approximated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTransform {
    /// Translation in the rig's declared frame and units.
    pub translation: [f32; 3],
    /// Rotation quaternion as `[x, y, z, w]`, the lane order the native importer
    /// stores. The rot/quat interpolator wire order is `w`-first; do not mix them.
    pub rotation: [f32; 4],
    /// One uniform scale shared by all three axes.
    pub scale: f32,
}

impl Default for LocalTransform {
    /// The identity transform.
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        }
    }
}

impl LocalTransform {
    /// Check every field is representable as a rigid body transform.
    ///
    /// # Errors
    /// Returns the first defect: a non-finite field, a scale that is not positive,
    /// or a rotation whose squared length is not one within the tolerance.
    pub fn validate(&self) -> Result<(), TransformDefect> {
        match self.translation.iter().all(|value| value.is_finite()) {
            true => {}
            false => return Err(TransformDefect::NonFiniteTranslation),
        }
        match self.rotation.iter().all(|value| value.is_finite()) {
            true => {}
            false => return Err(TransformDefect::NonFiniteRotation),
        }
        match self.scale.is_finite() {
            true => {}
            false => return Err(TransformDefect::NonFiniteScale),
        }
        match self.scale > 0.0 {
            true => {}
            false => return Err(TransformDefect::NonPositiveScale { scale: self.scale }),
        }
        let length_squared: f64 = self
            .rotation
            .iter()
            .map(|lane| f64::from(*lane).powi(2))
            .sum();
        match (length_squared - 1.0).abs() > ROTATION_LENGTH_SQUARED_TOLERANCE {
            true => Err(TransformDefect::RotationNotUnit { length_squared }),
            false => Ok(()),
        }
    }

    /// Column-major `T * R * S` matrix in the rig's declared frame.
    ///
    /// # Errors
    /// Returns the same defects as [`LocalTransform::validate`].
    pub fn matrix(&self) -> Result<[[f64; 4]; 4], TransformDefect> {
        self.validate()?;
        Ok(self.compose().to_cols_array_2d())
    }

    /// Compose without re-checking; every caller validates first.
    fn compose(&self) -> DMat4 {
        // A validated quaternion is within 1e-4 of unit, so renormalising here is
        // numerical conditioning rather than a silent correction.
        let rotation = DQuat::from_xyzw(
            f64::from(self.rotation[0]),
            f64::from(self.rotation[1]),
            f64::from(self.rotation[2]),
            f64::from(self.rotation[3]),
        )
        .normalize();
        DMat4::from_scale_rotation_translation(
            DVec3::splat(f64::from(self.scale)),
            rotation,
            DVec3::from(self.translation.map(f64::from)),
        )
    }
}

/// Why a rest or placement transform cannot describe a rigid body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformDefect {
    /// A translation lane is not finite.
    NonFiniteTranslation,
    /// A rotation lane is not finite.
    NonFiniteRotation,
    /// The scale is not finite.
    NonFiniteScale,
    /// The scale is zero or negative, so the matrix is singular or mirrored.
    NonPositiveScale {
        /// The refused scale.
        scale: f32,
    },
    /// The rotation is not a unit quaternion (degenerate or with baked scale).
    RotationNotUnit {
        /// Squared length that failed the tolerance.
        length_squared: f64,
    },
}

impl fmt::Display for TransformDefect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteTranslation => write!(formatter, "translation is not finite"),
            Self::NonFiniteRotation => write!(formatter, "rotation is not finite"),
            Self::NonFiniteScale => write!(formatter, "scale is not finite"),
            Self::NonPositiveScale { scale } => {
                write!(formatter, "scale {scale} is not positive")
            }
            Self::RotationNotUnit { length_squared } => {
                write!(
                    formatter,
                    "rotation length squared is {length_squared}, not 1"
                )
            }
        }
    }
}

impl std::error::Error for TransformDefect {}

/// One joint: an optional source name, its parent and its local rest transform.
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    /// Source node name, or `None` for an unnamed ancestor.
    ///
    /// An unnamed joint is valid source data and stays unnamed; no `BoneN` and no
    /// empty string is ever synthesized. `Some("")` (or blank) is refused.
    pub name: Option<String>,
    /// Parent joint, or `None` for a root.
    pub parent: Option<JointId>,
    /// Local transform relative to the parent, in the rig's frame and units.
    pub rest: LocalTransform,
}

/// A validated joint graph with its static rest-world cache.
///
/// The graph is immutable: joint identity, parents and rest transforms cannot be
/// edited after validation, so a bind proof made against one rig cannot be
/// invalidated by a later mutation. Duplicate names are retained as data and are
/// only refused by name lookup.
#[derive(Debug, Clone, PartialEq)]
pub struct Rig {
    joints: Vec<Joint>,
    roots: Vec<JointId>,
    order: Vec<JointId>,
    rest_world: Vec<[[f64; 4]; 4]>,
    frame: RigFrame,
}

impl Rig {
    /// Validate a joint graph and cache every joint's rest world transform.
    ///
    /// # Errors
    /// Refuses an empty graph, more joints than a [`JointId`] can address, a
    /// missing or self parent, a parent cycle, a blank name, and an invalid rest
    /// transform. An invalid required value is never replaced by an identity.
    pub fn new(joints: Vec<Joint>, frame: RigFrame) -> Result<Self, RigError> {
        let count = joints.len();
        match count {
            0 => return Err(RigError::EmptyGraph),
            above if above > u32::MAX as usize => {
                return Err(RigError::TooManyJoints { joints: above });
            }
            _ => {}
        }
        joints
            .iter()
            .enumerate()
            .try_for_each(|(index, joint)| check_joint(joint, index, count))?;
        let order = topological_order(&joints)?;
        let roots = joints
            .iter()
            .enumerate()
            .filter(|(_, joint)| joint.parent.is_none())
            .map(|(index, _)| id_of(index))
            .collect();
        let rest_world = rest_worlds(&joints, &order);
        Ok(Self {
            joints,
            roots,
            order,
            rest_world,
            frame,
        })
    }

    /// Every joint, in rig id order.
    pub fn joints(&self) -> &[Joint] {
        &self.joints
    }

    /// One joint, or `None` when the id belongs to another rig.
    pub fn joint(&self, id: JointId) -> Option<&Joint> {
        self.joints.get(id.index())
    }

    /// Root joints, in rig id order.
    pub fn roots(&self) -> &[JointId] {
        &self.roots
    }

    /// Declared frame of every rest transform in the rig.
    pub fn frame(&self) -> RigFrame {
        self.frame
    }

    /// Joint ids with every parent before its children.
    pub fn topological_order(&self) -> &[JointId] {
        &self.order
    }

    /// Resolve an exact source name.
    ///
    /// # Errors
    /// Returns [`RigError::UnknownJointName`] for no match and
    /// [`RigError::AmbiguousJointName`] when the source authored the name twice.
    /// Identity never comes from a name guess, so ambiguity is not resolved by
    /// picking the first match.
    pub fn joint_by_name(&self, name: &str) -> Result<JointId, RigError> {
        let matches: Vec<JointId> = self
            .joints
            .iter()
            .enumerate()
            .filter(|(_, joint)| joint.name.as_deref() == Some(name))
            .map(|(index, _)| id_of(index))
            .collect();
        match matches.as_slice() {
            [joint] => Ok(*joint),
            [] => Err(RigError::UnknownJointName {
                name: name.to_owned(),
            }),
            found => Err(RigError::AmbiguousJointName {
                name: name.to_owned(),
                matches: found.len(),
            }),
        }
    }

    /// Whether `ancestor` is a strict ancestor of `joint`.
    ///
    /// # Errors
    /// Refuses either id when it is outside this rig.
    pub fn is_ancestor(&self, ancestor: JointId, joint: JointId) -> Result<bool, RigError> {
        self.check(ancestor)?;
        self.check(joint)?;
        let mut cursor = self.joints[joint.index()].parent;
        // The constructor refuses cycles, so this chain is finite.
        while let Some(parent) = cursor {
            match parent == ancestor {
                true => return Ok(true),
                false => cursor = self.joints[parent.index()].parent,
            }
        }
        Ok(false)
    }

    /// Cached rest world transform, column-major in the rig's frame and units.
    ///
    /// # Errors
    /// Refuses an id outside this rig.
    pub fn rest_world(&self, joint: JointId) -> Result<[[f64; 4]; 4], RigError> {
        let index = self.check(joint)?;
        Ok(self.rest_world[index])
    }

    /// Validate an id against this rig's bounds.
    fn check(&self, joint: JointId) -> Result<usize, RigError> {
        match joint.index() < self.joints.len() {
            true => Ok(joint.index()),
            false => Err(RigError::UnknownJoint {
                joint,
                joints: self.joints.len(),
            }),
        }
    }
}

/// A rig, joint graph or transform that cannot describe a body rig.
#[derive(Debug, Clone, PartialEq)]
pub enum RigError {
    /// The graph has no joint, so it has no skeleton.
    EmptyGraph,
    /// More joints than a [`JointId`] can address.
    TooManyJoints {
        /// Joint count that was refused.
        joints: usize,
    },
    /// A joint carries an empty or blank source name.
    EmptyJointName {
        /// The joint holding the blank name.
        joint: JointId,
    },
    /// A `JointId` addresses no joint of this rig.
    UnknownJoint {
        /// The refused id.
        joint: JointId,
        /// Joint count of the rig that refused it.
        joints: usize,
    },
    /// No joint carries the requested name.
    UnknownJointName {
        /// The name that did not resolve.
        name: String,
    },
    /// More than one joint carries the requested name.
    AmbiguousJointName {
        /// The name that was not unique.
        name: String,
        /// How many joints carry it.
        matches: usize,
    },
    /// A parent id addresses no joint of this rig.
    ParentOutOfRange {
        /// The joint whose parent is wrong.
        joint: JointId,
        /// The refused parent id.
        parent: JointId,
        /// Joint count of this rig.
        joints: usize,
    },
    /// A joint names itself as its parent.
    SelfParent {
        /// The joint that is its own parent.
        joint: JointId,
    },
    /// The parent links close a cycle.
    Cycle {
        /// One joint on the cycle.
        joint: JointId,
    },
    /// A joint's rest transform is not a valid rigid body transform.
    Transform {
        /// The joint holding the transform.
        joint: JointId,
        /// What is wrong with it.
        defect: TransformDefect,
    },
}

impl fmt::Display for RigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGraph => write!(formatter, "rig has no joints"),
            Self::TooManyJoints { joints } => {
                write!(
                    formatter,
                    "rig has {joints} joints, above the joint id range"
                )
            }
            Self::EmptyJointName { joint } => write!(formatter, "joint {joint} has a blank name"),
            Self::UnknownJoint { joint, joints } => {
                write!(
                    formatter,
                    "joint {joint} is outside the rig's {joints} joints"
                )
            }
            Self::UnknownJointName { name } => {
                write!(formatter, "no joint is named {name:?}")
            }
            Self::AmbiguousJointName { name, matches } => {
                write!(formatter, "{matches} joints are named {name:?}")
            }
            Self::ParentOutOfRange {
                joint,
                parent,
                joints,
            } => write!(
                formatter,
                "joint {joint} names parent {parent} outside the rig's {joints} joints"
            ),
            Self::SelfParent { joint } => write!(formatter, "joint {joint} is its own parent"),
            Self::Cycle { joint } => write!(formatter, "joint {joint} closes a parent cycle"),
            Self::Transform { joint, defect } => {
                write!(
                    formatter,
                    "joint {joint} has an invalid rest transform: {defect}"
                )
            }
        }
    }
}

impl std::error::Error for RigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transform { defect, .. } => Some(defect),
            _ => None,
        }
    }
}

/// The id of a joint position the constructor already proved representable.
fn id_of(index: usize) -> JointId {
    JointId::new(u32::try_from(index).expect("joint count is bounded by u32::MAX"))
}

/// Check one joint's name, parent and rest transform against the graph.
fn check_joint(joint: &Joint, index: usize, count: usize) -> Result<(), RigError> {
    let id = id_of(index);
    if let Some(name) = joint.name.as_deref()
        && name.trim().is_empty()
    {
        return Err(RigError::EmptyJointName { joint: id });
    }
    joint
        .rest
        .validate()
        .map_err(|defect| RigError::Transform { joint: id, defect })?;
    match joint.parent {
        Some(parent) if parent.index() >= count => Err(RigError::ParentOutOfRange {
            joint: id,
            parent,
            joints: count,
        }),
        Some(parent) if parent == id => Err(RigError::SelfParent { joint: id }),
        _ => Ok(()),
    }
}

/// Order every joint after its parent, refusing a parent cycle.
///
/// Each start walks its parent chain and marks the path, so a long input chain
/// costs no stack frame per joint.
fn topological_order(joints: &[Joint]) -> Result<Vec<JointId>, RigError> {
    const UNVISITED: u8 = 0;
    const ON_PATH: u8 = 1;
    const PLACED: u8 = 2;
    let mut state = vec![UNVISITED; joints.len()];
    let mut order = Vec::with_capacity(joints.len());
    for start in 0..joints.len() {
        if state[start] == PLACED {
            continue;
        }
        let mut path = Vec::new();
        let mut cursor = Some(start);
        while let Some(node) = cursor {
            match state[node] {
                PLACED => break,
                ON_PATH => return Err(RigError::Cycle { joint: id_of(node) }),
                _ => {}
            }
            state[node] = ON_PATH;
            path.push(node);
            cursor = joints[node].parent.map(JointId::index);
        }
        for &node in path.iter().rev() {
            state[node] = PLACED;
            order.push(id_of(node));
        }
    }
    Ok(order)
}

/// Compose each joint's rest world once, parents first.
fn rest_worlds(joints: &[Joint], order: &[JointId]) -> Vec<[[f64; 4]; 4]> {
    let mut worlds = vec![DMat4::IDENTITY; joints.len()];
    for id in order {
        let joint = &joints[id.index()];
        let local = joint.rest.compose();
        worlds[id.index()] = match joint.parent {
            Some(parent) => worlds[parent.index()] * local,
            None => local,
        };
    }
    worlds
        .into_iter()
        .map(|world| world.to_cols_array_2d())
        .collect()
}
