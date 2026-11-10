//! Focused synthetic tests of `wrap::fit_to_body` (FIT-02).
//!
//! The first four cases retain the accepted standoff/strain behavior of the
//! preserved experiment (rev 9656c3ad) through the new seam; the rest cover
//! the native options this port fills in: measured from-shape and keep gaps,
//! weld seams, locks, symmetric pulls, rigid parts with component follow,
//! body pokes, and refusal of invalid input. Controls are hand-computed on
//! planes and cylinders so convergence is exact or tightly bounded.

use std::f32::consts::{SQRT_2, TAU};
use std::ops::Range;

use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::wrap::{FitError, FitOptions, GapMode, fit_to_body, fit_to_body_from};

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn xz_radius(position: [f32; 3]) -> f32 {
    (position[0] * position[0] + position[2] * position[2]).sqrt()
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

/// An open tube around the y axis with `corners` per ring, wound outward.
fn tube(radius: f32, corners: usize, y_low: f32, y_high: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let ring = |y: f32| -> Vec<[f32; 3]> {
        (0..corners)
            .map(|corner| {
                let angle = corner as f32 * TAU / corners as f32;
                [radius * angle.cos(), y, radius * angle.sin()]
            })
            .collect()
    };
    let mut positions = ring(y_low);
    positions.extend(ring(y_high));
    let mut faces = Vec::new();
    for corner in 0..corners {
        let next = (corner + 1) % corners;
        let b = corner as u32;
        let b_next = next as u32;
        let t = (corners + corner) as u32;
        let t_next = (corners + next) as u32;
        faces.push([b, t, t_next]);
        faces.push([b, t_next, b_next]);
    }
    (positions, faces)
}

/// An open tube whose last corner column duplicates the first exactly, like a
/// split UV seam; `corners` unique positions, `corners + 1` per ring.
fn seam_tube(
    radius: f32,
    corners: usize,
    y_low: f32,
    y_high: f32,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let columns = corners + 1;
    let ring = |y: f32| -> Vec<[f32; 3]> {
        (0..columns)
            .map(|column| {
                let angle = (column % corners) as f32 * TAU / corners as f32;
                [radius * angle.cos(), y, radius * angle.sin()]
            })
            .collect()
    };
    let mut positions = ring(y_low);
    positions.extend(ring(y_high));
    let mut faces = Vec::new();
    for column in 0..corners {
        let b = column as u32;
        let b_next = (column + 1) as u32;
        let t = (columns + column) as u32;
        let t_next = (columns + column + 1) as u32;
        faces.push([b, t, t_next]);
        faces.push([b, t_next, b_next]);
    }
    (positions, faces)
}

/// A 3x3 grid on the xy plane at height `z`, wound with normals in +z.
fn grid(half: f32, z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let axis = [-half, 0.0, half];
    let mut positions = Vec::new();
    for &y in &axis {
        for &x in &axis {
            positions.push([x, y, z]);
        }
    }
    let mut faces = Vec::new();
    for row in 0..2u32 {
        for column in 0..2u32 {
            let a = row * 3 + column;
            let b = a + 1;
            let c = a + 4;
            let d = a + 3;
            faces.push([a, b, c]);
            faces.push([a, c, d]);
        }
    }
    (positions, faces)
}

/// Two parallel open plates, the lower one wound with normals in -z, so every
/// plate vertex has a pure +/-z welded normal.
fn plates(half: f32, z_low: f32, z_high: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut faces) = grid(half, z_high);
    let (low, low_faces) = grid(half, z_low);
    let offset = positions.len() as u32;
    positions.extend(low);
    faces.extend(
        low_faces
            .iter()
            .map(|face| [face[0] + offset, face[2] + offset, face[1] + offset]),
    );
    (positions, faces)
}

/// A plane with a four-sided pyramid bump whose tip is off the plane diagonal.
fn plane_with_bump() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut faces) = plane(3.0);
    let ring = [
        [-0.3, -0.45, 0.0],
        [0.7, -0.45, 0.0],
        [0.7, 0.55, 0.0],
        [-0.3, 0.55, 0.0],
    ];
    let base = positions.len() as u32;
    positions.extend_from_slice(&ring);
    let tip = positions.len() as u32;
    positions.push([0.2, 0.05, 1.0]);
    for corner in 0..4u32 {
        let next = (corner + 1) % 4;
        faces.push([tip, base + corner, base + next]);
    }
    (positions, faces)
}

/// Mean movement over a vertex range.
fn mean_move(before: &[[f32; 3]], after: &[[f32; 3]], range: Range<usize>) -> f32 {
    let total: f32 = range
        .clone()
        .map(|vertex| length(sub(after[vertex], before[vertex])))
        .sum();
    total / range.len() as f32
}

#[test]
fn inside_vertex_is_pushed_to_the_gap() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, -0.5], [1.0, 0.0, 0.4], [0.0, 1.0, 0.4]];
    let faces = vec![[0, 1, 2]];
    let (positions, report) = fit_to_body(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("fit");
    assert_eq!(report.inside_before, 1);
    assert_eq!(report.inside_after, 0);
    assert!(positions[0][2] >= 0.0, "inside vertex must leave the body");
}

#[test]
fn far_vertices_are_not_pulled() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, 50.0], [1.0, 0.0, 50.0], [0.0, 1.0, 50.0]];
    let faces = vec![[0, 1, 2]];
    let (positions, report) = fit_to_body(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("fit");
    assert_eq!(report.pulled, 0);
    assert_eq!(positions[0], garment[0]);
    assert_eq!(positions[2], garment[2]);
}

#[test]
fn from_shape_reapplies_the_authored_standoff() {
    let (authored, authored_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, 5.0], [1.0, 0.0, 5.0], [0.0, 1.0, 5.0]];
    let faces = vec![[0, 1, 2]];
    // New body sits lower: z = -3 instead of 0.
    let new_body: Vec<[f32; 3]> = authored.iter().map(|p| [p[0], p[1], p[2] - 3.0]).collect();
    let options = FitOptions {
        gap_mode: GapMode::FromShape,
        stages: 2,
        iterations: 4,
        relief_smoothing: 0,
        clipping_passes: 0,
        ..FitOptions::default()
    };
    let (positions, _) = fit_to_body_from(
        (&garment, &faces),
        (&new_body, &authored_faces),
        (&authored, &authored_faces),
        None,
        None,
        &options,
    )
    .expect("fit");
    // Authored standoff is 5; over the new body the garment must sit at 5.
    assert!(
        (positions[0][2] - 2.0).abs() < 0.2,
        "expected z ~ 2 for a 5-unit standoff over a plane at -3, got {}",
        positions[0][2]
    );
}

#[test]
fn from_shape_measures_on_the_gap_source_copy() {
    let (authored, authored_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, 6.0], [1.0, 0.0, 6.0], [0.0, 1.0, 6.0]];
    let faces = vec![[0, 1, 2]];
    // The source copy sits 2 above the authored body while the fitted copy
    // drifted to 6: the measured standoff is 2, not 6.
    let gap_source = vec![[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
    let new_body: Vec<[f32; 3]> = authored.iter().map(|p| [p[0], p[1], p[2] - 3.0]).collect();
    let options = FitOptions {
        gap_mode: GapMode::FromShape,
        stages: 2,
        iterations: 4,
        relief_smoothing: 0,
        clipping_passes: 0,
        ..FitOptions::default()
    };
    let (positions, _) = fit_to_body_from(
        (&garment, &faces),
        (&new_body, &authored_faces),
        (&authored, &authored_faces),
        Some(&gap_source),
        None,
        &options,
    )
    .expect("fit");
    assert!(
        (positions[0][2] + 1.0).abs() < 0.2,
        "expected z ~ -1 for the 2-unit source standoff over a plane at -3, got {}",
        positions[0][2]
    );
}

#[test]
fn strain_cap_keeps_the_authored_edge() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, -8.0], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]];
    let faces = vec![[0, 1, 2]];
    let options = FitOptions {
        max_distance: 100.0,
        ..FitOptions::default()
    };
    let (positions, report) =
        fit_to_body((&garment, &faces), (&body, &body_faces), None, &options).expect("fit");
    let rest = length(sub(garment[0], garment[1]));
    let fitted = length(sub(positions[0], positions[1]));
    assert!(report.kept_form > 0, "the strained pull must give way");
    assert!(
        fitted / rest <= 1.0 + options.max_strain + 0.05,
        "edge strain above the cap: {fitted} vs {rest}"
    );
    // No-trim invariant: fitting moves vertices, it never deletes any.
    assert_eq!(positions.len(), garment.len());
}

#[test]
fn cylinder_sleeve_follows_a_changed_radius() {
    let (body, body_faces) = tube(1.5, 16, 0.0, 2.0);
    let (sleeve, sleeve_faces) = tube(1.0, 8, 0.5, 1.5);
    let options = FitOptions {
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, report) = fit_to_body(
        (&sleeve, &sleeve_faces),
        (&body, &body_faces),
        None,
        &options,
    )
    .expect("fit");
    assert_eq!(report.vertices, 16);
    assert_eq!(report.unknowns, 16);
    assert_eq!(report.pulled, 16);
    assert_eq!(report.inside_before, 16);
    assert_eq!(report.inside_after, 0);
    assert!(
        report.max_gap_error <= 1e-3,
        "gap error {} above 1e-3",
        report.max_gap_error
    );
    assert!(
        report.mean_gap_error <= 1e-3,
        "mean gap error {} above 1e-3",
        report.mean_gap_error
    );
    for position in &positions {
        let radius = xz_radius(*position);
        assert!(
            (radius - 1.8).abs() <= 1e-3,
            "radius {radius} must sit at the 1.5 body plus the 0.3 gap"
        );
    }
    assert_eq!(report.flipped, 0);
}

#[test]
fn welded_seam_vertices_share_one_unknown() {
    let (body, body_faces) = tube(1.5, 16, 0.0, 2.0);
    let (sleeve, sleeve_faces) = seam_tube(1.0, 8, 0.5, 1.5);
    let options = FitOptions {
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, report) = fit_to_body(
        (&sleeve, &sleeve_faces),
        (&body, &body_faces),
        None,
        &options,
    )
    .expect("fit");
    // 18 render vertices, two exact seam pairs: 16 unknowns.
    assert_eq!(report.vertices, 18);
    assert_eq!(report.unknowns, 16);
    assert_eq!(
        positions[0], positions[8],
        "low seam copies must move together"
    );
    assert_eq!(
        positions[9], positions[17],
        "high seam copies must move together"
    );
}

#[test]
fn fully_locked_vertices_stay_bit_identical_through_clip() {
    let (body, body_faces) = tube(1.5, 16, 0.0, 2.0);
    let (sleeve, sleeve_faces) = seam_tube(1.0, 8, 0.5, 1.5);
    // High ring locked, low ring free; the locked seam pair welds to one
    // unknown that stays inside the body through every clipping pass.
    let values: Vec<f32> = (0..18).map(|vertex| u8::from(vertex >= 9) as f32).collect();
    let lock = VertexMask::from_values(values).expect("valid mask");
    let options = FitOptions {
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, report) = fit_to_body(
        (&sleeve, &sleeve_faces),
        (&body, &body_faces),
        Some(&lock),
        &options,
    )
    .expect("fit");
    for vertex in 9..18 {
        assert_eq!(
            positions[vertex], sleeve[vertex],
            "locked vertex {vertex} moved"
        );
    }
    for (vertex, &position) in positions.iter().enumerate().take(9) {
        let radius = xz_radius(position);
        assert!(
            (radius - 1.8).abs() < 0.01,
            "free low vertex {vertex} at radius {radius}"
        );
    }
    assert_eq!(
        report.inside_after, 8,
        "locked unknowns stay inside by design"
    );
}

#[test]
fn welded_lock_is_the_strongest_copy() {
    let (body, body_faces) = tube(1.5, 16, 0.0, 2.0);
    let (sleeve, sleeve_faces) = seam_tube(1.0, 8, 0.5, 1.5);
    // Only one copy of the high seam pair is locked; the welded unknown takes
    // the maximum, so both copies stay.
    let mut values = vec![0.0f32; 18];
    values[9] = 1.0;
    let lock = VertexMask::from_values(values).expect("valid mask");
    let options = FitOptions {
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, report) = fit_to_body(
        (&sleeve, &sleeve_faces),
        (&body, &body_faces),
        Some(&lock),
        &options,
    )
    .expect("fit");
    assert_eq!(positions[9], sleeve[9], "locked seam copy moved");
    assert_eq!(
        positions[17], sleeve[17],
        "welded twin of the locked copy moved"
    );
    let free_high = xz_radius(positions[13]);
    assert!(
        (free_high - 1.8).abs() < 0.01,
        "unlocked high vertex moved to {free_high}"
    );
    assert_eq!(
        report.inside_after, 1,
        "only the welded seam unknown stays inside"
    );
}

#[test]
fn fractional_locks_move_less_than_free_vertices() {
    let (body, body_faces) = tube(1.5, 16, 0.0, 2.0);
    let (sleeve, sleeve_faces) = tube(1.0, 8, 0.5, 1.5);
    let values: Vec<f32> = (0..16)
        .map(|vertex| if vertex >= 8 { 0.5 } else { 0.0 })
        .collect();
    let lock = VertexMask::from_values(values).expect("valid mask");
    let options = FitOptions {
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, _) = fit_to_body(
        (&sleeve, &sleeve_faces),
        (&body, &body_faces),
        Some(&lock),
        &options,
    )
    .expect("fit");
    let soft = mean_move(&sleeve, &positions, 8..16);
    let free = mean_move(&sleeve, &positions, 0..8);
    assert!(soft < 0.1, "half-locked ring moved {soft}");
    assert!(free > 0.5, "free ring only moved {free}");
}

#[test]
fn keep_mode_preserves_standoff_and_pushes_out_clipping() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![
        [0.0, 0.0, -0.5],
        [1.0, 0.0, 0.5],
        [0.0, 1.0, 0.5],
        [1.0, 1.0, 0.5],
    ];
    let faces = vec![[0, 1, 2], [1, 3, 2]];
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        stiffness: 1e-3,
        max_strain: 0.0,
        ..FitOptions::default()
    };
    let (positions, report) =
        fit_to_body((&garment, &faces), (&body, &body_faces), None, &options).expect("fit");
    assert_eq!(report.inside_before, 1);
    assert!(
        positions[0][2] > 0.0,
        "clipping vertex must be pushed out, got {}",
        positions[0][2]
    );
    for (vertex, position) in positions.iter().enumerate().take(4).skip(1) {
        assert!(
            (position[2] - 0.5).abs() < 0.01,
            "kept vertex {vertex} drifted to {}",
            position[2]
        );
    }
}

#[test]
fn poke_detection_uses_the_first_ray_crossing_instead_of_the_closest_triangle() {
    let body = [
        [0.0, 0.0, 1.0],
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [0.0, 2.0, 0.0],
    ];
    let body_faces = [[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]];
    // The near vertical face misses the top vertex's inward ray. The farther
    // horizontal face crosses it exactly at the body's opposite surface.
    let garment = [
        [0.1, -0.1, 0.9],
        [0.1, 0.1, 0.9],
        [0.1, 0.0, 1.1],
        [-3.0, -3.0, 0.0],
        [3.0, -3.0, 0.0],
        [0.0, 4.0, 0.0],
    ];
    let garment_faces = [[0, 1, 2], [3, 4, 5]];
    let options = FitOptions {
        max_distance: 0.001,
        stages: 1,
        iterations: 1,
        clipping_passes: 0,
        ..FitOptions::default()
    };
    let (_, report) = fit_to_body(
        (&garment, &garment_faces),
        (&body, &body_faces),
        None,
        &options,
    )
    .expect("valid disjoint garment faces");
    assert_eq!(
        report.poke_before, 1,
        "a closer face must not hide a ray crossing"
    );
}

#[test]
fn symmetric_pulls_the_mesh_toward_body_points_it_does_not_reach() {
    let (body, body_faces) = plates(2.0, 0.0, 2.0);
    let (patch, patch_faces) = grid(1.0, 1.5);
    let plain = FitOptions {
        min_normal_agreement: -1.0,
        ..FitOptions::default()
    };
    let symmetric = FitOptions {
        min_normal_agreement: -1.0,
        symmetric: true,
        ..FitOptions::default()
    };
    let (plain_positions, _) =
        fit_to_body((&patch, &patch_faces), (&body, &body_faces), None, &plain).expect("plain fit");
    let (symmetric_positions, _) = fit_to_body(
        (&patch, &patch_faces),
        (&body, &body_faces),
        None,
        &symmetric,
    )
    .expect("symmetric fit");
    let center = patch
        .iter()
        .position(|position| position[0] == 0.0 && position[1] == 0.0)
        .expect("grid center vertex");
    // Without symmetric pulls the patch rides at the gap over the top plate
    // (z ~ 2.3); the bottom plate pulling through the middle drags it down.
    let difference = plain_positions[center][2] - symmetric_positions[center][2];
    // Executed against upstream 58dfbd30 with these same meshes/options:
    // plain=2.299998760, symmetric=2.019999981, difference=0.2799987793.
    assert!((plain_positions[center][2] - 2.299_998_8).abs() < 1e-5);
    assert!((symmetric_positions[center][2] - 2.02).abs() < 1e-5);
    assert!(
        (difference - 0.279_998_78).abs() < 1e-5,
        "native displacement mismatch: {difference}"
    );
}

#[test]
fn rigid_parts_follow_the_pulled_piece_as_a_whole() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
        [5.0, 5.0, 50.0],
        [6.0, 5.0, 50.0],
        [6.0, 6.0, 50.0],
        [5.0, 6.0, 50.0],
    ];
    let faces = vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
    let options = FitOptions {
        rigid_parts: vec![-1, -1, -1, -1, 0, 0, 0, 0],
        ..FitOptions::default()
    };
    let (positions, report) =
        fit_to_body((&garment, &faces), (&body, &body_faces), None, &options).expect("fit");
    assert_eq!(report.followed, 4, "the detached buckle must follow");
    assert_eq!(report.pulled, 4, "only the patch is near enough to pull");
    assert!(
        (positions[0][2] - 0.3).abs() < 0.05,
        "patch must ride at the gap, got {}",
        positions[0][2]
    );
    // A pure translation fits exactly: all four corners move by one vector
    // and keep the buckle's edge lengths.
    let moves: Vec<[f32; 3]> = (4..8)
        .map(|vertex| sub(positions[vertex], garment[vertex]))
        .collect();
    for vertex in 1..4 {
        let delta = length(sub(moves[vertex], moves[0]));
        assert!(
            delta < 1e-4,
            "buckle corner {vertex} broke the rigid motion by {delta}"
        );
    }
    for (a, b, rest) in [
        (4usize, 5usize, 1.0),
        (5, 6, 1.0),
        (6, 7, 1.0),
        (4, 6, SQRT_2),
    ] {
        let edge = length(sub(positions[a], positions[b]));
        assert!(
            (edge - rest).abs() < 1e-4,
            "buckle edge {a}-{b} strained to {edge}"
        );
    }
}

#[test]
fn body_pokes_between_garment_vertices_bulge_the_garment() {
    let (body, body_faces) = plane_with_bump();
    let garment = vec![
        [-2.0, -2.0, 0.5],
        [2.0, -2.0, 0.5],
        [2.0, 2.0, 0.5],
        [-2.0, 2.0, 0.5],
    ];
    let faces = vec![[0, 1, 2], [0, 2, 3]];
    let (positions, report) = fit_to_body(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("fit");
    assert!(
        report.poke_before >= 1,
        "the bump tip must poke through the coarse garment"
    );
    assert!(
        report.poke_after < report.poke_before,
        "clipping passes must resolve pokes: before {}, after {}",
        report.poke_before,
        report.poke_after
    );
    let highest = positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    assert!(
        highest > 0.6,
        "the garment must bulge over the bump, max z {highest}"
    );
}

#[test]
fn invalid_options_and_geometry_are_refused_before_any_query() {
    let (body, body_faces) = plane(10.0);
    let garment = vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];
    let faces = vec![[0, 1, 2]];
    let fit =
        |options: &FitOptions| fit_to_body((&garment, &faces), (&body, &body_faces), None, options);

    // Empty and malformed geometry.
    assert!(matches!(
        fit_to_body(
            (&[], &faces),
            (&body, &body_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::EmptyGarment)
    ));
    assert!(matches!(
        fit_to_body(
            (&garment, &faces),
            (&[], &body_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::EmptyBody)
    ));
    let no_faces: Vec<[u32; 3]> = Vec::new();
    assert!(matches!(
        fit_to_body(
            (&garment, &faces),
            (&body, &no_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::EmptyBodyFaces)
    ));
    let non_finite = vec![[f32::NAN, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];
    assert!(matches!(
        fit_to_body(
            (&non_finite, &faces),
            (&body, &body_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::NonFiniteGarmentPosition { vertex: 0 })
    ));
    let bad_garment_faces = vec![[0, 1, 3]];
    assert!(matches!(
        fit_to_body(
            (&garment, &bad_garment_faces),
            (&body, &body_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::GarmentFaceIndex {
            triangle: 0,
            vertex: 3
        })
    ));
    let bad_body_faces = vec![[0, 1, 9]];
    assert!(matches!(
        fit_to_body(
            (&garment, &faces),
            (&body, &bad_body_faces),
            None,
            &FitOptions::default()
        ),
        Err(FitError::BodyFaceIndex {
            triangle: 0,
            vertex: 9
        })
    ));

    // Per-vertex lengths and ids.
    let short_mask = VertexMask::from_values(vec![0.0]).expect("valid mask");
    assert!(matches!(
        fit_to_body(
            (&garment, &faces),
            (&body, &body_faces),
            Some(&short_mask),
            &FitOptions::default()
        ),
        Err(FitError::LockLength {
            expected: 3,
            actual: 1
        })
    ));
    assert!(matches!(
        fit(&FitOptions {
            target_gaps: vec![0.3; 7],
            ..FitOptions::default()
        }),
        Err(FitError::TargetGapLength {
            expected: 3,
            actual: 7
        })
    ));
    assert!(matches!(
        fit(&FitOptions {
            target_gaps: vec![f32::NAN, 0.2, 0.2],
            ..FitOptions::default()
        }),
        Err(FitError::TargetGapValue { vertex: 0, .. })
    ));
    assert!(matches!(
        fit(&FitOptions {
            rigid_parts: vec![-1, -1, -2],
            ..FitOptions::default()
        }),
        Err(FitError::RigidPartId {
            vertex: 2,
            part: -2
        })
    ));
    assert!(matches!(
        fit(&FitOptions {
            rigid_parts: vec![0],
            ..FitOptions::default()
        }),
        Err(FitError::RigidPartLength {
            expected: 3,
            actual: 1
        })
    ));

    // Entry-point mode consistency.
    let from_shape = FitOptions {
        gap_mode: GapMode::FromShape,
        ..FitOptions::default()
    };
    assert!(matches!(
        fit(&from_shape),
        Err(FitError::FromShapeNeedsSource)
    ));
    assert!(matches!(
        fit_to_body_from(
            (&garment, &faces),
            (&body, &body_faces),
            (&body, &body_faces),
            None,
            None,
            &FitOptions::default()
        ),
        Err(FitError::SourceNeedsFromShape)
    ));
    assert!(matches!(
        fit_to_body_from(
            (&garment, &faces),
            (&body, &body_faces),
            (&body, &body_faces),
            None,
            None,
            &FitOptions {
                gap_mode: GapMode::FromShape,
                target_gaps: vec![0.3; 3],
                ..FitOptions::default()
            }
        ),
        Err(FitError::TargetGapsWithFromShape)
    ));
    let wrong_source = vec![[0.0, 0.0, 1.0], [1.0, 0.0, 1.0]];
    assert!(matches!(
        fit_to_body_from(
            (&garment, &faces),
            (&body, &body_faces),
            (&body, &body_faces),
            Some(&wrong_source),
            None,
            &from_shape
        ),
        Err(FitError::GapSourceLength {
            expected: 3,
            actual: 2
        })
    ));

    // Option ranges.
    assert!(matches!(
        fit(&FitOptions {
            max_distance: 0.0,
            ..FitOptions::default()
        }),
        Err(FitError::MaxDistance(0.0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            stages: 0,
            ..FitOptions::default()
        }),
        Err(FitError::Stages(0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            solver_iterations: 0,
            ..FitOptions::default()
        }),
        Err(FitError::SolverIterations(0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            stiffness: -1.0,
            ..FitOptions::default()
        }),
        Err(FitError::Stiffness(-1.0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            min_normal_agreement: 2.0,
            ..FitOptions::default()
        }),
        Err(FitError::MinNormalAgreement(2.0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            flip_strength: 1.5,
            ..FitOptions::default()
        }),
        Err(FitError::FlipStrength(1.5))
    ));
    assert!(matches!(
        fit(&FitOptions {
            weld_distance: -1.0,
            ..FitOptions::default()
        }),
        Err(FitError::WeldDistance(-1.0))
    ));
    assert!(matches!(
        fit(&FitOptions {
            poke_depth: -1.0,
            ..FitOptions::default()
        }),
        Err(FitError::PokeDepth(-1.0))
    ));
}
