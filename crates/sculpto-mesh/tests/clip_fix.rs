//! Portable controls for the native post-fit clipping repair (`clip_fix`).
//!
//! Every expected value is hand-computed from the native constants: at
//! strength 0.5 the inflation distance is `0.05 + 0.25·0.5 = 0.175`, the
//! margin `0.0875`, the total influence band `0.2625`, the search radius
//! `2 + 3·0.5 = 3.5`, and the poke push cap `0.1 + 0.5·0.5 = 0.35`. Bodies
//! use at least two triangles, matching the native AABB range constructor
//! that skips smaller meshes.

use sculpto_mesh::clip_fix::{
    ClipFixError, ClipFixOptions, fix_clipping, signed_distances_to_body,
};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::wrap::FitError;

/// Isolate the repair from fitting on identical frozen native inputs.
/// @req DECL0013
#[test]
fn sentinel_native_clip_env_gated() {
    let root = match std::env::var_os("SCULPTO_FIT_FIXTURE") {
        Some(root) => std::path::PathBuf::from(root),
        None => {
            eprintln!("SKIP sentinel_native_clip_env_gated: SCULPTO_FIT_FIXTURE is unset");
            return;
        }
    };
    let bytes = std::fs::read(root.join("body.nif")).expect("configured body fixture");
    let document = sculpto_nif::read(&bytes).expect("body NIF");
    let body = document
        .shapes
        .iter()
        .find(|shape| shape.name == "HIMBO - Body")
        .expect("HIMBO body");
    let read = |name: &str| {
        let mesh = sculpto_mesh::intake_gltf::read_raw(&root.join(name))
            .expect("configured native geometry");
        let positions: Vec<_> = mesh
            .positions
            .iter()
            .map(|point| point.map(|value| value / mesh.scale.factor))
            .collect();
        (positions, mesh.triangles)
    };
    let results: Vec<_> = ["Armor001", "Armor001_1"].into_iter().map(|name| {
        let (input, triangles) = read(&format!("native-tool-zup-{name}.obj"));
        let (expected, expected_triangles) = read(&format!("native-tool-clip1-zup-{name}.obj"));
        assert_eq!(triangles, expected_triangles);
        let (positions, report) = fix_clipping(
            (&input, &triangles), (&body.positions, &body.triangles), None,
            &ClipFixOptions::default(),
        ).expect("native default repair");
        assert_eq!(positions.len(), expected.len());
        let deviations: Vec<_> = positions.iter().zip(&expected).map(|(ours, native)| {
            ours.iter().zip(native).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt()
        }).collect();
        let max = deviations.iter().copied().fold(0.0, f32::max);
        let mean = deviations.iter().sum::<f32>() / deviations.len() as f32;
        let distances = signed_distances_to_body(
            (&body.positions, &body.triangles), &positions, 3.5,
        ).expect("measured clearance");
        let inside = distances.iter().filter(|distance| distance.is_some_and(|value| value < 0.0)).count();
        let unknown = distances.iter().filter(|distance| distance.is_none()).count();
        eprintln!("Sentinel clip {name}: max_delta={max} mean_delta={mean} signed_negative={inside} signed_unknown={unknown} report={report:?}");
        (name, max)
    }).collect();
    for (name, max) in results {
        assert!(
            max <= 1e-4,
            "{name} repair deviation {max} exceeds diagnostic1e-4"
        );
    }
}

/// A square in the xy plane at z = 0, wound with normals in +z.
fn plane(half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-half, -half, 0.0],
            [half, -half, 0.0],
            [half, half, 0.0],
            [-half, half, 0.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

/// A small horizontal garment patch at height `z`.
fn patch(z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-0.1, -0.1, z],
            [0.1, -0.1, z],
            [0.1, 0.1, z],
            [-0.1, 0.1, z],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

/// A plane with a four-sided pyramid bump whose tip is off the plane diagonal.
fn plane_with_bump() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut faces) = plane(3.0);
    let ring = [
        [-0.3, -0.45, 0.0],
        [0.7, -0.45, 0.0],
        [0.7, 0.55, 0.0],
        [-0.3, 0.55, 0.0],
    ];
    let base = positions.len() as u32;
    positions.extend_from_slice(&ring);
    let tip = positions.len() as u32;
    positions.push([0.2, 0.05, 1.0]);
    for corner in 0..4u32 {
        let next = (corner + 1) % 4;
        faces.push([tip, base + corner, base + next]);
    }
    (positions, faces)
}

/// Two parallel open plates, the lower one wound with normals in -z, so the
/// plate center vertices have pure +/-z normals.
fn plates(half: f32, z_low: f32, z_high: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let axis = [-half, 0.0, half];
    let grid = |z: f32| -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for &y in &axis {
            for &x in &axis {
                positions.push([x, y, z]);
            }
        }
        let mut faces = Vec::new();
        for row in 0..2u32 {
            for column in 0..2u32 {
                let a = row * 3 + column;
                faces.push([a, a + 1, a + 4]);
                faces.push([a, a + 4, a + 3]);
            }
        }
        (positions, faces)
    };
    let (mut positions, mut faces) = grid(z_high);
    let (low, low_faces) = grid(z_low);
    let offset = positions.len() as u32;
    positions.extend(low);
    faces.extend(
        low_faces
            .iter()
            .map(|face| [face[0] + offset, face[2] + offset, face[1] + offset]),
    );
    (positions, faces)
}

/// The patch displaced to the inflated surface at strength 0.5:
/// `inflation = 0.175`, so a vertex 0.02 inside pushes `0.195` up.
const PUSHED_Z: f32 = 0.98 + 0.195;

#[test]
fn shallow_penetration_pushes_to_the_inflated_surface() {
    let (body, body_faces) = plates(3.0, -1.0, 1.0);
    let (garment, faces) = patch(0.98);
    let (positions, report) = fix_clipping(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &ClipFixOptions::default(),
    )
    .expect("repair");
    assert_eq!(report.moved, 4);
    assert!(
        (report.max_push - 0.195).abs() < 1e-5,
        "push {} must be inflation 0.175 minus signed -0.02",
        report.max_push
    );
    for (after, before) in positions.iter().zip(garment.iter()) {
        // Nonidentity offsets survive: x/y untouched, z lifted to the band.
        assert_eq!(after[0], before[0]);
        assert_eq!(after[1], before[1]);
        assert!(
            (after[2] - PUSHED_Z).abs() < 1e-5,
            "z {} must sit on the inflated surface {PUSHED_Z}",
            after[2]
        );
    }
}

#[test]
fn geometry_outside_the_influence_band_stays_put() {
    let (body, body_faces) = plates(3.0, -1.0, 1.0);
    let (garment, faces) = patch(2.0);
    let (positions, report) = fix_clipping(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &ClipFixOptions::default(),
    )
    .expect("repair");
    assert_eq!(report.moved, 0);
    assert_eq!(report.max_push, 0.0);
    // Found by the search radius (2 < 3.5) but outside the influence band:
    // the repair must not touch it at all.
    assert_eq!(positions, garment);
}

#[test]
fn locked_vertices_blend_like_the_native_caller_weights() {
    let (body, body_faces) = plates(3.0, -1.0, 1.0);
    let (garment, faces) = patch(0.98);

    // Fully locked copies keep their positions bit-identically while their
    // free neighbours move.
    let locked = VertexMask::from_values(vec![1.0, 1.0, 0.0, 0.0]).expect("valid mask");
    let (positions, report) = fix_clipping(
        (&garment, &faces),
        (&body, &body_faces),
        Some(&locked),
        &ClipFixOptions::default(),
    )
    .expect("repair");
    assert_eq!(positions[0], garment[0]);
    assert_eq!(positions[1], garment[1]);
    assert!((positions[2][2] - PUSHED_Z).abs() < 1e-5);
    assert!((positions[3][2] - PUSHED_Z).abs() < 1e-5);
    assert_eq!(report.moved, 2);

    // Half-locked vertices take half the fix, exactly like the source
    // caller's soft mask border blend.
    let soft = VertexMask::from_values(vec![0.5; 4]).expect("valid mask");
    let (positions, _) = fix_clipping(
        (&garment, &faces),
        (&body, &body_faces),
        Some(&soft),
        &ClipFixOptions::default(),
    )
    .expect("repair");
    let blended = 0.98 + 0.195 * 0.5;
    for position in &positions {
        assert!(
            (position[2] - blended).abs() < 1e-6,
            "z {} must be the half-weight blend {blended}",
            position[2]
        );
    }
}

#[test]
fn body_pokes_between_vertices_push_the_spanning_triangle() {
    let (body, body_faces) = plane_with_bump();
    let garment = vec![
        [-2.0, -2.0, 0.5],
        [2.0, -2.0, 0.5],
        [2.0, 2.0, 0.5],
        [-2.0, 2.0, 0.5],
    ];
    let faces = vec![[0, 1, 2], [0, 2, 3]];
    let options = ClipFixOptions {
        fix_poke_through: true,
        ..ClipFixOptions::default()
    };
    let (positions, report) =
        fix_clipping((&garment, &faces), (&body, &body_faces), None, &options).expect("repair");
    // Every patch vertex sits 0.5 above the plane, outside the influence
    // band, so only the poke path can move anything. The tip is 1.0 above
    // the plane and its unwelded normal is exactly +z; the push is
    // `min(0.5, cap 0.35) + inflation 0.175 = 0.525` on the triangle the ray
    // crosses, whose far corner stays.
    let lifted = 0.5 + 0.525;
    for (vertex, position) in positions.iter().enumerate().take(3) {
        assert!(
            (position[2] - lifted).abs() < 1e-5,
            "poked corner {vertex} at {} must lift to {lifted}",
            position[2]
        );
    }
    assert_eq!(positions[3], garment[3]);
    assert_eq!(report.moved, 3);
    assert!((report.max_push - 0.525).abs() < 1e-5);
}

#[test]
fn welded_seam_copies_move_together() {
    let (body, body_faces) = plates(3.0, -1.0, 1.0);
    // Vertex 4 duplicates vertex 0 exactly; the second triangle uses the
    // copy, like a split UV seam.
    let mut garment = vec![
        [-0.1, -0.1, 0.98],
        [0.1, -0.1, 0.98],
        [0.1, 0.1, 0.98],
        [-0.1, 0.1, 0.98],
    ];
    garment.push(garment[0]);
    let faces = vec![[0, 1, 2], [4, 2, 3]];
    let (positions, report) = fix_clipping(
        (&garment, &faces),
        (&body, &body_faces),
        None,
        &ClipFixOptions::default(),
    )
    .expect("repair");
    assert_eq!(report.moved, 5);
    assert_eq!(
        positions[0], positions[4],
        "co-located copies must take one displacement"
    );
    for position in &positions {
        assert!((position[2] - PUSHED_Z).abs() < 1e-5);
    }
}

#[test]
fn signed_distances_read_inside_outside_and_past_openings() {
    let (plane_body, plane_faces) = plane(3.0);

    // Coplanar centroid votes tie toward the native negative normal sign.
    let distances = signed_distances_to_body((&plane_body, &plane_faces), &[[1.0, 0.5, 0.7]], 2.0)
        .expect("distances");
    assert_eq!(distances, vec![Some(-0.7)]);

    // Beyond the search radius: no qualifying surface.
    let distances = signed_distances_to_body((&plane_body, &plane_faces), &[[0.0, 0.0, 5.0]], 2.0)
        .expect("distances");
    assert_eq!(distances, vec![None]);

    // Past an open edge of the plane the point reads its unsigned distance,
    // not a fake inside sign.
    let distances = signed_distances_to_body((&plane_body, &plane_faces), &[[5.0, 0.0, 0.3]], 5.0)
        .expect("distances");
    let expected = (4.0f32 + 0.3 * 0.3).sqrt();
    assert_eq!(distances.len(), 1);
    let value = distances[0].expect("the rim is within radius");
    assert!(
        (value - expected).abs() < 1e-5,
        "past-opening distance {value} must be unsigned {expected}"
    );

    // Between the plates reads negative, unlike the past-opening case:
    // the closest point of the slab's top plate is its center vertex, whose
    // unwelded normal is exactly +z.
    let (slab, slab_faces) = plates(3.0, -1.0, 1.0);
    let outside = signed_distances_to_body((&slab, &slab_faces), &[[0.0, 0.0, 1.7]], 2.0)
        .expect("outside distance");
    assert!((outside[0].expect("top plate within radius") - 0.7).abs() < 1e-5);
    let distances =
        signed_distances_to_body((&slab, &slab_faces), &[[0.0, 0.0, 0.3]], 2.0).expect("distances");
    let value = distances[0].expect("the slab center is within radius");
    assert!(
        (value + 0.7).abs() < 1e-5,
        "point 0.3 between plates at +/-1 must read -0.7, got {value}"
    );
}

#[test]
fn invalid_options_and_geometry_are_refused() {
    let (body, body_faces) = plane(3.0);
    let (garment, faces) = patch(0.98);
    let repair = |options: &ClipFixOptions| {
        fix_clipping((&garment, &faces), (&body, &body_faces), None, options)
    };

    assert!(matches!(
        repair(&ClipFixOptions {
            strength: 1.5,
            ..ClipFixOptions::default()
        }),
        Err(ClipFixError::Strength(1.5))
    ));
    assert!(matches!(
        repair(&ClipFixOptions {
            strength: f32::NAN,
            ..ClipFixOptions::default()
        }),
        Err(ClipFixError::Strength(_))
    ));
    // Zero strength is the source's no-op, not an error.
    let (positions, report) = repair(&ClipFixOptions {
        strength: 0.0,
        ..ClipFixOptions::default()
    })
    .expect("no-op");
    assert_eq!(positions, garment);
    assert_eq!(report.moved, 0);

    let short_mask = VertexMask::from_values(vec![0.0]).expect("valid mask");
    assert!(matches!(
        fix_clipping(
            (&garment, &faces),
            (&body, &body_faces),
            Some(&short_mask),
            &ClipFixOptions::default()
        ),
        Err(ClipFixError::LockLength {
            expected: 4,
            actual: 1
        })
    ));

    let empty: Vec<[f32; 3]> = Vec::new();
    assert!(matches!(
        fix_clipping(
            (&empty, &faces),
            (&body, &body_faces),
            None,
            &ClipFixOptions::default()
        ),
        Err(ClipFixError::Geometry(FitError::EmptyGarment))
    ));
    let no_faces: Vec<[u32; 3]> = Vec::new();
    assert!(matches!(
        fix_clipping(
            (&garment, &faces),
            (&body, &no_faces),
            None,
            &ClipFixOptions::default()
        ),
        Err(ClipFixError::Geometry(FitError::EmptyBodyFaces))
    ));
    let non_finite = vec![
        [0.0, 0.0, f32::NAN],
        [0.1, 0.0, 0.98],
        [0.0, 0.1, 0.98],
        [0.1, 0.1, 0.98],
    ];
    assert!(matches!(
        fix_clipping(
            (&non_finite, &faces),
            (&body, &body_faces),
            None,
            &ClipFixOptions::default()
        ),
        Err(ClipFixError::Geometry(FitError::NonFiniteGarmentPosition {
            vertex: 0
        }))
    ));
    let bad_faces = vec![[0, 1, 4]];
    assert!(matches!(
        fix_clipping(
            (&garment, &bad_faces),
            (&body, &body_faces),
            None,
            &ClipFixOptions::default()
        ),
        Err(ClipFixError::Geometry(FitError::GarmentFaceIndex {
            triangle: 0,
            vertex: 4
        }))
    ));

    assert!(matches!(
        signed_distances_to_body((&body, &body_faces), &[garment[0]], 0.0),
        Err(ClipFixError::SearchRadius(0.0))
    ));
    assert!(matches!(
        signed_distances_to_body((&body, &body_faces), &[[f32::NAN, 0.0, 0.0]], 2.0),
        Err(ClipFixError::NonFinitePoint { point: 0 })
    ));
    assert!(matches!(
        signed_distances_to_body((&empty, &no_faces), &[garment[0]], 2.0),
        Err(ClipFixError::Geometry(FitError::EmptyBody))
    ));
    assert!(matches!(
        signed_distances_to_body((&non_finite, &faces), &[garment[0]], 2.0),
        Err(ClipFixError::Geometry(FitError::NonFiniteBodyPosition {
            vertex: 0
        }))
    ));
    assert!(matches!(
        signed_distances_to_body((&body, &bad_faces), &[garment[0]], 2.0),
        Err(ClipFixError::Geometry(FitError::BodyFaceIndex {
            triangle: 0,
            vertex: 4
        }))
    ));
}
