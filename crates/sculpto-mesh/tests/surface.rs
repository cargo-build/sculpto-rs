use sculpto_mesh::surface::{SurfaceBinding, SurfaceError};

const CONTROL: [[f32; 3]; 3] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
const TRIANGLES: [[u32; 3]; 1] = [[0, 1, 2]];

fn close(actual: &[[f32; 3]], expected: &[[f32; 3]]) {
    assert_eq!(actual.len(), expected.len());
    actual.iter().zip(expected).for_each(|(a, b)| {
        a.iter()
            .zip(b)
            .for_each(|(a, b)| assert!((a - b).abs() < 1e-6, "{a} != {b}"));
    });
}

#[test]
fn rigid_motion_rotates_signed_thickness_and_keeps_edge_offsets() {
    let render = [[0.2, 0.3, 0.1], [0.7, 0.7, -0.2]];
    let binding = SurfaceBinding::bind(&render, &CONTROL, &TRIANGLES).unwrap();
    close(&binding.apply(&CONTROL, &TRIANGLES).unwrap(), &render);
    let transform = |[x, y, z]: [f32; 3]| [x + 2., 3. - z, y + 4.];
    close(
        &binding.apply(&CONTROL.map(transform), &TRIANGLES).unwrap(),
        &render.map(transform),
    );
    let [minimum, maximum] = binding.normal_offset_range();
    assert!((minimum + 0.2).abs() < 1e-7 && (maximum - 0.1).abs() < 1e-7);
}

#[test]
fn in_plane_stretch_keeps_thickness_in_source_units() {
    let binding = SurfaceBinding::bind(&[[0.2, 0.3, 0.1]], &CONTROL, &TRIANGLES).unwrap();
    let target = [[0., 0., 0.], [2., 0., 0.], [0., 3., 0.]];
    close(
        &binding.apply(&target, &TRIANGLES).unwrap(),
        &[[0.4, 0.9, 0.1]],
    );
}

#[test]
fn small_triangles_choose_the_actual_nearest_surface() {
    let mut control = [
        [0., 0., 0.],
        [1e-4, 0., 0.],
        [0., 1e-4, 0.],
        [2.5e-5, 2.5e-5, 2e-5],
        [4e-5, 2.5e-5, 2e-5],
        [2.5e-5, 4e-5, 2e-5],
    ];
    let triangles = [[0, 1, 2], [3, 4, 5]];
    let binding = SurfaceBinding::bind(&[[3e-5, 3e-5, 2e-6]], &control, &triangles).unwrap();
    control[..3].iter_mut().for_each(|point| point[2] += 0.001);
    close(
        &binding.apply(&control, &triangles).unwrap(),
        &[[3e-5, 3e-5, 0.001002]],
    );
}

#[test]
fn rejects_changed_topology_and_invalid_targets() {
    let binding = SurfaceBinding::bind(&[[0.2, 0.3, 0.1]], &CONTROL, &TRIANGLES).unwrap();
    assert_eq!(
        binding.apply(&CONTROL[..2], &TRIANGLES),
        Err(SurfaceError::TopologyMismatch)
    );
    assert_eq!(
        binding.apply(&CONTROL, &[[0, 2, 1]]),
        Err(SurfaceError::TopologyMismatch)
    );
    assert_eq!(
        binding.apply(&[[0.; 3]; 3], &TRIANGLES),
        Err(SurfaceError::CollapsedTriangle { triangle: 0 })
    );
    let mut invalid = CONTROL;
    invalid[1][0] = f32::NAN;
    assert_eq!(
        binding.apply(&invalid, &TRIANGLES),
        Err(SurfaceError::NonFinite {
            mesh: "control",
            vertex: 1
        })
    );
}

#[test]
fn rejects_missing_or_invalid_reference_geometry() {
    let render = [[0.2, 0.3, 0.1]];
    assert_eq!(
        SurfaceBinding::bind(&[], &CONTROL, &TRIANGLES),
        Err(SurfaceError::EmptyGarment)
    );
    assert_eq!(
        SurfaceBinding::bind(&render, &CONTROL, &[]),
        Err(SurfaceError::EmptyControl)
    );
    assert_eq!(
        SurfaceBinding::bind(&render, &CONTROL, &[[0, 1, 3]]),
        Err(SurfaceError::InvalidIndex {
            triangle: 0,
            vertex: 3,
            vertices: 3
        })
    );
    assert_eq!(
        SurfaceBinding::bind(&render, &[[0.; 3]; 3], &TRIANGLES),
        Err(SurfaceError::CollapsedTriangle { triangle: 0 })
    );
    assert_eq!(
        SurfaceBinding::bind(&[[f32::INFINITY, 0., 0.]], &CONTROL, &TRIANGLES),
        Err(SurfaceError::NonFinite {
            mesh: "render",
            vertex: 0
        })
    );
}
