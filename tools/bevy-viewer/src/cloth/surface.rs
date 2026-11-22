use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;

use super::descriptor::{Binding, ClothSpec};

/// Interpolate a smooth cloth surface between the integrated particles, preserving both faces and the hem.
pub(super) struct Surface {
    offsets: Vec<Vec3>,
}

struct Frame {
    origin: Vec3,
    basis: Mat3,
    normals: [Vec3; 3],
}

impl Frame {
    fn surface_point(&self, u: f32, v: f32) -> Vec3 {
        let x = self.basis.x_axis;
        let y = self.basis.y_axis;
        let relative = u * x + v * y;
        let [na, nb, nc] = self.normals;
        let projection = (1. - u - v) * na * na.dot(relative)
            + u * nb * nb.dot(relative - x)
            + v * nc * nc.dot(relative - y);
        self.origin + relative - projection
    }
}

fn frames(points: &[Vec3], triangles: &[[u32; 3]]) -> Vec<Frame> {
    let mut normals = vec![Vec3::ZERO; points.len()];
    let mut result: Vec<_> = triangles
        .iter()
        .map(|face| {
            let [i, j, k] = face.map(|x| x as usize);
            let x = points[j] - points[i];
            let y = points[k] - points[i];
            let area = x.cross(y);
            for vertex in [i, j, k] {
                normals[vertex] += area;
            }
            Frame {
                origin: points[i],
                basis: Mat3::from_cols(x, y, area.normalize_or_zero()),
                normals: [Vec3::ZERO; 3],
            }
        })
        .collect();
    for normal in &mut normals {
        *normal = normal.normalize_or_zero();
    }
    for (frame, tri) in result.iter_mut().zip(triangles) {
        frame.normals = tri.map(|i| normals[i as usize]);
    }
    result
}

impl Surface {
    pub fn new(mesh: &Mesh, rest: &[Vec3], spec: &ClothSpec) -> Result<Self, String> {
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return Err("cloth render mesh needs float positions".into());
        };
        let triangles = frames(rest, &spec.triangles);
        let offsets = positions
            .iter()
            .zip(&spec.bindings)
            .map(|(&p, &Binding(i, u, v, _))| {
                let frame = &triangles[i];
                if frame.basis.determinant().abs() <= f32::MIN_POSITIVE {
                    return Err("cloth rest triangle is degenerate".into());
                }
                let offset =
                    frame.basis.inverse() * (Vec3::from_array(p) - frame.surface_point(u, v));
                if !offset.is_finite() {
                    return Err("cloth rest data produced an invalid surface binding".into());
                }
                Ok(offset)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { offsets })
    }

    pub fn update(
        &self,
        mesh: &mut Mesh,
        proxy: &[Vec3],
        spec: &ClothSpec,
        seam: impl Iterator<Item = (usize, Vec3)>,
    ) -> usize {
        let triangles = frames(proxy, &spec.triangles);
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            return 0;
        };
        for ((point, &Binding(i, u, v, _)), offset) in
            positions.iter_mut().zip(&spec.bindings).zip(&self.offsets)
        {
            let frame = &triangles[i];
            *point = (frame.surface_point(u, v) + frame.basis * *offset).to_array();
        }
        for (i, exact) in seam {
            positions[i] = exact.to_array();
        }
        cloth_normals(mesh)
    }
}

/// Angle weighting discards millimetre-scale triangles below an absolute threshold in Bevy 0.20.
fn cloth_normals(mesh: &mut Mesh) -> usize {
    mesh.compute_area_weighted_normals();
    match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(normals)) => normals
            .iter()
            .filter(|&&n| Vec3::from_array(n).length_squared() > 0.99)
            .count(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    #[test]
    fn fine_two_sided_cloth_keeps_opposite_unit_normals() {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0., 0., 0.001],
                [0.005, 0., 0.001],
                [0., 0.005, 0.001],
                [0., 0., 0.],
                [0.005, 0., 0.],
                [0., 0.005, 0.],
            ],
        );
        mesh.insert_indices(Indices::U16(vec![0, 1, 2, 3, 5, 4]));
        assert_eq!(cloth_normals(&mut mesh), 6);
        let Some(VertexAttributeValues::Float32x3(n)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!()
        };
        assert!(Vec3::from_array(n[0]).dot(Vec3::Z) > 0.99);
        assert!(Vec3::from_array(n[3]).dot(Vec3::NEG_Z) > 0.99);
    }
}
