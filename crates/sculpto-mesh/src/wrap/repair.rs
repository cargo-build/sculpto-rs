//! Guarded post-fit contact repair. @req DECL0013

use std::collections::HashSet;

use glam::DVec3;

use crate::check::{Witness, intersection_pairs, self_intersection_witnesses};
use crate::clip_fix::{ClipFixError, ClipFixOptions, fix_clipping};
use crate::mask::VertexMask;

use super::{
    BodySurface, FitOptions, Mesh, ON_SURFACE, REPORT_STRAIN_THRESHOLD, Welded, add, count_flipped,
    first_non_finite, fit_to_body, length, scale, strain_stats, sub, validate_geometry,
    validate_lengths, weld,
};

mod openings;

/// Measurements on the geometry actually returned by the repair.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct FitRepairReport {
    /// Welded garment groups still inside according to the SurfaceFit query.
    pub inside_after: usize,
    /// Body/garment triangle contacts on the result.
    pub intersection_pairs: usize,
    /// The poke repair passed the contact and face-orientation checks.
    pub poke_repair_applied: bool,
    /// The local projection reduced inside groups without introducing defects.
    pub projection_applied: bool,
    /// The accepted geometry came from the one no-internal-clipping alternative
    /// tried after the registered fit candidate failed to qualify.
    pub no_clipping_fallback_applied: bool,
    /// Largest vertex displacement of the returned geometry against the
    /// incoming garment, in mesh units; vertex order and topology are kept.
    pub max_displacement: f32,
    /// Largest edge strain of the returned geometry against the incoming
    /// garment: per unique edge `max(after/before, before/after) - 1` on the
    /// edge lengths, skipping edges shorter than `1e-5` in either state
    /// (`AnalyzeStrain`, `MeshAnalysis.cpp:711-747`); `0` when none qualify.
    /// Reported for quality judgment, not an acceptance threshold.
    pub max_strain: f32,
    /// Unique edges at least `1e-5` long before fitting but shorter afterward.
    /// These edges are excluded from `max_strain` and need separate review.
    pub collapsed_edges: usize,
}

/// Repair a preview and try the configured SurfaceFit mode if contacts remain.
///
/// Keep the repaired preview unless the full fit improves at least one
/// contact measure, preserves both bounds and retains original orientation.
/// New self contacts are checked against the original garment. @req DECL0013
/// Each repair branch allows `clipping_passes` stages, stopping at clearance or
/// no guarded improvement. Zero retains one mandatory poke-only stage; it disables
/// retries and the stage's inside projection, not the authoring repair itself.
/// An explicit opening direction permits a bounded whole-loop candidate after
/// ordinary repair stalls; unsupported boundaries retain their residual geometry.
///
/// When clipping was enabled and the registered fit candidate fails to qualify,
/// exactly one no-internal-clipping alternative (`clipping_passes = 0`, same
/// pull correspondences and guards) is tried; `clipping_passes = 0` itself never
/// triggers it, and a hard fit refusal propagates instead. Selection rule: a
/// candidate replaces the baseline only when it strictly improves at least one
/// contact measure without worsening the other, keeps every original face
/// orientation and adds no self-contact witness. Equal-contact candidates keep
/// the baseline, so a more distorted candidate never wins over an equally clear
/// less distorted one; deformation is reported for quality judgment, while
/// `max_strain` keeps its pull-softening and per-stage repair meaning.
///
/// # Errors
/// Refuses invalid fit or repair inputs and non-finite results.
pub fn fit_and_repair(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    fit_and_repair_with_collision(garment, body, body, lock, options)
}

/// Fit with separate correspondence and collision surfaces. @req DECL0013
///
/// `body` supplies the SurfaceFit pull correspondences; every contact and
/// inside measurement, repair candidate and acceptance guard uses the declared
/// `collision` obstacle, which may add parts the garment must not penetrate.
/// The extra parts never become pull targets. Passing one mesh for both
/// surfaces reproduces [`fit_and_repair`]; both are validated before any query.
/// Fallback, selection and reporting rules are [`fit_and_repair`]'s.
///
/// # Errors
/// Refuses invalid fit or repair inputs and non-finite results.
pub fn fit_and_repair_with_collision(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    collision: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    validate_repair_input(garment, collision, lock, options)?;
    validate_geometry(garment, body).map_err(ClipFixError::Geometry)?;
    let original_contacts: HashSet<_> = self_intersection_witnesses(garment.0, garment.1)
        .into_iter()
        .collect();
    let repair = |mesh: Mesh<'_>| {
        bounded_repair(mesh, collision, lock, options, garment, &original_contacts)
    };
    let (mut preview, mut before) = repair(garment)?;
    if !preserves_self_contacts(&original_contacts, (&preview, garment.1)) {
        preview = garment.0.to_vec();
        before = measure_contacts(
            garment,
            collision,
            &BodySurface::build(collision.0, collision.1),
            options,
            garment.0,
        );
    }
    if before.inside_after == 0 && before.intersection_pairs == 0 {
        return Ok(attach_deformation(garment.0, garment.1, preview, before));
    }
    let (positions, fit) =
        fit_to_body(garment, body, lock, options).map_err(ClipFixError::FitOptions)?;
    let mut registered_accepted = false;
    if fit.flipped == 0 {
        let (candidate, after) = repair((&positions, garment.1))?;
        if improves_contacts(&after, &before)
            && count_flipped(garment.0, &candidate, garment.1) == 0
            && preserves_self_contacts(&original_contacts, (&candidate, garment.1))
        {
            preview = candidate;
            before = after;
            registered_accepted = true;
        }
    }
    let mut fallback_applied = false;
    if !registered_accepted && options.clipping_passes > 0 {
        let no_clipping = FitOptions {
            clipping_passes: 0,
            ..options.clone()
        };
        let (positions, fit) =
            fit_to_body(garment, body, lock, &no_clipping).map_err(ClipFixError::FitOptions)?;
        if fit.flipped == 0 {
            let (candidate, after) = bounded_repair(
                (&positions, garment.1),
                collision,
                lock,
                &no_clipping,
                garment,
                &original_contacts,
            )?;
            if improves_contacts(&after, &before)
                && count_flipped(garment.0, &candidate, garment.1) == 0
                && preserves_self_contacts(&original_contacts, (&candidate, garment.1))
            {
                preview = candidate;
                before = after;
                fallback_applied = true;
            }
        }
    }
    before.no_clipping_fallback_applied = fallback_applied;
    let (positions, report) = repair_openings(
        garment,
        collision,
        lock,
        options,
        &original_contacts,
        preview,
        before,
    )?;
    Ok(attach_deformation(garment.0, garment.1, positions, report))
}

/// Additional strain stays relative to the incoming repaired surface across every attempt.
/// @req DECL0013
fn repair_openings(
    original: Mesh<'_>,
    collision: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
    original_contacts: &HashSet<Witness>,
    mut positions: Vec<[f32; 3]>,
    mut report: FitRepairReport,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    let Some(direction) = options.opening_direction else {
        return Ok((positions, report));
    };
    if options.clipping_passes == 0 || report.intersection_pairs == 0 {
        return Ok((positions, report));
    }
    let Some(openings) = openings::OpeningSurface::build(original) else {
        return Ok((positions, report));
    };
    let direction = DVec3::from(direction.map(f64::from)).normalize();
    let incoming = positions.clone();
    let surface = BodySurface::build(collision.0, collision.1);
    for _ in 0..options.clipping_passes {
        let Some(candidate) = openings.candidate(
            (&positions, original.1),
            collision,
            lock,
            options,
            direction,
        )?
        else {
            break;
        };
        if !openings::within_strain((&incoming, original.1), &candidate, options.max_strain)
            || count_flipped(original.0, &candidate, original.1) != 0
            || !preserves_self_contacts(original_contacts, (&candidate, original.1))
        {
            break;
        }
        let after = measure_contacts(
            (&candidate, original.1),
            collision,
            &surface,
            options,
            &incoming,
        );
        if !improves_contacts(&after, &report) {
            break;
        }
        positions = candidate;
        report = FitRepairReport {
            poke_repair_applied: report.poke_repair_applied,
            projection_applied: report.projection_applied,
            no_clipping_fallback_applied: report.no_clipping_fallback_applied,
            ..after
        };
        if report.intersection_pairs == 0 {
            break;
        }
    }
    Ok((positions, report))
}

/// Retry only accepted repairs; the authoring source remains the guard baseline.
/// @req DECL0013
fn bounded_repair(
    garment: Mesh<'_>,
    collision: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
    original: Mesh<'_>,
    original_contacts: &HashSet<Witness>,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    let surface = BodySurface::build(collision.0, collision.1);
    let mut positions = garment.0.to_vec();
    let mut report = measure_contacts(garment, collision, &surface, options, garment.0);
    for _ in 0..options.clipping_passes.max(1) {
        if report.inside_after == 0 && report.intersection_pairs == 0 {
            break;
        }
        let (candidate, after) =
            repair_after_fit((&positions, garment.1), collision, lock, options)?;
        if !improves_contacts(&after, &report)
            || count_flipped(original.0, &candidate, original.1) != 0
            || !preserves_self_contacts(original_contacts, (&candidate, garment.1))
        {
            break;
        }
        positions = candidate;
        report = FitRepairReport {
            poke_repair_applied: report.poke_repair_applied || after.poke_repair_applied,
            projection_applied: report.projection_applied || after.projection_applied,
            no_clipping_fallback_applied: report.no_clipping_fallback_applied,
            ..after
        };
    }
    Ok((positions, report))
}

fn improves_contacts(candidate: &FitRepairReport, before: &FitRepairReport) -> bool {
    candidate.inside_after <= before.inside_after
        && candidate.intersection_pairs <= before.intersection_pairs
        && (candidate.inside_after < before.inside_after
            || candidate.intersection_pairs < before.intersection_pairs)
}

fn validate_repair_input(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(), ClipFixError> {
    options.validate().map_err(ClipFixError::FitOptions)?;
    validate_lengths(garment.0.len(), lock, options, false).map_err(ClipFixError::FitOptions)?;
    validate_geometry(garment, body).map_err(ClipFixError::Geometry)
}

/// Repair a fitted garment, measuring and retaining the better geometry.
///
/// Both stages preserve the strongest lock and rigid parts of each welded
/// group. A local projection must reduce inside groups while preserving
/// triangle contacts, face orientation and the configured strain limit.
///
/// # Errors
/// Refuses invalid geometry, options, mask lengths and non-finite results.
pub fn repair_after_fit(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    validate_repair_input(garment, body, lock, options)?;

    let surface = BodySurface::build(body.0, body.1);
    let contacts =
        |positions: &[[f32; 3]]| intersection_pairs(positions, garment.1, body.0, body.1);
    let mut report = measure_contacts(garment, body, &surface, options, garment.0);
    if report.inside_after == 0 && report.intersection_pairs == 0 {
        return Ok(attach_deformation(
            garment.0,
            garment.1,
            garment.0.to_vec(),
            report,
        ));
    }
    let mesh = weld(garment.0, garment.1, options.weld_distance);
    let mut locked = vec![0.0_f32; mesh.rest.len()];
    for (vertex, &group) in mesh.unknown_of.iter().enumerate() {
        let mask = lock.map_or(0.0, |mask| mask.as_slice()[vertex]);
        let rigid = options
            .rigid_parts
            .get(vertex)
            .is_some_and(|&part| part >= 0);
        locked[group] = locked[group].max(if rigid { 1.0 } else { mask });
    }
    let mask =
        VertexMask::from_values(mesh.unknown_of.iter().map(|&group| locked[group]).collect())
            .expect("welded locks retain the validated range");
    let (clipped, _) = fix_clipping(
        garment,
        body,
        Some(&mask),
        &ClipFixOptions {
            fix_poke_through: true,
            ..ClipFixOptions::default()
        },
    )?;
    let clipped = coherent_positions(garment.0, &clipped, &mesh, &locked)?;
    let repaired_pairs = contacts(&clipped);
    let repaired_inside = inside_count((&clipped, garment.1), &surface, options);
    let positions = if repaired_pairs <= report.intersection_pairs
        && repaired_inside <= report.inside_after
        && count_flipped(garment.0, &clipped, garment.1) == 0
    {
        report.inside_after = repaired_inside;
        report.intersection_pairs = repaired_pairs;
        report.poke_repair_applied = true;
        clipped
    } else {
        garment.0.to_vec()
    };

    if report.inside_after == 0 || options.clipping_passes == 0 {
        return Ok(attach_deformation(garment.0, garment.1, positions, report));
    }
    let projected = project_inside(&positions, &mesh, &surface, &locked, options)?;
    let inside = inside_count((&projected, garment.1), &surface, options);
    let pairs = contacts(&projected);
    let (_, _, strain, _, _) =
        strain_stats(&positions, &projected, garment.1, REPORT_STRAIN_THRESHOLD);
    if inside < report.inside_after
        && pairs <= report.intersection_pairs
        && count_flipped(&positions, &projected, garment.1) == 0
        && count_flipped(garment.0, &projected, garment.1) == 0
        && (options.max_strain == 0.0 || strain <= options.max_strain)
    {
        report.inside_after = inside;
        report.intersection_pairs = pairs;
        report.projection_applied = true;
        return Ok(attach_deformation(garment.0, garment.1, projected, report));
    }
    Ok(attach_deformation(garment.0, garment.1, positions, report))
}

pub(crate) fn preserves_self_contacts(original: &HashSet<Witness>, garment: Mesh<'_>) -> bool {
    self_intersection_witnesses(garment.0, garment.1)
        .iter()
        .all(|contact| original.contains(contact))
}

pub(crate) fn measure_contacts(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    surface: &BodySurface<'_>,
    options: &FitOptions,
    incoming: &[[f32; 3]],
) -> FitRepairReport {
    let (max_displacement, max_strain, collapsed_edges) =
        deformation(incoming, garment.0, garment.1);
    FitRepairReport {
        inside_after: inside_count(garment, surface, options),
        intersection_pairs: intersection_pairs(garment.0, garment.1, body.0, body.1),
        poke_repair_applied: false,
        projection_applied: false,
        no_clipping_fallback_applied: false,
        max_displacement,
        max_strain,
        collapsed_edges,
    }
}

/// Attach the returned geometry's deformation against the incoming garment.
fn attach_deformation(
    incoming: &[[f32; 3]],
    faces: &[[u32; 3]],
    positions: Vec<[f32; 3]>,
    mut report: FitRepairReport,
) -> (Vec<[f32; 3]>, FitRepairReport) {
    (
        report.max_displacement,
        report.max_strain,
        report.collapsed_edges,
    ) = deformation(incoming, &positions, faces);
    (positions, report)
}

/// Maximum vertex displacement and maximum edge strain of `returned` against
/// `incoming`, reusing the fit's symmetric unique-edge strain definition.
pub(crate) fn deformation(
    incoming: &[[f32; 3]],
    returned: &[[f32; 3]],
    faces: &[[u32; 3]],
) -> (f32, f32, usize) {
    let max_displacement = incoming
        .iter()
        .zip(returned)
        .map(|(before, after)| length(sub(*after, *before)))
        .fold(0.0f32, f32::max);
    let (_, _, max_strain, _, collapsed_edges) =
        strain_stats(incoming, returned, faces, REPORT_STRAIN_THRESHOLD);
    (max_displacement, max_strain, collapsed_edges)
}

fn coherent_positions(
    original: &[[f32; 3]],
    clipped: &[[f32; 3]],
    mesh: &Welded,
    locked: &[f32],
) -> Result<Vec<[f32; 3]>, ClipFixError> {
    let mut deltas = vec![None; mesh.rest.len()];
    for ((&original, &clipped), &group) in original.iter().zip(clipped).zip(&mesh.unknown_of) {
        deltas[group].get_or_insert(sub(clipped, original));
    }
    apply_deltas(
        original,
        mesh,
        locked,
        &deltas
            .into_iter()
            .map(|delta| delta.expect("every weld group has a representative"))
            .collect::<Vec<_>>(),
    )
}

fn project_inside(
    positions: &[[f32; 3]],
    mesh: &Welded,
    surface: &BodySurface<'_>,
    locked: &[f32],
    options: &FitOptions,
) -> Result<Vec<[f32; 3]>, ClipFixError> {
    let mut representatives = vec![None; mesh.rest.len()];
    for (&position, &group) in positions.iter().zip(&mesh.unknown_of) {
        representatives[group].get_or_insert(position);
    }
    let gap = options.min_gap.max(ON_SURFACE);
    let deltas: Vec<_> = representatives
        .into_iter()
        .zip(locked)
        .map(|(point, &lock)| {
            let original = point.expect("every weld group has a representative");
            let mut point = original;
            if lock < 1.0 {
                for _ in 0..options.clipping_passes {
                    let Some(hit) = surface.inside_hit(point, options.max_distance) else {
                        break;
                    };
                    let target = add(hit.point, scale(hit.normal, gap));
                    point = add(point, scale(sub(target, point), 1.0 - lock));
                }
            }
            sub(point, original)
        })
        .collect();
    apply_deltas(positions, mesh, locked, &deltas)
}

fn apply_deltas(
    positions: &[[f32; 3]],
    mesh: &Welded,
    locked: &[f32],
    deltas: &[[f32; 3]],
) -> Result<Vec<[f32; 3]>, ClipFixError> {
    let positions: Vec<_> = positions
        .iter()
        .zip(&mesh.unknown_of)
        .map(|(&point, &group)| {
            if locked[group] == 1.0 || deltas[group] == [0.0; 3] {
                point
            } else {
                add(point, deltas[group])
            }
        })
        .collect();
    if let Some(vertex) = first_non_finite(&positions) {
        return Err(ClipFixError::NonFiniteOutput { vertex });
    }
    Ok(positions)
}

fn inside_count(garment: Mesh<'_>, surface: &BodySurface<'_>, options: &FitOptions) -> usize {
    weld(garment.0, garment.1, options.weld_distance)
        .rest
        .iter()
        .filter(|&&point| surface.inside_hit(point, options.max_distance).is_some())
        .count()
}
