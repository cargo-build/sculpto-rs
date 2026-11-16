//! Source-skeleton-agnostic correspondence: which palette bone a clip authored
//! on *another* skeleton drives.
//!
//! A clip names its bones in its own convention (`mixamorig:LeftArm`, `NPC L
//! UpperArm [LUar]`, `upper_arm.L`, `Bone.042`). The palette names them in SSE's.
//! This module resolves one onto the other in three passes and reports which one
//! did the work:
//!
//! 1. **name convention** — the source bone's compact name resolves to the same
//!    anatomical role the palette bone carries ([`BoneRole`]);
//! 2. **hierarchy** — a palette bone with no named source takes the unclaimed
//!    source descendant whose rest direction best continues its parent's;
//! 3. **rest geometry** — what is left is matched by rest position after the one
//!    frame/unit similarity fitted on the bones pass 1 already paired.
//!
//! A bone no pass resolves is reported as unmatched and held at rest; a role two
//! source bones both claim is reported as ambiguous and resolved by a stated
//! rule, never silently.
//!
//! Palette slots are rig joint ids: the caller builds the rig from the palette in
//! palette order, so `JointId::new(slot)` is the palette bone. @req DECL0013

use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DMat4, DQuat, DVec3};
use serde_json::json;

use sculpto_core::rig::{JointId, Rig};
use sculpto_mesh::actor::Actor;

use crate::export_glb::actor_error;
use crate::{CommandError, errors};

/// Which side of the body a role belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// Left side.
    Left,
    /// Right side.
    Right,
    /// On the midline, or a role that carries no side.
    Centre,
}

/// One anatomical function a palette bone and a source bone can both name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// The root the whole body hangs from.
    Pelvis,
    /// One spine segment, `0` nearest the pelvis.
    Spine(u8),
    /// Neck.
    Neck,
    /// Head.
    Head,
    /// Clavicle.
    Clavicle,
    /// Upper arm.
    UpperArm,
    /// Forearm.
    Forearm,
    /// Hand.
    Hand,
    /// Thigh.
    Thigh,
    /// Calf.
    Calf,
    /// Foot.
    Foot,
    /// Toe base.
    Toe,
    /// One finger segment: `digit` 0 = thumb … 4 = little, `segment` 0 = proximal.
    Digit {
        /// Digit index, thumb first.
        digit: u8,
        /// Segment index, proximal first.
        segment: u8,
    },
    /// A twist helper between two bones of the arm.
    Twist {
        /// `true` for the forearm pair, `false` for the upper-arm pair.
        forearm: bool,
        /// `1` or `2`, parent side first.
        index: u8,
    },
    /// Soft tissue that no source skeleton of this family drives.
    Soft,
    /// Scaffolding a skeleton carries but no palette bone answers.
    Scaffold,
    /// A leaf marker (`HeadTop_End`, `Toe_End`), not a bone.
    Marker,
}

/// One anatomical role with its side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoneRole {
    /// Side the role belongs to.
    pub side: Side,
    /// Anatomical function.
    pub part: Part,
}

impl BoneRole {
    const fn new(side: Side, part: Part) -> Self {
        Self { side, part }
    }

    /// Whether the role drives the body's own motion, as opposed to soft tissue
    /// or scaffolding: only these are worth a fallback match.
    fn is_anatomical(self) -> bool {
        matches!(
            self.part,
            Part::Pelvis
                | Part::Spine(_)
                | Part::Neck
                | Part::Head
                | Part::Clavicle
                | Part::UpperArm
                | Part::Forearm
                | Part::Hand
                | Part::Thigh
                | Part::Calf
                | Part::Foot
                | Part::Toe
                | Part::Digit { .. }
        )
    }

    /// Stable report label, e.g. `L upperarm`, `R digit1.2`, `pelvis`.
    pub fn label(self) -> String {
        let side = match self.side {
            Side::Left => "L ",
            Side::Right => "R ",
            Side::Centre => "",
        };
        let part = match self.part {
            Part::Pelvis => "pelvis".to_owned(),
            Part::Spine(segment) => format!("spine{segment}"),
            Part::Neck => "neck".to_owned(),
            Part::Head => "head".to_owned(),
            Part::Clavicle => "clavicle".to_owned(),
            Part::UpperArm => "upperarm".to_owned(),
            Part::Forearm => "forearm".to_owned(),
            Part::Hand => "hand".to_owned(),
            Part::Thigh => "thigh".to_owned(),
            Part::Calf => "calf".to_owned(),
            Part::Foot => "foot".to_owned(),
            Part::Toe => "toe".to_owned(),
            Part::Digit { digit, segment } => format!("digit{digit}.{segment}"),
            Part::Twist { forearm, index } => format!(
                "{}twist{index}",
                match forearm {
                    true => "forearm",
                    false => "upperarm",
                }
            ),
            Part::Soft => "soft".to_owned(),
            Part::Scaffold => "scaffold".to_owned(),
            Part::Marker => "marker".to_owned(),
        };
        format!("{side}{part}")
    }
}

/// How one palette bone found its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Method {
    /// The source bone carries the palette's own name.
    Name,
    /// The source bone names the same role in another convention.
    Convention,
    /// Resolved by where the bone sits in the chain.
    Hierarchy,
    /// Resolved by rest position after the fitted similarity.
    Geometry,
    /// A twist helper the palette derives from its parent and child.
    Derived,
    /// Soft tissue or scaffolding: held at rest, deliberately.
    Rest,
    /// No source at all: reported, never guessed.
    Unmatched,
}

impl Method {
    /// Stable report token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Convention => "convention",
            Self::Hierarchy => "hierarchy",
            Self::Geometry => "geometry",
            Self::Derived => "derived",
            Self::Rest => "rest",
            Self::Unmatched => "unmatched",
        }
    }
}

/// One palette bone and the source bone that drives it.
#[derive(Clone, Debug)]
pub struct Pair {
    /// Palette slot.
    pub slot: usize,
    /// The rig joint that slot names.
    pub joint: JointId,
    /// Palette bone name.
    pub palette: String,
    /// Palette role, when the name carries one.
    pub role: Option<BoneRole>,
    /// Source node driving the bone.
    pub source: Option<usize>,
    /// Source node name.
    pub source_name: Option<String>,
    /// How the pair was found.
    pub method: Method,
    /// Extra source nodes whose local rotation is folded into this bone: the
    /// fourth finger segment, which the palette does not carry.
    pub fold: Vec<usize>,
    /// Rest orientation difference after the fitted frame, degrees.
    pub rest_angle_degrees: f64,
    /// Rest position residual after the fitted similarity, relative to extent.
    pub rest_offset: f64,
}

/// One source bone that drives nothing, with the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnmatchedSource {
    /// Node index.
    pub node: usize,
    /// Node name.
    pub name: String,
    /// Role the name resolves to, when it resolves at all.
    pub role: Option<String>,
    /// Why it drives nothing.
    pub reason: String,
}

/// One role that more than one source bone claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ambiguity {
    /// The contested role.
    pub role: String,
    /// Every claiming source bone.
    pub candidates: Vec<String>,
    /// The one kept.
    pub chosen: String,
    /// The rule that chose it.
    pub rule: String,
}

/// The fitted similarity between the source's world and the palette's.
#[derive(Clone, Copy, Debug)]
pub struct Similarity {
    /// Rotation taking a source world direction onto the palette's.
    pub rotation: DMat3,
    /// Palette units per source world unit.
    pub unit: f64,
    /// RMS position residual relative to the skeleton extent: how well the two
    /// rests are one body in another unit, which they are not across bodies.
    pub residual: f64,
    /// Bones the fit used.
    pub bones: usize,
}

/// A resolved source-to-palette correspondence.
#[derive(Clone, Debug)]
pub struct Correspondence {
    /// One row per palette bone, in palette order.
    pub pairs: Vec<Pair>,
    /// The fitted similarity.
    pub similarity: Similarity,
    /// Source bones that drive nothing.
    pub unmatched: Vec<UnmatchedSource>,
    /// Contested roles.
    pub ambiguous: Vec<Ambiguity>,
    /// Source leaf markers that are not bones.
    pub markers: Vec<String>,
}

/// Lower-case a bone name to letters and digits, dropping separators and the
/// import prefixes (`mixamorig:`, `mixamorig1:`, `Armature|`, `NPC `, `CME `).
pub fn compact(name: &str) -> String {
    let tail = name.rsplit('|').next().unwrap_or(name);
    // SSE writes a short name in brackets (`NPC L Foot [Lft ]`); it is a
    // comment on the bone, not a second name.
    let tail = match tail.find(" [") {
        Some(position) => &tail[..position],
        None => tail,
    };
    let lowered: String = tail
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let lowered = match lowered.starts_with("mixamorig") {
        true => lowered
            .trim_start_matches("mixamorig")
            .trim_start_matches(|character: char| character.is_ascii_digit())
            .to_owned(),
        false => lowered,
    };
    match lowered.strip_prefix("npc") {
        Some(rest) => rest.to_owned(),
        None => match lowered.strip_prefix("cme") {
            Some(rest) => rest.to_owned(),
            None => lowered,
        },
    }
}

/// Split a compact name into its letter stem and trailing digits, if any.
fn stem_digits(value: &str) -> (&str, Option<&str>) {
    let split = value
        .rfind(|character: char| !character.is_ascii_digit())
        .map_or(0, |position| position + 1);
    let digits = &value[split..];
    (
        &value[..split],
        Some(digits).filter(|digits| !digits.is_empty()),
    )
}

fn number(digits: Option<&str>) -> Option<u8> {
    digits.and_then(|digits| digits.parse().ok())
}

/// The anatomical role one bone name carries, in any convention the product
/// reads: Mixamo, SSE, Blender `upper_arm.L`, or a bare humanoid name.
pub fn role_of(name: &str) -> Option<BoneRole> {
    let compact = compact(name);
    if compact.is_empty() {
        return None;
    }
    if let Some(role) = role_compact(&compact) {
        return Some(role);
    }
    // `upper_arm.L`, `thigh_R`: some conventions put the side at the end.
    if compact.len() > 2 {
        let (head, last) = compact.split_at(compact.len() - 1);
        let side = match last {
            "l" => Some(Side::Left),
            "r" => Some(Side::Right),
            _ => None,
        };
        if let Some(side) = side
            && let Some(role) = role_compact(head)
            && role.side == Side::Centre
        {
            return Some(BoneRole::new(side, role.part));
        }
    }
    None
}

/// The role a compact name names, side-prefix conventions included.
fn role_compact(compact: &str) -> Option<BoneRole> {
    let (side, rest) = match compact {
        _ if compact.starts_with("left") => (Side::Left, &compact[4..]),
        _ if compact.starts_with("right") => (Side::Right, &compact[5..]),
        _ if compact.starts_with('l') => (Side::Left, &compact[1..]),
        _ if compact.starts_with('r') => (Side::Right, &compact[1..]),
        _ => (Side::Centre, compact),
    };
    let (stem, digits) = stem_digits(rest);
    let part = match stem {
        // `Hips` is the pelvis in Mixamo and Blender; a bare `Hip` bone is the
        // joint SSER's palettes use for the thigh (`CME L Hip`), not the pelvis.
        "pelvis" | "hips" => Part::Pelvis,
        "hip" => Part::Thigh,
        "spine" => Part::Spine(number(digits).unwrap_or(0)),
        "chest" | "torso" => Part::Spine(2),
        "neck" => Part::Neck,
        "head" => Part::Head,
        "headtop" | "headtopend" | "headend" | "toepend" | "toeend" | "top" | "end" => Part::Marker,
        "clavicle" | "shoulder" => Part::Clavicle,
        "upperarm" | "arm" | "uparm" => Part::UpperArm,
        "forearm" | "lowerarm" | "elbow" | "loarm" => Part::Forearm,
        "hand" => Part::Hand,
        "upleg" | "thigh" | "upperleg" => Part::Thigh,
        "leg" | "calf" | "shin" | "lowerleg" => Part::Calf,
        "foot" | "ankle" => Part::Foot,
        "toe" | "toebase" | "ball" => Part::Toe,
        "upperarmtwist" | "ut" => Part::Twist {
            forearm: false,
            index: number(digits).unwrap_or(1),
        },
        "forearmtwist" | "lt" => Part::Twist {
            forearm: true,
            index: number(digits).unwrap_or(1),
        },
        // Mixamo/Blender digits are 1-based; the palette's are 0-based.
        "thumb" | "handthumb" | "finger0" => Part::Digit {
            digit: 0,
            segment: number(digits).unwrap_or(1).saturating_sub(1),
        },
        "index" | "handindex" => Part::Digit {
            digit: 1,
            segment: number(digits).unwrap_or(1).saturating_sub(1),
        },
        "middle" | "handmiddle" => Part::Digit {
            digit: 2,
            segment: number(digits).unwrap_or(1).saturating_sub(1),
        },
        "ring" | "handring" => Part::Digit {
            digit: 3,
            segment: number(digits).unwrap_or(1).saturating_sub(1),
        },
        "pinky" | "little" | "handpinky" | "handlittle" => Part::Digit {
            digit: 4,
            segment: number(digits).unwrap_or(1).saturating_sub(1),
        },
        "finger" => {
            // SSE's two-digit family: the first digit names the finger, the
            // second the segment (`Finger10` = index proximal).
            let digits = digits?;
            let mut characters = digits.chars();
            let digit_index = characters.next()?.to_digit(10)? as u8;
            let segment = characters.next().and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
            Part::Digit {
                digit: digit_index,
                segment,
            }
        }
        "breast" | "prebreast" | "butt" | "prebutt" | "belly" | "genital" | "genitals"
        | "pussy" | "anus" | "clitoral" | "vagina" | "anal" | "eye" | "jaw" | "tongue"
        | "nipple" => Part::Soft,
        "root" | "scene" | "armature" | "alpha" | "com" | "object" | "skeleton" => Part::Scaffold,
        _ => return None,
    };
    let sided = matches!(
        part,
        Part::Clavicle
            | Part::UpperArm
            | Part::Forearm
            | Part::Hand
            | Part::Thigh
            | Part::Calf
            | Part::Foot
            | Part::Toe
            | Part::Digit { .. }
            | Part::Twist { .. }
    );
    Some(BoneRole::new(
        match sided {
            true => side,
            false => Side::Centre,
        },
        part,
    ))
}

/// Rest world transform of one palette joint.
fn slot_world(rig: &Rig, joint: JointId) -> Result<DMat4, CommandError> {
    Ok(DMat4::from_cols_array_2d(&rig.rest_world(joint).map_err(
        |error| {
            errors::invalid(
                "retarget_palette",
                format!("palette joint {joint}: {error}"),
            )
        },
    )?))
}

/// Rest world transforms of every node in one actor, from its rest locals.
fn rest_worlds(actor: &Actor) -> Result<Vec<DMat4>, CommandError> {
    fn resolve(
        actor: &Actor,
        node: usize,
        worlds: &mut Vec<Option<DMat4>>,
    ) -> Result<DMat4, CommandError> {
        if let Some(found) = worlds[node] {
            return Ok(found);
        }
        let local = DMat4::from_cols_array_2d(&actor.rest_local_matrix(node).map_err(actor_error)?);
        let world = match actor.node_parents()[node] {
            Some(parent) => resolve(actor, parent, worlds)? * local,
            None => local,
        };
        worlds[node] = Some(world);
        Ok(world)
    }
    let mut worlds: Vec<Option<DMat4>> = vec![None; actor.node_parents().len()];
    for node in 0..worlds.len() {
        resolve(actor, node, &mut worlds)?;
    }
    Ok(worlds
        .into_iter()
        .map(|world| world.expect("every node resolved"))
        .collect())
}

/// Fit the rotation and unit between paired rest bone origins.
///
/// Two axes are measured from the whole axial set, not from three picked bones:
/// an average up (head ring minus the foot/pelvis ring) and an average right
/// (left ring minus right ring), each orthonormalized. On a T-pose/A-pose pair a
/// three-bone pick can land on an arm and absorb the pose difference into the
/// frame; an average over the axial bones cannot.
fn fit_similarity(pairs: &[(DVec3, DVec3, BoneRole)]) -> Similarity {
    let axial: Vec<(DVec3, DVec3, BoneRole)> = pairs
        .iter()
        .copied()
        .filter(|(_, _, role)| {
            role.is_anatomical()
                && !matches!(
                    role.part,
                    Part::Hand | Part::Foot | Part::Toe | Part::Digit { .. }
                )
        })
        .collect();
    let usable: Vec<(DVec3, DVec3, BoneRole)> = match axial.len() >= 6 {
        true => axial,
        false => pairs.to_vec(),
    };
    if usable.len() < 3 {
        return Similarity {
            rotation: DMat3::IDENTITY,
            unit: 1.0,
            residual: f64::INFINITY,
            bones: usable.len(),
        };
    }
    let mean = |values: &[DVec3]| match values.is_empty() {
        true => DVec3::ZERO,
        false => values.iter().sum::<DVec3>() / values.len() as f64,
    };
    let target_center = mean(&usable.iter().map(|row| row.0).collect::<Vec<_>>());
    let source_center = mean(&usable.iter().map(|row| row.1).collect::<Vec<_>>());
    let ring = |side: Side, pick: fn(&(DVec3, DVec3, BoneRole)) -> DVec3| -> DVec3 {
        mean(
            &usable
                .iter()
                .filter(|(_, _, role)| role.side == side)
                .map(pick)
                .collect::<Vec<_>>(),
        )
    };
    let up = |pick: fn(&(DVec3, DVec3, BoneRole)) -> DVec3| -> DVec3 {
        let high = mean(
            &usable
                .iter()
                .filter(|(_, _, role)| matches!(role.part, Part::Head | Part::Neck))
                .map(pick)
                .collect::<Vec<_>>(),
        );
        let low = mean(
            &usable
                .iter()
                .filter(|(_, _, role)| {
                    matches!(
                        role.part,
                        Part::Pelvis | Part::Thigh | Part::Calf | Part::Foot
                    )
                })
                .map(pick)
                .collect::<Vec<_>>(),
        );
        high - low
    };
    let basis = |right: DVec3, up: DVec3| -> Option<DMat3> {
        let up = up.normalize_or_zero();
        let right = (right - up * right.dot(up)).normalize_or_zero();
        match up.length_squared() > 0.5 && right.length_squared() > 0.5 {
            true => Some(DMat3::from_cols(right, up, right.cross(up))),
            false => None,
        }
    };
    let target_basis = basis(
        ring(Side::Left, |row| row.0) - ring(Side::Right, |row| row.0),
        up(|row| row.0),
    );
    let source_basis = basis(
        ring(Side::Left, |row| row.1) - ring(Side::Right, |row| row.1),
        up(|row| row.1),
    );
    let rotation = match (target_basis, source_basis) {
        (Some(target), Some(source)) => target * source.inverse(),
        _ => DMat3::IDENTITY,
    };
    let source_radius: f64 = usable
        .iter()
        .map(|(_, source, _)| (*source - source_center).length_squared())
        .sum();
    let target_radius: f64 = usable
        .iter()
        .map(|(target, _, _)| (*target - target_center).length_squared())
        .sum();
    let unit = match source_radius > 0.0 && target_radius > 0.0 {
        true => (target_radius / source_radius).sqrt(),
        false => 1.0,
    };
    let extent = usable
        .iter()
        .map(|(target, _, _)| (*target - target_center).length())
        .fold(0.0_f64, f64::max);
    let residual = match extent > 0.0 {
        true => {
            usable
                .iter()
                .map(|(target, source, _)| {
                    (*target - (rotation * (*source - source_center) * unit + target_center))
                        .length()
                })
                .fold(0.0_f64, f64::max)
                / extent
        }
        false => 0.0,
    };
    Similarity {
        rotation,
        unit,
        residual,
        bones: usable.len(),
    }
}

/// The rotation of one affine with any scale divided out.
fn rotation_of(matrix: DMat4) -> DQuat {
    let (_, rotation, _) = matrix.to_scale_rotation_translation();
    rotation
}

/// Rotation angle of one unit quaternion, degrees.
fn angle_degrees(value: DQuat) -> f64 {
    (2.0 * value.w.abs().clamp(-1.0, 1.0).acos()).to_degrees()
}

impl Correspondence {
    /// Resolve every palette bone of `rig` against the source actor's skeleton.
    ///
    /// # Errors
    /// A source node whose rest transform cannot be composed, or a palette slot
    /// the rig does not hold.
    pub fn build(source: &Actor, rig: &Rig, palette: &[String]) -> Result<Self, CommandError> {
        let source_worlds = rest_worlds(source)?;
        let names: Vec<Option<String>> = (0..source.node_parents().len())
            .map(|node| source.node_name(node).map(str::to_owned))
            .collect();
        let mut claims: BTreeMap<BoneRole, Vec<usize>> = BTreeMap::new();
        let mut markers = Vec::new();
        let mut claimed: BTreeSet<usize> = BTreeSet::new();
        for (node, name) in names.iter().enumerate() {
            let Some(name) = name else { continue };
            let Some(role) = role_of(name) else { continue };
            match role.part {
                Part::Marker => {
                    markers.push(name.clone());
                    claimed.insert(node);
                }
                Part::Scaffold => {
                    claimed.insert(node);
                }
                _ => claims.entry(role).or_default().push(node),
            }
        }
        let mut ambiguous = Vec::new();
        let mut chosen: BTreeMap<BoneRole, usize> = BTreeMap::new();
        for (role, candidates) in &claims {
            let pick = candidates
                .iter()
                .copied()
                .min_by_key(|node| {
                    let name = names[*node].clone().unwrap_or_default();
                    // The ghost `CME` family duplicates every `NPC` bone at the
                    // same rest; the `NPC` one is the skeleton the skin weights.
                    let ghost = u8::from(name.starts_with("CME "));
                    (ghost, name.len(), *node)
                })
                .expect("a role has at least one candidate");
            if candidates.len() > 1 {
                ambiguous.push(Ambiguity {
                    role: role.label(),
                    candidates: candidates
                        .iter()
                        .map(|node| names[*node].clone().unwrap_or_default())
                        .collect(),
                    chosen: names[pick].clone().unwrap_or_default(),
                    rule: "prefers the `NPC` family over a ghost `CME` duplicate, then the \
                           shortest name, then the earliest node"
                        .to_owned(),
                });
            }
            chosen.insert(*role, pick);
        }

        let mut pairs: Vec<Pair> = palette
            .iter()
            .enumerate()
            .map(|(slot, name)| Pair {
                slot,
                joint: rig.joint_by_name(name).unwrap_or(JointId::new(0)),
                palette: name.clone(),
                role: role_of(name),
                source: None,
                source_name: None,
                method: Method::Unmatched,
                fold: Vec::new(),
                rest_angle_degrees: 0.0,
                rest_offset: 0.0,
            })
            .collect();
        // Pass 1: names.
        for pair in &mut pairs {
            let Some(role) = pair.role else {
                pair.method = Method::Rest;
                continue;
            };
            match role.part {
                Part::Soft => {
                    pair.method = Method::Rest;
                    continue;
                }
                Part::Twist { .. } => {
                    pair.method = Method::Derived;
                    continue;
                }
                _ => {}
            }
            let Some(node) = chosen.get(&role).copied() else {
                continue;
            };
            // One source bone drives one palette bone. A palette that carries the
            // same role twice (`NPC Pelvis` and its weightless `CME` mirror) would
            // otherwise send the source channel to whichever slot came last, and
            // the bone that actually deforms the body would never move.
            if claimed.contains(&node) {
                pair.method = Method::Rest;
                continue;
            }
            let source_name = names[node].clone().unwrap_or_default();
            pair.method = match compact(&pair.palette) == compact(&source_name) {
                true => Method::Name,
                false => Method::Convention,
            };
            pair.source = Some(node);
            pair.source_name = Some(source_name);
            claimed.insert(node);
        }
        // The fourth finger segment has no palette bone: fold it into the third.
        for pair in &mut pairs {
            let Some(BoneRole {
                side,
                part: Part::Digit { digit, segment: 2 },
            }) = pair.role
            else {
                continue;
            };
            let role = BoneRole::new(side, Part::Digit { digit, segment: 3 });
            if let Some(node) = chosen.get(&role).copied() {
                pair.fold.push(node);
                claimed.insert(node);
            }
        }
        // The fitted similarity, from the named pairs only.
        let mut measured: Vec<(DVec3, DVec3, BoneRole)> = Vec::new();
        for pair in &pairs {
            let (Some(node), Some(role)) = (pair.source, pair.role) else {
                continue;
            };
            measured.push((
                slot_world(rig, pair.joint)?.w_axis.truncate(),
                source_worlds[node].w_axis.truncate(),
                role,
            ));
        }
        let similarity = fit_similarity(&measured);

        // Pass 2: hierarchy.
        let mut unmapped: Vec<usize> = pairs
            .iter()
            .filter(|pair| pair.source.is_none() && pair.method == Method::Unmatched)
            .map(|pair| pair.slot)
            .collect();
        loop {
            let mut applied: Vec<(usize, usize)> = Vec::new();
            for slot in &unmapped {
                let Some(pair) = pairs.iter().find(|pair| pair.slot == *slot) else {
                    continue;
                };
                let Some(role) = pair.role else { continue };
                if !role.is_anatomical() {
                    continue;
                }
                let Some(parent) = rig.joint(pair.joint).and_then(|joint| joint.parent) else {
                    continue;
                };
                let Some(parent_node) = pairs
                    .iter()
                    .find(|other| other.joint == parent)
                    .and_then(|other| other.source)
                else {
                    continue;
                };
                let target_direction = (slot_world(rig, pair.joint)?.w_axis.truncate()
                    - slot_world(rig, parent)?.w_axis.truncate())
                .normalize_or_zero();
                if target_direction.length_squared() < 0.5 {
                    continue;
                }
                let mut best: Option<(f64, usize)> = None;
                let mut runner_up = f64::INFINITY;
                for node in descendants(source, parent_node, 3) {
                    if claimed.contains(&node) {
                        continue;
                    }
                    let direction = (source_worlds[node].w_axis.truncate()
                        - source_worlds[parent_node].w_axis.truncate())
                    .normalize_or_zero();
                    if direction.length_squared() < 0.5 {
                        continue;
                    }
                    let angle = (similarity.rotation * direction)
                        .dot(target_direction)
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees();
                    match best {
                        Some((best_angle, _)) if best_angle <= angle => {
                            runner_up = runner_up.min(angle);
                        }
                        Some((best_angle, _)) => {
                            runner_up = runner_up.min(best_angle);
                            best = Some((angle, node));
                        }
                        None => best = Some((angle, node)),
                    }
                }
                let Some((angle, node)) = best else { continue };
                // A candidate has to continue the chain, not merely be close:
                // 35° leaves room for a T-pose/A-pose difference and refuses a
                // bone that points somewhere else.
                if angle > 35.0 || (runner_up.is_finite() && angle * 1.5 > runner_up) {
                    continue;
                }
                applied.push((*slot, node));
            }
            if applied.is_empty() {
                break;
            }
            for (slot, node) in applied {
                if let Some(pair) = pairs.iter_mut().find(|pair| pair.slot == slot) {
                    pair.source = Some(node);
                    pair.source_name = names[node].clone();
                    pair.method = Method::Hierarchy;
                }
                claimed.insert(node);
                unmapped.retain(|other| *other != slot);
            }
        }

        // Pass 3: rest geometry.
        let mean = |values: &[DVec3]| match values.is_empty() {
            true => DVec3::ZERO,
            false => values.iter().sum::<DVec3>() / values.len() as f64,
        };
        let target_center = mean(&measured.iter().map(|row| row.0).collect::<Vec<_>>());
        let source_center = mean(&measured.iter().map(|row| row.1).collect::<Vec<_>>());
        let extent = measured
            .iter()
            .map(|(target, _, _)| (*target - target_center).length())
            .fold(0.0_f64, f64::max)
            .max(1e-9);
        let free: Vec<usize> = (0..names.len())
            .filter(|node| !claimed.contains(node) && names[*node].is_some())
            .collect();
        if !free.is_empty() {
            let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
            for slot in &unmapped {
                let Some(pair) = pairs.iter().find(|pair| pair.slot == *slot) else {
                    continue;
                };
                if !pair.role.is_some_and(|role| role.is_anatomical()) {
                    continue;
                }
                let target = slot_world(rig, pair.joint)?.w_axis.truncate();
                for node in &free {
                    let aligned = similarity.rotation
                        * (source_worlds[*node].w_axis.truncate() - source_center)
                        * similarity.unit
                        + target_center;
                    candidates.push(((target - aligned).length() / extent, *slot, *node));
                }
            }
            candidates.sort_by(|left, right| left.0.total_cmp(&right.0));
            let mut used: BTreeSet<usize> = BTreeSet::new();
            for (distance, slot, node) in candidates {
                if distance > 0.08 {
                    break;
                }
                if used.contains(&slot) || claimed.contains(&node) {
                    continue;
                }
                used.insert(slot);
                claimed.insert(node);
                if let Some(pair) = pairs.iter_mut().find(|pair| pair.slot == slot) {
                    pair.source = Some(node);
                    pair.source_name = names[node].clone();
                    pair.method = Method::Geometry;
                }
                unmapped.retain(|other| *other != slot);
            }
        }

        // Report what is left, with the reason.
        for pair in &mut pairs {
            if let Some(source) = pair.source {
                claimed.insert(source);
                for node in &pair.fold {
                    claimed.insert(*node);
                }
                continue;
            }
            pair.method = match pair.role {
                Some(BoneRole {
                    part: Part::Twist { .. },
                    ..
                }) => Method::Derived,
                Some(BoneRole {
                    part: Part::Soft, ..
                })
                | None => Method::Rest,
                _ => Method::Unmatched,
            };
        }
        let mut unmatched = Vec::new();
        for (node, name) in names.iter().enumerate() {
            if claimed.contains(&node) {
                continue;
            }
            let Some(name) = name else {
                unmatched.push(UnmatchedSource {
                    node,
                    name: format!("node {node}"),
                    role: None,
                    reason: "the node is unnamed, so no naming convention can resolve it"
                        .to_owned(),
                });
                continue;
            };
            let role = role_of(name);
            let reason = match role.map(|role| role.part) {
                Some(Part::Soft) => "soft tissue: the palette holds it at rest".to_owned(),
                Some(Part::Scaffold) => "scaffolding, not a bone".to_owned(),
                Some(Part::Marker) => "a leaf marker, not a bone".to_owned(),
                Some(Part::Twist { .. }) => "a twist helper the palette derives".to_owned(),
                Some(_) => "the palette has no bone for this role".to_owned(),
                None => {
                    "the name resolves to no known convention and no fallback matched".to_owned()
                }
            };
            unmatched.push(UnmatchedSource {
                node,
                name: name.clone(),
                role: role.map(|role| role.label()),
                reason,
            });
        }
        unmatched.sort_by(|left, right| left.name.cmp(&right.name));
        let mut correspondence = Self {
            pairs,
            similarity,
            unmatched,
            ambiguous,
            markers,
        };
        correspondence.measure(rig, source, &source_worlds)?;
        Ok(correspondence)
    }

    /// The source node driving one palette slot.
    pub fn source_of(&self, slot: usize) -> Option<usize> {
        self.pairs
            .iter()
            .find(|pair| pair.slot == slot)
            .and_then(|pair| pair.source)
    }

    /// The extra source nodes folded into one palette slot's rotation.
    pub fn fold_of(&self, slot: usize) -> &[usize] {
        self.pairs
            .iter()
            .find(|pair| pair.slot == slot)
            .map(|pair| pair.fold.as_slice())
            .unwrap_or(&[])
    }

    /// Every mapped palette bone and the source node that drives it.
    pub fn mapping(&self) -> Vec<(usize, usize, String)> {
        self.pairs
            .iter()
            .filter_map(|pair| {
                pair.source
                    .map(|node| (node, pair.slot, pair.palette.clone()))
            })
            .collect()
    }

    /// Palette bones no source drives, by name.
    pub fn unmatched_palette(&self) -> Vec<&str> {
        self.pairs
            .iter()
            .filter(|pair| pair.source.is_none() && pair.method == Method::Unmatched)
            .map(|pair| pair.palette.as_str())
            .collect()
    }

    /// Fill every pair's rest angle and position residual: the property of the
    /// two bodies, reported, never applied.
    fn measure(
        &mut self,
        rig: &Rig,
        source: &Actor,
        source_worlds: &[DMat4],
    ) -> Result<(), CommandError> {
        let pairs: Vec<(usize, JointId, usize)> = self
            .pairs
            .iter()
            .filter_map(|pair| pair.source.map(|node| (pair.slot, pair.joint, node)))
            .collect();
        if pairs.is_empty() {
            return Ok(());
        }
        let source_center = pairs
            .iter()
            .map(|(_, _, node)| source_worlds[*node].w_axis.truncate())
            .sum::<DVec3>()
            / pairs.len() as f64;
        let mut aligned: Vec<(usize, DVec3, DVec3)> = Vec::new();
        for (slot, joint, node) in &pairs {
            aligned.push((
                *slot,
                slot_world(rig, *joint)?.w_axis.truncate(),
                self.similarity.rotation
                    * (source_worlds[*node].w_axis.truncate() - source_center)
                    * self.similarity.unit,
            ));
        }
        let target_center = aligned.iter().map(|row| row.1).sum::<DVec3>() / aligned.len() as f64;
        let extent = aligned
            .iter()
            .map(|row| (row.1 - target_center).length())
            .fold(0.0_f64, f64::max)
            .max(1e-9);
        let rotation = DQuat::from_mat3(&self.similarity.rotation);
        let mut rows: Vec<(usize, f64, f64)> = Vec::new();
        for (slot, joint, node) in &pairs {
            let target = slot_world(rig, *joint)?;
            let mapped = rotation * DQuat::from_mat4(&source_worlds[*node]);
            let aligned_position = aligned
                .iter()
                .find(|row| row.0 == *slot)
                .map(|row| row.2 + target_center)
                .unwrap_or(target_center);
            rows.push((
                *slot,
                angle_degrees(rotation_of(target) * mapped.inverse()),
                (target.w_axis.truncate() - aligned_position).length() / extent,
            ));
        }
        for (slot, angle, offset) in rows {
            if let Some(pair) = self.pairs.iter_mut().find(|pair| pair.slot == slot) {
                pair.rest_angle_degrees = angle;
                pair.rest_offset = offset;
            }
        }
        let _ = source;
        Ok(())
    }
}

/// Every descendant of one source node, up to `depth` links down.
fn descendants(source: &Actor, node: usize, depth: usize) -> Vec<usize> {
    let parents = source.node_parents();
    let mut found = Vec::new();
    let mut frontier = vec![node];
    for _ in 0..depth {
        let mut next = Vec::new();
        for parent in frontier {
            for (child, entry) in parents.iter().enumerate() {
                if *entry == Some(parent) {
                    found.push(child);
                    next.push(child);
                }
            }
        }
        frontier = next;
    }
    found
}

/// The report one correspondence produces: per-bone method, the fitted
/// similarity, the contested roles and every source bone that drives nothing.
pub fn correspondence_report(correspondence: &Correspondence) -> serde_json::Value {
    let counts = |method: Method| {
        correspondence
            .pairs
            .iter()
            .filter(|pair| pair.method == method)
            .count()
    };
    let rotation = DQuat::from_mat3(&correspondence.similarity.rotation);
    let axis = rotation.xyz();
    let axis = match axis.length_squared() > 1e-12 {
        true => axis.normalize(),
        false => DVec3::Z,
    };
    json!({
        "similarity": {
            "rotation_degrees": angle_degrees(rotation),
            "rotation_axis": [axis.x, axis.y, axis.z],
            "unit": correspondence.similarity.unit,
            "residual": correspondence.similarity.residual,
            "bones": correspondence.similarity.bones,
        },
        "methods": {
            "name": counts(Method::Name),
            "convention": counts(Method::Convention),
            "hierarchy": counts(Method::Hierarchy),
            "geometry": counts(Method::Geometry),
            "derived": counts(Method::Derived),
            "rest": counts(Method::Rest),
            "unmatched": counts(Method::Unmatched),
        },
        "mapped": correspondence.mapping().len(),
        "palette_bones": correspondence.pairs.len(),
        "unmatched_palette": correspondence.unmatched_palette(),
        "ambiguous": correspondence
            .ambiguous
            .iter()
            .map(|row| json!({
                "role": row.role,
                "candidates": row.candidates,
                "chosen": row.chosen,
                "rule": row.rule,
            }))
            .collect::<Vec<_>>(),
        "markers": correspondence.markers,
        "unmatched_source": correspondence
            .unmatched
            .iter()
            .map(|row| json!({
                "node": row.node,
                "name": row.name,
                "role": row.role,
                "reason": row.reason,
            }))
            .collect::<Vec<_>>(),
        "pairs": correspondence
            .pairs
            .iter()
            .map(|pair| json!({
                "slot": pair.slot,
                "palette": pair.palette,
                "role": pair.role.map(BoneRole::label),
                "source": pair.source_name,
                "method": pair.method.token(),
                "fold": pair.fold,
                "rest_angle_degrees": pair.rest_angle_degrees,
                "rest_offset": pair.rest_offset,
            }))
            .collect::<Vec<_>>(),
    })
}

/// The standard rig roles this correspondence drives, from the roles themselves.
///
/// The palette's *names* are not the measurement: `clip_role` reads SSE prefixes,
/// so on a `mixamorig:*` palette it reports zero coverage for a complete clip.
/// A role is a function, and the correspondence resolved it by function, so the
/// coverage is counted here.
/// The standard role token a palette or source part answers, `None` for parts
/// no clip role names.
pub fn coverage_token(part: Part) -> Option<&'static str> {
    match part {
        Part::Pelvis | Part::Thigh => Some("hips"),
        Part::Spine(_) => Some("spine"),
        Part::Calf => Some("knees"),
        Part::Foot => Some("ankles"),
        Part::Toe => Some("toes"),
        Part::Clavicle | Part::UpperArm => Some("shoulders"),
        Part::Forearm => Some("elbows"),
        Part::Hand => Some("hands"),
        Part::Neck => Some("neck"),
        Part::Head => Some("head"),
        Part::Twist { .. } => Some("twists"),
        _ => None,
    }
}

pub fn role_coverage(correspondence: &Correspondence) -> RoleCoverage {
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();
    let mut present: BTreeSet<&'static str> = BTreeSet::new();
    for pair in &correspondence.pairs {
        let Some(role) = pair.role else { continue };
        let Some(token) = coverage_token(role.part) else {
            continue;
        };
        present.insert(token);
        // A derived twist is written by the plan even though no source bone drives
        // it, so it covers its role; a bone held at rest does not.
        if pair.source.is_some() || pair.method == Method::Derived {
            covered.insert(token);
        }
    }
    // Chain roles answer for their bone: a correspondence that maps a thigh but no
    // calf still drives `hips`, and the report says exactly that.
    const ALL: [&str; 11] = [
        "hips",
        "spine",
        "knees",
        "ankles",
        "toes",
        "shoulders",
        "elbows",
        "hands",
        "neck",
        "head",
        "twists",
    ];
    let covered: Vec<&'static str> = ALL
        .into_iter()
        .filter(|role| covered.contains(role))
        .collect();
    let missing: Vec<&'static str> = ALL
        .into_iter()
        .filter(|role| present.contains(role) && !covered.contains(role))
        .collect();
    // A role no palette bone carries cannot be required: a Mixamo palette has no
    // twist bones at all, and demanding one would refuse every correct clip.
    let unavailable: Vec<&'static str> = ALL
        .into_iter()
        .filter(|role| !present.contains(role))
        .collect();
    RoleCoverage {
        covered,
        missing,
        unavailable,
    }
}

/// Which standard roles a correspondence drives, which it cannot, and which its
/// palette does not carry at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleCoverage {
    /// Roles at least one mapped bone carries.
    pub covered: Vec<&'static str>,
    /// Roles the palette carries but no mapped bone drives.
    pub missing: Vec<&'static str>,
    /// Roles no palette bone answers.
    pub unavailable: Vec<&'static str>,
}

/// Refuse a correspondence that resolved nothing.
///
/// # Errors
/// A source skeleton whose names matched no convention and whose geometry
/// matched nothing drives no palette bone, so there is nothing to convert.
pub fn require_mapped(correspondence: &Correspondence, source: &str) -> Result<(), CommandError> {
    if correspondence.mapping().is_empty() {
        return Err(errors::invalid(
            "retarget_correspondence",
            format!(
                "no bone of the source skeleton {source:?} resolves to a palette bone; the names \
                 matched no convention and the rest geometry matched nothing"
            ),
        ));
    }
    Ok(())
}

/// The palette role of one bone name, exposed for the report and the tests.
pub fn palette_role(name: &str) -> Option<BoneRole> {
    role_of(name)
}
