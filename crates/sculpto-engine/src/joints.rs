//! Joint articulation limits: every palette joint has a type, an axis in its
//! bone frame, and a range, read from the product data file `joints.toml`.
//!
//! The schema is the single owner of "where the body bends, about which axis,
//! and how far". Two consumers read it through this module: the
//! `actor joints` verb (pose and clip validation with numbers) and the viewer
//! pose mode (a dragged joint clamps to the same ranges through the report's
//! clamp table). Axes are measured on the Mixamo reference rig; limits are
//! cited anatomical values or explicitly marked envelopes — never invented
//! numbers dressed as anatomy.
//!
//! @req DECL0013

use std::collections::HashMap;
use std::sync::OnceLock;

use glam::{DMat3, DMat4, DQuat, DVec3};
use sculpto_mesh::actor::{Actor, Pose};

use crate::retarget::role_of;
use crate::{CommandError, errors};

/// The product data file, parsed once.
const JOINTS_TOML: &str = include_str!("joints.toml");

/// How one joint articulates. `root` and `fixed` carry no enforceable range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Articulation {
    /// One axis, one sign (elbow, knee, finger segments).
    Hinge,
    /// Three axes around a socket (shoulder, hip, head).
    Ball,
    /// Two dominant axes plus a small third (ankle, thumb base).
    Saddle,
    /// Two axes plus twist (wrist).
    Universal,
    /// 6-DOF base of the hierarchy; no limit by design.
    Root,
    /// Skinning terminal; any articulation is a rig artifact.
    Fixed,
}

impl Articulation {
    fn parse(value: &str) -> Result<Self, CommandError> {
        match value {
            "hinge" => Ok(Self::Hinge),
            "ball" => Ok(Self::Ball),
            "saddle" => Ok(Self::Saddle),
            "universal" => Ok(Self::Universal),
            "root" => Ok(Self::Root),
            "fixed" => Ok(Self::Fixed),
            other => Err(errors::invalid(
                "joints schema",
                format!("unknown articulation type {other:?}"),
            )),
        }
    }

    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Hinge => "hinge",
            Self::Ball => "ball",
            Self::Saddle => "saddle",
            Self::Universal => "universal",
            Self::Root => "root",
            Self::Fixed => "fixed",
        }
    }
}

/// Where a limit value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Cited anatomical value used directly.
    Anatomical,
    /// Conservative working bound derived from a cited complex total.
    Envelope,
    /// No defensible value: observed and reported, never refused. The note
    /// names the measurement that would settle it.
    Unresolved,
}

impl Basis {
    fn parse(value: &str) -> Result<Self, CommandError> {
        match value {
            "anatomical" => Ok(Self::Anatomical),
            "envelope" => Ok(Self::Envelope),
            "unresolved" => Ok(Self::Unresolved),
            other => Err(errors::invalid(
                "joints schema",
                format!("unknown limit basis {other:?}"),
            )),
        }
    }

    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Anatomical => "anatomical",
            Self::Envelope => "envelope",
            Self::Unresolved => "unresolved",
        }
    }
}

/// One named axis with its range. `axis` lives in the driven bone's own
/// rest-local frame (the Mixamo convention); `anchor` is the same direction as
/// a world vector in that reference rest, used to recover the axis on a
/// foreign palette resolved by role.
#[derive(Clone, Debug)]
pub struct AxisLimit {
    pub name: String,
    pub axis: DVec3,
    pub anchor: DVec3,
    pub min_deg: f64,
    pub max_deg: f64,
    pub basis: Basis,
    pub cite: String,
    pub note: String,
}

/// One joint's entry: identity, type, tolerance and axes.
#[derive(Clone, Debug)]
pub struct JointEntry {
    pub name: String,
    pub role: String,
    pub articulation: Articulation,
    pub slack_deg: f64,
    /// Whether the stored axes were measured on the reference rig.
    pub measured_axes: bool,
    pub note: String,
    pub axes: Vec<AxisLimit>,
}

/// The parsed schema with lookup by palette name and by role label.
pub struct JointSchema {
    pub entries: Vec<JointEntry>,
    by_name: HashMap<String, usize>,
    by_role: HashMap<String, usize>,
}

#[derive(serde::Deserialize)]
struct AxisToml {
    name: String,
    axis: [f64; 3],
    anchor: [f64; 3],
    min_deg: f64,
    max_deg: f64,
    basis: String,
    cite: String,
    #[serde(default)]
    note: String,
}

#[derive(serde::Deserialize)]
struct JointToml {
    name: String,
    role: String,
    #[serde(rename = "type")]
    articulation: String,
    slack_deg: f64,
    axis_basis: String,
    note: String,
    #[serde(default)]
    axis: Vec<AxisToml>,
}

#[derive(serde::Deserialize)]
struct SchemaToml {
    #[serde(default)]
    joint: Vec<JointToml>,
}

impl JointSchema {
    fn parse() -> Result<Self, CommandError> {
        let raw: SchemaToml = toml::from_str(JOINTS_TOML).map_err(|error| {
            errors::invalid(
                "joints schema",
                format!("joints.toml does not parse: {error}"),
            )
        })?;
        let mut entries = Vec::with_capacity(raw.joint.len());
        let mut by_name = HashMap::new();
        let mut by_role = HashMap::new();
        for joint in &raw.joint {
            let articulation = Articulation::parse(&joint.articulation)?;
            let mut axes = Vec::with_capacity(joint.axis.len());
            for axis in &joint.axis {
                let vector = DVec3::from(axis.axis);
                let anchor = DVec3::from(axis.anchor);
                if (vector.length() - 1.0).abs() > 1e-3 || (anchor.length() - 1.0).abs() > 1e-3 {
                    return Err(errors::invalid(
                        "joints schema",
                        format!("{} axis {:?} is not a unit vector", joint.name, axis.name),
                    ));
                }
                if axis.min_deg > axis.max_deg
                    || !axis.min_deg.is_finite()
                    || !axis.max_deg.is_finite()
                {
                    return Err(errors::invalid(
                        "joints schema",
                        format!(
                            "{} axis {:?}: invalid range [{}, {}]",
                            joint.name, axis.name, axis.min_deg, axis.max_deg
                        ),
                    ));
                }
                axes.push(AxisLimit {
                    name: axis.name.clone(),
                    axis: vector.normalize(),
                    anchor: anchor.normalize(),
                    min_deg: axis.min_deg,
                    max_deg: axis.max_deg,
                    basis: Basis::parse(&axis.basis)?,
                    cite: axis.cite.clone(),
                    note: axis.note.clone(),
                });
            }
            if matches!(articulation, Articulation::Hinge) && axes.is_empty() {
                return Err(errors::invalid(
                    "joints schema",
                    format!("{} is a hinge with no axis", joint.name),
                ));
            }
            let measured = match joint.axis_basis.as_str() {
                "measured" => true,
                "anatomical" => false,
                other => {
                    return Err(errors::invalid(
                        "joints schema",
                        format!("{} unknown axis basis {other:?}", joint.name),
                    ));
                }
            };
            if by_name.insert(joint.name.clone(), entries.len()).is_some() {
                return Err(errors::invalid(
                    "joints schema",
                    format!("duplicate joint {}", joint.name),
                ));
            }
            by_role.insert(joint.role.clone(), entries.len());
            entries.push(JointEntry {
                name: joint.name.clone(),
                role: joint.role.clone(),
                articulation,
                slack_deg: joint.slack_deg,
                measured_axes: measured,
                note: joint.note.clone(),
                axes,
            });
        }
        Ok(Self {
            entries,
            by_name,
            by_role,
        })
    }

    /// The schema index for a palette joint name, by name or by role label.
    pub fn entry_index(&self, joint_name: &str) -> Option<usize> {
        if let Some(index) = self.by_name.get(joint_name) {
            return Some(*index);
        }
        role_of(joint_name)
            .map(|role| role.label())
            .and_then(|label| self.by_role.get(&label))
            .copied()
    }

    /// One entry by schema index.
    pub fn entry(&self, index: usize) -> &JointEntry {
        &self.entries[index]
    }

    /// The entry matching a palette joint name, if any.
    pub fn entry_for(&self, joint_name: &str) -> Option<&JointEntry> {
        self.entry_index(joint_name).map(|index| self.entry(index))
    }

    /// Entry count, for completeness checks.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the schema is empty (always false for the shipped file).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The parsed product schema; a bad file refuses at load, never at use.
pub fn schema() -> Result<&'static JointSchema, CommandError> {
    static SCHEMA: OnceLock<Result<JointSchema, String>> = OnceLock::new();
    let inner = SCHEMA.get_or_init(|| JointSchema::parse().map_err(|error| error.to_string()));
    inner
        .as_ref()
        .map_err(|message| errors::invalid("joints schema", message.clone()))
}

/// Signed twist of a rotation about a unit axis, in degrees, in (-180, 180].
///
/// Swing-twist decomposition: the component of `rotation` around `axis`. The
/// caller supplies the axis in the same frame as the quaternion.
pub fn signed_twist(rotation: DQuat, axis: DVec3) -> f64 {
    let rotation = canonical(rotation);
    let projection = rotation.xyz().dot(axis);
    let degrees = 2.0 * projection.atan2(rotation.w).to_degrees();
    let degrees = degrees.rem_euclid(360.0);
    if degrees > 180.0 {
        degrees - 360.0
    } else {
        degrees
    }
}

/// Same hemisphere canonical form, so antipodal quaternions agree.
fn canonical(rotation: DQuat) -> DQuat {
    match rotation.w >= 0.0 {
        true => rotation,
        false => -rotation,
    }
}

/// Rotation angle of a quaternion, degrees, in [0, 180].
fn angle_degrees(rotation: DQuat) -> f64 {
    2.0 * canonical(rotation).w.acos().clamp(0.0, 1.0).to_degrees()
}

/// Goniometric tolerance: an excess within this is measured, not violated, and
/// stays visible in the row. Real refusals start beyond it.
const TOLERANCE_DEG: f64 = 1.0;

/// The unit-twist quaternion of `rotation` about `axis`.
fn twist_quaternion(rotation: DQuat, axis: DVec3) -> DQuat {
    let rotation = canonical(rotation);
    let projection = rotation.xyz().dot(axis);
    let twist = DQuat::from_xyzw(
        axis.x * projection,
        axis.y * projection,
        axis.z * projection,
        rotation.w,
    );
    match twist.length_squared() > 1e-18 {
        true => twist.normalize(),
        false => DQuat::IDENTITY,
    }
}

/// What a row means to a consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// Outside the range (or off-axis beyond slack): the defect this exists for.
    Violation,
    /// Inside the range; reported so "nothing exceeds" is provable, not silent.
    Ok,
    /// Informational only (root, fixed): no range is enforced.
    Info,
}

impl Severity {
    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Violation => "violation",
            Self::Ok => "ok",
            Self::Info => "info",
        }
    }
}

/// One axis measurement at one pose: value against the range, with excess.
#[derive(Clone, Debug)]
pub struct AxisRow {
    pub joint: String,
    pub node_name: String,
    pub axis: String,
    pub value_deg: f64,
    pub min_deg: f64,
    pub max_deg: f64,
    pub basis: Basis,
    pub excess_deg: f64,
    pub severity: Severity,
}

/// The unexplained part of a joint's articulation.
#[derive(Clone, Debug)]
pub struct ResidualRow {
    pub joint: String,
    pub node_name: String,
    pub residual_deg: f64,
    pub slack_deg: f64,
    pub severity: Severity,
}

/// One pose's full check: per-axis rows plus the off-axis residual.
#[derive(Clone, Debug)]
pub struct PoseCheck {
    pub rows: Vec<AxisRow>,
    /// Off-axis rotation the listed axes do not explain.
    pub residuals: Vec<ResidualRow>,
}

/// A node bound to its schema entry, with the twins it collapsed.
#[derive(Clone, Debug)]
struct NodeMatch<'a> {
    node: usize,
    node_name: String,
    entry: &'a JointEntry,
    matched_by: &'static str,
    collapsed: usize,
}

/// Parent indices for every node, from the document's child lists.
fn parents_of(actor: &Actor) -> Vec<Option<usize>> {
    let mut parents = vec![None; actor.document().nodes().len()];
    for node in actor.document().nodes() {
        for child in node.children() {
            parents[child.index()] = Some(node.index());
        }
    }
    parents
}

/// Local rotation of every node from world rotations: parent inverse times world.
fn local_rotations(actor: &Actor, worlds: &[DQuat]) -> Vec<DQuat> {
    let parents = parents_of(actor);
    (0..worlds.len())
        .map(|node| match parents[node] {
            Some(parent) => worlds[parent].inverse() * worlds[node],
            None => worlds[node],
        })
        .collect()
}

/// World rotations of every node at one pose.
fn world_rotations(actor: &Actor, pose: Pose<'_>) -> Result<Vec<DQuat>, CommandError> {
    let worlds = actor
        .skeleton_worlds(pose)
        .map_err(|error| pose_error(pose, error))?;
    Ok(worlds
        .iter()
        .map(|world| world.to_scale_rotation_translation().1)
        .collect())
}

fn pose_error(pose: Pose<'_>, error: sculpto_mesh::actor::ActorError) -> CommandError {
    let _ = pose;
    CommandError::new(
        crate::ErrorKind::InvalidParams,
        "actor joints",
        format!("the pose cannot be evaluated: {error}"),
    )
}

/// Resolve the actor's joints onto schema entries; twins collapse with a count.
fn resolve<'a>(actor: &Actor, schema: &'a JointSchema) -> Vec<NodeMatch<'a>> {
    let mut matches: Vec<NodeMatch<'a>> = Vec::new();
    let mut by_entry: HashMap<usize, usize> = HashMap::new();
    for node in actor.document().nodes() {
        let Some(name) = node.name() else {
            continue;
        };
        let Some(index) = schema.entry_index(name) else {
            continue;
        };
        let entry = schema.entry(index);
        let matched_by = if entry.name == name { "name" } else { "role" };
        let row = NodeMatch {
            node: node.index(),
            node_name: name.to_owned(),
            entry,
            matched_by,
            collapsed: 1,
        };
        match by_entry.get(&index) {
            Some(row_index) => matches[*row_index].collapsed += 1,
            None => {
                by_entry.insert(index, matches.len());
                matches.push(row);
            }
        }
    }
    matches
}

/// Which world side the actor's "Left" is on: Mixamo's `+x`, or the mirrored
/// `−x` the native bodies carry. The schema's anchors are Mixamo-convention, so
/// a mirrored actor reflects them; an undetectable side is reported, not
/// guessed silently.
fn left_at_plus_x(matches: &[NodeMatch<'_>], worlds: &[DMat4]) -> Option<bool> {
    for (left, right) in [
        ("L thigh", "R thigh"),
        ("L upperarm", "R upperarm"),
        ("L clavicle", "R clavicle"),
        ("L calf", "R calf"),
    ] {
        let left = matches.iter().find(|row| row.entry.role == left);
        let right = matches.iter().find(|row| row.entry.role == right);
        if let (Some(left), Some(right)) = (left, right) {
            let (left_x, right_x) = (worlds[left.node].w_axis.x, worlds[right.node].w_axis.x);
            if (left_x - right_x).abs() > 1e-9 {
                return Some(left_x > right_x);
            }
        }
    }
    None
}

/// The schema axis recovered in the actor's own rest-local frame through the
/// world anchor, reflected when the actor carries its left on the other side.
fn actor_axis(axis: &AxisLimit, rest_world_rotation: DQuat, mirror: Option<bool>) -> (DVec3, f64) {
    let mut anchor = axis.anchor;
    if matches!(mirror, Some(false)) {
        anchor.x = -anchor.x;
    }
    let anchored = rest_world_rotation.inverse() * anchor;
    let anchored = anchored.normalize_or_zero();
    let agreement = axis.axis.dot(anchored).abs().acos().to_degrees().min(180.0);
    (anchored, agreement)
}

/// Check one pose (rest or one clip time) against the schema.
pub fn check_pose(actor: &Actor, pose: Pose<'_>) -> Result<PoseCheck, CommandError> {
    let schema = schema()?;
    let matches = resolve(actor, schema);
    let rest_worlds = world_rotations(actor, Pose::Rest)?;
    let rest = local_rotations(actor, &rest_worlds);
    let posed = local_rotations(actor, &world_rotations(actor, pose)?);
    let mirror = left_at_plus_x(
        &matches,
        &actor
            .skeleton_worlds(Pose::Rest)
            .map_err(|error| pose_error(pose, error))?,
    );

    let mut rows = Vec::new();
    let mut residuals = Vec::new();
    for matched in &matches {
        let entry = matched.entry;
        let node = matched.node;
        let delta = canonical(rest[node].inverse() * posed[node]);
        let total = angle_degrees(delta);
        match entry.articulation {
            Articulation::Root | Articulation::Fixed => {
                if total > 0.5 {
                    rows.push(AxisRow {
                        joint: entry.name.clone(),
                        node_name: matched.node_name.clone(),
                        axis: match entry.articulation {
                            Articulation::Root => "6-dof".to_owned(),
                            _ => "fixed".to_owned(),
                        },
                        value_deg: total,
                        min_deg: 0.0,
                        max_deg: 0.0,
                        basis: Basis::Envelope,
                        excess_deg: 0.0,
                        severity: Severity::Info,
                    });
                }
                continue;
            }
            _ => {}
        }
        let mut composed = DQuat::IDENTITY;
        for axis in &entry.axes {
            let (axis_local, _agreement) = actor_axis(axis, rest_worlds[node], mirror);
            let value = signed_twist(delta, axis_local);
            composed = twist_quaternion(delta, axis_local) * composed;
            let (excess, severity) = if axis.basis == Basis::Unresolved {
                (0.0, Severity::Info)
            } else if value < axis.min_deg - TOLERANCE_DEG {
                (axis.min_deg - value, Severity::Violation)
            } else if value > axis.max_deg + TOLERANCE_DEG {
                (value - axis.max_deg, Severity::Violation)
            } else {
                (0.0, Severity::Ok)
            };
            rows.push(AxisRow {
                joint: entry.name.clone(),
                node_name: matched.node_name.clone(),
                axis: axis.name.clone(),
                value_deg: value,
                min_deg: axis.min_deg,
                max_deg: axis.max_deg,
                basis: axis.basis,
                excess_deg: excess,
                severity,
            });
        }
        // The order-residual is only a hinge test: for multi-axis joints the
        // composed-twist order leaves degrees that legitimate combined motion
        // explains, so those read as information, not refusal.
        let residual = angle_degrees(delta * composed.inverse());
        let hinge = entry.articulation == Articulation::Hinge;
        residuals.push(ResidualRow {
            joint: entry.name.clone(),
            node_name: matched.node_name.clone(),
            residual_deg: residual,
            slack_deg: entry.slack_deg,
            severity: match hinge && residual > entry.slack_deg + TOLERANCE_DEG {
                true => Severity::Violation,
                false => Severity::Ok,
            },
        });
    }
    Ok(PoseCheck { rows, residuals })
}

/// One joint's swept measurement over a clip.
#[derive(Clone, Debug)]
pub struct SweepRow {
    pub joint: String,
    pub node_name: String,
    pub axis: String,
    pub min_deg: f64,
    pub max_deg: f64,
    pub observed_min_deg: f64,
    pub observed_max_deg: f64,
    pub worst_deg: f64,
    pub worst_time: f64,
    pub excess_deg: f64,
    pub basis: Basis,
    pub severity: Severity,
}

/// Check every sampled frame of a clip; the worst value per axis is the verdict.
pub fn check_clip(
    actor: &Actor,
    clip: &str,
    frames: usize,
) -> Result<(Vec<SweepRow>, f64, f64), CommandError> {
    let frames = frames.max(2);
    let (start, end) = actor.clip_span(clip).map_err(|error| {
        CommandError::new(
            crate::ErrorKind::InvalidParams,
            "actor joints",
            format!("the clip cannot be spanned: {error}"),
        )
    })?;
    struct Slot {
        joint: String,
        node_name: String,
        axis: String,
        observed_min: f64,
        observed_max: f64,
        worst_excess: f64,
        worst_value: f64,
        worst_time: f64,
    }
    let mut per_axis: Vec<Slot> = Vec::new();
    let mut per_residual: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for index in 0..frames {
        let time = start + (end - start) * index as f64 / (frames - 1) as f64;
        let check = check_pose(actor, Pose::Clip { name: clip, time })?;
        for row in &check.rows {
            if row.severity == Severity::Info {
                continue;
            }
            let found = per_axis
                .iter_mut()
                .find(|slot| slot.joint == row.joint && slot.axis == row.axis);
            match found {
                Some(slot) => {
                    slot.observed_min = slot.observed_min.min(row.value_deg);
                    slot.observed_max = slot.observed_max.max(row.value_deg);
                    if row.excess_deg >= slot.worst_excess {
                        slot.worst_excess = row.excess_deg;
                        slot.worst_value = row.value_deg;
                        slot.worst_time = time;
                    }
                }
                None => per_axis.push(Slot {
                    joint: row.joint.clone(),
                    node_name: row.node_name.clone(),
                    axis: row.axis.clone(),
                    observed_min: row.value_deg,
                    observed_max: row.value_deg,
                    worst_excess: row.excess_deg,
                    worst_value: row.value_deg,
                    worst_time: time,
                }),
            }
        }
        for residual in &check.residuals {
            let key = (residual.joint.clone(), residual.node_name.clone());
            let entry = per_residual.entry(key).or_insert((0.0, residual.slack_deg));
            if residual.residual_deg > entry.0 {
                *entry = (residual.residual_deg, residual.slack_deg);
            }
        }
    }
    let schema = schema()?;
    let mut sweeps = Vec::with_capacity(per_axis.len());
    for slot in per_axis {
        let entry = schema
            .entry_for(&slot.joint)
            .expect("resolved rows come from the schema");
        let axis = entry
            .axes
            .iter()
            .find(|candidate| candidate.name == slot.axis)
            .expect("rows name schema axes");
        sweeps.push(SweepRow {
            joint: slot.joint,
            node_name: slot.node_name,
            axis: slot.axis,
            min_deg: axis.min_deg,
            max_deg: axis.max_deg,
            observed_min_deg: slot.observed_min,
            observed_max_deg: slot.observed_max,
            worst_deg: slot.worst_value,
            worst_time: slot.worst_time,
            excess_deg: slot.worst_excess,
            basis: axis.basis,
            severity: match slot.worst_excess > 0.0 {
                true => Severity::Violation,
                false => Severity::Ok,
            },
        });
    }
    for ((joint, node_name), (residual, slack)) in per_residual {
        let hinge = schema
            .entry_for(&joint)
            .map(|entry| entry.articulation == Articulation::Hinge)
            .unwrap_or(false);
        let excess = (residual - slack).max(0.0);
        if excess > 0.0 {
            sweeps.push(SweepRow {
                joint,
                node_name,
                axis: "off-axis".to_owned(),
                min_deg: -slack,
                max_deg: slack,
                observed_min_deg: -residual,
                observed_max_deg: residual,
                worst_deg: residual,
                worst_time: f64::NAN,
                excess_deg: excess,
                basis: Basis::Envelope,
                severity: match hinge && excess > TOLERANCE_DEG {
                    true => Severity::Violation,
                    false => Severity::Info,
                },
            });
        }
    }
    sweeps.sort_by(|a, b| {
        b.excess_deg
            .partial_cmp(&a.excess_deg)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.joint.cmp(&b.joint))
    });
    Ok((sweeps, start, end))
}

/// A clamp row for the viewer pose mode: the same ranges, in this actor's own
/// node frame, with the node's rest rotation so a dragged delta composes.
pub fn clamp_table(actor: &Actor) -> Result<Vec<serde_json::Value>, CommandError> {
    let schema = schema()?;
    let matches = resolve(actor, schema);
    let rest_worlds = world_rotations(actor, Pose::Rest)?;
    let world_matrices = actor
        .skeleton_worlds(Pose::Rest)
        .map_err(|error| pose_error(Pose::Rest, error))?;
    let mirror = left_at_plus_x(&matches, &world_matrices);
    let parents = parents_of(actor);
    let mut rows = Vec::with_capacity(matches.len());
    for matched in &matches {
        if matched.entry.axes.is_empty() {
            continue;
        }
        let axes: Vec<serde_json::Value> = matched
            .entry
            .axes
            .iter()
            .map(|axis| {
                let (axis_local, _agreement) = actor_axis(axis, rest_worlds[matched.node], mirror);
                serde_json::json!({
                    "name": axis.name,
                    "axis_local": [axis_local.x, axis_local.y, axis_local.z],
                    "min_deg": axis.min_deg,
                    "max_deg": axis.max_deg,
                })
            })
            .collect();
        let rest_local = match parents[matched.node] {
            Some(parent) => rest_worlds[parent].inverse() * rest_worlds[matched.node],
            None => rest_worlds[matched.node],
        };
        rows.push(serde_json::json!({
            "joint": matched.entry.name,
            "role": matched.entry.role,
            "node": matched.node,
            "node_name": matched.node_name,
            "matched_by": matched.matched_by,
            "collapsed_nodes": matched.collapsed,
            "type": matched.entry.articulation.token(),
            "slack_deg": matched.entry.slack_deg,
            "axes": axes,
            "rest_rotation_xyzw": [rest_local.x, rest_local.y, rest_local.z, rest_local.w],
            "left_at_plus_x": mirror,
        }));
    }
    Ok(rows)
}

/// Which side the actor carries its "Left" on: `Some(true)` = `+x` (the Mixamo
/// convention the schema anchors use), `Some(false)` = the mirrored native
/// bodies, `None` = undetectable from a left/right pivot pair.
pub fn left_convention(actor: &Actor) -> Result<Option<bool>, CommandError> {
    let schema = schema()?;
    let matches = resolve(actor, schema);
    let worlds = actor
        .skeleton_worlds(Pose::Rest)
        .map_err(|error| pose_error(Pose::Rest, error))?;
    Ok(left_at_plus_x(&matches, &worlds))
}

/// One survey row: the flesh's crease axis against the schema axis, and the
/// medial centre against the pivot, at rest.
#[derive(Clone, Debug)]
pub struct SurveyRow {
    pub joint: String,
    pub node_name: String,
    pub schema_axis_world: [f64; 3],
    pub measured_axis_world: [f64; 3],
    pub angle_deg: f64,
    pub pivot_to_centre_mm: f64,
    pub vertices: usize,
}

/// Measure the flesh at every hinge, ball and saddle joint: the crease is the
/// largest spread of the slab around the pivot, perpendicular to the bone; the
/// medial centre is the slab centroid. The crease is unsigned — a crease has
/// no preferred sign.
pub fn survey(
    actor: &Actor,
    parts: &[&str],
    radius_mm: f64,
) -> Result<Vec<SurveyRow>, CommandError> {
    let schema = schema()?;
    let sample = actor.sample(parts, Pose::Rest).map_err(|error| {
        CommandError::new(
            crate::ErrorKind::InvalidParams,
            "actor joints",
            format!("the rest body cannot be sampled: {error}"),
        )
    })?;
    let worlds = actor.skeleton_worlds(Pose::Rest).map_err(|error| {
        CommandError::new(
            crate::ErrorKind::InvalidParams,
            "actor joints",
            format!("the rest skeleton cannot be composed: {error}"),
        )
    })?;
    let parents = parents_of(actor);
    let children = |node: usize| -> Vec<usize> {
        parents
            .iter()
            .enumerate()
            .filter(|(_candidate, parent)| **parent == Some(node))
            .map(|(candidate, _)| candidate)
            .collect()
    };
    let radius = radius_mm / 1000.0;
    let mirror = left_at_plus_x(&resolve(actor, schema), &worlds);
    let mut rows = Vec::new();
    for matched in resolve(actor, schema) {
        if !matches!(
            matched.entry.articulation,
            Articulation::Hinge | Articulation::Ball | Articulation::Saddle
        ) {
            continue;
        }
        let world = worlds[matched.node];
        let pivot = world.w_axis.truncate();
        let bone = children(matched.node)
            .first()
            .map(|child| worlds[*child].w_axis.truncate() - pivot)
            .map(|direction| direction.normalize_or_zero())
            .unwrap_or(DVec3::Y);
        let slab: Vec<DVec3> = sample
            .vertices
            .iter()
            .map(|vertex| DVec3::from(*vertex))
            .filter(|vertex| vertex.distance(pivot) < radius)
            .collect::<Vec<_>>();
        if slab.len() < 16 {
            continue;
        }
        let centroid = slab.iter().sum::<DVec3>() / slab.len() as f64;
        let mut spread = DMat3::ZERO;
        for vertex in &slab {
            let offset = *vertex - centroid;
            spread += DMat3::from_cols(offset * offset.x, offset * offset.y, offset * offset.z);
        }
        let spread = spread * (1.0 / slab.len() as f64);
        // Power iteration for the dominant spread, started off the bone.
        let mut axis = bone.cross(DVec3::X);
        if axis.length_squared() < 1e-6 {
            axis = bone.cross(DVec3::Y);
        }
        axis = axis.normalize_or_zero();
        for _ in 0..32 {
            let next = spread * axis;
            if next.length_squared() < 1e-12 {
                break;
            }
            axis = next.normalize_or_zero();
        }
        let axis = (axis - bone * axis.dot(bone)).normalize_or_zero();
        let schema_axis_world = matched
            .entry
            .axes
            .first()
            .map(|axis| {
                let mut anchor = axis.anchor;
                if matches!(mirror, Some(false)) {
                    anchor.x = -anchor.x;
                }
                world.transform_vector3(anchor).normalize_or_zero()
            })
            .unwrap_or(DVec3::ZERO);
        let angle = axis.dot(schema_axis_world).abs().acos().to_degrees();
        rows.push(SurveyRow {
            joint: matched.entry.name.clone(),
            node_name: matched.node_name,
            schema_axis_world: schema_axis_world.to_array(),
            measured_axis_world: axis.to_array(),
            angle_deg: angle,
            pivot_to_centre_mm: pivot.distance(centroid) * 1000.0,
            vertices: slab.len(),
        });
    }
    rows.sort_by(|a, b| {
        b.angle_deg
            .partial_cmp(&a.angle_deg)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}
