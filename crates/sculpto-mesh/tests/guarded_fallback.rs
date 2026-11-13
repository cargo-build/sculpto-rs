//! Guarded no-clipping fallback, returned-geometry deformation and separate
//! correspondence/collision surfaces. @req DECL0013
//!
//! The synthetic positive fallback case (registered candidate rejected, an
//! admissible no-clipping fit candidate improves) is not yet pinned here; the
//! pinned Mike top diagnostic below is the real positive case. See the
//! assignment OUTPUT for the bounded fixture search and its negative results.

use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::outfit::ClothRegion;
use sculpto_mesh::outfit::rigging::{fit_rigged_regions, fit_rigged_regions_with_collision};
use sculpto_mesh::wrap::repair::{fit_and_repair, fit_and_repair_with_collision, repair_after_fit};
use sculpto_mesh::wrap::{FitOptions, fit_to_body};

/// Closed axis-aligned box with outward winding.
fn box_body(min: [f32; 3], max: [f32; 3]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    let positions = vec![
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ];
    let faces = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    (positions, faces)
}

/// A square in the xy plane at z = 0, wound with normals in +z.
fn plane(half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-half, -half, 0.0],
            [half, -half, 0.0],
            [half, half, 0.0],
            [-half, half, 0.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

fn joined(
    mut first: (Vec<[f32; 3]>, Vec<[u32; 3]>),
    second: (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = first.0.len() as u32;
    first.0.extend(second.0);
    first
        .1
        .extend(second.1.iter().map(|face| face.map(|v| v + offset)));
    first
}

fn max_move(before: &[[f32; 3]], after: &[[f32; 3]]) -> f32 {
    before
        .iter()
        .zip(after)
        .map(|(a, b)| {
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0f32, f32::max)
}

/// Independent symmetric unique-edge strain, mirroring the engine definition.
fn max_strain(before: &[[f32; 3]], after: &[[f32; 3]], faces: &[[u32; 3]]) -> f32 {
    let mut edges = std::collections::BTreeSet::new();
    for face in faces {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            if a != b {
                edges.insert((a.min(b), a.max(b)));
            }
        }
    }
    edges
        .into_iter()
        .filter_map(|(a, b)| {
            let (a, b) = (a as usize, b as usize);
            let len = |p: &[[f32; 3]]| {
                let d = [p[a][0] - p[b][0], p[a][1] - p[b][1], p[a][2] - p[b][2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            };
            let old = len(before);
            let new = len(after);
            (old >= 1e-5 && new >= 1e-5).then(|| (new / old).max(old / new) - 1.0)
        })
        .fold(0.0f32, f32::max)
}

fn assert_deformation(
    report_move: f32,
    report_strain: f32,
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    faces: &[[u32; 3]],
) {
    let expected_move = max_move(before, after);
    let expected_strain = max_strain(before, after, faces);
    assert!(
        (report_move - expected_move).abs() <= 1e-6,
        "max_displacement {report_move} != independent {expected_move}"
    );
    assert!(
        (report_strain - expected_strain).abs() <= 1e-6,
        "max_strain {report_strain} != independent {expected_strain}"
    );
}

fn new_witnesses(source: &[[f32; 3]], faces: &[[u32; 3]], candidate: &[[f32; 3]]) -> usize {
    let original: std::collections::HashSet<_> = self_intersection_witnesses(source, faces)
        .into_iter()
        .collect();
    self_intersection_witnesses(candidate, faces)
        .iter()
        .filter(|w| !original.contains(w))
        .count()
}

// ---------------------------------------------------------------------------
// Guarded fallback behavior on fixed synthetic fixtures.
// ---------------------------------------------------------------------------

/// Two free layers over a box: both the registered and the no-clipping fit
/// pull the tilted upper layer across the lower one. The registered candidate
/// is rejected by the self-witness guard, the one fallback attempt is made,
/// its candidate is rejected by the same guard, and the baseline is retained.
#[test]
fn fallback_rejects_no_clipping_candidate_that_creates_a_self_crossing() {
    let (body, body_faces) = box_body([-5.0, -5.0, -5.0], [5.0, 5.0, 0.0]);
    let garment = [
        [-1.0, -1.0, -0.2],
        [1.0, -1.0, -0.2],
        [0.0, 1.0, 0.1],
        [-0.8, -0.8, 0.05],
        [0.8, -0.8, 0.35],
        [0.0, 0.8, 0.35],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    assert!(self_intersection_witnesses(&garment, &faces).is_empty());
    let source_pairs = intersection_pairs(&garment, &faces, &body, &body_faces);
    assert!(source_pairs > 0);
    // Both raw fit candidates create a new crossing, so the registered
    // candidate fails the self-witness guard and the fallback is exercised.
    let no_clipping = FitOptions {
        clipping_passes: 0,
        ..FitOptions::default()
    };
    let (raw_registered, registered_report) = fit_to_body(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("raw registered fit");
    assert_eq!(registered_report.flipped, 0);
    assert!(new_witnesses(&garment, &faces, &raw_registered) > 0);
    let (raw_alternative, alternative_report) =
        fit_to_body((&garment, &faces), (&body, &body_faces), None, &no_clipping)
            .expect("raw no-clipping fit");
    assert_eq!(alternative_report.flipped, 0);
    assert!(new_witnesses(&garment, &faces, &raw_alternative) > 0);
    // The pipeline retains the baseline and reports no fallback acceptance.
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("guarded pipeline");
    assert_eq!(output, garment, "neither candidate qualifies");
    assert!(!report.no_clipping_fallback_applied);
    assert_eq!(report.intersection_pairs, source_pairs);
    assert_eq!(new_witnesses(&garment, &faces, &output), 0);
    assert_eq!(report.max_displacement, 0.0);
    assert_eq!(report.max_strain, 0.0);
}

/// `clipping_passes = 0` is the existing no-clipping mode itself: it must not
/// trigger a second fit attempt, with or without a rejecting fixture.
#[test]
fn zero_clipping_passes_never_triggers_the_fallback() {
    let (body, body_faces) = box_body([-5.0, -5.0, -5.0], [5.0, 5.0, 0.0]);
    let garment = [
        [-1.0, -1.0, -0.2],
        [1.0, -1.0, -0.2],
        [0.0, 1.0, 0.1],
        [-0.8, -0.8, 0.05],
        [0.8, -0.8, 0.35],
        [0.0, 0.8, 0.35],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    let zero_budget = FitOptions {
        clipping_passes: 0,
        ..FitOptions::default()
    };
    let (output, report) =
        fit_and_repair((&garment, &faces), (&body, &body_faces), None, &zero_budget)
            .expect("zero-budget pipeline");
    assert_eq!(output, garment);
    assert!(!report.no_clipping_fallback_applied);
    // A deep free vertex the registered clipping pushes clear: with zero
    // budget the fit cannot clear it, and still no fallback may run.
    let mut deep = vec![[0.0, 0.0, -1.0]];
    for corner in 0..6 {
        let angle = corner as f32 * std::f32::consts::TAU / 6.0;
        deep.push([0.1 * angle.cos(), 0.1 * angle.sin(), 0.05]);
    }
    let fan: Vec<[u32; 3]> = (0..6)
        .map(|corner| [0, 1 + corner as u32, 1 + ((corner + 1) % 6) as u32])
        .collect();
    let (zero, zero_report) =
        fit_and_repair((&deep, &fan), (&body, &body_faces), None, &zero_budget)
            .expect("zero-budget fan");
    assert!(!zero_report.no_clipping_fallback_applied);
    assert!(
        zero_report.intersection_pairs > 0,
        "zero budget cannot clear"
    );
    assert_eq!(zero, deep);
}

/// A successful registered fit is kept: the fallback neither runs nor replaces
/// the cleared result with the worse no-clipping candidate.
#[test]
fn successful_registered_fit_is_not_replaced_by_the_fallback() {
    let (body, body_faces) = box_body([-5.0, -5.0, -5.0], [5.0, 5.0, 0.0]);
    let mut garment = vec![[0.0, 0.0, -1.0]];
    for corner in 0..6 {
        let angle = corner as f32 * std::f32::consts::TAU / 6.0;
        garment.push([0.1 * angle.cos(), 0.1 * angle.sin(), 0.05]);
    }
    let faces: Vec<[u32; 3]> = (0..6)
        .map(|corner| [0, 1 + corner as u32, 1 + ((corner + 1) % 6) as u32])
        .collect();
    assert!(intersection_pairs(&garment, &faces, &body, &body_faces) > 0);
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("registered pipeline");
    assert!(!report.no_clipping_fallback_applied);
    assert_eq!(report.intersection_pairs, 0, "registered clipping clears");
    assert_eq!(report.inside_after, 0);
    assert_ne!(output, garment);
    assert_eq!(new_witnesses(&garment, &faces, &output), 0);
    assert_deformation(
        report.max_displacement,
        report.max_strain,
        &garment,
        &output,
        &faces,
    );
}

// ---------------------------------------------------------------------------
// Deformation reporting on unchanged and moved geometry.
// ---------------------------------------------------------------------------

#[test]
fn unchanged_geometry_reports_zero_deformation() {
    let (body, body_faces) = plane(10.0);
    let garment = [[1.08, 0.0, 0.5], [2.0, 0.0, 0.5], [1.5, 1.0, 0.5]];
    let faces = [[0, 1, 2]];
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("clear garment");
    assert_eq!(output, garment);
    assert_eq!(report.max_displacement, 0.0);
    assert_eq!(report.max_strain, 0.0);
    assert!(!report.no_clipping_fallback_applied);
    let (repaired, repair_report) = repair_after_fit(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("clear repair");
    assert_eq!(repaired, garment);
    assert_eq!(repair_report.max_displacement, 0.0);
    assert_eq!(repair_report.max_strain, 0.0);
}

#[test]
fn moved_geometry_reports_measured_deformation() {
    let (body, body_faces) = plane(10.0);
    let garment = [[0.0, 0.0, -0.5], [1.0, 0.0, 0.4], [0.0, 1.0, 0.4]];
    let faces = [[0, 1, 2]];
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("fit pushes the inside vertex out");
    assert_ne!(output, garment);
    assert!(report.max_displacement > 0.0);
    assert_deformation(
        report.max_displacement,
        report.max_strain,
        &garment,
        &output,
        &faces,
    );
    let (repaired, repair_report) = repair_after_fit(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("repair moves the inside vertex");
    assert_deformation(
        repair_report.max_displacement,
        repair_report.max_strain,
        &garment,
        &repaired,
        &faces,
    );
}

// ---------------------------------------------------------------------------
// Separate correspondence and collision surfaces.
// ---------------------------------------------------------------------------

/// The extra obstacle part affects collision measurement and repair, but the
/// pull correspondences stay on the primary body. The garment crosses the
/// extra box with one vertex locked inside it, far beyond `max_distance` from
/// the small primary plane: repair cannot clear the locked residual, so the
/// fit branch runs and its correspondence surface is observable.
#[test]
fn extra_collision_part_never_becomes_the_pull_surface() {
    let primary = plane(1.0);
    let extra = box_body([4.0, -1.0, 0.0], [6.0, 1.0, 2.0]);
    let collision = joined(primary.clone(), extra.clone());
    let garment = [[4.5, -0.5, 1.0], [5.5, -0.5, 1.9], [5.0, 0.5, 2.3]];
    let faces = [[0, 1, 2]];
    let locks = VertexMask::from_values(vec![1.0, 0.0, 0.0]).expect("locked inside vertex");
    let options = FitOptions {
        max_distance: 2.0,
        clipping_passes: 1,
        ..FitOptions::default()
    };
    // The garment intersects only the extra part.
    assert_eq!(
        intersection_pairs(&garment, &faces, &primary.0, &primary.1),
        0
    );
    let source_pairs = intersection_pairs(&garment, &faces, &collision.0, &collision.1);
    assert!(source_pairs > 0, "fixture must intersect the extra part");
    // Primary-only correspondence cannot reach the garment.
    let (pulled, pull_report) = fit_to_body(
        (&garment, &faces),
        (&primary.0, &primary.1),
        Some(&locks),
        &options,
    )
    .expect("primary fit");
    assert_eq!(pulled, garment, "the extra part must not pull");
    assert_eq!(pull_report.pulled, 0);
    // The two-surface pipeline repairs against the full obstacle.
    let (output, report) = fit_and_repair_with_collision(
        (&garment, &faces),
        (&primary.0, &primary.1),
        (&collision.0, &collision.1),
        Some(&locks),
        &options,
    )
    .expect("two-surface fit");
    assert_eq!(
        report.intersection_pairs,
        intersection_pairs(&output, &faces, &collision.0, &collision.1),
        "contacts are measured on the full obstacle"
    );
    assert!(
        report.intersection_pairs <= source_pairs,
        "contacts must not worsen: {source_pairs} -> {}",
        report.intersection_pairs
    );
    assert_ne!(
        output, garment,
        "the repair must act on the extra part's contacts"
    );
    assert!(report.inside_after >= 1, "the locked vertex stays inside");
    assert_eq!(output[0], garment[0], "the locked vertex stays bit-exact");
    assert!(!report.no_clipping_fallback_applied);
    assert_deformation(
        report.max_displacement,
        report.max_strain,
        &garment,
        &output,
        &faces,
    );
    // With one repair stage and no reachable pull, the pipeline is exactly the
    // guarded collision repair of the source: the extra part never pulled.
    let (repaired, _) = repair_after_fit(
        (&garment, &faces),
        (&collision.0, &collision.1),
        Some(&locks),
        &options,
    )
    .expect("collision repair");
    assert_eq!(output, repaired);
    // The full obstacle would pull the free vertices if it were used as the
    // correspondence surface; the primary surface cannot reach them. The
    // pipeline's fit stage uses the primary, so the extra part never becomes
    // a silent pull target.
    let (would_pull, would_pull_report) = fit_to_body(
        (&garment, &faces),
        (&collision.0, &collision.1),
        Some(&locks),
        &options,
    )
    .expect("full-surface fit");
    assert_ne!(
        would_pull, garment,
        "the full obstacle is a reachable pull surface"
    );
    assert!(would_pull_report.pulled > 0);
}

#[test]
fn cloth_aware_two_surface_fit_keeps_cloth_fixed_and_reports_residuals() {
    let primary = plane(1.0);
    let extra = box_body([4.0, -1.0, 0.0], [6.0, 1.0, 2.0]);
    let collision = joined(primary.clone(), extra.clone());
    // Ordinary and cloth triangles both cross the extra box top, separated in
    // x so they share no positions and do not intersect each other.
    let garment = [
        [4.2, -0.5, 1.9],
        [5.0, -0.5, 1.9],
        [4.6, 0.5, 2.3],
        [5.2, -0.5, 1.9],
        [5.9, -0.5, 1.9],
        [5.55, 0.5, 2.3],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    let regions = [ClothRegion {
        name: "panel".into(),
        triangles: Some(vec![1]),
    }];
    let options = FitOptions {
        max_distance: 2.0,
        clipping_passes: 1,
        ..FitOptions::default()
    };
    let (output, report) = fit_rigged_regions_with_collision(
        (&garment, &faces),
        (&primary.0, &primary.1),
        (&collision.0, &collision.1),
        &regions,
        None,
        &options,
    )
    .expect("cloth-aware two-surface fit");
    assert_eq!(
        &output[3..],
        &garment[3..],
        "selected cloth stays bit-exact"
    );
    assert_eq!(report.ordinary_triangles, 1);
    assert_eq!(report.cloth_triangles, 1);
    assert_eq!(
        report.intersection_pairs,
        intersection_pairs(&output, &faces, &collision.0, &collision.1)
    );
    assert_eq!(
        report.cloth_intersection_pairs,
        intersection_pairs(&output, &faces[1..], &collision.0, &collision.1)
    );
    assert!(
        report.cloth_intersection_pairs > 0,
        "the fixed cloth panel keeps its authored contacts"
    );
    assert_deformation(
        report.max_displacement,
        report.max_strain,
        &garment,
        &output,
        &faces,
    );
    let ordinary = report.ordinary.expect("ordinary measurement");
    assert_deformation(
        ordinary.max_displacement,
        ordinary.max_strain,
        &garment[..3],
        &output[..3],
        &faces[..1],
    );
    // The one-surface API keeps its behavior for the same primary body.
    let (single, single_report) = fit_rigged_regions(
        (&garment, &faces),
        (&primary.0, &primary.1),
        &regions,
        None,
        &options,
    )
    .expect("single-surface region fit");
    assert_eq!(single, garment, "no contacts on the primary plane");
    assert_eq!(single_report.intersection_pairs, 0);
    assert_eq!(single_report.max_displacement, 0.0);
    assert_eq!(single_report.max_strain, 0.0);
}

/// After the outer mixed-region self-contact rollback the returned geometry is
/// the incoming garment, and every deformation field must say so.
#[test]
fn mixed_region_rollback_reports_zero_deformation() {
    let (body, body_faces) = box_body([-5.0, -5.0, -5.0], [5.0, 5.0, 0.0]);
    let garment = [
        [-1.0, -1.0, -0.1],
        [1.0, -1.0, -0.1],
        [0.0, 1.0, -0.1],
        [-0.5, -0.5, 0.12],
        [0.8, -0.4, 0.24],
        [0.2, 0.8, 0.24],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    let regions = [ClothRegion {
        name: "upper layer".into(),
        triangles: Some(vec![1]),
    }];
    let (output, report) = fit_rigged_regions(
        (&garment, &faces),
        (&body, &body_faces),
        &regions,
        None,
        &FitOptions::default(),
    )
    .expect("layered mixed garment");
    assert_eq!(
        output, garment,
        "the self-contact rollback restores the source"
    );
    assert_eq!(report.max_displacement, 0.0);
    assert_eq!(report.max_strain, 0.0);
    let ordinary = report.ordinary.expect("ordinary measurement");
    assert_eq!(ordinary.inside_after, 3);
    assert_eq!(ordinary.max_displacement, 0.0);
    assert_eq!(ordinary.max_strain, 0.0);
    assert!(!ordinary.no_clipping_fallback_applied);
}

// ---------------------------------------------------------------------------
// Opt-in pinned human Mike diagnostic. Reports residuals; never declares the
// garments accepted. Configure SCULPTO_FALLBACK_FIXTURE to the Mike
// qualification root holding assembled-mike-*.json and routing-selection.json.
// The vigil nordplate-top is the real positive fallback case: the registered
// clipping fit flips and the no-clipping alternative improves admissibly.
// ---------------------------------------------------------------------------

/// Position identity used by the cloth-coincident locks (matches the engine's
/// topology key: negative zero normalizes to zero).
fn position_key(point: [f32; 3]) -> [u32; 3] {
    point.map(|value| match value == 0.0 {
        true => 0,
        false => value.to_bits(),
    })
}

/// Mike diagnostic cases: (assembly label, routing prefix, shape/reference pairs).
type MikeCases<'a> = &'a [(&'a str, &'a str, &'a [(&'a str, &'a str)])];

#[test]
fn mike_guarded_fallback_env_gated() {
    let Some(root) = std::env::var_os("SCULPTO_FALLBACK_FIXTURE") else {
        eprintln!("SKIP mike_guarded_fallback_env_gated: SCULPTO_FALLBACK_FIXTURE unset");
        return;
    };
    let root = std::path::Path::new(&root);
    let read_json = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(root.join(name)).expect("configured Mike fixture is readable"),
        )
        .expect("fixture JSON")
    };
    let routing = read_json("routing-selection.json");
    let mesh = |shape: &serde_json::Value| -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        (
            serde_json::from_value(shape["vertices"].clone()).expect("vertices"),
            serde_json::from_value(shape["triangles"].clone()).expect("triangles"),
        )
    };
    // The engine's registered outfit.fit options.
    let options = FitOptions {
        gap_mode: sculpto_mesh::wrap::GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        opening_direction: Some([0.0, 0.0, 1.0]),
        ..FitOptions::default()
    };
    let cases: MikeCases = &[
        (
            "mike-vigil",
            "vigil",
            &[
                ("nordplate-top", "HIMBO - Body"),
                ("nordplate-legs", "HIMBO - Body"),
                ("sash", "HIMBO - Body"),
                ("gloves", "HIMBO - Hands"),
                ("gloveslow", "HIMBO - Hands"),
            ],
        ),
        (
            "mike-pilgrim",
            "pilgrim",
            &[("tunic", "HIMBO - Body"), ("robe", "HIMBO - Body")],
        ),
    ];
    let mut rows = Vec::new();
    let mut top_fallback_accepted = false;
    for (label, route, shapes) in cases {
        let dump = read_json(&format!("assembled-{label}.json"));
        let shapes_arr = dump["shapes"].as_array().expect("shapes");
        let find = |name: &str| {
            shapes_arr
                .iter()
                .find(|shape| shape["name"] == name)
                .unwrap_or_else(|| panic!("missing {name}"))
        };
        for (name, part) in *shapes {
            let (garment, faces) = mesh(find(name));
            let (body, body_faces) = mesh(find(part));
            let cloth_key = format!(
                "{}{}/{}",
                route,
                if *route == "vigil" && (*name == "gloves" || *name == "gloveslow") {
                    "-gloves"
                } else {
                    "-body"
                },
                name
            );
            let cloth_indices: Vec<usize> = serde_json::from_value(
                routing["selection"][&cloth_key]["cloth_triangle_indices"].clone(),
            )
            .expect("cloth selection");
            let cloth_set: std::collections::HashSet<usize> =
                cloth_indices.iter().copied().collect();
            let ordinary_faces: Vec<[u32; 3]> = faces
                .iter()
                .enumerate()
                .filter(|(index, _)| !cloth_set.contains(index))
                .map(|(_, &face)| face)
                .collect();
            let fixed: std::collections::HashSet<[u32; 3]> = faces
                .iter()
                .enumerate()
                .filter(|(index, _)| cloth_set.contains(index))
                .flat_map(|(_, face)| face.iter().map(|&v| position_key(garment[v as usize])))
                .collect();
            let (points, triangles, origins) =
                sculpto_mesh::control::compact(&garment, &ordinary_faces);
            let lock_values: Vec<f32> = origins
                .iter()
                .map(|&v| {
                    if fixed.contains(&position_key(garment[v as usize])) {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect();
            let locked: Vec<usize> = lock_values
                .iter()
                .enumerate()
                .filter(|&(_, &value)| value == 1.0)
                .map(|(index, _)| index)
                .collect();
            let locks = VertexMask::from_values(lock_values).expect("arena locks");
            let original: std::collections::HashSet<_> =
                self_intersection_witnesses(&points, &triangles)
                    .into_iter()
                    .collect();
            let before = intersection_pairs(&points, &triangles, &body, &body_faces);
            let measure_locks =
                VertexMask::from_values(vec![1.0; points.len()]).expect("measurement locks");
            let (_, source_inside) = fit_to_body(
                (&points, &triangles),
                (&body, &body_faces),
                Some(&measure_locks),
                &FitOptions::default(),
            )
            .expect("source inside measurement");
            let (fitted, report) = fit_and_repair(
                (&points, &triangles),
                (&body, &body_faces),
                Some(&locks),
                &options,
            )
            .expect("guarded Mike fit");
            let after = intersection_pairs(&fitted, &triangles, &body, &body_faces);
            let new_self = self_intersection_witnesses(&fitted, &triangles)
                .iter()
                .filter(|w| !original.contains(w))
                .count();
            let flipped = triangles
                .iter()
                .filter(|face| {
                    let normal = |p: &[[f32; 3]]| {
                        let [a, b, c] = face.map(|v| p[v as usize]);
                        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                        [
                            u[1] * v[2] - u[2] * v[1],
                            u[2] * v[0] - u[0] * v[2],
                            u[0] * v[1] - u[1] * v[0],
                        ]
                    };
                    let (n1, n2) = (normal(&points), normal(&fitted));
                    n1.iter().zip(n2).map(|(a, b)| a * b).sum::<f32>() < 0.0
                })
                .count();
            // Guards: nothing worsens, no new defect, locks bit-exact.
            assert!(after <= before, "{name} contacts worsened");
            assert!(
                report.inside_after <= source_inside.inside_after,
                "{name} inside groups worsened"
            );
            assert_eq!(new_self, 0, "{name} introduced self-crossings");
            assert_eq!(flipped, 0, "{name} flipped faces");
            assert_eq!(fitted.len(), points.len(), "{name} topology changed");
            for &vertex in &locked {
                assert_eq!(fitted[vertex], points[vertex], "{name} locked vertex moved");
            }
            assert_eq!(report.intersection_pairs, after, "{name} report contacts");
            assert_deformation(
                report.max_displacement,
                report.max_strain,
                &points,
                &fitted,
                &triangles,
            );
            if *name == "nordplate-top" {
                top_fallback_accepted = report.no_clipping_fallback_applied;
            }
            let row = serde_json::json!({
                "label": label, "shape": name, "reference": part,
                "pairs_before": before, "pairs_after": after,
                "inside_before": source_inside.inside_after,
                "inside_after": report.inside_after,
                "fallback_applied": report.no_clipping_fallback_applied,
                "max_displacement": report.max_displacement,
                "max_strain": report.max_strain,
                "new_self": new_self, "flipped": flipped,
                "locked_vertices": locked.len(),
                "poke_repair_applied": report.poke_repair_applied,
                "projection_applied": report.projection_applied,
            });
            eprintln!("MIKE_FALLBACK {row}");
            rows.push(row);
        }
    }
    assert!(
        top_fallback_accepted,
        "the pinned vigil top must accept the no-clipping fallback"
    );
    if let Some(path) = std::env::var_os("SCULPTO_FALLBACK_OUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&rows).expect("diagnostic JSON"),
        )
        .expect("write requested diagnostic artifact");
    }
}
