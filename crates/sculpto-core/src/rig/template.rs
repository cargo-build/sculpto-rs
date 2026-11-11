//! Recover the parent chains of an authored body rig from an explicit template.
//! @req DECL0008

use std::collections::BTreeMap;
use std::fmt;

use glam::{DMat4, DVec3};

use super::rest::REST_TOLERANCE;
use super::{JointId, LocalTransform, Rig, RigError, RigFrame};

/// Explicit pairing of the authored and family template scene roots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateRoots {
    /// Scene root to retain from the authored rig.
    pub authored: JointId,
    /// Scene root of the supplied family skeleton.
    pub template: JointId,
}

/// Recover missing family joints while retaining the authored rest worlds.
///
/// The authored graph stays intact. The returned rig keeps its joint IDs and
/// names. A flattened joint's local rest is recomputed under the template
/// parent. Template-only joints carry their template locals. @req DECL0008
///
/// # Errors
/// Refuses incompatible frames, roots, ambiguous names, conflicting authored
/// hierarchy, and unrepresentable transforms or rest drift.
pub fn complete_family_rig(
    authored: &Rig,
    template: &Rig,
    roots: TemplateRoots,
) -> Result<Rig, TemplateError> {
    if authored.frame() != template.frame() {
        return Err(TemplateError::Frame {
            authored: authored.frame(),
            template: template.frame(),
        });
    }
    if authored.roots() != [roots.authored] || template.roots() != [roots.template] {
        return Err(TemplateError::Roots);
    }
    let names = index_names(authored, "authored")?;
    let template_names = index_names(template, "template")?;
    validate_root_alias(authored, template, roots, &names, &template_names)?;

    let mut joints = authored.joints().to_vec();
    let mut mapped = vec![None; template.joints().len()];
    mapped[roots.template.index()] = Some(roots.authored);
    let mut worlds = vec![None; joints.len()];
    worlds[roots.authored.index()] = Some(rest_world(authored, roots.authored)?);
    let mut shared = 0;

    for &template_id in template.topological_order() {
        if template_id == roots.template {
            continue;
        }
        let node = &template.joints()[template_id.index()];
        let parent = node
            .parent
            .and_then(|parent| mapped[parent.index()])
            .ok_or(TemplateError::Roots)?;
        let parent_world = worlds[parent.index()].ok_or(TemplateError::InvalidRest { joint: parent })?;
        let matched = node.name.as_deref().and_then(|name| names.get(name)).copied();
        let (id, local) = match matched {
            Some(id) => {
                let original = &mut joints[id.index()];
                let local = match original.parent {
                    Some(old_parent) if old_parent == parent => original.rest,
                    Some(old_parent) if old_parent == roots.authored => {
                        reparent_rest(authored, id, parent_world)?
                    }
                    old_parent => return Err(TemplateError::ParentConflict { joint: id, authored: old_parent, template: parent }),
                };
                original.parent = Some(parent);
                original.rest = local;
                shared += 1;
                (id, local)
            }
            None => {
                let id = JointId::new(u32::try_from(joints.len()).map_err(|_| TemplateError::TooManyJoints)?);
                let mut joint = node.clone();
                joint.parent = Some(parent);
                joints.push(joint);
                worlds.push(None);
                (id, node.rest)
            }
        };
        let world = parent_world * local.compose();
        if !world.is_finite() {
            return Err(TemplateError::InvalidRest { joint: id });
        }
        mapped[template_id.index()] = Some(id);
        worlds[id.index()] = Some(world);
    }
    if shared == 0 {
        return Err(TemplateError::NoSharedJoints);
    }
    let completed = Rig::new(joints, authored.frame()).map_err(TemplateError::Rig)?;
    verify_authored_worlds(authored, &completed)?;
    Ok(completed)
}

fn index_names<'a>(
    rig: &'a Rig,
    side: &'static str,
) -> Result<BTreeMap<&'a str, JointId>, TemplateError> {
    let mut names = BTreeMap::new();
    for (index, node) in rig.joints().iter().enumerate() {
        if let Some(name) = node.name.as_deref()
            && names.insert(name, JointId::new(index as u32)).is_some()
        {
            return Err(TemplateError::DuplicateName { side, name: name.into() });
        }
    }
    Ok(names)
}

fn validate_root_alias(
    authored: &Rig,
    template: &Rig,
    roots: TemplateRoots,
    names: &BTreeMap<&str, JointId>,
    template_names: &BTreeMap<&str, JointId>,
) -> Result<(), TemplateError> {
    let authored_name = authored.joints()[roots.authored.index()].name.as_deref();
    let template_name = template.joints()[roots.template.index()].name.as_deref();
    if authored_name
        .and_then(|name| template_names.get(name))
        .is_some_and(|id| *id != roots.template)
        || template_name
            .and_then(|name| names.get(name))
            .is_some_and(|id| *id != roots.authored)
    {
        return Err(TemplateError::RootAlias);
    }
    Ok(())
}

fn rest_world(rig: &Rig, joint: JointId) -> Result<DMat4, TemplateError> {
    let world = DMat4::from_cols_array_2d(&rig.rest_world(joint).map_err(TemplateError::Rig)?);
    if !world.is_finite() {
        return Err(TemplateError::InvalidRest { joint });
    }
    Ok(world)
}

fn reparent_rest(authored: &Rig, joint: JointId, parent: DMat4) -> Result<LocalTransform, TemplateError> {
    let parent_determinant = parent.determinant();
    if !parent_determinant.is_finite() || parent_determinant <= 0.0 {
        return Err(TemplateError::InvalidRest { joint });
    }
    let local = parent.inverse() * rest_world(authored, joint)?;
    let determinant = local.determinant();
    if !local.is_finite() || !determinant.is_finite() || determinant <= 0.0 {
        return Err(TemplateError::InvalidRest { joint });
    }
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    let local = LocalTransform {
        scale: scale.x as f32,
        rotation: rotation.to_array().map(|value| value as f32),
        translation: translation.to_array().map(|value| value as f32),
    };
    local.validate().map_err(|_| TemplateError::InvalidRest { joint })?;
    Ok(local)
}

fn verify_authored_worlds(authored: &Rig, completed: &Rig) -> Result<(), TemplateError> {
    for &joint in authored.topological_order() {
        let source = rest_world(authored, joint)?;
        let resolved = rest_world(completed, joint)?;
        let error = [DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::Z]
            .into_iter()
            .map(|point| source.transform_point3(point).distance(resolved.transform_point3(point)))
            .try_fold(0.0_f64, |max, error| {
                if error.is_finite() {
                    Ok(max.max(error))
                } else {
                    Err(TemplateError::InvalidRest { joint })
                }
            })?;
        if error > REST_TOLERANCE {
            return Err(TemplateError::RestMismatch { joint, error });
        }
    }
    Ok(())
}

/// Why a family template cannot complete the authored rig.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateError {
    /// Rig frames differ.
    Frame { authored: RigFrame, template: RigFrame },
    /// Each supplied root must be the graph's sole scene root.
    Roots,
    /// A scene root name collides with a different joint in the other rig.
    RootAlias,
    /// Joint names are ambiguous within one source.
    DuplicateName { side: &'static str, name: String },
    /// No non-root names match between the two graphs.
    NoSharedJoints,
    /// A known non-root parent disagrees with the template.
    ParentConflict { joint: JointId, authored: Option<JointId>, template: JointId },
    /// The result exceeds the joint ID space.
    TooManyJoints,
    /// A rest transform cannot be represented.
    InvalidRest { joint: JointId },
    /// An authored joint would move at rest.
    RestMismatch { joint: JointId, error: f64 },
    /// The neutral rig rejected the completed graph.
    Rig(RigError),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frame { authored, template } => write!(formatter, "authored frame {authored:?} differs from template frame {template:?}"),
            Self::Roots => write!(formatter, "expected one explicitly paired scene root in each rig"),
            Self::RootAlias => write!(formatter, "a scene root name collides with a non-root joint"),
            Self::DuplicateName { side, name } => write!(formatter, "{side} rig repeats joint name `{name}`"),
            Self::NoSharedJoints => write!(formatter, "family template has no matching body joint names"),
            Self::ParentConflict { joint, authored, template } => write!(formatter, "joint {joint} has authored parent {authored:?} and template parent {template}"),
            Self::TooManyJoints => write!(formatter, "completed rig exceeds the joint ID range"),
            Self::InvalidRest { joint } => write!(formatter, "joint {joint} has an unrepresentable rest transform"),
            Self::RestMismatch { joint, error } => write!(formatter, "joint {joint} rest drift {error:.6e} exceeds {REST_TOLERANCE:.1e}"),
            Self::Rig(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TemplateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rig(error) => Some(error),
            _ => None,
        }
    }
}
