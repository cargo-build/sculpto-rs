//! Paired skin-profile transitions locate an elbow candidate without a spherical model.
//! @req DECL0013

use glam::DVec3;
use sculpto_mesh::actor::SectionLoop;
use serde_json::json;

use super::profile::{Border, Feature, Turn, border, transition};
use super::{JointEstimate, Surface};
use crate::CommandError;
use crate::export_glb::refused;

fn estimate(
    front: &[DVec3],
    back: &[DVec3],
    prior: f64,
    search: f64,
    window: f64,
) -> Result<(DVec3, Feature, Feature), CommandError> {
    let front = transition(front, prior, search, window, Turn::Positive)
        .ok_or_else(|| refused("humanoid_elbow", "anterior elbow transition is unresolved"))?;
    let back = transition(back, prior, search, window, Turn::Positive)
        .ok_or_else(|| refused("humanoid_elbow", "posterior elbow transition is unresolved"))?;
    Ok(((front.position + back.position) * 0.5, front, back))
}

/// Use both sides of the limb; an upper-arm narrowing alone does not place the hinge.
/// The current caller supplies A-pose sections. T-pose qualification is separate. @req DECL0013
pub(super) fn place(
    body: &Surface,
    rows: &[(usize, &SectionLoop)],
    prior_y: f64,
    arm_vertical_span: f64,
) -> Result<JointEstimate, CommandError> {
    let (_, near) = rows
        .iter()
        .min_by(|(a, _), (b, _)| {
            (body.sections[*a].plane - prior_y)
                .abs()
                .total_cmp(&(body.sections[*b].plane - prior_y).abs())
        })
        .ok_or_else(|| refused("humanoid_elbow", "arm sections are absent"))?;
    let radius = (near.area.abs() / std::f64::consts::PI).sqrt();
    let window = radius * 0.7;
    let profile = |side| {
        rows.iter()
            .filter_map(|(i, r)| border(r, side, body.sections[*i].plane))
            .collect::<Vec<_>>()
    };
    let front = profile(Border::Anterior);
    let back = profile(Border::Posterior);
    let search = arm_vertical_span * 0.20;
    let (pivot, anterior, posterior) = estimate(&front, &back, prior_y, search, window)?;
    let (narrow, _, _) = estimate(&front, &back, prior_y, search, window * 0.85)?;
    let (wide, _, _) = estimate(&front, &back, prior_y, search, window * 1.15)?;
    let sensitivity = pivot.distance(narrow).max(pivot.distance(wide));
    Ok(JointEstimate {
        pivot,
        report: json!({
            "method":"midpoint of paired anterior/posterior profile transitions",
            "anterior_transition_m":anterior.position.to_array(),
            "posterior_transition_m":posterior.position.to_array(),
            "anterior_turn_degrees":anterior.turn.to_degrees(),
            "posterior_turn_degrees":posterior.turn.to_degrees(),
            "profile_half_window_m":window,
            "selection_sensitivity_m":sensitivity,
            "sensitivity_half_windows_m":[window*0.85,window*1.15],
            "uncertainty":"Skin-profile transitions estimate a hinge region; spherical shape is not required and internal anatomy is not directly observed."
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_angular_profile_resolves_a_transition_without_a_round_patch() {
        let profile: Vec<DVec3> = (0..=100)
            .map(|i| {
                let y = i as f64 / 100.0;
                let z = match y < 0.5 {
                    true => -0.5 * y,
                    false => -0.25 - 0.1 * (y - 0.5),
                };
                DVec3::new(0.2, y, z)
            })
            .collect();
        let feature = transition(&profile, 0.52, 0.2, 0.1, Turn::Positive).unwrap();
        assert!((feature.position.y - 0.5).abs() < 0.01);
        assert!((feature.position.z + 0.25).abs() < 0.01);
        assert!(feature.turn > 15.0_f64.to_radians());
    }

    #[test]
    fn a_straight_featureless_profile_reports_missing_evidence() {
        let profile: Vec<DVec3> = (0..=100)
            .map(|i| DVec3::new(0.2, i as f64 / 100.0, 0.1))
            .collect();
        assert!(transition(&profile, 0.5, 0.2, 0.1, Turn::Positive).is_none());
    }
}
