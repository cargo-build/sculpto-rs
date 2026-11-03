//! Garment classification and corset-aura profiles.
//!
//! [`classify`] measures a garment mesh against the body it must fit and
//! returns a [`GarmentKind`] with the evidence behind the verdict. The kind
//! resolves to a [`CorsetProfile`] — the corset-aura: fitting-envelope layout
//! and movement discipline per height band, and whether the garment scales to
//! its body part before fitting. Classification is geometry plus recorded
//! evidence; ambiguity is [`GarmentKind::Unknown`], never a guess.

use std::fmt;

use crate::aura::{
    AuraMode, AuraParams, BandRules, BandSpec, Layout, Segment, extent, group_segments, plane_axes,
    slice_triangle, up_axis,
};

/// Height thresholds as fractions of body height (0 = body bottom, 1 = top).
const GROUND: f32 = 0.06;
const FOOTWEAR_TOP: f32 = 0.38;
const BELT_BOTTOM: f32 = 0.40;
const BELT_TOP: f32 = 0.65;
const BELT_COVERAGE: f32 = 0.20;
const CAPE_TOP: f32 = 0.70;
const CAPE_BOTTOM: f32 = 0.45;
const CAPE_ANGULAR: f32 = 0.80;
const HIP_LINE: f32 = 0.48;
const TROUSER_TOP: f32 = 0.55;
const DRESS_TOP: f32 = 0.68;
const ROBE_RATIO: f32 = 1.70;
const SNUG_RATIO: f32 = 1.20;
const FITTED_RATIO: f32 = 1.70;
/// Sections sampled across a garment's own height range.
const SAMPLES: usize = 6;
/// Angular bins for open-shell (cape) detection; coarse enough that any
/// closed contour fills every bin.
const ANGULAR_BINS: usize = 12;
/// A connected component below this vertex fraction is an ornament, not a part.
const MAJOR_PART: f32 = 0.10;
const MIN_SCALE: f32 = 0.5;
const MAX_SCALE: f32 = 2.0;

/// What a garment is, as construction: the enum selects fitting behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GarmentKind {
    UpperBody,
    Trousers,
    Skirt,
    Dress,
    Robe,
    Footwear,
    Cape,
    Accessory,
    Composite,
    Unknown,
}

impl GarmentKind {
    /// CLI name; also the `--class` value.
    pub fn name(&self) -> &'static str {
        match self {
            Self::UpperBody => "upper-body",
            Self::Trousers => "trousers",
            Self::Skirt => "skirt",
            Self::Dress => "dress",
            Self::Robe => "robe",
            Self::Footwear => "footwear",
            Self::Cape => "cape",
            Self::Accessory => "accessory",
            Self::Composite => "composite",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a CLI kind name.
    pub fn parse(value: &str) -> Result<Self, ClassifyError> {
        [
            Self::UpperBody,
            Self::Trousers,
            Self::Skirt,
            Self::Dress,
            Self::Robe,
            Self::Footwear,
            Self::Cape,
            Self::Accessory,
        ]
        .into_iter()
        .find(|kind| kind.name() == value)
        .ok_or_else(|| ClassifyError::UnknownKind(value.to_string()))
    }
}
/// How closely the garment intends to follow the body: a classified profile
/// parameter, not a second taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adherence {
    /// Hugs the body: a corset, a base layer.
    Snug,
    /// Follows the body with ordinary ease: a shirt, trousers.
    Fitted,
    /// Keeps free volume: a cloak, a flowing robe.
    Relaxed,
}

impl Adherence {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Snug => "snug",
            Self::Fitted => "fitted",
            Self::Relaxed => "relaxed",
        }
    }

    /// Parse a CLI adherence name.
    pub fn parse(value: &str) -> Result<Self, ClassifyError> {
        match value {
            "snug" => Ok(Self::Snug),
            "fitted" => Ok(Self::Fitted),
            "relaxed" => Ok(Self::Relaxed),
            other => Err(ClassifyError::UnknownAdherence(other.to_string())),
        }
    }
}

/// How the garment reaches the body before the radial fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartFit {
    /// Press straight onto the aura.
    Radial,
    /// Scale to the body part's section extents first (footwear).
    ScaleToPart,
}

/// Where the profile declares an attachment; enforced by the offline cloth
/// phase, reported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    None,
    Waist,
    Shoulders,
}

impl Anchor {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Waist => "waist",
            Self::Shoulders => "shoulders",
        }
    }
}

/// The corset-aura of one garment: resolved `AuraParams` plus fit behavior.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorsetProfile {
    /// Kind the profile was resolved from.
    pub kind: GarmentKind,
    /// Classified adherence behind the movement parameters.
    pub adherence: Adherence,
    /// Fully resolved aura parameters (`mode` is the above-band mode when
    /// `params.bands` is set).
    pub params: AuraParams,
    /// Declared attachment.
    pub anchor: Anchor,
    /// Scale-to-part before the radial fit.
    pub fit: PartFit,
}

/// The measured shape facts the verdict rests on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// Garment bottom as a body-height fraction.
    pub bottom: f32,
    /// Garment top as a body-height fraction.
    pub top: f32,
    /// `top - bottom`.
    pub coverage: f32,
    /// Most garment contour columns over the sampled sections.
    pub columns: usize,
    /// Most garment contour columns below the hip line, where the body has
    /// two legs: the trouser/skirt discriminator, sleeve-blind.
    pub leg_columns: usize,
    /// Most body contour columns over the same sections.
    pub body_columns: usize,
    /// One garment column wrapped two or more body columns for at least half
    /// the sampled sections: a merged skirt volume.
    pub merged: bool,
    /// Mean garment-over-body contour extent where both exist.
    pub radial_ratio: f32,
    /// Same, restricted to sections at or above the hip line.
    pub top_ratio: f32,
    /// Mean angular coverage of the garment around the body section centroid.
    pub angular: f32,
    /// Major connected components (parts of a composite).
    pub components: usize,
    /// Garment vertices in the classified part.
    pub vertices: usize,
}

/// One classification verdict: kind, parameters and the evidence trail.
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    /// Verdict.
    pub kind: GarmentKind,
    /// Classified adherence.
    pub adherence: Adherence,
    /// Measured features.
    pub features: Features,
    /// Per-part verdicts; non-empty only for `Composite`.
    pub parts: Vec<Classified>,
    /// Why the verdict is what it is, one line per branch taken.
    pub evidence: Vec<String>,
    /// Body frame the fractions refer to (kept private: callers use
    /// `profile` and `scale_to_part`, which need the frame).
    frame: Frame,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Frame {
    axis: usize,
    min: f32,
    height: f32,
}

/// Why classification could not run.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassifyError {
    /// The body has no vertices.
    EmptyBody,
    /// The body is flat on its up axis; fractions are undefined.
    FlatBody,
    /// The garment has no vertices.
    EmptyGarment,
    /// The garment has no triangles; contours cannot be measured.
    NoGarmentTriangles,
    /// A garment triangle references a vertex the garment does not have.
    BadIndex { triangle: usize, vertex: usize },
    /// The kind name is not one of the classifiable kinds.
    UnknownKind(String),
    /// The adherence name is not snug, fitted or relaxed.
    UnknownAdherence(String),
}

impl fmt::Display for ClassifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyBody => write!(formatter, "the body has no vertices"),
            Self::FlatBody => write!(formatter, "the body has no height on its up axis"),
            Self::EmptyGarment => write!(formatter, "the garment has no vertices"),
            Self::NoGarmentTriangles => write!(formatter, "the garment has no triangles"),
            Self::BadIndex { triangle, vertex } => write!(
                formatter,
                "garment triangle {triangle} references vertex {vertex}, which does not exist"
            ),
            Self::UnknownKind(name) => write!(
                formatter,
                "unknown kind `{name}`: expected one of upper-body, trousers, skirt, dress, \
                 robe, footwear, cape, accessory"
            ),
            Self::UnknownAdherence(name) => write!(
                formatter,
                "unknown adherence `{name}`: expected snug, fitted or relaxed"
            ),
        }
    }
}

impl std::error::Error for ClassifyError {}

/// One section's contours of a mesh, in the section plane.
struct Section {
    /// Contour point pairs, one vec per connected contour, flattened as
    /// `(start, end)` segment pairs.
    groups: Vec<Vec<[f32; 2]>>,
}

impl Section {
    fn of(positions: &[[f32; 3]], triangles: &[[u32; 3]], axis: usize, height: f32) -> Self {
        let (plane_u, plane_v) = plane_axes(axis);
        let segments: Vec<Segment> = triangles
            .iter()
            .filter_map(|triangle| {
                slice_triangle(
                    [
                        &positions[triangle[0] as usize],
                        &positions[triangle[1] as usize],
                        &positions[triangle[2] as usize],
                    ],
                    axis,
                    height,
                    plane_u,
                    plane_v,
                )
            })
            .collect();
        Self {
            groups: group_segments(&segments)
                .into_iter()
                .map(|group| {
                    group
                        .iter()
                        .flat_map(|(start, end)| [*start, *end])
                        .collect()
                })
                .collect(),
        }
    }

    /// Contours with nonzero total segment length count as a column.
    fn columns(&self) -> usize {
        self.groups
            .iter()
            .filter(|group| {
                group
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| (pair[0][0] - pair[1][0]).hypot(pair[0][1] - pair[1][1]))
                    .sum::<f32>()
                    > 1e-6
            })
            .count()
    }

    fn points(&self) -> impl Iterator<Item = [f32; 2]> + '_ {
        self.groups.iter().flatten().copied()
    }
}

/// Classify a garment against the body it must fit. Both meshes must share a
/// frame (the same requirement as the aura fit).
pub fn classify(
    body: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
) -> Result<Classified, ClassifyError> {
    if body.is_empty() {
        return Err(ClassifyError::EmptyBody);
    }
    if garment.is_empty() {
        return Err(ClassifyError::EmptyGarment);
    }

    if garment_triangles.is_empty() {
        return Err(ClassifyError::NoGarmentTriangles);
    }
    for (triangle, [a, b, c]) in garment_triangles.iter().enumerate() {
        if [*a, *b, *c]
            .iter()
            .any(|index| *index as usize >= garment.len())
        {
            return Err(ClassifyError::BadIndex {
                triangle,
                vertex: [*a, *b, *c].iter().copied().max().unwrap_or_default() as usize,
            });
        }
    }
    let axis = up_axis(body);
    let (min, max) = extent(body, axis);
    if max - min <= f32::EPSILON {
        return Err(ClassifyError::FlatBody);
    }
    let frame = Frame {
        axis,
        min,
        height: max - min,
    };

    let parts = major_components(garment, garment_triangles);
    let mut whole = classify_single(frame, garment, garment_triangles, body, body_triangles);
    whole.features.components = parts.len();

    // A pair of boots or a two-piece outfit arrives as several disconnected
    // components. The union's shape decides when it can: two grounded columns
    // are footwear however many components they are. Only an inconclusive
    // union falls back to per-part verdicts.
    if parts.len() >= 2 {
        if whole.kind != GarmentKind::Unknown {
            whole.evidence.insert(
                0,
                format!(
                    "{} disconnected parts classified together as {}",
                    parts.len(),
                    whole.kind.name()
                ),
            );
            return Ok(whole);
        }
        let mut verdicts = Vec::with_capacity(parts.len());
        for (positions, triangles) in &parts {
            verdicts.push(classify_single(
                frame,
                positions,
                triangles,
                body,
                body_triangles,
            ));
        }
        let total: usize = parts.iter().map(|(positions, _)| positions.len()).sum();
        let shares: Vec<String> = parts
            .iter()
            .map(|(positions, _)| format!("{:.0}%", 100.0 * positions.len() as f32 / total as f32))
            .collect();
        let mut evidence = vec![format!(
            "{} major components ({}) -> composite outfit",
            parts.len(),
            shares.join("/")
        )];
        for verdict in &verdicts {
            evidence.push(format!(
                "part: {} ({})",
                verdict.kind.name(),
                verdict.adherence.name()
            ));
        }
        // Composite features describe the widest part; adherence follows it.
        let mut widest = verdicts
            .iter()
            .max_by(|left, right| left.features.coverage.total_cmp(&right.features.coverage))
            .expect("composite has parts")
            .clone();
        widest.features.components = verdicts.len();
        widest.kind = GarmentKind::Composite;
        widest.parts = verdicts;
        widest.evidence = evidence;
        return Ok(widest);
    }
    if let Some((positions, _)) = parts.first() {
        let unreferenced = garment.len() - positions.len();
        if unreferenced > 0 {
            whole
                .evidence
                .push(format!("{unreferenced} unreferenced vertices ignored"));
        }
    }
    Ok(whole)
}

fn classify_single(
    frame: Frame,
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
    body: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
) -> Classified {
    let (gmin, gmax) = extent(garment, frame.axis);
    let bottom = ((gmin - frame.min) / frame.height).clamp(0.0, 1.0);
    let top = ((gmax - frame.min) / frame.height).clamp(0.0, 1.0);
    let coverage = top - bottom;
    let margin = 0.02 * frame.height;
    let mut columns = 0;
    let mut body_columns = 0;
    let mut ratios = Vec::new();
    let mut top_ratios = Vec::new();
    let mut angulars = Vec::new();
    let mut merged_samples = 0;
    let mut leg_columns = 0;

    for height in sample_heights(gmin + margin, gmax - margin) {
        let body_section = Section::of(body, body_triangles, frame.axis, height);
        let garment_section = Section::of(garment, garment_triangles, frame.axis, height);
        let garment_columns = garment_section.columns();
        let body_column_count = body_section.columns();
        columns = columns.max(garment_columns);
        if (height - frame.min) / frame.height < HIP_LINE {
            leg_columns = leg_columns.max(garment_columns);
        }
        body_columns = body_columns.max(body_column_count);

        let Some(body_center) = centroid(body_section.points()) else {
            continue;
        };
        let body_radius = radius_of(body_section.points(), body_center);
        if body_radius <= f32::EPSILON {
            continue;
        }
        let garment_radius = radius_of(garment_section.points(), body_center);
        if garment_radius <= f32::EPSILON {
            continue;
        }
        // A merged volume sits between the body columns (a skirt around both
        // legs); a single-column part around one leg does not.
        if garment_columns < body_column_count {
            let centered = centroid(garment_section.points()).is_some_and(|center| {
                (center[0] - body_center[0]).hypot(center[1] - body_center[1]) <= 0.25 * body_radius
            });
            if centered {
                merged_samples += 1;
            }
        }
        ratios.push(garment_radius / body_radius);
        if (height - frame.min) / frame.height >= HIP_LINE {
            top_ratios.push(garment_radius / body_radius);
        }
        angulars.push(angular_coverage(garment_section.points(), body_center));
    }

    let radial_ratio = ratios.iter().sum::<f32>() / ratios.len().max(1) as f32;
    let top_ratio = top_ratios.iter().sum::<f32>() / top_ratios.len().max(1) as f32;
    let angular = angulars.iter().sum::<f32>() / angulars.len().max(1) as f32;
    let adherence = if ratios.is_empty() {
        Adherence::Fitted
    } else if radial_ratio < SNUG_RATIO {
        Adherence::Snug
    } else if radial_ratio < FITTED_RATIO {
        Adherence::Fitted
    } else {
        Adherence::Relaxed
    };
    let features = Features {
        bottom,
        top,
        coverage,
        columns,
        leg_columns,
        body_columns,
        merged: merged_samples >= SAMPLES / 2,
        radial_ratio,
        top_ratio,
        angular,
        components: 1,
        vertices: garment.len(),
    };

    decide(frame, features, adherence, radial_ratio, top_ratio)
}

/// The decision tree: first match wins, every branch records its evidence.
fn decide(
    frame: Frame,
    features: Features,
    adherence: Adherence,
    radial_ratio: f32,
    top_ratio: f32,
) -> Classified {
    let mut evidence = Vec::new();
    // Column counts on dense real meshes carry noise (open chains, detail
    // chains), so branches read them as small ranges, never exact equality.
    let kind = if features.bottom <= GROUND
        && features.top <= FOOTWEAR_TOP
        && (2..=4).contains(&features.leg_columns)
    {
        evidence.push(format!(
            "grounded (bottom {:.2}) two-column garment to {:.2} -> footwear",
            features.bottom, features.top
        ));
        GarmentKind::Footwear
    } else if features.coverage <= BELT_COVERAGE
        && features.bottom >= BELT_BOTTOM
        && features.top <= BELT_TOP
        && features.columns <= 2
    {
        evidence.push(format!(
            "narrow band {:.2}..{:.2} around one column -> accessory",
            features.bottom, features.top
        ));
        GarmentKind::Accessory
    } else if features.top >= CAPE_TOP
        && features.bottom <= CAPE_BOTTOM
        && features.angular < CAPE_ANGULAR
        && features.columns <= 3
    {
        evidence.push(format!(
            "open shell (angular {:.2}, {} columns) over {:.2}..{:.2} -> cape",
            features.angular, features.columns, features.bottom, features.top
        ));
        GarmentKind::Cape
    } else if features.bottom <= HIP_LINE {
        if (2..=4).contains(&features.leg_columns) {
            if features.top <= TROUSER_TOP {
                evidence.push(format!(
                    "two columns below the hip line ({:.2}) -> trousers",
                    features.bottom
                ));
                GarmentKind::Trousers
            } else {
                evidence.push(format!(
                    "two columns to {:.2}: above the trouser line and no jumpsuit kind \
                     -> unknown (nearest: trousers, dress)",
                    features.top
                ));
                GarmentKind::Unknown
            }
        } else if features.leg_columns >= 5 {
            evidence.push(format!(
                "leg-band sections fragment into {} columns: not one garment \
                 silhouette -> unknown (split the parts or pass --class)",
                features.leg_columns
            ));
            GarmentKind::Unknown
        } else if features.leg_columns <= 1 && features.merged {
            if features.top >= DRESS_TOP {
                if top_ratio < ROBE_RATIO {
                    evidence.push(format!(
                        "one merged column around two legs, fitted top (ratio {:.2}) -> dress",
                        top_ratio
                    ));
                    GarmentKind::Dress
                } else {
                    evidence.push(format!(
                        "one merged column around two legs, full top (ratio {:.2}) -> robe",
                        top_ratio
                    ));
                    GarmentKind::Robe
                }
            } else {
                evidence.push(
                    "one merged column around two legs below the hip line -> skirt".to_string(),
                );
                GarmentKind::Skirt
            }
        } else {
            evidence.push(format!(
                "reaches below the hip line ({:.2}) but follows one body column \
                 -> upper body, long cut",
                features.bottom
            ));
            GarmentKind::UpperBody
        }
    } else {
        evidence.push(format!(
            "single column above the hip line ({:.2}..{:.2}) -> upper body",
            features.bottom, features.top
        ));
        GarmentKind::UpperBody
    };
    evidence.push(format!(
        "radial ratio {:.2} (top {:.2}), angular {:.2} -> adherence {}",
        radial_ratio,
        top_ratio,
        features.angular,
        adherence.name()
    ));
    Classified {
        kind,
        adherence,
        features,
        parts: Vec::new(),
        evidence,
        frame,
    }
}

impl Classified {
    /// Resolve the corset-aura profile. `None` for `Unknown` (report the
    /// alternatives instead of fitting) and `Composite` (use the parts).
    pub fn profile(&self) -> Option<CorsetProfile> {
        let (mode, clearance_fraction, slack, max_ease) = adherence_params(self.adherence);
        // Clearance is a distance the raw body never crosses, stated as a
        // fraction of body height so the same profile fits Skyrim units and
        // meter-scale actor scenes (0.006 x 1.6 m ≈ 1 cm on a human).
        let clearance = clearance_fraction * self.frame.height;
        let mut params = AuraParams {
            mode,
            clearance,
            slack,
            max_ease,
            ..AuraParams::default()
        };

        let split_at = |fraction: f32| self.frame.min + fraction * self.frame.height;
        let (bands, anchor, fit) = match self.kind {
            GarmentKind::Skirt => (
                Some(BandRules {
                    split: split_at((self.features.top - 0.03).max(0.0)),
                    above: BandSpec {
                        layout: Layout::PerContour,
                        mode: AuraMode::Press,
                    },
                    below: BandSpec {
                        layout: Layout::Merged,
                        mode: AuraMode::Loose,
                    },
                }),
                Anchor::Waist,
                PartFit::Radial,
            ),
            GarmentKind::Dress | GarmentKind::Robe => {
                if self.kind == GarmentKind::Robe {
                    // At least 1% of body height and 10% perimeter ease.
                    params.clearance = params.clearance.max(0.01 * self.frame.height);
                    params.slack = params.slack.max(0.10);
                }
                (
                    Some(BandRules {
                        split: split_at(HIP_LINE),
                        above: BandSpec {
                            layout: Layout::PerContour,
                            mode,
                        },
                        below: BandSpec {
                            layout: Layout::Merged,
                            mode: AuraMode::Loose,
                        },
                    }),
                    Anchor::Waist,
                    PartFit::Radial,
                )
            }
            GarmentKind::Footwear => (None, Anchor::None, PartFit::ScaleToPart),
            GarmentKind::Cape => (None, Anchor::Shoulders, PartFit::Radial),
            GarmentKind::Trousers => (None, Anchor::Waist, PartFit::Radial),
            GarmentKind::UpperBody | GarmentKind::Accessory => {
                (None, Anchor::None, PartFit::Radial)
            }
            GarmentKind::Composite | GarmentKind::Unknown => return None,
        };
        params.bands = bands;
        Some(CorsetProfile {
            kind: self.kind,
            adherence: self.adherence,
            params,
            anchor,
            fit,
        })
    }
}

/// Movement parameters per adherence: snug hugs, fitted follows with ease,
/// relaxed keeps free volume. The clearance value is a fraction of body
/// height (0.006 ≈ the classic 0.6 Skyrim units on a 100-unit body).
fn adherence_params(adherence: Adherence) -> (AuraMode, f32, f32, f32) {
    match adherence {
        Adherence::Snug => (AuraMode::Press, 0.003, 0.02, 1.15),
        Adherence::Fitted => (AuraMode::Press, 0.006, 0.05, 1.5),
        Adherence::Relaxed => (AuraMode::Loose, 0.010, 0.10, 1.5),
    }
}

/// What scaling to the body part did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleReport {
    /// Applied uniform plane scale.
    pub scale: f32,
    /// Plane point the scale pivoted on (the garment's bottom-center).
    pub anchor: [f32; 2],
    /// Whether the body offered contours to measure against.
    pub applied: bool,
}

/// Scale a garment's plane extents to the body's extents over the garment's
/// own height band (footwear's scalable behavior), anchored at the garment's
/// bottom-center; the up axis is left to the radial fit.
pub fn scale_to_part(
    body: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
    garment: &[[f32; 3]],
    garment_triangles: &[[u32; 3]],
    classified: &Classified,
) -> (Vec<[f32; 3]>, ScaleReport) {
    let (plane_u, plane_v) = plane_axes(classified.frame.axis);
    let (gmin, gmax) = extent(garment, classified.frame.axis);
    let margin = 0.02 * classified.frame.height;
    let heights = sample_heights(gmin + margin, gmax - margin);
    let mut body_points: Vec<[f32; 2]> = Vec::new();
    let mut garment_points: Vec<[f32; 2]> = Vec::new();
    let mut anchor_points: Vec<[f32; 2]> = Vec::new();
    for (index, height) in heights.iter().enumerate() {
        let body_section = Section::of(body, body_triangles, classified.frame.axis, *height);
        let garment_section =
            Section::of(garment, garment_triangles, classified.frame.axis, *height);
        body_points.extend(body_section.points());
        garment_points.extend(garment_section.points());
        if index == 0 {
            anchor_points.extend(garment_section.points());
        }
    }
    let unapplied = || {
        (
            garment.to_vec(),
            ScaleReport {
                scale: 1.0,
                anchor: [0.0, 0.0],
                applied: false,
            },
        )
    };
    if body_points.is_empty() || garment_points.is_empty() || anchor_points.is_empty() {
        return unapplied();
    }
    let extents = |points: &[[f32; 2]]| {
        (
            points
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p[0]), hi.max(p[0]))
                }),
            points
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p[1]), hi.max(p[1]))
                }),
        )
    };
    let ((bu_lo, bu_hi), (bv_lo, bv_hi)) = extents(&body_points);
    let ((gu_lo, gu_hi), (gv_lo, gv_hi)) = extents(&garment_points);
    let ratio = |body_span: f32, garment_span: f32| {
        if garment_span > 1e-6 {
            body_span / garment_span
        } else {
            1.0
        }
    };
    let scale = (ratio(bu_hi - bu_lo, gu_hi - gu_lo) * ratio(bv_hi - bv_lo, gv_hi - gv_lo))
        .sqrt()
        .clamp(MIN_SCALE, MAX_SCALE);
    let anchor = [
        anchor_points.iter().map(|p| p[0]).sum::<f32>() / anchor_points.len() as f32,
        anchor_points.iter().map(|p| p[1]).sum::<f32>() / anchor_points.len() as f32,
    ];
    let scaled = garment
        .iter()
        .map(|position| {
            let mut moved = *position;
            moved[plane_u] = anchor[0] + (position[plane_u] - anchor[0]) * scale;
            moved[plane_v] = anchor[1] + (position[plane_v] - anchor[1]) * scale;
            moved
        })
        .collect();
    (
        scaled,
        ScaleReport {
            scale,
            anchor,
            applied: true,
        },
    )
}

fn sample_heights(low: f32, high: f32) -> Vec<f32> {
    if high <= low {
        return Vec::new();
    }
    (0..SAMPLES)
        .map(|index| low + (high - low) * index as f32 / (SAMPLES - 1) as f32)
        .collect()
}

fn centroid(points: impl Iterator<Item = [f32; 2]>) -> Option<[f32; 2]> {
    let mut sum = [0.0f32; 2];
    let mut count = 0usize;
    for point in points {
        sum[0] += point[0];
        sum[1] += point[1];
        count += 1;
    }
    (count > 0).then(|| [sum[0] / count as f32, sum[1] / count as f32])
}

fn radius_of(points: impl Iterator<Item = [f32; 2]>, center: [f32; 2]) -> f32 {
    points
        .map(|point| (point[0] - center[0]).hypot(point[1] - center[1]))
        .fold(0.0, f32::max)
}

fn angular_coverage(points: impl Iterator<Item = [f32; 2]>, center: [f32; 2]) -> f32 {
    let mut occupied = [false; ANGULAR_BINS];
    let mut count = 0usize;
    for point in points {
        let angle = (point[1] - center[1])
            .atan2(point[0] - center[0])
            .rem_euclid(std::f32::consts::TAU);
        let bin = (angle / std::f32::consts::TAU * ANGULAR_BINS as f32) as usize % ANGULAR_BINS;
        occupied[bin] = true;
        count += 1;
    }
    if count == 0 {
        0.0
    } else {
        occupied.iter().filter(|filled| **filled).count() as f32 / ANGULAR_BINS as f32
    }
}

/// Positions plus triangles of one connected component.
type Part = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// Every connected component of the mesh as a vertex-index group, largest
/// first; empty groups (unreferenced vertices) are dropped.
pub(crate) fn component_groups(garment: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..garment.len()).collect();
    for [a, b, c] in triangles {
        for (left, right) in [(a, b), (b, c), (a, c)] {
            let (left, right) = (*left as usize, *right as usize);
            let (left, right) = (find_root(&mut parent, left), find_root(&mut parent, right));
            if left != right {
                parent[right] = left;
            }
        }
    }
    let mut members: Vec<Option<Vec<usize>>> = vec![None; garment.len()];
    for index in 0..garment.len() {
        let root = find_root(&mut parent, index);
        members[root].get_or_insert_with(Vec::new).push(index);
    }
    let mut groups: Vec<Vec<usize>> = members.into_iter().flatten().collect();
    groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    groups
}

/// Split a mesh into connected components of at least `MAJOR_PART` of the
/// referenced vertices, reindexed to their own vertex range.
pub(crate) fn major_components(garment: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<Part> {
    let mut parent: Vec<usize> = (0..garment.len()).collect();
    for [a, b, c] in triangles {
        for (left, right) in [(a, b), (b, c), (a, c)] {
            let (left, right) = (*left as usize, *right as usize);
            let (left, right) = (find_root(&mut parent, left), find_root(&mut parent, right));
            if left != right {
                parent[right] = left;
            }
        }
    }
    let mut members: Vec<Option<Vec<usize>>> = vec![None; garment.len()];
    for index in 0..garment.len() {
        let root = find_root(&mut parent, index);
        members[root].get_or_insert_with(Vec::new).push(index);
    }
    let total: usize = members.iter().flatten().map(Vec::len).sum();
    let mut major: Vec<Vec<usize>> = Vec::new();
    for group in members.into_iter().flatten() {
        if !group.is_empty() && group.len() as f32 >= MAJOR_PART * total as f32 {
            major.push(group);
        }
    }
    major
        .into_iter()
        .map(|indices| {
            let mut reindex: Vec<Option<u32>> = vec![None; garment.len()];
            let positions: Vec<[f32; 3]> = indices
                .iter()
                .enumerate()
                .map(|(new, &old)| {
                    reindex[old] = Some(new as u32);
                    garment[old]
                })
                .collect();
            let part_triangles: Vec<[u32; 3]> = triangles
                .iter()
                .filter(|[a, _, _]| reindex[*a as usize].is_some())
                .map(|[a, b, c]| {
                    [
                        reindex[*a as usize].expect("triangle vertex is in the component"),
                        reindex[*b as usize].expect("triangle vertex is in the component"),
                        reindex[*c as usize].expect("triangle vertex is in the component"),
                    ]
                })
                .collect();
            (positions, part_triangles)
        })
        .filter(|(_, triangles)| !triangles.is_empty())
        .collect()
}

fn find_root(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        parent[index] = parent[parent[index]];
        index = parent[index];
    }
    index
}
