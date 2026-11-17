//! Surface measurements have no skeleton or tessellation-density dependency. @req DECL0013

use sculpto_mesh::actor::horizontal_sections;

fn box_surface() -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-1.0, 0.0, -2.0], [1.0, 0.0, -2.0],
            [1.0, 0.0, 2.0], [-1.0, 0.0, 2.0],
            [-1.0, 3.0, -2.0], [1.0, 3.0, -2.0],
            [1.0, 3.0, 2.0], [-1.0, 3.0, 2.0],
        ],
        vec![
            [0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6],
            [0, 4, 5], [0, 5, 1], [3, 2, 6], [3, 6, 7],
            [1, 5, 6], [1, 6, 2], [0, 3, 7], [0, 7, 4],
        ],
    )
}

#[test]
fn closed_sections_measure_area_independently_of_triangle_density() {
    let (mut vertices, mut triangles) = box_surface();
    let before = horizontal_sections(&vertices, &triangles, &[1.0]).unwrap();
    let triangle = triangles.remove(4);
    let middle = triangle
        .iter()
        .map(|index| vertices[*index as usize])
        .fold([0.0; 3], |sum, point| std::array::from_fn(|k| sum[k] + point[k] / 3.0));
    let added = vertices.len() as u32;
    vertices.push(middle);
    triangles.extend([
        [triangle[0], triangle[1], added],
        [triangle[1], triangle[2], added],
        [triangle[2], triangle[0], added],
    ]);
    let after = horizontal_sections(&vertices, &triangles, &[1.0]).unwrap();
    assert_eq!(before[0].open_chains, 0);
    assert_eq!(after[0].open_chains, 0);
    assert!((after[0].area() - 8.0).abs() < 1e-10);
    assert!((before[0].area() - after[0].area()).abs() < 1e-10);
    assert!(after[0].centroid().unwrap().iter().all(|value| value.abs() < 1e-10));
}

#[test]
fn an_open_section_cannot_claim_selected_area() {
    let (vertices, mut triangles) = box_surface();
    triangles.drain(4..6);
    let sections = horizontal_sections(&vertices, &triangles, &[1.0]).unwrap();
    assert!(sections[0].open_chains > 0);
    assert!(sections[0].unresolved.is_some());
    assert_eq!(sections[0].area(), 0.0);
    assert!(sections[0].centroid().is_none());
    assert!(horizontal_sections(&vertices, &[[0, 1, 99]], &[1.0]).is_err());
    assert!(horizontal_sections(&vertices, &triangles, &[f64::NAN]).is_err());
}

#[test]
fn roundoff_at_render_seams_keeps_contour_direction_and_closure() {
    let (vertices, triangles) = box_surface();
    let duplicated: Vec<[f64; 3]> = triangles.iter().enumerate().flat_map(|(i, t)| {
        let vertices = &vertices;
        t.iter().map(move |j| {
            let p = vertices[*j as usize];
            [p[0] + (i % 3) as f64 * 2e-8, p[1], p[2] - (i % 2) as f64 * 2e-8]
        })
    }).collect();
    let faces: Vec<[u32; 3]> = (0..triangles.len() as u32)
        .map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
    let cut = horizontal_sections(&duplicated, &faces, &[1.0000004]).unwrap();
    assert_eq!(cut[0].open_chains, 0);
    assert!(cut[0].unresolved.is_none());
    assert!((cut[0].area() - 8.0).abs() < 1e-6);
    assert!(cut[0].centroid().unwrap().iter().all(|v| v.abs() < 1e-6));
}
