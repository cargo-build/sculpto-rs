//! Resolve explicit joint mappings for supported body rigs. @req DECL0010

use std::collections::BTreeMap;
use std::fmt;

use sculpto_core::rig::rest::RestBinding;
use sculpto_core::rig::{JointId, Rig, RigError, RigFrame};

/// Body rigs supported by the cross-family workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFamily {
    Himbo,
    Cbbe,
    ThreeBa,
}

/// A named fallback for one source joint with no exact destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Collapse<'a> {
    pub source: &'a str,
    pub target: &'a str,
}

/// Resolution in the target rig's joint identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Direct(JointId),
    Collapsed(JointId),
    Unsupported,
}

/// One source joint and whether it contributes to the checked skin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JointMapping {
    pub source: JointId,
    pub weighted: bool,
    pub destination: Destination,
}

/// Mapping bound to the two graphs that own its joint IDs.
#[derive(Debug)]
pub struct BoneMap<'a> {
    source: &'a Rig,
    target: &'a Rig,
    entries: Vec<JointMapping>,
}

impl<'a> BoneMap<'a> {
    /// Measure every source joint against the selected target family. @req DECL0010
    ///
    /// # Errors
    /// Refuses unsupported family pairs, mixed frames, ambiguous joint names,
    /// missing named collapse endpoints and competing declarations.
    pub fn build(
        source_family: BodyFamily,
        source: &RestBinding<'a>,
        target_family: BodyFamily,
        target: &'a Rig,
        collapses: &[Collapse<'_>],
    ) -> Result<Self, BoneMapError> {
        use BodyFamily::{Cbbe, Himbo, ThreeBa};
        if !matches!((source_family, target_family), (Himbo, Cbbe | ThreeBa) | (Cbbe | ThreeBa, Himbo)) {
            return Err(BoneMapError::Families { source: source_family, target: target_family });
        }
        let rig = source.rig();
        if rig.frame() != target.frame() {
            return Err(BoneMapError::Frame { source: rig.frame(), target: target.frame() });
        }
        let source_names = names(rig, "source")?;
        let target_names = names(target, "target")?;
        let mut authored = BTreeMap::new();
        for collapse in collapses {
            let joint = named(&source_names, collapse.source, "source")?;
            let destination = named(&target_names, collapse.target, "target")?;
            if target_names.contains_key(collapse.source) {
                return Err(BoneMapError::DirectCollapse { joint, name: collapse.source.into() });
            }
            if authored.insert(joint, destination).is_some() {
                return Err(BoneMapError::DuplicateCollapse { joint, name: collapse.source.into() });
            }
        }
        let weighted = weighted_joints(source);
        let entries = rig
            .joints()
            .iter()
            .enumerate()
            .map(|(index, joint)| {
                let source = JointId::new(index as u32);
                let direct = joint.name.as_deref().and_then(|name| target_names.get(name));
                let destination = match (direct, authored.get(&source)) {
                    (Some(&id), _) => Destination::Direct(id),
                    (None, Some(&id)) => Destination::Collapsed(id),
                    (None, None) => Destination::Unsupported,
                };
                JointMapping { source, weighted: weighted[index], destination }
            })
            .collect();
        Ok(Self { source: rig, target, entries })
    }

    /// Graph that owns the source joint IDs.
    pub fn source(&self) -> &'a Rig {
        self.source
    }

    /// Graph that owns the resolved destination IDs.
    pub fn target(&self) -> &'a Rig {
        self.target
    }

    /// One outcome per source joint in identity order.
    pub fn entries(&self) -> &[JointMapping] {
        &self.entries
    }

    /// Resolve a joint for a skin remap, preserving unsupported-joint context.
    ///
    /// # Errors
    /// Refuses an unknown source ID or a source with no destination.
    pub fn resolve(&self, joint: JointId) -> Result<JointId, BoneMapError> {
        let entry = self.entries.get(joint.index()).ok_or(BoneMapError::UnknownSource(joint))?;
        match entry.destination {
            Destination::Direct(id) | Destination::Collapsed(id) => Ok(id),
            Destination::Unsupported => Err(BoneMapError::Unsupported {
                joint,
                name: self.source.joints()[joint.index()].name.clone(),
            }),
        }
    }
}

fn names<'a>(rig: &'a Rig, side: &'static str) -> Result<BTreeMap<&'a str, JointId>, BoneMapError> {
    let mut names = BTreeMap::new();
    for (index, joint) in rig.joints().iter().enumerate() {
        if let Some(name) = joint.name.as_deref()
            && names.insert(name, JointId::new(index as u32)).is_some()
        {
            let error = rig.joint_by_name(name).expect_err("duplicate joint name");
            return Err(BoneMapError::NamedJoint { side, error });
        }
    }
    Ok(names)
}

fn named(
    names: &BTreeMap<&str, JointId>,
    name: &str,
    side: &'static str,
) -> Result<JointId, BoneMapError> {
    names.get(name).copied().ok_or_else(|| BoneMapError::NamedJoint {
        side,
        error: RigError::UnknownJointName { name: name.into() },
    })
}

fn weighted_joints(binding: &RestBinding<'_>) -> Vec<bool> {
    let mut weighted = vec![false; binding.rig().joints().len()];
    for skin in binding.skins() {
        let weights = &skin.source().skin.influences;
        for vertex in 0..weights.rows() {
            let row = weights.row(vertex).expect("checked skin row");
            for (&slot, &weight) in row.joints.iter().zip(row.weights) {
                if weight > 0.0 {
                    weighted[skin.palette()[usize::from(slot)].index()] = true;
                }
            }
        }
    }
    weighted
}

/// A requested body-family mapping could not resolve its inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum BoneMapError {
    Families { source: BodyFamily, target: BodyFamily },
    Frame { source: RigFrame, target: RigFrame },
    NamedJoint { side: &'static str, error: RigError },
    DirectCollapse { joint: JointId, name: String },
    DuplicateCollapse { joint: JointId, name: String },
    UnknownSource(JointId),
    Unsupported { joint: JointId, name: Option<String> },
}

impl fmt::Display for BoneMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Families { source, target } => write!(f, "unsupported body family pair {source:?} to {target:?}"),
            Self::Frame { source, target } => write!(f, "rig frame {source:?} differs from target {target:?}"),
            Self::NamedJoint { side, error } => write!(f, "{side} joint: {error}"),
            Self::DirectCollapse { joint, name } => write!(f, "source joint {joint} `{name}` already has an exact destination"),
            Self::DuplicateCollapse { joint, name } => write!(f, "source joint {joint} `{name}` has multiple collapse declarations"),
            Self::UnknownSource(joint) => write!(f, "source joint {joint} is outside the mapping"),
            Self::Unsupported { joint, name: Some(name) } => write!(f, "source joint {joint} `{name}` has no supported destination"),
            Self::Unsupported { joint, name: None } => write!(f, "unnamed source joint {joint} has no supported destination"),
        }
    }
}

impl std::error::Error for BoneMapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NamedJoint { error, .. } => Some(error),
            _ => None,
        }
    }
}
