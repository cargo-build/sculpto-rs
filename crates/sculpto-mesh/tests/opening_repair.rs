//! Opt-in opening repair is checked on freshly fitted, placed source geometry. @req DECL0013

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use glam::DVec3;
use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::control::{ControlDiagnostics, SourceSurface};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::repair::fit_and_repair;
use sculpto_mesh::wrap::{FitOptions, GapMode, fit_to_body};
use serde_json::json;

fn read(path: &Path) -> sculpto_nif::NifDocument {
    sculpto_nif::read(&std::fs::read(path).expect("configured source NIF"))
        .expect("supported source NIF")
}

fn placed(document: &sculpto_nif::NifDocument, index: usize) -> Vec<[f32; 3]> {
    let placement = document.shape_placement(index).expect("source placement");
    document.shapes[index]
        .positions
        .iter()
        .map(|&p| placement.position_in_source(p))
        .collect()
}

fn maximum_strain(
    before: &[[f32; 3]],
    after: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> (f64, [u32; 2], f64, f64) {
    faces
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .map(|(a, b)| {
            let length = |p: &[[f32; 3]]| {
                DVec3::from(p[a as usize].map(f64::from))
                    .distance(DVec3::from(p[b as usize].map(f64::from)))
            };
            let old = length(before);
            let new = length(after);
            ((new / old).max(old / new) - 1.0, [a, b], old, new)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("fixture has retained edges")
}

#[test]
fn configured_sentinel_openings_clear_actual_body_and_feet() {
    let (Some(fit), Some(mods)) = (
        std::env::var_os("SCULPTO_FIT_FIXTURE"),
        std::env::var_os("SCULPTO_MOD_FIXTURE"),
    ) else {
        eprintln!("SKIP Sentinel opening repair: SCULPTO_FIT_FIXTURE or SCULPTO_MOD_FIXTURE unset");
        return;
    };
    let garment = read(&Path::new(&fit).join("tunic.nif"));
    let body = read(&Path::new(&fit).join("body.nif"));
    let feet = read(
        &Path::new(&mods)
            .join("himbo-v5-core/CalienteTools/BodySlide/ShapeData/HIMBO Res/HIMBO Feet.nif"),
    );
    let gi = garment
        .shapes
        .iter()
        .position(|s| s.name == "Armor001_1")
        .expect("pinned ordinary auxiliary garment");
    let bi = body
        .shapes
        .iter()
        .position(|s| s.name == "HIMBO - Body")
        .expect("accepted body");
    let source = placed(&garment, gi);
    let faces = &garment.shapes[gi].triangles;
    assert_eq!((source.len(), faces.len()), (702, 1200));
    let mut reference = placed(&body, bi);
    let mut reference_faces = body.shapes[bi].triangles.clone();
    assert_eq!((reference.len(), reference_faces.len()), (9346, 18074));
    feet.shapes.iter().enumerate().for_each(|(index, shape)| {
        let offset = reference.len() as u32;
        reference.extend(placed(&feet, index));
        reference_faces.extend(shape.triangles.iter().map(|f| f.map(|v| v + offset)));
    });
    assert_eq!((reference.len(), reference_faces.len()), (10630, 20318));
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        ..Default::default()
    };
    assert_eq!(options.opening_direction, None);
    let (ordinary, before) = fit_and_repair(
        (&source, faces),
        (&reference, &reference_faces),
        None,
        &options,
    )
    .expect("unchanged ordinary authoring repair");
    assert!(
        before.intersection_pairs > 0,
        "negative opt-out must retain a witnessed contact"
    );
    assert_eq!(before.inside_after, 0);
    let enabled = FitOptions {
        opening_direction: Some([0.0, 0.0, 7.0]),
        ..options
    };
    let started = Instant::now();
    let (output, report) = fit_and_repair(
        (&source, faces),
        (&reference, &reference_faces),
        None,
        &enabled,
    )
    .expect("fresh opt-in fit from original NIF");
    let seconds = started.elapsed().as_secs_f64();
    assert_eq!(output.len(), source.len());
    output.iter().zip(&ordinary).for_each(|(p, b)| {
        assert_eq!(p[0], b[0]);
        assert_eq!(p[1], b[1]);
    });
    let source_topology =
        SourceSurface::identify(&source, faces).expect("source physical topology");
    source.iter().enumerate().for_each(|(v, p)| {
        let group = source_topology
            .positions()
            .iter()
            .position(|q| q == p)
            .expect("referenced source vertex");
        assert_eq!(
            output[v],
            output[source_topology.vertex_sources()[group] as usize],
            "exact source seam displacement"
        );
    });
    let normal = |p: &[[f32; 3]], face: [u32; 3]| {
        let [a, b, c] = face.map(|v| DVec3::from(p[v as usize].map(f64::from)));
        (b - a).cross(c - a)
    };
    let flips = faces
        .iter()
        .filter(|&&f| normal(&source, f).dot(normal(&output, f)) <= 0.0)
        .count();
    let minimum_area = faces
        .iter()
        .map(|&f| normal(&output, f).length())
        .fold(f64::INFINITY, f64::min);
    let original_self: HashSet<_> = self_intersection_witnesses(&source, faces)
        .into_iter()
        .collect();
    let new_self = self_intersection_witnesses(&output, faces)
        .iter()
        .filter(|w| !original_self.contains(w))
        .count();
    let locks = VertexMask::from_values(vec![1.0; output.len()]).expect("measurement locks");
    let (same, inside) = fit_to_body(
        (&output, faces),
        (&reference, &reference_faces),
        Some(&locks),
        &FitOptions::default(),
    )
    .expect("independent inside query");
    assert_eq!(same, output);
    let body_pairs = intersection_pairs(&output, faces, &reference, &reference_faces[..18074]);
    let feet_pairs = intersection_pairs(&output, faces, &reference, &reference_faces[18074..]);
    let frame = OrthographicFrame::new([[-31.0, 31.0], [7.0, 118.0]], RasterSettings::default())
        .expect("original fixed Sentinel frame");
    let overlap = compare((&source, faces), (&output, faces), &frame)
        .expect("unchanged original silhouette frame");
    let added = maximum_strain(&ordinary, &output, faces);
    let original = maximum_strain(&source, &output, faces);
    let physical = SourceSurface::identify(&output, faces).expect("retained physical topology");
    let topology = ControlDiagnostics::measure(physical.positions(), physical.triangles())
        .expect("fitted construction");
    let measured = json!({"vertices":output.len(),"faces":faces.len(),"before":before,"report":report,"body_pairs":body_pairs,"feet_pairs":feet_pairs,
        "inside":inside.inside_after,"new_self":new_self,"original_flips":flips,"minimum_double_area":minimum_area,"source_silhouette_iou":overlap.iou,
        "additional_symmetric_strain":added.0,"additional_strain_edge":added.1,"incoming_length":added.2,"output_length":added.3,"original_symmetric_strain":original.0,
        "loops":topology.loops.len(),"genus":topology.measured_genus(),"fit_seconds":seconds,
        "max_opening_move":output.iter().zip(&ordinary).map(|(p,b)|p[2]-b[2]).fold(0.0_f32,f32::max)});
    eprintln!("OPENING_REPAIR {measured}");
    if let Some(path) = std::env::var_os("SCULPTO_OPENING_OUT") {
        std::fs::write(path,serde_json::to_vec_pretty(&json!({"source":source,"ordinary":ordinary,"positions":output,"triangles":faces,"measured":measured})).expect("JSON")).expect("requested diagnostic artifact");
    }
    assert_eq!(
        (body_pairs, feet_pairs, inside.inside_after, new_self, flips),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(report.intersection_pairs, body_pairs + feet_pairs);
    assert_eq!(report.inside_after, inside.inside_after);
    assert!(minimum_area > 0.0 && added.0 <= f64::from(enabled.max_strain));
    assert!(overlap.iou >= 0.904_066_017_564_630_5 && seconds < 60.0);
    assert_eq!(topology.loops.len(), 3);
    assert_eq!(topology.measured_genus(), Some(0));
}
