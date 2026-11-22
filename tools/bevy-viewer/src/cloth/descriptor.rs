use bevy::prelude::Vec3;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub(super) struct ClothSpec {
    pub version: u32,
    pub source_node: String,
    pub particles: Vec<usize>,
    pub triangles: Vec<[u32; 3]>,
    pub pinned: Vec<u32>,
    pub bindings: Vec<Binding>,
    pub seam: Vec<[usize; 2]>,
    pub colliders: Vec<Capsule>,
    pub material: Material,
}

/// Triangle, two barycentric coordinates and the signed offset from its surface.
#[derive(Clone, Copy, Deserialize)]
pub(super) struct Binding(pub usize, pub f32, pub f32, pub f32);

#[derive(Clone, Deserialize)]
pub(super) struct Capsule {
    pub frame: Frame,
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Frame {
    Joint(usize),
    Node(String),
}

#[derive(Clone, Copy, Deserialize)]
pub(super) struct Material {
    pub stretch_hz: f32,
    pub bend_hz: f32,
    pub damping_ratio: f32,
    pub air_damping: f32,
    pub particle_mass: f32,
    pub thickness: f32,
}

impl ClothSpec {
    pub fn validate(&self, count: usize) -> Result<(), String> {
        if self.version != 1 || self.source_node.trim().is_empty() {
            return Err("expected cloth version 1 and a named skinned source".into());
        }
        let n = self.particles.len();
        if n < 3 || self.triangles.is_empty() || self.pinned.is_empty() || self.pinned.len() >= n {
            return Err("cloth needs a triangulated proxy with pinned and free particles".into());
        }
        for &[a, b, c] in &self.triangles {
            if a == b || a == c || b == c || [a, b, c].into_iter().any(|v| v as usize >= n) {
                return Err("invalid proxy triangle".into());
            }
        }
        if self.pinned.iter().any(|&v| v as usize >= n)
            || self.seam.iter().any(|&[vertex, _]| vertex >= count)
            || self.bindings.len() != count
        {
            return Err("cloth binding count or pin index is invalid".into());
        }
        for &Binding(tri, u, v, h) in &self.bindings {
            if tri >= self.triangles.len() || ![u, v, h].into_iter().all(f32::is_finite) {
                return Err("invalid render-to-proxy binding".into());
            }
        }
        let m = self.material;
        if ![m.stretch_hz, m.bend_hz, m.particle_mass, m.thickness]
            .into_iter()
            .all(|v| v.is_finite() && v > 0.0)
            || ![m.damping_ratio, m.air_damping]
                .into_iter()
                .all(|v| v.is_finite() && v >= 0.0)
        {
            return Err("invalid cloth material".into());
        }
        for capsule in &self.colliders {
            if !(capsule.radius.is_finite() && capsule.radius > 0.0)
                || !Vec3::from_array(capsule.a).is_finite()
                || !Vec3::from_array(capsule.b).is_finite()
                || matches!(&capsule.frame, Frame::Node(n) if n.trim().is_empty())
            {
                return Err("invalid cloth collision capsule".into());
            }
        }
        Ok(())
    }
}
