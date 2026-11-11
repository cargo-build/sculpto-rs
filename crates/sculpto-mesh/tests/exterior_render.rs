//! Source-guided preparation carries retained render corners directly. @req DECL0013

use std::collections::HashSet;
use std::path::Path;

use glam::DVec3;
use sculpto_mesh::check::intersection_pairs;
use sculpto_mesh::control::measure::ControlDiagnostics;
use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{Guide, GuideSegment, SourceSurface, compact, exterior_triangles};
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use serde_json::json;

#[test]
fn configured_exterior_render_preserves_natural_source_construction() {
    let Some(root) = std::env::var_os("SCULPTO_EXTERIOR_RENDER_FIXTURE") else {
        eprintln!("SKIP exterior render: SCULPTO_EXTERIOR_RENDER_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root.join(name)).expect("configured artifact"))
            .expect("artifact JSON")
    };
    let original = read("original.json");
    let previous = read("previous-control.json");
    let points: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("source points");
    let triangles: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("source faces");
    let uvs: Vec<[f32; 2]> = serde_json::from_value(original["uvs"].clone()).expect("source UVs");
    let body: Vec<[f32; 3]> = serde_json::from_value(original["reference"]["positions"].clone())
        .expect("actual body and feet");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("full reference faces");
    let old_segments: Vec<[[f32; 3]; 2]> =
        serde_json::from_value(previous["guide"].clone()).expect("saved guide");
    let old_guide = Guide::new(
        old_segments
            .into_iter()
            .map(|[start, end]| GuideSegment { start, end }),
    )
    .expect("declared old guide");
    let profile = ControlProfile::from_json(
        &std::fs::read(root.join("source-profile-proposal.json")).expect("source profile"),
    )
    .expect("strict profile");
    let physical = SourceSurface::identify(&points, &triangles).expect("physical source");
    let old_selection = exterior_triangles(physical.positions(), physical.triangles(), &old_guide)
        .expect("old selection");
    let selected: Vec<u32> = serde_json::from_value(previous["selected_faces"].clone())
        .expect("selected original face IDs");
    assert_eq!(
        old_selection.kept, selected,
        "reproduce frozen selection exactly"
    );
    let source_selection =
        exterior_triangles(physical.positions(), physical.triangles(), &profile.guide)
            .expect("actual source guide selection");
    let physical_faces: Vec<_> = selected
        .iter()
        .map(|&face| physical.triangles()[face as usize])
        .collect();
    let (control, control_faces, _) = compact(physical.positions(), &physical_faces);
    let old_control: Vec<[f32; 3]> =
        serde_json::from_value(previous["control"].clone()).expect("frozen control");
    let old_faces: Vec<[u32; 3]> =
        serde_json::from_value(previous["control_faces"].clone()).expect("frozen faces");
    assert_eq!(control, old_control);
    assert_eq!(control_faces, old_faces);
    let retained_render_faces: Vec<_> = selected
        .iter()
        .map(|&face| triangles[face as usize])
        .collect();
    let (render, render_faces, origins) = compact(&points, &retained_render_faces);
    let render_uvs: Vec<_> = origins.iter().map(|&v| uvs[v as usize]).collect();
    let render_to_physical: Vec<_> = render
        .iter()
        .map(|point| {
            control
                .iter()
                .position(|p| p == point)
                .expect("render vertex belongs to unchanged physical source")
        })
        .collect();
    assert_eq!(
        render_faces
            .iter()
            .map(|face| face.map(|v| render_to_physical[v as usize] as u32))
            .collect::<Vec<_>>(),
        control_faces
    );
    render_faces
        .iter()
        .zip(&selected)
        .for_each(|(face, &source)| {
            assert_eq!(
                face.map(|v| origins[v as usize]),
                triangles[source as usize],
                "exact original corner order"
            );
            assert_eq!(
                face.map(|v| render_uvs[v as usize]),
                triangles[source as usize].map(|v| uvs[v as usize]),
                "exact retained UV corners"
            );
        });
    let selected_set: HashSet<_> = selected.iter().copied().collect();
    let removed: Vec<_> = (0..triangles.len() as u32)
        .filter(|v| !selected_set.contains(v))
        .collect();
    let area = |face: [u32; 3]| {
        let [a, b, c] = face.map(|v| DVec3::from(points[v as usize].map(f64::from)));
        0.5 * (b - a).cross(c - a).length()
    };
    let original_area: f64 = triangles.iter().map(|&face| area(face)).sum();
    let removed_area: f64 = removed.iter().map(|&v| area(triangles[v as usize])).sum();
    let topology = ControlDiagnostics::measure(&control, &control_faces)
        .expect("physical control diagnostics");
    let qualification = topology.qualify(&profile);
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("fixed original frame");
    let silhouette = compare((&points, &triangles), (&render, &render_faces), &frame)
        .expect("same original fixed frame");
    let measured = json!({"source_vertices":points.len(),"source_faces":triangles.len(),"prepared_render_vertices":render.len(),"prepared_faces":render_faces.len(),
        "physical_vertices":control.len(),"physical_faces":control_faces.len(),"source_guide_selection_identical":source_selection.kept==selected,
        "source_guide_selected_faces":source_selection.kept,"removed_original_faces":removed,"original_area":original_area,"removed_area":removed_area,"removed_area_share":removed_area/original_area,
        "prepared_original_silhouette_iou":silhouette.iou,"source_flared_hem_positions_moved":0,"uv_corner_max_error":0,
        "topology_qualification":format!("{qualification:?}"),"components":topology.components,"euler":topology.euler,"non_manifold_vertices":topology.non_manifold_vertices,
        "non_manifold_edges":topology.non_manifold_edges,"orientation_conflicts":topology.orientation_conflicts,"self_contacts":topology.self_contacts,
        "loops":topology.loops.iter().map(|l|json!({"edges":l.edges,"closed":l.closed,"center":l.center,"perimeter":l.perimeter,
            "points":l.vertices.iter().map(|&v|control[v as usize]).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "body_contacts":intersection_pairs(&render,&render_faces,&body,&body_faces[..6516]),"feet_contacts":intersection_pairs(&render,&render_faces,&body,&body_faces[6516..]),
        "coverage":"original omitted area retained in denominator; no full-source coverage pass claimed"});
    let output = json!({"positions":render,"triangles":render_faces,"uvs":render_uvs,"render_source_vertices":origins,"source_faces":selected,
        "physical_positions":control,"physical_triangles":control_faces,"render_to_physical":render_to_physical,"measured":measured,
        "material_reference":"unchanged original digest-pinned source NIF and retained original face IDs; no material rewrite",
        "source_nif_sha256":"b7dee8dda3a9c5642e262569f0eae398f1082d193a3de6351b49db8c1f2e7bc9"});
    std::fs::write(
        root.join("prepared-render.json"),
        serde_json::to_vec_pretty(&output).expect("prepared JSON"),
    )
    .expect("prepared artifact");
    eprintln!(
        "EXTERIOR_RENDER vertices={} faces={} loops={} self={} nonmanifold={} conflicts={} source_guide_equal={} removed_area_share={} original_iou={} qualification={qualification:?}",
        render.len(),
        render_faces.len(),
        topology.loops.len(),
        topology.self_contacts,
        topology.non_manifold_vertices,
        topology.orientation_conflicts,
        source_selection.kept == selected,
        removed_area / original_area,
        silhouette.iou
    );
    assert_eq!(
        source_selection.kept, selected,
        "actual source guide must reproduce the proposed selection"
    );
    assert!(
        qualification.is_ok(),
        "declared source construction must qualify"
    );
    assert!(
        silhouette.iou >= 0.888_341_722_214_664_7,
        "original silhouette"
    );
}
