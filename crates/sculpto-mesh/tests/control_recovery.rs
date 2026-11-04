//! Source-disk recovery: an original omitted disk re-enters the control only when it
//! closes one incidental loop exactly, and every other shape keeps the native path.

use std::collections::HashSet;

use sculpto_mesh::control::measure::CoverageLimits;
use sculpto_mesh::control::profile::{ControlFrame, ControlProfile};
use sculpto_mesh::control::{
    ContactMode, ContactPolicy, ControlError, Guide, GuideSegment, RecoveryRefusal,
    ReductionLimits, RepairLimits, SourceRecovery, SourceSurface, SurfacePolicy, exterior_control,
    recovery::recover_disk, source_selection,
};

/// A closed unit box in source coordinates: walls, top cap, and the corners the
/// bottom cap would use.
fn box_shell() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
        // The bottom cap keeps its own vertices; their positions alias the corners,
        // so recovery must map them back to the first-use source vertices.
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let triangles = vec![
        [8, 11, 10],
        [8, 10, 9],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
        [4, 5, 6],
        [4, 6, 7],
    ];
    (positions, triangles)
}

/// The box with an annular skirt instead of a bottom cap: the omitted component has
/// an extra boundary, so it cannot be a recovered disk.
fn box_with_annulus() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = box_shell();
    // Remove the bottom cap (the first two faces) and replace it with an annulus
    // whose outer ring is the kept bottom rim, wound opposite to the kept faces.
    triangles.drain(0..2);
    positions.truncate(8);
    positions.extend([
        [0.25, 0.25, 0.0],
        [0.75, 0.25, 0.0],
        [0.75, 0.75, 0.0],
        [0.25, 0.75, 0.0],
    ]);
    triangles.extend([
        [1, 0, 8],
        [1, 8, 9],
        [2, 1, 9],
        [2, 9, 10],
        [3, 2, 10],
        [3, 10, 11],
        [0, 3, 11],
        [0, 11, 8],
    ]);
    (positions, triangles)
}

fn profile(recovery: SourceRecovery) -> ControlProfile {
    ControlProfile {
        frame: ControlFrame::SourceLocalZUp,
        guide: Guide::new([GuideSegment {
            start: [0.5, 0.5, -1.0],
            end: [0.5, 0.5, 2.0],
        }])
        .unwrap(),
        surface: SurfacePolicy::Guide,
        apertures: vec![sculpto_mesh::control::ApertureSpec {
            role: "waist".to_string(),
            anchor: [0.5, 0.5, 1.0],
            radius: 0.1,
        }],
        source_repair: RepairLimits {
            max_incidental_loops: 2,
            collapse_below_edges: 0,
            max_fill_edges: 32,
            max_tip_removal_share: 0.05,
        },
        reduced_repair: RepairLimits {
            max_incidental_loops: 2,
            collapse_below_edges: 10,
            max_fill_edges: 32,
            max_tip_removal_share: 0.05,
        },
        reduction: ReductionLimits {
            target_triangles: 10,
            minimum_triangles: 4,
            maximum_triangles: 140,
        },
        coverage: CoverageLimits {
            tolerance: 0.01,
            minimum: 0.5,
        },
        contact: ContactPolicy {
            mode: ContactMode::Reject,
            clearance: 1e-6,
            max_displacement: 1e-3,
            max_pairs: 1,
        },
        source_cuts: Vec::new(),
        source_recovery: recovery,
        expected_genus: 0,
    }
}

/// The output triangle behind one selected source face.
fn triangle_of<'a>(selected_faces: &[u32], triangles: &'a [[u32; 3]], face: u32) -> &'a [u32; 3] {
    let index = selected_faces
        .iter()
        .position(|selected| *selected == face)
        .unwrap_or_else(|| panic!("face {face} is not selected"));
    &triangles[index]
}

#[test]
fn a_recovered_disk_keeps_its_original_faces_corners_and_first_use_vertices() {
    let (positions, triangles) = box_shell();
    let declared = profile(SourceRecovery::IncidentalDisks);
    // The recovered disk closes the bottom rim, so the surface keeps one opening.
    let outcome = exterior_control(&positions, &triangles, &declared).unwrap();

    assert_eq!(outcome.recovery.disks.len(), 1);
    assert!(outcome.recovery.refused.is_empty());
    let disk = &outcome.recovery.disks[0];
    assert_eq!(disk.faces.len(), 2, "the bottom cap is two original faces");
    assert_eq!(disk.boundary_vertices.len(), 4);
    assert_eq!(disk.vertices.len(), 4);
    assert_eq!(disk.edges, 5, "four rim edges and the cap diagonal");
    assert_eq!(disk.euler, 1, "a recovered component must be a disk");

    // Original corners and positions: every recovered face resolves to the four
    // bottom corners of the original shell, with their exact coordinates.
    let bottom = [
        [0.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let mut recovered_corners = Vec::new();
    for face in &disk.faces {
        let triangle = triangle_of(&outcome.selected_faces, &outcome.triangles, *face);
        for corner in triangle {
            let point = outcome.positions[*corner as usize];
            assert!(
                bottom.contains(&point),
                "recovered corner {point:?} is not an original bottom corner"
            );
            recovered_corners.push(point);
        }
        assert!(
            !outcome
                .removals
                .iter()
                .any(|removal| removal.triangle == *face),
            "a recovered face must not be reported removed"
        );
    }
    recovered_corners.sort_by(|left, right| left.partial_cmp(right).unwrap());
    recovered_corners.dedup();
    assert_eq!(
        recovered_corners.len(),
        4,
        "the disk spans all four corners"
    );

    // Seam mapping: the duplicated cap vertices are the same positions as the kept
    // corners, and the lineage keeps the first-use source vertex.
    for (index, point) in outcome.positions.iter().enumerate() {
        if let Some(position) = bottom.iter().position(|corner| corner == point) {
            assert_eq!(
                outcome.vertex_sources[index] as usize, position,
                "position {point:?} must keep its first-use source vertex"
            );
        }
    }

    // Composition accounting: recovered faces count as kept, only the aperture cap
    // is removed, and the surface now has one opening.
    assert_eq!(outcome.composition.source_triangles, 12);
    assert_eq!(outcome.composition.exterior_triangles, 10);
    assert_eq!(outcome.composition.interior_removed, 2);
    assert_eq!(outcome.composition.recovered_triangles, 2);
    assert_eq!(outcome.recovery.recovered_faces, 2);
    assert_eq!(outcome.loops.len(), 1);
    assert_eq!(outcome.apertures.matched.len(), 1);
    assert_eq!(outcome.apertures.matched[0].role, "waist");
    assert!(
        outcome.actions.is_empty(),
        "the recovered disk leaves no closure to plan: {:?}",
        outcome.actions
    );

    // The coverage path sees the same selection, so it measures the recovered faces.
    let selection = source_selection(&positions, &triangles, &declared).unwrap();
    assert_eq!(selection.recovery, outcome.recovery);
    assert_eq!(selection.selected_faces, outcome.selected_faces);
}

#[test]
fn the_published_route_still_plans_a_fill_for_the_same_loop() {
    let (positions, triangles) = box_shell();
    let outcome = exterior_control(&positions, &triangles, &profile(SourceRecovery::None)).unwrap();
    assert!(outcome.recovery.disks.is_empty());
    assert!(outcome.recovery.refused.is_empty());
    assert_eq!(outcome.composition.interior_removed, 4);
    assert_eq!(outcome.composition.exterior_triangles, 8);
    assert_eq!(outcome.composition.recovered_triangles, 0);
    assert_eq!(outcome.loops.len(), 2);
    match outcome.actions.as_slice() {
        [sculpto_mesh::control::RepairAction::Fill { edges, .. }] => {
            assert_eq!(*edges, 4, "the omitted bottom cap leaves a four-edge loop")
        }
        other => panic!("expected one planned fill, got {other:?}"),
    }
}

#[test]
fn an_omitted_component_with_an_extra_boundary_keeps_the_native_path() {
    let (positions, triangles) = box_with_annulus();
    let outcome = exterior_control(
        &positions,
        &triangles,
        &profile(SourceRecovery::IncidentalDisks),
    )
    .unwrap();
    assert!(outcome.recovery.disks.is_empty());
    assert_eq!(outcome.recovery.refused.len(), 1);
    let (loop_index, refusal) = &outcome.recovery.refused[0];
    assert_eq!(
        refusal,
        &RecoveryRefusal::BoundaryMismatch {
            extra: 4,
            missing: 0
        },
        "the annular skirt carries a second boundary"
    );
    assert_eq!(outcome.loops[*loop_index].edges, 4);
    assert_eq!(
        outcome.actions.len(),
        1,
        "the loop is still filled natively"
    );
    assert_eq!(outcome.composition.recovered_triangles, 0);
}

/// Kept square plus the disk behind its boundary, as a direct recovery fixture.
fn double_sheet() -> (SourceSurface, HashSet<u32>, Vec<u32>) {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let triangles = vec![[0, 1, 2], [0, 2, 3], [1, 3, 2], [1, 0, 3]];
    let surface = SourceSurface::identify(&positions, &triangles).unwrap();
    let kept = [0u32, 1].into_iter().collect::<HashSet<_>>();
    (surface, kept, vec![0, 1, 2, 3])
}

#[test]
fn a_disk_touching_a_named_aperture_is_refused() {
    let (surface, kept, boundary) = double_sheet();
    let aperture_vertices = [1u32].into_iter().collect::<HashSet<_>>();
    assert_eq!(
        recover_disk(
            &surface,
            &HashSet::new(),
            &kept,
            0,
            &boundary,
            true,
            &aperture_vertices
        ),
        Err(RecoveryRefusal::TouchesAperture { vertex: 1 })
    );
    // The same component qualifies when it touches no named opening.
    let disk = recover_disk(
        &surface,
        &HashSet::new(),
        &kept,
        0,
        &boundary,
        true,
        &HashSet::new(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(disk.faces, vec![2, 3]);
    assert_eq!(disk.vertices, vec![0, 1, 2, 3]);
    assert_eq!(disk.boundary_vertices, vec![0, 1, 2, 3]);
}

#[test]
fn inconsistent_winding_and_non_manifold_faces_are_refused() {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let kept = [0u32, 1].into_iter().collect::<HashSet<_>>();
    let boundary = vec![0, 1, 2, 3];
    let empty = HashSet::new();

    // The second cap face is wound the same way as the kept square, so the shared
    // boundary edge has two faces traversing it in one direction.
    let folded = vec![[0, 1, 2], [0, 2, 3], [1, 3, 2], [1, 3, 0]];
    let surface = SourceSurface::identify(&positions, &folded).unwrap();
    assert_eq!(
        recover_disk(&surface, &empty, &kept, 0, &boundary, true, &empty),
        Err(RecoveryRefusal::WindingConflict { edge: [0, 1] })
    );

    // A third face on the shared rim edge is a non-manifold edge, not a disk.
    let triple = vec![[0, 1, 2], [0, 2, 3], [1, 3, 2], [1, 0, 3], [1, 0, 2]];
    let surface = SourceSurface::identify(&positions, &triple).unwrap();
    assert_eq!(
        recover_disk(&surface, &empty, &kept, 0, &boundary, true, &empty),
        Err(RecoveryRefusal::NonManifoldEdge {
            edge: [0, 1],
            faces: 3
        })
    );
}

#[test]
fn a_loop_with_nothing_behind_it_is_not_a_recovery_case() {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let surface = SourceSurface::identify(&positions, &[[0, 1, 2], [0, 2, 3]]).unwrap();
    let kept = [0u32, 1].into_iter().collect::<HashSet<_>>();
    assert_eq!(
        recover_disk(
            &surface,
            &HashSet::new(),
            &kept,
            0,
            &[0, 1, 2, 3],
            true,
            &HashSet::new()
        ),
        Ok(None),
        "an open wound is filled natively, not recovered"
    );
}

#[test]
fn an_ineligible_loop_keeps_the_existing_refusal_path() {
    let (positions, triangles) = box_shell();
    let mut profile = profile(SourceRecovery::IncidentalDisks);
    // Above `max_fill_edges` the source stage refuses the loop before recovery.
    profile.source_repair.max_fill_edges = 3;
    match exterior_control(&positions, &triangles, &profile) {
        Err(ControlError::FillLimit {
            edges: 4, limit: 3, ..
        }) => {}
        other => panic!("expected the unchanged fill refusal, got {other:?}"),
    }
    assert!(
        profile.source_repair.max_fill_edges < 4,
        "recovery eligibility follows the same declared limit"
    );
}
