//! Opt-in bounded final push-out pass over visible violations. @req DECL0013 DECL0015

use std::collections::BTreeMap;

use crate::check::{OCCLUSION_EPSILON, VisibleDepth, visible_depths};
use crate::mask::VertexMask;
use crate::surface_util::coincident_groups;

use super::super::{
    FitError, FitOptions, MAX_FINAL_PUSH_PASSES, Mesh, validate_geometry, validate_lengths,
};

/// Clearance a push leaves under the caller's tolerance, in input units.
///
/// It is the crate's own resolution limit, [`OCCLUSION_EPSILON`]: the same
/// distance below which the certification cannot tell two surfaces apart, so a
/// smaller margin would re-measure as "at or over the tolerance" by noise
/// alone. One push therefore lands a vertex at `tolerance - 1e-4`.
const PUSH_MARGIN: f32 = OCCLUSION_EPSILON;

/// The caller's opt-in settings of the final push-out pass.
///
/// Absent ([`FitOptions::final_push`] `None`) means the pass never runs and
/// costs no measurement; there is no default tolerance, because accepting a
/// visible depth is a fit decision, not a library default. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FinalPushOptions {
    /// Passes the caller allows, `1..=10`.
    pub passes: u32,
    /// Accepted visible depth in input units; a deeper graded vertex is pushed
    /// out to just under it.
    pub tolerance: f32,
}

impl FinalPushOptions {
    /// Refuse a budget or tolerance that has no accepted reading.
    pub(crate) fn validate(&self) -> Result<(), FitError> {
        if self.passes == 0 || self.passes > MAX_FINAL_PUSH_PASSES {
            return Err(FitError::FinalPushPasses(self.passes));
        }
        if !self.tolerance.is_finite() || self.tolerance < 0.0 {
            return Err(FitError::FinalPushTolerance(self.tolerance));
        }
        Ok(())
    }
}

/// One executed pass, measured on the geometry it produced.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct FinalPushPass {
    /// 1-based pass number.
    pub pass: u32,
    /// Garment vertices the pass moved, welded copies included.
    pub moved: usize,
    /// Graded vertices still deeper than the tolerance after this pass.
    pub violations: usize,
    /// Deepest graded depth after this pass, `0` when nothing is graded.
    pub max: f32,
}

/// What the bounded pass measured and left.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FinalPushReport {
    /// Accepted visible depth the caller declared.
    pub tolerance: f32,
    /// Passes the caller allowed.
    pub budget: u32,
    /// Passes that moved geometry; `0` when the garment was already within
    /// tolerance or nothing could move.
    pub passes: u32,
    /// Graded vertices deeper than the tolerance before the first pass.
    pub before_violations: usize,
    /// Deepest graded depth before the first pass.
    pub before_max: f32,
    /// One entry per executed pass, in order.
    pub per_pass: Vec<FinalPushPass>,
    /// Graded vertices still deeper than the tolerance on the returned
    /// geometry.
    pub violations: usize,
    /// Deepest graded depth on the returned geometry.
    pub max: f32,
    /// Violations left in place because a lock, a rigid part or a degenerate
    /// body triangle owns them.
    pub held: usize,
}

/// Move every visible violation out to just under `tolerance`, at most
/// `options.passes` times.
///
/// One pass grades the garment against `obstacle` ([`visible_depths`], the
/// DECL0015 rule), and for every graded vertex deeper than the tolerance moves
/// its welded group along the outward normal of the nearest obstacle triangle by
/// `depth - tolerance + PUSH_MARGIN`. The pass stops when no graded vertex
/// exceeds the tolerance, when the budget is spent, or when no violation can
/// move. `obstacle` is the fit's declared contact scope, so a fit that measures
/// contacts against a complete collision surface pushes against the same
/// surface.
///
/// Welded copies move as one group along the group's deepest violation, so a
/// render seam cannot open; a group is held when any member is fully locked or
/// belongs to a rigid part, and a vertex whose nearest obstacle triangle is
/// degenerate has no direction and is held. Those are DECL0013's post-fit
/// guarantees. Locks are the fit's pull mask: partially locked vertices may
/// still move, only a full lock holds them.
///
/// The pass changes positions only; topology, vertex order and face winding
/// stay. `FitRepairReport::final_push` carries the per-pass record of the fit
/// that ran it.
///
/// # Errors
/// Refuses invalid geometry, a zero or over-budget pass count, a negative or
/// non-finite tolerance, mismatched lock/rigid-part lengths and a non-finite
/// pushed position.
pub fn final_push(
    garment: Mesh<'_>,
    obstacle: Mesh<'_>,
    lock: Option<&VertexMask>,
    fit: &FitOptions,
    options: &FinalPushOptions,
) -> Result<(Vec<[f32; 3]>, FinalPushReport), FitError> {
    options.validate()?;
    validate_geometry(garment, obstacle)?;
    validate_lengths(garment.0.len(), lock, fit, false)?;
    let groups = Movement::new(garment.0, fit.weld_distance);
    let held = groups.held(lock, &fit.rigid_parts);
    let mut positions = garment.0.to_vec();
    let mut measurement = measure(&positions, garment.1, obstacle, options.tolerance)?;
    let before_violations = measurement.violations.len();
    let before_max = measurement.max;
    let mut per_pass = Vec::new();
    let mut passes = 0;
    while passes < options.passes && !measurement.violations.is_empty() {
        let moved = push_violations(
            &mut positions,
            &groups,
            &held,
            &measurement.violations,
            options.tolerance,
        )?;
        if moved == 0 {
            break;
        }
        passes += 1;
        measurement = measure(&positions, garment.1, obstacle, options.tolerance)?;
        per_pass.push(FinalPushPass {
            pass: passes,
            moved,
            violations: measurement.violations.len(),
            max: measurement.max,
        });
    }
    let held_violations = measurement
        .violations
        .iter()
        .filter(|violation| held[groups.group_of[violation.vertex]] || violation.normal.is_none())
        .count();
    Ok((
        positions,
        FinalPushReport {
            tolerance: options.tolerance,
            budget: options.passes,
            passes,
            before_violations,
            before_max,
            per_pass,
            violations: measurement.violations.len(),
            max: measurement.max,
            held: held_violations,
        },
    ))
}

/// One measurement of the graded set: its violations and its deepest depth.
struct Measurement {
    /// Graded vertices deeper than the tolerance, deepest first.
    violations: Vec<VisibleDepth>,
    /// Deepest graded depth, `0` when nothing is graded.
    max: f32,
}

/// Grade the current geometry; the comparison is
/// [`crate::check::DepthReport::visible_verdict`]'s.
fn measure(
    positions: &[[f32; 3]],
    faces: &[[u32; 3]],
    obstacle: Mesh<'_>,
    tolerance: f32,
) -> Result<Measurement, FitError> {
    let graded = visible_depths((positions, faces), obstacle)?;
    let max = graded.iter().map(|graded| graded.depth).fold(0.0, f32::max);
    let mut violations: Vec<VisibleDepth> = graded
        .into_iter()
        .filter(|graded| graded.depth > tolerance)
        .collect();
    violations.sort_by(|a, b| b.depth.total_cmp(&a.depth).then(a.vertex.cmp(&b.vertex)));
    Ok(Measurement { violations, max })
}

/// Move each free weld group by its deepest violation's push.
///
/// Returns the number of moved vertices; zero means every violation is held.
fn push_violations(
    positions: &mut [[f32; 3]],
    groups: &Movement,
    held: &[bool],
    violations: &[VisibleDepth],
    tolerance: f32,
) -> Result<usize, FitError> {
    let mut required: BTreeMap<usize, (f32, [f64; 3])> = BTreeMap::new();
    for violation in violations {
        let group = groups.group_of[violation.vertex];
        if held[group] {
            continue;
        }
        let Some(normal) = violation.normal else {
            continue;
        };
        let distance = violation.depth - tolerance + PUSH_MARGIN;
        required
            .entry(group)
            .and_modify(|entry| {
                if distance > entry.0 {
                    *entry = (distance, normal);
                }
            })
            .or_insert((distance, normal));
    }
    let mut moved = 0;
    for (group, (distance, normal)) in required {
        for &vertex in &groups.members[group] {
            let point = positions[vertex];
            let pushed: [f32; 3] = std::array::from_fn(|axis| {
                (f64::from(point[axis]) + normal[axis] * f64::from(distance)) as f32
            });
            if pushed.iter().any(|value| !value.is_finite()) {
                return Err(FitError::NonFiniteResult { vertex });
            }
            positions[vertex] = pushed;
            moved += 1;
        }
    }
    Ok(moved)
}

/// Vertex copies that must move together, numbered like the fit's own weld
/// groups (`weld_distance <= 0` welds nothing).
struct Movement {
    /// Weld group of every garment vertex.
    group_of: Vec<usize>,
    /// Garment vertices of each group.
    members: Vec<Vec<usize>>,
}

impl Movement {
    fn new(positions: &[[f32; 3]], weld_distance: f32) -> Self {
        let mut group_of = vec![usize::MAX; positions.len()];
        let mut members: Vec<Vec<usize>> = Vec::new();
        if weld_distance > 0.0 {
            for group in coincident_groups(positions, weld_distance) {
                let index = members.len();
                for &vertex in &group {
                    group_of[vertex] = index;
                }
                members.push(group);
            }
        }
        for (vertex, group) in group_of.iter_mut().enumerate() {
            if *group != usize::MAX {
                continue;
            }
            *group = members.len();
            members.push(vec![vertex]);
        }
        Self { group_of, members }
    }

    /// A group is held when any copy is fully locked or in a rigid part.
    fn held(&self, lock: Option<&VertexMask>, rigid_parts: &[i32]) -> Vec<bool> {
        let mut held = vec![false; self.members.len()];
        for (vertex, &group) in self.group_of.iter().enumerate() {
            let locked = lock.is_some_and(|mask| mask.as_slice()[vertex] >= 1.0);
            let rigid = rigid_parts.get(vertex).is_some_and(|&part| part >= 0);
            held[group] |= locked || rigid;
        }
        held
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FitRepairReport, final_push_stage};
    use super::*;

    /// Outward-wound unit cube spanning `[-1, 1]` on each axis.
    const CUBE: [[f32; 3]; 8] = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    const CUBE_FACES: [[u32; 3]; 12] = [
        [4, 5, 6],
        [4, 6, 7],
        [1, 0, 3],
        [1, 3, 2],
        [1, 2, 6],
        [1, 6, 5],
        [0, 4, 7],
        [0, 7, 3],
        [3, 7, 6],
        [3, 6, 2],
        [0, 1, 5],
        [0, 5, 4],
    ];

    /// One triangle `0.1` inside the cube behind its `y = 1` face, wound `+y`.
    const PENETRATING: [[f32; 3]; 3] = [[0.0, 0.9, 0.0], [0.0, 0.9, 0.2], [0.2, 0.9, 0.0]];
    const PENETRATING_FACE: [[u32; 3]; 1] = [[0, 1, 2]];

    /// Three probes `0.004` behind three different cube faces, `8.5e-3` apart:
    /// one weld group with three push directions.
    const CORNERS: [[f32; 3]; 9] = [
        [0.996, 0.990, 0.990],
        [0.996, 1.200, 0.990],
        [0.996, 0.990, 1.200],
        [0.990, 0.996, 0.990],
        [0.990, 0.996, 1.200],
        [1.200, 0.996, 0.990],
        [0.990, 0.990, 0.996],
        [1.200, 0.990, 0.996],
        [0.990, 1.200, 0.996],
    ];
    const CORNER_FACES: [[u32; 3]; 3] = [[0, 1, 2], [3, 4, 5], [6, 7, 8]];

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    fn pass(tolerance: f32, passes: u32) -> FinalPushOptions {
        FinalPushOptions { passes, tolerance }
    }

    fn clear_report() -> FitRepairReport {
        FitRepairReport {
            inside_after: 0,
            intersection_pairs: 0,
            poke_repair_applied: false,
            projection_applied: false,
            no_clipping_fallback_applied: false,
            max_displacement: 0.0,
            max_strain: 0.0,
            collapsed_edges: 0,
            final_push: None,
        }
    }

    #[test]
    fn a_visible_violation_moves_out_to_just_under_the_tolerance() {
        let (positions, report) = final_push(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &FitOptions::default(),
            &pass(0.05, 1),
        )
        .expect("a valid push");

        assert_eq!(report.before_violations, 3);
        close(report.before_max, 0.1);
        assert_eq!(report.passes, 1);
        assert_eq!(report.per_pass.len(), 1);
        assert_eq!(report.per_pass[0].moved, 3);
        assert_eq!(report.per_pass[0].violations, 0);
        close(report.per_pass[0].max, 0.0499);
        assert_eq!(report.violations, 0);
        close(report.max, 0.0499);
        assert_eq!(report.held, 0);
        // One push along the nearest body normal: `0.1 - 0.05 + 1e-4`.
        for point in &positions {
            close(point[1], 0.9501);
        }
        assert_eq!(positions[0][0], 0.0);
        close(positions[1][2], 0.2);
        close(positions[2][0], 0.2);
    }

    #[test]
    fn the_pass_stops_at_the_first_clear_measurement() {
        let (_, report) = final_push(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &FitOptions::default(),
            &pass(0.05, 5),
        )
        .expect("a valid push");

        assert_eq!(report.budget, 5);
        assert_eq!(report.passes, 1);
        assert_eq!(report.per_pass.len(), 1);
        assert_eq!(report.violations, 0);
    }

    #[test]
    fn nothing_to_push_leaves_the_geometry_untouched() {
        let clear = [[0.0, 1.2, 0.0], [0.0, 1.2, 0.2], [0.2, 1.2, 0.0]];
        let (positions, report) = final_push(
            (&clear, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &FitOptions::default(),
            &pass(0.05, 3),
        )
        .expect("a valid push");

        assert_eq!(positions, clear.to_vec());
        assert_eq!(report.before_violations, 0);
        assert_eq!(report.passes, 0);
        assert!(report.per_pass.is_empty());
        assert_eq!(report.violations, 0);
        assert_eq!(report.max, 0.0);
    }

    #[test]
    fn a_fully_locked_vertex_is_held_and_the_residual_is_reported() {
        let lock = VertexMask::from_values(vec![1.0; PENETRATING.len()]).expect("a valid mask");
        let (positions, report) = final_push(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            Some(&lock),
            &FitOptions::default(),
            &pass(0.05, 2),
        )
        .expect("a valid push");

        assert_eq!(positions, PENETRATING.to_vec());
        assert_eq!(report.passes, 0);
        assert!(report.per_pass.is_empty());
        assert_eq!(report.violations, 3);
        assert_eq!(report.held, 3);
        close(report.max, 0.1);
    }

    #[test]
    fn a_rigid_part_is_held() {
        let options = FitOptions {
            rigid_parts: vec![0; PENETRATING.len()],
            ..FitOptions::default()
        };
        let (positions, report) = final_push(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &options,
            &pass(0.05, 2),
        )
        .expect("a valid push");

        assert_eq!(positions, PENETRATING.to_vec());
        assert_eq!(report.passes, 0);
        assert_eq!(report.held, 3);
    }

    #[test]
    fn a_partial_lock_still_moves() {
        let lock = VertexMask::from_values(vec![0.5; PENETRATING.len()]).expect("a valid mask");
        let (_, report) = final_push(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            Some(&lock),
            &FitOptions::default(),
            &pass(0.05, 1),
        )
        .expect("a valid push");

        assert_eq!(report.passes, 1);
        assert_eq!(report.violations, 0);
        assert_eq!(report.held, 0);
    }

    #[test]
    fn coincident_copies_move_together_and_keep_the_seam_closed() {
        // Two copies of the first vertex and the third and second vertex: the
        // duplicated triangle pairs make a render seam at index 0/1.
        let positions = [
            [0.0, 0.9, 0.0],
            [0.0, 0.9, 0.0],
            [0.0, 0.9, 0.2],
            [0.2, 0.9, 0.0],
        ];
        let faces = [[0, 2, 3], [1, 3, 2]];
        let (pushed, report) = final_push(
            (&positions, &faces),
            (&CUBE, &CUBE_FACES),
            None,
            &FitOptions::default(),
            &pass(0.05, 1),
        )
        .expect("a valid push");

        assert_eq!(pushed[0], pushed[1]);
        close(pushed[0][1], 0.9501);
        // The reversed copy is `inner` and therefore not graded, but it is the
        // same welded seam and moves with its graded copy.
        assert_eq!(report.before_violations, 3);
        assert_eq!(report.per_pass[0].moved, 4);
        assert_eq!(report.violations, 0);
    }

    #[test]
    fn a_three_face_weld_group_spends_the_budget_and_keeps_a_residual() {
        // One weld group whose three copies have a different nearest face each:
        // every pass can satisfy only the axis it pushes along.
        let options = FitOptions {
            weld_distance: 0.01,
            ..FitOptions::default()
        };
        let (_, report) = final_push(
            (&CORNERS, &CORNER_FACES),
            (&CUBE, &CUBE_FACES),
            None,
            &options,
            &pass(0.003, 2),
        )
        .expect("a valid push");

        assert_eq!(report.before_violations, 3);
        close(report.before_max, 0.004);
        assert_eq!(report.budget, 2);
        assert_eq!(report.passes, 2);
        assert_eq!(report.per_pass.len(), 2);
        assert_eq!(report.per_pass[0].moved, 3);
        assert_eq!(report.per_pass[0].violations, 2);
        assert_eq!(report.per_pass[1].moved, 3);
        assert_eq!(report.per_pass[1].violations, 1);
        assert_eq!(report.violations, 1);
        close(report.max, 0.004);
        assert_eq!(report.held, 0);
    }

    #[test]
    fn the_graded_set_is_the_depth_reports_visible_layer() {
        let report = crate::check::garment_depth_report(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
        )
        .expect("valid geometry");
        let graded =
            visible_depths((&PENETRATING, &PENETRATING_FACE), (&CUBE, &CUBE_FACES)).expect("valid");
        assert_eq!(graded.len(), report.visible_occluded.count);
        for (vertex, depth) in &report.visible_occluded.depths {
            let found = graded
                .iter()
                .find(|graded| graded.vertex == *vertex)
                .expect("every graded vertex is returned");
            close(found.depth, *depth);
        }
        let verdict = report.visible_verdict(0.05);
        let violations = graded.iter().filter(|graded| graded.depth > 0.05).count();
        assert_eq!(violations, verdict.violations.len());
    }

    #[test]
    fn a_bad_budget_or_tolerance_is_refused_by_name() {
        let refused = |options| {
            final_push(
                (&PENETRATING, &PENETRATING_FACE),
                (&CUBE, &CUBE_FACES),
                None,
                &FitOptions::default(),
                &options,
            )
            .expect_err("refused")
        };
        assert_eq!(refused(pass(0.05, 0)), FitError::FinalPushPasses(0));
        assert_eq!(
            refused(pass(0.05, crate::wrap::MAX_FINAL_PUSH_PASSES + 1)),
            FitError::FinalPushPasses(crate::wrap::MAX_FINAL_PUSH_PASSES + 1)
        );
        assert_eq!(refused(pass(-1.0, 1)), FitError::FinalPushTolerance(-1.0));
        assert!(matches!(
            refused(pass(f32::NAN, 1)),
            FitError::FinalPushTolerance(value) if value.is_nan()
        ));
    }

    #[test]
    fn the_fit_stage_is_off_by_default_and_pushes_when_enabled() {
        let (off, off_report) = final_push_stage(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &FitOptions::default(),
            PENETRATING.to_vec(),
            clear_report(),
        )
        .expect("a valid stage");
        assert_eq!(off, PENETRATING.to_vec());
        assert!(off_report.final_push.is_none());

        let options = FitOptions {
            final_push: Some(pass(0.05, 1)),
            ..FitOptions::default()
        };
        let (on, on_report) = final_push_stage(
            (&PENETRATING, &PENETRATING_FACE),
            (&CUBE, &CUBE_FACES),
            None,
            &options,
            PENETRATING.to_vec(),
            clear_report(),
        )
        .expect("a valid stage");
        assert_eq!(on, {
            let (pushed, _) = final_push(
                (&PENETRATING, &PENETRATING_FACE),
                (&CUBE, &CUBE_FACES),
                None,
                &options,
                &pass(0.05, 1),
            )
            .expect("the same push");
            pushed
        });
        let measured = on_report.final_push.expect("the stage reports its pass");
        assert_eq!(measured.passes, 1);
        assert_eq!(measured.violations, 0);
        // The contact measures describe the returned geometry, not the incoming
        // one: the pushed triangle is still inside the body.
        assert!(on_report.inside_after > 0);
    }
}
