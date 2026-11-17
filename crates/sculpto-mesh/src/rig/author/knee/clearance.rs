//! Keep flesh over the knee shafts while limiting compression along the fitted surface edges.

use super::{DMat3, DVec3, FoldBand, KneeChain};

struct EdgeLimit {
    reference: DVec3,
    direction: DVec3,
    minimum: f64,
    maximum: f64,
    projected: DVec3,
    dual: DVec3,
}

impl EdgeLimit {
    fn new(reference: DVec3, rest_length: f64) -> Result<Self, &'static str> {
        let length = reference.length();
        let direction = reference
            .try_normalize()
            .ok_or("collapsed reference fold edge")?;
        Ok(Self {
            reference,
            direction,
            minimum: length.min(0.65 * rest_length),
            maximum: length.max(1.75 * rest_length),
            projected: reference,
            dual: DVec3::ZERO,
        })
    }

    /// Keep the signed edge projection positive even when neighbouring triangles fold back. @req DECL0013
    fn project(&mut self, edge: DVec3) -> DVec3 {
        let shifted = edge + self.dual;
        let bounded = shifted.clamp_length_max(self.maximum);
        self.projected = if bounded.dot(self.direction) >= self.minimum {
            bounded
        } else {
            let tangent = shifted - self.direction * shifted.dot(self.direction);
            let radius = (self.maximum.powi(2) - self.minimum.powi(2)).sqrt();
            self.minimum * self.direction + tangent.clamp_length_max(radius)
        };
        self.reference - edge + self.projected - self.dual - edge
    }
}

struct RearSkin {
    inward: DVec3,
    side: f64,
    depth: f64,
    strength: f64,
}

impl RearSkin {
    fn displacement(&self, point: DVec3, bisector: DVec3) -> DVec3 {
        let inward = self.inward * (self.depth - point.dot(self.inward)).max(0.0);
        let across = ((point + inward).dot(bisector) * self.side).min(0.0);
        inward - across * self.side * bisector
    }
}

fn smooth(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl FoldBand<'_> {
    /// Fit posterior thickness into the space between the posed shafts and retain the fixed core rims. @req DECL0013
    pub(super) fn guard_shafts(
        &self,
        posed: &mut [DVec3],
        rotation: DMat3,
        chain: &KneeChain,
        along: &[f64],
        radius: f64,
    ) -> Result<(), &'static str> {
        let [hip, knee, ankle, _] = chain.landmarks;
        let thigh = (hip - knee).normalize();
        let rest_shin = (ankle - knee).normalize();
        let shin = rotation * rest_shin;
        let cosine = thigh.dot(shin).clamp(-1.0, 1.0);
        let half_tangent = (0.5 * cosine.acos()).tan();
        let bisector = (shin - thigh)
            .try_normalize()
            .ok_or("parallel knee shafts")?;
        let normalize = |v: DVec3| {
            v.try_normalize()
                .ok_or("ambiguous posterior knee direction")
        };
        let inward = [
            normalize(shin - cosine * thigh)?,
            normalize(thigh - cosine * shin)?,
        ];
        let back = [
            normalize(self.hinge.cross(-thigh))?,
            normalize(self.hinge.cross(rest_shin))?,
        ];
        let skin: Vec<_> = self
            .free
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let station = along[v];
                let branch = usize::from(station >= 0.0);
                let posterior = (self.p[v] - knee).dot(back[branch]).max(0.0);
                let available = station.abs() * half_tangent;
                let depth = if posterior > 0.0 {
                    posterior * available / (posterior + available)
                } else {
                    0.0
                };
                RearSkin {
                    inward: inward[branch],
                    side: if branch == 0 { -1.0 } else { 1.0 },
                    depth,
                    strength: self.diagonal[i]
                        * smooth(posterior / radius)
                        * smooth(station.abs() / radius),
                }
            })
            .collect();

        let mut edges = self
            .edges
            .iter()
            .map(|&(a, b, _)| EdgeLimit::new(posed[b] - posed[a], self.p[b].distance(self.p[a])))
            .collect::<Result<Vec<_>, _>>()?;
        let reference: Vec<_> = self.free.iter().map(|&v| posed[v]).collect();
        // Both edge terms use the same Laplacian. Divide the equation by two to reuse its PCG solver.
        let diagonal: Vec<_> = self
            .diagonal
            .iter()
            .zip(&self.support)
            .zip(&skin)
            .map(|((&edge, &rim), s)| edge + 0.5 * (rim + s.strength))
            .collect();
        for _ in 0..150 {
            let mut force = vec![DVec3::ZERO; self.free.len()];
            for (&(a, b, w), limit) in self.edges.iter().zip(&mut edges) {
                self.add_edge_force(
                    &mut force,
                    a,
                    b,
                    0.5 * w * limit.project(posed[b] - posed[a]),
                );
            }
            for (i, &v) in self.free.iter().enumerate() {
                let correction = skin[i].displacement(posed[v] - knee, bisector);
                force[i] += 0.5
                    * (skin[i].strength * correction + self.support[i] * (reference[i] - posed[v]));
            }
            let step = self.displacements(force, &diagonal)?;
            let mut max_change = 0.0_f64;
            for (&v, dx) in self.free.iter().zip(step) {
                posed[v] += dx;
                max_change = max_change.max(dx.length_squared());
            }
            for (&(a, b, _), limit) in self.edges.iter().zip(&mut edges) {
                let residual = posed[b] - posed[a] - limit.projected;
                limit.dual += residual;
                max_change = max_change.max(residual.length_squared());
            }
            if max_change < 1e-10 {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_edge_reversing_at_the_fold_keeps_its_reference_orientation() {
        let mut limit = EdgeLimit::new(DVec3::Y, 1.0).unwrap();
        limit.project(-DVec3::Y);
        assert!((limit.projected.dot(DVec3::Y) - 0.65).abs() < 1e-12);
        limit.project(4.0 * DVec3::X - DVec3::Y);
        assert!((limit.projected.dot(DVec3::Y) - 0.65).abs() < 1e-12);
        assert!((limit.projected.length() - 1.75).abs() < 1e-12);
    }
}
