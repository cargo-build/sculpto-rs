//! Conform behaviour: nearest-vertex binding, offset preservation, topology guard.

use sculpto_mesh::conform::{Conform, ConformError};

fn plane(size: usize) -> Vec<[f32; 3]> {
    (0..size).map(|index| [index as f32, 0.0, 0.0]).collect()
}

#[test]
fn bind_pins_each_vertex_to_the_nearest_reference() {
    let reference = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
    let garment = [[9.0, 0.0, 0.0], [0.5, 0.0, 0.0]];
    let conform = Conform::bind(&garment, &reference).unwrap();
    let vertices: Vec<usize> = conform
        .bindings()
        .iter()
        .map(|binding| binding.vertex)
        .collect();
    assert_eq!(vertices, vec![1, 0]);
    assert_eq!(conform.bindings()[0].offset, [-1.0, 0.0, 0.0]);
    assert_eq!(conform.bindings()[1].offset, [0.5, 0.0, 0.0]);
    assert!((conform.offset_max() - 1.0).abs() < 1e-6);
    assert!((conform.offset_mean() - 0.75).abs() < 1e-6);
}

#[test]
fn apply_follows_a_moved_body_and_keeps_the_authored_offset() {
    let reference = plane(8);
    let garment: Vec<[f32; 3]> = reference
        .iter()
        .map(|[x, y, z]| [*x, *y, z + 0.25])
        .collect();
    let conform = Conform::bind(&garment, &reference).unwrap();
    let moved: Vec<[f32; 3]> = reference
        .iter()
        .map(|[x, y, z]| [x + 1.5, y - 2.0, *z])
        .collect();
    let fitted = conform.apply(&moved).unwrap();
    assert_eq!(fitted.len(), moved.len());
    for (index, position) in fitted.iter().enumerate() {
        assert!((position[0] - moved[index][0]).abs() < 1e-6);
        assert!((position[1] - moved[index][1]).abs() < 1e-6);
        assert!((position[2] - moved[index][2] - 0.25).abs() < 1e-6);
    }
}

#[test]
fn apply_rejects_a_body_with_another_topology() {
    let conform = Conform::bind(&plane(4), &plane(4)).unwrap();
    let error = conform.apply(&plane(3)).unwrap_err();
    assert_eq!(
        error,
        ConformError::Topology {
            expected: 4,
            actual: 3
        }
    );
}

#[test]
fn follow_reports_zero_difference_when_the_garment_rides_the_body() {
    let reference = plane(8);
    let garment: Vec<[f32; 3]> = reference
        .iter()
        .map(|[x, y, z]| [*x, *y, z + 0.25])
        .collect();
    let conform = Conform::bind(&garment, &reference).unwrap();
    let moved: Vec<[f32; 3]> = reference
        .iter()
        .map(|[x, y, z]| [x + 1.5, y - 2.0, *z])
        .collect();
    let follow = conform.follow(&reference, &moved).unwrap();
    assert!(follow.body > 1.0);
    assert!((follow.garment - follow.body).abs() < 1e-6);
    assert!(follow.difference < 1e-6);
}

#[test]
fn bind_rejects_empty_inputs() {
    assert_eq!(
        Conform::bind(&[], &plane(1)).unwrap_err(),
        ConformError::EmptyGarment
    );
    assert_eq!(
        Conform::bind(&plane(1), &[]).unwrap_err(),
        ConformError::EmptyReference
    );
}
