//! Band-seat kernel controls: synthetic bodies with exact radii.

use sculpto_mesh::seat::{SeatError, SeatOptions, UpAxis, seat_band};

/// A vertical prism body of the given radius, `sides` sides, y in `[y0, y0+height]`,
/// with a vertex ring every `step` so every seat slice resolves a centre.
fn prism(
    radius: f32,
    sides: usize,
    y0: f32,
    height: f32,
    step: f32,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let levels = (height / step + 0.5) as usize;
    let ring_at = |level: usize| -> usize { level * sides };
    for level in 0..=levels {
        let y = y0 + (level as f32 * step).min(height);
        for index in 0..sides {
            let angle = index as f32 * std::f32::consts::TAU / sides as f32;
            positions.push([radius * angle.cos(), y, radius * angle.sin()]);
        }
    }
    for level in 0..levels {
        for index in 0..sides {
            let next = (index + 1) % sides;
            let low = (ring_at(level) + index) as u32;
            let low_next = (ring_at(level) + next) as u32;
            let top = (ring_at(level + 1) + index) as u32;
            let top_next = (ring_at(level + 1) + next) as u32;
            triangles.push([low, low_next, top]);
            triangles.push([low_next, top_next, top]);
        }
    }
    (positions, triangles)
}

/// A garment ring at one height: `radii` per side around the centre.
fn ring(y: f32, radius: f32, sides: usize) -> Vec<[f32; 3]> {
    (0..sides)
        .map(|index| {
            let angle = index as f32 * std::f32::consts::TAU / sides as f32;
            [radius * angle.cos(), y, radius * angle.sin()]
        })
        .collect()
}

fn radius_of(point: [f32; 3]) -> f32 {
    (point[0] * point[0] + point[2] * point[2]).sqrt()
}

#[test]
fn a_flaring_band_seats_to_the_declared_clearance() {
    let (body, triangles) = prism(10.0, 48, 0.0, 20.0, 1.0);
    // The garment flares from 12 at y=10 to 16 at y=16: the authored bell.
    let garment: Vec<[f32; 3]> = [10.0_f32, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0]
        .iter()
        .flat_map(|y| {
            let radius = 12.0 + (y - 10.0) * 0.8;
            ring(*y, radius, 24)
        })
        .collect();
    let options = SeatOptions {
        band: [10.0, 16.0],
        clearance: 0.2,
        blend: 0.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Y,
    };
    let (seated, report) = seat_band(&garment, &body, &triangles, &options).expect("seat runs");
    // Above the blend every seated silhouette sits at the clearance, so the
    // worst radial gap per slice collapses from up to 5.8 to about 0.2.
    for slice in &report.slices {
        if slice.height >= 12.0 && slice.bins_seated > 0 {
            assert!(
                slice.worst_after < 0.3,
                "height {}: worst standoff after {:.3}",
                slice.height,
                slice.worst_after
            );
            assert!(
                slice.worst_before > slice.worst_after,
                "height {}: the flare did not seat ({:.3} -> {:.3})",
                slice.height,
                slice.worst_before,
                slice.worst_after
            );
        }
    }
    // No vertex changed height: a seat can never open a hole the band covered.
    for (before, after) in garment.iter().zip(seated.iter()) {
        assert_eq!(before[1], after[1], "a seated vertex moved vertically");
    }
    assert_eq!(seated.len(), garment.len());
}

#[test]
fn the_blend_keeps_the_authored_shape_at_the_low_edge() {
    let (body, triangles) = prism(10.0, 48, 0.0, 20.0, 1.0);
    let garment: Vec<[f32; 3]> = [10.0_f32, 11.0, 12.0, 14.0]
        .iter()
        .flat_map(|y| ring(*y, 15.0, 24))
        .collect();
    let options = SeatOptions {
        band: [10.0, 14.0],
        clearance: 0.2,
        blend: 2.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Y,
    };
    let (seated, _) = seat_band(&garment, &body, &triangles, &options).expect("seat runs");
    let bottom = &seated[0..24];
    for point in bottom {
        assert!(
            (radius_of(*point) - 15.0).abs() < 1e-4,
            "the band's low edge must keep its authored radius, got {:.3}",
            radius_of(*point)
        );
    }
    for point in &seated[3 * 24..] {
        assert!(
            radius_of(*point) < 11.0,
            "above the blend the ring must seat, got {:.3}",
            radius_of(*point)
        );
    }
}

#[test]
fn an_inner_shell_follows_the_silhouette_without_crossing_the_body() {
    let (body, triangles) = prism(10.0, 48, 0.0, 20.0, 1.0);
    let mut garment = ring(12.0, 16.0, 24);
    // An inner lining shell inside the body at the same height.
    garment.extend(ring(12.0, 9.0, 24));
    let options = SeatOptions {
        band: [11.0, 13.0],
        clearance: 0.2,
        blend: 0.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Y,
    };
    let (seated, report) = seat_band(&garment, &body, &triangles, &options).expect("seat runs");
    let lining = &seated[24..];
    for point in lining {
        assert!(
            radius_of(*point) >= 10.0,
            "a lining vertex crossed the body surface: {:.3}",
            radius_of(*point)
        );
    }
    assert!(report.clamped >= 24, "the lining must report its clamp");
}

#[test]
fn vertices_outside_the_band_and_empty_slices_keep_their_shape() {
    // The body spans y 8..16 only: the garment rings at 2 and 18.5 hang over
    // a height with no body surface, like underwear over an open body band.
    let (body, triangles) = prism(10.0, 48, 8.0, 8.0, 1.0);
    let mut garment = ring(12.0, 15.0, 24);
    garment.extend(ring(2.0, 15.0, 24));
    garment.extend(ring(18.5, 15.0, 24));
    let options = SeatOptions {
        band: [1.5, 19.0],
        clearance: 0.2,
        blend: 0.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Y,
    };
    let (seated, _) = seat_band(&garment, &body, &triangles, &options).expect("seat runs");
    for point in &seated[24..] {
        assert!(
            (radius_of(*point) - 15.0).abs() < 1e-4,
            "a vertex without a body slice moved: {:.3}",
            radius_of(*point)
        );
    }
    // The ring over the body still seats.
    for point in &seated[0..24] {
        assert!(
            radius_of(*point) < 12.0,
            "the ring over the body did not seat: {:.3}",
            radius_of(*point)
        );
    }
}

#[test]
fn a_source_frame_body_seats_on_the_z_axis() {
    // The actor path reads OBJ meshes in the NIF source scene frame, where
    // `+z` is up and the horizontal plane is `x, y`. The same flare must seat
    // there: the heights move to `z` and nothing else changes.
    let (body, triangles) = {
        let (positions, faces) = prism(10.0, 48, 0.0, 20.0, 1.0);
        (
            positions
                .iter()
                .map(|&[x, y, z]| [x, -z, y])
                .collect::<Vec<_>>(),
            faces,
        )
    };
    let garment: Vec<[f32; 3]> = [12.0_f32, 14.0]
        .iter()
        .flat_map(|y| {
            ring(*y, 15.0, 24)
                .into_iter()
                .map(|[x, yy, z]| [x, -z, yy])
                .collect::<Vec<_>>()
        })
        .collect();
    let options = SeatOptions {
        band: [11.0, 15.0],
        clearance: 0.2,
        blend: 0.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Z,
    };
    let (seated, report) = seat_band(&garment, &body, &triangles, &options).expect("seat runs");
    assert!(report.moved >= 40, "the z-frame band must seat");
    for point in &seated {
        let horizontal = (point[0] * point[0] + point[1] * point[1]).sqrt();
        assert!(
            horizontal < 12.0,
            "a z-frame vertex did not seat: {horizontal:.3}"
        );
    }
}

#[test]
fn a_surface_beyond_the_silhouette_is_not_a_body_to_seat_on() {
    // A pillar stands far outside the garment on one side, like an arm beside
    // the waist. The bin pointing at it must keep the authored shape instead
    // of seating the band out onto the pillar.
    let (mut body, triangles) = prism(10.0, 48, 0.0, 20.0, 1.0);
    for level in 0..=20 {
        for index in 0..8 {
            let angle = index as f32 * std::f32::consts::TAU / 8.0;
            body.push([30.0 + 2.0 * angle.cos(), level as f32, 2.0 * angle.sin()]);
        }
    }
    let faces: Vec<[u32; 3]> = triangles;
    let garment: Vec<[f32; 3]> = [12.0_f32].iter().flat_map(|y| ring(*y, 15.0, 48)).collect();
    let options = SeatOptions {
        band: [11.0, 13.0],
        clearance: 0.2,
        blend: 0.0,
        slice: 1.0,
        bins: 64,
        up: UpAxis::Y,
    };
    let (seated, report) = seat_band(&garment, &body, &faces, &options).expect("seat runs");
    for point in &seated {
        assert!(
            radius_of(*point) < 15.5,
            "a vertex flew outward: {:.3}",
            radius_of(*point)
        );
    }
    // The bins over the cylinder itself still seat.
    assert!(report.moved > 0);
}

#[test]
fn malformed_options_refuse_instead_of_moving_anything() {
    let (body, triangles) = prism(10.0, 8, 0.0, 20.0, 1.0);
    let garment = ring(12.0, 15.0, 8);
    for bad in [
        SeatOptions {
            band: [16.0, 10.0],
            clearance: 0.2,
            blend: 0.0,
            slice: 1.0,
            bins: 64,
            up: UpAxis::Y,
        },
        SeatOptions {
            band: [10.0, 16.0],
            clearance: -0.2,
            blend: 0.0,
            slice: 1.0,
            bins: 64,
            up: UpAxis::Y,
        },
        SeatOptions {
            band: [10.0, 16.0],
            clearance: 0.2,
            blend: 8.0,
            slice: 1.0,
            bins: 64,
            up: UpAxis::Y,
        },
        SeatOptions {
            band: [10.0, 16.0],
            clearance: 0.2,
            blend: 0.0,
            slice: 0.0,
            bins: 64,
            up: UpAxis::Y,
        },
        SeatOptions {
            band: [10.0, 16.0],
            clearance: 0.2,
            blend: 0.0,
            slice: 1.0,
            bins: 4,
            up: UpAxis::Y,
        },
    ] {
        assert!(matches!(
            seat_band(&garment, &body, &triangles, &bad),
            Err(SeatError::Band(_))
                | Err(SeatError::Clearance(_))
                | Err(SeatError::Blend(_))
                | Err(SeatError::Slice(_))
                | Err(SeatError::Bins(_))
        ));
    }
}
