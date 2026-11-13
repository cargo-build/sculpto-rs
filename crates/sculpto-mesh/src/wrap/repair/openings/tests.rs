use super::*;
use crate::check::{intersection_pairs, self_intersection_witnesses};
use crate::wrap::repair::{measure_contacts, repair_openings};
use crate::wrap::{BodySurface, FitError};

type OwnedMesh = (Vec<[f32; 3]>, Vec<[u32; 3]>);

fn tube(x: f32, levels: &[f32], taper: f32) -> OwnedMesh {
    let points = levels
        .iter()
        .flat_map(|&z| {
            let r = 1.0 + taper * z;
            [[x - r, -r, z], [x + r, -r, z], [x + r, r, z], [x - r, r, z]]
        })
        .collect();
    let faces = (0..levels.len() - 1)
        .flat_map(|row| {
            (0..4).flat_map(move |column| {
                let a = (row * 4 + column) as u32;
                let b = (row * 4 + (column + 1) % 4) as u32;
                [[a, b, b + 4], [a, b + 4, a + 4]]
            })
        })
        .collect();
    (points, faces)
}

fn cube(x: f32, z: f32, half_height: f32) -> OwnedMesh {
    (
        vec![
            [x - 2.0, -2.0, z - half_height],
            [x + 2.0, -2.0, z - half_height],
            [x + 2.0, 2.0, z - half_height],
            [x - 2.0, 2.0, z - half_height],
            [x - 2.0, -2.0, z + half_height],
            [x + 2.0, -2.0, z + half_height],
            [x + 2.0, 2.0, z + half_height],
            [x - 2.0, 2.0, z + half_height],
        ],
        vec![
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
        ],
    )
}

fn append(mut a: OwnedMesh, b: OwnedMesh) -> OwnedMesh {
    let offset = a.0.len() as u32;
    a.0.extend(b.0);
    a.1.extend(b.1.into_iter().map(|face| face.map(|v| v + offset)));
    a
}

fn options() -> FitOptions {
    FitOptions {
        opening_direction: Some([0.0, 0.0, 1.0]),
        min_gap: 0.05,
        ..Default::default()
    }
}

#[test]
fn complete_multiple_openings_and_render_seams_follow_explicit_rotated_direction() {
    let mut source = append(
        tube(-4.0, &[0.0, 4.0, 8.0], 0.0),
        tube(4.0, &[0.0, 4.0, 8.0], 0.0),
    );
    let body = append(cube(-4.0, 0.0, 0.2), cube(4.0, 0.0, 0.3));
    let alias = source.0.len() as u32;
    source.0.push(source.0[0]);
    source.1[0][0] = alias;
    let surface = OpeningSurface::build((&source.0, &source.1)).expect("valid source loops");
    let output = surface
        .candidate(
            (&source.0, &source.1),
            (&body.0, &body.1),
            None,
            &options(),
            DVec3::Z,
        )
        .expect("finite candidate")
        .expect("boundary contacts");
    assert!(intersection_pairs(&source.0, &source.1, &body.0, &body.1) > 0);
    assert_eq!(intersection_pairs(&output, &source.1, &body.0, &body.1), 0);
    assert_eq!(output[0], output[alias as usize]);
    assert!(output[..4].iter().all(|p| (p[2] - 0.25).abs() < 1e-6));
    assert!(output[12..16].iter().all(|p| (p[2] - 0.35).abs() < 1e-6));
    assert!(within_strain((&source.0, &source.1), &output, 0.3));
    let rotate = |p: [f32; 3]| [p[2], p[1], -p[0]];
    let rotated: Vec<_> = source.0.iter().copied().map(rotate).collect();
    let rotated_body: Vec<_> = body.0.iter().copied().map(rotate).collect();
    let rotated_surface = OpeningSurface::build((&rotated, &source.1)).expect("rotated loops");
    let rotated_output = rotated_surface
        .candidate(
            (&rotated, &source.1),
            (&rotated_body, &body.1),
            None,
            &options(),
            DVec3::X,
        )
        .expect("rotated candidate")
        .expect("rotated witnesses");
    assert_eq!(
        rotated_output,
        output.into_iter().map(rotate).collect::<Vec<_>>()
    );
}

#[test]
fn interior_or_ambiguous_contacts_never_select_a_distant_opening() {
    let source = tube(0.0, &[0.0, 3.0, 5.0, 8.0], 0.0);
    let body = append(cube(0.0, 0.0, 0.2), cube(0.0, 4.0, 0.2));
    let surface = OpeningSurface::build((&source.0, &source.1)).expect("source");
    assert!(
        surface
            .candidate(
                (&source.0, &source.1),
                (&body.0, &body.1),
                None,
                &options(),
                DVec3::Z
            )
            .expect("finite")
            .is_none()
    );
    let short = tube(0.0, &[0.0, 4.0], 0.0);
    let surface = OpeningSurface::build((&short.0, &short.1)).expect("two valid loops");
    let body = cube(0.0, 0.0, 0.2);
    assert!(
        surface
            .candidate(
                (&short.0, &short.1),
                (&body.0, &body.1),
                None,
                &options(),
                DVec3::Z
            )
            .expect("finite")
            .is_none()
    );
}

#[test]
fn locks_rigid_parts_and_soft_masks_are_not_waived_by_the_candidate() {
    let mut source = tube(0.0, &[0.0, 4.0, 8.0], 0.0);
    source.0.push(source.0[0]);
    let body = cube(0.0, 0.0, 0.2);
    let surface = OpeningSurface::build((&source.0, &source.1)).expect("source");
    let free = surface
        .candidate(
            (&source.0, &source.1),
            (&body.0, &body.1),
            None,
            &options(),
            DVec3::Z,
        )
        .expect("finite")
        .expect("free candidate");
    let mut values = vec![0.0; source.0.len()];
    *values.last_mut().expect("alias") = 1.0;
    let lock = VertexMask::from_values(values).expect("strongest alias lock");
    assert!(
        surface
            .candidate(
                (&source.0, &source.1),
                (&body.0, &body.1),
                Some(&lock),
                &options(),
                DVec3::Z
            )
            .expect("finite")
            .is_none()
    );
    let mut rigid = options();
    rigid.rigid_parts = vec![-1; source.0.len()];
    rigid.rigid_parts[0] = 0;
    assert!(
        surface
            .candidate(
                (&source.0, &source.1),
                (&body.0, &body.1),
                None,
                &rigid,
                DVec3::Z
            )
            .expect("finite")
            .is_none()
    );
    let soft = VertexMask::from_values(vec![0.5; source.0.len()]).expect("soft mask");
    let output = surface
        .candidate(
            (&source.0, &source.1),
            (&body.0, &body.1),
            Some(&soft),
            &options(),
            DVec3::Z,
        )
        .expect("finite")
        .expect("soft candidate is still measured");
    assert!((output[0][2] - free[0][2] * 0.5).abs() < 1e-6);
    assert!(
        intersection_pairs(&output, &source.1, &body.0, &body.1) > 0,
        "soft mask cannot be ignored to claim clearance"
    );
}

#[test]
fn source_self_and_additional_strain_guards_reject_an_unsafe_opening_candidate() {
    let source = append(
        tube(0.0, &[0.0, 0.5, 1.0], 0.4),
        (
            vec![[-4.0, -4.0, 1.08], [4.0, -4.0, 1.08], [0.0, 4.0, 1.08]],
            vec![[0, 1, 2]],
        ),
    );
    let body = cube(0.0, 0.0, 0.4);
    let surface = OpeningSurface::build((&source.0, &source.1)).expect("valid source");
    let candidate = surface
        .candidate(
            (&source.0, &source.1),
            (&body.0, &body.1),
            None,
            &options(),
            DVec3::Z,
        )
        .expect("finite")
        .expect("complete opening lift");
    assert!(self_intersection_witnesses(&source.0, &source.1).is_empty());
    assert!(!self_intersection_witnesses(&candidate, &source.1).is_empty());
    assert!(within_strain((&source.0, &source.1), &candidate, 0.3));
    assert!(!within_strain((&source.0, &source.1), &candidate, 0.01));
    let report = measure_contacts(
        (&source.0, &source.1),
        (&body.0, &body.1),
        &BodySurface::build(&body.0, &body.1),
        &options(),
        &source.0,
    );
    let (output, after) = repair_openings(
        (&source.0, &source.1),
        (&body.0, &body.1),
        None,
        &options(),
        &HashSet::new(),
        source.0.clone(),
        report,
    )
    .expect("guarded proposal");
    assert_eq!(output, source.0);
    assert_eq!(after, report);
}

#[test]
fn invalid_configuration_and_zero_budget_are_explicit() {
    for direction in [[0.0; 3], [f32::NAN, 0.0, 1.0], [0.0, f32::INFINITY, 0.0]] {
        assert!(matches!(
            FitOptions {
                opening_direction: Some(direction),
                ..Default::default()
            }
            .validate(),
            Err(FitError::OpeningDirection(_))
        ));
    }
    assert!(matches!(
        FitOptions {
            max_strain: 0.0,
            ..options()
        }
        .validate(),
        Err(FitError::OpeningStrainRequired)
    ));
    assert!(
        FitOptions {
            opening_direction: Some([f32::from_bits(1), 0.0, 0.0]),
            ..Default::default()
        }
        .validate()
        .is_ok()
    );
    let source = tube(0.0, &[0.0, 4.0, 8.0], 0.0);
    let body = cube(0.0, 0.0, 0.2);
    let report = measure_contacts(
        (&source.0, &source.1),
        (&body.0, &body.1),
        &BodySurface::build(&body.0, &body.1),
        &options(),
        &source.0,
    );
    let (output, after) = repair_openings(
        (&source.0, &source.1),
        (&body.0, &body.1),
        None,
        &FitOptions {
            clipping_passes: 0,
            ..options()
        },
        &HashSet::new(),
        source.0.clone(),
        report,
    )
    .expect("zero opening budget");
    assert_eq!(output, source.0);
    assert_eq!(after, report);
    assert!(
        OpeningSurface::build((&[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]], &[[0, 1, 2]]))
            .is_none()
    );
}
