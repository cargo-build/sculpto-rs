//! Recover only an eligible original source disk before direct render fitting. @req DECL0013

use std::collections::HashSet;
use std::path::Path;

use glam::DVec3;
use sculpto_mesh::check::{intersection_pairs, nearest_distances, self_intersection_witnesses};
use sculpto_mesh::control::measure::ControlDiagnostics;
use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{compact, source_selection};
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use serde_json::json;

#[test]
fn configured_recovered_native_render_meets_original_contract() {
    let Some(root) = std::env::var_os("SCULPTO_DISK_NATIVE_FIXTURE") else {
        eprintln!("SKIP recovered native fit: SCULPTO_DISK_NATIVE_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |p: &Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(p).expect("configured native artifact"))
            .expect("native artifact JSON")
    };
    let original = read(&root.join("original.json"));
    let prepared = read(
        &root
            .parent()
            .expect("prepared fixture parent")
            .join("prepared-render.json"),
    );
    let candidate = read(&root.join("candidate.json"));
    let source: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("original positions");
    let source_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("original faces");
    let source_uvs: Vec<[f32; 2]> =
        serde_json::from_value(original["uvs"].clone()).expect("original UVs");
    let rest: Vec<[f32; 3]> =
        serde_json::from_value(prepared["positions"].clone()).expect("prepared rest");
    let faces: Vec<[u32; 3]> =
        serde_json::from_value(prepared["triangles"].clone()).expect("prepared faces");
    let output: Vec<[f32; 3]> =
        serde_json::from_value(candidate["positions"].clone()).expect("native positions");
    let origins: Vec<u32> = serde_json::from_value(prepared["render_source_vertices"].clone())
        .expect("source vertices");
    let original_faces: Vec<u32> =
        serde_json::from_value(prepared["source_faces"].clone()).expect("source face IDs");
    let prepared_uvs: Vec<[f32; 2]> =
        serde_json::from_value(prepared["uvs"].clone()).expect("retained UVs");
    let mapping: Vec<usize> =
        serde_json::from_value(prepared["render_to_physical"].clone()).expect("physical mapping");
    let body: Vec<[f32; 3]> = serde_json::from_value(original["reference"]["positions"].clone())
        .expect("actual body and feet");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("actual full reference faces");
    assert_eq!(
        (
            source.len(),
            source_faces.len(),
            rest.len(),
            faces.len(),
            output.len()
        ),
        (392, 632, 370, 612, 370)
    );
    assert_eq!((body.len(), body_faces.len()), (5825, 10488));
    let native =
        std::fs::read_to_string(root.join("native-physical.obj")).expect("actual saved topology");
    let native_faces: Vec<[usize; 3]> = native
        .lines()
        .filter(|l| l.starts_with("f "))
        .map(|line| {
            let values: Vec<usize> = line
                .split_whitespace()
                .skip(1)
                .map(|v| {
                    v.split('/')
                        .next()
                        .expect("OBJ vertex")
                        .parse::<usize>()
                        .expect("integer")
                        - 1
                })
                .collect();
            values.try_into().expect("triangles")
        })
        .collect();
    assert_eq!(
        native_faces,
        faces
            .iter()
            .map(|face| face.map(|v| mapping[v as usize]))
            .collect::<Vec<_>>(),
        "actual native ordered topology"
    );
    let mut representatives = vec![None; 324];
    mapping
        .iter()
        .enumerate()
        .for_each(|(v, &physical)| match representatives[physical] {
            None => representatives[physical] = Some(output[v]),
            Some(p) => assert_eq!(p, output[v], "welded UV seam positions"),
        });
    let physical_output: Vec<_> = representatives
        .into_iter()
        .map(|p| p.expect("every prepared physical vertex is retained"))
        .collect();
    let physical_faces: Vec<_> = native_faces
        .iter()
        .map(|face| face.map(|v| u32::try_from(v).expect("physical index width")))
        .collect();
    let profile = ControlProfile::from_json(
        &std::fs::read(root.parent().expect("fixture parent").join("profile.json"))
            .expect("source profile"),
    )
    .expect("valid source profile");
    let final_topology = ControlDiagnostics::measure(&physical_output, &physical_faces)
        .expect("fitted physical topology");
    let final_construction = final_topology.qualify(&profile);
    origins.iter().enumerate().for_each(|(v, &source_vertex)| {
        assert_eq!(rest[v], source[source_vertex as usize]);
        assert_eq!(prepared_uvs[v], source_uvs[source_vertex as usize]);
    });
    let normal = |p: &[[f32; 3]], face: [u32; 3]| {
        let [a, b, c] = face.map(|v| DVec3::from(p[v as usize].map(f64::from)));
        (b - a).cross(c - a)
    };
    let flips = faces
        .iter()
        .zip(&original_faces)
        .filter(|(face, id)| {
            normal(&output, **face).dot(normal(&source, source_faces[**id as usize])) < 0.0
        })
        .count();
    let minimum_double_area = faces
        .iter()
        .map(|&face| normal(&output, face).length())
        .fold(f64::INFINITY, f64::min);
    faces.iter().zip(&original_faces).for_each(|(face, &id)| {
        assert_eq!(
            face.map(|v| origins[v as usize]),
            source_faces[id as usize],
            "original face/corner provenance"
        )
    });
    let self_contacts = self_intersection_witnesses(&output, &faces);
    assert!(self_intersection_witnesses(&source, &source_faces).is_empty());
    let radius = 10.0_f32.max(
        nearest_distances(&output, &body, &body_faces)
            .into_iter()
            .fold(0.0_f32, f32::max)
            + 0.05,
    );
    let locks = sculpto_mesh::mask::VertexMask::from_values(vec![1.0; output.len()])
        .expect("measurement locks");
    let (unchanged, inside) = sculpto_mesh::wrap::fit_to_body(
        (&output, &faces),
        (&body, &body_faces),
        Some(&locks),
        &sculpto_mesh::wrap::FitOptions {
            max_distance: radius,
            ..Default::default()
        },
    )
    .expect("locked inside diagnostic");
    assert_eq!(unchanged, output);
    let contacts = intersection_pairs(&output, &faces, &body, &body_faces);
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
        .expect("source retained edges");
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("original fixed frame");
    let silhouette = compare((&source, &source_faces), (&output, &faces), &frame);
    let mesh = |name: &str| -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let value = read(&root.join(name));
        (
            serde_json::from_value(value["positions"].clone()).expect("avatar positions"),
            serde_json::from_value(value["triangles"].clone()).expect("avatar faces"),
        )
    };
    let (avatar, avatar_faces) = mesh("avatar.json");
    let (target, target_faces) = mesh("target.json");
    let paired_error = nearest_distances(&avatar, &target, &target_faces)
        .into_iter()
        .fold(0.0_f32, f32::max);
    let reverse_error = nearest_distances(&target, &avatar, &avatar_faces)
        .into_iter()
        .fold(0.0_f32, f32::max);
    let bounds = [
        output
            .iter()
            .map(|p| DVec3::from(p.map(f64::from)))
            .fold(DVec3::splat(f64::INFINITY), DVec3::min)
            .to_array(),
        output
            .iter()
            .map(|p| DVec3::from(p.map(f64::from)))
            .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max)
            .to_array(),
    ];
    let measured = json!({"prepared_vertices":rest.len(),"returned_vertices":output.len(),"prepared_faces":faces.len(),"returned_faces":native_faces.len(),"ordered_prepared_topology":true,
        "exact_source_uv_corners":true,"body_contacts":intersection_pairs(&output,&faces,&body,&body_faces[..6516]),"feet_contacts":intersection_pairs(&output,&faces,&body,&body_faces[6516..]),
        "fitted_construction":format!("{final_construction:?}"),"minimum_double_triangle_area":minimum_double_area,
        "contacts":contacts,"inside":inside.inside_after,"new_self":self_contacts.len(),"source_flips":flips,"original_silhouette":match &silhouette {Ok(s)=>json!({"iou":s.iou}),Err(e)=>json!({"error":e.to_string()})},
        "maximum_source_symmetric_strain":strain.0,"prepared_strain_edge":strain.1,"original_strain_edge":strain.1.map(|v|origins[v as usize]),"source_edge_length":strain.2,"returned_edge_length":strain.3,
        "source_edge_points":strain.1.map(|v|rest[v as usize]),"returned_edge_points":strain.1.map(|v|output[v as usize]),"bounds":bounds,"paired_avatar_to_target_max":paired_error,"target_to_paired_avatar_max":reverse_error});
    std::fs::write(
        root.join("qualification.json"),
        serde_json::to_vec_pretty(&measured).expect("qualification JSON"),
    )
    .expect("qualification artifact");
    eprintln!("RECOVERED_NATIVE {measured}");
    assert!(
        paired_error < 1e-4 && reverse_error < 1e-4,
        "paired native avatar agrees with recorded target"
    );
    assert!(
        final_construction.is_ok(),
        "fitted natural openings and physical construction"
    );
    assert!(minimum_double_area > 0.0, "no collapsed fitted triangle");
    assert_eq!(
        (contacts, inside.inside_after, self_contacts.len(), flips),
        (0, 0, 0, 0)
    );
    assert!(silhouette.expect("fixed original frame").iou >= 0.888_341_722_214_664_7);
}

#[test]
fn configured_incidental_disk_recovery_preserves_source_render() {
    let Some(root) = std::env::var_os("SCULPTO_DISK_RECOVERY_FIXTURE") else {
        eprintln!("SKIP source disk recovery: SCULPTO_DISK_RECOVERY_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let original: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("original.json")).expect("original fixture"),
    )
    .expect("original JSON");
    let points: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("source points");
    let triangles: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("source faces");
    let uvs: Vec<[f32; 2]> = serde_json::from_value(original["uvs"].clone()).expect("source UVs");
    let body: Vec<[f32; 3]> = serde_json::from_value(original["reference"]["positions"].clone())
        .expect("actual reference");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("reference faces");
    let profile = ControlProfile::from_json(
        &std::fs::read(root.join("profile.json")).expect("declared profile"),
    )
    .expect("valid profile");
    let selection = source_selection(&points, &triangles, &profile)
        .expect("existing source selection and disk recovery owner");
    let (physical, physical_faces, _) =
        compact(selection.surface.positions(), &selection.selected_triangles);
    let retained: Vec<_> = selection
        .selected_faces
        .iter()
        .map(|&v| triangles[v as usize])
        .collect();
    let (render, render_faces, origins) = compact(&points, &retained);
    let render_uvs: Vec<_> = origins.iter().map(|&v| uvs[v as usize]).collect();
    let render_to_physical: Vec<_> = render
        .iter()
        .map(|p| {
            physical
                .iter()
                .position(|q| p == q)
                .expect("exact original physical identity")
        })
        .collect();
    assert_eq!(
        render_faces
            .iter()
            .map(|face| face.map(|v| render_to_physical[v as usize] as u32))
            .collect::<Vec<_>>(),
        physical_faces
    );
    render_faces
        .iter()
        .zip(&selection.selected_faces)
        .for_each(|(face, &source_face)| {
            assert_eq!(
                face.map(|v| origins[v as usize]),
                triangles[source_face as usize],
                "original corner order"
            );
            assert_eq!(
                face.map(|v| render_uvs[v as usize]),
                triangles[source_face as usize].map(|v| uvs[v as usize]),
                "original UV corner identity"
            );
        });
    let kept: HashSet<_> = selection.selected_faces.iter().copied().collect();
    let removed: Vec<_> = (0..triangles.len() as u32)
        .filter(|face| !kept.contains(face))
        .collect();
    let area = |face: [u32; 3]| {
        let [a, b, c] = face.map(|v| DVec3::from(points[v as usize].map(f64::from)));
        0.5 * (b - a).cross(c - a).length()
    };
    let source_area: f64 = triangles.iter().map(|&face| area(face)).sum();
    let removed_area: f64 = removed
        .iter()
        .map(|&face| area(triangles[face as usize]))
        .sum();
    let topology =
        ControlDiagnostics::measure(&physical, &physical_faces).expect("physical topology");
    let qualified = topology.qualify(&profile);
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("unchanged original frame");
    let iou = compare((&points, &triangles), (&render, &render_faces), &frame)
        .expect("original silhouette")
        .iou;
    let measured = json!({"source_vertices":points.len(),"source_faces":triangles.len(),"physical_vertices":physical.len(),"prepared_vertices":render.len(),"prepared_faces":render_faces.len(),
        "recovered_faces":selection.recovery.recovered_faces,"recovered_disks":selection.recovery.disks.iter().map(|d|json!({"faces":d.faces,"loop_index":d.loop_index,"boundary_vertices":d.boundary_vertices,"vertices":d.vertices,"edges":d.edges,"euler":d.euler})).collect::<Vec<_>>(),
        "recovery_refusals":format!("{:?}",selection.recovery.refused),"removed_faces":removed,"original_area":source_area,"removed_area":removed_area,"removed_area_share":removed_area/source_area,
        "source_faces_moved":0,"source_uv_corner_error":0,"prepared_original_silhouette_iou":iou,"components":topology.components,"euler":topology.euler,
        "non_manifold_vertices":topology.non_manifold_vertices,"non_manifold_edges":topology.non_manifold_edges,"orientation_conflicts":topology.orientation_conflicts,"self_contacts":topology.self_contacts,
        "loops":topology.loops.iter().map(|l|json!({"center":l.center,"edges":l.edges,"perimeter":l.perimeter,"closed":l.closed})).collect::<Vec<_>>(),
        "topology_qualification":format!("{qualified:?}"),"body_contacts":intersection_pairs(&render,&render_faces,&body,&body_faces[..6516]),"feet_contacts":intersection_pairs(&render,&render_faces,&body,&body_faces[6516..])});
    std::fs::write(root.join("prepared-render.json"),serde_json::to_vec_pretty(&json!({"positions":render,"triangles":render_faces,"uvs":render_uvs,
        "render_source_vertices":origins,"source_faces":selection.selected_faces,"physical_positions":physical,"physical_triangles":physical_faces,"render_to_physical":render_to_physical,"measured":measured,
        "material_reference":"immutable original NIF and exact retained/recovered source face IDs; no material rewrite",
        "source_nif_sha256":"b7dee8dda3a9c5642e262569f0eae398f1082d193a3de6351b49db8c1f2e7bc9"})).expect("prepared JSON")).expect("prepared artifact");
    eprintln!(
        "SOURCE_DISK_RECOVERY recovered={} refusals={} vertices={} faces={} loops={} self={} iou={iou} qualification={qualified:?}",
        selection.recovery.recovered_faces,
        selection.recovery.refused.len(),
        render.len(),
        render_faces.len(),
        topology.loops.len(),
        topology.self_contacts
    );
    assert_eq!(
        selection.recovery.recovered_faces, 1,
        "only the bounded eligible original disk"
    );
    assert!(selection.recovery.refused.is_empty());
    assert!(
        qualified.is_ok(),
        "three named openings and intact construction"
    );
    assert!(iou >= 0.888_341_722_214_664_7);
}
