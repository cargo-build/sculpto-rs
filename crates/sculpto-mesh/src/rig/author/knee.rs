//! Surface-distance thigh/shin weights and pose-space knee shapes on the original triangles.

use glam::{DMat3, DQuat, DVec3};

use super::{Surface, seams::SeamWeights, surface_distances};
mod clearance;

/// Absolute flexion angles of the two shapes, spanning ordinary gait and a deep bend. @req DECL0013
pub const FLEXION_DEGREES: [f64; 2] = [70.0, 130.0];
const CORE_START: f64 = 0.35;
const CORE_END: f64 = 1.0 - CORE_START;

/// The two palette influences and the world-space centres of hip, knee, ankle and toe base.
pub struct KneeChain {
    pub thigh: u16,
    pub shin: u16,
    pub landmarks: [DVec3; 4],
}

/// One shape per sampled flexion. Offsets are expressed in the world rest frame.
pub struct KneeTransition {
    pub rest_angle: f64,
    pub changed_rows: usize,
    pub free_vertices: usize,
    /// Signed longitudinal coordinates in metres, increasing from thigh to foot.
    pub core_rims: [f64; 2],
    pub mixed_extent: Option<[f64; 2]>,
    pub equal_extent: Option<[f64; 2]>,
    pub targets: Vec<Vec<DVec3>>,
}

/// Angle between the two successive segments of a knee. @req DECL0013
pub fn flexion(hip: DVec3, knee: DVec3, ankle: DVec3) -> Result<f64, &'static str> {
    let upper = (knee - hip).try_normalize().ok_or("degenerate thigh")?;
    let lower = (ankle - knee).try_normalize().ok_or("degenerate shin")?;
    Ok(upper.dot(lower).clamp(-1.0, 1.0).acos().to_degrees())
}

/// Interpolate absolute bend poses with zero contribution at the authored bind angle. @req DECL0013
pub fn shape_weights(rest_angle: f64, angle: f64) -> [f32; 2] {
    let [walk, run] = FLEXION_DEGREES;
    let deep = ((angle - walk) / (run - walk)).clamp(0.0, 1.0);
    [
        (((angle - rest_angle) / (walk - rest_angle)).clamp(0.0, 1.0) * (1.0 - deep)) as f32,
        deep as f32,
    ]
}

/// Author both sides together so physical seam aliases share the solve. The input rows stay intact on error. @req DECL0013
pub fn knee_transitions(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    flesh: &[bool],
    rows: &mut [Vec<(u16, f32)>],
    chains: &[KneeChain; 2],
    with_shapes: bool,
) -> Result<Vec<KneeTransition>, &'static str> {
    let n = positions.len();
    let slots = [
        chains[0].thigh,
        chains[0].shin,
        chains[1].thigh,
        chains[1].shin,
    ];
    if n == 0
        || rows.len() != n
        || flesh.len() != n
        || triangles.is_empty()
        || triangles.iter().flatten().any(|&v| v as usize >= n)
        || positions.iter().flatten().any(|v| !v.is_finite())
        || slots
            .iter()
            .enumerate()
            .any(|(i, slot)| slots[..i].contains(slot))
        || rows.iter().any(|r| {
            r.is_empty()
                || r.len() > 4
                || r.iter().any(|(_, w)| !w.is_finite() || *w < 0.0)
                || (r.iter().map(|(_, w)| f64::from(*w)).sum::<f64>() - 1.0).abs() > 2e-6
        })
        || chains
            .iter()
            .flat_map(|c| c.landmarks)
            .any(|v| !v.is_finite())
    {
        return Err("invalid knee surface or landmarks");
    }
    let seams = SeamWeights::find(positions, triangles, flesh);
    let mut alias: Vec<_> = (0..n).collect();
    seams
        .links()
        .for_each(|(a, b)| alias[b as usize] = a as usize);
    let faces: Vec<_> = triangles
        .iter()
        .map(|face| face.map(|v| alias[v as usize] as u32))
        .collect();
    let points: Vec<_> = positions.iter().copied().map(DVec3::from_array).collect();
    let surface = Surface::build(&points, &faces, std::iter::empty());
    let mut result = rows.to_vec();
    let mut transitions = Vec::new();
    for chain in chains {
        transitions.push(transition(
            &points,
            &surface,
            &alias,
            &mut result,
            chain,
            with_shapes,
        )?);
    }
    if result.iter().any(|row| row.len() > 4) {
        return Err("knee blend requires more than four influences on an input vertex");
    }
    rows.clone_from_slice(&result);
    Ok(transitions)
}

fn mass(row: &[(u16, f32)], slot: u16) -> f64 {
    row.iter()
        .filter(|(i, _)| *i == slot)
        .map(|(_, w)| f64::from(*w))
        .sum()
}

fn transition(
    p: &[DVec3],
    surface: &Surface,
    alias: &[usize],
    rows: &mut [Vec<(u16, f32)>],
    chain: &KneeChain,
    with_shapes: bool,
) -> Result<KneeTransition, &'static str> {
    let [hip, knee, ankle, toe] = chain.landmarks;
    let upper = knee - hip;
    let lower = ankle - knee;
    let rest_angle = flexion(hip, knee, ankle)?;
    if rest_angle >= FLEXION_DEGREES[0] {
        return Err("bind knee is already beyond the first calibration pose");
    }
    let axis = (upper.normalize() + lower.normalize())
        .try_normalize()
        .ok_or("ambiguous knee bisector")?;
    let hinge = lower
        .cross(ankle - toe)
        .try_normalize()
        .ok_or("toe direction does not resolve the knee flexion plane")?;
    let total: Vec<_> = rows
        .iter()
        .map(|r| mass(r, chain.thigh) + mass(r, chain.shin))
        .collect();
    let along: Vec<_> = p.iter().map(|&v| (v - knee).dot(axis)).collect();
    let in_band = |v: usize| {
        total[v] > 0.0
            && along[v] >= -CORE_END * upper.length()
            && along[v] <= CORE_END * lower.length()
    };
    let distances = |proximal: bool| {
        let seeds: Vec<_> = (0..p.len())
            .filter(|&v| {
                alias[v] == v
                    && in_band(v)
                    && match proximal {
                        true => along[v] <= -CORE_START * upper.length(),
                        false => along[v] >= CORE_START * lower.length(),
                    }
            })
            .map(|v| v as u32)
            .collect();
        let mut dist = vec![f64::INFINITY; p.len()];
        surface_distances(surface, &seeds, &mut dist, in_band);
        dist
    };
    let proximal = distances(true);
    let distal = distances(false);
    let mut mix = vec![0.0; p.len()];
    let mut free = vec![false; p.len()];
    let mut changed = 0;
    for (v, row) in rows.iter_mut().enumerate() {
        let id = alias[v];
        let (a, b) = (proximal[id], distal[id]);
        let old = mass(row, chain.shin);
        mix[v] = if total[v] > 0.0 { old / total[v] } else { 0.0 };
        if !(a + b).is_finite() || total[v] == 0.0 {
            continue;
        }
        let t = a / (a + b);
        mix[v] = t * t * (3.0 - 2.0 * t);
        free[v] = v == id && a > 0.0 && b > 0.0;
        let next = total[v] * mix[v];
        if (next - old).abs() <= 1e-10 {
            continue;
        }
        row.retain(|(slot, _)| *slot != chain.thigh && *slot != chain.shin);
        for (slot, weight) in [(chain.thigh, total[v] - next), (chain.shin, next)] {
            if weight > 0.0 {
                row.push((slot, weight as f32));
            }
        }
        changed += 1;
    }
    let free_vertices = free.iter().filter(|&&v| v).count();
    if free_vertices == 0 {
        return Err("no connected surface between the thigh and shin cores");
    }
    let mut targets = Vec::new();
    if with_shapes {
        let (radius, support) = joint_support(
            p,
            &free,
            &along,
            knee,
            axis,
            0.2 * CORE_START * upper.length().min(lower.length()),
        )?;
        let band = FoldBand::new(p, surface, &free, &mix, knee, hinge, support)?;
        for degrees in FLEXION_DEGREES {
            let mut theta = (degrees - rest_angle).to_radians();
            for _ in 0..8 {
                let lower_posed = DMat3::from_axis_angle(hinge, theta) * lower;
                let error = degrees - flexion(hip, knee, knee + lower_posed)?;
                theta += error.to_radians();
            }
            let target_angle = flexion(
                hip,
                knee,
                knee + DMat3::from_axis_angle(hinge, theta) * lower,
            )?;
            if (degrees - target_angle).abs() > 0.01 {
                return Err("calibration angle is unreachable in the limb plane");
            }
            let offset = band.solve(theta, chain, &along, radius)?;
            targets.push(alias.iter().map(|&v| offset[v]).collect());
        }
    }
    Ok(KneeTransition {
        rest_angle,
        changed_rows: changed,
        free_vertices,
        core_rims: [-CORE_START * upper.length(), CORE_START * lower.length()],
        mixed_extent: weight_extent(&free, &along, &mix, 0.10),
        equal_extent: weight_extent(&free, &along, &mix, 0.45),
        targets,
    })
}

fn weight_extent(free: &[bool], along: &[f64], mix: &[f64], lower: f64) -> Option<[f64; 2]> {
    let mut points = (0..free.len())
        .filter(|&i| free[i] && (lower..=1.0 - lower).contains(&mix[i]))
        .map(|i| along[i]);
    let first = points.next()?;
    Some(points.fold([first, first], |[lo, hi], v| [lo.min(v), hi.max(v)]))
}

/// Preserve the joint's measured transverse section while posterior rings are free to fold. @req DECL0013
fn joint_support(
    p: &[DVec3],
    free: &[bool],
    along: &[f64],
    knee: DVec3,
    axis: DVec3,
    half_width: f64,
) -> Result<(f64, Vec<f64>), &'static str> {
    let mut radial: Vec<_> = (0..p.len())
        .filter(|&i| free[i] && along[i].abs() < half_width)
        .map(|i| (p[i] - knee - along[i] * axis).length())
        .collect();
    if radial.is_empty() {
        return Err("knee has no measured transverse section");
    }
    radial.sort_by(f64::total_cmp);
    let radius = 0.5 * (radial[(radial.len() - 1) / 2] + radial[radial.len() / 2]);
    if radius <= 1e-6 {
        return Err("degenerate knee transverse section");
    }
    let strength = (0..p.len())
        .filter(|&i| free[i])
        .map(|i| {
            let t = (1.0 - along[i].abs() / radius).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        })
        .collect();
    Ok((radius, strength))
}

/// Least-squares edge rotation field with the two rigid segment cores as Dirichlet boundaries.
struct FoldBand<'a> {
    p: &'a [DVec3],
    mix: &'a [f64],
    knee: DVec3,
    hinge: DVec3,
    free: Vec<usize>,
    index: Vec<Option<usize>>,
    edges: Vec<(usize, usize, f64)>,
    diagonal: Vec<f64>,
    support: Vec<f64>,
}

impl<'a> FoldBand<'a> {
    fn new(
        p: &'a [DVec3],
        surface: &Surface,
        mobile: &[bool],
        mix: &'a [f64],
        knee: DVec3,
        hinge: DVec3,
        mut support: Vec<f64>,
    ) -> Result<Self, &'static str> {
        let free: Vec<_> = (0..p.len()).filter(|&v| mobile[v]).collect();
        let mut index = vec![None; p.len()];
        for (i, &v) in free.iter().enumerate() {
            index[v] = Some(i);
        }
        let mut edges = Vec::new();
        let mut diagonal = vec![0.0; free.len()];
        for &a in &free {
            for slot in surface.neighbours(a) {
                let b = surface.neighbours[slot] as usize;
                if mobile[b] && b <= a {
                    continue;
                }
                let length = surface.lengths[slot];
                if length <= 1e-9 {
                    continue;
                }
                let w = length.recip().powi(2);
                edges.push((a, b, w));
                for i in [index[a], index[b]].into_iter().flatten() {
                    diagonal[i] += w;
                }
            }
        }
        if diagonal.iter().any(|&v| v <= 0.0) {
            return Err("isolated mobile vertex in knee fold");
        }
        for (w, d) in support.iter_mut().zip(&diagonal) {
            *w *= d;
        }
        Ok(Self {
            p,
            mix,
            knee,
            hinge,
            free,
            index,
            edges,
            diagonal,
            support,
        })
    }

    fn multiply(&self, v: &[DVec3], out: &mut [DVec3], diagonal: &[f64]) {
        for ((dst, &v), &d) in out.iter_mut().zip(v).zip(diagonal) {
            *dst = v * d;
        }
        for &(a, b, w) in &self.edges {
            if let (Some(i), Some(j)) = (self.index[a], self.index[b]) {
                out[i] -= w * v[j];
                out[j] -= w * v[i];
            }
        }
    }

    fn solve(
        &self,
        theta: f64,
        chain: &KneeChain,
        along: &[f64],
        radius: f64,
    ) -> Result<Vec<DVec3>, &'static str> {
        let full = DMat3::from_axis_angle(self.hinge, theta);
        let lbs: Vec<_> = self
            .p
            .iter()
            .zip(self.mix)
            .map(|(&p, &mix)| p + mix * ((full - DMat3::IDENTITY) * (p - self.knee)))
            .collect();
        let mut posed = lbs.clone();
        let first = self.edge_step(&posed, None, |a, b| {
            let rotation =
                DMat3::from_axis_angle(self.hinge, 0.5 * (self.mix[a] + self.mix[b]) * theta);
            rotation * (self.p[b] - self.p[a])
        })?;
        for (&v, shift) in self.free.iter().zip(first) {
            posed[v] += shift;
        }
        let mut rotations: Vec<_> = self
            .mix
            .iter()
            .map(|&w| DMat3::from_axis_angle(self.hinge, w * theta))
            .collect();
        // A prescribed rotation field still compresses posterior edges in deep flexion.
        // Alternate fitted one-ring rotations and a global edge solve with fixed core rims.
        let half = DMat3::from_axis_angle(self.hinge, 0.5 * theta);
        let rim: Vec<_> = self
            .free
            .iter()
            .map(|&v| self.knee + half * (self.p[v] - self.knee))
            .collect();
        for _ in 0..60 {
            self.fit_rotations(&posed, &mut rotations);
            let step = self.edge_step(&posed, Some(&rim), |a, b| {
                (rotations[a] + rotations[b]) * (self.p[b] - self.p[a]) * 0.5
            })?;
            let converged = step.iter().all(|d| d.length_squared() < 1e-10);
            for (&v, shift) in self.free.iter().zip(step) {
                posed[v] += shift;
            }
            if converged {
                break;
            }
        }
        self.guard_shafts(&mut posed, full, chain, along, radius)?;
        let mut target = vec![DVec3::ZERO; self.p.len()];
        for &v in &self.free {
            let linear = DMat3::IDENTITY + (full - DMat3::IDENTITY) * self.mix[v];
            target[v] = linear.inverse() * (posed[v] - lbs[v]);
            if !target[v].is_finite() {
                return Err("nonfinite bind-space corrective");
            }
        }
        Ok(target)
    }

    fn fit_rotations(&self, posed: &[DVec3], rotations: &mut [DMat3]) {
        let mut covariance = vec![DMat3::ZERO; self.free.len()];
        for &(a, b, w) in &self.edges {
            let from = self.p[b] - self.p[a];
            let to = w * (posed[b] - posed[a]);
            let outer = DMat3::from_cols(from.x * to, from.y * to, from.z * to);
            for i in [self.index[a], self.index[b]].into_iter().flatten() {
                covariance[i] += outer;
            }
        }
        for (&v, c) in self.free.iter().zip(covariance) {
            let [w, x, y, z] = crate::surface_util::alignment_quaternion(c.to_cols_array_2d());
            rotations[v] = DMat3::from_quat(DQuat::from_xyzw(x, y, z, w).normalize());
        }
    }

    fn edge_step(
        &self,
        posed: &[DVec3],
        rim: Option<&[DVec3]>,
        wanted: impl Fn(usize, usize) -> DVec3,
    ) -> Result<Vec<DVec3>, &'static str> {
        let mut force = vec![DVec3::ZERO; self.free.len()];
        for &(a, b, w) in &self.edges {
            let difference = w * (wanted(a, b) - (posed[b] - posed[a]));
            self.add_edge_force(&mut force, a, b, difference);
        }
        let mut diagonal = self.diagonal.clone();
        if let Some(rim) = rim {
            for (i, &v) in self.free.iter().enumerate() {
                force[i] += self.support[i] * (rim[i] - posed[v]);
                diagonal[i] += self.support[i];
            }
        }
        self.displacements(force, &diagonal)
    }

    fn add_edge_force(&self, force: &mut [DVec3], a: usize, b: usize, difference: DVec3) {
        if let Some(i) = self.index[a] {
            force[i] -= difference;
        }
        if let Some(j) = self.index[b] {
            force[j] += difference;
        }
    }

    fn displacements(
        &self,
        mut residual: Vec<DVec3>,
        diagonal: &[f64],
    ) -> Result<Vec<DVec3>, &'static str> {
        let mut x = vec![DVec3::ZERO; residual.len()];
        let precondition = |r: &[DVec3]| {
            r.iter()
                .zip(diagonal)
                .map(|(&v, &d)| v / d)
                .collect::<Vec<_>>()
        };
        let dot = |a: &[DVec3], b: &[DVec3]| a.iter().zip(b).map(|(u, v)| u.dot(*v)).sum::<f64>();
        let mut direction = precondition(&residual);
        let mut rho = dot(&residual, &direction);
        let tolerance = (rho * 1e-16).max(1e-24);
        let mut product = x.clone();
        for _ in 0..self.free.len().clamp(64, 2000) {
            if rho <= tolerance {
                return Ok(x);
            }
            self.multiply(&direction, &mut product, diagonal);
            let denom = dot(&direction, &product);
            if !denom.is_finite() || denom <= 0.0 {
                break;
            }
            let alpha = rho / denom;
            for i in 0..x.len() {
                x[i] += alpha * direction[i];
                residual[i] -= alpha * product[i];
            }
            let z = precondition(&residual);
            let next = dot(&residual, &z);
            for (v, z) in direction.iter_mut().zip(z) {
                *v = z + (next / rho) * *v;
            }
            rho = next;
        }
        Err("knee edge-field solver did not converge")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_cores_keep_pair_mass_and_fold_targets_end_at_the_boundary() {
        let mut points = Vec::new();
        let mut triangles = Vec::new();
        let mut rows = Vec::new();
        let chains = [0, 1].map(|side| {
            let u = side * 2;
            let x = if side == 0 { -0.2 } else { 0.2 };
            let first = points.len() as u32;
            for ring in 0..=20 {
                for i in 0..8 {
                    let theta = f64::from(i) * std::f64::consts::TAU / 8.;
                    points.push([
                        x + 0.045 * theta.cos(),
                        f64::from(ring) / 20.,
                        0.045 * theta.sin(),
                    ]);
                    rows.push(vec![(u, 0.375), (u + 1, 0.375), (4, 0.25)]);
                    if ring < 20 {
                        let a = first + ring * 8 + i;
                        let b = first + ring * 8 + (i + 1) % 8;
                        triangles.extend([[a, b, a + 8], [b, b + 8, a + 8]]);
                    }
                }
            }
            KneeChain {
                thigh: u,
                shin: u + 1,
                landmarks: [
                    DVec3::new(x, 1., 0.),
                    DVec3::new(x, 0.5, 0.),
                    DVec3::new(x, 0., 0.),
                    DVec3::new(x, 0., -0.15),
                ],
            }
        });
        let out = knee_transitions(
            &points,
            &triangles,
            &vec![true; points.len()],
            &mut rows,
            &chains,
            true,
        )
        .unwrap();
        for (i, side) in out.iter().enumerate() {
            let u = chains[i].thigh;
            let k = chains[i].shin;
            let base = i * 21 * 8;
            for (ring, expected) in [(4, 1.0), (16, 0.0)] {
                let v = base + ring * 8;
                assert_eq!(mass(&rows[v], k), expected * 0.75);
                assert_eq!(mass(&rows[v], u), (1.0 - expected) * 0.75);
                assert_eq!(mass(&rows[v], 4), 0.25);
                assert!(side.targets.iter().all(|t| t[v] == DVec3::ZERO));
            }
            assert!(
                side.targets
                    .iter()
                    .all(|t| t.iter().any(|v| v.length() > 0.001))
            );
            assert_eq!(shape_weights(side.rest_angle, side.rest_angle), [0., 0.]);
            assert_eq!(shape_weights(side.rest_angle, 130.), [0., 1.]);
        }
    }
}
