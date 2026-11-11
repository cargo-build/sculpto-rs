//! Fit ordinary garment regions while retaining selected cloth geometry. @req DECL0013

use std::collections::HashSet;
use std::fmt;

use serde::Serialize;

use super::{ClothDefect, ClothPart, ClothPath, ClothRegion, select_cloth};
use crate::check::{intersection_pairs, self_intersection_witnesses};
use crate::clip_fix::ClipFixError;
use crate::control::compact;
use crate::mask::VertexMask;
use crate::topology::key;
use crate::wrap::repair::{FitRepairReport, fit_and_repair, measure_contacts, preserves_self_contacts};
use crate::wrap::{BodySurface, FitOptions, Mesh, validate_geometry, validate_lengths};

/// Measured paths and contacts on the complete returned garment.
#[derive(Debug, Clone, Serialize)]
pub struct RegionFitReport {
    /// Source triangle selection for both deformation routes.
    pub selection: Vec<ClothPart>,
    /// Triangle count used by the ordinary fit.
    pub ordinary_triangles: usize,
    /// Triangle count retained for Blender Cloth.
    pub cloth_triangles: usize,
    /// Ordinary clearance, absent when the garment has no ordinary faces.
    pub ordinary: Option<FitRepairReport>,
    /// Body contacts on the retained cloth region.
    pub cloth_intersection_pairs: usize,
    /// Body contacts on the complete garment.
    pub intersection_pairs: usize,
}

/// A failed cloth selection or fit, retaining its original cause.
#[derive(Debug)]
pub enum RegionFitError {
    /// A declared cloth region cannot bind to the source triangles.
    Selection(ClothDefect),
    /// Geometry, per-vertex settings or the fit operation failed validation.
    Fit(ClipFixError),
}

impl fmt::Display for RegionFitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selection(error) => write!(formatter, "invalid cloth region: {error}"),
            Self::Fit(error) => write!(formatter, "ordinary-region fit: {error}"),
        }
    }
}

impl std::error::Error for RegionFitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Selection(error) => Some(error),
            Self::Fit(error) => Some(error),
        }
    }
}

/// Fit ordinary faces in their own arena and restore their source vertex order.
/// Cloth and coincident seam copies retain incoming positions. @req DECL0013
///
/// # Errors
/// Refuses invalid geometry, fit settings, mask lengths and cloth selections.
pub fn fit_rigged_regions(
    garment: Mesh<'_>,
    body: Mesh<'_>,
    regions: &[ClothRegion],
    lock: Option<&VertexMask>,
    options: &FitOptions,
) -> Result<(Vec<[f32; 3]>, RegionFitReport), RegionFitError> {
    options
        .validate()
        .map_err(ClipFixError::FitOptions)
        .map_err(RegionFitError::Fit)?;
    validate_geometry(garment, body)
        .map_err(ClipFixError::Geometry)
        .map_err(RegionFitError::Fit)?;
    validate_lengths(garment.0.len(), lock, options, false)
        .map_err(ClipFixError::FitOptions)
        .map_err(RegionFitError::Fit)?;
    let selection = select_cloth(garment.1.len(), regions).map_err(RegionFitError::Selection)?;
    let mut cloth = vec![false; garment.1.len()];
    selection
        .iter()
        .filter(|part| part.path == ClothPath::BlenderCloth)
        .for_each(|part| match &part.triangle_indices {
            Some(indices) => indices.iter().for_each(|&index| cloth[index] = true),
            None => cloth.fill(true),
        });
    let cloth_faces: Vec<_> = garment
        .1
        .iter()
        .zip(&cloth)
        .filter(|(_, cloth)| **cloth)
        .map(|(&face, _)| face)
        .collect();
    let ordinary_faces: Vec<_> = garment
        .1
        .iter()
        .zip(&cloth)
        .filter(|(_, cloth)| !**cloth)
        .map(|(&face, _)| face)
        .collect();
    let fixed: HashSet<_> = cloth_faces
        .iter()
        .flatten()
        .map(|&vertex| key(garment.0[vertex as usize]))
        .collect();
    let (positions, triangles, origins) = compact(garment.0, &ordinary_faces);
    let locks = VertexMask::from_values(
        origins
            .iter()
            .map(|&vertex| {
                let index = vertex as usize;
                match fixed.contains(&key(garment.0[index])) {
                    true => 1.0,
                    false => lock.map_or(0.0, |mask| mask.as_slice()[index]),
                }
            })
            .collect(),
    )
    .expect("source masks and exact cloth locks have validated ranges");
    let compact_options = FitOptions {
        target_gaps: origins
            .iter()
            .filter_map(|&index| options.target_gaps.get(index as usize).copied())
            .collect(),
        rigid_parts: origins
            .iter()
            .filter_map(|&index| options.rigid_parts.get(index as usize).copied())
            .collect(),
        ..options.clone()
    };
    let (fitted, mut ordinary) = match triangles.is_empty() {
        true => (Vec::new(), None),
        false => {
            let (positions, report) = fit_and_repair(
            (&positions, &triangles),
            body,
            Some(&locks),
            &compact_options,
        )
            .map_err(RegionFitError::Fit)?;
            (positions, Some(report))
        }
    };
    let mut output = garment.0.to_vec();
    origins
        .iter()
        .zip(fitted)
        .for_each(|(&index, position)| output[index as usize] = position);
    let original_contacts = self_intersection_witnesses(garment.0, garment.1)
        .into_iter().collect();
    if !preserves_self_contacts(&original_contacts, (&output, garment.1)) {
        output.copy_from_slice(garment.0);
        ordinary = Some(measure_contacts(
            (&positions, &triangles),
            body,
            &BodySurface::build(body.0, body.1),
            &compact_options,
        ));
    }
    let report = RegionFitReport {
        selection,
        ordinary_triangles: ordinary_faces.len(),
        cloth_triangles: cloth_faces.len(),
        ordinary,
        cloth_intersection_pairs: intersection_pairs(&output, &cloth_faces, body.0, body.1),
        intersection_pairs: intersection_pairs(&output, garment.1, body.0, body.1),
    };
    Ok((output, report))
}
