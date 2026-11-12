//! Authoring retries retain the original garment's construction guards. @req DECL0013

use std::path::Path;
use std::time::Instant;

use glam::DVec3;
use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::clip_fix::{ClipFixOptions, fix_clipping};
use sculpto_mesh::control::SourceSurface;
use sculpto_mesh::control::measure::ControlDiagnostics;
use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::repair::{fit_and_repair, repair_after_fit};
use sculpto_mesh::wrap::{FitOptions, GapMode, Mesh, fit_to_body};
use serde_json::json;

fn two_pokes() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let positions = [(-1.0, 0.75), (1.0, 1.25)]
        .into_iter()
        .flat_map(|(x, top)| {
            [
                [x - 0.25, -0.25, -1.0],
                [x + 0.25, -0.25, -1.0],
                [x + 0.25, 0.25, -1.0],
                [x - 0.25, 0.25, -1.0],
                [x, 0.0, top],
            ]
        })
        .collect();
    let faces = (0..2)
        .flat_map(|part| {
            [
                [0, 1, 4],
                [1, 2, 4],
                [2, 3, 4],
                [3, 0, 4],
                [0, 2, 1],
                [0, 3, 2],
            ]
            .map(|face| face.map(|v| v + part * 5))
        })
        .collect();
    (positions, faces)
}

fn repair_options(budget: u32) -> FitOptions {
    // Freeze the SurfaceFit fallback so the test isolates the authoring repair budget.
    FitOptions {
        max_distance: 0.01,
        poke_depth: 0.0,
        clipping_passes: budget,
        ..Default::default()
    }
}

fn inside(garment: Mesh<'_>, body: Mesh<'_>) -> usize {
    let locks = VertexMask::from_values(vec![1.0; garment.0.len()]).expect("measurement locks");
    let (same, report) = fit_to_body(garment, body, Some(&locks), &FitOptions::default())
        .expect("full-radius inside query");
    assert_eq!(same, garment.0);
    report.inside_after
}

#[test]
fn outside_vertices_do_not_stop_a_subsequent_face_contact_repair() {
    let body = two_pokes();
    let garment = [[-3.0, -3.0, 0.25], [3.0, -3.0, 0.25], [0.0, 3.0, 0.25]];
    let faces = [[0, 1, 2]];
    let body_mesh = (&body.0[..], &body.1[..]);
    let primitive = |p: &[[f32; 3]]| {
        fix_clipping(
            (p, &faces),
            body_mesh,
            None,
            &ClipFixOptions {
                fix_poke_through: true,
                ..Default::default()
            },
        )
        .expect("existing clipping primitive")
        .0
    };
    let first = primitive(&garment);
    let second = primitive(&first);
    let contacts = |p: &[[f32; 3]]| intersection_pairs(p, &faces, &body.0, &body.1);
    assert_eq!(
        (contacts(&garment), contacts(&first), contacts(&second)),
        (8, 4, 0)
    );
    assert_eq!(
        (
            inside((&garment, &faces), body_mesh),
            inside((&first, &faces), body_mesh)
        ),
        (0, 0)
    );
    let (single, single_report) =
        repair_after_fit((&garment, &faces), body_mesh, None, &repair_options(2))
            .expect("unchanged single repair stage");
    assert_eq!(single, first);
    assert_eq!(
        (single_report.inside_after, single_report.intersection_pairs),
        (0, 4)
    );
    let (repaired, report) =
        fit_and_repair((&garment, &faces), body_mesh, None, &repair_options(2))
            .expect("bounded authoring repair");
    assert_eq!(repaired, second);
    assert_eq!((report.inside_after, report.intersection_pairs), (0, 0));
    assert!(report.poke_repair_applied);
    assert!(self_intersection_witnesses(&repaired, &faces).is_empty());
    let (more, _) = fit_and_repair((&garment, &faces), body_mesh, None, &repair_options(20))
        .expect("stop at clearance");
    assert_eq!(
        more, repaired,
        "unused budget cannot move an already clear garment"
    );
}

#[test]
fn zero_budget_keeps_one_poke_stage_and_disables_inside_projection() {
    let body = two_pokes();
    let garment = [[-3.0, -3.0, 0.25], [3.0, -3.0, 0.25], [0.0, 3.0, 0.25]];
    let faces = [[0, 1, 2]];
    let run = |budget| {
        fit_and_repair(
            (&garment, &faces),
            (&body.0, &body.1),
            None,
            &repair_options(budget),
        )
        .expect("budget control")
    };
    let (zero, report) = run(0);
    let (one, _) = run(1);
    assert_eq!(zero, one);
    assert_ne!(
        zero, garment,
        "zero retains the existing mandatory poke repair"
    );
    assert_eq!(report.intersection_pairs, 4);
    assert!(!report.projection_applied);
    let plane = [
        [-10.0, -10.0, 0.0],
        [10.0, -10.0, 0.0],
        [10.0, 10.0, 0.0],
        [-10.0, 10.0, 0.0],
    ];
    let plane_faces = [[0, 1, 2], [0, 2, 3]];
    let deep = [[0.0, 0.0, -5.0]];
    let stage = |budget| {
        repair_after_fit(
            (&deep, &[]),
            (&plane, &plane_faces),
            None,
            &FitOptions {
                clipping_passes: budget,
                ..Default::default()
            },
        )
        .expect("projection budget")
    };
    let (unchanged, disabled) = stage(0);
    let (projected, enabled) = stage(1);
    assert_eq!(unchanged, deep);
    assert_eq!(disabled.inside_after, 1);
    assert!(!disabled.projection_applied);
    assert!(projected[0][2] > 0.0 && enabled.projection_applied);
    assert_eq!(enabled.inside_after, 0);
}

#[test]
fn rejected_later_self_contact_keeps_the_previous_accepted_geometry() {
    let body = two_pokes();
    let garment = [
        [-3.0, -3.0, 0.25],
        [3.0, -3.0, 0.25],
        [0.0, 3.0, 0.25],
        [-3.0, -3.0, 1.0],
        [3.0, -3.0, 1.4],
        [0.0, 3.0, 1.4],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    let locks =
        VertexMask::from_values(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]).expect("fixed upper layer");
    let stage = |p: &[[f32; 3]]| {
        repair_after_fit(
            (p, &faces),
            (&body.0, &body.1),
            Some(&locks),
            &repair_options(1),
        )
        .expect("diagnostic stage")
    };
    let (first, before) = stage(&garment);
    let (second, after) = stage(&first);
    assert!(self_intersection_witnesses(&garment, &faces).is_empty());
    assert!(self_intersection_witnesses(&first, &faces).is_empty());
    assert!(
        !self_intersection_witnesses(&second, &faces).is_empty(),
        "the second stage must expose the new self contact"
    );
    assert!(after.intersection_pairs < before.intersection_pairs);
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body.0, &body.1),
        Some(&locks),
        &repair_options(20),
    )
    .expect("original-self guarded retry");
    assert_eq!(
        output, first,
        "retain the accepted prefix rather than the unsafe candidate"
    );
    assert_eq!(report.intersection_pairs, before.intersection_pairs);
    assert_eq!(&output[3..], &garment[3..]);
    assert!(self_intersection_witnesses(&output, &faces).is_empty());
}

#[test]
fn no_improvement_preserves_strongest_welded_locks_and_rigid_vertices() {
    let body = two_pokes();
    let garment = [
        [-3.0, -3.0, 0.25],
        [3.0, -3.0, 0.25],
        [0.0, 3.0, 0.25],
        [-3.0, -3.0, 0.25],
    ];
    let faces = [[0, 1, 2]];
    let locks =
        VertexMask::from_values(vec![0.0, 1.0, 0.0, 1.0]).expect("only duplicate corner locked");
    let options = FitOptions {
        rigid_parts: vec![-1, -1, 0, -1],
        ..repair_options(20)
    };
    let (output, report) = fit_and_repair(
        (&garment, &faces),
        (&body.0, &body.1),
        Some(&locks),
        &options,
    )
    .expect("locked budget exhaustion");
    assert_eq!(output, garment);
    assert_eq!(report.intersection_pairs, 8);
    assert!(!report.poke_repair_applied && !report.projection_applied);
}

#[test]
fn authoring_validation_precedes_original_witness_queries() {
    let body = two_pokes();
    assert!(
        fit_and_repair(
            (&[[0.0, 0.0, 1.0]], &[[0, 1, 2]]),
            (&body.0, &body.1),
            None,
            &FitOptions::default()
        )
        .is_err()
    );
    assert!(
        fit_and_repair(
            (&[[f32::NAN, 0.0, 1.0]], &[]),
            (&body.0, &body.1),
            None,
            &FitOptions::default()
        )
        .is_err()
    );
}

#[test]
fn configured_recovered_pants_rust_repair_meets_original_quality() {
    let Some(root) = std::env::var_os("SCULPTO_REPAIR_FIXTURE") else {
        eprintln!("SKIP recovered pants Rust repair: SCULPTO_REPAIR_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(root.join(name)).expect("configured recovered fixture"),
        )
        .expect("fixture JSON")
    };
    let original = read("original.json");
    let prepared = read("prepared-render.json");
    let source: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("original positions");
    let source_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("original faces");
    let source_uvs: Vec<[f32; 2]> =
        serde_json::from_value(original["uvs"].clone()).expect("original UVs");
    let rest: Vec<[f32; 3]> =
        serde_json::from_value(prepared["positions"].clone()).expect("prepared original geometry");
    let faces: Vec<[u32; 3]> =
        serde_json::from_value(prepared["triangles"].clone()).expect("prepared faces");
    let uvs: Vec<[f32; 2]> = serde_json::from_value(prepared["uvs"].clone()).expect("retained UVs");
    let origins: Vec<usize> = serde_json::from_value(prepared["render_source_vertices"].clone())
        .expect("source vertices");
    let original_faces: Vec<usize> =
        serde_json::from_value(prepared["source_faces"].clone()).expect("source faces");
    let mapping: Vec<usize> =
        serde_json::from_value(prepared["render_to_physical"].clone()).expect("seam mapping");
    let body: Vec<[f32; 3]> = serde_json::from_value(original["reference"]["positions"].clone())
        .expect("complete reference");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("complete reference faces");
    assert_eq!(
        (
            source.len(),
            source_faces.len(),
            rest.len(),
            faces.len(),
            body.len(),
            body_faces.len()
        ),
        (392, 632, 370, 612, 5825, 10488)
    );
    origins.iter().enumerate().for_each(|(v, &id)| {
        assert_eq!(rest[v], source[id]);
        assert_eq!(uvs[v], source_uvs[id]);
    });
    faces.iter().zip(&original_faces).for_each(|(face, &id)| {
        assert_eq!(face.map(|v| origins[v as usize] as u32), source_faces[id])
    });
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        ..Default::default()
    };
    let started = Instant::now();
    let (output, report) = fit_and_repair((&rest, &faces), (&body, &body_faces), None, &options)
        .expect("fresh Rust fitting result, no precomputed candidate");
    let seconds = started.elapsed().as_secs_f64();
    assert_eq!(output.len(), rest.len());
    let normal = |p: &[[f32; 3]], f: [u32; 3]| {
        let [a, b, c] = f.map(|v| DVec3::from(p[v as usize].map(f64::from)));
        (b - a).cross(c - a)
    };
    let flips = faces
        .iter()
        .zip(&original_faces)
        .filter(|(face, id)| normal(&output, **face).dot(normal(&source, source_faces[**id])) < 0.0)
        .count();
    let minimum_area = faces
        .iter()
        .map(|&f| normal(&output, f).length())
        .fold(f64::INFINITY, f64::min);
    let source_self: std::collections::HashSet<_> = self_intersection_witnesses(&rest, &faces)
        .into_iter()
        .collect();
    let new_self = self_intersection_witnesses(&output, &faces)
        .iter()
        .filter(|w| !source_self.contains(w))
        .count();
    let measured_inside = inside((&output, &faces), (&body, &body_faces));
    let contacts = intersection_pairs(&output, &faces, &body, &body_faces);
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("original fixed frame");
    let overlap =
        compare((&source, &source_faces), (&output, &faces), &frame).expect("original silhouette");
    let mut representatives = vec![None; 324];
    mapping
        .iter()
        .enumerate()
        .for_each(|(v, &group)| match representatives[group] {
            None => representatives[group] = Some(output[v]),
            Some(p) => assert_eq!(p, output[v], "welded seam identity"),
        });
    let physical = SourceSurface::identify(&output, &faces).expect("physical output");
    let profile =
        ControlProfile::from_json(&std::fs::read(root.join("profile.json")).expect("profile"))
            .expect("valid profile");
    let topology = ControlDiagnostics::measure(physical.positions(), physical.triangles())
        .expect("fitted topology");
    let construction = topology.qualify(&profile);
    let strain = faces
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .map(|(a, b)| {
            let length = |p: &[[f32; 3]]| {
                DVec3::from(p[a as usize].map(f64::from))
                    .distance(DVec3::from(p[b as usize].map(f64::from)))
            };
            let old = length(&rest);
            let new = length(&output);
            ((new / old).max(old / new) - 1.0, [a, b], old, new)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("original retained edges");
    let measured = json!({"body_contacts":intersection_pairs(&output,&faces,&body,&body_faces[..6516]),"feet_contacts":intersection_pairs(&output,&faces,&body,&body_faces[6516..]),
        "inside":measured_inside,"new_self":new_self,"source_flips":flips,"original_silhouette_iou":overlap.iou,"maximum_source_symmetric_strain":strain.0,
        "original_strain_edge":strain.1.map(|v|origins[v as usize]),"source_edge_length":strain.2,"output_edge_length":strain.3,
        "minimum_double_triangle_area":minimum_area,"fitted_construction":format!("{construction:?}"),"fit_seconds":seconds,"report":report,
        "ordered_prepared_topology":true,"exact_original_uv_corners":true,"vertices":output.len(),"faces":faces.len()});
    eprintln!("BOUNDED_RUST_REPAIR {measured}");
    if let Some(path) = std::env::var_os("SCULPTO_REPAIR_OUT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"positions":output,"triangles":faces,"measured":measured}),
            )
            .expect("result JSON"),
        )
        .expect("requested diagnostic artifact");
    }
    assert_eq!(report.intersection_pairs, contacts);
    assert_eq!(report.inside_after, measured_inside);
    assert_eq!((contacts, measured_inside, new_self, flips), (0, 0, 0, 0));
    assert!(minimum_area > 0.0 && construction.is_ok());
    assert!(overlap.iou >= 0.888_341_722_214_664_7 && seconds < 60.0);
}
