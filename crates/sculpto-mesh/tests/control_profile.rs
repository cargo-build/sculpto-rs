//! The control profile is data: it validates, converts frames and rejects typos.

use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{ContactMode, ControlError, ControlFrame, GuideSegment};

const VALID: &str = r#"{
  "frame": "source-local-z-up",
  "guide_segments": [
    [[0.0, 0.0, -1.0], [0.0, 0.0, 0.0]],
    [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
  ],
  "apertures": [
    {"role": "hem", "anchor": [0.0, 0.0, -1.0], "radius": 0.05},
    {"role": "collar", "anchor": [0.0, 0.0, 1.0], "radius": 0.05}
  ],
  "source_repair": {
    "max_incidental_loops": 4,
    "collapse_below_edges": 0,
    "max_fill_edges": 16,
    "max_tip_removal_share": 0.05
  },
  "reduced_repair": {
    "max_incidental_loops": 4,
    "collapse_below_edges": 10,
    "max_fill_edges": 64,
    "max_tip_removal_share": 0.05
  },
  "reduction": {
    "target_triangles": 1800,
    "minimum_triangles": 1200,
    "maximum_triangles": 1890
  },
  "coverage": {"tolerance": 0.004, "minimum": 0.95},
  "contact": {
    "mode": "separate",
    "clearance": 0.000002,
    "max_displacement": 0.0001,
    "max_pairs": 1
  }
}"#;

fn profile(text: &str) -> Result<ControlProfile, ControlError> {
    ControlProfile::from_json(text.as_bytes())
}

#[test]
fn a_complete_profile_round_trips_its_declared_values() {
    let profile = profile(VALID).unwrap();
    assert_eq!(profile.frame, ControlFrame::SourceLocalZUp);
    assert_eq!(profile.frame.name(), "source-local-z-up");
    assert_eq!(profile.guide.segments().len(), 2);
    assert_eq!(profile.apertures.len(), 2);
    assert_eq!(profile.apertures[0].role, "hem");
    assert_eq!(profile.source_repair.max_fill_edges, 16);
    assert_eq!(profile.reduced_repair.max_fill_edges, 64);
    assert_eq!(profile.contact.mode, ContactMode::Separate);
    assert_eq!(profile.contact.max_pairs, 1);
    assert_eq!(profile.reduction.target_triangles, 1800);
    assert_eq!(profile.coverage.minimum, 0.95);
}

#[test]
fn the_display_frame_is_converted_into_the_mesh_frame() {
    // Display (x, y, z) is source (x, -z, y): a display-frame `+y` point is the
    // source `+z` axis.
    let display = profile(&display_frame(VALID)).unwrap();
    assert_eq!(display.frame, ControlFrame::SculptoDisplayYUp);
    assert_eq!(display.frame.name(), "sculpto-display-y-up");
    assert_eq!(
        display.guide.segments(),
        [
            GuideSegment {
                start: [0.0, 0.0, -1.0],
                end: [0.0, 0.0, 0.0],
            },
            GuideSegment {
                start: [0.0, 0.0, 0.0],
                end: [0.0, 0.0, 1.0],
            },
        ]
    );
    assert_eq!(
        display.apertures[0].anchor,
        [0.0, 0.0, -1.0],
        "an anchor follows the guide's frame"
    );
}

#[test]
fn a_display_frame_profile_lands_in_the_same_source_axis() {
    let source = profile(VALID).unwrap();
    let display = profile(&display_frame(VALID)).unwrap();
    for (left, right) in source.guide.segments().iter().zip(display.guide.segments()) {
        assert_eq!(left.start, right.start);
        assert_eq!(left.end, right.end);
    }
    for (left, right) in source.apertures.iter().zip(&display.apertures) {
        assert_eq!(left.anchor, right.anchor);
    }
}

/// `VALID` with the frame name changed and every point moved to display axes,
/// where display `(x, y, z)` is source `(x, -z, y)`.
fn display_frame(text: &str) -> String {
    text.replace("source-local-z-up", "sculpto-display-y-up")
        .replace("[0.0, 0.0, -1.0]", "[0.0, -1.0, 0.0]")
        .replace("[0.0, 0.0, 1.0]", "[0.0, 1.0, 0.0]")
}

#[test]
fn a_missing_field_is_named() {
    let text = VALID.replace(
        ",\n  \"coverage\": {\"tolerance\": 0.004, \"minimum\": 0.95}",
        "",
    );
    assert_eq!(
        profile(&text),
        Err(ControlError::InvalidProfile {
            field: "document".to_string(),
            reason: "missing `coverage`".to_string(),
        })
    );
}

#[test]
fn an_unknown_field_is_a_typo_not_a_default() {
    let text = VALID.replace("\"frame\"", "\"framee\"");
    assert_eq!(
        profile(&text),
        Err(ControlError::InvalidProfile {
            field: "document".to_string(),
            reason: "unknown `framee`".to_string(),
        })
    );
}

#[test]
fn an_unknown_frame_is_rejected() {
    let text = VALID.replace("source-local-z-up", "blender-world-z-up");
    let error = profile(&text).unwrap_err();
    assert!(
        matches!(&error, ControlError::InvalidProfile { field, .. } if field == "frame"),
        "{error}"
    );
    assert!(
        error.to_string().contains("sculpto-display-y-up"),
        "{error}"
    );
}

#[test]
fn degenerate_guides_and_bad_apertures_are_rejected() {
    let flat = VALID.replace(
        "[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0]]",
        "[[0.0, 0.0, 0.5], [0.0, 0.0, 0.5]]",
    );
    assert_eq!(
        profile(&flat),
        Err(ControlError::DegenerateSegment { segment: 1 })
    );
    let no_apertures = VALID.replace(
        r#"{"role": "hem", "anchor": [0.0, 0.0, -1.0], "radius": 0.05},
    {"role": "collar", "anchor": [0.0, 0.0, 1.0], "radius": 0.05}"#,
        "",
    );
    assert_eq!(profile(&no_apertures), Err(ControlError::EmptyProfile));
    let zero_radius = VALID.replace("\"radius\": 0.05", "\"radius\": 0.0");
    assert_eq!(
        profile(&zero_radius),
        Err(ControlError::NonPositiveRadius {
            role: "hem".to_string()
        })
    );
    let short_anchor = VALID.replace("[0.0, 0.0, -1.0], \"radius\"", "[0.0, 0.0], \"radius\"");
    let error = profile(&short_anchor).unwrap_err();
    assert!(error.to_string().contains("apertures[0].anchor"), "{error}");
}

#[test]
fn reduction_bounds_must_be_ordered() {
    let inverted = VALID.replace("\"minimum_triangles\": 1200", "\"minimum_triangles\": 2000");
    assert_eq!(
        profile(&inverted),
        Err(ControlError::InvalidReductionLimits {
            minimum: 2000,
            target: 1800,
            maximum: 1890,
        })
    );
    let widened = VALID.replace("\"maximum_triangles\": 1890", "\"maximum_triangles\": 1700");
    assert_eq!(
        profile(&widened),
        Err(ControlError::InvalidReductionLimits {
            minimum: 1200,
            target: 1800,
            maximum: 1700,
        })
    );
}

#[test]
fn repair_and_coverage_bounds_are_checked() {
    let share = VALID.replace(
        "\"max_tip_removal_share\": 0.05",
        "\"max_tip_removal_share\": 1.5",
    );
    let error = profile(&share).unwrap_err();
    assert!(error.to_string().contains("0..=1"), "{error}");
    let order = VALID.replace(
        "\"collapse_below_edges\": 10",
        "\"collapse_below_edges\": 99",
    );
    let error = profile(&order).unwrap_err();
    assert!(error.to_string().contains("max_fill_edges"), "{error}");
    let coverage = VALID.replace("\"minimum\": 0.95", "\"minimum\": 1.5");
    assert_eq!(
        profile(&coverage),
        Err(ControlError::InvalidCoverage {
            tolerance: 0.004,
            minimum: 1.5,
        })
    );
}

#[test]
fn a_non_numeric_value_is_not_coerced() {
    let text = VALID.replace("\"radius\": 0.05", "\"radius\": \"0.05\"");
    let error = profile(&text).unwrap_err();
    assert!(
        matches!(&error, ControlError::InvalidProfile { field, .. } if field == "apertures[0].radius"),
        "{error}"
    );
    let malformed = profile("{\n  \"frame\":");
    assert!(matches!(
        malformed,
        Err(ControlError::InvalidProfile { field, .. }) if field == "document"
    ));
}
