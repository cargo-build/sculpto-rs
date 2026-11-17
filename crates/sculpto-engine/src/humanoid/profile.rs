//! Fitted skin-profile transitions shared by limb landmark estimates. @req DECL0013

use glam::DVec3;
use sculpto_mesh::actor::SectionLoop;

#[derive(Clone, Copy)]
pub(super) enum Border {
    Anterior,
    Posterior,
}

#[derive(Clone, Copy)]
pub(super) enum Turn {
    Positive,
    Negative,
}

impl Turn {
    fn score(self, angle: f64) -> f64 {
        match self {
            Self::Positive => angle,
            Self::Negative => -angle,
        }
    }
}

#[derive(Clone, Copy)]
struct Line {
    slope: f64,
    intercept: f64,
}

pub(super) struct Feature {
    pub position: DVec3,
    pub turn: f64,
}

pub(super) struct Peak {
    pub position: DVec3,
    pub prominence: f64,
}

pub(super) fn border(section: &SectionLoop, side: Border, y: f64) -> Option<DVec3> {
    let point = match side {
        Border::Anterior => section.ring.iter().max_by(|a, b| a[1].total_cmp(&b[1])),
        Border::Posterior => section.ring.iter().min_by(|a, b| a[1].total_cmp(&b[1])),
    }?;
    Some(DVec3::new(point[0], y, point[1]))
}

fn line(points: &[DVec3]) -> Option<Line> {
    if points.len() < 4 {
        return None;
    }
    let n = points.len() as f64;
    let y = points.iter().map(|p| p.y).sum::<f64>() / n;
    let z = points.iter().map(|p| p.z).sum::<f64>() / n;
    let yy = points.iter().map(|p| (p.y - y).powi(2)).sum::<f64>();
    if yy <= 1e-12 {
        return None;
    }
    let slope = points.iter().map(|p| (p.y - y) * (p.z - z)).sum::<f64>() / yy;
    Some(Line {
        slope,
        intercept: z - slope * y,
    })
}

fn interpolate(profile: &[DVec3], y: f64) -> Option<DVec3> {
    let index = profile.partition_point(|p| p.y < y);
    let a = profile.get(index.checked_sub(1)?)?;
    let b = profile.get(index)?;
    Some(a.lerp(*b, (y - a.y) / (b.y - a.y)))
}

/// Adjacent line fits suppress individual triangle corners without requiring a round joint.
pub(super) fn transition(
    profile: &[DVec3],
    prior: f64,
    search: f64,
    window: f64,
    direction: Turn,
) -> Option<Feature> {
    profile
        .iter()
        .filter(|p| (p.y - prior).abs() < search)
        .filter_map(|p| {
            let below: Vec<DVec3> = profile
                .iter()
                .filter(|q| q.y < p.y && q.y >= p.y - window)
                .copied()
                .collect();
            let above: Vec<DVec3> = profile
                .iter()
                .filter(|q| q.y > p.y && q.y <= p.y + window)
                .copied()
                .collect();
            let lower = line(&below)?;
            let upper = line(&above)?;
            let turn = upper.slope.atan() - lower.slope.atan();
            if direction.score(turn) < 3.0_f64.to_radians() {
                return None;
            }
            let y = (upper.intercept - lower.intercept) / (lower.slope - upper.slope);
            if !y.is_finite() || (y - p.y).abs() > window {
                return None;
            }
            let mut position = interpolate(profile, y)?;
            position.z = lower.slope * y + lower.intercept;
            Some(Feature { position, turn })
        })
        .max_by(|a, b| direction.score(a.turn).total_cmp(&direction.score(b.turn)))
}

/// Local prominence excludes a larger forefoot at the search boundary. The returned point
/// lies on the measured profile; smoothing only selects its height. @req DECL0013
pub(super) fn prominent_peak(
    profile: &[DVec3],
    prior: f64,
    search: f64,
    half_samples: usize,
) -> Option<Peak> {
    let end = profile.len().checked_sub(half_samples + 1)?;
    let start = half_samples + 1;
    if start >= end {
        return None;
    }
    let smoothed: Vec<f64> = (0..profile.len())
        .map(|i| {
            let begin = i.saturating_sub(half_samples);
            let end = (i + half_samples + 1).min(profile.len());
            profile[begin..end].iter().map(|p| p.z).sum::<f64>() / (end - begin) as f64
        })
        .collect();
    (start..end)
        .filter_map(|i| {
            let value = smoothed[i];
            if (profile[i].y - prior).abs() > search
                || value <= smoothed[i - 1]
                || value < smoothed[i + 1]
            {
                return None;
            }
            let left = smoothed[..i]
                .iter()
                .rev()
                .copied()
                .take_while(|v| *v <= value)
                .fold(value, f64::min);
            let right = smoothed[i + 1..]
                .iter()
                .copied()
                .take_while(|v| *v <= value)
                .fold(value, f64::min);
            let prominence = value - left.max(right);
            if prominence <= 0.0 {
                return None;
            }
            let x1 = profile[i - 1].y - profile[i].y;
            let x2 = profile[i + 1].y - profile[i].y;
            let slope1 = (smoothed[i - 1] - value) / x1;
            let slope2 = (smoothed[i + 1] - value) / x2;
            let curvature = (slope2 - slope1) / (x2 - x1);
            if !curvature.is_finite() || curvature >= 0.0 {
                return None;
            }
            let slope = slope1 - curvature * x1;
            let offset = (-slope / (2.0 * curvature)).clamp(x1, x2);
            let position = interpolate(profile, profile[i].y + offset)?;
            Some(Peak {
                position,
                prominence,
            })
        })
        .max_by(|a, b| a.prominence.total_cmp(&b.prominence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_posterior_concavity_requires_the_opposite_turn() {
        let profile: Vec<DVec3> = (0..=100)
            .map(|i| {
                let y = i as f64 / 100.0;
                DVec3::new(0.2, y, 0.1 - 0.3 * (y - 0.6).abs())
            })
            .collect();
        let feature = transition(&profile, 0.55, 0.2, 0.1, Turn::Negative).unwrap();
        assert!((feature.position.y - 0.6).abs() < 0.01);
        assert!((feature.position.z - 0.1).abs() < 0.01);
        assert!(transition(&profile, 0.55, 0.2, 0.1, Turn::Positive).is_none());
    }

    #[test]
    fn local_prominence_finds_a_bump_below_a_larger_boundary_value() {
        let profile: Vec<DVec3> = (0..=100)
            .map(|i| {
                let y = i as f64 / 100.0;
                let bump = 0.8 * (1.0 - (y - 0.5).abs() / 0.1).max(0.0);
                DVec3::new(0.1, y, 3.0 - 2.0 * y + bump)
            })
            .collect();
        let peak = prominent_peak(&profile, 0.48, 0.25, 2).unwrap();
        assert!((peak.position.y - 0.5).abs() < 0.01);
        assert!(peak.position.z < profile[0].z);
        assert!(peak.prominence > 0.3);
        let straight: Vec<_> = (0..=100)
            .map(|i| DVec3::new(0.1, i as f64 / 100.0, 1.0 - i as f64 / 100.0))
            .collect();
        assert!(prominent_peak(&straight, 0.5, 0.4, 2).is_none());
    }
}
