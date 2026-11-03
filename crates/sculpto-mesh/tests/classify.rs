//! Classification behaviour: the decision tree, adherence, composites, and
//! the corset-aura profiles each kind resolves to.

use sculpto_mesh::aura::{AuraMode, Layout};
use sculpto_mesh::classify::{
    Adherence, ClassifyError, GarmentKind, PartFit, classify, scale_to_part,
};

const SEGMENTS: usize = 16;

/// A prism tube offset in the section plane.
fn tube(cx: f32, cy: f32, radius: f32, heights: &[f32]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = prism(radius, heights, SEGMENTS);
    for position in &mut positions {
        position[0] += cx;
        position[1] += cy;
    }
    triangles.clear();
    let levels = heights.len();
    for level in 1..levels {
        for index in 0..SEGMENTS {
            let current = (
                (level * SEGMENTS + index) as u32,
                (level * SEGMENTS + (index + 1) % SEGMENTS) as u32,
            );
            let below = (current.0 - SEGMENTS as u32, current.1 - SEGMENTS as u32);
            triangles.push([below.0, below.1, current.0]);
            triangles.push([below.1, current.1, current.0]);
        }
    }
    (positions, triangles)
}

fn prism(radius: f32, heights: &[f32], segments: usize) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    for height in heights {
        for index in 0..segments {
            let angle = std::f32::consts::TAU * index as f32 / segments as f32;
            positions.push([radius * angle.cos(), radius * angle.sin(), *height]);
        }
    }
    (positions, Vec::new())
}

/// An open arc tube covering `arc_deg` degrees: the cape shell.
fn arc_tube(radius: f32, arc_deg: f32, heights: &[f32]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let span = (arc_deg / 360.0 * SEGMENTS as f32).round() as usize;
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for (level, height) in heights.iter().enumerate() {
        for index in 0..span {
            let angle =
                -90.0f32.to_radians() + std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
            positions.push([radius * angle.cos(), radius * angle.sin(), *height]);
            if level > 0 {
                let current = (level * span + index) as u32;
                let next = (level * span + (index + 1) % span) as u32;
                let below = current - span as u32;
                let below_next = next - span as u32;
                triangles.push([below, below_next, current]);
                triangles.push([below_next, next, current]);
            }
        }
    }
    (positions, triangles)
}

/// Merge meshes, reindexing the second into the first.
fn merge(
    (mut left_positions, mut left_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
    (right_positions, right_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = left_positions.len() as u32;
    left_positions.extend(right_positions);
    left_triangles.extend(
        right_triangles
            .into_iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );
    (left_positions, left_triangles)
}

/// Torso (z 35..85, r 10), head (z 86..100, r 6) and two legs (z 0..45, r 4
/// at x = ±6): body height 100 on z.
fn mannequin() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let torso = tube(0.0, 0.0, 10.0, &[45.0, 65.0, 85.0]);
    let head = tube(0.0, 0.0, 6.0, &[86.0, 100.0]);
    let legs = merge(
        tube(-6.0, 0.0, 4.0, &[0.0, 22.0, 45.0]),
        tube(6.0, 0.0, 4.0, &[0.0, 22.0, 45.0]),
    );
    merge(merge(torso, head), legs)
}

fn heights(z0: f32, z1: f32, rings: usize) -> Vec<f32> {
    (0..rings)
        .map(|index| z0 + (z1 - z0) * index as f32 / (rings - 1) as f32)
        .collect()
}

fn kind_of(garment: (Vec<[f32; 3]>, Vec<[u32; 3]>)) -> sculpto_mesh::classify::Classified {
    let (body, body_triangles) = mannequin();
    classify(&body, &body_triangles, &garment.0, &garment.1).expect("classification runs")
}

#[test]
fn tunic_is_fitted_upper_body() {
    let verdict = kind_of(tube(0.0, 0.0, 13.0, &heights(42.0, 80.0, 4)));
    assert_eq!(verdict.kind, GarmentKind::UpperBody);
    assert_eq!(verdict.adherence, Adherence::Fitted);
}

#[test]
fn corset_tube_is_snug_upper_body() {
    let verdict = kind_of(tube(0.0, 0.0, 11.0, &heights(42.0, 70.0, 4)));
    assert_eq!(verdict.kind, GarmentKind::UpperBody);
    assert_eq!(verdict.adherence, Adherence::Snug);
}

#[test]
fn two_leg_columns_are_trousers() {
    let legs = merge(
        tube(-6.0, 0.0, 5.0, &heights(5.0, 50.0, 4)),
        tube(6.0, 0.0, 5.0, &heights(5.0, 50.0, 4)),
    );
    let verdict = kind_of(legs);
    assert_eq!(verdict.kind, GarmentKind::Trousers);
}

#[test]
fn one_column_around_two_legs_to_the_waist_is_a_skirt() {
    let verdict = kind_of(tube(0.0, 0.0, 16.0, &heights(20.0, 62.0, 5)));
    assert_eq!(
        verdict.kind,
        GarmentKind::Skirt,
        "evidence: {:?} features: {:?}",
        verdict.evidence,
        verdict.features
    );
    assert!(
        verdict.features.merged,
        "skirt must read as a merged volume"
    );
}

#[test]
fn merged_volume_with_a_fitted_top_is_a_dress() {
    let verdict = kind_of(tube(0.0, 0.0, 13.0, &heights(15.0, 80.0, 6)));
    assert_eq!(verdict.kind, GarmentKind::Dress);
    assert_eq!(verdict.adherence, Adherence::Fitted);
}

#[test]
fn merged_volume_with_a_full_top_is_a_robe() {
    let verdict = kind_of(tube(0.0, 0.0, 20.0, &heights(5.0, 85.0, 6)));
    assert_eq!(verdict.kind, GarmentKind::Robe);
    assert_eq!(verdict.adherence, Adherence::Relaxed);
}

#[test]
fn grounded_two_columns_are_footwear() {
    let boots = merge(
        tube(-6.0, 0.0, 5.5, &heights(0.0, 15.0, 3)),
        tube(6.0, 0.0, 5.5, &heights(0.0, 15.0, 3)),
    );
    let verdict = kind_of(boots);
    assert_eq!(verdict.kind, GarmentKind::Footwear);
}

#[test]
fn open_shell_over_the_torso_is_a_cape() {
    let verdict = kind_of(arc_tube(16.0, 160.0, &heights(30.0, 80.0, 4)));
    assert_eq!(verdict.kind, GarmentKind::Cape);
    assert!(verdict.features.angular < 0.80);
}

#[test]
fn narrow_waist_band_is_an_accessory() {
    let verdict = kind_of(tube(0.0, 0.0, 10.5, &heights(50.0, 56.0, 3)));
    assert_eq!(verdict.kind, GarmentKind::Accessory);
}

/// Bridge the top rings of two tubes with a sheet: the crotch of trousers
/// and jumpsuits, which makes the garment one connected component.
fn bridged(
    (mut left_positions, mut left_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
    (right_positions, right_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = left_positions.len() as u32;
    let right_top_ring = offset + (right_positions.len() - SEGMENTS) as u32;
    left_positions.extend(right_positions);
    left_triangles.extend(
        right_triangles
            .into_iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );
    for index in 0..SEGMENTS {
        let next = (index + 1) % SEGMENTS;
        let left_top = offset - SEGMENTS as u32 + index as u32;
        let left_next = offset - SEGMENTS as u32 + next as u32;
        let right_top = right_top_ring + index as u32;
        let right_next = right_top_ring + next as u32;
        left_triangles.push([left_top, left_next, right_top]);
        left_triangles.push([left_next, right_next, right_top]);
    }
    (left_positions, left_triangles)
}

#[test]
fn jumpsuit_shape_is_unknown_with_alternatives() {
    let legs = bridged(
        tube(-6.0, 0.0, 5.0, &heights(5.0, 60.0, 4)),
        tube(6.0, 0.0, 5.0, &heights(5.0, 60.0, 4)),
    );
    let verdict = kind_of(legs);
    assert_eq!(verdict.kind, GarmentKind::Unknown);
    assert!(verdict.profile().is_none());
    assert!(
        verdict.evidence.iter().any(|line| line.contains("nearest")),
        "unknown must name its alternatives: {:?}",
        verdict.evidence
    );
}

#[test]
fn disconnected_major_parts_form_a_composite() {
    let outfit = merge(
        tube(0.0, 0.0, 13.0, &heights(42.0, 80.0, 4)),
        tube(0.0, 0.0, 16.0, &heights(20.0, 62.0, 5)),
    );
    let verdict = kind_of(outfit);
    assert_eq!(verdict.kind, GarmentKind::Composite);
    assert_eq!(verdict.features.components, 2);
    let kinds: Vec<GarmentKind> = verdict.parts.iter().map(|part| part.kind).collect();
    assert!(kinds.contains(&GarmentKind::UpperBody));
    assert!(kinds.contains(&GarmentKind::Skirt));
    assert!(verdict.profile().is_none(), "composite fits per part");
}

#[test]
fn every_kind_names_and_parses_back() {
    for name in [
        "upper-body",
        "trousers",
        "skirt",
        "dress",
        "robe",
        "footwear",
        "cape",
        "accessory",
    ] {
        assert_eq!(GarmentKind::parse(name).expect("parses").name(), name);
    }
    assert!(GarmentKind::parse("hat").is_err());
}

#[test]
fn dress_profile_splits_at_the_hip_and_merges_below() {
    let verdict = kind_of(tube(0.0, 0.0, 13.0, &heights(15.0, 80.0, 6)));
    let profile = verdict.profile().expect("dress has a profile");
    let bands = profile.params.bands.expect("dress is band-split");
    assert!((bands.split - 48.0).abs() < 0.5, "split at 0.48 * 100");
    assert_eq!(bands.above.layout, Layout::PerContour);
    assert_eq!(bands.above.mode, AuraMode::Press);
    assert_eq!(bands.below.layout, Layout::Merged);
    assert_eq!(bands.below.mode, AuraMode::Loose);
    assert_eq!(profile.anchor.name(), "waist");
    assert_eq!(profile.fit, PartFit::Radial);
}

#[test]
fn skirt_profile_presses_the_waistband_only() {
    let verdict = kind_of(tube(0.0, 0.0, 16.0, &heights(20.0, 62.0, 5)));
    let profile = verdict.profile().expect("skirt has a profile");
    let bands = profile.params.bands.expect("skirt is band-split");
    assert!((bands.split - 59.0).abs() < 0.5, "waistband at top - 0.03");
    assert_eq!(bands.above.mode, AuraMode::Press);
    assert_eq!(bands.below.mode, AuraMode::Loose);
    assert_eq!(bands.below.layout, Layout::Merged);
}

#[test]
fn robe_profile_keeps_volume_and_footwear_scales() {
    let robe = kind_of(tube(0.0, 0.0, 20.0, &heights(5.0, 85.0, 6)));
    let robe_profile = robe.profile().expect("robe has a profile");
    assert!(robe_profile.params.clearance >= 1.0);
    assert!(robe_profile.params.slack >= 0.10);
    assert_eq!(
        robe_profile.params.bands.expect("robe splits").below.mode,
        AuraMode::Loose
    );

    let boots = merge(
        tube(-6.0, 0.0, 5.5, &heights(0.0, 15.0, 3)),
        tube(6.0, 0.0, 5.5, &heights(0.0, 15.0, 3)),
    );
    let footwear_profile = kind_of(boots).profile().expect("footwear has a profile");
    assert_eq!(footwear_profile.fit, PartFit::ScaleToPart);
    assert!(footwear_profile.params.bands.is_none());
}

#[test]
fn snug_adherence_hugs_and_relaxed_stays_loose() {
    let snug = kind_of(tube(0.0, 0.0, 11.0, &heights(42.0, 70.0, 4)));
    let snug_params = snug.profile().expect("profile").params;
    assert_eq!(snug_params.mode, AuraMode::Press);
    assert!((snug_params.clearance - 0.3).abs() < 1e-6);
    assert!((snug_params.max_ease - 1.15).abs() < 1e-6);

    let relaxed = kind_of(tube(0.0, 0.0, 20.0, &heights(5.0, 85.0, 6)));
    let relaxed_params = relaxed.profile().expect("profile").params;
    assert_eq!(relaxed_params.mode, AuraMode::Loose);
    assert!((relaxed_params.clearance - 1.0).abs() < 1e-6);
}

#[test]
fn scale_to_part_shrinks_oversized_boots_onto_the_feet() {
    let (body, body_triangles) = mannequin();
    let (boots, boot_triangles) = merge(
        tube(-6.0, 0.0, 8.0, &heights(0.0, 15.0, 3)),
        tube(6.0, 0.0, 8.0, &heights(0.0, 15.0, 3)),
    );
    let classified = classify(&body, &body_triangles, &boots, &boot_triangles).expect("runs");
    let (scaled, report) =
        scale_to_part(&body, &body_triangles, &boots, &boot_triangles, &classified);
    assert!(report.applied, "body has contours in the band");
    assert!(
        report.scale < 0.8 && report.scale >= 0.5,
        "scale {}",
        report.scale
    );
    let x_span = scaled
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p[0]), hi.max(p[0]))
        });
    assert!(
        (x_span.1 - x_span.0) < 20.0,
        "plane span {} approaches the body's 20",
        x_span.1 - x_span.0
    );
    assert!(
        scaled.iter().all(|p| p[2] >= 0.0 && p[2] <= 15.0),
        "axis untouched"
    );
}

#[test]
fn rejects_empty_and_broken_inputs() {
    let (body, body_triangles) = mannequin();
    let (garment, triangles) = tube(0.0, 0.0, 13.0, &heights(42.0, 80.0, 4));
    assert_eq!(
        classify(&body, &body_triangles, &[], &triangles).unwrap_err(),
        ClassifyError::EmptyGarment
    );
    assert_eq!(
        classify(&body, &body_triangles, &garment, &[]).unwrap_err(),
        ClassifyError::NoGarmentTriangles
    );
    assert!(matches!(
        classify(&body, &body_triangles, &garment[..8], &triangles).unwrap_err(),
        ClassifyError::BadIndex { .. }
    ));
}
