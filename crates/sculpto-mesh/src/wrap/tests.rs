use super::*;
use crate::surface_util::outward_vertex_normals;

/// A seam's own face normal finds the cover; its welded diagonal misses it.
/// @req DECL0013
#[test]
fn split_seams_keep_native_poke_ray_normals() {
    let body = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
    ];
    let body_faces = [[0, 2, 1], [3, 5, 4], [6, 8, 7]];
    let garment = [
        [-0.1, -0.1, 0.5],
        [0.1, -0.1, 0.5],
        [0.1, 0.1, 0.5],
        [-0.1, 0.1, 0.5],
    ];
    let faces = [[0, 1, 2], [0, 2, 3]];
    let (normals, sign) = outward_vertex_normals(&body, &body_faces);
    let surface = BodySurface::build(&body, &body_faces);
    assert_eq!(sign, 1.0);
    let hits = surface.poke_hits((&garment, &faces), &normals, 1.5);
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].0, hits[0].1.triangle, hits[0].2), (0, 0, 0.5));
    let (_, report) = fit_to_body(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &FitOptions::default(),
    )
    .expect("seam fit");
    assert_eq!(
        report.poke_before, 1,
        "native ray at body vertex0 crosses the cover at depth0.5"
    );
}

/// Isolate the normal policy on frozen native geometry before changing fitting.
/// @req DECL0013
#[test]
fn sentinel_poke_normals_env_gated() {
    let root = match std::env::var_os("SCULPTO_FIT_FIXTURE") {
        Some(root) => std::path::PathBuf::from(root),
        None => {
            eprintln!("SKIP sentinel_poke_normals_env_gated: SCULPTO_FIT_FIXTURE is unset");
            return;
        }
    };
    let bytes = std::fs::read(root.join("body.nif")).expect("body fixture");
    let document = sculpto_nif::read(&bytes).expect("body NIF");
    let body = document
        .shapes
        .iter()
        .find(|shape| shape.name == "HIMBO - Body")
        .expect("HIMBO body");
    let surface = BodySurface::build(&body.positions, &body.triangles);
    let (native_normals, _) = outward_vertex_normals(&body.positions, &body.triangles);
    let changed: Vec<_> = surface
        .normals
        .iter()
        .zip(&native_normals)
        .enumerate()
        .filter_map(|(vertex, (&welded, &native))| {
            (length(sub(welded, native)) > 1e-3).then_some(vertex)
        })
        .collect();
    eprintln!(
        "poke_normals_changed={}",
        serde_json::to_string(&changed).expect("trace JSON")
    );
    for name in ["Armor001", "Armor001_1"] {
        let native = crate::intake_gltf::read_raw(&root.join(format!("native-zup-{name}.obj")))
            .expect("native frozen output");
        let positions: Vec<_> = native
            .positions
            .iter()
            .map(|point| point.map(|value| value / native.scale.factor))
            .collect();
        for (label, normals) in [("welded", &surface.normals), ("native", &native_normals)] {
            let records: Vec<_> = surface
                .poke_hits((&positions, &native.triangles), normals, 1.5)
                .into_iter()
                .map(|(vertex, hit, depth)| (vertex, hit.triangle, depth))
                .collect();
            eprintln!(
                "poke_trace case={name} policy={label} records={}",
                serde_json::to_string(&records).expect("trace JSON")
            );
        }
    }
}
