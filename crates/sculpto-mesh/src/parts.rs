//! Read-only audit of attached body-part placement against their sockets.
//!
//! A multi-part body OBJ (a torso shell plus attached part shells, as produced
//! by the native pipeline's blind four-file concatenation) is measured, never
//! modified: every attached part is paired with the shell it points at, the
//! seam is classified as coincident or slotted, and slot, coverage and
//! coincidence residuals are reported per part. A boundary chain that no part
//! points at is reported as an open socket, not a failure: a kit body that
//! ships no head, arms or feet is described plainly and accepted.
//!
//! Thresholds come from the measured exact-Bijin and 3BA fixtures (BLD-11
//! researcher OUTPUT, 2026-11); they are facts, not tunables. Correction stays
//! with `component.seam_fit` as an opt-in follow-up: this module refuses, it
//! never moves a vertex.
//!
//! Distances are vertex-to-vertex in the input frame, which preserves the
//! researcher's OBJ-frame measurements exactly under the engine's
//! display-to-source rotation. Height bands use the detected up axis
//! ([`crate::aura::up_axis`]), so Y-up and Z-up inputs classify the same way.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use crate::aura::{extent, up_axis};
use crate::classify::component_groups;
use crate::point_query::PointIndex;

/// Coincidence scale: float noise of 6-decimal OBJ coordinates.
pub const COINCIDENT_EPS: f64 = 0.02;
/// Authored-coincident seam classification band; one fit tolerance (~2 mm).
/// A 3° seam roll displaces ring vertices ~0.05–0.09: still classified
/// coincident, but counted zero at [`COINCIDENT_EPS`].
pub const SEAM_CLASSIFY_EPS: f64 = 0.15;
/// Head slot floor; authored exact-Bijin head slot minimum is 0.7981.
pub const HEAD_SLOT_MIN: f64 = 0.70;
/// Head slot ceiling; the authored head must not sink into the socket.
pub const HEAD_SLOT_MAX: f64 = 1.20;
/// Head coverage ceiling above the authored baseline (researcher rule 2).
pub const HEAD_COVERAGE_SLACK: f64 = 0.10;
/// Foot slot floor; authored exact-Bijin shell minimum is 0.4907.
pub const FOOT_SLOT_MIN: f64 = 0.44;
/// A foot slotted seam is relevant within this band (authored 0.49..2.96).
pub const FOOT_SLOTTED_BAND: f64 = 3.0;
/// Socket-arc selection radius around the closest ring-to-ring point: the
/// near arc of the exact-Bijin neck ring sits within ~5 of that point while
/// the torso's next ring starts ~23 away, and a second ring 7 units off is
/// already another socket.
pub const SOCKET_PROXIMITY: f64 = 6.0;
/// A boundary chain with no other shell's ring vertex within this band is an
/// open socket: authored seams sit at 0.0..0.8, the kit micro ring's nearest
/// leg ring vertex is ~2.9 away, and the kit's unused neck ring is 31.7 from
/// the nearest part vertex.
pub const OPEN_SOCKET_BAND: f64 = 2.0;
/// Shells below this size are loose fragments (the exact-Bijin eye-region
/// fragments are 3-vertex triangles), not audited parts; the smallest real
/// part is a 169-vertex toe cap.
pub const MIN_PART_VERTICES: usize = 10;
/// Head/foot bands: this fraction of the torso's height extent from its top
/// and bottom. The authored neck ring sits in the top band, ankle arcs in the
/// bottom band.
pub const HEIGHT_BAND: f64 = 0.15;
/// An override part file matches a shipped part by ring-centroid distance;
/// measured witness transforms displace ring centroids by at most ~2.
pub const OVERRIDE_MATCH_MAX: f64 = 6.0;

/// How a part meets its socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamKind {
    /// Ring vertices sit on the socket shell (authored distance 0.0000).
    Coincident,
    /// The ring hovers in a socket opening (authored 0.49..4.16).
    Slotted,
}

impl SeamKind {
    /// Stable wire string.
    pub fn name(self) -> &'static str {
        match self {
            Self::Coincident => "coincident",
            Self::Slotted => "slotted",
        }
    }
}

/// Coarse anatomical class used for naming and rule selection. The class is a
/// measured heuristic (height band plus seam correspondence), not a declared
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartClass {
    /// The socket owner: largest bounding-box volume.
    Torso,
    /// Ring centroid in the torso's top band.
    Head,
    /// Coincident seam onto the torso.
    Leg,
    /// Bottom band with a slotted seam within [`FOOT_SLOTTED_BAND`].
    Foot,
    /// Bottom band, coincident-only, large ring.
    Sole,
    /// Bottom band, coincident-only, small ring.
    ToeCap,
    /// Slotted-only seam, mid band.
    Arm,
    /// Anything else attached.
    Attached,
}

impl PartClass {
    /// Stable wire string.
    pub fn name(self) -> &'static str {
        match self {
            Self::Torso => "torso",
            Self::Head => "head",
            Self::Leg => "leg",
            Self::Foot => "foot",
            Self::Sole => "sole",
            Self::ToeCap => "toe cap",
            Self::Arm => "arm",
            Self::Attached => "attached",
        }
    }
}

/// Typed refusal codes; the caller decides from the code, never the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    /// Head slot minimum outside the measured window.
    HeadSlot,
    /// Head coverage above the authored baseline plus slack.
    HeadCoverage,
    /// Foot slot minimum below the measured floor.
    FootSlot,
    /// An authored-coincident seam has no coincident ring vertices left.
    SeamOpen,
}

impl RefusalCode {
    /// Stable wire code.
    pub fn wire_code(self) -> &'static str {
        match self {
            Self::HeadSlot => "part_head_slot",
            Self::HeadCoverage => "part_head_coverage",
            Self::FootSlot => "part_foot_slot",
            Self::SeamOpen => "part_seam_open",
        }
    }
}

/// One named refusal with the measured value; nothing is corrected. @req BLD-11
#[derive(Debug, Clone, PartialEq)]
pub struct PartRefusal {
    /// The refusal kind.
    pub code: RefusalCode,
    /// The audited part, by report name.
    pub part: String,
    /// The socket the measurement is against.
    pub socket: String,
    /// The measured value: a distance, a coverage, or the surviving count.
    pub observed: f64,
    /// The bound the value is measured against, rendered for the report.
    pub bound: String,
}

impl fmt::Display for PartRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            RefusalCode::HeadSlot => write!(
                formatter,
                "{}: slot_min {:.4} outside [{}, {}] (socket {})",
                self.part, self.observed, HEAD_SLOT_MIN, HEAD_SLOT_MAX, self.socket
            ),
            RefusalCode::HeadCoverage | RefusalCode::FootSlot => write!(
                formatter,
                "{}: {} {:.4} {} (socket {})",
                self.part,
                self.metric_name(),
                self.observed,
                self.bound,
                self.socket
            ),
            RefusalCode::SeamOpen => write!(
                formatter,
                "{}: {} of {} ring vertices coincide with {} (within {})",
                self.part, self.observed, self.bound, self.socket, COINCIDENT_EPS
            ),
        }
    }
}

impl PartRefusal {
    /// Metric noun for the Display line.
    fn metric_name(&self) -> &'static str {
        match self.code {
            RefusalCode::HeadCoverage => "coverage",
            _ => "slot_min",
        }
    }
}

/// One measured seam between a part and a socket shell.
#[derive(Debug, Clone, PartialEq)]
pub struct SeamRow {
    /// The socket shell, by report name.
    pub socket: String,
    /// Coincident or slotted correspondence.
    pub kind: SeamKind,
    /// Part-side ring vertices in this seam's part.
    pub ring: usize,
    /// Selected socket-side arc vertices.
    pub socket_ring: usize,
    /// Nearest shell-to-shell vertex distance.
    pub slot_min: f64,
    /// Nearest and farthest part-ring to socket-shell vertex distances.
    pub ring_min: f64,
    pub ring_max: f64,
    /// Part-ring vertices within [`COINCIDENT_EPS`] of the socket shell.
    pub coincident: usize,
    /// Farthest selected socket-arc vertex to part-ring distance.
    pub coverage: f64,
    /// The part's strongest coincident correspondence (most near vertices);
    /// the seam-open rule fires here, so incidental proximity between family
    /// shells never refuses.
    pub primary: bool,
}

/// One audited attached part.
#[derive(Debug, Clone, PartialEq)]
pub struct PartRow {
    /// Report name, e.g. `head`, `foot L`, `toe cap R2`.
    pub name: String,
    /// Measured class behind the name.
    pub class: PartClass,
    /// Shell vertex count.
    pub vertices: usize,
    /// Metrics measured on an override part file instead of the shipped shell.
    pub overridden: bool,
    /// One row per corresponding seam.
    pub seams: Vec<SeamRow>,
    /// `ok`, or the wire code of the refusal that fired.
    pub verdict: &'static str,
}

/// A boundary chain no part points at: the socket exists, the part does not.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenSocket {
    /// The shell owning the chain, by report name.
    pub component: String,
    /// Chain vertex count.
    pub ring: usize,
    /// Chain centroid in the input frame.
    pub centroid: [f64; 3],
}

/// The whole audit: measured rows, open sockets and typed refusals.
#[derive(Debug, Clone, PartialEq)]
pub struct PartAudit {
    /// Connected components in the input index graph, fragments included.
    pub components: usize,
    /// Shells large enough to be audited parts (≥ [`MIN_PART_VERTICES`]).
    pub shells: usize,
    /// The socket owner's report name, when one exists.
    pub torso: Option<String>,
    /// One row per attached part.
    pub parts: Vec<PartRow>,
    /// Boundary chains with no attached part.
    pub open_sockets: Vec<OpenSocket>,
    /// Every measured threshold violation; empty means the body accepts.
    pub refusals: Vec<PartRefusal>,
}

impl PartAudit {
    /// `accept` while no refusal fired.
    pub fn verdict(&self) -> &'static str {
        if self.refusals.is_empty() {
            "accept"
        } else {
            "refuse"
        }
    }
}

/// A replacement part to audit against the body's own sockets; the shipped
/// shell stays the measurement baseline. One file is one part.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartOverride<'a> {
    /// Positions of the replacement part.
    pub positions: &'a [[f32; 3]],
    /// Triangles of the replacement part.
    pub triangles: &'a [[u32; 3]],
}

/// Invalid geometry or an unmatched replacement; inputs are never mutated.
#[derive(Debug, Clone, PartialEq)]
pub enum PartsError {
    /// Positions or triangles are empty.
    EmptyInput,
    /// A position contains a nonfinite coordinate.
    NonFinite { vertex: usize },
    /// A triangle refers outside the positions.
    BadIndex { triangle: usize, vertex: usize },
    /// An override part matches no shipped part ring.
    UnmatchedOverride { index: usize, nearest: f64 },
    /// Two override parts target the same shipped part.
    OverrideConflict { index: usize, part: String },
}

impl fmt::Display for PartsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => formatter.write_str("positions and triangles must be nonempty"),
            Self::NonFinite { vertex } => {
                write!(formatter, "nonfinite position at vertex {vertex}")
            }
            Self::BadIndex { triangle, vertex } => write!(
                formatter,
                "triangle {triangle} references vertex {vertex} outside the source"
            ),
            Self::UnmatchedOverride { index, nearest } => write!(
                formatter,
                "override part {index} matches no shipped part ring (nearest {nearest:.4} > {OVERRIDE_MATCH_MAX})"
            ),
            Self::OverrideConflict { index, part } => write!(
                formatter,
                "override part {index} targets {part}, which is already replaced"
            ),
        }
    }
}

impl std::error::Error for PartsError {}

/// One connected shell with its boundary chains, in input frame coordinates.
struct Shell {
    /// Global vertex indices, ascending.
    vertices: Vec<usize>,
    /// Global-to-local map for position lookups by global id.
    local: HashMap<usize, usize>,
    /// Materialized positions, parallel to `vertices`.
    positions: Vec<[f32; 3]>,
    /// Global indices of boundary vertices.
    boundary: Vec<usize>,
    /// Boundary chains, largest first.
    chains: Vec<Chain>,
}

/// One connected run of boundary vertices.
struct Chain {
    /// Global vertex indices.
    vertices: Vec<usize>,
    /// Chain centroid in input frame coordinates.
    centroid: [f64; 3],
}

impl Shell {
    /// Build one shell from `members` (global vertex ids) and its own
    /// triangles; boundary edges are the ones exactly one triangle uses.
    fn new(positions: &[[f32; 3]], triangles: &[[u32; 3]], members: &[usize]) -> Option<Self> {
        if members.is_empty() {
            return None;
        }
        let local: HashMap<usize, usize> = members
            .iter()
            .enumerate()
            .map(|(at, &global)| (global, at))
            .collect();
        let own: Vec<[u32; 3]> = triangles
            .iter()
            .copied()
            .filter(|triangle| {
                triangle
                    .iter()
                    .all(|&vertex| local.contains_key(&(vertex as usize)))
            })
            .collect();
        if own.is_empty() {
            return None;
        }
        let mut counts: HashMap<(u32, u32), u32> = HashMap::new();
        for [a, b, c] in &own {
            for (left, right) in [(a, b), (b, c), (c, a)] {
                let key = if left <= right {
                    (*left, *right)
                } else {
                    (*right, *left)
                };
                *counts.entry(key).or_default() += 1;
            }
        }
        let mut adjacency: HashMap<u32, Vec<u32>> = HashMap::new();
        for ((left, right), count) in &counts {
            if *count == 1 {
                adjacency.entry(*left).or_default().push(*right);
                adjacency.entry(*right).or_default().push(*left);
            }
        }
        let mut chains: Vec<Chain> = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        for start in adjacency.keys().copied().collect::<Vec<_>>() {
            if !visited.insert(start) {
                continue;
            }
            let mut walk = VecDeque::from([start]);
            let mut vertices: Vec<usize> = Vec::new();
            while let Some(current) = walk.pop_front() {
                vertices.push(current as usize);
                for neighbor in adjacency.get(&current).into_iter().flatten() {
                    if visited.insert(*neighbor) {
                        walk.push_back(*neighbor);
                    }
                }
            }
            let centroid = mean_over(&vertices, |&global| positions[global]);
            chains.push(Chain { vertices, centroid });
        }
        chains.sort_by_key(|chain| std::cmp::Reverse(chain.vertices.len()));
        let boundary: Vec<usize> = chains
            .iter()
            .flat_map(|chain| chain.vertices.iter().copied())
            .collect();
        let materialized: Vec<[f32; 3]> = members.iter().map(|&vertex| positions[vertex]).collect();
        Some(Self {
            vertices: members.to_vec(),
            local,
            positions: materialized,
            boundary,
            chains,
        })
    }

    /// Position of one global vertex of this shell.
    fn position_of(&self, global: usize) -> [f32; 3] {
        self.positions[self.local[&global]]
    }

    /// Ring centroid in input frame coordinates.
    fn ring_centroid(&self) -> [f64; 3] {
        let ring: Vec<[f32; 3]> = self
            .boundary
            .iter()
            .map(|&vertex| self.position_of(vertex))
            .collect();
        mean_positions(ring.as_slice())
    }
}

/// Per-axis mean of the given positions.
fn mean_positions(positions: &[[f32; 3]]) -> [f64; 3] {
    let sum = positions.iter().fold([0.0; 3], |sum, p| {
        [
            sum[0] + f64::from(p[0]),
            sum[1] + f64::from(p[1]),
            sum[2] + f64::from(p[2]),
        ]
    });
    let count = positions.len().max(1) as f64;
    [sum[0] / count, sum[1] / count, sum[2] / count]
}

/// Per-axis mean of the positions `pick` selects by index.
fn mean_over<P>(indices: &[usize], pick: P) -> [f64; 3]
where
    P: Fn(&usize) -> [f32; 3],
{
    let picked: Vec<[f32; 3]> = indices.iter().map(pick).collect();
    mean_positions(picked.as_slice())
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (f64::from(*a) - f64::from(b)).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn distance64(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn validate(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<(), PartsError> {
    if positions.is_empty() || triangles.is_empty() {
        return Err(PartsError::EmptyInput);
    }
    if let Some(vertex) = positions
        .iter()
        .position(|position| position.iter().any(|value| !value.is_finite()))
    {
        return Err(PartsError::NonFinite { vertex });
    }
    if let Some((triangle, vertex)) = triangles.iter().enumerate().find_map(|(index, t)| {
        t.iter()
            .find(|&&vertex| vertex as usize >= positions.len())
            .map(|&vertex| (index, vertex as usize))
    }) {
        return Err(PartsError::BadIndex { triangle, vertex });
    }
    Ok(())
}

/// Bounding-box volume of a shell; the socket-owner heuristic. The trunk's
/// volume dominates every limb shell on measured bodies (exact-Bijin torso
/// ~40×49×24 against ~8×42×8 arms).
fn volume(positions: &[[f32; 3]]) -> f64 {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(f64::from(position[axis]));
            max[axis] = max[axis].max(f64::from(position[axis]));
        }
    }
    (max[0] - min[0]) * (max[1] - min[1]) * (max[2] - min[2])
}

/// Cheap per-partner correspondence summary, gathered before seams are named.
struct PartnerSummary {
    partner: usize,
    slot_min: f64,
    ring_min: f64,
    ring_max: f64,
    /// Ring vertices within [`COINCIDENT_EPS`] of the partner shell.
    coincident: usize,
    /// Ring vertices within [`SEAM_CLASSIFY_EPS`]: the classification band.
    near: usize,
}

/// One part's measured class and seams, before refusal evaluation.
struct PartAnalysis {
    /// Shell index in the body.
    shell: usize,
    /// Measured class from band and seam structure.
    class: PartClass,
    /// Partner correspondence summaries, one per large partner shell.
    summaries: Vec<PartnerSummary>,
    /// One row per corresponding seam, sockets already named.
    seams: Vec<SeamRow>,
}

/// The body's shells with their nearest-vertex trees, shared by every analysis.
struct Bodies<'a> {
    shells: &'a [Shell],
    /// Ring positions per shell, stable so the ring trees can borrow them.
    ring_points: &'a [Vec<[f32; 3]>],
    shell_trees: &'a [PointIndex<'a>],
    ring_trees: &'a [Option<PointIndex<'a>>],
    /// Shell indices large enough to be parts or sockets.
    large: &'a [usize],
}

/// Audit a multi-part body against its own sockets. Read-only: nothing is
/// written and no vertex moves. @req BLD-11
///
/// # Errors
/// Refuses empty, nonfinite or out-of-range geometry, and override parts that
/// match no shipped part ring or collide with another override.
pub fn audit(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    overrides: &[PartOverride<'_>],
) -> Result<PartAudit, PartsError> {
    validate(positions, triangles)?;
    for override_part in overrides {
        validate(override_part.positions, override_part.triangles)?;
    }
    let groups = component_groups(positions, triangles);
    let shells: Vec<Shell> = groups
        .iter()
        .filter_map(|group| Shell::new(positions, triangles, group))
        .collect();
    let large: Vec<usize> = shells
        .iter()
        .enumerate()
        .filter(|(_, shell)| shell.vertices.len() >= MIN_PART_VERTICES)
        .map(|(index, _)| index)
        .collect();

    // The socket owner is the trunk shell every attached part points at; its
    // bounding-box volume dominates every limb shell on measured bodies.
    let torso = large.iter().copied().max_by(|&left, &right| {
        volume(shells[left].positions.as_slice())
            .total_cmp(&volume(shells[right].positions.as_slice()))
            .then_with(|| {
                shells[left]
                    .vertices
                    .len()
                    .cmp(&shells[right].vertices.len())
            })
    });
    let axis = up_axis(positions);
    let bands = torso.map(|index| {
        let (min, max) = extent(shells[index].positions.as_slice(), axis);
        let band = f64::from(max - min) * HEIGHT_BAND;
        (f64::from(min), f64::from(max), band)
    });

    let ring_points: Vec<Vec<[f32; 3]>> = shells.iter().map(Shell::ring_positions_owned).collect();
    let shell_trees: Vec<PointIndex<'_>> = shells
        .iter()
        .map(|shell| PointIndex::build(shell.positions.as_slice()))
        .collect();
    let ring_trees: Vec<Option<PointIndex<'_>>> = ring_points
        .iter()
        .map(|ring| match ring.is_empty() {
            true => None,
            false => Some(PointIndex::build(ring.as_slice())),
        })
        .collect();
    let bodies = Bodies {
        shells: shells.as_slice(),
        ring_points: ring_points.as_slice(),
        shell_trees: shell_trees.as_slice(),
        ring_trees: ring_trees.as_slice(),
        large: large.as_slice(),
    };

    // Per-part correspondence search against every large shell; loose
    // fragments are never sockets, but still cover boundary chains below.
    let mut analyses: Vec<PartAnalysis> = Vec::new();
    for &index in &large {
        if Some(index) == torso {
            continue;
        }
        let summaries: Vec<PartnerSummary> = bodies
            .large
            .iter()
            .copied()
            .filter(|&other| other != index)
            .map(|partner| {
                summarize(
                    partner,
                    bodies.shells[index].positions.as_slice(),
                    bodies.ring_points[index].as_slice(),
                    bodies.shells[partner].positions.as_slice(),
                    &bodies.shell_trees[partner],
                )
            })
            .collect();
        analyses.push(PartAnalysis {
            shell: index,
            class: PartClass::Attached,
            summaries,
            seams: Vec::new(),
        });
    }

    // Coincident families: parts joined by coincident seams share one, so the
    // foot family's own members never masquerade as each other's sockets.
    let mut union: Families = large.iter().map(|&index| (index, index)).collect();
    for analysis in &analyses {
        for summary in analysis.summaries.iter().filter(|summary| summary.near > 0) {
            let left = family_root(&union, analysis.shell);
            let right = family_root(&union, summary.partner);
            if left != right {
                union.insert(right, left);
            }
        }
    }
    let families: Families = union
        .keys()
        .copied()
        .map(|shell| (shell, family_root(&union, shell)))
        .collect();
    let layout = Layout {
        torso,
        axis,
        bands,
        families: &families,
    };

    for analysis in &mut analyses {
        analysis.class = classify(
            bodies.shells[analysis.shell].ring_centroid(),
            bodies.shells[analysis.shell].boundary.len(),
            analysis.summaries.as_slice(),
            families[&analysis.shell],
            &layout,
        );
    }

    // Names come after classification: bilateral classes take an L/R suffix
    // from the ring centroid's offset against the torso centroid (L marks the
    // +X side), and repeated class names gain a counter.
    let mut names: Vec<String> = vec![String::new(); shells.len()];
    if let Some(torso_index) = torso {
        names[torso_index] = "torso".to_string();
    }
    let torso_center =
        torso.map(|index| mean_over(&shells[index].vertices, |&global| positions[global]));
    let mut pool = NamePool::default();
    for analysis in &analyses {
        names[analysis.shell] = pool.assign(
            bodies.shells[analysis.shell].ring_centroid(),
            analysis.class,
            axis,
            torso_center,
        );
    }
    for analysis in &mut analyses {
        analysis.seams = seam_rows(
            analysis.summaries.as_slice(),
            families[&analysis.shell],
            &families,
            &bodies,
            names.as_slice(),
            bodies.ring_points[analysis.shell].as_slice(),
        );
    }

    let mut report = PartAudit {
        components: groups.len(),
        shells: large.len(),
        torso: torso.map(|index| names[index].clone()),
        parts: Vec::new(),
        open_sockets: Vec::new(),
        refusals: Vec::new(),
    };
    let mut row_of_shell: HashMap<usize, usize> = HashMap::new();
    for analysis in &analyses {
        let name = names[analysis.shell].clone();
        let (verdict, refusals) =
            refusals_for(analysis.class, &name, &analysis.seams, &analysis.seams);
        row_of_shell.insert(analysis.shell, report.parts.len());
        report.parts.push(PartRow {
            name,
            class: analysis.class,
            vertices: bodies.shells[analysis.shell].vertices.len(),
            overridden: false,
            seams: analysis.seams.clone(),
            verdict,
        });
        report.refusals.extend(refusals);
    }

    // Open sockets: boundary chains of large shells that no other shell's ring
    // approaches. Loose fragments still count as coverers, so the exact-Bijin
    // eye ring stays quiet while the kit's unused neck ring is named.
    for &index in &large {
        let component = names[index].clone();
        for chain in &shells[index].chains {
            let covered = shells.iter().enumerate().any(|(other, _)| {
                other != index
                    && ring_trees[other].as_ref().is_some_and(|ring| {
                        chain.vertices.iter().any(|&vertex| {
                            let point = shells[index].position_of(vertex);
                            !ring
                                .within_radius(point, OPEN_SOCKET_BAND as f32)
                                .is_empty()
                        })
                    })
            });
            if !covered {
                report.open_sockets.push(OpenSocket {
                    component: component.clone(),
                    ring: chain.vertices.len(),
                    centroid: chain.centroid,
                });
            }
        }
    }

    apply_overrides(
        overrides,
        &bodies,
        &families,
        names.as_slice(),
        &mut report,
        &row_of_shell,
    )?;
    Ok(report)
}

/// Ring-centroid nearest distances from one shell to a partner shell, plus the
/// coincident and near counts the seam classification needs.
fn summarize(
    partner: usize,
    part_positions: &[[f32; 3]],
    ring_positions: &[[f32; 3]],
    partner_positions: &[[f32; 3]],
    partner_tree: &PointIndex<'_>,
) -> PartnerSummary {
    let slot_min = part_positions
        .iter()
        .map(|&position| {
            let nearest = partner_tree.nearest(position);
            distance(position, partner_positions[nearest])
        })
        .fold(f64::INFINITY, f64::min);
    let mut ring_min = f64::INFINITY;
    let mut ring_max = 0.0f64;
    let mut coincident = 0usize;
    let mut near = 0usize;
    for &position in ring_positions {
        let nearest = partner_tree.nearest(position);
        let candidate = distance(position, partner_positions[nearest]);
        ring_min = ring_min.min(candidate);
        ring_max = ring_max.max(candidate);
        coincident += (candidate <= COINCIDENT_EPS) as usize;
        near += (candidate <= SEAM_CLASSIFY_EPS) as usize;
    }
    PartnerSummary {
        partner,
        slot_min,
        ring_min: ring_min.min(slot_min),
        ring_max,
        coincident,
        near,
    }
}

/// Family membership over shell indices: parts joined by coincident seams
/// share a family (the foot with its sole and toe caps), so a family member's
/// proximity never masquerades as another part's ankle slot.
type Families = HashMap<usize, usize>;

/// Union-find root over the family map.
fn family_root(families: &Families, mut shell: usize) -> usize {
    while let Some(&parent) = families.get(&shell) {
        if parent == shell {
            return shell;
        }
        shell = parent;
    }
    shell
}

/// Shared layout context for classification: the socket owner, the detected up
/// axis with the torso's height bands, and the coincident families.
struct Layout<'a> {
    torso: Option<usize>,
    axis: usize,
    bands: Option<(f64, f64, f64)>,
    families: &'a Families,
}

/// Class from height band and seam structure; the checks run in the documented
/// order so a coincident leg never falls into the bottom-band foot rules and
/// the head wins its band before the shoulder-slot arm rule.
fn classify(
    ring_centroid: [f64; 3],
    ring_size: usize,
    summaries: &[PartnerSummary],
    own_family: usize,
    layout: &Layout<'_>,
) -> PartClass {
    let Layout {
        torso,
        axis,
        bands,
        families,
    } = layout;
    // 1. Coincident onto the torso: a leg.
    if torso.is_some_and(|torso| {
        summaries
            .iter()
            .any(|summary| summary.partner == torso && summary.near > 0)
    }) {
        return PartClass::Leg;
    }
    let Some((min, max, band)) = bands else {
        return PartClass::Attached;
    };
    let height = ring_centroid[*axis];
    // 2. Top band: the head.
    if height >= max - band {
        return PartClass::Head;
    }
    // 3. Nearest cross-family slotted partner: the ankle slot reaches a leg,
    //    the shoulder slot reaches the torso.
    let slotted = summaries
        .iter()
        .filter(|summary| {
            summary.near == 0
                && families
                    .get(&summary.partner)
                    .copied()
                    .map(|family| family_root(families, family))
                    != Some(own_family)
        })
        .min_by(|left, right| left.slot_min.total_cmp(&right.slot_min));
    if let Some(slotted) = slotted {
        if Some(slotted.partner) == *torso {
            return PartClass::Arm;
        }
        if height <= min + band && slotted.slot_min <= FOOT_SLOTTED_BAND {
            return PartClass::Foot;
        }
    }
    // 4. Bottom band without a near cross-family slot: the coincident family
    //    under a foot.
    if height <= min + band {
        return match ring_size >= 100 {
            true => PartClass::Sole,
            false => PartClass::ToeCap,
        };
    }
    PartClass::Attached
}

/// Full seam rows: every classified-coincident partner plus the nearest slotted
/// one, with the socket arc and coverage measured for each.
fn seam_rows(
    summaries: &[PartnerSummary],
    own_family: usize,
    families: &Families,
    bodies: &Bodies<'_>,
    names: &[String],
    part_ring_positions: &[[f32; 3]],
) -> Vec<SeamRow> {
    if part_ring_positions.is_empty() {
        return Vec::new();
    }
    let coincident: Vec<&PartnerSummary> = summaries
        .iter()
        .filter(|summary| summary.near > 0)
        .collect();
    // The seam-open rule fires on the strongest coincident correspondence
    // only: a rolled seam stays the part's nearest family shell, while
    // incidental proximity between family members stays quiet.
    let primary = coincident
        .iter()
        .copied()
        .max_by(|left, right| {
            left.near
                .cmp(&right.near)
                .then_with(|| right.slot_min.total_cmp(&left.slot_min))
        })
        .map(|summary| summary.partner);
    let mut partners: Vec<usize> = coincident.iter().map(|summary| summary.partner).collect();
    if let Some(slotted) = summaries
        .iter()
        .filter(|summary| {
            summary.near == 0
                && families
                    .get(&summary.partner)
                    .copied()
                    .map(|family| family_root(families, family))
                    != Some(own_family)
        })
        .min_by(|left, right| left.slot_min.total_cmp(&right.slot_min))
        .map(|summary| summary.partner)
    {
        partners.push(slotted);
    }
    partners
        .into_iter()
        .filter_map(|partner| {
            summaries
                .iter()
                .find(|summary| summary.partner == partner)
                .map(|summary| {
                    let (socket_ring, coverage) = measure_coverage(
                        part_ring_positions,
                        bodies.ring_points[partner].as_slice(),
                        bodies.ring_trees[partner].as_ref(),
                    );
                    SeamRow {
                        socket: names[partner].clone(),
                        kind: match summary.near > 0 {
                            true => SeamKind::Coincident,
                            false => SeamKind::Slotted,
                        },
                        ring: part_ring_positions.len(),
                        socket_ring,
                        slot_min: summary.slot_min,
                        ring_min: summary.ring_min,
                        ring_max: summary.ring_max,
                        coincident: summary.coincident,
                        coverage,
                        primary: Some(partner) == primary,
                    }
                })
        })
        .collect()
}

/// Socket-arc selection and coverage: the socket ring vertices within
/// [`SOCKET_PROXIMITY`] of the closest ring-to-ring point, each measured to the
/// nearest part-ring vertex; the maximum is the coverage.
fn measure_coverage(
    part_ring_positions: &[[f32; 3]],
    partner_ring: &[[f32; 3]],
    partner_ring_tree: Option<&PointIndex<'_>>,
) -> (usize, f64) {
    let Some(ring_tree) = partner_ring_tree else {
        return (0, 0.0);
    };
    if part_ring_positions.is_empty() || partner_ring.is_empty() {
        return (0, 0.0);
    }
    let part_ring_tree = PointIndex::build(part_ring_positions);
    let anchor = partner_ring
        .iter()
        .copied()
        .map(|point| {
            let nearest = part_ring_tree.nearest(point);
            (point, distance(point, part_ring_positions[nearest]))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1));
    let Some((anchor, _)) = anchor else {
        return (0, 0.0);
    };
    let arc = ring_tree.within_radius(anchor, SOCKET_PROXIMITY as f32);
    let coverage = arc
        .iter()
        .map(|&(at, _)| {
            let point = partner_ring[at];
            let nearest = part_ring_tree.nearest(point);
            distance(point, part_ring_positions[nearest])
        })
        .fold(0.0, f64::max);
    (arc.len(), coverage)
}

/// Assigns unique report names; a repeated class name gains a counter.
#[derive(Default)]
struct NamePool {
    used: HashMap<String, usize>,
}

impl NamePool {
    /// Name one shell from its class and side against the torso centroid.
    fn assign(
        &mut self,
        ring_centroid: [f64; 3],
        class: PartClass,
        axis: usize,
        torso_center: Option<[f64; 3]>,
    ) -> String {
        let plane = [0usize, 1, 2]
            .into_iter()
            .find(|candidate| *candidate != axis)
            .unwrap_or(0);
        let offset = match torso_center {
            Some(center) => ring_centroid[plane] - center[plane],
            None => 0.0,
        };
        let base = match class {
            bilateral @ (PartClass::Foot | PartClass::Leg | PartClass::Arm | PartClass::Sole) => {
                let suffix = match offset {
                    positive if positive > 0.5 => " L",
                    negative if negative < -0.5 => " R",
                    _ => "",
                };
                format!("{}{suffix}", bilateral.name())
            }
            PartClass::ToeCap => {
                let side = match offset {
                    negative if negative < -0.5 => "R",
                    _ => "L",
                };
                format!("toe cap {side}")
            }
            other => other.name().to_string(),
        };
        let count = self.used.entry(base.clone()).or_default();
        *count += 1;
        match *count {
            1 => base,
            repeated => format!("{base} {repeated}"),
        }
    }
}

/// Typed refusals for one part; `baseline` carries the shipped coverage per
/// socket so an override is judged against the shell it replaces.
fn refusals_for(
    class: PartClass,
    name: &str,
    seams: &[SeamRow],
    baseline: &[SeamRow],
) -> (&'static str, Vec<PartRefusal>) {
    let mut refusals: Vec<PartRefusal> = Vec::new();
    for seam in seams {
        let socket = seam.socket.as_str();
        match class {
            PartClass::Head if seam.kind == SeamKind::Slotted => {
                if !(HEAD_SLOT_MIN..=HEAD_SLOT_MAX).contains(&seam.slot_min) {
                    refusals.push(PartRefusal {
                        code: RefusalCode::HeadSlot,
                        part: name.to_string(),
                        socket: socket.to_string(),
                        observed: seam.slot_min,
                        bound: format!("[{HEAD_SLOT_MIN}, {HEAD_SLOT_MAX}]"),
                    });
                }
                if let Some(authored) = baseline
                    .iter()
                    .find(|row| row.socket == socket && row.kind == SeamKind::Slotted)
                {
                    let ceiling = authored.coverage + HEAD_COVERAGE_SLACK;
                    if seam.coverage > ceiling {
                        refusals.push(PartRefusal {
                            code: RefusalCode::HeadCoverage,
                            part: name.to_string(),
                            socket: socket.to_string(),
                            observed: seam.coverage,
                            bound: format!(
                                "authored {:.4} + {}",
                                authored.coverage, HEAD_COVERAGE_SLACK
                            ),
                        });
                    }
                }
            }
            PartClass::Foot
                if seam.kind == SeamKind::Slotted
                    && seam.slot_min <= FOOT_SLOTTED_BAND
                    && seam.slot_min < FOOT_SLOT_MIN =>
            {
                refusals.push(PartRefusal {
                    code: RefusalCode::FootSlot,
                    part: name.to_string(),
                    socket: socket.to_string(),
                    observed: seam.slot_min,
                    bound: format!("below {FOOT_SLOT_MIN}"),
                });
            }
            _ => {}
        }
        if seam.kind == SeamKind::Coincident && seam.primary && seam.coincident == 0 {
            refusals.push(PartRefusal {
                code: RefusalCode::SeamOpen,
                part: name.to_string(),
                socket: socket.to_string(),
                observed: 0.0,
                bound: seam.ring.to_string(),
            });
        }
    }
    let verdict = refusals
        .first()
        .map(|refusal| refusal.code.wire_code())
        .unwrap_or("ok");
    (verdict, refusals)
}

/// Audit override part files against the body's own sockets; the shipped shell
/// keeps its name, class and coverage baseline, the metrics come from the file.
fn apply_overrides(
    overrides: &[PartOverride<'_>],
    bodies: &Bodies<'_>,
    families: &Families,
    names: &[String],
    report: &mut PartAudit,
    row_of_shell: &HashMap<usize, usize>,
) -> Result<(), PartsError> {
    let mut replaced: HashSet<usize> = HashSet::new();
    for (file_index, override_part) in overrides.iter().enumerate() {
        let members: Vec<usize> = (0..override_part.positions.len()).collect();
        let Some(shell) = Shell::new(override_part.positions, override_part.triangles, &members)
        else {
            return Err(PartsError::EmptyInput);
        };
        // The file's ring centroid locates the shipped part it replaces.
        let centroid = shell.ring_centroid();
        let nearest = row_of_shell
            .keys()
            .copied()
            .filter(|index| !replaced.contains(index))
            .map(|index| {
                (
                    index,
                    distance64(centroid, bodies.shells[index].ring_centroid()),
                )
            })
            .min_by(|left, right| left.1.total_cmp(&right.1));
        let Some((target, gap)) = nearest else {
            return Err(PartsError::UnmatchedOverride {
                index: file_index,
                nearest: f64::INFINITY,
            });
        };
        if gap > OVERRIDE_MATCH_MAX {
            return Err(PartsError::UnmatchedOverride {
                index: file_index,
                nearest: gap,
            });
        }
        if !replaced.insert(target) {
            let part = report.parts[row_of_shell[&target]].name.clone();
            return Err(PartsError::OverrideConflict {
                index: file_index,
                part,
            });
        }
        let row_index = row_of_shell[&target];
        let class = report.parts[row_index].class;
        let name = report.parts[row_index].name.clone();
        let baseline = report.parts[row_index].seams.clone();
        let part_ring = shell.ring_positions_owned();
        let summaries: Vec<PartnerSummary> = bodies
            .large
            .iter()
            .copied()
            .filter(|&index| index != target)
            .map(|partner| {
                summarize(
                    partner,
                    shell.positions.as_slice(),
                    part_ring.as_slice(),
                    bodies.shells[partner].positions.as_slice(),
                    &bodies.shell_trees[partner],
                )
            })
            .collect();
        let seams = seam_rows(
            summaries.as_slice(),
            families[&target],
            families,
            bodies,
            names,
            part_ring.as_slice(),
        );
        let (verdict, refusals) = refusals_for(class, &name, &seams, &baseline);
        let row = &mut report.parts[row_index];
        row.overridden = true;
        row.seams = seams;
        row.verdict = verdict;
        row.vertices = shell.vertices.len();
        report.refusals.retain(|refusal| refusal.part != name);
        report.refusals.extend(refusals);
    }
    Ok(())
}

impl Shell {
    /// Materialized ring positions, in boundary order; the stable copy the ring
    /// trees borrow.
    fn ring_positions_owned(&self) -> Vec<[f32; 3]> {
        self.boundary
            .iter()
            .map(|&vertex| self.position_of(vertex))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDES: usize = 8;
    const TORSO: std::ops::Range<usize> = 0..16;
    const LEG: std::ops::Range<usize> = 16..32;
    const FOOT: std::ops::Range<usize> = 32..48;
    const HEAD: std::ops::Range<usize> = 48..64;

    fn ring(radius: f32, z: f32) -> Vec<[f32; 3]> {
        (0..SIDES)
            .map(|k| {
                let angle = k as f32 * std::f32::consts::FRAC_PI_4;
                [radius * angle.cos(), radius * angle.sin(), z]
            })
            .collect()
    }

    /// Quads between two rings of [`SIDES`] vertices at the given bases.
    fn prism(bottom: usize, top: usize) -> Vec<[u32; 3]> {
        (0..SIDES)
            .flat_map(|k| {
                let next = (k + 1) % SIDES;
                let b0 = (bottom + k) as u32;
                let b1 = (bottom + next) as u32;
                let t0 = (top + k) as u32;
                let t1 = (top + next) as u32;
                [[b0, b1, t1], [b0, t1, t0]]
            })
            .collect()
    }

    fn cap(base: usize) -> Vec<[u32; 3]> {
        (1..SIDES - 1)
            .map(|k| [base as u32, (base + k) as u32, (base + k + 1) as u32])
            .collect()
    }

    /// Torso (rings z 4 and 11, both open), leg (top ring coincident with the
    /// torso bottom, bottom ring open), foot (open ring over a capped bottom),
    /// head (open loop over a capped top). All shells have 16 vertices, so the
    /// component order follows vertex order: torso, leg, foot, head.
    fn fixture() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        let mut triangles = Vec::new();
        let push = |positions: &mut Vec<[f32; 3]>, ring: Vec<[f32; 3]>| {
            let base = positions.len();
            positions.extend(ring);
            base
        };
        let a = push(&mut positions, ring(2.0, 4.0));
        let b = push(&mut positions, ring(2.0, 11.0));
        triangles.extend(prism(a, b));
        let c = push(&mut positions, ring(2.0, 4.0));
        let d = push(&mut positions, ring(0.9, 1.0));
        triangles.extend(prism(c, d));
        let e = push(&mut positions, ring(1.0, 1.5));
        let f = push(&mut positions, ring(0.8, 0.0));
        triangles.extend(prism(e, f));
        triangles.extend(cap(f));
        let g = push(&mut positions, ring(2.0, 11.8));
        let h = push(&mut positions, ring(1.4, 13.6));
        triangles.extend(prism(g, h));
        triangles.extend(cap(h));
        (positions, triangles)
    }

    fn map_range(
        positions: &mut [[f32; 3]],
        range: std::ops::Range<usize>,
        map: impl Fn([f32; 3]) -> [f32; 3],
    ) {
        for position in &mut positions[range] {
            *position = map(*position);
        }
    }

    /// One shell's positions and triangles as a standalone mesh, for overrides.
    fn part_mesh(
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
        range: std::ops::Range<usize>,
    ) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let picked: Vec<[f32; 3]> = positions[range.clone()].to_vec();
        let own: Vec<[u32; 3]> = triangles
            .iter()
            .copied()
            .filter(|triangle| {
                triangle
                    .iter()
                    .all(|&vertex| (range.start..range.end).contains(&(vertex as usize)))
            })
            .map(|triangle| triangle.map(|vertex| vertex - range.start as u32))
            .collect();
        (picked, own)
    }

    fn rotate_x(position: [f32; 3], pivot_z: f32, angle: f32) -> [f32; 3] {
        let [x, y, z] = position;
        let dy = y;
        let dz = z - pivot_z;
        [
            x,
            dy * angle.cos() - dz * angle.sin(),
            pivot_z + dy * angle.sin() + dz * angle.cos(),
        ]
    }

    fn rotate_z([x, y, z]: [f32; 3], angle: f32) -> [f32; 3] {
        [
            x * angle.cos() - y * angle.sin(),
            x * angle.sin() + y * angle.cos(),
            z,
        ]
    }

    fn code_of(report: &PartAudit, code: RefusalCode) -> Option<&PartRefusal> {
        report.refusals.iter().find(|refusal| refusal.code == code)
    }

    #[test]
    fn correct_placement_passes_with_measured_rows() {
        let (positions, triangles) = fixture();
        let report = audit(&positions, &triangles, &[]).unwrap();
        assert_eq!(report.verdict(), "accept");
        assert!(report.refusals.is_empty(), "{:?}", report.refusals);
        assert!(report.open_sockets.is_empty());
        assert_eq!(report.torso.as_deref(), Some("torso"));
        let named: Vec<_> = report
            .parts
            .iter()
            .map(|part| (part.name.as_str(), part.class))
            .collect();
        assert_eq!(
            named,
            [
                ("leg", PartClass::Leg),
                ("foot", PartClass::Foot),
                ("head", PartClass::Head)
            ]
        );
        let leg = &report.parts[0];
        assert_eq!(leg.seams.len(), 2, "coincident torso plus slotted foot");
        assert_eq!(leg.seams[0].kind, SeamKind::Coincident);
        assert_eq!(leg.seams[0].socket, "torso");
        assert_eq!(leg.seams[0].coincident, SIDES);
        let foot = &report.parts[1];
        let foot_seam = &foot.seams[0];
        assert_eq!(foot_seam.kind, SeamKind::Slotted);
        assert_eq!(foot_seam.socket, "leg");
        assert!((foot_seam.slot_min - 0.5099).abs() < 1e-2, "{foot_seam:?}");
        let head = &report.parts[2];
        let head_seam = &head.seams[0];
        assert_eq!(head_seam.socket, "torso");
        assert!((head_seam.slot_min - 0.8).abs() < 1e-2, "{head_seam:?}");
        assert!((head_seam.coverage - 0.8).abs() < 1e-2, "{head_seam:?}");
        assert_eq!(head.verdict, "ok");
    }

    #[test]
    fn head_rotated_five_degrees_is_refused_by_name() {
        let (mut positions, triangles) = fixture();
        map_range(&mut positions, HEAD, |p| {
            rotate_x(p, 11.8, 5f32.to_radians())
        });
        let report = audit(&positions, &triangles, &[]).unwrap();
        let refusal = code_of(&report, RefusalCode::HeadSlot).expect("slot refusal");
        assert_eq!(refusal.part, "head");
        assert_eq!(refusal.socket, "torso");
        assert!((refusal.observed - 0.6257).abs() < 1e-2, "{refusal}");
        assert!(refusal.to_string().contains("head"));
    }

    #[test]
    fn head_shifted_half_a_unit_is_refused_once() {
        let (mut positions, triangles) = fixture();
        map_range(&mut positions, HEAD, |[x, y, z]| [x, y, z - 0.5]);
        let report = audit(&positions, &triangles, &[]).unwrap();
        assert_eq!(report.refusals.len(), 1, "{:?}", report.refusals);
        let refusal = &report.refusals[0];
        assert_eq!(refusal.code, RefusalCode::HeadSlot);
        assert!((refusal.observed - 0.3).abs() < 1e-2, "{refusal}");
    }

    #[test]
    fn foot_shifted_below_the_slot_is_refused() {
        let (mut positions, triangles) = fixture();
        map_range(&mut positions, FOOT, |[x, y, z]| [x, y, z - 0.3]);
        let report = audit(&positions, &triangles, &[]).unwrap();
        assert_eq!(report.refusals.len(), 1, "{:?}", report.refusals);
        let refusal = &report.refusals[0];
        assert_eq!(refusal.code, RefusalCode::FootSlot);
        assert_eq!(refusal.part, "foot");
        assert_eq!(refusal.socket, "leg");
        assert!((refusal.observed - 0.2236).abs() < 1e-2, "{refusal}");
        assert!(refusal.to_string().contains("foot"));
    }

    #[test]
    fn rolled_coincident_seam_is_refused_with_the_count() {
        let (mut positions, triangles) = fixture();
        map_range(&mut positions, LEG, |p| rotate_z(p, 3f32.to_radians()));
        let report = audit(&positions, &triangles, &[]).unwrap();
        assert_eq!(report.refusals.len(), 1, "{:?}", report.refusals);
        let refusal = &report.refusals[0];
        assert_eq!(refusal.code, RefusalCode::SeamOpen);
        assert_eq!(refusal.part, "leg");
        assert_eq!(refusal.socket, "torso");
        assert_eq!(refusal.observed, 0.0);
        assert_eq!(refusal.bound, "16");
        assert_eq!(report.parts[0].verdict, "part_seam_open");
    }

    #[test]
    fn yawed_override_head_breaks_the_coverage_baseline() {
        let (positions, triangles) = fixture();
        let (head_positions, head_triangles) = part_mesh(&positions, &triangles, HEAD);
        let mut yawed = head_positions.clone();
        for position in &mut yawed {
            *position = rotate_z(*position, 15f32.to_radians());
        }
        let overrides = [PartOverride {
            positions: yawed.as_slice(),
            triangles: head_triangles.as_slice(),
        }];
        let report = audit(&positions, &triangles, &overrides.as_slice()).unwrap();
        assert_eq!(report.refusals.len(), 1, "{:?}", report.refusals);
        let refusal = &report.refusals[0];
        assert_eq!(refusal.code, RefusalCode::HeadCoverage);
        assert_eq!(refusal.part, "head");
        assert!(refusal.observed > 0.9, "{refusal}");
        assert!(refusal.bound.contains("authored 0.8"), "{refusal}");
        let head = report.parts.last().unwrap();
        assert!(head.overridden);
        assert_eq!(head.verdict, "part_head_coverage");
    }

    #[test]
    fn open_sockets_are_reported_without_refusing() {
        let (mut positions, mut triangles) = fixture();
        positions.truncate(HEAD.start);
        triangles.retain(|triangle| triangle.iter().all(|&v| (v as usize) < HEAD.start));
        let report = audit(&positions, &triangles, &[]).unwrap();
        assert_eq!(report.verdict(), "accept");
        assert!(report.refusals.is_empty());
        assert_eq!(report.parts.len(), 2, "leg and foot only");
        assert_eq!(report.open_sockets.len(), 1, "{:?}", report.open_sockets);
        let socket = &report.open_sockets[0];
        assert_eq!(socket.component, "torso");
        assert_eq!(socket.ring, SIDES);
    }

    #[test]
    fn far_override_part_is_named_unmatched() {
        let (positions, triangles) = fixture();
        let (mut head_positions, head_triangles) = part_mesh(&positions, &triangles, HEAD);
        for position in &mut head_positions {
            position[0] += 30.0;
        }
        let overrides = [PartOverride {
            positions: head_positions.as_slice(),
            triangles: head_triangles.as_slice(),
        }];
        let error = audit(&positions, &triangles, &overrides.as_slice()).unwrap_err();
        assert!(matches!(
            error,
            PartsError::UnmatchedOverride { index: 0, .. }
        ));
    }

    #[test]
    fn empty_and_broken_inputs_are_refused() {
        assert_eq!(audit(&[], &[], &[]), Err(PartsError::EmptyInput));
        let (positions, mut triangles) = fixture();
        triangles[0][0] = u32::MAX;
        assert!(matches!(
            audit(&positions, &triangles, &[]),
            Err(PartsError::BadIndex { triangle: 0, .. })
        ));
        let (positions, _) = fixture();
        let mut broken = positions;
        broken[0][1] = f32::NAN;
        assert!(matches!(
            audit(&broken, &triangles, &[]),
            Err(PartsError::NonFinite { vertex: 0 })
        ));
    }
}
