//! Focused synthetic tests of `contact_morph` (FIT-04).
//!
//! Controls are hand-derived on a flat plane, flat cover plates and a closed
//! pyramid, so each expectation comes from the formula rather than from a
//! previous run. A single flat plane has no inside: the native centroid majority
//! (`ClippingFixer::OutwardSign`) counts every triangle of it as inward, so the
//! plane's outward side is `-z` and its covers sit below it. Sink then moves away
//! from the cover and swell towards the cover's side, as the native convention
//! intends. The pyramid is closed, so its orientation is decided by a clear
//! majority and its top face points `+z`.

use sculpto_mesh::contact_morph::{
    ContactError, ContactMorph, ContactMorphOptions, follow_deltas, make_contact_morph,
};
use sculpto_mesh::mask::VertexMask;

/// A square grid of `step` over `[-half, half]²` in the `z = 0` plane, wound
/// with `+z` normals.
struct Grid {
    positions: Vec<[f32; 3]>,
    faces: Vec<[u32; 3]>,
    count: usize,
    half: f32,
    step: f32,
}

impl Grid {
    fn plane(half: f32, step: f32) -> Self {
        let count = (2.0 * half / step).round() as usize + 1;
        let mut positions = Vec::with_capacity(count * count);
        for row in 0..count {
            for column in 0..count {
                positions.push([-half + column as f32 * step, -half + row as f32 * step, 0.0]);
            }
        }
        let mut faces = Vec::with_capacity(2 * (count - 1) * (count - 1));
        for row in 0..count - 1 {
            for column in 0..count - 1 {
                let corner = (row * count + column) as u32;
                let below = corner + count as u32;
                faces.push([corner, corner + 1, below + 1]);
                faces.push([corner, below + 1, below]);
            }
        }
        Self {
            positions,
            faces,
            count,
            half,
            step,
        }
    }

    fn index(&self, x: f32, y: f32) -> usize {
        let column = ((x + self.half) / self.step).round() as usize;
        let row = ((y + self.half) / self.step).round() as usize;
        row * self.count + column
    }
}

/// A flat rectangular plate at `z`, as one quad.
fn plate(half_x: f32, half_y: f32, z: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-half_x, -half_y, z],
            [half_x, -half_y, z],
            [half_x, half_y, z],
            [-half_x, half_y, z],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

/// A spherical-style cap about the `z` axis: apex at `-depth`, rim rising by
/// `rise`, so every point of it stays below the plane it presses.
fn cap(
    sectors: usize,
    rings: usize,
    radius: f32,
    depth: f32,
    rise: f32,
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = vec![[0.0, 0.0, -depth]];
    for ring in 1..=rings {
        let ring_radius = radius * ring as f32 / rings as f32;
        let ratio = ring_radius / radius;
        let z = -depth + rise * ratio * ratio;
        for sector in 0..sectors {
            let angle = std::f32::consts::TAU * sector as f32 / sectors as f32;
            positions.push([ring_radius * angle.cos(), ring_radius * angle.sin(), z]);
        }
    }
    let ring = |index: usize, sector: usize| (1 + (index - 1) * sectors + sector % sectors) as u32;
    let mut faces = Vec::new();
    for sector in 0..sectors {
        faces.push([0, ring(1, sector), ring(1, sector + 1)]);
    }
    for index in 1..rings {
        for sector in 0..sectors {
            let inner = ring(index, sector);
            let inner_next = ring(index, sector + 1);
            let outer = ring(index + 1, sector);
            let outer_next = ring(index + 1, sector + 1);
            faces.push([inner, inner_next, outer_next]);
            faces.push([inner, outer_next, outer]);
        }
    }
    (positions, faces)
}

/// A closed pyramid: the `[-1, 1]²` grid at `z = 0` and one apex below.
///
/// Returns the positions, the faces and how many vertices the top face has.
fn pyramid(apex: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>, usize) {
    let top = Grid::plane(1.0, 1.0);
    let count = top.count;
    let top_vertices = top.positions.len();
    let mut positions = top.positions;
    let apex_index = top_vertices as u32;
    positions.push([0.0, 0.0, -apex]);
    let mut faces = top.faces;
    // The top face's boundary, counter-clockwise seen from +z.
    let mut ring = Vec::new();
    for column in 0..count {
        ring.push(column);
    }
    for row in 1..count {
        ring.push(row * count + count - 1);
    }
    for column in (0..count - 1).rev() {
        ring.push((count - 1) * count + column);
    }
    for row in (1..count - 1).rev() {
        ring.push(row * count);
    }
    for (pair, &from) in ring.iter().enumerate() {
        let to = ring[(pair + 1) % ring.len()];
        faces.push([apex_index, to as u32, from as u32]);
    }
    (positions, faces, top_vertices)
}

/// Two halves of a plane that do not share vertices on their touching column.
fn split_plane(seam_gap: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let half = Grid::plane(1.0, 1.0);
    let mut positions = half
        .positions
        .iter()
        .map(|position| [position[0] - 1.0, position[1], position[2]])
        .collect::<Vec<_>>();
    positions.extend(
        half.positions
            .iter()
            .map(|position| [position[0] + 1.0 + seam_gap, position[1], position[2]]),
    );
    let offset = half.positions.len() as u32;
    let mut faces = half.faces.clone();
    faces.extend(
        half.faces
            .iter()
            .map(|face| face.map(|vertex| vertex + offset)),
    );
    (positions, faces)
}

/// A cover mesh, kept together so the fixtures read as one surface.
type Cover = (Vec<[f32; 3]>, Vec<[u32; 3]>);

fn morph(
    body: &Grid,
    cover: &Cover,
    lock: Option<&VertexMask>,
    options: &ContactMorphOptions,
) -> ContactMorph {
    make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        lock,
        options,
    )
    .expect("the fixture geometry is valid")
}

#[test]
fn dome_over_plane_sinks_the_covered_body() {
    let body = Grid::plane(2.0, 0.5);
    let cover = cap(16, 4, 1.0, 0.3, 0.2);
    let options = ContactMorphOptions::default();
    let result = morph(&body, &cover, None, &options);

    assert!(
        result.covered > 0,
        "the dome covers the middle of the plane"
    );
    assert!(
        result.covered < body.positions.len(),
        "the plane's rim stays free, but {} of {} vertices are covered",
        result.covered,
        body.positions.len()
    );
    assert!(result.max_depth > 0.0, "the covered body sinks in");
    assert!(
        result.max_depth <= options.depth,
        "sink {} exceeds the configured depth {}",
        result.max_depth,
        options.depth
    );
    assert!(
        result.max_bulge <= options.bulge,
        "swell {} exceeds the configured bulge {}",
        result.max_bulge,
        options.bulge
    );

    // The plane's outward side is -z, so each delta runs along z with its offset.
    for (vertex, delta) in result.deltas.iter().enumerate() {
        assert_eq!(delta[0], 0.0, "vertex {vertex} moved in x");
        assert_eq!(delta[1], 0.0, "vertex {vertex} moved in y");
        assert!(
            (delta[2] + result.offsets[vertex]).abs() < 1e-6,
            "vertex {vertex}: delta {delta:?} does not match offset {}",
            result.offsets[vertex]
        );
    }

    let center = body.index(0.0, 0.0);
    let corner = body.index(2.0, 2.0);
    assert!(
        result.edge_distance[center] > 0.0,
        "the centre is under the dome, edge distance {}",
        result.edge_distance[center]
    );
    assert!(
        result.edge_distance[corner] < 0.0,
        "the corner is free, edge distance {}",
        result.edge_distance[corner]
    );
}

#[test]
fn deep_cover_reaches_but_does_not_exceed_the_configured_depth() {
    let body = Grid::plane(6.0, 0.5);
    let cover = plate(4.0, 4.0, -1.0);
    let options = ContactMorphOptions::default();
    let result = morph(&body, &cover, None, &options);

    // The covered area is 8 units across, so the centre is 4.25 along the
    // surface from the edge: 1.3 * (1 - exp(-(4.25 / 1.8)^2)) = 1.295.
    assert!(
        result.max_depth > 1.2,
        "the deep cover should press near the full depth, got {}",
        result.max_depth
    );
    assert!(
        result.max_depth <= options.depth + 1e-6,
        "sink {} exceeds the configured depth {}",
        result.max_depth,
        options.depth
    );
    assert!(result.max_bulge <= options.bulge + 1e-6);
}

#[test]
fn edge_distance_is_positive_under_the_cover_and_negative_outside() {
    let body = Grid::plane(2.0, 0.5);
    // 1.5 below the plane: too far for contact, inside the tent distance.
    let cover = plate(1.0, 1.0, -1.5);
    let options = ContactMorphOptions::default();
    let result = morph(&body, &cover, None, &options);

    assert!(result.covered > 0, "the tent ray reaches the plate");
    for (vertex, &position) in body.positions.iter().enumerate() {
        let under = position[0].abs() <= 1.0 && position[1].abs() <= 1.0;
        let distance = result.edge_distance[vertex];
        assert_ne!(
            distance,
            -f32::MAX,
            "vertex {vertex} at {position:?} is beyond the geodesic reach"
        );
        assert_eq!(
            distance > 0.0,
            under,
            "vertex {vertex} at {position:?}: edge distance {distance} does not follow its side of the cover"
        );
    }
}

#[test]
fn tent_detection_uses_the_configured_distance() {
    let body = Grid::plane(2.0, 0.5);
    let cover = plate(1.0, 1.0, -1.5);

    let default = morph(&body, &cover, None, &ContactMorphOptions::default());
    assert!(
        default.covered > 0,
        "with the default tent distance the cover in front of the body counts"
    );

    let near = ContactMorphOptions {
        tent_distance: 0.5,
        ..ContactMorphOptions::default()
    };
    let short = morph(&body, &cover, None, &near);
    assert_eq!(
        short.covered, 0,
        "1.5 is beyond a 0.5 tent distance and beyond the 0.45 contact distance"
    );
    assert!(short.offsets.iter().all(|&offset| offset == 0.0));
    assert!(short.deltas.iter().all(|&delta| delta == [0.0; 3]));
}

#[test]
fn without_a_cover_or_a_contact_nothing_moves() {
    let body = Grid::plane(2.0, 0.5);
    let options = ContactMorphOptions::default();

    let empty = make_contact_morph(&body.positions, &body.faces, &[], &[], None, &options)
        .expect("an absent cover is valid");
    assert_eq!(empty.covered, 0);
    assert!(empty.offsets.iter().all(|&offset| offset == 0.0));
    assert!(empty.deltas.iter().all(|&delta| delta == [0.0; 3]));
    assert!(empty.edge_distance.iter().all(|&edge| edge == -f32::MAX));
    assert_eq!((empty.max_depth, empty.max_bulge), (0.0, 0.0));

    // Beyond both the contact and the tent distance.
    let far = morph(&body, &plate(1.0, 1.0, -3.0), None, &options);
    assert_eq!(far.covered, 0);
    assert_eq!((far.max_depth, far.max_bulge), (0.0, 0.0));

    // An empty body has nothing to press.
    let bare =
        make_contact_morph(&[], &[], &[], &[], None, &options).expect("an empty body is valid");
    assert_eq!(bare.covered, 0);
    assert!(bare.offsets.is_empty() && bare.deltas.is_empty());
}

#[test]
fn fully_locked_body_vertices_do_not_move() {
    let body = Grid::plane(2.0, 0.5);
    let cover = plate(1.0, 1.0, -1.5);
    let options = ContactMorphOptions::default();

    let free = morph(&body, &cover, None, &options);
    assert!(free.max_depth > 0.0 && free.max_bulge > 0.0);

    let lock =
        VertexMask::from_values(vec![1.0; body.positions.len()]).expect("1 is a valid mask value");
    let locked = morph(&body, &cover, Some(&lock), &options);
    assert_eq!(
        locked.covered, free.covered,
        "locks change what moves, not what is covered"
    );
    assert!(
        locked.deltas.iter().all(|&delta| delta == [0.0; 3]),
        "a fully locked vertex keeps an exact zero delta"
    );
    assert!(locked.offsets.iter().all(|&offset| offset == 0.0));
    assert_eq!((locked.max_depth, locked.max_bulge), (0.0, 0.0));
    assert_eq!(
        locked.edge_distance, free.edge_distance,
        "the profile keeps its shape"
    );
}

#[test]
fn fractional_freedom_scales_the_profile() {
    let body = Grid::plane(6.0, 0.5);
    let cover = plate(4.0, 4.0, -1.0);
    let options = ContactMorphOptions::default();

    let free = morph(&body, &cover, None, &options);
    let half_lock = VertexMask::from_values(vec![0.5; body.positions.len()])
        .expect("0.5 is a valid mask value");
    let half = morph(&body, &cover, Some(&half_lock), &options);

    let mut compared = 0;
    for (vertex, &offset) in free.offsets.iter().enumerate() {
        // Below this the 0.002 tail cutoff can zero the halved value.
        if offset.abs() < 0.01 {
            continue;
        }
        compared += 1;
        assert!(
            (half.offsets[vertex] - offset * 0.5).abs() < 1e-6,
            "vertex {vertex}: {} is not half of {offset}",
            half.offsets[vertex]
        );
    }
    assert!(
        compared > 10,
        "only {compared} vertices carry a profile worth comparing"
    );
}

#[test]
fn bulge_never_exceeds_the_room_the_cover_leaves() {
    let body = Grid::plane(6.0, 0.5);
    // 0.1 below the plane: contact covers its footprint, and the free vertices
    // just outside it have almost no room over them.
    let cover = plate(4.0, 4.0, -0.1);
    let options = ContactMorphOptions {
        smoothing: 0,
        ..ContactMorphOptions::default()
    };
    let result = morph(&body, &cover, None, &options);

    let room = |position: [f32; 3]| -> f32 {
        let across = (position[0].abs() - 4.0).max(0.0);
        let along = (position[1].abs() - 4.0).max(0.0);
        ((across * across + along * along + 0.01).sqrt() - options.contact_distance).max(0.0)
    };
    for (vertex, &offset) in result.offsets.iter().enumerate() {
        assert!(
            offset >= -options.depth - 1e-6,
            "vertex {vertex} sinks {offset}, deeper than {}",
            options.depth
        );
        if offset > 0.0 {
            let allowed = room(body.positions[vertex]);
            assert!(
                offset <= allowed + 1e-6,
                "vertex {vertex} at {:?} swells {offset} past its room {allowed}",
                body.positions[vertex]
            );
        }
    }
    assert!(result.max_bulge > 0.0, "some free vertices swell");

    // The first free vertex outside the plate would swell
    // 0.85 * (1.2907 / 1.3) * 0.2083² * exp(2 * (1 - 0.2083)) = 0.178, but the
    // cover leaves sqrt(0.25 + 0.01) - 0.45 = 0.0599: the room decides.
    let rim = body.index(4.5, 0.0);
    let expected = (0.25f32 + 0.01).sqrt() - options.contact_distance;
    assert!(
        (result.offsets[rim] - expected).abs() < 1e-4,
        "rim offset {} is not the room {expected}",
        result.offsets[rim]
    );
}

#[test]
fn seam_copies_share_the_profile_across_the_weld_distance() {
    // Two halves whose touching column is 2e-3 apart: only a weld distance of
    // 1e-2 bridges the seam, and without the bridge the right half is a separate
    // component with no covered vertex of its own.
    let (positions, faces) = split_plane(2e-3);
    let cover = plate(0.5, 1.0, -1.5);
    let cover = (
        cover
            .0
            .iter()
            .map(|position| [position[0] - 1.5, position[1], position[2]])
            .collect::<Vec<_>>(),
        cover.1,
    );
    let left_vertices = 9;
    let joined = ContactMorphOptions {
        weld_distance: 1e-2,
        ..ContactMorphOptions::default()
    };
    let bridged = make_contact_morph(&positions, &faces, &cover.0, &cover.1, None, &joined)
        .expect("the split plane and cover are valid");

    for pair in [(2usize, 9usize), (5, 12), (8, 15)] {
        assert!(
            (bridged.offsets[pair.0] - bridged.offsets[pair.1]).abs() < 1e-6,
            "seam pair {pair:?} moves apart: {} vs {}",
            bridged.offsets[pair.0],
            bridged.offsets[pair.1]
        );
        assert!(
            (bridged.edge_distance[pair.0] - bridged.edge_distance[pair.1]).abs() < 1e-4,
            "seam pair {pair:?} measures apart: {} vs {}",
            bridged.edge_distance[pair.0],
            bridged.edge_distance[pair.1]
        );
    }
    assert!(
        bridged.offsets[left_vertices..]
            .iter()
            .any(|&offset| offset != 0.0),
        "the bridged half receives the profile"
    );

    let split = ContactMorphOptions {
        weld_distance: 1e-4,
        ..ContactMorphOptions::default()
    };
    let apart = make_contact_morph(&positions, &faces, &cover.0, &cover.1, None, &split)
        .expect("the split plane and cover are valid");
    for (vertex, &edge) in apart.edge_distance.iter().enumerate().skip(left_vertices) {
        assert_eq!(edge, -f32::MAX, "unbridged vertex {vertex} has no distance");
        assert_eq!(
            apart.offsets[vertex], 0.0,
            "unbridged vertex {vertex} moves"
        );
    }
}

#[test]
fn repeated_follower_corners_retain_the_native_self_neighbour() {
    let body = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
    let body_deltas = [[0.0; 3], [0.4, 0.0, 0.0], [0.0; 3]];
    let follower = [[0.5, 0.5, 0.1], [1.5, 0.5, 0.1]];
    let deltas = follow_deltas(&body, &[[0, 1, 2]], &body_deltas, &follower, &[[0, 0, 1]], 1.0)
        .expect("native topology permits repeated corners");
    // Native neighbours are {0,1} and {0}. Starting at .05/.15, two
    // half-strength smoothing passes yield .08125/.0875, not .1/.1.
    assert!((deltas[0][0] - 0.08125).abs() < 1e-6, "{:?}", deltas[0]);
    assert!((deltas[1][0] - 0.0875).abs() < 1e-6, "{:?}", deltas[1]);
    assert_eq!([deltas[0][1], deltas[0][2], deltas[1][1], deltas[1][2]], [0.0; 4]);
}

#[test]
fn follower_translates_with_a_uniform_body_motion() {
    let body = Grid::plane(2.0, 0.25);
    let body_deltas = vec![[0.1, 0.0, 0.0]; body.positions.len()];
    let follower_positions = vec![
        [-0.5, -0.5, 0.3],
        [0.5, -0.5, 0.3],
        [0.5, 0.5, 0.3],
        [-0.5, 0.5, 0.3],
    ];
    let follower_faces = vec![[0, 1, 2], [0, 2, 3]];

    let deltas = follow_deltas(
        &body.positions,
        &body.faces,
        &body_deltas,
        &follower_positions,
        &follower_faces,
        1.0,
    )
    .expect("the follower and body are valid");

    for (vertex, delta) in deltas.iter().enumerate() {
        assert!(
            (delta[0] - 0.1).abs() < 1e-6,
            "vertex {vertex} moved {delta:?} instead of following the body"
        );
        assert_eq!((delta[1], delta[2]), (0.0, 0.0), "vertex {vertex}");
    }
}

#[test]
fn follower_blends_the_body_motion_at_its_closest_point() {
    // A single triangle with an in-plane motion, so the follower keeps its gap
    // and the gap-restoring passes have nothing to correct.
    let body_positions = vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]];
    let body_faces = vec![[0, 1, 2]];
    let body_deltas = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]];
    let follower_positions = vec![[0.0, 0.0, 0.4]];

    let deltas = follow_deltas(
        &body_positions,
        &body_faces,
        &body_deltas,
        &follower_positions,
        &[],
        1.0,
    )
    .expect("the follower and body are valid");

    // The foot of the follower sits at weights 1/4, 1/4, 1/2 of the corners:
    // 0.25 * 1 + 0.5 * -1 = -0.25, at full motion inside three quarters of the
    // radius.
    assert!(
        (deltas[0][0] + 0.25).abs() < 1e-6,
        "blended delta {:?}",
        deltas[0]
    );
    assert_eq!((deltas[0][1], deltas[0][2]), (0.0, 0.0));
}

#[test]
fn follower_fades_over_the_outer_quarter_of_the_radius() {
    let (body_positions, body_faces) = plate(4.0, 4.0, 0.0);
    let body_deltas = vec![[0.5, 0.0, 0.0]; body_positions.len()];
    // A translated plane has constant normals. Gap restoration cannot alter
    // this fade-only control, unlike the pyramid's varying vertex normals.
    let follower_positions = vec![
        [0.0, 0.0, 0.5],
        [0.0, 0.0, 0.75],
        [0.0, 0.0, 0.9],
        [0.0, 0.0, 1.0],
    ];

    let deltas = follow_deltas(
        &body_positions,
        &body_faces,
        &body_deltas,
        &follower_positions,
        &[],
        1.0,
    )
    .expect("the follower and body are valid");

    assert!(
        (deltas[0][0] - 0.5).abs() < 1e-6,
        "half the radius follows in full: {:?}",
        deltas[0]
    );
    assert!(
        (deltas[1][0] - 0.5).abs() < 1e-6,
        "three quarters of the radius still follows in full: {:?}",
        deltas[1]
    );
    // t = (0.9 - 0.75) / 0.25 = 0.6, fade = 1 - 0.6² * (3 - 1.2) = 0.352.
    assert!(
        (deltas[2][0] - 0.176).abs() < 1e-4,
        "the fade at 0.9 of the radius is {:?}",
        deltas[2]
    );
    assert_eq!(deltas[3], [0.0, 0.0, 0.0], "the radius follows nothing");
}

#[test]
fn follower_goes_back_out_to_its_old_gap() {
    let (body_positions, body_faces, top) = pyramid(2.0);
    let mut body_deltas = vec![[0.0; 3]; body_positions.len()];
    body_deltas[..top]
        .iter_mut()
        .for_each(|delta| *delta = [0.0, 0.0, 1.0]);
    let follower_positions = vec![[0.0, 0.0, 0.9]];

    let deltas = follow_deltas(
        &body_positions,
        &body_faces,
        &body_deltas,
        &follower_positions,
        &[],
        1.0,
    )
    .expect("the follower and body are valid");

    // The top rises to z = 1 while the follower follows only 0.352 of it, which
    // would leave a 0.252 gap instead of 0.9: it goes back out to the 0.9 it had,
    // so it ends one unit up.
    assert!(
        (deltas[0][2] - 1.0).abs() < 1e-6,
        "restored delta {:?}",
        deltas[0]
    );
    assert_eq!((deltas[0][0], deltas[0][1]), (0.0, 0.0));
}

#[test]
fn invalid_input_is_refused_by_name() {
    let body = Grid::plane(1.0, 1.0);
    let cover = plate(0.5, 0.5, -0.2);
    let options = ContactMorphOptions::default();

    let mut nonfinite = body.positions.clone();
    nonfinite[3] = [f32::NAN, 0.0, 0.0];
    let error = make_contact_morph(&nonfinite, &body.faces, &cover.0, &cover.1, None, &options)
        .expect_err("a nonfinite body is refused");
    assert_eq!(error, ContactError::NonFiniteBody { vertex: 3 });

    let error = make_contact_morph(
        &body.positions,
        &[[0, 1, 99]],
        &cover.0,
        &cover.1,
        None,
        &options,
    )
    .expect_err("a missing body vertex is refused");
    assert_eq!(
        error,
        ContactError::BodyFaceIndex {
            triangle: 0,
            vertex: 99
        }
    );

    let error = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &[[0, 1, 9]],
        None,
        &options,
    )
    .expect_err("a missing cover vertex is refused");
    assert_eq!(
        error,
        ContactError::CoverFaceIndex {
            triangle: 0,
            vertex: 9
        }
    );

    let short_mask = VertexMask::from_values(vec![0.0; 2]).expect("0 is a valid mask value");
    let error = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        Some(&short_mask),
        &options,
    )
    .expect_err("a short mask is refused");
    assert_eq!(
        error,
        ContactError::MaskLength {
            expected: 9,
            actual: 2
        }
    );

    let depth = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        None,
        &ContactMorphOptions {
            depth: -1.0,
            ..options
        },
    )
    .expect_err("a negative depth is refused");
    assert_eq!(depth, ContactError::Depth(-1.0));

    let width = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        None,
        &ContactMorphOptions {
            bulge_width: 0.0,
            ..options
        },
    )
    .expect_err("a zero bulge width is refused");
    assert_eq!(width, ContactError::BulgeWidth(0.0));

    let contact = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        None,
        &ContactMorphOptions {
            contact_distance: f32::NAN,
            ..options
        },
    )
    .expect_err("a nonfinite contact distance is refused");
    assert!(
        matches!(contact, ContactError::ContactDistance(value) if value.is_nan()),
        "unexpected error {contact:?}"
    );

    let weld = make_contact_morph(
        &body.positions,
        &body.faces,
        &cover.0,
        &cover.1,
        None,
        &ContactMorphOptions {
            weld_distance: -1e-3,
            ..options
        },
    )
    .expect_err("a negative weld distance is refused");
    assert_eq!(weld, ContactError::WeldDistance(-1e-3));

    let body_deltas = vec![[0.0; 3]; body.positions.len()];
    let follower = vec![[0.0, 0.0, 0.5]];

    let radius = follow_deltas(
        &body.positions,
        &body.faces,
        &body_deltas,
        &follower,
        &[],
        0.0,
    )
    .expect_err("a zero radius is refused");
    assert_eq!(radius, ContactError::Radius(0.0));

    let count = follow_deltas(
        &body.positions,
        &body.faces,
        &body_deltas[..3],
        &follower,
        &[],
        1.0,
    )
    .expect_err("mismatched deltas are refused");
    assert_eq!(
        count,
        ContactError::DeltaCount {
            expected: 9,
            actual: 3
        }
    );

    let mut infinite = body_deltas.clone();
    infinite[4] = [f32::INFINITY, 0.0, 0.0];
    let nonfinite_delta =
        follow_deltas(&body.positions, &body.faces, &infinite, &follower, &[], 1.0)
            .expect_err("a nonfinite delta is refused");
    assert_eq!(nonfinite_delta, ContactError::NonFiniteDelta { vertex: 4 });

    let follower_face = follow_deltas(
        &body.positions,
        &body.faces,
        &body_deltas,
        &follower,
        &[[0, 1, 9]],
        1.0,
    )
    .expect_err("a missing follower vertex is refused");
    assert_eq!(
        follower_face,
        ContactError::FollowerFaceIndex {
            triangle: 0,
            vertex: 1
        }
    );

    let body_face = follow_deltas(
        &body.positions,
        &[[0, 1, 9]],
        &body_deltas,
        &follower,
        &[],
        1.0,
    )
    .expect_err("a missing body vertex is refused");
    assert_eq!(
        body_face,
        ContactError::BodyFaceIndex {
            triangle: 0,
            vertex: 9
        }
    );

    let mut nonfinite_follower = follower.clone();
    nonfinite_follower[0] = [f32::NAN, 0.0, 0.0];
    let follower_position = follow_deltas(
        &body.positions,
        &body.faces,
        &body_deltas,
        &nonfinite_follower,
        &[],
        1.0,
    )
    .expect_err("a nonfinite follower is refused");
    assert_eq!(
        follower_position,
        ContactError::NonFiniteFollower { vertex: 0 }
    );
}
