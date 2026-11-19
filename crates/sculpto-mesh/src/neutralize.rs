//! Fixed-rim removal of local chest relief on a profiled BodySlide topology.

mod patch;

use std::fmt;
use std::sync::LazyLock;

use glam::Vec3;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::surface_util::vertex_normals;
use patch::Patch;

/// A topology or geometric refusal. The caller can continue with another bake.
#[derive(Debug, PartialEq, Eq)]
pub enum NeutralizeError {
    /// The indexed topology has no authored vertex mask.
    UnsupportedTopology {
        vertices: usize,
        triangles: usize,
        fingerprint: String,
    },
    /// The arrays or bundled mask cannot supply the patch.
    InvalidMesh(&'static str),
    /// The current shape cannot supply a well-conditioned local surface.
    Patch { region: usize, reason: &'static str },
    /// A face failed the area or orientation check.
    Triangle { index: usize, reason: &'static str },
}

impl fmt::Display for NeutralizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTopology {
                vertices,
                triangles,
                fingerprint,
            } => write!(
                f,
                "no neutralization profile for {vertices} vertices, {triangles} triangles (sha256 {fingerprint})"
            ),
            Self::InvalidMesh(reason) => f.write_str(reason),
            Self::Patch { region, reason } => write!(f, "patch {region}: {reason}"),
            Self::Triangle { index, reason } => write!(f, "triangle {index}: {reason}"),
        }
    }
}

impl std::error::Error for NeutralizeError {}

/// Measurements for one fixed-rim patch, in the input coordinate units.
#[derive(Debug, Serialize)]
pub struct PatchReport {
    pub moved_vertices: usize,
    pub touched_triangles: usize,
    pub max_displacement: f64,
    pub min_normal_dot: f64,
    pub min_double_area: f64,
    /// Signed swept volume after rounding the edited positions back to f32.
    pub volume_delta: f64,
}

/// Successful application of one profile to both patches.
#[derive(Debug, Serialize)]
pub struct NeutralizeReport {
    pub profile: &'static str,
    pub topology_sha256: &'static str,
    pub patches: [PatchReport; 2],
}

#[derive(Deserialize)]
struct Profile {
    name: String,
    topology_sha256: String,
    core_radius_fraction: f64,
    patches: [Mask; 2],
}

#[derive(Deserialize)]
struct Mask {
    /// Vertex and normalized distance from the authored center, in units of 1/65535.
    vertices: Vec<(u32, u16)>,
    collar: Vec<u32>,
}

static PROFILE: LazyLock<Result<Profile, serde_json::Error>> =
    LazyLock::new(|| serde_json::from_str(include_str!("neutralize/cbbe-3ba-v2.json")));

/// A mask and fixed-rim planar solve bound once to a source vertex arena.
pub struct Neutralizer {
    profile: &'static Profile,
    count: usize,
    triangles: Vec<[u32; 3]>,
    patches: [Patch; 2],
}

impl Neutralizer {
    /// Match the identified topology and prepare its source-weighted adjacency.
    pub fn bind(base: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Self, NeutralizeError> {
        let profile = PROFILE
            .as_ref()
            .map_err(|_| NeutralizeError::InvalidMesh("invalid bundled neutralization profile"))?;
        let fingerprint = topology_fingerprint(base.len(), triangles)?;
        if fingerprint != profile.topology_sha256 {
            return Err(NeutralizeError::UnsupportedTopology {
                vertices: base.len(),
                triangles: triangles.len(),
                fingerprint,
            });
        }
        let [left, right] = &profile.patches;
        Ok(Self {
            profile,
            count: base.len(),
            triangles: triangles.to_vec(),
            patches: [
                Patch::bind(left, profile.core_radius_fraction, base, triangles)?,
                Patch::bind(right, profile.core_radius_fraction, base, triangles)?,
            ],
        })
    }

    /// Fair the baked positions in their bound vertex order, updating only patch normals.
    /// Both patches are checked before any caller-owned row is changed.
    pub fn apply(
        &self,
        positions: &mut [[f32; 3]],
        normals: &mut [[f32; 3]],
    ) -> Result<NeutralizeReport, NeutralizeError> {
        if positions.len() != self.count || normals.len() != self.count {
            return Err(NeutralizeError::InvalidMesh(
                "baked rows do not match the bound arena",
            ));
        }
        if positions
            .iter()
            .flatten()
            .chain(normals.iter().flatten())
            .any(|v| !v.is_finite())
        {
            return Err(NeutralizeError::InvalidMesh(
                "baked positions and normals must be finite",
            ));
        }
        let original: Vec<_> = positions
            .iter()
            .map(|&row| Vec3::from(row).as_dvec3())
            .collect();
        let mut proposed = positions.to_vec();
        let [left, right] = &self.patches;
        let left_report = left.fair(0, &original, &mut proposed)?;
        let right_report = right.fair(1, &original, &mut proposed)?;
        let geometric = vertex_normals(&proposed, &self.triangles);
        let updates = self
            .patches
            .iter()
            .flat_map(Patch::weights)
            .map(|(vertex, _)| {
                let normal = Vec3::from(geometric[vertex]);
                if !normal.is_finite() || normal == Vec3::ZERO {
                    return Err(NeutralizeError::InvalidMesh("cannot update a patch normal"));
                }
                Ok((vertex, normal.to_array()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        positions.copy_from_slice(&proposed);
        for (vertex, normal) in updates {
            normals[vertex] = normal;
        }
        Ok(NeutralizeReport {
            profile: &self.profile.name,
            topology_sha256: &self.profile.topology_sha256,
            patches: [left_report, right_report],
        })
    }
}

fn topology_fingerprint(count: usize, triangles: &[[u32; 3]]) -> Result<String, NeutralizeError> {
    let count = u32::try_from(count)
        .map_err(|_| NeutralizeError::InvalidMesh("vertex count exceeds the index range"))?;
    if triangles.iter().flatten().any(|&index| index >= count) {
        return Err(NeutralizeError::InvalidMesh(
            "triangle index outside the vertex arena",
        ));
    }
    // Canonicalize face order and the first corner while retaining winding and vertex IDs.
    let mut indexed: Vec<_> = triangles
        .iter()
        .map(|&[a, b, c]| {
            let mut rotations = [[a, b, c], [b, c, a], [c, a, b]];
            rotations.sort_unstable();
            rotations[0]
        })
        .collect();
    indexed.sort_unstable();
    let mut hash = Sha256::new();
    hash.update(count.to_le_bytes());
    for index in indexed.into_iter().flatten() {
        hash.update(index.to_le_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_accepts_face_order_but_refuses_winding_or_index_changes() {
        let fingerprint = topology_fingerprint(5, &[[0, 1, 2], [1, 3, 2]]).unwrap();
        assert_eq!(
            fingerprint,
            topology_fingerprint(5, &[[2, 1, 3], [1, 2, 0]]).unwrap()
        );
        for triangles in [[[0, 2, 1], [1, 3, 2]], [[0, 1, 4], [1, 3, 4]]] {
            assert_ne!(fingerprint, topology_fingerprint(5, &triangles).unwrap());
        }
        assert_ne!(
            fingerprint,
            topology_fingerprint(6, &[[0, 1, 2], [1, 3, 2]]).unwrap()
        );
        assert!(matches!(
            Neutralizer::bind(&[[0.0; 3]; 5], &[[0, 1, 2]]),
            Err(NeutralizeError::UnsupportedTopology { .. })
        ));
    }
}
