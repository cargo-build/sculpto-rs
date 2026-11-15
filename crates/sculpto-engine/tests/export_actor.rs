//! `export.actor`: the reach split, the extremity fallback and the smoothed
//! seam, measured on hand-built geometry. @req DECL0008

use std::collections::BTreeMap;

use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_engine::export_actor::{BodySurface, PartBinder};
use sculpto_mesh::garment::Influences;

/// Root at the origin, a torso at z=1 and a head at z=5: the extremity bone sits
/// where a body's head does, far above the surface the copy can reach.
fn rig() -> Rig {
    let joint = |name: &str, parent: Option<u32>, translation: [f32; 3]| Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    };
    Rig::new(
        vec![
            joint("Root", None, [0.0, 0.0, 0.0]),
            joint("Torso", Some(0), [0.0, 0.0, 1.0]),
            joint("Head", Some(1), [0.0, 0.0, 5.0]),
        ],
        RigFrame::SourceZUp,
    )
    .expect("a valid source graph")
}

fn palette() -> Vec<String> {
    vec!["Root".into(), "Torso".into(), "Head".into()]
}

/// A body quad spanning z 0..1; every vertex is weighted to the torso.
fn body_rows() -> Vec<Influences> {
    let row: Influences = [("Torso".to_string(), 1.0)].into_iter().collect();
    vec![row; 4]
}

fn body() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [-1.0, 0.0, 1.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

/// A strip in the body's own plane, from the surface up to z=6.
///
/// Row z sits `z - 1` units from the body surface, so with a three-unit reach
/// the rows at z=4.5 and z=6 are the ones the copy cannot answer for.
fn part() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    for z in [0.0_f32, 1.5, 3.0, 4.5, 6.0] {
        positions.push([-1.0, 0.0, z]);
        positions.push([1.0, 0.0, z]);
    }
    let mut triangles = Vec::new();
    for row in 0..4_u32 {
        let base = row * 2;
        triangles.push([base, base + 1, base + 3]);
        triangles.push([base, base + 3, base + 2]);
    }
    (positions, triangles)
}

fn bind(
    fallback: &[usize],
    reach: f32,
    iterations: usize,
) -> sculpto_engine::export_actor::BoundPart {
    let (body_positions, body_triangles) = body();
    let rows = body_rows();
    let rig = rig();
    let palette = palette();
    let (positions, triangles) = part();
    PartBinder::new(
        &rig,
        &palette,
        fallback,
        BodySurface {
            positions: &body_positions,
            triangles: &body_triangles,
            rows: &rows,
        },
        iterations,
    )
    .bind(&positions, &triangles, reach)
    .expect("the portable part binds")
}

#[test]
fn reach_decides_between_copied_weights_and_the_extremity_bone() {
    let bound = bind(&[2], 3.0, 0);
    assert_eq!(bound.reached(), 6, "the rows at z 0, 1.5 and 3 copy");
    assert_eq!(bound.bound(), 4, "the rows at z 4.5 and 6 fall back");
    assert_eq!(bound.smoothed(), 0, "no smoothing was asked for");

    let rows = bound.rows();
    for (vertex, row) in rows.iter().enumerate().take(6) {
        assert_eq!(row.len(), 1, "vertex {vertex} copies one body bone");
        assert_eq!(row.get("Torso"), Some(&1.0), "vertex {vertex} copies Torso");
    }
    for (vertex, row) in rows.iter().enumerate().skip(6) {
        assert_eq!(
            row.get("Head"),
            Some(&1.0),
            "vertex {vertex} binds to the head bone, got {row:?}"
        );
        assert_eq!(row.len(), 1);
    }
}

#[test]
fn smoothing_blends_the_seam_without_moving_the_copied_rows() {
    let bound = bind(&[2], 3.0, 6);
    assert_eq!(bound.bound(), 4);
    assert_eq!(bound.smoothed(), 4, "every bound vertex was blended");
    let rows = bound.rows();

    // The copied rows keep the copy's exact values.
    for (vertex, row) in rows.iter().enumerate().take(6) {
        assert_eq!(row.get("Torso"), Some(&1.0), "vertex {vertex} stays copied");
    }
    // The seam row now carries both: a pure fallback row would tear.
    let seam = &rows[6];
    let head = seam.get("Head").copied().unwrap_or_default();
    let torso = seam.get("Torso").copied().unwrap_or_default();
    assert!(
        head > 0.4 && torso > 0.0,
        "the seam row is a blend, got {seam:?}"
    );
    assert!(
        (head + torso - 1.0).abs() <= 1e-3,
        "a smoothed row still sums to one, got {seam:?}"
    );
    // The far row is still dominated by the extremity bone.
    let far = &rows[8];
    assert!(
        far.get("Head").copied().unwrap_or_default() > head,
        "further from the seam the fallback dominates: {far:?} vs {seam:?}"
    );
}

#[test]
fn a_reach_that_covers_the_part_binds_nothing() {
    let bound = bind(&[2], 10.0, 6);
    assert_eq!(bound.bound(), 0);
    assert_eq!(bound.smoothed(), 0);
    assert!(bound.rows().iter().all(|row| row.len() == 1));
}

#[test]
fn an_unreached_vertex_with_no_fallback_refuses_with_its_measure() {
    let (body_positions, body_triangles) = body();
    let rows = body_rows();
    let rig = rig();
    let palette = palette();
    let (positions, triangles) = part();
    let refused = PartBinder::new(
        &rig,
        &palette,
        &[],
        BodySurface {
            positions: &body_positions,
            triangles: &body_triangles,
            rows: &rows,
        },
        0,
    )
    .bind(&positions, &triangles, 3.0)
    .expect_err("four vertices have nowhere to go");
    assert_eq!(refused.code(), "export_actor_unreachable");
    let message = format!("{refused}");
    assert!(
        message.contains("4 of 10"),
        "the refusal names the measured count: {message}"
    );
}

#[test]
fn a_palette_slot_is_not_a_rig_index() {
    // The palette keeps only weighted bones and their ancestors, so its slots do
    // not line up with the rig's joints. A joint the palette skips sits far above
    // both, where a slot read as a joint index would win.
    let joint = |name: &str, parent: Option<u32>, translation: [f32; 3]| Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    };
    let rig = Rig::new(
        vec![
            joint("Root", None, [0.0, 0.0, 0.0]),
            joint("Skipped", Some(0), [0.0, 0.0, 50.0]),
            joint("Torso", Some(0), [0.0, 0.0, 1.0]),
            joint("Head", Some(2), [0.0, 0.0, 5.0]),
        ],
        RigFrame::SourceZUp,
    )
    .expect("a valid source graph");
    let palette = vec!["Root".to_string(), "Torso".to_string(), "Head".to_string()];
    let (body_positions, body_triangles) = body();
    let rows = body_rows();
    let (positions, triangles) = part();
    let bound = PartBinder::new(
        &rig,
        &palette,
        &[1, 2],
        BodySurface {
            positions: &body_positions,
            triangles: &body_triangles,
            rows: &rows,
        },
        0,
    )
    .bind(&positions, &triangles, 3.0)
    .expect("the portable part binds");
    for (vertex, row) in bound.rows().iter().enumerate().skip(6) {
        assert_eq!(
            row.get("Head"),
            Some(&1.0),
            "vertex {vertex} must bind to the head joint by name, got {row:?}"
        );
    }
}

#[test]
fn the_nearest_fallback_bone_wins() {
    // With both the torso and the head declared, the far rows still prefer the
    // head, whose origin is nearer to them than the torso's.
    let bound = bind(&[1, 2], 3.0, 0);
    let rows = bound.rows();
    for (vertex, row) in rows.iter().enumerate().skip(6) {
        assert_eq!(
            row.get("Head"),
            Some(&1.0),
            "vertex {vertex} is nearer the head bone, got {row:?}"
        );
    }
    let counts: BTreeMap<&str, usize> = bound
        .fallback()
        .iter()
        .map(|(bone, vertices)| (bone.as_str(), *vertices))
        .collect();
    assert_eq!(counts.get("Head"), Some(&4));
}

#[test]
fn a_layer_welds_its_panels_but_keeps_an_authored_dart() {
    // Panel A is triangle [0,1,2]; panel B is [3,4,5], [5,7,6] and [3,5,8],
    // joined through shared indices. Vertices 1 and 4 coincide across the two
    // panels — the seam merges them. Vertices 6 and 8 coincide *inside* panel
    // B: an authored dart the weld must leave open.
    use sculpto_engine::export_actor::weld_layer;
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [2.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [2.0, 2.0, 0.0],
        [1.0, 1.0, 0.0],
        [2.0, 2.0, 0.0],
    ];
    let triangles = [[0_u32, 1, 2], [3, 4, 5], [5, 7, 6], [3, 5, 8]];
    let (welded, welded_triangles, report) = weld_layer(&positions, &triangles);

    assert_eq!(report.components_before, 2, "the panels start separate");
    assert_eq!(report.components_after, 1, "the shared corner joins them");
    assert_eq!(welded.len(), 8, "one twin merges, the dart copy stays");
    assert_eq!(welded.len(), report.welded_vertices);
    assert_eq!(welded_triangles.len(), 4, "no face collapses");
    assert_eq!(report.dropped_faces, 0);
    assert_eq!(report.merged_pairs, 1, "only the cross-panel pair merges");
    assert_eq!(report.vertex_map[1], report.vertex_map[4]);
    assert_ne!(
        report.vertex_map[6], report.vertex_map[8],
        "the intra-panel dart stays open"
    );
}
