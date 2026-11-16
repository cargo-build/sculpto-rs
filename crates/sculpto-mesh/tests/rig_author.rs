//! The authored skin: one owner in the middle of a limb, a blend at the joint,
//! and no influence across the midline or into a shell no bone reaches.
//! @req DECL0008

use glam::DVec3;
use sculpto_mesh::rig::author::{AuthorSettings, BodySide, DeformBone, author_weights};

/// A closed tube along `axis` from `start` for `length`, `radius` thick: a
/// surface with real adjacency, `rings` stations of `sides` vertices each.
fn tube(
    start: DVec3,
    axis: DVec3,
    length: f64,
    radius: f64,
    rings: usize,
    sides: usize,
) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    let axis = axis.normalize();
    let up = match axis.cross(DVec3::Y).length() > 1e-6 {
        true => DVec3::Y,
        false => DVec3::Z,
    };
    let basis = axis.cross(up).normalize();
    let third = axis.cross(basis).normalize();
    let mut points = Vec::new();
    for ring in 0..rings {
        let t = ring as f64 / (rings - 1) as f64;
        let centre = start + axis * (length * t);
        for side in 0..sides {
            let angle = std::f64::consts::TAU * side as f64 / sides as f64;
            let offset = basis * (radius * angle.cos()) + third * (radius * angle.sin());
            points.push((centre + offset).to_array());
        }
    }
    let mut triangles = Vec::new();
    let index = |ring: usize, side: usize| (ring * sides + side % sides) as u32;
    for ring in 0..rings - 1 {
        for side in 0..sides {
            let a = index(ring, side);
            let b = index(ring, side + 1);
            let c = index(ring + 1, side);
            let d = index(ring + 1, side + 1);
            triangles.push([a, c, b]);
            triangles.push([b, c, d]);
        }
    }
    (points, triangles)
}

/// One straight limb of two bones sharing the joint at its middle.
fn limb() -> (Vec<[f64; 3]>, Vec<[u32; 3]>, Vec<DeformBone>) {
    let (points, triangles) = tube(DVec3::ZERO, DVec3::X, 0.30, 0.05, 40, 24);
    let bones = vec![
        DeformBone {
            name: "Upper".into(),
            side: None,
            segments: vec![(DVec3::ZERO, DVec3::new(0.15, 0.0, 0.0))],
        },
        DeformBone {
            name: "Lower".into(),
            side: None,
            segments: vec![(DVec3::new(0.15, 0.0, 0.0), DVec3::new(0.30, 0.0, 0.0))],
        },
    ];
    (points, triangles, bones)
}

/// Weight of one bone in one row.
fn weight(row: &sculpto_mesh::garment::Influences, bone: &str) -> f64 {
    row.get(bone).copied().unwrap_or(0.0) as f64
}

#[test]
fn a_limb_mid_segment_has_one_owner_and_its_joint_blends() {
    let (points, triangles, bones) = limb();
    let flesh = vec![true; points.len()];
    let authored = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect("the tube is a valid surface");

    assert_eq!(authored.rows.len(), points.len());
    for row in &authored.rows {
        let sum: f32 = row.values().sum();
        assert!((sum - 1.0).abs() < 1e-4, "row sum {sum}");
        assert!(row.len() <= 4, "row carries {} influences", row.len());
    }

    // The middle of the upper bone: one owner, essentially to the exclusion of
    // the neighbour — the property 0158 measured on Mixamo's own skin.
    let mid_upper = points
        .iter()
        .enumerate()
        .filter(|(_, point)| (point[0] - 0.075).abs() < 0.004)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(!mid_upper.is_empty());
    for vertex in mid_upper {
        let owned = weight(&authored.rows[vertex], "Upper");
        assert!(
            owned > 0.9,
            "mid-segment ownership {owned} is not concentrated"
        );
    }

    // The joint itself: both bones carry the flesh, neither dominates it.
    let at_joint = points
        .iter()
        .enumerate()
        .filter(|(_, point)| (point[0] - 0.15).abs() < 0.004)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(!at_joint.is_empty());
    for vertex in at_joint {
        let row = &authored.rows[vertex];
        let (upper, lower) = (weight(row, "Upper"), weight(row, "Lower"));
        assert!(
            upper > 0.2 && lower > 0.2,
            "joint blend upper {upper} lower {lower}"
        );
    }
}

#[test]
fn opposite_side_bones_never_reach_the_other_side() {
    // Two parallel tubes, the left one on -x the way the seated palette lays out
    // a character: the side tag, not the sign convention, decides.
    let (mut points, mut triangles) =
        tube(DVec3::new(-0.20, 0.0, 0.0), DVec3::X, 0.30, 0.04, 20, 16);
    let (right_points, right_triangles) =
        tube(DVec3::new(0.20, 0.0, 0.0), DVec3::X, 0.30, 0.04, 20, 16);
    let offset = points.len() as u32;
    points.extend(right_points);
    triangles.extend(
        right_triangles
            .iter()
            .map(|triangle| triangle.map(|index| index + offset)),
    );
    let bones = vec![
        DeformBone {
            name: "LeftLimb".into(),
            side: Some(BodySide::Left),
            segments: vec![(DVec3::new(-0.20, 0.0, 0.0), DVec3::new(-0.20, 0.30, 0.0))],
        },
        DeformBone {
            name: "RightLimb".into(),
            side: Some(BodySide::Right),
            segments: vec![(DVec3::new(0.20, 0.0, 0.0), DVec3::new(0.20, 0.30, 0.0))],
        },
    ];
    let flesh = vec![true; points.len()];
    let authored = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect("two tubes are a valid surface");

    for (vertex, point) in points.iter().enumerate() {
        let row = &authored.rows[vertex];
        match point[0] > 0.05 {
            true => assert_eq!(
                weight(row, "LeftLimb"),
                0.0,
                "vertex {vertex} on the right carries the left limb"
            ),
            false if point[0] < -0.05 => assert_eq!(
                weight(row, "RightLimb"),
                0.0,
                "vertex {vertex} on the left carries the right limb"
            ),
            false => {}
        }
    }
}

#[test]
fn a_shell_no_bone_reaches_carries_no_weight_from_it() {
    // A separate patch far from the limb: geodesically unreachable, so the bone
    // that does not own it contributes nothing — never a silent Euclidean guess.
    let (mut points, mut triangles, bones) = limb();
    let (patch, patch_triangles) = tube(DVec3::new(0.0, 0.6, 0.0), DVec3::X, 0.10, 0.02, 6, 8);
    let offset = points.len() as u32;
    points.extend(patch);
    triangles.extend(
        patch_triangles
            .iter()
            .map(|triangle| triangle.map(|index| index + offset)),
    );
    let flesh = vec![true; points.len()];
    let authored = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect("the patch is a valid surface");

    let reach = authored.report.footprints[1].seeds;
    assert!(reach > 0, "the lower bone owns flesh on the limb");
    for (vertex, point) in points.iter().enumerate() {
        if point[1] < 0.3 {
            continue;
        }
        let row = &authored.rows[vertex];
        let sum: f32 = row.values().sum();
        assert!((sum - 1.0).abs() < 1e-4, "patch row sum {sum}");
        assert!(
            weight(row, "Lower") < 1e-3,
            "the patch borrowed the lower bone at {}",
            weight(row, "Lower")
        );
    }
}

#[test]
fn a_bone_with_no_segment_is_refused() {
    let (points, triangles, mut bones) = limb();
    bones[1].segments.clear();
    let flesh = vec![true; points.len()];
    let error = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect_err("a leaf has no flesh to seed");
    assert_eq!(
        error.to_string(),
        "bone \"Lower\" has no segment to own flesh"
    );
}

#[test]
fn a_bone_nothing_owns_carries_no_weight() {
    // A third bone whose segment sits in empty space owns no surface vertex, so
    // the authored skin leaves it at zero instead of guessing a Euclidean band.
    let (points, triangles, mut bones) = limb();
    bones.push(DeformBone {
        name: "Ghost".into(),
        side: None,
        segments: vec![(DVec3::new(0.0, 2.0, 0.0), DVec3::new(0.0, 2.2, 0.0))],
    });
    let flesh = vec![true; points.len()];
    let authored = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect("the limb is a valid surface");
    assert_eq!(authored.report.footprints[2].seeds, 0);
    assert_eq!(authored.report.footprints[2].mass, 0.0);
    assert!(authored.rows.iter().all(|row| weight(row, "Ghost") == 0.0));
}

#[test]
fn a_shell_inherits_the_flesh_it_lies_on() {
    // A sleeve around the limb: its own surface, no anatomy of its own. It must
    // carry the flesh's weights, or the layer and the body move independently.
    let (mut points, mut triangles, bones) = limb();
    let (shell_points, shell_triangles) = tube(DVec3::ZERO, DVec3::X, 0.30, 0.055, 20, 16);
    let offset = points.len() as u32;
    let flesh_len = points.len();
    points.extend(shell_points);
    triangles.extend(
        shell_triangles
            .iter()
            .map(|triangle| triangle.map(|index| index + offset)),
    );

    let mut flesh = vec![true; points.len()];
    for flag in flesh.iter_mut().skip(flesh_len) {
        *flag = false;
    }
    let authored = author_weights(
        &points,
        &triangles,
        &bones,
        &flesh,
        &AuthorSettings::default(),
    )
    .expect("the sleeve is a valid surface");

    assert_eq!(authored.report.flesh_vertices, flesh_len);
    assert_eq!(authored.report.inherited_vertices, points.len() - flesh_len);
    for vertex in flesh_len..points.len() {
        let row = &authored.rows[vertex];
        let borrowed = authored.rows[..flesh_len]
            .iter()
            .any(|flesh_row| flesh_row == row);
        assert!(borrowed, "shell vertex {vertex} authored its own weights");
        // The sleeve above the upper bone rides the upper bone.
        if points[vertex][0] < 0.12 {
            assert!(weight(row, "Upper") > 0.5, "sleeve weighted {row:?}");
        }
    }
}
