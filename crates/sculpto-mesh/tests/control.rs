//! Characterization of the source-guided exterior rule on synthetic thick shells.

use sculpto_mesh::control::measure::{ControlDiagnostics, CoverageLimits};
use sculpto_mesh::control::profile::{ControlFrame, ControlProfile};
use sculpto_mesh::control::{
    ApertureResolution, ApertureSpec, BoundaryLoop, ContactMode, ContactPolicy, ControlError,
    Guide, GuideSegment, ReductionLimits, RepairAction, RepairLimits, boundary_loops,
    exterior_control, largest_component, non_manifold_tips, repair_control, resolve_apertures,
};

/// A closed thick shell plus the counts its construction is made of.
struct Shell {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    /// Centre of the first station's ring pair.
    start: [f32; 3],
    /// Centre of the last station's ring pair.
    end: [f32; 3],
    outer_triangles: usize,
    inner_triangles: usize,
    rim_triangles: usize,
    /// Straight-line end frames, so a guide can extend past the rims.
    start_frame: [f32; 3],
    end_frame: [f32; 3],
}

fn subtract(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn scale(value: [f32; 3], factor: f32) -> [f32; 3] {
    value.map(|axis| axis * factor)
}

fn cross(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2]).sqrt();
    value.map(|axis| axis / length)
}

/// A closed double-walled tube along `path`: outer wall, inner lining and rims.
fn shell(path: &[[f32; 3]], outer: f32, inner: f32, sides: usize) -> Shell {
    let stations = path.len();
    let tangents = (0..stations)
        .map(|index| match index {
            0 => normalize(subtract(path[1], path[0])),
            last if last == stations - 1 => normalize(subtract(path[last], path[last - 1])),
            middle => normalize(subtract(path[middle + 1], path[middle - 1])),
        })
        .collect::<Vec<_>>();
    let frames = tangents
        .iter()
        .map(|tangent| {
            let reference = [0.0, 0.0, 1.0];
            let candidate = cross(*tangent, reference);
            let length = candidate[0] * candidate[0]
                + candidate[1] * candidate[1]
                + candidate[2] * candidate[2];
            let u = match length > 1e-6 {
                true => normalize(candidate),
                false => [1.0, 0.0, 0.0],
            };
            (u, cross(*tangent, u))
        })
        .collect::<Vec<_>>();
    let ring = |station: usize, radius: f32, (u, v): ([f32; 3], [f32; 3])| {
        (0..sides)
            .map(|side| {
                let angle = std::f32::consts::TAU * side as f32 / sides as f32;
                add(
                    path[station],
                    add(
                        scale(u, radius * angle.cos()),
                        scale(v, radius * angle.sin()),
                    ),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut positions = Vec::new();
    let mut outer_index = Vec::new();
    let mut inner_index = Vec::new();
    for (station, frame) in frames.iter().enumerate().take(stations) {
        outer_index.push(
            ring(station, outer, *frame)
                .into_iter()
                .map(|point| {
                    positions.push(point);
                    (positions.len() - 1) as u32
                })
                .collect::<Vec<_>>(),
        );
        inner_index.push(
            ring(station, inner, *frame)
                .into_iter()
                .map(|point| {
                    positions.push(point);
                    (positions.len() - 1) as u32
                })
                .collect::<Vec<_>>(),
        );
    }
    let mut triangles = Vec::new();
    for station in 0..stations - 1 {
        for side in 0..sides {
            let next = (side + 1) % sides;
            triangles.push([
                outer_index[station][side],
                outer_index[station][next],
                outer_index[station + 1][next],
            ]);
            triangles.push([
                outer_index[station][side],
                outer_index[station + 1][next],
                outer_index[station + 1][side],
            ]);
        }
    }
    let outer_triangles = triangles.len();
    for station in 0..stations - 1 {
        for side in 0..sides {
            let next = (side + 1) % sides;
            triangles.push([
                inner_index[station][side],
                inner_index[station + 1][next],
                inner_index[station][next],
            ]);
            triangles.push([
                inner_index[station][side],
                inner_index[station + 1][side],
                inner_index[station + 1][next],
            ]);
        }
    }
    let inner_triangles = triangles.len() - outer_triangles;
    for side in 0..sides {
        let next = (side + 1) % sides;
        let last = stations - 1;
        triangles.push([
            outer_index[0][side],
            inner_index[0][side],
            inner_index[0][next],
        ]);
        triangles.push([
            outer_index[0][side],
            inner_index[0][next],
            outer_index[0][next],
        ]);
        triangles.push([
            outer_index[last][side],
            inner_index[last][next],
            inner_index[last][side],
        ]);
        triangles.push([
            outer_index[last][side],
            outer_index[last][next],
            inner_index[last][next],
        ]);
    }
    let rim_triangles = triangles.len() - outer_triangles - inner_triangles;
    Shell {
        positions,
        triangles,
        start: path[0],
        end: path[stations - 1],
        outer_triangles,
        inner_triangles,
        rim_triangles,
        start_frame: tangents[0],
        end_frame: tangents[stations - 1],
    }
}

fn straight_shell() -> Shell {
    shell(
        &[
            [0.0, 0.0, -0.5],
            [0.0, 0.0, -0.15],
            [0.0, 0.0, 0.15],
            [0.0, 0.0, 0.5],
        ],
        0.20,
        0.12,
        8,
    )
}

/// The guide runs past both rims so the rims sit at a nonzero axial offset.
fn shell_guide(shell: &Shell) -> Guide {
    Guide::new([
        GuideSegment {
            start: add(shell.start, scale(shell.start_frame, -0.25)),
            end: shell.start,
        },
        GuideSegment {
            start: shell.start,
            end: shell.end,
        },
        GuideSegment {
            start: shell.end,
            end: add(shell.end, scale(shell.end_frame, 0.25)),
        },
    ])
    .unwrap()
}

fn aperture(role: &str, anchor: [f32; 3]) -> ApertureSpec {
    ApertureSpec {
        role: role.to_string(),
        anchor,
        radius: 0.06,
    }
}

fn profile(shell: &Shell) -> ControlProfile {
    ControlProfile {
        frame: ControlFrame::SourceLocalZUp,
        guide: shell_guide(shell),
        apertures: vec![aperture("hem", shell.start), aperture("collar", shell.end)],
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
            target_triangles: 120,
            minimum_triangles: 20,
            maximum_triangles: 140,
        },
        coverage: CoverageLimits {
            tolerance: 0.01,
            minimum: 0.9,
        },
        contact: ContactPolicy {
            mode: ContactMode::Reject,
            clearance: 1e-6,
            max_displacement: 1e-3,
            max_pairs: 1,
        },
    }
}

#[test]
fn the_synthetic_shell_is_closed_before_reconstruction() {
    let shell = straight_shell();
    assert_eq!(
        shell.triangles.len(),
        shell.outer_triangles + shell.inner_triangles + shell.rim_triangles
    );
    let loops = boundary_loops(&shell.positions, &shell.triangles);
    assert!(
        loops.is_empty(),
        "fixture must be a closed shell, got {loops:?}"
    );
    let diagnostics = ControlDiagnostics::measure(&shell.positions, &shell.triangles).unwrap();
    assert_eq!(diagnostics.components, 1);
    assert_eq!(diagnostics.non_manifold_edges, 0);
    assert_eq!(diagnostics.self_contacts, 0);
}

#[test]
fn thick_shell_keeps_only_the_outward_wall() {
    let shell = straight_shell();
    let profile = profile(&shell);
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile).unwrap();
    assert_eq!(outcome.composition.source_triangles, shell.triangles.len());
    assert_eq!(
        outcome.composition.exterior_triangles, shell.outer_triangles,
        "the inward wall and both rims are not cloth"
    );
    assert_eq!(
        outcome.composition.interior_removed,
        shell.inner_triangles + shell.rim_triangles
    );
    assert_eq!(outcome.composition.coincident_vertices, 0);
    assert_eq!(outcome.loops.len(), 2);
    assert!(outcome.loops.iter().all(|boundary| boundary.closed));
    assert_eq!(outcome.apertures.matched.len(), 2);
    assert!(outcome.apertures.incidental.is_empty());

    let diagnostics = ControlDiagnostics::measure(&outcome.positions, &outcome.triangles).unwrap();
    assert_eq!(diagnostics.loops.len(), 2);
    assert_eq!(diagnostics.euler, 0);
    assert_eq!(diagnostics.components, 1);
    assert_eq!(diagnostics.non_manifold_vertices, 0);
    assert_eq!(diagnostics.non_manifold_edges, 0);
    assert_eq!(diagnostics.self_contacts, 0);
    let qualification = diagnostics.qualify(&profile).unwrap();
    assert_eq!(qualification.apertures.len(), 2);
    assert_eq!(qualification.incidental, 0);
}

#[test]
fn a_guide_branch_outside_the_shell_changes_nothing() {
    let shell = straight_shell();
    let base = profile(&shell);
    let mut segments = base.guide.segments().to_vec();
    segments.push(GuideSegment {
        start: [2.0, 0.0, 0.0],
        end: [3.0, 0.0, 0.0],
    });
    let branched = ControlProfile {
        guide: Guide::new(segments).unwrap(),
        ..base.clone()
    };
    let plain = exterior_control(&shell.positions, &shell.triangles, &base).unwrap();
    let branch = exterior_control(&shell.positions, &shell.triangles, &branched).unwrap();
    assert_eq!(plain, branch);
}

#[test]
fn the_inward_wall_never_becomes_a_second_shell() {
    let shell = straight_shell();
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile(&shell)).unwrap();
    assert_eq!(outcome.composition.island_components_removed, 0);
    assert_eq!(outcome.composition.island_triangles_removed, 0);
    assert_eq!(outcome.composition.tip_triangles_removed, 0);
    let diagnostics = ControlDiagnostics::measure(&outcome.positions, &outcome.triangles).unwrap();
    assert_eq!(diagnostics.components, 1);
}

#[test]
fn reconstruction_is_deterministic_and_keeps_source_lineage() {
    let shell = straight_shell();
    let profile = profile(&shell);
    let first = exterior_control(&shell.positions, &shell.triangles, &profile).unwrap();
    let second = exterior_control(&shell.positions, &shell.triangles, &profile).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.vertex_sources.len(), first.positions.len());
    assert!(
        first
            .vertex_sources
            .iter()
            .all(|source| (*source as usize) < shell.positions.len())
    );
    let kept = first
        .vertex_sources
        .iter()
        .enumerate()
        .map(|(index, source)| (index as u32, shell.positions[*source as usize]))
        .collect::<Vec<_>>();
    assert!(
        kept.iter()
            .all(|(index, point)| first.positions[*index as usize] == *point),
        "each kept vertex keeps the source position it came from"
    );
}

#[test]
fn translated_and_scaled_shells_keep_the_same_decisions() {
    let shell = straight_shell();
    let base = profile(&shell);
    let offset = [5.0, -3.0, 2.0];
    let factor = 2.5;
    let moved_positions = shell
        .positions
        .iter()
        .map(|point| add(scale(*point, factor), offset))
        .collect::<Vec<_>>();
    let moved_guide = Guide::new(base.guide.segments().iter().map(|segment| GuideSegment {
        start: add(scale(segment.start, factor), offset),
        end: add(scale(segment.end, factor), offset),
    }))
    .unwrap();
    let moved = ControlProfile {
        guide: moved_guide,
        apertures: base
            .apertures
            .iter()
            .map(|spec| ApertureSpec {
                role: spec.role.clone(),
                anchor: add(scale(spec.anchor, factor), offset),
                radius: spec.radius * factor,
            })
            .collect(),
        ..base.clone()
    };
    let original = exterior_control(&shell.positions, &shell.triangles, &base).unwrap();
    let translated = exterior_control(&moved_positions, &shell.triangles, &moved).unwrap();
    assert_eq!(original.triangles, translated.triangles);
    assert_eq!(original.composition, translated.composition);
    assert_eq!(original.loops.len(), translated.loops.len());
    assert_eq!(
        original
            .apertures
            .matched
            .iter()
            .map(|matched| matched.role.clone())
            .collect::<Vec<_>>(),
        translated
            .apertures
            .matched
            .iter()
            .map(|matched| matched.role.clone())
            .collect::<Vec<_>>()
    );
    let extent = ControlDiagnostics::measure(&translated.positions, &translated.triangles)
        .unwrap()
        .extent();
    assert!(
        (extent[2] / factor - 1.0).abs() < 1e-5,
        "height scales with the input"
    );
}

#[test]
fn corner_flare_fans_keep_the_largest_group() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, -1.0],
    ];
    let triangles = [
        [0, 1, 2],
        [0, 2, 3],
        [0, 3, 1],
        [0, 4, 5],
        [0, 5, 6],
        [0, 6, 4],
    ];
    let prune = non_manifold_tips(&triangles);
    assert_eq!(prune.vertices, vec![0]);
    assert_eq!(prune.removed, vec![3, 4, 5]);
    let survivors = triangles
        .iter()
        .enumerate()
        .filter(|(index, _)| !prune.removed.contains(&(*index as u32)))
        .map(|(_, triangle)| *triangle)
        .collect::<Vec<_>>();
    assert_eq!(largest_component(&survivors).removed_components, 0);
    assert_eq!(positions.len(), 7);
}

#[test]
fn aperture_identification_refuses_an_ambiguous_radius() {
    let shell = straight_shell();
    let mut profile = profile(&shell);
    profile.apertures = vec![ApertureSpec {
        role: "opening".to_string(),
        anchor: shell.start,
        radius: 2.0,
    }];
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile);
    assert_eq!(
        outcome,
        Err(ControlError::AmbiguousAperture {
            role: "opening".to_string(),
            candidates: 2,
        }),
        "two openings inside one declared radius are ambiguous, never merged"
    );
}

#[test]
fn a_lost_opening_is_reported_instead_of_being_filled() {
    let shell = straight_shell();
    let mut profile = profile(&shell);
    profile
        .apertures
        .push(aperture("left cuff", [0.0, 0.0, 0.0]));
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile);
    assert_eq!(
        outcome,
        Err(ControlError::UnknownAperture {
            role: "left cuff".to_string(),
            radius: 0.06,
        })
    );
}

#[test]
fn invalid_sources_are_rejected_before_any_decision() {
    let shell = straight_shell();
    let profile = profile(&shell);

    let mut nonfinite = shell.positions.clone();
    nonfinite[0][1] = f32::NAN;
    assert!(matches!(
        exterior_control(&nonfinite, &shell.triangles, &profile),
        Err(ControlError::NonFiniteVertex { vertex: 0 })
    ));

    let mut bad = shell.triangles.clone();
    bad[0][2] = 9999;
    assert!(matches!(
        exterior_control(&shell.positions, &bad, &profile),
        Err(ControlError::BadIndex { triangle: 0, .. })
    ));

    let collapsed = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    assert_eq!(
        exterior_control(&collapsed, &[[0, 1, 2]], &profile),
        Err(ControlError::CollapsedTriangle { triangle: 0 })
    );

    assert_eq!(
        exterior_control(&[], &shell.triangles, &profile),
        Err(ControlError::EmptyMesh)
    );
    assert_eq!(
        exterior_control(&shell.positions, &[], &profile),
        Err(ControlError::NoTriangles)
    );
}

#[test]
fn invalid_guides_are_rejected_before_any_decision() {
    assert_eq!(Guide::new([]), Err(ControlError::EmptyGuide));
    assert_eq!(
        Guide::new([GuideSegment {
            start: [0.0, 0.0, 0.0],
            end: [0.0, 0.0, 0.0],
        }]),
        Err(ControlError::DegenerateSegment { segment: 0 })
    );
    assert_eq!(
        Guide::new([GuideSegment {
            start: [0.0, 0.0, 0.0],
            end: [f32::INFINITY, 0.0, 0.0],
        }]),
        Err(ControlError::NonFiniteSegment { segment: 0 })
    );
}

#[test]
fn a_centroid_on_the_guide_has_no_outward_side() {
    let positions = vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 2.0, 0.0]];
    let guide = Guide::new([GuideSegment {
        start: [0.0, 0.0, -1.0],
        end: [0.0, 0.0, 1.0],
    }])
    .unwrap();
    let profile = ControlProfile {
        frame: ControlFrame::SourceLocalZUp,
        guide,
        apertures: vec![aperture("hem", [0.0, 0.0, 0.0])],
        source_repair: RepairLimits::default(),
        reduced_repair: RepairLimits::default(),
        reduction: ReductionLimits {
            target_triangles: 10,
            minimum_triangles: 5,
            maximum_triangles: 20,
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
    };
    assert_eq!(
        exterior_control(&positions, &[[0, 1, 2]], &profile),
        Err(ControlError::AmbiguousOrientation {
            triangles: 1,
            first: 0,
        })
    );
}

#[test]
fn a_harness_free_surface_keeps_its_declared_openings() {
    let shell = straight_shell();
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile(&shell)).unwrap();
    let diagnostics = ControlDiagnostics::measure(&outcome.positions, &outcome.triangles).unwrap();
    let ApertureResolution {
        matched,
        incidental,
    } = diagnostics.apertures(&profile(&shell)).unwrap();
    assert_eq!(matched.len(), 2);
    assert!(incidental.is_empty());
}

#[test]
fn repair_collapses_a_small_loop_and_fills_a_large_one() {
    let shell = straight_shell();
    let profile = profile(&shell);
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile).unwrap();
    // Punch one interior wall triangle: its three edges stay a closed loop.
    let mut holed = outcome.triangles.clone();
    let interior = 2 * 8;
    let removed = holed.remove(interior);
    assert!(removed.iter().all(|vertex| *vertex != 0));
    assert!(holed.iter().all(|triangle| *triangle != removed));
    let repair = repair_control(&outcome.positions, &holed, &profile).unwrap();
    assert_eq!(repair.apertures.matched.len(), 2);
    assert_eq!(repair.apertures.incidental.len(), 1);
    match repair.actions.as_slice() {
        [RepairAction::Collapse { edges, .. }] => assert_eq!(*edges, 3),
        other => panic!("expected one collapse, got {other:?}"),
    }
    let small = ControlProfile {
        reduced_repair: RepairLimits {
            collapse_below_edges: 2,
            max_fill_edges: 32,
            ..profile.reduced_repair
        },
        ..profile.clone()
    };
    let filled = repair_control(&outcome.positions, &holed, &small).unwrap();
    match filled.actions.as_slice() {
        [RepairAction::Fill { edges, .. }] => assert_eq!(*edges, 3),
        other => panic!("expected one fill, got {other:?}"),
    }
    let mut strict = profile;
    strict.reduced_repair.max_incidental_loops = 0;
    assert_eq!(
        repair_control(&outcome.positions, &holed, &strict).err(),
        Some(ControlError::IncidentalLimit { loops: 1, limit: 0 })
    );
}

#[test]
fn reduction_bounds_are_enforced_by_qualification() {
    use sculpto_mesh::control::ReductionPlan;
    let shell = straight_shell();
    let base = profile(&shell);
    let outcome = exterior_control(&shell.positions, &shell.triangles, &base).unwrap();
    let diagnostics = ControlDiagnostics::measure(&outcome.positions, &outcome.triangles).unwrap();
    let triangles = diagnostics.triangles;
    diagnostics
        .qualify(&base)
        .expect("the fixture is inside its declared bounds");

    let plan = ReductionPlan::plan(400, &base.reduction).unwrap();
    assert_eq!(plan.ratio, 120.0 / 400.0);
    assert_eq!(plan.removed, 280);

    let below = ControlProfile {
        reduction: ReductionLimits {
            target_triangles: triangles + 10,
            minimum_triangles: triangles + 1,
            maximum_triangles: triangles + 50,
        },
        ..base.clone()
    };
    assert_eq!(
        diagnostics.qualify(&below),
        Err(ControlError::ReductionBelowMinimum {
            triangles,
            minimum: triangles + 1,
        })
    );
    let above = ControlProfile {
        reduction: ReductionLimits {
            target_triangles: triangles.saturating_sub(5),
            minimum_triangles: triangles.saturating_sub(10),
            maximum_triangles: triangles.saturating_sub(1),
        },
        ..base.clone()
    };
    assert_eq!(
        diagnostics.qualify(&above),
        Err(ControlError::ReductionAboveMaximum {
            triangles,
            maximum: triangles - 1,
        })
    );
    assert_eq!(
        ReductionPlan::plan(
            400,
            &ReductionLimits {
                target_triangles: 10,
                minimum_triangles: 20,
                maximum_triangles: 30,
            }
        ),
        Err(ControlError::InvalidReductionLimits {
            minimum: 20,
            target: 10,
            maximum: 30,
        })
    );
    assert_eq!(
        ReductionPlan::plan(30, &base.reduction).unwrap().ratio,
        1.0,
        "a surface at or below target is not reduced"
    );
}

#[test]
fn loop_walks_report_openings_in_a_stable_order() {
    let shell = straight_shell();
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile(&shell)).unwrap();
    let loops: Vec<BoundaryLoop> = outcome.loops.clone();
    assert_eq!(loops.len(), 2);
    assert_eq!(loops[0].edges, loops[1].edges);
    assert!(loops[0].vertices.first() <= loops[1].vertices.first());
    let total_edges: usize = loops.iter().map(|boundary| boundary.edges).sum();
    assert_eq!(total_edges, 2 * 8);
}

#[test]
fn resolving_apertures_validates_the_declared_specs() {
    let shell = straight_shell();
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile(&shell)).unwrap();
    let loops = &outcome.loops;
    assert_eq!(
        resolve_apertures(loops, &[]),
        Err(ControlError::EmptyProfile)
    );
    let duplicate = vec![aperture("hem", shell.start), aperture("hem", shell.end)];
    assert_eq!(
        resolve_apertures(loops, &duplicate),
        Err(ControlError::DuplicateRole {
            role: "hem".to_string()
        })
    );
    let nonfinite = vec![aperture("hem", [f32::NAN, 0.0, 0.0])];
    assert_eq!(
        resolve_apertures(loops, &nonfinite),
        Err(ControlError::NonFiniteAnchor {
            role: "hem".to_string()
        })
    );
    let unradiused = vec![ApertureSpec {
        role: "hem".to_string(),
        anchor: shell.start,
        radius: 0.0,
    }];
    assert_eq!(
        resolve_apertures(loops, &unradiused),
        Err(ControlError::NonPositiveRadius {
            role: "hem".to_string()
        })
    );
    assert_eq!(
        resolve_apertures(&[], &[aperture("hem", shell.start)]),
        Err(ControlError::UnknownAperture {
            role: "hem".to_string(),
            radius: 0.06
        })
    );
    assert_eq!(
        loops.len(),
        2,
        "the fixture keeps both openings; only the declaration was wrong"
    );
}

#[test]
fn a_self_contact_rejects_the_control_and_keeps_the_measurement() {
    let shell = straight_shell();
    let profile = profile(&shell);
    let outcome = exterior_control(&shell.positions, &shell.triangles, &profile).unwrap();
    // Push one wall region through the opposite side of the tube.
    let mut crossed = outcome.positions.clone();
    let flipped = outcome
        .triangles
        .iter()
        .position(|triangle| triangle.contains(&0))
        .unwrap();
    for vertex in outcome.triangles[flipped] {
        crossed[vertex as usize][0] = -crossed[vertex as usize][0];
    }
    let diagnostics = ControlDiagnostics::measure(&crossed, &outcome.triangles).unwrap();
    assert!(
        diagnostics.self_contacts > 0,
        "the fixture must create a real rest contact"
    );
    assert_eq!(
        diagnostics.qualify(&profile),
        Err(ControlError::SelfContacts {
            pairs: diagnostics.self_contacts,
        }),
        "a contact terminates qualification instead of being smoothed away"
    );
}
