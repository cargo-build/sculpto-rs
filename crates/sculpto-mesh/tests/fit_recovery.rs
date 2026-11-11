//! Complete collision assemblies retain garment construction during repair. @req DECL0013

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use glam::Vec3;
use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::repair::fit_and_repair;
use sculpto_mesh::wrap::{FitOptions, GapMode, fit_to_body};
use sculpto_nif::NifDocument;

fn read(path: &Path) -> NifDocument {
    let bytes = std::fs::read(path).expect("configured fixture is readable");
    sculpto_nif::read(&bytes).expect("configured fixture is a supported NIF")
}

fn placed(document: &NifDocument, index: usize) -> Vec<[f32; 3]> {
    let placement = document.shape_placement(index).expect("shape placement");
    document.shapes[index]
        .positions
        .iter()
        .map(|&point| placement.position_in_source(point))
        .collect()
}

#[test]
fn configured_pants_clear_complete_reference_without_changing_construction() {
    let (Some(mods), Some(rigs)) = (
        std::env::var_os("SCULPTO_MOD_FIXTURE"),
        std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES"),
    ) else {
        eprintln!("SKIP real pants: SCULPTO_MOD_FIXTURE or SCULPTO_RIG_TEMPLATE_FIXTURES unset");
        return;
    };
    let mods = Path::new(&mods);
    let garment = read(&mods.join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif"));
    let body = read(&Path::new(&rigs).join("bijin.nif"));
    let feet = read(&mods.join("bijin-warmaidens/Bijin Warmaidens v3.1.3/03 CBBE/meshes/actors/character/Bijin Warmaidens/femalefeet_1.nif"));
    let garment_index = garment
        .shapes
        .iter()
        .position(|s| s.name == "pants")
        .expect("pants");
    let body_index = body
        .shapes
        .iter()
        .position(|s| s.name == "BaseShape")
        .expect("Bijin body");
    let source = placed(&garment, garment_index);
    let triangles = &garment.shapes[garment_index].triangles;
    assert_eq!((source.len(), triangles.len()), (392, 632));
    let mut reference = placed(&body, body_index);
    let mut reference_faces = body.shapes[body_index].triangles.clone();
    assert_eq!((reference.len(), reference_faces.len()), (3471, 6516));
    feet.shapes.iter().enumerate().for_each(|(index, shape)| {
        let offset = u32::try_from(reference.len()).expect("fixture vertex width");
        reference.extend(placed(&feet, index));
        reference_faces.extend(shape.triangles.iter().map(|face| face.map(|v| v + offset)));
    });
    assert_eq!((reference.len(), reference_faces.len()), (5825, 10488));
    assert!(self_intersection_witnesses(&source, triangles).is_empty());
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        ..FitOptions::default()
    };
    let start = Instant::now();
    let (output, report) = fit_and_repair(
        (&source, triangles),
        (&reference, &reference_faces),
        None,
        &options,
    )
    .expect("valid repair input");
    let seconds = start.elapsed().as_secs_f64();
    let contacts = intersection_pairs(&output, triangles, &reference, &reference_faces);
    let self_contacts = self_intersection_witnesses(&output, triangles);
    let locks = VertexMask::from_values(vec![1.0; output.len()]).expect("measurement lock");
    let (unchanged, diagnostic) = fit_to_body(
        (&output, triangles),
        (&reference, &reference_faces),
        Some(&locks),
        &options,
    )
    .expect("locked measurement");
    assert_eq!(unchanged, output);
    let normal = |points: &[[f32; 3]], face: &[u32; 3]| {
        let [a, b, c] = face.map(|vertex| Vec3::from(points[vertex as usize]));
        (b - a).cross(c - a)
    };
    let flipped = triangles
        .iter()
        .filter(|face| normal(&source, face).dot(normal(&output, face)) < 0.0)
        .count();
    let edges: BTreeSet<_> = triangles
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)].map(|(a, b)| (a.min(b), a.max(b))))
        .collect();
    let (strain, strain_edge, old_length, new_length) = edges
        .iter()
        .map(|&(a, b)| {
            let distance =
                |p: &[[f32; 3]]| Vec3::from(p[a as usize]).distance(Vec3::from(p[b as usize]));
            let old = distance(&source);
            let new = distance(&output);
            let ratio = new / old;
            (ratio.max(1.0 / ratio) - 1.0, [a, b], old, new)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("pants have edges");
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("fixed frame");
    let overlap = compare((&source, triangles), (&output, triangles), &frame)
        .expect("shared silhouette frame");
    let measured = serde_json::json!({
        "contacts":contacts, "inside":diagnostic.inside_after, "self":self_contacts.len(),
        "flipped":flipped, "silhouette_iou":overlap.iou, "max_symmetric_strain":strain,
        "strain_witness":{"edge":strain_edge,"source_length":old_length,"returned_length":new_length,
            "source_points":strain_edge.map(|v|source[v as usize]),"returned_points":strain_edge.map(|v|output[v as usize])},
        "body_contacts":intersection_pairs(&output, triangles, &reference, &reference_faces[..6516]),
        "feet_contacts":intersection_pairs(&output, triangles, &reference, &reference_faces[6516..]),
        "seconds":seconds, "report":report,
    });
    eprintln!("FIT_RECOVERY {measured}");
    if let Some(path) = std::env::var_os("SCULPTO_FIT_RECOVERY_OUT") {
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "source":source,"output":output,"triangles":triangles,"measured":measured,
            }))
            .expect("diagnostic JSON"),
        )
        .expect("write requested diagnostic artifact");
    }
    assert_eq!(output.len(), source.len());
    assert_eq!(report.intersection_pairs, contacts);
    assert_eq!(report.inside_after, diagnostic.inside_after);
    assert_eq!(
        (
            contacts,
            diagnostic.inside_after,
            self_contacts.len(),
            flipped
        ),
        (0, 0, 0, 0)
    );
    assert!(
        overlap.iou >= 0.888_341_722_214_664_7,
        "source silhouette: {}",
        overlap.iou
    );
    assert!(seconds < 60.0, "repair took {seconds}s");
}
