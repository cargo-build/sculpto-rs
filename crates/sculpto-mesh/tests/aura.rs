//! Aura behaviour: sections, clearance floor, bump bridging and movement modes.

use sculpto_mesh::aura::{Aura, AuraError, AuraMode, AuraParams, BandRules, BandSpec, Layout};

/// A closed prism: `segments` sides, one ring per height.
fn prism(radius: f32, heights: &[f32], segments: usize) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for (level, height) in heights.iter().enumerate() {
        for index in 0..segments {
            let angle = std::f32::consts::TAU * index as f32 / segments as f32;
            positions.push([radius * angle.cos(), radius * angle.sin(), *height]);
            if level > 0 {
                let current = (level * segments + index) as u32;
                let next = (level * segments + (index + 1) % segments) as u32;
                let below = current - segments as u32;
                let below_next = next - segments as u32;
                triangles.push([below, below_next, current]);
                triangles.push([below_next, next, current]);
            }
        }
    }
    (positions, triangles)
}

/// A prism whose first column sticks out: the operator's bump example.
fn bumped_prism(
    base: f32,
    bump: f32,
    heights: &[f32],
    segments: usize,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, triangles) = prism(base, heights, segments);
    for (index, position) in positions.iter_mut().enumerate() {
        if index % segments == 0 {
            let scale = bump / base;
            position[0] *= scale;
            position[1] *= scale;
        }
    }
    (positions, triangles)
}

fn radius_of(position: &[f32; 3]) -> f32 {
    position[0].hypot(position[1])
}

fn angle_of(position: &[f32; 3]) -> f32 {
    position[1]
        .atan2(position[0])
        .rem_euclid(std::f32::consts::TAU)
}

#[test]
fn press_lifts_a_garment_that_sits_inside_the_body() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0, 2.0, 3.0], 24);
    let params = AuraParams {
        clearance: 0.2,
        slack: 0.0,
        smooth: 1,
        mode: AuraMode::Press,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let garment = prism(0.5, &[0.4, 0.8, 1.2, 1.6, 2.0, 2.4], 24).0;
    let (fitted, report) = aura.apply(&garment);
    assert_eq!(report.inside_before, garment.len());
    assert_eq!(report.inside_after, 0);
    for position in &fitted {
        assert!(
            (radius_of(position) - 1.2).abs() < 0.05,
            "radius {} should sit on the aura",
            radius_of(position)
        );
    }
}

#[test]
fn tight_pulls_an_oversized_garment_onto_the_aura() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0, 2.0], 24);
    let params = AuraParams {
        clearance: 0.1,
        slack: 0.0,
        smooth: 1,
        mode: AuraMode::Tight,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let garment = prism(2.0, &[0.25, 0.75, 1.25, 1.75], 24).0;
    let (fitted, _) = aura.apply(&garment);
    for position in &fitted {
        assert!((radius_of(position) - 1.1).abs() < 0.05);
    }
}

#[test]
fn loose_keeps_free_volume_outside_the_aura() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0, 2.0], 24);
    let params = AuraParams {
        clearance: 0.1,
        slack: 0.0,
        smooth: 1,
        mode: AuraMode::Loose,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let garment = prism(3.0, &[0.25, 0.75, 1.25, 1.75], 24).0;
    let (fitted, _) = aura.apply(&garment);
    for position in &fitted {
        assert!((radius_of(position) - 3.0).abs() < 1e-4);
    }
}

#[test]
fn a_bump_raises_a_wide_arc_and_stays_cleared() {
    let (body, triangles) = bumped_prism(1.0, 2.0, &[0.0, 1.0, 2.0, 3.0], 48);
    let params = AuraParams {
        clearance: 0.1,
        slack: 0.0,
        smooth: 9,
        mode: AuraMode::Tight,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let garment = prism(1.0, &[0.5, 1.5, 2.5], 96).0;
    let (fitted, report) = aura.apply(&garment);
    assert_eq!(report.inside_after, 0);
    let at_bump = fitted
        .iter()
        .min_by(|left, right| angle_of(left).total_cmp(&angle_of(right)))
        .copied()
        .unwrap_or_default();
    let opposite = fitted
        .iter()
        .min_by(|left, right| {
            (angle_of(left) - std::f32::consts::PI)
                .abs()
                .total_cmp(&(angle_of(right) - std::f32::consts::PI).abs())
        })
        .copied()
        .unwrap_or_default();
    assert!(
        radius_of(&at_bump) > 2.0,
        "aura radius {} at the bump must clear the bump",
        radius_of(&at_bump)
    );
    assert!(
        radius_of(&opposite) < 1.4,
        "aura radius {} opposite the bump must stay near the base",
        radius_of(&opposite)
    );
    assert!(radius_of(&at_bump) > radius_of(&opposite) + 0.5);
}

#[test]
fn rejects_empty_and_undersized_inputs() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0], 8);
    assert_eq!(
        Aura::build(&[], &triangles, AuraParams::default()).unwrap_err(),
        AuraError::EmptyBody
    );
    assert_eq!(
        Aura::build(&body, &[], AuraParams::default()).unwrap_err(),
        AuraError::EmptyTriangles
    );
    let bad = [[0u32, 1, 99]];
    assert_eq!(
        Aura::build(&body, &bad, AuraParams::default()).unwrap_err(),
        AuraError::BadIndex {
            triangle: 0,
            vertex: 99
        }
    );
    assert_eq!(
        Aura::build(
            &body,
            &triangles,
            AuraParams {
                slices: 1,
                ..AuraParams::default()
            }
        )
        .unwrap_err(),
        AuraError::TooFewSlices
    );
    assert_eq!(
        AuraMode::parse("baggie").unwrap_err(),
        AuraError::UnknownMode("baggie".to_string())
    );
}

#[test]
fn displacement_report_is_ordered_and_bounded() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0, 2.0, 3.0], 24);
    let params = AuraParams {
        clearance: 0.2,
        slack: 0.0,
        smooth: 1,
        mode: AuraMode::Loose,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    // A tapered garment: inner rings move further than outer rings.
    let mut garment = prism(0.5, &[0.4, 1.0, 1.6, 2.2], 24).0;
    for position in garment.iter_mut() {
        position[0] *= 1.0 + position[2] * 0.2;
        position[1] *= 1.0 + position[2] * 0.2;
    }
    let (_, report) = aura.apply(&garment);
    assert!(report.moved > 0);
    assert!(report.moved_p50 <= report.moved_p95, "p50 <= p95");
    assert!(report.moved_p95 <= report.moved_max, "p95 <= max");
    assert!(report.moved_p50 <= report.moved_mean + 1e-6 || report.moved_mean <= report.moved_max);
    assert!(report.moved_mean <= report.moved_max, "mean <= max");
}

/// A torso over two legs, legs at x = ±1.2, all prisms centered per part.
fn legged_body() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = prism(1.5, &[2.0, 3.0, 4.0], 24);
    for dx in [-1.2f32, 1.2] {
        let start = positions.len() as u32;
        let (leg_positions, leg_triangles) = prism(0.5, &[0.0, 1.0, 2.0], 24);
        positions.extend(leg_positions.iter().map(|p| [p[0] + dx, p[1], p[2]]));
        triangles.extend(
            leg_triangles
                .into_iter()
                .map(|[a, b, c]| [a + start, b + start, c + start]),
        );
    }
    (positions, triangles)
}

#[test]
fn merged_layout_bridges_the_gap_between_legs() {
    let (body, triangles) = legged_body();
    let bands = BandRules {
        split: 2.0,
        above: BandSpec {
            layout: Layout::PerContour,
            mode: AuraMode::Press,
        },
        below: BandSpec {
            layout: Layout::Merged,
            mode: AuraMode::Press,
        },
    };
    let mut params = AuraParams {
        clearance: 0.2,
        slack: 0.0,
        smooth: 1,
        slices: 24,
        bins: 48,
        mode: AuraMode::Press,
        ..AuraParams::default()
    };
    // A skirt vertex hanging in the gap between the legs: per-contour fitting
    // leaves it there (it is outside both leg auras), where swinging legs
    // would pass through it; the merged envelope must clear both legs.
    let gap_vertex = [0.2, 0.0, 1.0];

    params.bands = Some(bands);
    let merged = Aura::build(&body, &triangles, params).unwrap();
    let (merged_fitted, _) = merged.apply(std::slice::from_ref(&gap_vertex));
    let merged_radius = radius_of(&merged_fitted[0]);

    params.bands = None;
    let per_contour = Aura::build(&body, &triangles, params).unwrap();
    let (contour_fitted, _) = per_contour.apply(std::slice::from_ref(&gap_vertex));
    let contour_radius = radius_of(&contour_fitted[0]);

    assert_eq!(
        contour_radius, 0.2,
        "per-contour leaves the gap vertex alone"
    );
    assert!(
        merged_radius > 1.6,
        "merged envelope clears both legs (outer edge 1.7), got {merged_radius:.3}"
    );
}

#[test]
fn band_modes_fit_the_bodice_and_free_the_hem() {
    let (body, triangles) = prism(1.0, &[0.0, 1.0, 2.0, 3.0], 24);
    let params = AuraParams {
        clearance: 0.2,
        slack: 0.0,
        smooth: 1,
        slices: 24,
        bins: 48,
        bands: Some(BandRules {
            split: 1.5,
            above: BandSpec {
                layout: Layout::PerContour,
                mode: AuraMode::Press,
            },
            below: BandSpec {
                layout: Layout::PerContour,
                mode: AuraMode::Loose,
            },
        }),
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let bodice = [0.5, 0.0, 2.0];
    let hem = [3.0, 0.0, 0.5];
    let garment = vec![bodice, hem];
    let (fitted, report) = aura.apply(&garment);
    assert!(
        radius_of(&fitted[0]) > 0.9,
        "bodice is pressed onto the aura"
    );
    assert_eq!(fitted[1], hem, "a loose hem keeps its flare");
    assert_eq!(report.moved, 1);
}

#[test]
fn proxies_keep_one_contour_per_body_part() {
    let (body, triangles) = legged_body();
    let params = AuraParams {
        slices: 12,
        bins: 24,
        ..AuraParams::default()
    };
    let aura = Aura::build(&body, &triangles, params).unwrap();
    let proxies = aura.proxies();
    assert_eq!(proxies.len(), params.slices);
    let leg_sections = proxies
        .iter()
        .filter(|section| section.height < 2.0)
        .collect::<Vec<_>>();
    assert!(leg_sections.len() >= 6, "most of the body height is legs");
    assert!(
        leg_sections
            .iter()
            .all(|section| section.contours.len() == 2),
        "every leg section keeps two separate contours"
    );
    let torso = proxies
        .iter()
        .find(|section| section.height > 2.2)
        .expect("torso sections exist");
    assert_eq!(torso.contours.len(), 1);
    // Proxy points sit on the raw body surface, not on the eased aura;
    // contour order is arbitrary, so accept either leg.
    let leg = &leg_sections[0].contours[0];
    let on_surface = leg
        .iter()
        .all(|point| ((point[0].abs() - 1.2).hypot(point[1]) - 0.5).abs() < 0.1);
    assert!(on_surface, "leg proxy follows the raw leg contour");
}
