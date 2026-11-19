//! Reconstruct a curved height field on the baked patch and match its swept volume.

use glam::{DMat3, DVec2, DVec3, Vec3, Vec3Swizzles};
use nalgebra::{DMatrix, DVector};

use super::{Mask, NeutralizeError, PatchReport};

pub(super) struct Patch {
    collar: Vec<usize>,
    weights: Vec<f64>,
    volume_weights: Vec<f64>,
    core: Vec<HarmonicRow>,
    faces: Vec<(usize, [usize; 3])>,
    frame_faces: Vec<[usize; 3]>,
}

struct HarmonicRow {
    vertex: usize,
    diagonal: f64,
    free: Vec<(usize, f64)>,
    rim: Vec<(usize, f64)>,
}

struct Frame {
    origin: DVec3,
    axes: DMat3,
    radius: f64,
}

impl Frame {
    fn project(&self, point: DVec3) -> DVec3 {
        self.axes.transpose() * (point - self.origin) / self.radius
    }

    fn unproject(&self, point: DVec3) -> DVec3 {
        self.origin + self.axes * (point * self.radius)
    }
}

impl Patch {
    pub(super) fn bind(
        mask: &Mask,
        core_radius_fraction: f64,
        base: &[[f32; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<Self, NeutralizeError> {
        let bad_mask =
            || NeutralizeError::InvalidMesh("invalid vertex in bundled neutralization mask");
        if !(0.0..1.0).contains(&core_radius_fraction) {
            return Err(NeutralizeError::InvalidMesh(
                "invalid core radius in neutralization profile",
            ));
        }
        let mut weights = vec![0.0; base.len()];
        let mut volume_weights = vec![0.0; base.len()];
        let mut free_row = vec![None; base.len()];
        let mut core_vertices = Vec::new();
        let mut sample = vec![false; base.len()];
        for &(vertex, distance) in &mask.vertices {
            let vertex = vertex as usize;
            let radius = f64::from(distance) / f64::from(u16::MAX);
            let fade = ((1.0 - radius) / (1.0 - core_radius_fraction)).clamp(0.0, 1.0);
            *weights.get_mut(vertex).ok_or_else(bad_mask)? = fade * fade * (3.0 - 2.0 * fade);
            volume_weights[vertex] = (1.0 - radius.powi(4)).powi(3);
            if radius < core_radius_fraction {
                free_row[vertex] = Some(core_vertices.len());
                core_vertices.push(vertex);
            }
            sample[vertex] = true;
        }
        let collar: Vec<_> = mask.collar.iter().map(|&vertex| vertex as usize).collect();
        for &vertex in &collar {
            *sample.get_mut(vertex).ok_or_else(bad_mask)? = true;
        }
        let mut neighbors = vec![Vec::new(); core_vertices.len()];
        let mut faces = Vec::new();
        let mut frame_faces = Vec::new();
        for (index, triangle) in triangles.iter().enumerate() {
            let face = triangle.map(|vertex| vertex as usize);
            if face.iter().any(|&vertex| weights[vertex] > 0.0) {
                faces.push((index, face));
                for corner in 0..3 {
                    if let Some(row) = free_row[face[corner]] {
                        neighbors[row].extend([face[(corner + 1) % 3], face[(corner + 2) % 3]]);
                    }
                }
            }
            if face.iter().all(|&vertex| sample[vertex]) {
                frame_faces.push(face);
            }
        }
        let core = core_vertices
            .into_iter()
            .zip(neighbors)
            .map(|(vertex, mut neighbors)| {
                let point = Vec3::from(base[vertex]).as_dvec3();
                neighbors.sort_unstable();
                neighbors.dedup();
                let mut row = HarmonicRow {
                    vertex,
                    diagonal: 0.0,
                    free: Vec::new(),
                    rim: Vec::new(),
                };
                for neighbor in neighbors {
                    let length = point.distance(Vec3::from(base[neighbor]).as_dvec3());
                    if !length.is_finite() || length <= 0.0 {
                        return Err(NeutralizeError::InvalidMesh(
                            "invalid source edge in neutralization mask",
                        ));
                    }
                    let weight = length.recip();
                    row.diagonal += weight;
                    match free_row[neighbor] {
                        Some(index) => row.free.push((index, weight)),
                        None => row.rim.push((neighbor, weight)),
                    }
                }
                Ok(row)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            collar,
            weights,
            volume_weights,
            core,
            faces,
            frame_faces,
        })
    }

    pub(super) fn weights(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.weights
            .iter()
            .copied()
            .enumerate()
            .filter(|&(_, weight)| weight > 0.0)
    }

    pub(super) fn fair(
        &self,
        region: usize,
        before: &[DVec3],
        proposed: &mut [[f32; 3]],
    ) -> Result<PatchReport, NeutralizeError> {
        let failure = |reason| NeutralizeError::Patch { region, reason };
        let frame = self.frame(before).map_err(failure)?;
        let original: Vec<_> = before.iter().map(|&point| frame.project(point)).collect();
        let mut local = original.clone();
        let fit = fit_collar(&self.collar, &original).map_err(failure)?;
        // A height-only fit can fold the triangles around a steep cap. Spread its
        // interior in the tangent plane, using source edge lengths to keep vertex spacing.
        let planar = solve_tangent_plane(&self.core, &local).map_err(failure)?;
        for (row, point) in self.core.iter().zip(planar) {
            local[row.vertex].x = point.x;
            local[row.vertex].y = point.y;
        }
        for (vertex, weight) in self.weights() {
            let row = &mut local[vertex];
            let surface = cubic(row.xy())
                .iter()
                .zip(fit.iter())
                .map(|(a, b)| a * b)
                .sum::<f64>();
            row.z += weight * (surface - row.z);
        }
        // With a fixed rim the swept volume is linear in depth. Spread the
        // correction across the whole patch to avoid a raised ring at the core.
        let volume_before = self.volume(|vertex| original[vertex]);
        let change = self.volume(|vertex| local[vertex]) - volume_before;
        let slope = self.volume(|vertex| local[vertex].xy().extend(self.volume_weights[vertex]));
        if !change.is_finite() || !slope.is_finite() || slope.abs() <= 1e-10 {
            return Err(failure("invalid volume correction"));
        }
        let mut report = PatchReport {
            moved_vertices: 0,
            touched_triangles: self.faces.len(),
            max_displacement: 0.0,
            min_normal_dot: 1.0,
            min_double_area: f64::INFINITY,
            volume_delta: 0.0,
        };
        for (vertex, _) in self.weights() {
            local[vertex].z -= change / slope * self.volume_weights[vertex];
            let position = frame.unproject(local[vertex]).as_vec3();
            if !position.is_finite() {
                return Err(failure("non-finite fitted vertex"));
            }
            let distance = position.as_dvec3().distance(before[vertex]);
            report.moved_vertices += usize::from(distance > 0.0);
            report.max_displacement = report.max_displacement.max(distance);
            proposed[vertex] = position.to_array();
        }
        for &(index, face) in &self.faces {
            let original_normal = face_normal(face.map(|vertex| before[vertex]));
            let new_normal =
                face_normal(face.map(|vertex| Vec3::from(proposed[vertex]).as_dvec3()));
            let (alignment, area) = check_face(index, original_normal, new_normal, &frame)?;
            report.min_normal_dot = report.min_normal_dot.min(alignment);
            report.min_double_area = report.min_double_area.min(area);
        }
        let volume_after =
            self.volume(|vertex| frame.project(Vec3::from(proposed[vertex]).as_dvec3()));
        let residual = volume_after - volume_before;
        if !residual.is_finite() || residual.abs() > 2e-5 {
            return Err(failure(
                "rounded positions exceed the patch volume tolerance",
            ));
        }
        report.volume_delta = residual * frame.radius.powi(3);
        Ok(report)
    }

    fn frame(&self, points: &[DVec3]) -> Result<Frame, &'static str> {
        let normal = self
            .frame_faces
            .iter()
            .map(|face| face_normal(face.map(|vertex| points[vertex])))
            .sum::<DVec3>()
            .try_normalize()
            .ok_or("cannot orient the patch")?;
        let (u, v) = normal.any_orthonormal_pair();
        let origin = self
            .collar
            .iter()
            .map(|&vertex| points[vertex])
            .sum::<DVec3>()
            / self.collar.len() as f64;
        let radius = (self
            .collar
            .iter()
            .map(|&vertex| {
                let point = points[vertex] - origin;
                point.dot(u).powi(2) + point.dot(v).powi(2)
            })
            .sum::<f64>()
            / self.collar.len() as f64)
            .sqrt();
        if !radius.is_finite() || radius <= 0.0 {
            return Err("collapsed patch collar");
        }
        Ok(Frame {
            origin,
            axes: DMat3::from_cols(u, v, normal),
            radius,
        })
    }

    fn volume(&self, point: impl Fn(usize) -> DVec3) -> f64 {
        self.faces
            .iter()
            .map(|&(_, [a, b, c])| point(a).dot(point(b).cross(point(c))))
            .sum::<f64>()
            / 6.0
    }
}

fn cubic(uv: DVec2) -> [f64; 10] {
    let DVec2 { x: u, y: v } = uv;
    [
        1.0,
        u,
        v,
        u * u,
        u * v,
        v * v,
        u.powi(3),
        u * u * v,
        u * v * v,
        v.powi(3),
    ]
}

fn fit_collar(collar: &[usize], positions: &[DVec3]) -> Result<DVector<f64>, &'static str> {
    if collar.len() < 10 {
        return Err("insufficient collar samples");
    }
    let rows = collar
        .iter()
        .flat_map(|&vertex| cubic(positions[vertex].xy()));
    let matrix = DMatrix::from_row_iterator(collar.len(), 10, rows);
    let depths = DVector::from_iterator(
        collar.len(),
        collar.iter().map(|&vertex| positions[vertex].z),
    );
    let svd = matrix.svd(true, true);
    let tolerance = svd.singular_values[0] * 1e-10;
    if svd.singular_values.iter().any(|&value| value <= tolerance) {
        return Err("rank-deficient surface fit");
    }
    svd.solve(&depths, tolerance)
}

fn solve_tangent_plane(rows: &[HarmonicRow], points: &[DVec3]) -> Result<Vec<DVec2>, &'static str> {
    let apply = |row: &HarmonicRow, i: usize, field: &[DVec2]| {
        row.diagonal * field[i] - row.free.iter().map(|&(j, w)| w * field[j]).sum::<DVec2>()
    };
    let mut x: Vec<_> = rows.iter().map(|row| points[row.vertex].xy()).collect();
    let rhs: Vec<_> = rows
        .iter()
        .map(|row| {
            row.rim
                .iter()
                .map(|&(vertex, weight)| points[vertex].xy() * weight)
                .sum::<DVec2>()
        })
        .collect();
    let mut residual: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| rhs[i] - apply(row, i, &x))
        .collect();
    let tolerance = dot_field(&rhs, &rhs).max(1e-6) * 1e-20;
    let mut direction: Vec<_> = residual
        .iter()
        .zip(rows)
        .map(|(&r, row)| r / row.diagonal)
        .collect();
    let mut energy = dot_field(&residual, &direction);
    let mut applied = vec![DVec2::ZERO; rows.len()];
    // Diagonally preconditioned conjugate gradients on the fixed-rim graph.
    for _ in 0..rows.len().max(32) {
        if dot_field(&residual, &residual) <= tolerance {
            return Ok(x);
        }
        applied
            .iter_mut()
            .zip(rows)
            .enumerate()
            .for_each(|(i, (value, row))| {
                *value = apply(row, i, &direction);
            });
        let step = energy / dot_field(&direction, &applied);
        if !step.is_finite() || step <= 0.0 {
            return Err("singular planar solve");
        }
        for i in 0..rows.len() {
            x[i] += step * direction[i];
            residual[i] -= step * applied[i];
        }
        let next = residual
            .iter()
            .zip(rows)
            .map(|(r, row)| r.length_squared() / row.diagonal)
            .sum::<f64>();
        let ratio = next / energy;
        for (direction, (&r, row)) in direction.iter_mut().zip(residual.iter().zip(rows)) {
            *direction = r / row.diagonal + ratio * *direction;
        }
        energy = next;
    }
    Err("planar solve did not converge")
}

fn dot_field(a: &[DVec2], b: &[DVec2]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a.dot(*b)).sum()
}

fn face_normal([a, b, c]: [DVec3; 3]) -> DVec3 {
    (b - a).cross(c - a)
}

fn check_face(
    index: usize,
    before: DVec3,
    after: DVec3,
    frame: &Frame,
) -> Result<(f64, f64), NeutralizeError> {
    let area = after.length();
    let prior_area = before.length();
    let floor = 1e-8 * frame.radius.powi(2);
    let error = |reason| NeutralizeError::Triangle { index, reason };
    if prior_area <= floor {
        return Err(error("degenerate input in the edit region"));
    }
    if !area.is_finite() || area <= floor.max(prior_area * 1e-5) {
        return Err(error("patch would create a degenerate face"));
    }
    let alignment = after.dot(before) / (area * prior_area);
    if alignment <= 0.0 || after.dot(frame.axes.z_axis) <= 0.0 {
        return Err(error("patch would leave an inverted face"));
    }
    Ok((alignment, area))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raised_cap_is_smoothed_against_the_current_collar_with_a_fixed_rim() {
        let mut mask = Mask {
            vertices: Vec::new(),
            collar: Vec::new(),
        };
        let mut positions = Vec::new();
        let width = 17u32;
        for y in 0..width {
            for x in 0..width {
                let index = x + y * width;
                let u = (x as f32 - 8.0) / 4.0;
                let v = (y as f32 - 8.0) / 4.0;
                let r = u.hypot(v);
                let cap = 0.2 * (1.0 - r / 0.6).max(0.0).powi(2);
                positions.push([u, v, -0.18 * r * r + cap]);
                if r < 1.05 {
                    mask.vertices
                        .push((index, (r / 1.05 * f32::from(u16::MAX)).round() as u16));
                } else if r < 1.55 {
                    mask.collar.push(index);
                }
            }
        }
        let mut triangles = Vec::new();
        for y in 0..width - 1 {
            for x in 0..width - 1 {
                let i = x + y * width;
                triangles.extend([[i, i + 1, i + width + 1], [i, i + width + 1, i + width]]);
            }
        }
        let prepared = Patch::bind(&mask, 0.65 / 1.05, &positions, &triangles).unwrap();
        let original: Vec<_> = positions
            .iter()
            .map(|&point| Vec3::from(point).as_dvec3())
            .collect();
        let mut edited = positions.clone();
        let report = prepared.fair(0, &original, &mut edited).unwrap();
        let peak = (8 + 8 * width) as usize;
        assert!(edited[peak][2] < positions[peak][2] - 0.1);
        assert!(report.volume_delta.abs() < 1e-6, "{report:?}");
        assert!(report.min_normal_dot > 0.0);
        for (vertex, &weight) in prepared.weights.iter().enumerate() {
            if weight == 0.0 {
                assert_eq!(edited[vertex], positions[vertex]);
            }
        }
    }

    #[test]
    fn the_face_guard_rejects_zero_area_and_reversed_winding() {
        let frame = Frame {
            origin: DVec3::ZERO,
            axes: DMat3::IDENTITY,
            radius: 1.0,
        };
        assert_eq!(check_face(0, DVec3::Z, DVec3::Z, &frame), Ok((1.0, 1.0)));
        assert!(check_face(0, DVec3::Z, DVec3::ZERO, &frame).is_err());
        assert!(check_face(0, DVec3::Z, DVec3::NEG_Z, &frame).is_err());
    }
}
