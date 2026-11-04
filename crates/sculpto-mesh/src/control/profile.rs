//! The declared garment control profile: frame, guide, apertures and limits.
//! @req DECL0013
//!
//! Everything a construction may not invent lives here as data — the source-space
//! guide, the openings a garment must keep, and the bounds on removal, closure and
//! reduction. Missing fields and unknown fields both fail, so a typo cannot become
//! a silent default.

use serde_json::{Map, Value};

use super::contact::{ContactMode, ContactPolicy};
use super::measure::CoverageLimits;
use super::{
    ApertureSpec, ControlCut, ControlError, Guide, GuideSegment, ReductionLimits, RepairLimits,
    RetainSide, SourceRecovery, SurfacePolicy, validate_apertures,
};

const FIELDS: &[&str] = &[
    "frame",
    "guide_segments",
    "apertures",
    "source_repair",
    "reduced_repair",
    "reduction",
    "coverage",
    "contact",
    "source_cuts",
    "surface",
    "source_recovery",
    "expected_genus",
];
const REPAIR_FIELDS: &[&str] = &[
    "max_incidental_loops",
    "collapse_below_edges",
    "max_fill_edges",
    "max_tip_removal_share",
];
const REDUCTION_FIELDS: &[&str] = &["target_triangles", "minimum_triangles", "maximum_triangles"];
const COVERAGE_FIELDS: &[&str] = &["tolerance", "minimum"];
const APERTURE_FIELDS: &[&str] = &["role", "anchor", "radius"];
const CONTACT_FIELDS: &[&str] = &["mode", "clearance", "max_displacement", "max_pairs"];
const CUT_FIELDS: &[&str] = &["label", "plane_point", "normal", "retain"];

/// The frame the profile's guide and aperture anchors are written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFrame {
    /// Source-local Z-up coordinates, the frame the extracted object carries.
    SourceLocalZUp,
    /// The Y-up display frame Sculpto OBJ files use.
    SculptoDisplayYUp,
}

impl ControlFrame {
    pub fn name(&self) -> &'static str {
        match self {
            Self::SourceLocalZUp => "source-local-z-up",
            Self::SculptoDisplayYUp => "sculpto-display-y-up",
        }
    }

    /// Convert one declared point into the source-local mesh frame.
    pub fn to_mesh(&self, point: [f32; 3]) -> [f32; 3] {
        match self {
            Self::SourceLocalZUp => point,
            Self::SculptoDisplayYUp => [point[0], -point[2], point[1]].map(signed_zero),
        }
    }

    /// Convert a declared direction; the frame conversion is a rotation, so no
    /// translation applies and the normal stays unit length after normalization.
    pub fn to_mesh_direction(&self, direction: [f32; 3]) -> [f32; 3] {
        match self {
            Self::SourceLocalZUp => direction,
            Self::SculptoDisplayYUp => [direction[0], -direction[2], direction[1]].map(signed_zero),
        }
    }

    fn parse(text: &str) -> Result<Self, ControlError> {
        match text {
            "source-local-z-up" => Ok(Self::SourceLocalZUp),
            "sculpto-display-y-up" => Ok(Self::SculptoDisplayYUp),
            other => Err(ControlError::InvalidProfile {
                field: "frame".to_string(),
                reason: format!(
                    "`{other}` is not a declared frame; expected source-local-z-up or \
                     sculpto-display-y-up"
                ),
            }),
        }
    }
}

/// A garment control profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlProfile {
    pub frame: ControlFrame,
    pub guide: Guide,
    /// Which source faces become the control surface; absent means `guide`.
    pub surface: SurfacePolicy,
    pub apertures: Vec<ApertureSpec>,
    /// Limits on the source-resolution stage, checked before its native fills.
    pub source_repair: RepairLimits,
    /// Limits on the post-reduction stage, checked before its fills/collapses.
    pub reduced_repair: RepairLimits,
    pub reduction: ReductionLimits,
    pub coverage: CoverageLimits,
    pub contact: ContactPolicy,
    /// Explicit source-opening cuts; empty means the source keeps its closed shell.
    pub source_cuts: Vec<ControlCut>,
    /// Whether original excluded source disks may be recovered; absent means `none`.
    pub source_recovery: SourceRecovery,
    /// Genus the finished control must have; absent means 0 (no unexpected handles).
    pub expected_genus: u32,
}

impl ControlProfile {
    /// Parse and validate a profile.
    ///
    /// Every required member must be present; only `source_cuts` (no cut is declared,
    /// so no derivative is built), `surface` (the published guide route),
    /// `source_recovery` (no recovery) and `expected_genus` (genus zero) have a
    /// documented absence. @req DECL0013
    pub fn from_json(bytes: &[u8]) -> Result<Self, ControlError> {
        let text = std::str::from_utf8(bytes).map_err(|error| ControlError::InvalidProfile {
            field: "document".to_string(),
            reason: error.to_string(),
        })?;
        let document =
            serde_json::from_str::<Value>(text).map_err(|error| ControlError::InvalidProfile {
                field: "document".to_string(),
                reason: error.to_string(),
            })?;
        let object = object(&document, "document")?;
        only(object, FIELDS, "document")?;
        let frame = ControlFrame::parse(string(require(object, "frame", "document")?, "frame")?)?;
        let guide = guide(require(object, "guide_segments", "document")?, &frame)?;
        let apertures = apertures(require(object, "apertures", "document")?, &frame)?;
        validate_apertures(&apertures)?;
        let source_repair = repair(
            require(object, "source_repair", "document")?,
            "source_repair",
        )?;
        let reduced_repair = repair(
            require(object, "reduced_repair", "document")?,
            "reduced_repair",
        )?;
        let reduction = reduction(require(object, "reduction", "document")?)?;
        let coverage = coverage(require(object, "coverage", "document")?)?;
        let contact = contact(require(object, "contact", "document")?)?;
        let surface = match object.get("surface") {
            Some(value) => surface_policy(value)?,
            None => SurfacePolicy::Guide,
        };
        let source_cuts = match object.get("source_cuts") {
            Some(value) => source_cuts(value, &frame)?,
            None => Vec::new(),
        };
        let source_recovery = match object.get("source_recovery") {
            Some(value) => source_recovery(value)?,
            None => SourceRecovery::None,
        };
        let expected_genus = match object.get("expected_genus") {
            Some(value) => genus(value)?,
            None => 0,
        };
        Ok(Self {
            frame,
            guide,
            surface,
            apertures,
            source_repair,
            reduced_repair,
            reduction,
            coverage,
            contact,
            source_cuts,
            source_recovery,
            expected_genus,
        })
    }
}

/// `-0.0` and `0.0` are the same coordinate; one spelling keeps a converted plane
/// identical to its twin in another declared frame.
fn signed_zero(value: f32) -> f32 {
    match value == 0.0 {
        true => 0.0,
        false => value,
    }
}

/// Parse the declared surface policy; only the two named routes exist.
fn surface_policy(value: &Value) -> Result<SurfacePolicy, ControlError> {
    let name = string(value, "surface")?;
    match name {
        "guide" => Ok(SurfacePolicy::Guide),
        "boundary" => Ok(SurfacePolicy::Boundary),
        other => Err(ControlError::InvalidProfile {
            field: "surface".to_string(),
            reason: format!(
                "`{other}` is not a declared surface policy; expected guide or boundary"
            ),
        }),
    }
}

/// Parse the declared source-recovery policy; only the two named values exist.
fn source_recovery(value: &Value) -> Result<SourceRecovery, ControlError> {
    let name = string(value, "source_recovery")?;
    match name {
        "none" => Ok(SourceRecovery::None),
        "incidental-disks" => Ok(SourceRecovery::IncidentalDisks),
        other => Err(ControlError::InvalidProfile {
            field: "source_recovery".to_string(),
            reason: format!(
                "`{other}` is not a declared recovery policy; expected none or incident-disks"
            ),
        }),
    }
}

/// Parse the declared topology genus; a value that is not a `u32` is refused.
fn genus(value: &Value) -> Result<u32, ControlError> {
    let number = value.as_u64().ok_or_else(|| ControlError::InvalidProfile {
        field: "expected_genus".to_string(),
        reason: "expected a nonnegative integer".to_string(),
    })?;
    u32::try_from(number).map_err(|_| ControlError::InvalidProfile {
        field: "expected_genus".to_string(),
        reason: "does not fit a genus count".to_string(),
    })
}

/// Parse and validate the optional source-opening cuts.
fn source_cuts(value: &Value, frame: &ControlFrame) -> Result<Vec<ControlCut>, ControlError> {
    let entries = value
        .as_array()
        .ok_or_else(|| ControlError::InvalidProfile {
            field: "source_cuts".to_string(),
            reason: "expected an array of cut objects".to_string(),
        })?;
    let mut labels = std::collections::HashSet::new();
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let field = format!("source_cuts[{index}]");
            let object = object(entry, &field)?;
            only(object, CUT_FIELDS, &field)?;
            let label =
                string(require(object, "label", &field)?, &format!("{field}.label"))?.to_string();
            if label.trim().is_empty() {
                return Err(ControlError::InvalidProfile {
                    field: format!("{field}.label"),
                    reason: "expected a non-blank label".to_string(),
                });
            }
            if !labels.insert(label.clone()) {
                return Err(ControlError::InvalidProfile {
                    field: format!("{field}.label"),
                    reason: format!("duplicate cut label `{label}`"),
                });
            }
            let plane_point = frame.to_mesh(point(
                require(object, "plane_point", &field)?,
                &format!("{field}.plane_point"),
            )?);
            let raw_normal = frame.to_mesh_direction(point(
                require(object, "normal", &field)?,
                &format!("{field}.normal"),
            )?);
            let length = (f64::from(raw_normal[0]).powi(2)
                + f64::from(raw_normal[1]).powi(2)
                + f64::from(raw_normal[2]).powi(2))
            .sqrt();
            // The norm is taken in f64, so any finite non-zero f32 direction has a
            // representable non-zero length: the smallest subnormal squared is ~2e-90.
            if !length.is_finite() || length == 0.0 {
                return Err(ControlError::InvalidProfile {
                    field: format!("{field}.normal"),
                    reason: "expected a nonzero finite normal".to_string(),
                });
            }
            let normal =
                raw_normal.map(|component| signed_zero((f64::from(component) / length) as f32));
            let retain = match string(
                require(object, "retain", &field)?,
                &format!("{field}.retain"),
            )? {
                "positive" => RetainSide::Positive,
                "negative" => RetainSide::Negative,
                other => {
                    return Err(ControlError::InvalidProfile {
                        field: format!("{field}.retain"),
                        reason: format!(
                            "`{other}` is not a declared side; expected positive or negative"
                        ),
                    });
                }
            };
            Ok(ControlCut {
                label,
                point: plane_point,
                normal,
                retain,
            })
        })
        .collect()
}

fn guide(value: &Value, frame: &ControlFrame) -> Result<Guide, ControlError> {
    let entries = value
        .as_array()
        .ok_or_else(|| ControlError::InvalidProfile {
            field: "guide_segments".to_string(),
            reason: "expected an array of [start, end] segments".to_string(),
        })?;
    let segments = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let field = format!("guide_segments[{index}]");
            let pair = entry
                .as_array()
                .ok_or_else(|| ControlError::InvalidProfile {
                    field: field.clone(),
                    reason: "expected [start, end]".to_string(),
                })?;
            match pair.as_slice() {
                [start, end] => Ok(GuideSegment {
                    start: frame.to_mesh(point(start, &format!("{field}[0]"))?),
                    end: frame.to_mesh(point(end, &format!("{field}[1]"))?),
                }),
                _ => Err(ControlError::InvalidProfile {
                    field,
                    reason: "expected exactly two points".to_string(),
                }),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Guide::new(segments)
}

fn apertures(value: &Value, frame: &ControlFrame) -> Result<Vec<ApertureSpec>, ControlError> {
    let entries = value
        .as_array()
        .ok_or_else(|| ControlError::InvalidProfile {
            field: "apertures".to_string(),
            reason: "expected an array of aperture objects".to_string(),
        })?;
    if entries.is_empty() {
        return Err(ControlError::EmptyProfile);
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let field = format!("apertures[{index}]");
            let object = object(entry, &field)?;
            only(object, APERTURE_FIELDS, &field)?;
            Ok(ApertureSpec {
                role: string(require(object, "role", &field)?, &format!("{field}.role"))?
                    .to_string(),
                anchor: frame.to_mesh(point(
                    require(object, "anchor", &field)?,
                    &format!("{field}.anchor"),
                )?),
                radius: number(
                    require(object, "radius", &field)?,
                    &format!("{field}.radius"),
                )?,
            })
        })
        .collect()
}

fn repair(value: &Value, stage: &str) -> Result<RepairLimits, ControlError> {
    let object = object(value, stage)?;
    only(object, REPAIR_FIELDS, stage)?;
    let limits = RepairLimits {
        max_incidental_loops: count(object, "max_incidental_loops", stage)?,
        collapse_below_edges: count(object, "collapse_below_edges", stage)?,
        max_fill_edges: count(object, "max_fill_edges", stage)?,
        max_tip_removal_share: number(
            require(object, "max_tip_removal_share", stage)?,
            &format!("{stage}.max_tip_removal_share"),
        )?,
    };
    if limits.max_tip_removal_share < 0.0 || limits.max_tip_removal_share > 1.0 {
        return Err(ControlError::InvalidProfile {
            field: format!("{stage}.max_tip_removal_share"),
            reason: "expected a share in 0..=1".to_string(),
        });
    }
    if limits.collapse_below_edges > limits.max_fill_edges {
        return Err(ControlError::InvalidProfile {
            field: format!("{stage}.max_fill_edges"),
            reason: format!("must be at least {stage}.collapse_below_edges"),
        });
    }
    Ok(limits)
}

fn contact(value: &Value) -> Result<ContactPolicy, ControlError> {
    let object = object(value, "contact")?;
    only(object, CONTACT_FIELDS, "contact")?;
    let mode = match string(require(object, "mode", "contact")?, "contact.mode")? {
        "reject" => ContactMode::Reject,
        "separate" => ContactMode::Separate,
        other => {
            return Err(ControlError::InvalidProfile {
                field: "contact.mode".to_string(),
                reason: format!("`{other}` is not a declared mode; expected reject or separate"),
            });
        }
    };
    let policy = ContactPolicy {
        mode,
        clearance: number(
            require(object, "clearance", "contact")?,
            "contact.clearance",
        )?,
        max_displacement: number(
            require(object, "max_displacement", "contact")?,
            "contact.max_displacement",
        )?,
        max_pairs: count(object, "max_pairs", "contact")?,
    };
    policy
        .validate()
        .map_err(|error| ControlError::InvalidProfile {
            field: "contact".to_string(),
            reason: error.to_string(),
        })?;
    Ok(policy)
}
fn reduction(value: &Value) -> Result<ReductionLimits, ControlError> {
    let object = object(value, "reduction")?;
    only(object, REDUCTION_FIELDS, "reduction")?;
    let limits = ReductionLimits {
        target_triangles: count(object, "target_triangles", "reduction")?,
        minimum_triangles: count(object, "minimum_triangles", "reduction")?,
        maximum_triangles: count(object, "maximum_triangles", "reduction")?,
    };
    if limits.minimum_triangles == 0
        || limits.minimum_triangles > limits.target_triangles
        || limits.target_triangles > limits.maximum_triangles
    {
        return Err(ControlError::InvalidReductionLimits {
            minimum: limits.minimum_triangles,
            target: limits.target_triangles,
            maximum: limits.maximum_triangles,
        });
    }
    Ok(limits)
}

fn coverage(value: &Value) -> Result<CoverageLimits, ControlError> {
    let object = object(value, "coverage")?;
    only(object, COVERAGE_FIELDS, "coverage")?;
    let limits = CoverageLimits {
        tolerance: number(
            require(object, "tolerance", "coverage")?,
            "coverage.tolerance",
        )?,
        minimum: number(require(object, "minimum", "coverage")?, "coverage.minimum")?,
    };
    if limits.tolerance <= 0.0 || limits.minimum <= 0.0 || limits.minimum > 1.0 {
        return Err(ControlError::InvalidCoverage {
            tolerance: limits.tolerance,
            minimum: limits.minimum,
        });
    }
    Ok(limits)
}

fn object<'a>(value: &'a Value, field: &str) -> Result<&'a Map<String, Value>, ControlError> {
    value
        .as_object()
        .ok_or_else(|| ControlError::InvalidProfile {
            field: field.to_string(),
            reason: "expected an object".to_string(),
        })
}

fn require<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a Value, ControlError> {
    object.get(key).ok_or_else(|| ControlError::InvalidProfile {
        field: context.to_string(),
        reason: format!("missing `{key}`"),
    })
}

fn only(object: &Map<String, Value>, allowed: &[&str], field: &str) -> Result<(), ControlError> {
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(ControlError::InvalidProfile {
            field: field.to_string(),
            reason: format!("unknown `{key}`"),
        }),
        None => Ok(()),
    }
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ControlError> {
    value.as_str().ok_or_else(|| ControlError::InvalidProfile {
        field: field.to_string(),
        reason: "expected a string".to_string(),
    })
}

fn number(value: &Value, field: &str) -> Result<f32, ControlError> {
    let number = value.as_f64().ok_or_else(|| ControlError::InvalidProfile {
        field: field.to_string(),
        reason: "expected a number".to_string(),
    })?;
    let number = number as f32;
    match number.is_finite() {
        true => Ok(number),
        false => Err(ControlError::InvalidProfile {
            field: field.to_string(),
            reason: "must be finite".to_string(),
        }),
    }
}

fn count(object: &Map<String, Value>, key: &str, field: &str) -> Result<usize, ControlError> {
    let value = require(object, key, field)?;
    let number = value.as_u64().ok_or_else(|| ControlError::InvalidProfile {
        field: format!("{field}.{key}"),
        reason: "expected a nonnegative integer".to_string(),
    })?;
    usize::try_from(number).map_err(|_| ControlError::InvalidProfile {
        field: format!("{field}.{key}"),
        reason: "does not fit a platform length".to_string(),
    })
}

fn point(value: &Value, field: &str) -> Result<[f32; 3], ControlError> {
    let array = value
        .as_array()
        .ok_or_else(|| ControlError::InvalidProfile {
            field: field.to_string(),
            reason: "expected [x, y, z]".to_string(),
        })?;
    match array.as_slice() {
        [x, y, z] => Ok([
            number(x, &format!("{field}[0]"))?,
            number(y, &format!("{field}[1]"))?,
            number(z, &format!("{field}[2]"))?,
        ]),
        _ => Err(ControlError::InvalidProfile {
            field: field.to_string(),
            reason: "expected exactly three coordinates".to_string(),
        }),
    }
}
