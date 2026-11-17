//! Paired transverse ankle prominences in each foot's own surface-derived frame. @req DECL0013

use glam::DVec3;
use serde_json::json;

use super::profile::prominent_peak;
use super::{JointEstimate, Surface, foot_axes};
use crate::CommandError;
use crate::export_glb::refused;

const SEARCH_HEIGHT_FRACTION: f64 = 0.04;
const SMOOTHING_HALF_SAMPLES: usize = 2;

/// Turned-out feet need their own transverse direction; body-X extrema can select the
/// forefoot instead of either ankle prominence. @req DECL0013
pub(super) fn place(
    body: &Surface,
    seed: DVec3,
    forefoot: DVec3,
) -> Result<JointEstimate, CommandError> {
    let (forward, outward) = foot_axes(seed, forefoot)?;
    let sign = seed.x.signum();
    let search = body.height * SEARCH_HEIGHT_FRACTION;
    let rows: Vec<_> = body
        .sections
        .iter()
        .filter(|s| s.plane > body.height * 0.01 && s.plane < seed.y + search * 2.0)
        .filter_map(|s| {
            s.loops
                .iter()
                .filter(|r| r.closed && sign * r.centroid[0] > 0.0)
                .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
                .map(|r| (s.plane, r))
        })
        .collect();
    let profile = |direction: f64| {
        rows.iter()
            .filter_map(|(y, r)| {
                r.ring
                    .iter()
                    .map(|p| {
                        let point = DVec3::new(p[0], *y, p[1]) - seed;
                        DVec3::new(point.dot(forward), *y, point.dot(outward) * direction)
                    })
                    .max_by(|a, b| a.z.total_cmp(&b.z))
            })
            .collect::<Vec<_>>()
    };
    let outer = profile(1.0);
    let inner = profile(-1.0);
    let pair = |smoothing| {
        let outer = prominent_peak(&outer, seed.y, search, smoothing)
            .ok_or_else(|| refused("humanoid_ankle", "lateral surface prominence is unresolved"))?;
        let inner = prominent_peak(&inner, seed.y, search, smoothing)
            .ok_or_else(|| refused("humanoid_ankle", "medial surface prominence is unresolved"))?;
        Ok::<_, CommandError>((outer, inner))
    };
    let restore = |p: DVec3, direction: f64| {
        seed + forward * p.x + outward * (p.z * direction) + DVec3::Y * (p.y - seed.y)
    };
    let (outer, inner) = pair(SMOOTHING_HALF_SAMPLES)?;
    let lateral = restore(outer.position, 1.0);
    let medial = restore(inner.position, -1.0);
    let pivot = (lateral + medial) * 0.5;
    let sensitivity = [1, 3]
        .into_iter()
        .map(|smoothing| {
            let (a, b) = pair(smoothing)?;
            Ok(pivot.distance((restore(a.position, 1.0) + restore(b.position, -1.0)) * 0.5))
        })
        .collect::<Result<Vec<_>, CommandError>>()?
        .into_iter()
        .fold(0.0, f64::max);
    let (_, inside, _) = crate::medial::surface_probe(&body.vertices, &body.triangles, pivot);
    if !inside {
        return Err(refused(
            "humanoid_ankle",
            "ankle landmark midpoint is outside the flesh",
        ));
    }
    Ok(JointEstimate {
        pivot,
        report: json!({
            "method":"midpoint of paired transverse ankle prominences in the local foot frame",
            "seed_m":seed.to_array(),"foot_forward":forward.to_array(),"foot_outward":outward.to_array(),
            "lateral_surface_peak_m":lateral.to_array(),"medial_surface_peak_m":medial.to_array(),
            "lateral_prominence_m":outer.prominence,"medial_prominence_m":inner.prominence,
            "smoothing_half_samples":SMOOTHING_HALF_SAMPLES,"selection_sensitivity_m":sensitivity,
            "uncertainty":"The midpoint is a surface-conditioned ankle candidate; internal articulation and motion remain separately validated."
        }),
    })
}
