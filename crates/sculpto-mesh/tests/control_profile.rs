//! The control profile is data: it validates, converts frames and rejects typos.

use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{
    ContactMode, ControlError, ControlFrame, GuideSegment, RetainSide, SourceRecovery,
};

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
fn source_recovery_and_declared_genus_are_optional_and_strict() {
    // Absence preserves the published route and the genus-zero expectation.
    let absent = profile(VALID).unwrap();
    assert_eq!(absent.source_recovery, SourceRecovery::None);
    assert_eq!(absent.source_recovery.name(), "none");
    assert_eq!(absent.expected_genus, 0);

    let declared = profile(&with_fields(
        r#""source_recovery": "incidental-disks", "expected_genus": 4"#,
    ))
    .unwrap();
    assert_eq!(declared.source_recovery, SourceRecovery::IncidentalDisks);
    assert_eq!(declared.source_recovery.name(), "incidental-disks");
    assert_eq!(declared.expected_genus, 4);

    let explicit_none = profile(&with_fields(r#""source_recovery": "none""#)).unwrap();
    assert_eq!(explicit_none.source_recovery, SourceRecovery::None);

    for (member, field, fragment) in [
        (
            r#""source_recovery": "nearest-disk""#,
            "source_recovery",
            "none or incident-disks",
        ),
        (
            r#""source_recovery": 1"#,
            "source_recovery",
            "expected a string",
        ),
        (
            r#""expected_genus": "one""#,
            "expected_genus",
            "nonnegative integer",
        ),
        (
            r#""expected_genus": -1"#,
            "expected_genus",
            "nonnegative integer",
        ),
        (
            r#""expected_genus": 1.5"#,
            "expected_genus",
            "nonnegative integer",
        ),
        (
            r#""expected_genus": 4294967296"#,
            "expected_genus",
            "does not fit a genus count",
        ),
    ] {
        let error = profile(&with_fields(member)).unwrap_err();
        assert!(
            matches!(&error, ControlError::InvalidProfile { field: named, .. } if named == field),
            "{member}: {error:?}"
        );
        assert!(error.to_string().contains(fragment), "{member}: {error}");
    }
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

const CUTS: &str = r#"[
    {"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 2.0], "retain": "negative"},
    {"label": "left-cuff", "plane_point": [0.0, 0.0, 0.4], "normal": [0.0, 0.0, -1.0], "retain": "positive"}
  ]"#;

/// `VALID` with extra members appended.
fn with_fields(members: &str) -> String {
    let body = VALID.trim_end().strip_suffix('}').unwrap();
    format!("{body},\n  {members}\n}}")
}

/// `VALID` with an optional `source_cuts` member appended.
fn with_cuts(cuts: &str) -> String {
    let body = VALID.trim_end().strip_suffix('}').unwrap();
    format!("{body},\n  \"source_cuts\": {cuts}\n}}")
}

#[test]
fn source_cuts_are_optional_and_absence_is_no_operation() {
    assert!(profile(VALID).unwrap().source_cuts.is_empty());
    assert!(profile(&with_cuts("[]")).unwrap().source_cuts.is_empty());
}

#[test]
fn declared_source_cuts_keep_their_fields_and_unit_normal() {
    let declared = profile(&with_cuts(CUTS)).unwrap();
    assert_eq!(declared.source_cuts.len(), 2);
    assert_eq!(declared.source_cuts[0].label, "waist");
    assert_eq!(declared.source_cuts[0].point, [0.0, 0.0, 0.6]);
    assert_eq!(declared.source_cuts[0].normal, [0.0, 0.0, 1.0]);
    assert_eq!(declared.source_cuts[0].retain, RetainSide::Negative);
    assert_eq!(declared.source_cuts[1].label, "left-cuff");
    assert_eq!(declared.source_cuts[1].normal, [0.0, 0.0, -1.0]);
    assert_eq!(declared.source_cuts[1].retain, RetainSide::Positive);
}

#[test]
fn a_cut_plane_follows_the_declared_frame() {
    // Display `(x, y, z)` is source `(x, -z, y)`, so this display cut is the same
    // source plane as the `waist` entry above.
    let display = with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.6, 0.0], "normal": [0.0, 4.0, 0.0], "retain": "negative"}]"#,
    )
    .replace("source-local-z-up", "sculpto-display-y-up");
    let cut = profile(&display).unwrap().source_cuts.remove(0);
    assert_eq!(cut.point, [0.0, 0.0, 0.6]);
    assert_eq!(cut.normal, [0.0, 0.0, 1.0]);
}

#[test]
fn a_malformed_source_cut_is_named_not_defaulted() {
    let missing = profile(&with_cuts(
        r#"[{"label": "waist", "normal": [0.0, 0.0, 1.0], "retain": "negative"}]"#,
    ));
    assert_eq!(
        missing,
        Err(ControlError::InvalidProfile {
            field: "source_cuts[0]".to_string(),
            reason: "missing `plane_point`".to_string(),
        })
    );
    let unknown = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 1.0], "retain": "negative", "side": "keep"}]"#,
    ));
    assert_eq!(
        unknown,
        Err(ControlError::InvalidProfile {
            field: "source_cuts[0]".to_string(),
            reason: "unknown `side`".to_string(),
        })
    );
    let short_point = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.6], "normal": [0.0, 0.0, 1.0], "retain": "negative"}]"#,
    ));
    let error = short_point.unwrap_err();
    assert!(
        error.to_string().contains("source_cuts[0].plane_point"),
        "{error}"
    );
    let not_numbers = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, "0.6"], "normal": [0.0, 0.0, 1.0], "retain": "negative"}]"#,
    ));
    let error = not_numbers.unwrap_err();
    assert!(
        error.to_string().contains("source_cuts[0].plane_point[2]"),
        "{error}"
    );
    let not_an_array = profile(&with_cuts("{}"));
    assert_eq!(
        not_an_array,
        Err(ControlError::InvalidProfile {
            field: "source_cuts".to_string(),
            reason: "expected an array of cut objects".to_string(),
        })
    );
}

#[test]
fn a_cut_without_a_usable_plane_normal_or_side_is_refused() {
    let zero = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 0.0], "retain": "negative"}]"#,
    ));
    let error = zero.unwrap_err();
    assert!(
        error.to_string().contains("source_cuts[0].normal"),
        "{error}"
    );
    assert!(
        error.to_string().contains("nonzero finite normal"),
        "{error}"
    );
    // A tiny but representable direction is a direction: 1e-45 is the smallest
    // positive f32 value, and the f64 norm keeps it non-zero.
    let subnormal = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [1e-45, 0.0, 0.0], "retain": "negative"}]"#,
    ))
    .unwrap();
    assert_eq!(subnormal.source_cuts[0].normal, [1.0, 0.0, 0.0]);
    let infinite = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [1e39, 0.0, 0.0], "retain": "negative"}]"#,
    ));
    let error = infinite.unwrap_err();
    assert!(
        error.to_string().contains("source_cuts[0].normal[0]"),
        "{error}"
    );
    let side = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 1.0], "retain": "both"}]"#,
    ));
    let error = side.unwrap_err();
    assert!(
        error.to_string().contains("source_cuts[0].retain"),
        "{error}"
    );
    assert!(
        error.to_string().contains("positive or negative"),
        "{error}"
    );
    let blank = profile(&with_cuts(
        r#"[{"label": "  ", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 1.0], "retain": "negative"}]"#,
    ));
    assert!(
        blank
            .unwrap_err()
            .to_string()
            .contains("expected a non-blank label")
    );
    let duplicate = profile(&with_cuts(
        r#"[{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": [0.0, 0.0, 1.0], "retain": "negative"},
            {"label": "waist", "plane_point": [0.0, 0.0, 0.4], "normal": [0.0, 0.0, -1.0], "retain": "positive"}]"#,
    ));
    assert!(
        duplicate
            .unwrap_err()
            .to_string()
            .contains("duplicate cut label `waist`")
    );
}

#[test]
fn equivalent_normals_normalize_across_the_f32_range() {
    // Three magnitudes, three axes: only the direction may survive.
    for (declared, expected) in [
        ("[1e-45, 0.0, 0.0]", [1.0, 0.0, 0.0]),
        ("[1.0, 0.0, 0.0]", [1.0, 0.0, 0.0]),
        ("[1e38, 0.0, 0.0]", [1.0, 0.0, 0.0]),
        ("[0.0, 1e-30, 0.0]", [0.0, 1.0, 0.0]),
        ("[0.0, 0.0, 3.0]", [0.0, 0.0, 1.0]),
        ("[0.0, 0.0, -1e30]", [0.0, 0.0, -1.0]),
        ("[1e-20, 1e-20, 0.0]", [0.70710677, 0.70710677, 0.0]),
    ] {
        let cut = with_cuts(&format!(
            r#"[{{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": {declared}, "retain": "negative"}}]"#
        ));
        let declared = profile(&cut).unwrap().source_cuts.remove(0);
        assert_eq!(
            declared.normal, expected,
            "magnitude must not change the normalized direction ({declared:?})"
        );
    }
    // Only a truly zero or non-finite direction is left to refuse.
    for declared in ["[0.0, 0.0, 0.0]", "[1e39, 0.0, 0.0]"] {
        let cut = with_cuts(&format!(
            r#"[{{"label": "waist", "plane_point": [0.0, 0.0, 0.6], "normal": {declared}, "retain": "negative"}}]"#
        ));
        let error = profile(&cut).unwrap_err().to_string();
        assert!(
            error.contains("nonzero finite normal") || error.contains("must be finite"),
            "{declared}: {error}"
        );
    }
}

#[test]
fn cut_directions_convert_like_points_without_translation() {
    let display = profile(&display_frame(VALID)).unwrap();
    assert_eq!(display.frame.to_mesh([1.0, 2.0, 3.0]), [1.0, -3.0, 2.0]);
    assert_eq!(
        display.frame.to_mesh_direction([1.0, 2.0, 3.0]),
        [1.0, -3.0, 2.0]
    );
    assert_eq!(
        ControlFrame::SourceLocalZUp.to_mesh_direction([0.0, 0.0, -1.0]),
        [0.0, 0.0, -1.0]
    );
}
