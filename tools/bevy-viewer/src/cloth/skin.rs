use bevy::mesh::{Mesh, VertexAttributeValues};
use bevy::prelude::{Mat4, Vec3};

/// CPU sampling uses the same morph deltas and inverse-bind joint transforms as the displayed shoulder.
pub(super) struct SkinSample {
    pub rest: Vec3,
    joints: [usize; 4],
    weights: [f32; 4],
    morphs: Vec<(usize, Vec3)>,
}
impl SkinSample {
    pub fn read(mesh: &Mesh, index: usize, joint_count: usize) -> Result<Self, String> {
        let Some(VertexAttributeValues::Float32x3(p)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return Err("cloth source needs positions".into());
        };
        let Some(VertexAttributeValues::Uint16x4(j)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
        else {
            return Err("cloth source needs four joint indices".into());
        };
        let Some(VertexAttributeValues::Float32x4(w)) =
            mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT)
        else {
            return Err("cloth source needs four skin weights".into());
        };
        let rest = Vec3::from_array(*p.get(index).ok_or("cloth source vertex is out of range")?);
        let joints = j
            .get(index)
            .ok_or("missing joint indices")?
            .map(usize::from);
        let weights = *w.get(index).ok_or("missing joint weights")?;
        if !rest.is_finite()
            || joints.iter().any(|&v| v >= joint_count)
            || !weights.iter().all(|x| x.is_finite())
        {
            return Err("invalid cloth source skinning".into());
        }
        let mut morphs = Vec::new();
        if let Some(deltas) = mesh.get_morph_targets() {
            for (shape, values) in deltas.chunks_exact(p.len()).enumerate() {
                let delta = values[index].position;
                if delta != Vec3::ZERO {
                    morphs.push((shape, delta));
                }
            }
        }
        Ok(Self {
            rest,
            joints,
            weights,
            morphs,
        })
    }
    pub fn world(&self, joints: &[Mat4], weights: &[f32]) -> Vec3 {
        let posed = self.morphs.iter().fold(self.rest, |p, (i, d)| {
            p + *d * weights.get(*i).copied().unwrap_or(0.0)
        });
        self.joints
            .iter()
            .zip(self.weights)
            .map(|(&j, w)| joints[j].transform_point3(posed) * w)
            .sum()
    }
}
