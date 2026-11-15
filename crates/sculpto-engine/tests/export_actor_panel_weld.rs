//! The authored underwear's panel weld on the real fixtures: topology before
//! and after, measured through the engine's own weld. @req DECL0013

use std::collections::BTreeMap;
use std::path::PathBuf;

use sculpto_engine::export_actor::weld_layer;
use sculpto_engine::obj::read_obj_mesh;

/// The host fixture this gate reads; unset means the gate prints its SKIP line.
const FIXTURE: &str = "SCULPTO_ACTOR_FIXTURES";

/// Edge census of a triangle soup: `(boundary, non_manifold)` edge counts.
fn edge_census(triangles: &[[u32; 3]]) -> (usize, usize) {
    let mut usage: BTreeMap<[u64; 2], usize> = BTreeMap::new();
    for [a, b, c] in triangles {
        for (from, to) in [
            (u64::from(*a), u64::from(*b)),
            (u64::from(*b), u64::from(*c)),
            (u64::from(*c), u64::from(*a)),
        ] {
            let edge = if from < to { [from, to] } else { [to, from] };
            *usage.entry(edge).or_default() += 1;
        }
    }
    let mut census = (0, 0);
    for count in usage.values() {
        match count {
            1 => census.0 += 1,
            2 => {}
            _ => census.1 += 1,
        }
    }
    census
}

/// Largest distance the merge moved a merged twin onto its representative.
fn largest_shift(positions: &[[f32; 3]], groups: &[Vec<u32>]) -> f32 {
    groups
        .iter()
        .flat_map(|group| {
            let rest = positions[group[0] as usize];
            group.iter().map(move |&vertex| {
                let point = positions[vertex as usize];
                ((point[0] - rest[0]).powi(2)
                    + (point[1] - rest[1]).powi(2)
                    + (point[2] - rest[2]).powi(2))
                .sqrt()
            })
        })
        .fold(0.0_f32, f32::max)
}

/// Connected pieces of a triangle soup as vertex-id sets, first-appearance order.
fn pieces_of(triangles: &[[u32; 3]], vertices: usize) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..vertices).collect();
    let find = |vertex: usize, parent: &mut Vec<usize>| -> usize {
        let mut root = vertex;
        while parent[root] != root {
            root = parent[root];
        }
        let mut walk = vertex;
        while parent[walk] != root {
            let next = parent[walk];
            parent[walk] = root;
            walk = next;
        }
        root
    };
    for [a, b, c] in triangles {
        for other in [b, c] {
            let root_a = find(*a as usize, &mut parent);
            let root_b = find(*other as usize, &mut parent);
            if root_a != root_b {
                parent[root_a] = root_b;
            }
        }
    }
    let mut order: Vec<usize> = Vec::new();
    let mut pieces: Vec<Vec<usize>> = Vec::new();
    for vertex in 0..vertices {
        let root = find(vertex, &mut parent);
        if let Some(position) = order.iter().position(|&root_of| root_of == root) {
            pieces[position].push(vertex);
        } else {
            order.push(root);
            pieces.push(vec![vertex]);
        }
    }
    pieces.retain(|piece| !piece.is_empty());
    pieces
}

#[test]
fn the_authored_panels_weld_into_shared_seams() {
    let Ok(path) = std::env::var(FIXTURE) else {
        eprintln!("SKIP panel weld topology: {FIXTURE} is not set");
        return;
    };
    let text = std::fs::read_to_string(&path).expect("the declared fixture JSON");
    let fixture: serde_json::Value = serde_json::from_str(&text).expect("the fixture JSON parses");

    // One row per distinct layer OBJ, in declaration order.
    let mut seen: Vec<PathBuf> = Vec::new();
    for case in fixture["cases"].as_array().expect("cases") {
        for layer in case["layers"].as_array().expect("layers") {
            let obj = PathBuf::from(layer["obj"].as_str().expect("obj"));
            if seen.contains(&obj) {
                continue;
            }
            seen.push(obj.clone());
            let label = obj.file_stem().expect("layer name").to_string_lossy();
            let (positions, triangles) = read_obj_mesh(&obj).expect("the layer OBJ parses");
            let (welded, welded_triangles, report) = weld_layer(&positions, &triangles);
            let (boundary_before, non_manifold_before) = edge_census(&triangles);
            let (boundary_after, non_manifold_after) = edge_census(&welded_triangles);
            let shift = largest_shift(&positions, &report.groups);

            eprintln!(
                "{label}: {} v / {} t, {} -> {} components, {} -> {} boundary, \
                 {} -> {} non-manifold, {} merged pairs, {} dropped, largest shift {shift:.6}",
                positions.len(),
                triangles.len(),
                report.components_before,
                report.components_after,
                boundary_before,
                boundary_after,
                non_manifold_before,
                non_manifold_after,
                report.merged_pairs,
                report.dropped_faces,
            );
            for group in &report.groups {
                let rest = positions[group[0] as usize];
                let moved: Vec<_> = group[1..]
                    .iter()
                    .map(|&vertex| {
                        let point = positions[vertex as usize];
                        ((point[0] - rest[0]).powi(2)
                            + (point[1] - rest[1]).powi(2)
                            + (point[2] - rest[2]).powi(2))
                        .sqrt()
                    })
                    .collect();
                if moved.iter().any(|distance| *distance > 0.0) {
                    eprintln!("{label}: near twin {group:?} at {rest:?} moved by {moved:?}");
                }
            }
            if report.components_after > 1 {
                let pieces = pieces_of(&welded_triangles, welded.len());
                for piece in &pieces {
                    eprintln!(
                        "{label}: surviving piece: {} vertices, y {}..{}, x {}..{}, z {}..{}",
                        piece.len(),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][1])
                            .fold(f32::INFINITY, f32::min),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][1])
                            .fold(f32::NEG_INFINITY, f32::max),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][0])
                            .fold(f32::INFINITY, f32::min),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][0])
                            .fold(f32::NEG_INFINITY, f32::max),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][2])
                            .fold(f32::INFINITY, f32::min),
                        piece
                            .iter()
                            .map(|&vertex| welded[vertex][2])
                            .fold(f32::NEG_INFINITY, f32::max),
                    );
                }
                for (index, left) in pieces.iter().enumerate() {
                    for right in pieces.iter().skip(index + 1) {
                        let mut nearest = (f32::INFINITY, 0_usize, 0_usize);
                        for &one in left {
                            for &two in right {
                                let first = welded[one];
                                let second = welded[two];
                                let distance = ((first[0] - second[0]).powi(2)
                                    + (first[1] - second[1]).powi(2)
                                    + (first[2] - second[2]).powi(2))
                                .sqrt();
                                if distance < nearest.0 {
                                    nearest = (distance, one, two);
                                }
                            }
                        }
                        eprintln!(
                            "{label}: pieces {index}/{} nearest pair {:?}..{:?} at {:.6} u \
                             (welded ids {}, {})",
                            pieces.len(),
                            welded[nearest.1],
                            welded[nearest.2],
                            nearest.0,
                            nearest.1,
                            nearest.2
                        );
                    }
                }
            }

            // The authored panels abut exactly, so the merge never pulls
            // geometry further than the tolerance it grouped at.
            assert!(report.merged_pairs > 0, "{label}: nothing merged");
            assert_eq!(
                welded.len(),
                report.welded_vertices,
                "{label}: vertex count"
            );
            assert_eq!(
                welded_triangles.len(),
                triangles.len() - report.dropped_faces,
                "{label}: face count"
            );
            assert_eq!(report.dropped_faces, 0, "{label}: no face may collapse");
            assert!(
                shift <= report.tolerance,
                "{label}: a merged twin moved {shift} beyond the weld tolerance"
            );
            assert_eq!(
                non_manifold_before, 0,
                "{label}: the authored panels are manifold"
            );
            assert_eq!(
                non_manifold_after, 0,
                "{label}: the weld must not create a non-manifold edge"
            );
            assert_eq!(boundary_before, report.boundary_edges_before, "{label}");
            assert_eq!(boundary_after, report.boundary_edges_after, "{label}");
            assert!(
                report.components_after < report.components_before,
                "{label}: the panels did not join"
            );
            assert!(
                boundary_after < boundary_before,
                "{label}: the seams did not close"
            );

            // The measured topology of the three standard garments. The bra and
            // the panty close to one piece; the boxers keep two because the
            // waistband ring is authored as a detached shell whose nearest
            // approach to the shorts is a real gap (measured 0.139547 u), not a
            // seam the weld may force.
            match label.as_ref() {
                "boxers" => {
                    assert_eq!(positions.len(), 917);
                    assert_eq!(triangles.len(), 1_406);
                    assert_eq!(report.components_before, 11);
                    assert_eq!(report.components_after, 2);
                    assert_eq!(boundary_before, 406);
                    assert_eq!(boundary_after, 184);
                    assert_eq!(report.merged_pairs, 149);
                }
                "bra" => {
                    assert_eq!(positions.len(), 500);
                    assert_eq!(triangles.len(), 926);
                    assert_eq!(report.components_before, 6);
                    assert_eq!(report.components_after, 1);
                    assert_eq!(boundary_before, 80);
                    assert_eq!(boundary_after, 8);
                    assert_eq!(report.merged_pairs, 41);
                }
                "panty" => {
                    assert_eq!(positions.len(), 664);
                    assert_eq!(triangles.len(), 1_056);
                    assert_eq!(report.components_before, 5);
                    assert_eq!(report.components_after, 1);
                    assert_eq!(boundary_before, 264);
                    assert_eq!(boundary_after, 112);
                    assert_eq!(report.merged_pairs, 90);
                }
                other => panic!("{other} is not a standard underwear fixture"),
            }
        }
    }
    assert!(
        seen.len() >= 3,
        "the fixture must declare boxers, bra and panty"
    );
}
