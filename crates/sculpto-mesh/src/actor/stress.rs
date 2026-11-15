//! Per-frame dressed clearance over one actor clip. @req DECL0013
//!
//! Poses the declared parts at evenly spaced clip times through
//! [`Actor::sample`] and grades every frame with the existing depth owner
//! ([`garment_depth_report`]); nothing here reimplements the metric. Each
//! primitive of a sampled node arrives with the full vertex bank, so every
//! part is compacted to the vertices its triangles reference before grading —
//! the depth owner grades every vertex it is handed.
//!
//! The rows are a sampled clearance table, not a fit proof: discrete frames
//! never establish between-frame clearance, and the graded depth is a local,
//! winding-side approximation, not containment.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use serde::Serialize;

use super::{Actor, ActorError, Part, Pose, Sample, invalid};
use crate::check::garment_depth_report;
use crate::wrap::FitError;

/// One body or garment part address: a mesh-node name, optionally `Name#i`
/// to pick primitive `i` of a multi-primitive node.
///
/// Product exports carry body and underwear as primitives of one node
/// (`HIMBO - Body#0` / `#1`), so node-level names alone cannot address a
/// dressed pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartAddress {
    node_name: String,
    primitive: Option<usize>,
}

impl PartAddress {
    /// Parse `Name` or `Name#i`; a `#` suffix must carry a primitive index.
    pub fn parse(text: &str) -> Result<Self, StressError> {
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
                Err(_) => Err(invalid_plan(
                    "part address",
                    format!("{text:?}: a `#` suffix must be a primitive index"),
                )),
            },
        }
    }

    /// Resolve against the actor document: the unique named mesh node and a
    /// primitive in range.
    fn resolve(&self, actor: &Actor) -> Result<(usize, usize), StressError> {
        let mut matches = 0usize;
        let mut node = usize::MAX;
        for candidate in actor.document.nodes() {
            if candidate.name() == Some(self.node_name.as_str()) {
                node = candidate.index();
                matches += 1;
            }
        }
        if matches != 1 {
            return Err(ActorError::Selection {
                name: self.node_name.clone(),
                matches,
            }
            .into());
        }
        let primitives = actor
            .document
            .nodes()
            .nth(node)
            .and_then(|candidate| candidate.mesh())
            .map(|mesh| mesh.primitives().len())
            .unwrap_or(0);
        let primitive = match self.primitive {
            Some(primitive) if primitive < primitives => primitive,
            Some(primitive) => {
                return Err(invalid(
                    format!("part {:?}", self.display()),
                    format!("primitive {primitive} is out of range; the node has {primitives}"),
                )
                .into());
            }
            None if primitives == 1 => 0,
            None => {
                return Err(invalid(
                    format!("part {:?}", self.display()),
                    format!(
                        "the node carries {primitives} primitives; address one as {}#0..{}",
                        self.node_name,
                        primitives - 1
                    ),
                )
                .into());
            }
        };
        Ok((node, primitive))
    }

    /// The address as written: `Name` or `Name#i`.
    pub fn display(&self) -> String {
        match self.primitive {
            Some(primitive) => format!("{}#{primitive}", self.node_name),
            None => self.node_name.clone(),
        }
    }
}

/// What is posed and graded: one body part, one or more garment parts, one
/// clip (or the rest pose) and the accepted visible-depth tolerance.
#[derive(Debug, Clone, PartialEq)]
pub struct StressPlan<'a> {
    /// Body part address; the surface garments are graded against.
    pub body: &'a str,
    /// Garment part addresses, each graded against the body every frame.
    pub garments: Vec<&'a str>,
    /// Clip name; `None` poses the rest once and ignores `frames`.
    pub clip: Option<&'a str>,
    /// Number of evenly spaced frames across the clip span, endpoints
    /// included; at least 1.
    pub frames: usize,
    /// Accepted graded visible depth in input-frame units
    /// ([`crate::check::DepthReport::visible_verdict`]).
    pub tolerance: f32,
}

/// Why a stress plan cannot run.
#[derive(Debug)]
#[non_exhaustive]
pub enum StressError {
    /// The actor refused the part selection, clip or pose.
    Actor(ActorError),
    /// The depth owner refused the posed mesh pair.
    Fit(FitError),
    /// The plan itself is malformed.
    Plan { context: String, reason: String },
}

impl fmt::Display for StressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Actor(error) => write!(formatter, "{error}"),
            Self::Fit(error) => write!(formatter, "posed pair refused by the depth owner: {error}"),
            Self::Plan { context, reason } => write!(formatter, "stress {context}: {reason}"),
        }
    }
}

impl std::error::Error for StressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Actor(error) => Some(error),
            Self::Fit(error) => Some(error),
            Self::Plan { .. } => None,
        }
    }
}

impl From<ActorError> for StressError {
    fn from(error: ActorError) -> Self {
        Self::Actor(error)
    }
}

impl From<FitError> for StressError {
    fn from(error: FitError) -> Self {
        Self::Fit(error)
    }
}

fn invalid_plan(context: &str, reason: impl Into<String>) -> StressError {
    StressError::Plan {
        context: context.to_owned(),
        reason: reason.into(),
    }
}

/// The posed frames a report belongs to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StressPose {
    /// Authored transforms, one row.
    Rest,
    /// One clip evaluated at evenly declared times.
    Clip {
        /// The unique clip name.
        name: String,
        /// Authored clip span start in seconds.
        start: f64,
        /// Authored clip span end in seconds.
        end: f64,
    },
}

/// One garment's graded numbers at one frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GarmentRow {
    /// Garment address as written.
    pub part: String,
    /// Compact garment vertices graded.
    pub vertices: usize,
    /// Deepest graded (`visible_occluded`) vertex depth, input units.
    pub visible_max: f32,
    /// Graded vertices inside the body
    /// ([`crate::check::DepthReport::visible_occluded`]).
    pub penetrations: usize,
    /// Graded vertices deeper than the tolerance.
    pub violations: usize,
    /// Vertices behind the nearest body triangle, graded or not.
    pub inside: usize,
    /// Garment/body triangle intersections at this frame.
    pub body_pairs: usize,
    /// Garment self intersections at this frame.
    pub self_pairs: usize,
    /// `visible_verdict(tolerance)` for this garment.
    pub passed: bool,
}

/// One sampled frame's clearance row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameRow {
    /// Frame order in the plan.
    pub index: usize,
    /// Clip time in seconds; `null` for the rest pose.
    pub time_seconds: Option<f64>,
    /// Deepest graded depth across the garments, input units.
    pub visible_max: f32,
    /// Graded penetrations across the garments.
    pub penetrations: usize,
    /// Graded violations across the garments.
    pub violations: usize,
    /// Garment/body intersections across the garments.
    pub body_pairs: usize,
    /// Garment self intersections across the garments.
    pub self_pairs: usize,
    /// Every garment's verdict passed at this frame.
    pub passed: bool,
    /// Per-garment numbers.
    pub garments: Vec<GarmentRow>,
}

/// The complete per-frame clearance table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StressReport {
    /// The posed frames the rows belong to.
    pub pose: StressPose,
    /// Body address as written.
    pub body: String,
    /// Garment addresses as written.
    pub garments: Vec<String>,
    /// Accepted graded visible depth, input-frame units.
    pub tolerance: f32,
    /// One row per sampled frame, in declaration order.
    pub frames: Vec<FrameRow>,
    /// Every row passed at the tolerance.
    pub passed: bool,
    /// Fixed statement of what the table does not prove.
    pub statement: &'static str,
}

/// Pose the plan's parts at the declared frames and grade every frame with the
/// depth owner.
///
/// # Errors
/// Typed refusals for a malformed plan, a missing or ambiguous part/clip
/// ([`ActorError::Selection`]/[`ActorError::Clip`]) and a posed pair the depth
/// owner refuses ([`FitError`]).
pub fn stress(actor: &Actor, plan: &StressPlan<'_>) -> Result<StressReport, StressError> {
    if plan.garments.is_empty() {
        return Err(invalid_plan(
            "garments",
            "at least one garment part is required",
        ));
    }
    if plan.frames == 0 {
        return Err(invalid_plan("frames", "at least one frame is required"));
    }
    if !plan.tolerance.is_finite() || plan.tolerance < 0.0 {
        return Err(invalid_plan(
            "tolerance",
            format!("{} is not a finite non-negative depth", plan.tolerance),
        ));
    }
    let body = PartAddress::parse(plan.body)?;
    let mut garments = Vec::with_capacity(plan.garments.len());
    for address in &plan.garments {
        let address = PartAddress::parse(address)?;
        if address == body {
            return Err(invalid_plan(
                "garments",
                format!(
                    "{:?} is also the body; a garment is graded against a different part",
                    address.display()
                ),
            ));
        }
        if garments.contains(&address) {
            return Err(invalid_plan(
                "garments",
                format!("duplicate garment part {:?}", address.display()),
            ));
        }
        garments.push(address);
    }
    let resolved_body = body.resolve(actor)?;
    let mut resolved_garments = Vec::with_capacity(garments.len());
    for address in &garments {
        resolved_garments.push(address.resolve(actor)?);
    }
    let mut names: Vec<&str> = Vec::new();
    let mut seen = BTreeSet::new();
    for &(node, _) in resolved_garments
        .iter()
        .chain(std::iter::once(&resolved_body))
    {
        if seen.insert(node) {
            let name = actor
                .document
                .nodes()
                .nth(node)
                .and_then(|candidate| candidate.name())
                .ok_or_else(|| invalid(format!("node {node}"), "missing selected name"))?;
            names.push(name);
        }
    }

    let (pose, times): (StressPose, Vec<Option<f64>>) = match plan.clip {
        Some(name) => {
            let (start, end) = actor.clip_span(name)?;
            let times: Vec<Option<f64>> = frame_times(start, end, plan.frames)
                .into_iter()
                .map(Some)
                .collect();
            let pose = StressPose::Clip {
                name: name.to_owned(),
                start,
                end,
            };
            (pose, times)
        }
        None => (StressPose::Rest, vec![None]),
    };

    let mut frames = Vec::with_capacity(times.len());
    for (index, time) in times.into_iter().enumerate() {
        let frame_pose = match (plan.clip, time) {
            (Some(name), Some(time)) => Pose::Clip { name, time },
            (None, None) => Pose::Rest,
            _ => unreachable!("clip rows carry times and rest rows do not"),
        };
        let sampled = actor.sample(&names, frame_pose)?;
        let body_part = sampled_part(&sampled, resolved_body)?;
        let body_mesh = part_mesh(&sampled, body_part)?;
        let mut rows = Vec::with_capacity(resolved_garments.len());
        for (address, &(node, primitive)) in garments.iter().zip(&resolved_garments) {
            let part = sampled_part(&sampled, (node, primitive))?;
            let mesh = part_mesh(&sampled, part)?;
            let report = garment_depth_report((&mesh.0, &mesh.1), (&body_mesh.0, &body_mesh.1))?;
            let verdict = report.visible_verdict(plan.tolerance);
            rows.push(GarmentRow {
                part: address.display(),
                vertices: mesh.0.len(),
                visible_max: report.visible_occluded.max,
                penetrations: report.visible_occluded.count,
                violations: verdict.violations.len(),
                inside: report.inside,
                body_pairs: report.body_pairs,
                self_pairs: report.self_pairs,
                passed: verdict.passed,
            });
        }
        let visible_max = rows
            .iter()
            .map(|row| row.visible_max)
            .fold(0.0_f32, f32::max);
        frames.push(FrameRow {
            index,
            time_seconds: time,
            visible_max,
            penetrations: rows.iter().map(|row| row.penetrations).sum(),
            violations: rows.iter().map(|row| row.violations).sum(),
            body_pairs: rows.iter().map(|row| row.body_pairs).sum(),
            self_pairs: rows.iter().map(|row| row.self_pairs).sum(),
            passed: rows.iter().all(|row| row.passed),
            garments: rows,
        });
    }
    let passed = frames.iter().all(|row| row.passed);
    Ok(StressReport {
        pose,
        body: body.display(),
        garments: garments.iter().map(PartAddress::display).collect(),
        tolerance: plan.tolerance,
        frames,
        passed,
        statement: "sampled graded visible depth per frame; not containment or continuous clearance",
    })
}

/// Evenly spaced times across `[start, end]`, endpoints exact.
fn frame_times(start: f64, end: f64, frames: usize) -> Vec<f64> {
    if frames == 1 {
        return vec![start];
    }
    (0..frames)
        .map(|index| {
            if index + 1 == frames {
                end
            } else {
                start + (end - start) * index as f64 / (frames - 1) as f64
            }
        })
        .collect()
}

/// The sampled part carrying `(node, primitive)`; the sampler keeps primitive
/// order, so the identity is exact.
fn sampled_part(sampled: &Sample, identity: (usize, usize)) -> Result<&Part, StressError> {
    sampled
        .parts
        .iter()
        .find(|part| (part.node, part.primitive) == identity)
        .ok_or_else(|| {
            invalid_plan(
                "sample",
                format!(
                    "node {} primitive {} is missing from the sample",
                    identity.0, identity.1
                ),
            )
        })
}

/// A measurement-ready mesh: compact positions and triangles.
type PartMesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Compact one primitive to the vertices its triangles reference.
///
/// Each sampled primitive repeats the full export vertex bank; the depth
/// owner grades every vertex it is handed, so unreferenced bank copies must
/// not survive into the measurement.
fn part_mesh(sampled: &Sample, part: &Part) -> Result<PartMesh, StressError> {
    let base = part.vertices.start;
    let triangles = &sampled.triangles[part.triangles.clone()];
    let referenced: BTreeSet<usize> = triangles
        .iter()
        .flatten()
        .map(|&index| {
            index
                .checked_sub(base)
                .filter(|&local| local < part.vertices.len())
                .ok_or_else(|| {
                    invalid_plan(
                        "sample",
                        format!(
                            "part {:?} triangle references vertex {index} outside its range",
                            part.name
                        ),
                    )
                })
        })
        .collect::<Result<_, _>>()?;
    let mut remap: HashMap<usize, u32> = HashMap::with_capacity(referenced.len());
    let positions: Vec<[f32; 3]> = referenced
        .into_iter()
        .enumerate()
        .map(|(compact, local)| {
            remap.insert(base + local, compact as u32);
            let point = sampled.vertices[base + local];
            [point[0] as f32, point[1] as f32, point[2] as f32]
        })
        .collect();
    let compact = triangles
        .iter()
        .map(|triangle| triangle.map(|index| remap[&index]))
        .collect();
    Ok((positions, compact))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_are_even_and_endpoint_exact() {
        let times = frame_times(0.0, 1.0, 4);
        assert_eq!(times, vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]);
        assert_eq!(frame_times(2.0, 5.0, 1), vec![2.0]);
        let constant = frame_times(1.0, 1.0, 3);
        assert_eq!(constant, vec![1.0, 1.0, 1.0]);
        let last = frame_times(0.1, 0.3, 7);
        assert_eq!(last.last().copied(), Some(0.3));
        assert!(last.iter().all(|&time| (0.1..=0.3).contains(&time)));
    }

    #[test]
    fn part_addresses_parse_names_and_indices() {
        let plain = PartAddress::parse("HIMBO - Body").expect("plain name");
        assert_eq!(plain.display(), "HIMBO - Body");
        let indexed = PartAddress::parse("3BA#2").expect("indexed");
        assert_eq!(indexed.display(), "3BA#2");
        let malformed = PartAddress::parse("3BA#x").expect_err("non-numeric suffix");
        assert!(matches!(malformed, StressError::Plan { .. }));
    }
}
