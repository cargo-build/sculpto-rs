//! Cross-section aura fit: a smoothed, offset envelope of the body that garment
//! vertices are pressed onto.
//!
//! The body is cut into horizontal sections; each section's contour becomes a
//! radial envelope ("aura") with two independent margins: a clearance that the
//! raw body may never cross, and an ease allowance that scales the smoothed
//! contour. Smoothing skips small body detail (the operator's nipple example)
//! while the clearance floor keeps the raw body inside, so the cloth bridges a
//! bump in a wide arc instead of printing it through or clipping it.
//!
//! Sections are constraints, not cuts: garment vertices move radially in their
//! own section plane, so the garment keeps its topology and UVs.

use std::collections::HashMap;
use std::fmt;

/// One straight piece of a section contour.
pub(crate) type Segment = ([f32; 2], [f32; 2]);

/// How a garment vertex may move relative to the aura.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraMode {
    /// Place every vertex on the aura: a tight garment.
    Tight,
    /// Lift vertices inside the aura, cap far outliers, keep free volume between.
    Press,
    /// Only lift vertices inside the aura; never pull a loose garment in.
    Loose,
}

impl AuraMode {
    /// Parse a CLI mode name.
    pub fn parse(value: &str) -> Result<Self, AuraError> {
        match value {
            "tight" => Ok(Self::Tight),
            "press" => Ok(Self::Press),
            "loose" => Ok(Self::Loose),
            other => Err(AuraError::UnknownMode(other.to_string())),
        }
    }
}

/// Fitting-envelope topology for one section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One envelope per connected body contour: a trouser leg, a sleeve.
    PerContour,
    /// One envelope around every contour in the section: a skirt over both legs.
    Merged,
}

/// Layout and movement rule for one side of a band split.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandSpec {
    /// Fitting-envelope topology in this band.
    pub layout: Layout,
    /// How a garment vertex may move in this band.
    pub mode: AuraMode,
}

/// A height band on the section axis: at or above `split` the `above` rules
/// apply, below it the `below` rules (a fitted bodice over a loose skirt).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandRules {
    /// Split height on the section axis, in body coordinates.
    pub split: f32,
    /// Rules at and above the split.
    pub above: BandSpec,
    /// Rules below the split.
    pub below: BandSpec,
}

/// Aura construction parameters.
///
/// `slack` is linear: `0.05` scales the smoothed contour by 1.05, which grows
/// the perimeter by 5% — not the area (that would be ×1.1025).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuraParams {
    /// Minimum distance the raw body keeps inside the aura.
    pub clearance: f32,
    /// Linear ease allowance on the smoothed contour (0.05 = +5% perimeter).
    pub slack: f32,
    /// Number of body sections.
    pub slices: usize,
    /// Angular samples per section contour.
    pub bins: usize,
    /// Circular smoothing window in bins; 1 disables smoothing.
    pub smooth: usize,
    /// Movement rule when `bands` is absent.
    pub mode: AuraMode,
    /// Largest factor of the target radius press mode keeps (1.5 default).
    pub max_ease: f32,
    /// Band rules: layout and mode per height band. `None` keeps one
    /// per-contour layout and `mode` for the whole body.
    pub bands: Option<BandRules>,
}

impl Default for AuraParams {
    fn default() -> Self {
        Self {
            clearance: 0.6,
            slack: 0.05,
            slices: 96,
            bins: 192,
            smooth: 5,
            mode: AuraMode::Press,
            max_ease: 1.5,
            bands: None,
        }
    }
}

/// Why an aura could not be built or applied.
#[derive(Debug, Clone, PartialEq)]
pub enum AuraError {
    /// The body has no vertices.
    EmptyBody,
    /// The body has no triangles.
    EmptyTriangles,
    /// A triangle references a vertex the body does not have.
    BadIndex { triangle: usize, vertex: usize },
    /// Fewer than two sections were requested.
    TooFewSlices,
    /// Press ease factor below one would clamp garments inside the aura.
    InvalidEase(f32),
    /// Fewer than eight angular bins were requested.
    TooFewBins,
    /// The smoothing window covers the whole circle.
    WindowTooWide,
    /// The mode name is not one of `tight`, `press`, `loose`.
    UnknownMode(String),
}

impl fmt::Display for AuraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBody => write!(formatter, "the body has no vertices"),
            Self::EmptyTriangles => write!(formatter, "the body has no triangles"),
            Self::BadIndex { triangle, vertex } => write!(
                formatter,
                "triangle {triangle} references vertex {vertex}, which the body does not have"
            ),
            Self::TooFewSlices => write!(formatter, "at least two slices are required"),
            Self::TooFewBins => write!(formatter, "at least eight bins are required"),
            Self::WindowTooWide => {
                write!(
                    formatter,
                    "the smoothing window must stay below the bin count"
                )
            }
            Self::InvalidEase(value) => write!(formatter, "max ease {value} must be at least 1.0"),
            Self::UnknownMode(name) => write!(
                formatter,
                "unknown mode `{name}`: expected tight, press or loose"
            ),
        }
    }
}

impl std::error::Error for AuraError {}

/// What applying an aura did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuraReport {
    /// Section axis index (0=x, 1=y, 2=z).
    pub axis: usize,
    /// Number of sections built.
    pub slices: usize,
    /// Most contours a single section produced (torso, arms, legs split).
    pub max_contours: usize,
    /// Vertices whose position changed.
    pub moved: usize,
    /// Mean displacement over moved vertices.
    pub moved_mean: f32,
    /// Median displacement over moved vertices (nearest rank).
    pub moved_p50: f32,
    /// 95th percentile displacement over moved vertices (nearest rank).
    pub moved_p95: f32,
    /// Largest displacement.
    pub moved_max: f32,
    /// Vertices the aura reached that sat inside the raw contour before the fit.
    pub inside_before: usize,
    /// Vertices still inside the raw contour after the fit.
    pub inside_after: usize,
}

/// Collision proxies for one body section: polylines of the raw surface.
#[derive(Debug, Clone, PartialEq)]
pub struct ProxySection {
    /// Section height on the body's up axis.
    pub height: f32,
    /// One closed polyline per connected body contour, in body coordinates.
    pub contours: Vec<Vec<[f32; 3]>>,
}

/// One section's envelopes. `components` is what garments press onto (merged
/// when the band rules say so); `proxies` always keeps one per body contour.
#[derive(Debug, Clone, PartialEq)]
struct Ring {
    components: Vec<Component>,
    proxies: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq)]
struct Component {
    /// Contour centroid in the section plane.
    center: [f32; 2],
    /// Raw radial samples: the body surface, spikes included.
    raw: Vec<f32>,
    /// Smoothed contour with clearance and ease applied: where cloth may sit.
    target: Vec<f32>,
}

/// A body envelope built from sections; applies to any garment in the same frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Aura {
    axis: usize,
    origin: f32,
    step: f32,
    rings: Vec<Ring>,
    params: AuraParams,
}

impl Aura {
    /// Slice the body and build one smoothed envelope per contour per section.
    pub fn build(
        body: &[[f32; 3]],
        triangles: &[[u32; 3]],
        params: AuraParams,
    ) -> Result<Self, AuraError> {
        if body.is_empty() {
            return Err(AuraError::EmptyBody);
        }
        if triangles.is_empty() {
            return Err(AuraError::EmptyTriangles);
        }
        if params.slices < 2 {
            return Err(AuraError::TooFewSlices);
        }
        if params.bins < 8 {
            return Err(AuraError::TooFewBins);
        }
        let window = odd_window(params.smooth);
        if window >= params.bins {
            return Err(AuraError::WindowTooWide);
        }
        for (triangle, [a, b, c]) in triangles.iter().enumerate() {
            if [*a, *b, *c]
                .iter()
                .any(|index| *index as usize >= body.len())
            {
                return Err(AuraError::BadIndex {
                    triangle,
                    vertex: [*a, *b, *c].iter().copied().max().unwrap_or_default() as usize,
                });
            }
        }

        let axis = up_axis(body);
        let (min, max) = extent(body, axis);
        let step = (max - min) / params.slices as f32;
        let rings = (0..params.slices)
            .map(|index| {
                let height = min + (index as f32 + 0.5) * step;
                let merged = params.bands.is_some_and(|bands| height < bands.split);
                build_ring(body, triangles, axis, height, &params, merged)
            })
            .collect();
        Ok(Self {
            axis,
            origin: min,
            step,
            rings,
            params,
        })
    }

    /// Move garment vertices onto the aura and report what happened.
    pub fn apply(&self, garment: &[[f32; 3]]) -> (Vec<[f32; 3]>, AuraReport) {
        let (plane_u, plane_v) = plane_axes(self.axis);
        let mut report = AuraReport {
            axis: self.axis,
            slices: self.rings.len(),
            max_contours: self
                .rings
                .iter()
                .fold(0usize, |most, ring| most.max(ring.components.len())),
            moved: 0,
            moved_mean: 0.0,
            moved_p50: 0.0,
            moved_p95: 0.0,
            moved_max: 0.0,
            inside_before: 0,
            inside_after: 0,
        };
        let mut displacement_sum = 0.0f32;
        let mut displacements: Vec<f32> = Vec::new();
        let fitted: Vec<[f32; 3]> = garment
            .iter()
            .map(|position| {
                let Some((center, raw_radius, target_radius)) =
                    self.lookup(position, plane_u, plane_v)
                else {
                    return *position;
                };
                let offset = [position[plane_u] - center[0], position[plane_v] - center[1]];
                let radius = (offset[0] * offset[0] + offset[1] * offset[1]).sqrt();
                if radius < 1e-6 {
                    return *position;
                }
                if raw_radius - radius > 1e-3 {
                    report.inside_before += 1;
                }
                let mode = match self.params.bands {
                    Some(bands) if position[self.axis] < bands.split => bands.below.mode,
                    Some(bands) => bands.above.mode,
                    None => self.params.mode,
                };
                let desired = match mode {
                    AuraMode::Tight => target_radius,
                    AuraMode::Press => {
                        radius.clamp(target_radius, target_radius * self.params.max_ease)
                    }
                    AuraMode::Loose => radius.max(target_radius),
                };
                if raw_radius - desired > 1e-3 {
                    report.inside_after += 1;
                }
                if (desired - radius).abs() <= 1e-4 {
                    return *position;
                }
                let scale = desired / radius;
                let mut moved = *position;
                moved[plane_u] = center[0] + offset[0] * scale;
                moved[plane_v] = center[1] + offset[1] * scale;
                let displacement = (moved[0] - position[0])
                    .mul_add(
                        moved[0] - position[0],
                        (moved[1] - position[1]).mul_add(
                            moved[1] - position[1],
                            (moved[2] - position[2]) * (moved[2] - position[2]),
                        ),
                    )
                    .sqrt();
                report.moved += 1;
                report.moved_max = report.moved_max.max(displacement);
                displacement_sum += displacement;
                displacements.push(displacement);
                moved
            })
            .collect();
        if report.moved > 0 {
            report.moved_mean = displacement_sum / report.moved as f32;
            displacements.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let rank = |q: f32| {
                let at = ((displacements.len() as f32 - 1.0) * q).round() as usize;
                displacements[at.min(displacements.len() - 1)]
            };
            report.moved_p50 = rank(0.50);
            report.moved_p95 = rank(0.95);
        }
        (fitted, report)
    }

    /// Interpolated envelope values for one point: center, raw and target radius.
    fn lookup(
        &self,
        position: &[f32; 3],
        plane_u: usize,
        plane_v: usize,
    ) -> Option<([f32; 2], f32, f32)> {
        let relative = (position[self.axis] - self.origin) / self.step - 0.5;
        if relative < 0.0 || relative > self.rings.len() as f32 - 1.0 {
            return None;
        }
        let lower = relative.floor() as usize;
        let upper = (lower + 1).min(self.rings.len() - 1);
        let blend = relative - lower as f32;
        let point = [position[plane_u], position[plane_v]];
        let below = self.rings[lower].nearest(point);
        let above = self.rings[upper].nearest(point);
        // A contour that ends between two sections (a hand tip, a sleeve edge)
        // keeps the section that still exists instead of dropping the vertex.
        let (below, above) = match (below, above) {
            (Some(below), Some(above)) => (below, above),
            (Some(only), None) | (None, Some(only)) => (only, only),
            (None, None) => return None,
        };
        let sampled_below = sample_component(below, point);
        let sampled_above = sample_component(above, point);
        let center = [
            sampled_below.0[0] + blend * (sampled_above.0[0] - sampled_below.0[0]),
            sampled_below.0[1] + blend * (sampled_above.0[1] - sampled_below.0[1]),
        ];
        Some((
            center,
            sampled_below.1 + blend * (sampled_above.1 - sampled_below.1),
            sampled_below.2 + blend * (sampled_above.2 - sampled_below.2),
        ))
    }

    /// Collision proxies: per section, one polyline per connected body
    /// contour of the raw surface, independent of the fitting layout.
    pub fn proxies(&self) -> Vec<ProxySection> {
        let (plane_u, plane_v) = plane_axes(self.axis);
        self.rings
            .iter()
            .enumerate()
            .map(|(index, ring)| {
                let height = self.origin + (index as f32 + 0.5) * self.step;
                let contours = ring
                    .proxies
                    .iter()
                    .map(|component| {
                        (0..component.raw.len())
                            .map(|bin| {
                                let angle =
                                    std::f32::consts::TAU * bin as f32 / component.raw.len() as f32;
                                let mut point = [0.0; 3];
                                point[plane_u] =
                                    component.center[0] + component.raw[bin] * angle.cos();
                                point[plane_v] =
                                    component.center[1] + component.raw[bin] * angle.sin();
                                point[self.axis] = height;
                                point
                            })
                            .collect()
                    })
                    .collect();
                ProxySection { height, contours }
            })
            .collect()
    }
}

/// Radius values for one point against one component's contour.
fn sample_component(component: &Component, point: [f32; 2]) -> ([f32; 2], f32, f32) {
    let bins = component.raw.len();
    let angle = (point[1] - component.center[1])
        .atan2(point[0] - component.center[0])
        .rem_euclid(std::f32::consts::TAU);
    let exact = angle / std::f32::consts::TAU * bins as f32;
    let lower = exact.floor() as usize % bins;
    let upper = (lower + 1) % bins;
    let blend = exact - exact.floor();
    let interpolate = |values: &[f32]| values[lower] + blend * (values[upper] - values[lower]);
    (
        component.center,
        interpolate(&component.raw),
        interpolate(&component.target),
    )
}

impl Ring {
    /// The component whose raw contour passes closest to `point`.
    fn nearest(&self, point: [f32; 2]) -> Option<&Component> {
        self.components.iter().min_by(|left, right| {
            left.distance_to_contour(point)
                .total_cmp(&right.distance_to_contour(point))
        })
    }
}

impl Component {
    /// Distance from `point` to the sampled raw contour; smaller means the point
    /// belongs to this body part rather than a neighbour in the same section.
    fn distance_to_contour(&self, point: [f32; 2]) -> f32 {
        self.raw
            .iter()
            .enumerate()
            .map(|(bin, radius)| {
                let angle = std::f32::consts::TAU * bin as f32 / self.raw.len() as f32;
                let dx = point[0] - self.center[0] - radius * angle.cos();
                let dy = point[1] - self.center[1] - radius * angle.sin();
                dx * dx + dy * dy
            })
            .fold(f32::INFINITY, f32::min)
            .sqrt()
    }
}

fn build_ring(
    body: &[[f32; 3]],
    triangles: &[[u32; 3]],
    axis: usize,
    height: f32,
    params: &AuraParams,
    merged: bool,
) -> Ring {
    let (plane_u, plane_v) = plane_axes(axis);
    let segments: Vec<Segment> = triangles
        .iter()
        .filter_map(|triangle| {
            slice_triangle(
                [
                    &body[triangle[0] as usize],
                    &body[triangle[1] as usize],
                    &body[triangle[2] as usize],
                ],
                axis,
                height,
                plane_u,
                plane_v,
            )
        })
        .collect();
    let groups = group_segments(&segments);
    let proxies: Vec<Component> = groups
        .iter()
        .filter_map(|group| build_component(group, params))
        .collect();
    let components = if merged {
        let all: Vec<Segment> = groups.into_iter().flatten().collect();
        build_component(&all, params).into_iter().collect()
    } else {
        proxies.clone()
    };
    Ring {
        components,
        proxies,
    }
}

/// Intersect one triangle with the plane; a coplanar triangle yields nothing.
pub(crate) fn slice_triangle(
    corners: [&[f32; 3]; 3],
    axis: usize,
    height: f32,
    plane_u: usize,
    plane_v: usize,
) -> Option<([f32; 2], [f32; 2])> {
    let signed: [f32; 3] = corners.map(|corner| corner[axis] - height);
    if signed.iter().all(|value| *value == 0.0) {
        return None;
    }
    let mut points: Vec<[f32; 2]> = Vec::with_capacity(3);
    for (start, end) in [(0usize, 1usize), (1, 2), (2, 0)] {
        let (ds, de) = (signed[start], signed[end]);
        if ds == 0.0 {
            points.push(project(corners[start], plane_u, plane_v));
        } else if ds * de < 0.0 {
            let t = ds / (ds - de);
            let lerp = |a: f32, b: f32| a + t * (b - a);
            points.push([
                lerp(corners[start][plane_u], corners[end][plane_u]),
                lerp(corners[start][plane_v], corners[end][plane_v]),
            ]);
        }
    }
    let mut unique: Vec<[f32; 2]> = Vec::with_capacity(2);
    for point in points {
        if !unique
            .iter()
            .any(|seen| (seen[0] - point[0]).abs() < 1e-4 && (seen[1] - point[1]).abs() < 1e-4)
        {
            unique.push(point);
        }
    }
    match unique.as_slice() {
        [first, second] => Some((*first, *second)),
        _ => None,
    }
}

fn project(point: &[f32; 3], plane_u: usize, plane_v: usize) -> [f32; 2] {
    [point[plane_u], point[plane_v]]
}

/// Chain segments into connected contours by matching endpoints.
pub(crate) fn group_segments(segments: &[Segment]) -> Vec<Vec<Segment>> {
    const CELL: f32 = 1e-2;
    let mut parent: Vec<usize> = (0..segments.len()).collect();
    let mut anchors: HashMap<(i64, i64), usize> = HashMap::new();
    for (index, (start, end)) in segments.iter().enumerate() {
        for point in [start, end] {
            let cell = (key(point[0], CELL), key(point[1], CELL));
            let mut linked = false;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let neighbour = (cell.0 + dx, cell.1 + dy);
                    if let Some(other) = anchors.get(&neighbour) {
                        let (other_start, other_end) = segments[*other];
                        for other_point in [other_start, other_end] {
                            let distance =
                                (other_point[0] - point[0]).hypot(other_point[1] - point[1]);
                            if distance < CELL {
                                union(&mut parent, index, *other);
                                linked = true;
                            }
                        }
                    }
                }
            }
            if !linked {
                anchors.insert(cell, index);
            }
        }
    }
    let mut groups: HashMap<usize, Vec<Segment>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        groups
            .entry(find(&mut parent, index))
            .or_default()
            .push(*segment);
    }
    groups.into_values().collect()
}

fn key(value: f32, cell: f32) -> i64 {
    (value / cell).floor() as i64
}

fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let (left, right) = (find(parent, left), find(parent, right));
    if left != right {
        parent[right] = left;
    }
}

/// Radial envelope of one contour: raw surface plus the cloth target.
fn build_component(segments: &[Segment], params: &AuraParams) -> Option<Component> {
    let total: f32 = segments
        .iter()
        .map(|(start, end)| (start[0] - end[0]).hypot(start[1] - end[1]))
        .sum();
    if total <= f32::EPSILON {
        return None;
    }
    let weighted: [f32; 2] = segments
        .iter()
        .map(|(start, end)| {
            let length = (start[0] - end[0]).hypot(start[1] - end[1]);
            [
                (start[0] + end[0]) * 0.5 * length,
                (start[1] + end[1]) * 0.5 * length,
            ]
        })
        .fold([0.0, 0.0], |sum, add| [sum[0] + add[0], sum[1] + add[1]]);
    let center = [weighted[0] / total, weighted[1] / total];

    let mut raw = vec![0.0f32; params.bins];
    for (bin, radius) in raw.iter_mut().enumerate() {
        let angle = std::f32::consts::TAU * bin as f32 / params.bins as f32;
        let direction = [angle.cos(), angle.sin()];
        *radius = segments
            .iter()
            .filter_map(|(start, end)| ray_segment(center, direction, *start, *end))
            .fold(0.0, f32::max);
    }
    if raw.iter().all(|radius| *radius <= 0.0) {
        return None;
    }
    fill_gaps(&mut raw);
    let reach = odd_window(params.smooth) / 2;
    let dilated = dilate_circular(&raw, reach);
    let smoothed = smooth_circular(&dilated, reach * 2 + 1);
    let target = dilated
        .iter()
        .zip(&smoothed)
        .map(|(&floor, &smooth)| (smooth * (1.0 + params.slack)).max(floor + params.clearance))
        .collect();
    Some(Component {
        center,
        raw,
        target,
    })
}

/// Furthest intersection of a ray from `center` with one segment, if any.
fn ray_segment(
    center: [f32; 2],
    direction: [f32; 2],
    start: [f32; 2],
    end: [f32; 2],
) -> Option<f32> {
    let edge = [end[0] - start[0], end[1] - start[1]];
    let denominator = direction[0] * edge[1] - direction[1] * edge[0];
    if denominator.abs() < 1e-9 {
        return None;
    }
    let offset = [start[0] - center[0], start[1] - center[1]];
    let cross = |left: [f32; 2], right: [f32; 2]| left[0] * right[1] - left[1] * right[0];
    let ray = cross(offset, edge) / denominator;
    let position = cross(offset, direction) / denominator;
    if ray > 0.0 && (0.0..=1.0).contains(&position) {
        Some(ray)
    } else {
        None
    }
}

/// Circularly interpolate empty bins between their flanking samples.
fn fill_gaps(values: &mut [f32]) {
    let bins = values.len();
    let present: Vec<usize> = (0..bins).filter(|index| values[*index] > 0.0).collect();
    if present.len() < 2 {
        return;
    }
    for gap in 0..bins {
        if values[gap] > 0.0 {
            continue;
        }
        let backward = (0..bins)
            .map(|step| (gap + bins - step) % bins)
            .find(|index| values[*index] > 0.0);
        let forward = (0..bins)
            .map(|step| (gap + step) % bins)
            .find(|index| values[*index] > 0.0);
        if let (Some(backward), Some(forward)) = (backward, forward) {
            let span = (forward + bins - backward) % bins;
            if span == 0 {
                continue;
            }
            let blend = (gap + bins - backward) % bins;
            values[gap] = values[backward]
                + (values[forward] - values[backward]) * blend as f32 / span as f32;
        }
    }
}

/// Circular maximum filter: every bin rises to the largest value within
/// `reach` bins. Dilating before smoothing is what keeps a smoothed aura from
/// cutting into a bump it must still clear.
fn dilate_circular(values: &[f32], reach: usize) -> Vec<f32> {
    let bins = values.len();
    if reach == 0 || bins < 2 {
        return values.to_vec();
    }
    (0..bins)
        .map(|bin| {
            (-(reach as i64)..=(reach as i64))
                .map(|offset| values[((bin as i64 + offset).rem_euclid(bins as i64)) as usize])
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect()
}

/// Circular box filter; a wide window flattens small body detail.
fn smooth_circular(values: &[f32], window: usize) -> Vec<f32> {
    let bins = values.len();
    if window <= 1 || bins < 2 {
        return values.to_vec();
    }
    let reach = window as i64 / 2;
    (0..bins)
        .map(|bin| {
            let sum: f32 = (-reach..=reach)
                .map(|offset| values[((bin as i64 + offset).rem_euclid(bins as i64)) as usize])
                .sum();
            sum / window as f32
        })
        .collect()
}

fn odd_window(smooth: usize) -> usize {
    smooth.max(1) | 1
}

/// Largest body extent wins: a standing figure is tallest along its up axis.
pub(crate) fn up_axis(body: &[[f32; 3]]) -> usize {
    let mut spans = [(f32::INFINITY, f32::NEG_INFINITY); 3];
    for position in body {
        for axis in 0..3 {
            spans[axis].0 = spans[axis].0.min(position[axis]);
            spans[axis].1 = spans[axis].1.max(position[axis]);
        }
    }
    spans
        .iter()
        .enumerate()
        .max_by(|(left_axis, left), (right_axis, right)| {
            (left.1 - left.0)
                .total_cmp(&(right.1 - right.0))
                .then(left_axis.cmp(right_axis))
        })
        .map(|(axis, _)| axis)
        .unwrap_or(1)
}

pub(crate) fn extent(body: &[[f32; 3]], axis: usize) -> (f32, f32) {
    body.iter().fold(
        (f32::INFINITY, f32::NEG_INFINITY),
        |(low, high), position| (low.min(position[axis]), high.max(position[axis])),
    )
}

pub(crate) fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    }
}
