//! Torso bone placement measured against the flesh's own cross-sections. @req DECL0013
//!
//! The operator reported the spine sitting *behind and left of* the body. The
//! skeleton's own numbers cannot answer that — `actor adapt` leaves the chain
//! where the NIF put it — so the answer has to come from the surface: cut the
//! actor at the height of each torso joint and compare the chain with the flesh
//! standing there.
//!
//! # What a cross-section can and cannot decide
//!
//! The cut is *area weighted*. Every triangle crossing the plane contributes the
//! segment where it cuts; the segments are oriented against the triangle normal,
//! linked into closed loops through their shared edges, and each loop's enclosed
//! area and area centroid come from the shoelace terms. A raw vertex mean would
//! move with tessellation; this does not. An open chain means the surface is not
//! closed at that height, so the level is reported unresolved instead of being
//! measured off a hole.
//!
//! The two horizontal axes are not equally decidable, and the difference is
//! measured rather than assumed:
//!
//! * **Lateral** — the body is mirror symmetric, so the section's lateral centre
//!   *is* a landmark: the symmetry plane. [`SectionLoop::asymmetry`] reports how
//!   well a section actually spans one (0.02 mm on the shipped bodies). A lateral
//!   offset larger than [`LATERAL_TOLERANCE`] is a placement defect and is
//!   fitted.
//! * **Anterior** — the section's centre of area is *not* a vertebral landmark.
//!   Flesh is not symmetric front to back: breasts, belly and glutes move the
//!   centre of area by tens of millimetres while no bone follows that mass. The
//!   verb measures that effect on every run ([`LimbProbe`]): on a thigh or an
//!   upper arm — where the flesh is near a cylinder and the bone's position is
//!   trusted — the same area centroid sits [`TorsoOutcome::anterior_bias`] away
//!   from the bone. Fitting the chain onto it would move the vertebral pivots
//!   into the viscera, so the anterior axis is measured per level and never
//!   moved. That is the assignment's rule: no invented offsets.
//!
//! # What the fit writes
//!
//! Only the lateral axis ever moves a joint, and only by the measured lateral
//! offset. Rest geometry is untouched because every joint's inverse bind matrix
//! is recomputed as `inverse(new_world) * skin_base` — the one constant the fit
//! proves over the whole skin first (owned by [`super::fit`]) — and a joint the
//! fit does not move keeps its rest world, its local transform absorbing the
//! parent's change instead. Weights, triangles, materials and clips are not
//! re-encoded at all.

use std::collections::{BTreeMap, HashMap};

use glam::DVec3;

use super::fit::{build_frames, node_names, parse_name, skin_base, skin_matrices};
use super::{Actor, ActorError, FittedFrame, Part, Sample, geometry, invalid};

/// A lateral offset smaller than this is not a placement defect: the shipped
/// bodies report a section asymmetry of 0.02 mm, so this is 50× the instrument's
/// own noise and still far below a visible displacement.
pub const LATERAL_TOLERANCE: f64 = 1e-3;

/// Identity quantisation of a cut point, in metres. Two triangles either side of
/// a shared edge compute the same point two ways; 1 µm groups those variants and
/// keeps distinct surfaces apart.
const POINT_KEY: f64 = 1e-6;

/// One joint of the torso chain, in the order the chain runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TorsoBone {
    pub level: &'static str,
    pub name: &'static str,
    /// Whether the fit may move this joint. The head is measured for reference:
    /// `012E` already placed its pivot, and it is not part of the spine chain.
    pub owned: bool,
}

/// Pelvis, three spine joints and the neck, with the head as a reference level.
pub const TORSO_CHAIN: [TorsoBone; 6] = [
    TorsoBone {
        level: "pelvis",
        name: "Pelvis",
        owned: true,
    },
    TorsoBone {
        level: "spine0",
        name: "Spine",
        owned: true,
    },
    TorsoBone {
        level: "spine1",
        name: "Spine1",
        owned: true,
    },
    TorsoBone {
        level: "spine2",
        name: "Spine2",
        owned: true,
    },
    TorsoBone {
        level: "neck",
        name: "Neck",
        owned: true,
    },
    TorsoBone {
        level: "head",
        name: "Head",
        owned: false,
    },
];

/// One body or garment part: a mesh node and, when the node carries several
/// primitives, the index that names the layer (`3BA#1` for the bra).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartSelector {
    pub node_name: String,
    pub primitive: Option<usize>,
}

impl PartSelector {
    /// Parse `Name` or `Name#i`.
    ///
    /// # Errors
    /// A `#` suffix that is not a primitive index.
    pub fn parse(text: &str) -> Result<Self, ActorError> {
        match text.rsplit_once('#') {
            None => Ok(Self {
                node_name: text.to_owned(),
                primitive: None,
            }),
            Some((name, suffix)) => match suffix.parse::<usize>() {
                Ok(primitive) => Ok(Self {
                    node_name: name.to_owned(),
                    primitive: Some(primitive),
                }),
                Err(_) => Err(invalid(
                    "torso part",
                    format!("{text:?}: a `#` suffix must be a primitive index"),
                )),
            },
        }
    }

    /// Stable report spelling.
    pub fn label(&self) -> String {
        match self.primitive {
            Some(primitive) => format!("{}#{primitive}", self.node_name),
            None => self.node_name.clone(),
        }
    }
}

/// What the fit decided about one axis of one joint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Measured, fitted, and the pivot moved.
    Fitted,
    /// Measured, and the authored pivot already sits on the flesh.
    Agreed,
    /// The surface cannot decide this axis at this level.
    Unresolved,
}

impl Decision {
    /// Stable report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fitted => "fitted",
            Self::Agreed => "agreed",
            Self::Unresolved => "unresolved",
        }
    }
}

/// The actor's up/lateral/anterior frame: orthonormal, right-handed, unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyFrame {
    pub up: [f64; 3],
    pub lateral: [f64; 3],
    pub anterior: [f64; 3],
    /// Where the anterior axis came from: `feet` when the actor carries an ankle
    /// and a ball, `world` when it does not.
    pub anterior_source: &'static str,
}

/// One loop of a section, in the plane's own two coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionLoop {
    /// Enclosed area, square metres; the sign follows the loop's winding.
    pub area: f64,
    /// Area centroid as `(lateral, anterior)`, metres.
    pub centroid: [f64; 2],
    /// The loop's points, `(lateral, anterior)`, metres.
    pub ring: Vec<[f64; 2]>,
    pub segments: usize,
    pub closed: bool,
}

impl SectionLoop {
    fn bounds(&self, axis: usize) -> (f64, f64) {
        self.ring
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), point| {
                (low.min(point[axis]), high.max(point[axis]))
            })
    }

    /// How far the centre of area sits from the midpoint of the loop's own
    /// lateral extent: the symmetry the lateral landmark rests on.
    pub fn asymmetry(&self) -> f64 {
        let (low, high) = self.bounds(0);
        (self.centroid[0] - 0.5 * (low + high)).abs()
    }
}

/// Every cut of one part at one height.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub part: String,
    pub level: String,
    pub plane: f64,
    pub segments: usize,
    /// Junction points joining more than two segments: a self-touching surface.
    pub pinched: usize,
    pub loops: Vec<SectionLoop>,
    pub open_chains: usize,
    /// Indices into `loops` the level was read from.
    pub selected: Vec<usize>,
    /// Why the level could not be read, when it could not.
    pub unresolved: Option<String>,
}

impl Section {
    /// Area of the selected loops, square metres.
    pub fn area(&self) -> f64 {
        self.selected_loops().map(|row| row.area.abs()).sum()
    }

    fn selected_loops(&self) -> impl Iterator<Item = &SectionLoop> {
        self.selected
            .iter()
            .filter_map(|index| self.loops.get(*index))
    }

    /// Area-weighted centre of the selected loops, in the plane.
    pub fn centroid(&self) -> Option<[f64; 2]> {
        let mut weight = 0.0;
        let mut sum = [0.0_f64; 2];
        for row in self.selected_loops() {
            let area = row.area.abs();
            sum[0] += row.centroid[0] * area;
            sum[1] += row.centroid[1] * area;
            weight += area;
        }
        (weight > 0.0).then(|| [sum[0] / weight, sum[1] / weight])
    }

    /// `(lateral_min, lateral_max, anterior_min, anterior_max, asymmetry)`.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64, f64)> {
        let mut bounds = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            0.0_f64,
        );
        let mut any = false;
        for row in self.selected_loops() {
            let (lateral_low, lateral_high) = row.bounds(0);
            let (anterior_low, anterior_high) = row.bounds(1);
            bounds.0 = bounds.0.min(lateral_low);
            bounds.1 = bounds.1.max(lateral_high);
            bounds.2 = bounds.2.min(anterior_low);
            bounds.3 = bounds.3.max(anterior_high);
            bounds.4 = bounds.4.max(row.asymmetry());
            any = true;
        }
        any.then_some(bounds)
    }

    /// Shortest in-plane distance from a point to a selected ring.
    pub fn nearest_boundary(&self, point: [f64; 2]) -> Option<f64> {
        self.selected_loops()
            .flat_map(|row| row.ring.iter())
            .map(|boundary| {
                ((boundary[0] - point[0]).powi(2) + (boundary[1] - point[1]).powi(2)).sqrt()
            })
            .fold(None, |best: Option<f64>, distance| {
                Some(best.map_or(distance, |value| value.min(distance)))
            })
    }

    /// The section's centre as a 3D point in the actor's frame.
    fn centroid3(&self, frame: &Frame) -> Option<DVec3> {
        self.centroid().map(|centre| {
            frame.lateral * centre[0] + frame.anterior * centre[1] + frame.up * self.plane
        })
    }
}

/// One limb probe: the instrument's calibration on flesh that is near a cylinder.
#[derive(Clone, Debug, PartialEq)]
pub struct LimbProbe {
    pub label: String,
    pub plane: f64,
    pub bone: [f64; 3],
    pub centroid: [f64; 3],
    /// `centroid - bone` along the lateral axis, metres.
    pub lateral: f64,
    /// `centroid - bone` along the anterior axis, metres.
    pub anterior: f64,
    pub area: f64,
    pub asymmetry: f64,
}

/// The waist and the chest, located by scanning the body between its own joints.
#[derive(Clone, Debug, PartialEq)]
pub struct AnatomyLevel {
    pub label: &'static str,
    pub plane: f64,
    pub area: f64,
    pub centroid: [f64; 3],
}

/// The three records one joint carries, plus the evidence the mesh gives.
#[derive(Clone, Debug, PartialEq)]
pub struct TorsoJoint {
    pub level: &'static str,
    pub name: String,
    pub node: usize,
    pub owned: bool,
    pub pivot_before: [f64; 3],
    pub pivot_after: [f64; 3],
    /// `pivot - section lateral centre`, metres; negative is the actor's right.
    pub lateral_before: f64,
    pub lateral_after: f64,
    /// `pivot - section anterior centre`, metres; positive is forward.
    pub anterior_before: f64,
    /// Where the pivot sits between the section's anterior and posterior faces,
    /// 0 at the front.
    pub fraction_from_anterior: Option<f64>,
    /// Distance from the pivot back to the posterior skin, metres.
    pub posterior_standoff: Option<f64>,
    /// Shortest in-plane distance from the pivot to the section's boundary.
    pub nearest_boundary: Option<f64>,
    /// The anatomical estimate the mesh supports: the section's lateral centre at
    /// this joint's own height, the authored height and depth kept.
    pub anatomical_estimate: Option<[f64; 3]>,
    /// Measured uncertainty of the estimate, metres.
    pub uncertainty: f64,
    pub lateral_decision: Decision,
    pub anterior_decision: Decision,
    pub evidence: String,
}

/// The whole outcome: the instrument, the sections and the joint records.
#[derive(Clone, Debug, PartialEq)]
pub struct TorsoOutcome {
    pub frame: BodyFrame,
    pub instrument: Vec<LimbProbe>,
    /// Limb probes whose loop spans the body's midline: not a cylinder, skipped.
    pub instrument_skipped: usize,
    pub lateral_bias: f64,
    pub anterior_bias: f64,
    pub sections: Vec<Section>,
    pub anatomy: Vec<AnatomyLevel>,
    pub joints: Vec<TorsoJoint>,
    pub frames: Vec<FittedFrame>,
    pub moved_joints: usize,
    pub max_pivot_shift: f64,
    pub skin_base_spread: f64,
    pub vertex_count: usize,
}

/// Measure the torso chain, decide per axis, and return the frames the fit would
/// write.
///
/// # Errors
/// A part selection that names no unique mesh node or a primitive out of range, a
/// palette without a spine chain, or a skin whose `world * inverse_bind` is not
/// one constant matrix.
pub fn torso_fit(
    actor: &Actor,
    body: &PartSelector,
    layers: &[PartSelector],
) -> Result<TorsoOutcome, ActorError> {
    let frame = Frame::of(actor)?;
    let body_part = Selection::read(actor, body)?;
    let layer_parts = layers
        .iter()
        .map(|layer| Selection::read(actor, layer))
        .collect::<Result<Vec<_>, _>>()?;
    let worlds = super::animation::compose_worlds(actor, &actor.transforms)?;
    let binds = skin_matrices(actor, &worlds)?;
    let base = skin_base(&binds)?;
    let names = node_names(actor);
    let palette = Palette::read(&names)?;
    let pivots: BTreeMap<usize, DVec3> = palette
        .chain_rows
        .iter()
        .map(|row| (row.node, worlds[row.node].transform_point3(DVec3::ZERO)))
        .collect();

    let anatomy = body_part.anatomy_levels(&frame, &palette, &pivots);
    let mut levels: Vec<(&str, f64)> = palette
        .chain_rows
        .iter()
        .filter_map(|row| {
            pivots
                .get(&row.node)
                .map(|pivot| (row.level, pivot.dot(frame.up)))
        })
        .collect();
    levels.extend(anatomy.iter().map(|level| (level.label, level.plane)));

    let mut sections = Vec::new();
    for (level, height) in &levels {
        sections.push(
            body_part.section_torso(&frame, level, *height, &|frame, plane| {
                palette.chain_point(frame, plane, &pivots)
            }),
        );
    }
    for layer in &layer_parts {
        for (level, height) in &levels {
            sections.push(layer.section_all(&frame, level, *height));
        }
    }

    let instrument = instrument(&body_part, &frame, &palette, &worlds);
    let bias = |pick: fn(&LimbProbe) -> f64| {
        instrument
            .probes
            .iter()
            .map(|probe| pick(probe).abs())
            .fold(0.0_f64, f64::max)
    };
    let lateral_bias = bias(|probe| probe.lateral);
    let anterior_bias = bias(|probe| probe.anterior);
    let extent = extent_of(&body_part);

    let mut joints = Vec::new();
    let mut heads: BTreeMap<usize, DVec3> = BTreeMap::new();
    for row in &palette.chain_rows {
        let Some(pivot) = pivots.get(&row.node).copied() else {
            continue;
        };
        let height = pivot.dot(frame.up);
        let plane_point = [pivot.dot(frame.lateral), pivot.dot(frame.anterior)];
        let section = body_part.section_torso(&frame, row.level, height, &|_, _| Some(plane_point));
        let joint = joint_record(row, &section, &frame, pivot, anterior_bias, extent);
        if joint.lateral_decision == Decision::Fitted {
            heads.insert(joint.node, pivot - frame.lateral * joint.lateral_before);
        }
        joints.push(joint);
    }

    let frames = build_frames(
        actor,
        &names,
        &worlds,
        &binds,
        &base,
        &heads,
        &BTreeMap::new(),
    )?;
    let moved_joints = frames.iter().filter(|frame| frame.moved).count();
    let max_pivot_shift = frames
        .iter()
        .map(|frame| {
            let rest = worlds[frame.node].transform_point3(DVec3::ZERO);
            rest.distance(DVec3::from_array(frame.translation))
        })
        .fold(0.0_f64, f64::max);

    Ok(TorsoOutcome {
        frame: frame.report(),
        instrument: instrument.probes,
        instrument_skipped: instrument.skipped,
        lateral_bias,
        anterior_bias,
        sections,
        anatomy,
        joints,
        frames,
        moved_joints,
        max_pivot_shift,
        skin_base_spread: base.spread,
        vertex_count: body_part.positions.len()
            + layer_parts
                .iter()
                .map(|layer| layer.positions.len())
                .sum::<usize>(),
    })
}

/// The actor's diagonal extent, the scale a residual is judged against.
fn extent_of(selection: &Selection) -> f64 {
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for point in &selection.positions {
        let point = DVec3::from_array(*point);
        low = low.min(point);
        high = high.max(point);
    }
    (high - low).length()
}

/// One joint's three records and the decision on each axis.
fn joint_record(
    row: &ChainRow,
    section: &Section,
    frame: &Frame,
    pivot: DVec3,
    anterior_bias: f64,
    extent: f64,
) -> TorsoJoint {
    let centroid = section.centroid();
    let bounds = section.bounds();
    let plane_point = [pivot.dot(frame.lateral), pivot.dot(frame.anterior)];
    let lateral_before = centroid.map_or(0.0, |centre| plane_point[0] - centre[0]);
    let anterior_before = centroid.map_or(0.0, |centre| plane_point[1] - centre[1]);
    let asymmetry = bounds.map_or(extent, |row| row.4);
    let nearest_boundary = section.nearest_boundary(plane_point);
    // The anterior axis points forward, so `anterior_max` is the front face and
    // `anterior_min` the back one.
    let fraction_from_anterior = bounds.and_then(|(_, _, anterior_min, anterior_max, _)| {
        let depth = anterior_max - anterior_min;
        (depth > 1e-6).then(|| (anterior_max - plane_point[1]) / depth)
    });
    let posterior_standoff = bounds.map(|(_, _, anterior_min, _, _)| plane_point[1] - anterior_min);

    let lateral_decision = match &section.unresolved {
        Some(_) => Decision::Unresolved,
        None if lateral_before.abs() <= LATERAL_TOLERANCE.max(3.0 * asymmetry) => Decision::Agreed,
        None if !row.owned => Decision::Unresolved,
        None => Decision::Fitted,
    };
    // The anterior axis has no landmark on the mesh: the area centre is where the
    // soft tissue balances, and the limb probes measure how far that sits from a
    // trusted bone. Reporting the offset is honest; moving the chain by it would
    // put the pivots in the viscera.
    let anterior_decision = Decision::Unresolved;
    let anatomical_estimate =
        centroid.map(|centre| (pivot + frame.lateral * (centre[0] - plane_point[0])).to_array());
    let evidence = match (&section.unresolved, centroid) {
        (None, Some(_)) => format!(
            "{} section at {:.1} mm: area {:.2} cm2 from {} loop(s), asymmetry {:.3} mm, \
             lateral {:+.2} mm, anterior {:+.2} mm, posterior standoff {:.2} mm, \
             nearest boundary {:.2} mm",
            row.level,
            section.plane * 1000.0,
            section.area() * 1e4,
            section.selected.len(),
            asymmetry * 1000.0,
            lateral_before * 1000.0,
            anterior_before * 1000.0,
            posterior_standoff.unwrap_or(f64::NAN) * 1000.0,
            nearest_boundary.unwrap_or(f64::NAN) * 1000.0,
        ),
        (Some(reason), _) => format!("{}: {reason}", row.level),
        (None, None) => format!("{}: the selected loops enclose no area", row.level),
    };
    let uncertainty = match section.unresolved {
        None => anterior_bias,
        Some(_) => extent.max(anterior_bias),
    };
    let pivot_after = match lateral_decision {
        Decision::Fitted => pivot - frame.lateral * lateral_before,
        _ => pivot,
    };
    TorsoJoint {
        level: row.level,
        name: row.name.clone(),
        node: row.node,
        owned: row.owned,
        pivot_before: pivot.to_array(),
        pivot_after: pivot_after.to_array(),
        lateral_before,
        lateral_after: match lateral_decision {
            Decision::Fitted => 0.0,
            _ => lateral_before,
        },
        anterior_before,
        fraction_from_anterior,
        posterior_standoff,
        nearest_boundary,
        anatomical_estimate,
        uncertainty,
        lateral_decision,
        anterior_decision,
        evidence,
    }
}

/// The instrument: its probes, and the levels it could not localise.
struct Instrument {
    probes: Vec<LimbProbe>,
    skipped: usize,
}

/// Measure the area centroid of flesh that is near a cylinder, where the bone's
/// position is trusted, and report how far the centroid sits from it.
fn instrument(
    body: &Selection,
    frame: &Frame,
    palette: &Palette,
    worlds: &[glam::DMat4],
) -> Instrument {
    let mut probes = Vec::new();
    let mut skipped = 0usize;
    for (label, side, start, end) in [
        ("L thigh", 'L', "Thigh", "Calf"),
        ("R thigh", 'R', "Thigh", "Calf"),
        ("L upper arm", 'L', "UpperArm", "Forearm"),
        ("R upper arm", 'R', "UpperArm", "Forearm"),
    ] {
        let (Some(start), Some(end)) = (palette.find(side, start), palette.find(side, end)) else {
            continue;
        };
        let (Some(start), Some(end)) = (worlds.get(start), worlds.get(end)) else {
            continue;
        };
        let (start, end) = (
            start.transform_point3(DVec3::ZERO),
            end.transform_point3(DVec3::ZERO),
        );
        for fraction in [0.3_f64, 0.5, 0.7] {
            let bone = start + (end - start) * fraction;
            let plane = bone.dot(frame.up);
            let plane_point = [bone.dot(frame.lateral), bone.dot(frame.anterior)];
            let section = body.section_torso(frame, label, plane, &|_, _| Some(plane_point));
            let (Some(centroid), Some(bounds)) = (section.centroid(), section.bounds()) else {
                skipped += 1;
                continue;
            };
            // A limb that still touches the torso at this height shares the
            // torso's loop; a limb's own section never spans the midline, so a
            // probe that does is not measuring a cylinder and is not a probe.
            if bounds.0 < 0.0 && bounds.1 > 0.0 {
                skipped += 1;
                continue;
            }
            probes.push(LimbProbe {
                label: format!("{label} {:.0}%", fraction * 100.0),
                plane,
                bone: bone.to_array(),
                centroid: (frame.lateral * centroid[0]
                    + frame.anterior * centroid[1]
                    + frame.up * plane)
                    .to_array(),
                lateral: centroid[0] - plane_point[0],
                anterior: centroid[1] - plane_point[1],
                area: section.area(),
                asymmetry: bounds.4,
            });
        }
    }
    Instrument { probes, skipped }
}

/// A sampled part: its world-space vertices and its triangles, rebased so the
/// triangle indices address `positions` directly.
/// `pub(super)` fields: the shoulder seat casts rays through the same sampled
/// surface instead of sampling the body a second time.
pub(super) struct Selection {
    part: String,
    pub(super) positions: Vec<[f64; 3]>,
    pub(super) triangles: Vec<[usize; 3]>,
}

impl Selection {
    pub(super) fn read(actor: &Actor, selector: &PartSelector) -> Result<Self, ActorError> {
        let nodes = geometry::select(&actor.document, &[selector.node_name.as_str()])?;
        let worlds = super::animation::compose_worlds(actor, &actor.transforms)?;
        let sample: Sample = geometry::sample(actor, &nodes, &worlds)?;
        let part = select_part(&sample, selector).ok_or_else(|| {
            invalid(
                format!("part {:?}", selector.label()),
                "the actor does not carry that primitive",
            )
        })?;
        let base = part.vertices.start;
        Ok(Self {
            part: selector.label(),
            positions: sample.vertices[part.vertices.clone()].to_vec(),
            triangles: sample.triangles[part.triangles.clone()]
                .iter()
                .map(|triangle| triangle.map(|index| index - base))
                .collect(),
        })
    }

    /// The cut, with the loop the caller names as the one that matters. `inside`
    /// receives the plane and returns the point whose loop is the answer — the
    /// joint for a body, `None` for a garment, whose every closed loop counts.
    pub(super) fn section_torso(
        &self,
        frame: &Frame,
        level: &str,
        plane: f64,
        inside: &dyn Fn(&Frame, f64) -> Option<[f64; 2]>,
    ) -> Section {
        let mut section = self.section_all(frame, level, plane);
        if section.unresolved.is_some() {
            return section;
        }
        let Some(point) = inside(frame, plane) else {
            section.selected.clear();
            section.unresolved = Some(
                "the palette has no chain point at this height to name the torso loop".to_owned(),
            );
            return section;
        };
        let containing: Vec<usize> = section
            .loops
            .iter()
            .enumerate()
            .filter(|(_, row)| row.closed && inside_ring(&row.ring, point))
            .map(|(index, _)| index)
            .collect();
        match containing.is_empty() {
            true => {
                section.selected.clear();
                section.unresolved = Some(format!(
                    "no closed loop contains the chain point at ({:+.2}, {:+.2}) mm",
                    point[0] * 1000.0,
                    point[1] * 1000.0
                ));
            }
            false => section.selected = containing,
        }
        section
    }

    /// Every closed loop of the cut.
    pub(super) fn section_all(&self, frame: &Frame, level: &str, plane: f64) -> Section {
        let cut = cut(self, frame, plane);
        let mut section = Section {
            part: self.part.clone(),
            level: level.to_owned(),
            plane,
            segments: cut.segments,
            pinched: cut.pinched,
            loops: cut.loops,
            open_chains: cut.open_chains,
            selected: Vec::new(),
            unresolved: None,
        };
        if section.loops.is_empty() {
            section.unresolved = Some(format!(
                "the plane cuts {} triangles and closes no loop",
                section.segments
            ));
            return section;
        }
        if section.open_chains > 0 {
            section.unresolved = Some(format!(
                "the surface is not closed at this height: {} open chain(s) among {} loop(s)",
                section.open_chains,
                section.loops.len()
            ));
            return section;
        }
        section.selected = (0..section.loops.len()).collect();
        section
    }

    /// The waist and the chest: the narrowest and the widest torso cut between
    /// the chain's own joints, found by scanning so the two named levels are the
    /// body's own anatomy rather than a chosen height.
    fn anatomy_levels(
        &self,
        frame: &Frame,
        palette: &Palette,
        pivots: &BTreeMap<usize, DVec3>,
    ) -> Vec<AnatomyLevel> {
        let height = |level: &str| {
            palette
                .chain_rows
                .iter()
                .find(|row| row.level == level)
                .and_then(|row| pivots.get(&row.node))
                .map(|pivot| pivot.dot(frame.up))
        };
        let (Some(spine0), Some(spine2), Some(neck)) =
            (height("spine0"), height("spine2"), height("neck"))
        else {
            return Vec::new();
        };
        let scan =
            |low: f64, high: f64, widest: bool, label: &'static str| -> Option<AnatomyLevel> {
                let mut best: Option<(f64, Section)> = None;
                for step in 0..=16 {
                    let plane = low + (high - low) * f64::from(step) / 16.0;
                    let section = self.section_torso(frame, label, plane, &|frame, plane| {
                        palette.chain_point(frame, plane, pivots)
                    });
                    let Some(centroid) = section.centroid3(frame) else {
                        continue;
                    };
                    let area = section.area();
                    let take = match (&best, widest) {
                        (None, _) => true,
                        (Some((value, _)), true) => area > *value,
                        (Some((value, _)), false) => area < *value,
                    };
                    if take {
                        let _ = centroid;
                        best = Some((area, section));
                    }
                }
                best.map(|(area, section)| AnatomyLevel {
                    label,
                    plane: section.plane,
                    area,
                    centroid: section.centroid3(frame).unwrap_or(DVec3::ZERO).to_array(),
                })
            };
        [
            scan(spine0, spine2, false, "waist"),
            scan(spine2, neck, true, "chest"),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// One part's loops, joined through their shared edges.
struct Cut {
    segments: usize,
    pinched: usize,
    loops: Vec<SectionLoop>,
    open_chains: usize,
}

/// Cut a part at one height, link the segments into loops and measure them.
fn cut(selection: &Selection, frame: &Frame, plane: f64) -> Cut {
    let points: Vec<DVec3> = selection
        .positions
        .iter()
        .map(|point| DVec3::from_array(*point))
        .collect();
    let mut segments: Vec<Segment> = Vec::new();
    let mut counts: HashMap<(i64, i64), usize> = HashMap::new();
    for triangle in &selection.triangles {
        let [first, second, third] = [triangle[0], triangle[1], triangle[2]];
        let (Some(a), Some(b), Some(c)) =
            (points.get(first), points.get(second), points.get(third))
        else {
            continue;
        };
        let corners = [*a, *b, *c];
        let heights = corners.map(|corner| corner.dot(frame.up) - plane);
        let above = heights.iter().filter(|height| **height > 0.0).count();
        if above != 1 && above != 2 {
            continue;
        }
        let crossed: Vec<(usize, f64)> = [(0, 1), (1, 2), (2, 0)]
            .into_iter()
            .filter(|(from, to)| (heights[*from] > 0.0) != (heights[*to] > 0.0))
            .map(|(from, to)| (from, heights[from] / (heights[from] - heights[to])))
            .collect();
        let [Some((start, start_t)), Some((end, end_t))] = [crossed.first(), crossed.get(1)] else {
            continue;
        };
        let mut q0 = corners[*start] + (corners[(start + 1) % 3] - corners[*start]) * *start_t;
        let mut q1 = corners[*end] + (corners[(end + 1) % 3] - corners[*end]) * *end_t;
        let normal = (*b - *a).cross(*c - *a);
        if (q1 - q0).cross(normal).dot(frame.up) < 0.0 {
            std::mem::swap(&mut q0, &mut q1);
        }
        let start_key = key(q0, frame);
        let end_key = key(q1, frame);
        for value in [start_key, end_key] {
            *counts.entry(value).or_insert(0) += 1;
        }
        segments.push(Segment {
            start: start_key,
            end: end_key,
            start_point: q0,
            end_point: q1,
        });
    }
    let pinched = counts.values().filter(|count| **count > 2).count();
    let (loops, open_chains) = link(&segments, frame);
    Cut {
        segments: segments.len(),
        pinched,
        loops,
        open_chains,
    }
}

/// One oriented cut segment.
#[derive(Clone, Copy, Debug)]
struct Segment {
    start: (i64, i64),
    end: (i64, i64),
    start_point: DVec3,
    end_point: DVec3,
}

/// A cut point's identity: the 1 µm cell it falls in.
fn key(point: DVec3, frame: &Frame) -> (i64, i64) {
    (
        (point.dot(frame.lateral) / POINT_KEY).round() as i64,
        (point.dot(frame.anterior) / POINT_KEY).round() as i64,
    )
}

/// Walk the segments into rings through their shared edges.
fn link(segments: &[Segment], frame: &Frame) -> (Vec<SectionLoop>, usize) {
    let mut at_point: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        at_point.entry(segment.start).or_default().push(index);
        at_point.entry(segment.end).or_default().push(index);
    }
    let mut unused = vec![true; segments.len()];
    let mut loops = Vec::new();
    let mut open_chains = 0usize;
    for first in 0..segments.len() {
        if !unused[first] {
            continue;
        }
        unused[first] = false;
        let mut chain = vec![first];
        let head = segments[first].start;
        let mut tail = segments[first].end;
        let mut closed = head == tail;
        while !closed {
            let Some(next) = at_point
                .get(&tail)
                .and_then(|owners| owners.iter().copied().find(|index| unused[*index]))
            else {
                break;
            };
            unused[next] = false;
            chain.push(next);
            tail = match segments[next].start == tail {
                true => segments[next].end,
                false => segments[next].start,
            };
            closed = tail == head;
        }
        if !closed {
            open_chains += 1;
            // Extend backwards too, so the reported chain is whole.
            let mut before = head;
            while let Some(previous) = at_point
                .get(&before)
                .and_then(|owners| owners.iter().copied().find(|index| unused[*index]))
            {
                unused[previous] = false;
                chain.push(previous);
                before = match segments[previous].end == before {
                    true => segments[previous].start,
                    false => segments[previous].end,
                };
            }
        }
        loops.push(measure_loop(segments, &chain, closed, frame));
    }
    (loops, open_chains)
}

/// Area, centroid and ring of one chain of segments.
fn measure_loop(segments: &[Segment], chain: &[usize], closed: bool, frame: &Frame) -> SectionLoop {
    let mut ring: Vec<[f64; 2]> = Vec::with_capacity(chain.len());
    let mut area = 0.0;
    let mut sum = [0.0_f64; 2];
    let mut cursor = segments[chain[0]].start_point;
    for index in chain {
        let segment = segments[*index];
        let (from, to) = match segment.start_point.distance(cursor) < 1e-9 {
            true => (segment.start_point, segment.end_point),
            false => (segment.end_point, segment.start_point),
        };
        let (u0, v0) = (from.dot(frame.lateral), from.dot(frame.anterior));
        let (u1, v1) = (to.dot(frame.lateral), to.dot(frame.anterior));
        ring.push([u0, v0]);
        let term = u0 * v1 - u1 * v0;
        area += term;
        sum[0] += (u0 + u1) * term;
        sum[1] += (v0 + v1) * term;
        cursor = to;
    }
    area *= 0.5;
    let centroid = match area.abs() > 1e-12 {
        true => [sum[0] / (6.0 * area), sum[1] / (6.0 * area)],
        false => [f64::NAN, f64::NAN],
    };
    SectionLoop {
        area,
        centroid,
        ring,
        segments: chain.len(),
        closed,
    }
}

/// Whether a point lies inside a ring, by winding number.
fn inside_ring(ring: &[[f64; 2]], point: [f64; 2]) -> bool {
    let mut winding = 0i32;
    for index in 0..ring.len() {
        let from = ring[index];
        let to = ring[(index + 1) % ring.len()];
        let cross =
            (to[0] - from[0]) * (point[1] - from[1]) - (point[0] - from[0]) * (to[1] - from[1]);
        if from[1] <= point[1] && point[1] < to[1] && cross > 0.0 {
            winding += 1;
        } else if to[1] <= point[1] && point[1] < from[1] && cross < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

/// The actor's own frame.
///
/// `pub(super)` because the shoulder seat measures against the same axes: one
/// owner for "which way is up" keeps the two verbs' sections comparable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frame {
    pub(super) up: DVec3,
    pub(super) lateral: DVec3,
    pub(super) anterior: DVec3,
    pub(super) anterior_source: &'static str,
}

impl Frame {
    /// Up is glTF's Y; the anterior axis is the direction the feet point (ankle
    /// to ball), the only authored forward the palette carries.
    pub(super) fn of(actor: &Actor) -> Result<Self, ActorError> {
        let names = node_names(actor);
        let palette = Palette::read(&names)?;
        let worlds = super::animation::compose_worlds(actor, &actor.transforms)?;
        let mut sum = DVec3::ZERO;
        for side in ['L', 'R'] {
            let (Some(ankle), Some(ball)) =
                (palette.find(side, "Foot"), palette.find(side, "Toe0"))
            else {
                continue;
            };
            sum += worlds[ball].transform_point3(DVec3::ZERO)
                - worlds[ankle].transform_point3(DVec3::ZERO);
        }
        let up = DVec3::Y;
        let horizontal = (sum - up * sum.dot(up)).normalize_or_zero();
        let (anterior, anterior_source) = match horizontal.length() > 0.5 {
            true => (horizontal, "feet"),
            false => (-DVec3::Z, "world"),
        };
        Ok(Self {
            up,
            lateral: anterior.cross(up).normalize_or(DVec3::X),
            anterior,
            anterior_source,
        })
    }

    pub(super) fn report(self) -> BodyFrame {
        BodyFrame {
            up: self.up.to_array(),
            lateral: self.lateral.to_array(),
            anterior: self.anterior.to_array(),
            anterior_source: self.anterior_source,
        }
    }
}

/// One palette joint of the torso chain.
struct ChainRow {
    level: &'static str,
    name: String,
    node: usize,
    owned: bool,
}

/// The palette's torso joints, and every named joint the instrument needs.
struct Palette {
    chain_rows: Vec<ChainRow>,
    named: Vec<(char, String, usize)>,
}

impl Palette {
    fn read(names: &[Option<String>]) -> Result<Self, ActorError> {
        let named: Vec<(char, String, usize)> = names
            .iter()
            .enumerate()
            .filter_map(|(node, name)| {
                let (side, base) = name.as_deref().and_then(parse_name)?;
                Some((side, base.to_owned(), node))
            })
            .collect();
        let chain_rows: Vec<ChainRow> = TORSO_CHAIN
            .iter()
            .filter_map(|bone| {
                let (_, _, node) = named
                    .iter()
                    .find(|(side, base, _)| *side == 'C' && base == bone.name)?;
                Some(ChainRow {
                    level: bone.level,
                    name: names[*node].clone().unwrap_or_else(|| bone.name.to_owned()),
                    node: *node,
                    owned: bone.owned,
                })
            })
            .collect();
        if chain_rows.len() < 3 {
            return Err(invalid(
                "actor torso-fit",
                format!(
                    "the palette names {} torso chain joint(s); the fit needs the spine chain \
                     (`NPC Pelvis`, `NPC Spine`, `NPC Spine1`, `NPC Spine2`, `NPC Neck`)",
                    chain_rows.len()
                ),
            ));
        }
        Ok(Self { chain_rows, named })
    }

    fn find(&self, side: char, base: &str) -> Option<usize> {
        self.named
            .iter()
            .find(|(found_side, found, _)| *found_side == side && found == base)
            .map(|(_, _, node)| *node)
    }

    /// The chain's own position at a height: linear between the two joints the
    /// plane falls between, extrapolated from the end pair outside them. This is
    /// the point whose loop is the torso at that height, not a placement claim.
    fn chain_point(
        &self,
        frame: &Frame,
        plane: f64,
        pivots: &BTreeMap<usize, DVec3>,
    ) -> Option<[f64; 2]> {
        let chain: Vec<DVec3> = self
            .chain_rows
            .iter()
            .filter_map(|row| pivots.get(&row.node).copied())
            .collect();
        if chain.len() < 2 {
            return None;
        }
        let point = |pivot: DVec3| [pivot.dot(frame.lateral), pivot.dot(frame.anterior)];
        for pair in chain.windows(2) {
            let (low, high) = (
                pair[0].dot(frame.up).min(pair[1].dot(frame.up)),
                pair[0].dot(frame.up).max(pair[1].dot(frame.up)),
            );
            if plane >= low && plane <= high {
                let span = high - low;
                let fraction = match span > 1e-9 {
                    true => (plane - low) / span,
                    false => 0.5,
                };
                let (first, second) = match pair[0].dot(frame.up) <= pair[1].dot(frame.up) {
                    true => (pair[0], pair[1]),
                    false => (pair[1], pair[0]),
                };
                let point = point(first + (second - first) * fraction);
                return Some(point);
            }
        }
        let (first, last) = (chain[0], chain[chain.len() - 1]);
        Some(point(if plane < first.dot(frame.up) {
            first
        } else {
            last
        }))
    }
}

/// The part of a sample a selector names.
pub(super) fn select_part<'a>(sample: &'a Sample, selector: &PartSelector) -> Option<&'a Part> {
    sample.parts.iter().find(|part| {
        part.name == selector.node_name
            && selector
                .primitive
                .is_none_or(|primitive| part.primitive == primitive)
    })
}
