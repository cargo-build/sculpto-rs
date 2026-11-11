//! Guarded post-fit contact repair. @req DECL0013

use std::collections::HashSet;

use crate::check::{Witness, intersection_pairs, self_intersection_witnesses};
use crate::clip_fix::{ClipFixError, ClipFixOptions, fix_clipping};
use crate::mask::VertexMask;

use super::{
    BodySurface, FitOptions, Mesh, ON_SURFACE, REPORT_STRAIN_THRESHOLD, Welded, add, count_flipped,
    first_non_finite, fit_to_body, scale, strain_stats, sub, validate_geometry, validate_lengths,
    weld,
};

/// Measurements on the geometry actually returned by the repair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct FitRepairReport {
    /// Welded garment groups still inside according to the SurfaceFit query.
    pub inside_after: usize,
    /// Body/garment triangle contacts on the result.
    pub intersection_pairs: usize,
    /// The poke repair passed the contact and face-orientation checks.
    pub poke_repair_applied: bool,
    /// The local projection reduced inside groups without introducing defects.
    pub projection_applied: bool,
}

/// Repair a preview and try the configured SurfaceFit mode if contacts remain.
///
/// Keep the repaired preview unless the full fit improves at least one
/// contact measure, preserves both bounds and retains original orientation.
/// New self contacts are checked against the original garment. @req DECL0013
///
/// # Errors
/// Refuses invalid fit or repair inputs and non-finite results.
pub fn fit_and_repair(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, FitRepairReport), ClipFixError> {
    let repair = |mesh: Mesh<'_>| repair_after_fit(mesh, body, lock, options);
    let (mut preview, mut before) = repair(garment)?;
    let original_contacts: HashSet<_> =
        self_intersection_witnesses(garment.0, garment.1).into_iter().collect();
    if !preserves_self_contacts(&original_contacts, (&preview, garment.1)) {
        preview = garment.0.to_vec();
        before = measure_contacts(garment, body, &BodySurface::build(body.0, body.1), options);
    }
    if before.inside_after == 0 && before.intersection_pairs == 0 {
        return Ok((preview, before));
    }
    let (positions, fit) =
        fit_to_body(garment, body, lock, options).map_err(ClipFixError::FitOptions)?;
    if fit.flipped != 0 {
        return Ok((preview, before));
    }
    let (candidate, after) = repair((&positions, garment.1))?;
    if after.inside_after <= before.inside_after
        && after.intersection_pairs <= before.intersection_pairs
        && (after.inside_after < before.inside_after
            || after.intersection_pairs < before.intersection_pairs)
        && count_flipped(garment.0, &candidate, garment.1) == 0
        && preserves_self_contacts(&original_contacts, (&candidate, garment.1))
    {
        Ok((candidate, after))
    } else {
        Ok((preview, before))
    }
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
    options.validate().map_err(ClipFixError::FitOptions)?;
    validate_lengths(garment.0.len(), lock, options, false).map_err(ClipFixError::FitOptions)?;
    validate_geometry(garment, body).map_err(ClipFixError::Geometry)?;

    let surface = BodySurface::build(body.0, body.1);
    let contacts =
        |positions: &[[f32; 3]]| intersection_pairs(positions, garment.1, body.0, body.1);
    let mut report = measure_contacts(garment, body, &surface, options);
    if report.inside_after == 0 && report.intersection_pairs == 0 {
        return Ok((garment.0.to_vec(), report));
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
        return Ok((positions, report));
    }
    let projected = project_inside(&positions, &mesh, &surface, &locked, options)?;
    let inside = inside_count((&projected, garment.1), &surface, options);
    let pairs = contacts(&projected);
    let (_, _, strain, _) =
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
        return Ok((projected, report));
    }
    Ok((positions, report))
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
) -> FitRepairReport {
    FitRepairReport {
        inside_after: inside_count(garment, surface, options),
        intersection_pairs: intersection_pairs(garment.0, garment.1, body.0, body.1),
        poke_repair_applied: false,
        projection_applied: false,
    }
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
