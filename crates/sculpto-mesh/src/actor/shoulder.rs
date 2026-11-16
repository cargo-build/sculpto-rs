//! The shoulder girdle's seat measured against the flesh. @req DECL0013
//!
//! `0148` reported the clavicle rest offset as the shared cause of two visible
//! defects — a shoulder girdle that reads as a high, wide frame over the chest
//! and hands that hang at the crotch — because our `Spine2 → Clavicle` rest
//! distance is **2.598 ×** the source skeleton's `Spine2 → LeftShoulder`. That
//! ratio is not evidence of a defect: it compares two different anatomical
//! landmarks between two different bodies. Skyrim starts the clavicle near the
//! neck base and runs it out to the shoulder; Mixamo's `LeftShoulder` sits barely
//! 7 mm above its own `Spine2`, which is a different vertebra.
//!
//! So the question is not the ratio but the flesh: **is the shoulder joint inside
//! the deltoid mass, and is the clavicle root inside the upper chest / neck
//! base?** This verb answers exactly that, from the surface, and nothing else.
//!
//! # What the surface can and cannot decide
//!
//! * **Inside or outside — decidable, exactly.** The shortest distance from a
//!   pivot to the body's triangles, with the sign of `pivot − nearest_point`
//!   along the nearest triangle's own normal. A pivot in front of the skin is a
//!   fact, not an opinion. Six axis rays ([`SeatWall`]) say which side is thin and
//!   which side is open air, so "outside" can be reported with the excursion.
//! * **How deep it belongs — not decidable.** No clavicle, acromion or glenoid
//!   marker exists in a skinned surface. The verb therefore reports the
//!   excursion and refuses to invent an interior target; a pivot that is already
//!   inside is reported `agreed` and never moved. That is `0144`'s discipline
//!   applied to a second seat: move only what the flesh proves wrong, and where
//!   the flesh cannot decide, say so with the measurement attached.
//!
//! # What it also places in the same frame
//!
//! The section at each pivot's own height (the same area-weighted cut
//! `actor torso-fit` uses), the parent link that produced the famous 285.5 mm
//! figure, and the trapezius line — the height where the body stops being a neck
//! column and becomes shoulders — so "near the neck base" is a number instead of
//! a phrase.

use std::collections::BTreeMap;

use glam::DVec3;

use super::fit::{node_names, parse_name};
use super::torso::{BodyFrame, Decision, Frame, Section, Selection};
use super::{Actor, ActorError, PartSelector, invalid};

/// The joints of one side the seat is read for, parent before child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeatBone {
    pub level: &'static str,
    pub name: &'static str,
    /// Whether the joint's seat is what this verb is asked about. `Forearm` and
    /// `Hand` are measured as the arm's own context: they say where the arm
    /// hangs and how long it is in the flesh, and they are never the subject of
    /// a seat decision here.
    pub owned: bool,
}

/// Clavicle, shoulder joint, elbow and wrist. The first two are the seat; the
/// last two are the arm the seat carries.
pub const SEAT_CHAIN: [SeatBone; 4] = [
    SeatBone {
        level: "clavicle",
        name: "Clavicle",
        owned: true,
    },
    SeatBone {
        level: "shoulder",
        name: "UpperArm",
        owned: true,
    },
    SeatBone {
        level: "elbow",
        name: "Forearm",
        owned: false,
    },
    SeatBone {
        level: "wrist",
        name: "Hand",
        owned: false,
    },
];

/// How far below the neck joint the trapezius line is searched for, metres.
const NECK_BASE_REACH: f64 = 0.120;

/// The neck-base scan's height step, metres.
const NECK_BASE_STEP: f64 = 0.002;

/// The section half-width at which the body has become shoulders, as a multiple
/// of the neck's own half-width at the neck joint.
const NECK_BASE_RATIO: f64 = 1.5;

/// One directional wall: the first surface crossing along an axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeatWall {
    pub direction: &'static str,
    /// Distance to the first crossing, metres; `None` means the ray never meets
    /// the surface, i.e. that side is open air.
    pub distance: Option<f64>,
}

/// One joint's seat, with the three records the assignment's model asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatJoint {
    pub side: char,
    pub level: &'static str,
    pub name: String,
    pub node: usize,
    pub owned: bool,
    /// The rig pivot, metres, in the actor's frame.
    pub pivot: [f64; 3],
    /// Parent joint's name and the link vector to it, metres.
    pub parent: Option<String>,
    pub parent_link: Option<[f64; 3]>,
    /// Shortest distance from the pivot to the body's surface, metres.
    pub nearest: f64,
    /// `pivot − nearest_point` along the nearest triangle's normal, metres;
    /// positive is outside the flesh.
    pub signed: f64,
    /// The nearest surface point, metres.
    pub closest: [f64; 3],
    /// The six directional walls.
    pub walls: Vec<SeatWall>,
    /// Height of the pivot above the actor's floor, metres.
    pub height: f64,
    /// The section of the body at the pivot's own height.
    pub section: Section,
    /// For a clavicle row: how far the pivot sits below the trapezius line,
    /// metres; positive is below. `None` when the line could not be found.
    pub below_neck_base: Option<f64>,
    pub decision: Decision,
    pub evidence: String,
}

/// The trapezius line: where the body stops being a neck column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NeckBase {
    /// The level, metres.
    pub level: f64,
    /// Section half-width there, metres.
    pub half_width: f64,
    /// The neck's own half-width at the neck joint, metres.
    pub neck_half_width: f64,
    /// `half_width / neck_half_width`, dimensionless and always ≥ the threshold.
    pub ratio: f64,
    /// `neck_joint_height − level`, metres.
    pub below_neck_joint: f64,
}

/// The whole reading.
#[derive(Clone, Debug, PartialEq)]
pub struct SeatOutcome {
    pub frame: BodyFrame,
    pub neck_base: Option<NeckBase>,
    pub joints: Vec<SeatJoint>,
    /// Layers cut at every joint's height, for the garment context.
    pub layers: Vec<Section>,
    pub vertex_count: usize,
}

impl SeatOutcome {
    /// Owned rows the flesh proves wrong: outside the surface.
    pub fn outside(&self) -> usize {
        self.joints
            .iter()
            .filter(|joint| joint.owned && joint.decision == Decision::Unresolved)
            .count()
    }
}

/// Read the shoulder girdle's seat against the body surface.
///
/// # Errors
/// A part selection that names no unique mesh node or a primitive out of range,
/// or a palette that carries no clavicle/upper-arm pair.
pub fn shoulder_seat(
    actor: &Actor,
    body: &PartSelector,
    layers: &[PartSelector],
) -> Result<SeatOutcome, ActorError> {
    let frame = Frame::of(actor)?;
    let body_part = Selection::read(actor, body)?;
    let layer_parts = layers
        .iter()
        .map(|layer| Selection::read(actor, layer))
        .collect::<Result<Vec<_>, _>>()?;
    let worlds = super::animation::compose_worlds(actor, &actor.transforms)?;
    let names = node_names(actor);
    let palette = Palette::read(&names)?;
    let pivots: BTreeMap<usize, DVec3> = palette
        .named
        .iter()
        .map(|row| (row.node, worlds[row.node].transform_point3(DVec3::ZERO)))
        .collect();
    let neck_base = neck_base(&body_part, &frame, &palette, &pivots);

    let mut joints = Vec::new();
    for side in ['L', 'R'] {
        for bone in &SEAT_CHAIN {
            let Some(node) = palette.find(side, bone.name) else {
                continue;
            };
            let Some(&pivot) = pivots.get(&node) else {
                continue;
            };
            let height = pivot.dot(frame.up);
            let plane_point = [pivot.dot(frame.lateral), pivot.dot(frame.anterior)];
            let section =
                body_part.section_torso(&frame, bone.level, height, &|_, _| Some(plane_point));
            let walls = walls_of(&body_part, &frame, pivot);
            let (nearest, signed, closest) = nearest_surface(&body_part, pivot);
            let parent = palette
                .parent_of(actor, node)
                .and_then(|parent| names[parent].clone());
            let parent_link = palette
                .parent_of(actor, node)
                .and_then(|parent| pivots.get(&parent).map(|point| (pivot - *point).to_array()));
            let below_neck_base = match (bone.level, neck_base) {
                ("clavicle", Some(base)) => Some(base.level - height),
                _ => None,
            };
            // The flesh decides inside from outside. It cannot decide how deep a
            // pivot belongs — no surface landmark names a joint centre — so an
            // outside pivot is reported, never moved to an invented depth.
            let decision = match signed > 0.0 {
                true => Decision::Unresolved,
                false => Decision::Agreed,
            };
            let evidence = seat_evidence(
                &parent_link,
                side,
                bone,
                nearest,
                signed,
                &walls,
                &section,
                below_neck_base,
                &frame,
            );
            joints.push(SeatJoint {
                side,
                level: bone.level,
                name: names[node].clone().unwrap_or_else(|| bone.name.to_owned()),
                node,
                owned: bone.owned,
                pivot: pivot.to_array(),
                parent,
                parent_link,
                nearest,
                signed,
                closest: closest.to_array(),
                walls,
                height,
                section,
                below_neck_base,
                decision,
                evidence,
            });
        }
    }
    if joints.is_empty() {
        return Err(invalid(
            "actor shoulder-seat",
            "the palette names no `NPC L/R Clavicle` or `NPC L/R UpperArm` joint",
        ));
    }

    let mut layer_sections = Vec::new();
    for layer in &layer_parts {
        for joint in &joints {
            if !joint.owned {
                continue;
            }
            layer_sections.push(layer.section_all(&frame, joint.level, joint.height));
        }
    }

    Ok(SeatOutcome {
        frame: frame.report(),
        neck_base,
        joints,
        layers: layer_sections,
        vertex_count: body_part.positions.len()
            + layer_parts
                .iter()
                .map(|layer| layer.positions.len())
                .sum::<usize>(),
    })
}

/// One line of evidence per joint, in the report's own units.
#[allow(clippy::too_many_arguments)]
fn seat_evidence(
    parent_link: &Option<[f64; 3]>,
    side: char,
    bone: &SeatBone,
    nearest: f64,
    signed: f64,
    walls: &[SeatWall],
    section: &Section,
    below_neck_base: Option<f64>,
    frame: &Frame,
) -> String {
    let wall = |direction: &str| {
        walls
            .iter()
            .find(|wall| wall.direction == direction)
            .and_then(|wall| wall.distance)
            .map_or_else(
                || "open".to_owned(),
                |value| format!("{:.2}", value * 1000.0),
            )
    };
    let link = parent_link.map_or_else(
        || "no parent link".to_owned(),
        |link| {
            let vector = DVec3::from_array(link);
            format!(
                "parent link {:.2} mm ({:.2} mm up)",
                vector.length() * 1000.0,
                vector.dot(frame.up) * 1000.0
            )
        },
    );
    let neck = below_neck_base.map_or_else(String::new, |below| {
        format!(", {:.2} mm below the trapezius line", below * 1000.0)
    });
    let section = section.unresolved.as_ref().map_or_else(
        || {
            let area = section.area() * 1e4;
            let bounds = section.bounds();
            let width = bounds.map_or(f64::NAN, |row| (row.1 - row.0) * 1000.0);
            format!(
                "section {:.2} cm2, {:.2} mm wide, {} loop(s)",
                area,
                width,
                section.selected.len()
            )
        },
        |reason| format!("section unresolved: {reason}"),
    );
    format!(
        "{side} {}: {:.2} mm from the surface, signed {:+.2} mm ({}), walls out/in/up/down/ant/post \
         {} / {} / {} / {} / {} / {} mm{neck}; {link}; {section}",
        bone.level,
        nearest * 1000.0,
        signed * 1000.0,
        match signed > 0.0 {
            true => "outside",
            false => "inside",
        },
        wall("outward"),
        wall("inward"),
        wall("up"),
        wall("down"),
        wall("anterior"),
        wall("posterior"),
    )
}

/// The six directional walls around one pivot.
fn walls_of(selection: &Selection, frame: &Frame, pivot: DVec3) -> Vec<SeatWall> {
    let outward = match pivot.dot(frame.lateral) >= 0.0 {
        true => frame.lateral,
        false => -frame.lateral,
    };
    [
        ("outward", outward),
        ("inward", -outward),
        ("up", frame.up),
        ("down", -frame.up),
        ("anterior", frame.anterior),
        ("posterior", -frame.anterior),
    ]
    .into_iter()
    .map(|(direction, axis)| SeatWall {
        direction,
        distance: first_hit(selection, pivot, axis),
    })
    .collect()
}

/// The shortest distance from a point to the surface, with the sign read off the
/// nearest triangle's own normal.
fn nearest_surface(selection: &Selection, point: DVec3) -> (f64, f64, DVec3) {
    let mut best: Option<(f64, DVec3, DVec3)> = None;
    for triangle in &selection.triangles {
        let [Some(a), Some(b), Some(c)] = [
            selection.positions.get(triangle[0]),
            selection.positions.get(triangle[1]),
            selection.positions.get(triangle[2]),
        ] else {
            continue;
        };
        let (a, b, c) = (
            DVec3::from_array(*a),
            DVec3::from_array(*b),
            DVec3::from_array(*c),
        );
        let closest = closest_on_triangle(point, a, b, c);
        let distance = point.distance(closest);
        if best.is_none_or(|(found, _, _)| distance < found) {
            best = Some((distance, closest, (b - a).cross(c - a)));
        }
    }
    match best {
        Some((distance, closest, normal)) => {
            let normal = normal.normalize_or_zero();
            (distance, (point - closest).dot(normal), closest)
        }
        None => (f64::INFINITY, 0.0, point),
    }
}

/// Closest point on a triangle to `point` (Ericson, *Real-Time Collision
/// Detection*, §5.1.5).
fn closest_on_triangle(point: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = point - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = point - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denominator = 1.0 / (va + vb + vc);
    a + ab * (vb * denominator) + ac * (vc * denominator)
}

/// How far the first surface crossing along `direction` is, if there is one.
fn first_hit(selection: &Selection, origin: DVec3, direction: DVec3) -> Option<f64> {
    let direction = direction.normalize_or_zero();
    if direction == DVec3::ZERO {
        return None;
    }
    let mut best: Option<f64> = None;
    for triangle in &selection.triangles {
        let [Some(a), Some(b), Some(c)] = [
            selection.positions.get(triangle[0]),
            selection.positions.get(triangle[1]),
            selection.positions.get(triangle[2]),
        ] else {
            continue;
        };
        let edge1 = DVec3::from_array(*b) - DVec3::from_array(*a);
        let edge2 = DVec3::from_array(*c) - DVec3::from_array(*a);
        let h = direction.cross(edge2);
        let determinant = edge1.dot(h);
        if determinant.abs() < 1e-12 {
            continue;
        }
        let inverse = 1.0 / determinant;
        let s = origin - DVec3::from_array(*a);
        let u = s.dot(h) * inverse;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(edge1);
        let v = direction.dot(q) * inverse;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = edge2.dot(q) * inverse;
        if t > 1e-6 && best.is_none_or(|found| t < found) {
            best = Some(t);
        }
    }
    best
}

/// The trapezius line: the highest level under the neck joint at which the body's
/// section is already wider than the neck by [`NECK_BASE_RATIO`].
fn neck_base(
    body: &Selection,
    frame: &Frame,
    palette: &Palette,
    pivots: &BTreeMap<usize, DVec3>,
) -> Option<NeckBase> {
    let neck = palette.find('C', "Neck")?;
    let pivot = *pivots.get(&neck)?;
    let neck_height = pivot.dot(frame.up);
    let point = [pivot.dot(frame.lateral), pivot.dot(frame.anterior)];
    let section = body.section_torso(frame, "neck", neck_height, &|_, _| Some(point));
    let half_width = |section: &Section| -> Option<f64> {
        let bounds = section.bounds()?;
        Some((bounds.1 - bounds.0) * 0.5)
    };
    let neck_half_width = half_width(&section)?;
    let mut level = neck_height;
    while level > neck_height - NECK_BASE_REACH {
        level -= NECK_BASE_STEP;
        let section = body.section_torso(frame, "neck_base", level, &|_, _| Some(point));
        let Some(width) = half_width(&section) else {
            continue;
        };
        let ratio = width / neck_half_width;
        if ratio >= NECK_BASE_RATIO {
            return Some(NeckBase {
                level,
                half_width: width,
                neck_half_width,
                ratio,
                below_neck_joint: neck_height - level,
            });
        }
    }
    None
}

/// The named palette joints the seat needs.
struct Palette {
    named: Vec<Named>,
}

/// One named joint of the actor's palette.
struct Named {
    side: char,
    base: String,
    node: usize,
}

impl Palette {
    fn read(names: &[Option<String>]) -> Result<Self, ActorError> {
        let named: Vec<Named> = names
            .iter()
            .enumerate()
            .filter_map(|(node, name)| {
                let (side, base) = name.as_deref().and_then(parse_name)?;
                Some(Named {
                    side,
                    base: base.to_owned(),
                    node,
                })
            })
            .collect();
        match named
            .iter()
            .any(|row| row.base == "Clavicle" || row.base == "UpperArm")
        {
            true => Ok(Self { named }),
            false => Err(invalid(
                "actor shoulder-seat",
                "the palette names no Clavicle or UpperArm joint",
            )),
        }
    }

    fn find(&self, side: char, base: &str) -> Option<usize> {
        self.named
            .iter()
            .find(|row| row.side == side && row.base == base)
            .map(|row| row.node)
    }

    fn parent_of(&self, actor: &Actor, node: usize) -> Option<usize> {
        actor.parent_of(node)
    }
}
