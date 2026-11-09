//! Build the export model from a parsed NIF shape.
//!
//! The builder is the one place a reader-side shape becomes the flat export
//! arena; the NIF reader keeps its own types, and the writer reads the model.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};

use crate::NifShape;

impl From<&NifShape> for MeshAsset {
    fn from(shape: &NifShape) -> Self {
        let skin = (!shape.bones.is_empty()).then(|| Skin {
            bones: shape.bones.clone(),
            indices: shape.bone_indices.clone(),
            weights: shape.weights.clone(),
        });
        Self {
            name: shape.name.clone(),
            positions: shape.positions.clone(),
            uvs: shape.uvs.clone(),
            triangles: shape.triangles.clone(),
            // NifShape carries no partition metadata; one partition is the
            // honest default until the reader exposes it.
            triangle_partitions: vec![PartitionId(0); shape.triangles.len()],
            skin,
            ..Self::default()
        }
    }
}

/// Project the model back to the reader's shape type.
///
/// This is the compatibility path for the existing slice-based consumers:
/// `NifShape -> MeshAsset -> NifShape` preserves every field `NifShape` owns.
/// Normals, tangents, materials and morphs have no `NifShape` slot and are
/// dropped by this projection.
impl From<&MeshAsset> for NifShape {
    fn from(mesh: &MeshAsset) -> Self {
        let (bones, bone_indices, weights) = match &mesh.skin {
            Some(skin) => (
                skin.bones.clone(),
                skin.indices.clone(),
                skin.weights.clone(),
            ),
            None => (Vec::new(), Vec::new(), Vec::new()),
        };
        Self {
            name: mesh.name.clone(),
            positions: mesh.positions.clone(),
            uvs: mesh.uvs.clone(),
            triangles: mesh.triangles.clone(),
            bones,
            bone_indices,
            weights,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> NifShape {
        NifShape {
            name: "body".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            bones: vec!["root".into(), "spine".into()],
            bone_indices: vec![[0, 1, 0, 0]; 3],
            weights: vec![[0.75, 0.25, 0.0, 0.0]; 3],
        }
    }

    #[test]
    fn builder_maps_arena_uvs_and_skin() {
        let mesh = MeshAsset::from(&shape());
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.uvs.len(), 3);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.triangle_partitions, vec![PartitionId(0)]);
        let skin = mesh.skin.as_ref().expect("skin");
        assert_eq!(skin.bones, vec!["root", "spine"]);
        assert_eq!(skin.indices.len(), 3);
        assert_eq!(skin.weights.len(), 3);
        assert!(mesh.morphs.is_empty());
        assert!(mesh.materials.is_empty());
        mesh.validate().expect("valid");
    }

    #[test]
    fn shape_roundtrips_through_the_model() {
        let source = shape();
        let mesh = MeshAsset::from(&source);
        let back = NifShape::from(&mesh);
        assert_eq!(back, source);
        mesh.validate().expect("valid");
    }

    #[test]
    fn an_unskinned_shape_has_no_skin() {
        let mut source = shape();
        source.bones.clear();
        source.bone_indices.clear();
        source.weights.clear();
        let mesh = MeshAsset::from(&source);
        assert!(mesh.skin.is_none());
        mesh.validate().expect("valid");
    }
}
