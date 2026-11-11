//! A declared intake derivative is measured against the immutable rendered source. @req DECL0013

use std::path::Path;
use std::time::Instant;

use glam::DVec3;
use sculpto_mesh::check::{
    intersection_pair_list, intersection_pairs, self_intersection_witnesses,
};
use sculpto_mesh::control::measure::ControlDiagnostics;
use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{SourceSurface, boundary_loops};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::repair::fit_and_repair;
use sculpto_mesh::wrap::{FitOptions, GapMode, fit_to_body};
use serde::Deserialize;
use serde_json::json;

fn document(path: &Path) -> sculpto_nif::NifDocument {
    sculpto_nif::read(&std::fs::read(path).expect("configured NIF is readable"))
        .expect("configured NIF is supported")
}

#[derive(Deserialize)]
struct Geometry {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
}

struct Original {
    mesh: Geometry,
    uvs: Vec<[f64; 2]>,
    reference: Geometry,
}

#[derive(Deserialize)]
struct Corner {
    source_face: usize,
    source_corners: [u32; 3],
    weights: [f64; 3],
}

struct Prepared {
    mesh: Geometry,
    uvs: Vec<[f64; 2]>,
    corner_lineage: Vec<[Corner; 3]>,
    profile: serde_json::Value,
}

#[test]
fn configured_prepared_pants_keep_original_silhouette_and_clear_complete_reference() {
    let Some(path) = std::env::var_os("SCULPTO_SOURCE_PREPARATION_INPUT") else {
        eprintln!("SKIP preparation proposal: SCULPTO_SOURCE_PREPARATION_INPUT unset");
        return;
    };
    let path = Path::new(&path);
    let original_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("input.json")).expect("original input"))
            .expect("input JSON");
    let original = Original {
        mesh: serde_json::from_value(original_json.clone()).expect("original mesh"),
        uvs: serde_json::from_value(original_json["uvs"].clone()).expect("original UVs"),
        reference: serde_json::from_value(original_json["reference"].clone()).expect("reference"),
    };
    let prepared_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path.join("attachment-cut02/prepared.json"))
            .expect("declared prepared input"),
    )
    .expect("prepared JSON");
    let prepared = Prepared {
        mesh: serde_json::from_value(prepared_json.clone()).expect("prepared mesh"),
        uvs: serde_json::from_value(prepared_json["uvs"].clone()).expect("prepared UVs"),
        corner_lineage: serde_json::from_value(prepared_json["corner_lineage"].clone())
            .expect("corner lineage"),
        profile: prepared_json["profile"].clone(),
    };
    let source = &original.mesh;
    let garment = &prepared.mesh;
    let body = &original.reference;
    assert_eq!((source.positions.len(), source.triangles.len()), (392, 632));
    assert_eq!((body.positions.len(), body.triangles.len()), (5825, 10488));
    assert_eq!(prepared.corner_lineage.len(), garment.triangles.len());
    assert_eq!(prepared.uvs.len(), garment.positions.len());
    let normal = |positions: &[[f32; 3]], face: [u32; 3]| {
        let [a, b, c] = face.map(|v| DVec3::from(positions[v as usize].map(f64::from)));
        (b - a).cross(c - a)
    };
    let mut max_uv_error = 0.0_f64;
    let mut max_geometry_error = 0.0_f64;
    garment
        .triangles
        .iter()
        .zip(&prepared.corner_lineage)
        .for_each(|(face, lineage)| {
            face.iter().zip(lineage).for_each(|(&vertex, corner)| {
                assert_eq!(corner.source_corners, source.triangles[corner.source_face]);
                assert!((corner.weights.iter().sum::<f64>() - 1.0).abs() < 1e-10);
                let point: [f64; 3] = std::array::from_fn(|axis| {
                    (0..3)
                        .map(|c| {
                            corner.weights[c]
                                * f64::from(
                                    source.positions[corner.source_corners[c] as usize][axis],
                                )
                        })
                        .sum()
                });
                let uv: [f64; 2] = std::array::from_fn(|axis| {
                    (0..3)
                        .map(|c| {
                            corner.weights[c]
                                * original.uvs[corner.source_corners[c] as usize][axis]
                        })
                        .sum()
                });
                max_geometry_error = max_geometry_error.max(DVec3::from(point).distance(
                    DVec3::from(garment.positions[vertex as usize].map(f64::from)),
                ));
                uv.iter()
                    .zip(prepared.uvs[vertex as usize])
                    .for_each(|(a, b)| max_uv_error = max_uv_error.max((a - b).abs()));
            });
            assert!(
                normal(&garment.positions, *face).dot(normal(
                    &source.positions,
                    source.triangles[lineage[0].source_face]
                )) > 0.0,
                "cut preserves source orientation"
            );
        });
    assert!(max_geometry_error < 1e-4 && max_uv_error < 1e-12);
    let profile =
        ControlProfile::from_json(&serde_json::to_vec(&prepared.profile).expect("profile JSON"))
            .expect("valid profile");
    let physical = SourceSurface::identify(&garment.positions, &garment.triangles)
        .expect("prepared physical topology");
    let topology = ControlDiagnostics::measure(physical.positions(), physical.triangles())
        .expect("prepared control topology");
    let qualification = topology.qualify(&profile);
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("original fixed frame");
    let before_iou = compare(
        (&source.positions, &source.triangles),
        (&garment.positions, &garment.triangles),
        &frame,
    )
    .expect("prepared silhouette")
    .iou;
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        ..FitOptions::default()
    };
    let started = Instant::now();
    let (output, report) = fit_and_repair(
        (&garment.positions, &garment.triangles),
        (&body.positions, &body.triangles),
        None,
        &options,
    )
    .expect("existing fit");
    let seconds = started.elapsed().as_secs_f64();
    assert_eq!(output.len(), garment.positions.len());
    let locked = VertexMask::from_values(vec![1.0; output.len()]).expect("measurement locks");
    let (unchanged, diagnostic) = fit_to_body(
        (&output, &garment.triangles),
        (&body.positions, &body.triangles),
        Some(&locked),
        &options,
    )
    .expect("inside measurement");
    assert_eq!(unchanged, output);
    let contacts = intersection_pairs(
        &output,
        &garment.triangles,
        &body.positions,
        &body.triangles,
    );
    let self_contacts = self_intersection_witnesses(&output, &garment.triangles);
    let flips = garment
        .triangles
        .iter()
        .zip(&prepared.corner_lineage)
        .filter(|(face, lineage)| {
            normal(&output, **face).dot(normal(
                &source.positions,
                source.triangles[lineage[0].source_face],
            )) < 0.0
        })
        .count();
    let iou = compare(
        (&source.positions, &source.triangles),
        (&output, &garment.triangles),
        &frame,
    )
    .expect("original silhouette")
    .iou;
    let witnesses = intersection_pair_list(
        &output,
        &garment.triangles,
        &body.positions,
        &body.triangles,
    );
    let contact_faces: std::collections::BTreeSet<_> =
        witnesses.iter().map(|&(face, _)| face).collect();
    let centroid = |face: usize| {
        garment.triangles[face]
            .iter()
            .map(|&v| DVec3::from(output[v as usize].map(f64::from)))
            .sum::<DVec3>()
            / 3.0
    };
    let contact_bounds = match contact_faces.is_empty() {
        true => None,
        false => Some([
            contact_faces
                .iter()
                .map(|&face| centroid(face))
                .fold(DVec3::splat(f64::INFINITY), DVec3::min)
                .to_array(),
            contact_faces
                .iter()
                .map(|&face| centroid(face))
                .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max)
                .to_array(),
        ]),
    };
    let strain = garment
        .triangles
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .map(|(a, b)| {
            let length = |p: &[[f32; 3]]| {
                DVec3::from(p[a as usize].map(f64::from))
                    .distance(DVec3::from(p[b as usize].map(f64::from)))
            };
            let old = length(&garment.positions);
            let new = length(&output);
            ((new / old).max(old / new) - 1.0, [a, b], old, new)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("prepared edges");
    let measured = json!({"original_vertices":source.positions.len(),"original_faces":source.triangles.len(),
        "prepared_vertices":garment.positions.len(),"prepared_faces":garment.triangles.len(),
        "returned_vertices":output.len(),"returned_faces":garment.triangles.len(),
        "loops_before_fit":topology.loops.len(),"topology_qualification":format!("{qualification:?}"),
        "prepared_self":topology.self_contacts,"prepared_orientation_conflicts":topology.orientation_conflicts,
        "prepared_contacts":intersection_pairs(&garment.positions,&garment.triangles,&body.positions,&body.triangles),
        "contact_faces":contact_faces.len(),"contact_centroid_bounds":contact_bounds,
        "contact_witnesses":contact_faces.iter().take(8).map(|&face|json!({"prepared_face":face,
            "original_face":prepared.corner_lineage[face][0].source_face,"centroid":centroid(face).to_array()})).collect::<Vec<_>>(),
        "maximum_symmetric_strain":strain.0,"strain_edge":strain.1,"strain_source_fragment_length":strain.2,"strain_fitted_length":strain.3,
        "max_uv_lineage_error":max_uv_error,"max_geometry_lineage_error":max_geometry_error,
        "contacts":contacts,"body_contacts":intersection_pairs(&output,&garment.triangles,&body.positions,&body.triangles[..6516]),
        "feet_contacts":intersection_pairs(&output,&garment.triangles,&body.positions,&body.triangles[6516..]),
        "inside":diagnostic.inside_after,"self":self_contacts.len(),"source_flips":flips,
        "prepared_original_silhouette_iou":before_iou,"fitted_original_silhouette_iou":iou,"fit_seconds":seconds,"report":report});
    eprintln!("PREPARED_FIT08 {measured}");
    std::fs::write(
        path.join("attachment-cut02/fit-result.json"),
        serde_json::to_vec(
            &json!({"measured":measured,"output":output,"triangles":garment.triangles}),
        )
        .expect("result JSON"),
    )
    .expect("result artifact");
    assert!(
        qualification.is_ok(),
        "declared openings and manifold surface"
    );
    assert_eq!(
        (
            contacts,
            diagnostic.inside_after,
            self_contacts.len(),
            flips
        ),
        (0, 0, 0, 0)
    );
    assert!(iou >= 0.888_341_722_214_664_7, "original silhouette {iou}");
    assert!(seconds < 60.0);
}

fn placed(document: &sculpto_nif::NifDocument, index: usize) -> Vec<[f32; 3]> {
    let placement = document.shape_placement(index).expect("placement");
    document.shapes[index]
        .positions
        .iter()
        .map(|&p| placement.position_in_source(p))
        .collect()
}

fn loops(points: &[[f32; 3]], faces: &[[u32; 3]]) -> serde_json::Value {
    let mesh = SourceSurface::identify(points, faces).expect("physical topology");
    json!(
        boundary_loops(mesh.positions(), mesh.triangles())
            .iter()
            .map(|boundary| json!({
                "center":boundary.center,"closed":boundary.closed,"edges":boundary.edges,
                "perimeter":boundary.perimeter,"points":boundary.vertices.iter()
                    .map(|&v|mesh.positions()[v as usize]).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>()
    )
}

#[test]
fn export_source_preparation_inputs() {
    let Some(output) = std::env::var_os("SCULPTO_SOURCE_PREPARATION_EXPORT") else {
        eprintln!("SKIP preparation export: SCULPTO_SOURCE_PREPARATION_EXPORT unset");
        return;
    };
    let mods = std::env::var_os("SCULPTO_MOD_FIXTURE").expect("configured mod fixtures");
    let rigs = std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES").expect("configured rig fixtures");
    let mods = Path::new(&mods);
    let garment = document(&mods.join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif"));
    let body = document(&Path::new(&rigs).join("bijin.nif"));
    let feet = document(&mods.join("bijin-warmaidens/Bijin Warmaidens v3.1.3/03 CBBE/meshes/actors/character/Bijin Warmaidens/femalefeet_1.nif"));
    let gi = garment
        .shapes
        .iter()
        .position(|shape| shape.name == "pants")
        .expect("pants");
    let bi = body
        .shapes
        .iter()
        .position(|shape| shape.name == "BaseShape")
        .expect("body");
    let shape = &garment.shapes[gi];
    let positions = placed(&garment, gi);
    let mut reference = placed(&body, bi);
    let mut faces = body.shapes[bi].triangles.clone();
    let feet_boundaries: Vec<_> = feet
        .shapes
        .iter()
        .enumerate()
        .map(|(index, shape)| {
            let points = placed(&feet, index);
            let boundary = loops(&points, &shape.triangles);
            let offset = u32::try_from(reference.len()).expect("index width");
            reference.extend(points);
            faces.extend(shape.triangles.iter().map(|face| face.map(|v| v + offset)));
            boundary
        })
        .collect();
    assert_eq!(
        (positions.len(), shape.triangles.len(), shape.uvs.len()),
        (392, 632, 392)
    );
    assert_eq!((reference.len(), faces.len()), (5825, 10488));
    let physical =
        SourceSurface::identify(&positions, &shape.triangles).expect("source physical topology");
    let result = json!({
        "positions":positions,"triangles":shape.triangles,"uvs":shape.uvs,
        "appearance_source":"original digest-pinned NIF; material/partition metadata stays source-bound",
        "source_boundaries":loops(&positions,&shape.triangles),"feet_boundaries":feet_boundaries,
        "reference":{"positions":reference,"triangles":faces},
        "physical_source":{"positions":physical.positions(),"triangles":physical.triangles(),
            "vertex_sources":physical.vertex_sources()},
    });
    std::fs::write(output, serde_json::to_vec(&result).expect("JSON"))
        .expect("write requested fixture export");
}
