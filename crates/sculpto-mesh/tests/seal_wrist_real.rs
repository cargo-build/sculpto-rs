//! Real-fixture wrist-seat acceptance (BLD-13): the Bijin body plus the
//! normalized hands OBJ, seated and welded through `seal_joints` with both
//! wrist joints declared. Env-gated on `SCULPTO_SEAL_FIXTURES` pointing at a
//! directory holding `bijin0-body-feet-yup.obj` (sha256
//! `af75623c0c127c44…`) and `hands-actual.obj` (`aba9b19b4a6f38a9…`); the
//! pair is assembled in memory only. The asserted numbers are the measured
//! seat from the BLD-13 seams (00C0/0100): scale 64.82/64.73, rotation
//! 176.5°/176.8° about ≈ ∓Y, cross-side landing, plane deviation under the
//! socket's own planarity, 20 bridge triangles per 10↔10 wrist and no
//! fold-back edge beyond 150° after the weld.

use std::path::PathBuf;

use sculpto_mesh::joint_seal::{JointRequest, SealOptions, seal_joints};
use sculpto_mesh::parts;

/// Read a raw OBJ surface (positions and fan-triangulated faces); the
/// fixtures are plain `v`/`f` triangle soup in the source Y-up frame.
fn read_obj(path: &std::path::Path) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} must be readable: {error}", path.display()));
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("v ") {
            let lanes: Vec<f32> = rest
                .split_whitespace()
                .map(|token| token.parse().expect("numeric vertex lane"))
                .collect();
            assert_eq!(lanes.len(), 3, "vertex line {line:?}");
            positions.push([lanes[0], lanes[1], lanes[2]]);
        } else if let Some(rest) = line.strip_prefix("f ") {
            let ids: Vec<u32> = rest
                .split_whitespace()
                .map(|token| {
                    token
                        .split('/')
                        .next()
                        .unwrap()
                        .parse::<u32>()
                        .expect("index")
                        - 1
                })
                .collect();
            for corner in 1..ids.len() - 1 {
                triangles.push([ids[0], ids[corner], ids[corner + 1]]);
            }
        }
    }
    (positions, triangles)
}

/// Rotation angle in degrees and unit axis of a row-major matrix.
fn angle_axis(rotation: &[[f64; 3]; 3]) -> (f64, [f64; 3]) {
    let trace = rotation[0][0] + rotation[1][1] + rotation[2][2];
    let angle = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees();
    let raw = [
        rotation[2][1] - rotation[1][2],
        rotation[0][2] - rotation[2][0],
        rotation[1][0] - rotation[0][1],
    ];
    let length = raw.iter().map(|lane| lane * lane).sum::<f64>().sqrt();
    let axis = raw.map(|lane| lane / length.max(1e-300));
    (angle, axis)
}

#[test]
fn bijin_wrists_seat_cross_side_and_weld() {
    let root = std::env::var_os("SCULPTO_SEAL_FIXTURES").filter(|value| !value.is_empty());
    let Some(root) = root else {
        eprintln!("SKIP bijin_wrists_seat_cross_side_and_weld: SCULPTO_SEAL_FIXTURES is unset");
        return;
    };
    let root = PathBuf::from(root);
    let (body_pos, body_tri) = read_obj(&root.join("bijin0-body-feet-yup.obj"));
    let (hands_pos, hands_tri) = read_obj(&root.join("hands-actual.obj"));
    // Structural pins for the sha256-pinned fixtures above.
    assert_eq!(body_pos.len(), 5825, "bijin0 body vertex count");
    assert_eq!(body_tri.len(), 10488, "bijin0 body face count");
    assert_eq!(hands_pos.len(), 1780, "hands vertex count");
    assert_eq!(hands_tri.len(), 2912, "hands face count");
    let offset = body_pos.len() as u32;
    let mut positions = body_pos;
    positions.extend(hands_pos);
    let mut triangles = body_tri;
    triangles.extend(
        hands_tri
            .iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );

    let identities = parts::shell_identities(&positions, &triangles);
    let torso = identities
        .iter()
        .find(|shell| shell.name == "torso")
        .expect("the torso shell resolves");
    let hands: Vec<&parts::ShellIdentity> = identities
        .iter()
        .filter(|shell| shell.members.len() == 890)
        .collect();
    assert_eq!(hands.len(), 2, "the two hand shells");
    let centre_x = |shell: &parts::ShellIdentity| -> f64 {
        shell
            .members
            .iter()
            .map(|&vertex| f64::from(positions[vertex][0]))
            .sum::<f64>()
            / shell.members.len() as f64
    };
    let mut hands = hands;
    hands.sort_by(|left, right| centre_x(right).total_cmp(&centre_x(left)));
    assert!(centre_x(hands[0]) > 0.3, "first hand is the +X source");

    let requests: Vec<JointRequest> = hands
        .iter()
        .map(|hand| JointRequest {
            part: hand.name.clone(),
            socket: torso.name.clone(),
            seat: true,
        })
        .collect();
    let outcome = seal_joints(&positions, &triangles, &requests, &SealOptions::default())
        .expect("both Bijin wrists seat, weld and bridge");
    assert_eq!(outcome.report.joints.len(), 2);

    // Cross-side landing: the +X source hand's seated centroid is on −X.
    let seated_x = |shell: &parts::ShellIdentity| -> f64 {
        shell
            .members
            .iter()
            .map(|&vertex| f64::from(outcome.positions[vertex][0]))
            .sum::<f64>()
            / shell.members.len() as f64
    };
    assert!(
        seated_x(hands[0]) < 0.0,
        "+X hand must land on −X, got {}",
        seated_x(hands[0])
    );
    assert!(seated_x(hands[1]) > 0.0, "−X hand lands on +X");

    for (joint, hand) in outcome.report.joints.iter().zip(&hands) {
        let seat = joint.seat.expect("the wrist seated");
        assert!(
            (64.0..=65.8).contains(&seat.scale),
            "scale {} outside [64.0, 65.8]",
            seat.scale
        );
        let (angle, axis) = angle_axis(&seat.rotation);
        assert!(
            (174.0..=179.0).contains(&angle),
            "rotation angle {angle} outside [174, 179]"
        );
        assert!(axis[1].abs() > 0.98, "rotation axis {axis:?} not about ±Y");
        assert!(seat.gap < 0.5, "post-seat gap {}", seat.gap);
        assert_eq!(joint.bridge_triangles, 20, "10 ↔ 10 after the weld");
        assert_eq!(joint.edges_consumed, [10, 10]);
        assert_eq!(joint.boundary_ring_after, 0, "wrist ring closed");
        assert_eq!(joint.boundary_arc_after, 0, "cuff ring closed");
        assert!(joint.pinch_edges > 0, "the rim twins actually merged");
        // The falsifying check: the seated cuff hugs the wrist plane at
        // least as well as the socket's own planarity.
        assert!(
            seat.plane_dev <= 0.111149,
            "plane_dev {} over the socket planarity",
            seat.plane_dev
        );
        assert!(
            seat.max_residual < 0.25 * 2.2,
            "residual {}",
            seat.max_residual
        );
        // Ring geometry cannot verify the side: the report warns.
        assert_eq!(
            joint.handedness,
            "undecided: ring geometry cannot detect a swap; verify by sheet"
        );
        // Orientation witness: the seated hand hangs below the wrist
        // (fingertips y ≈ 57.4; an inverted seat leaves them at ≈ 72.5).
        let y_min = hand
            .members
            .iter()
            .map(|&vertex| outcome.positions[vertex][1])
            .fold(f32::INFINITY, f32::min);
        assert!(y_min < 60.0, "fingertips must hang below y=60, got {y_min}");
        // Wrist smoothing target: bounded displacement, no fold-back edge.
        assert!(
            joint.smoothing.max_displacement <= f64::from(joint.smoothing.max_shift),
            "displacement {} over bound {}",
            joint.smoothing.max_displacement,
            joint.smoothing.max_shift
        );
        assert!(
            joint.smoothing.max_dihedral_after_deg <= 150.0,
            "fold-back edge {}° over 150°",
            joint.smoothing.max_dihedral_after_deg
        );
        assert_eq!(joint.non_manifold_after, 0, "no edge above two faces");
    }
}
