//! Build the export model from a parsed NIF shape.
//!
//! The builder is the one place a reader-side shape becomes the flat export
//! arena; the NIF reader keeps its own types, and the writer reads the model.
//! A shape carries one [`sculpto_core::mesh::Skin`], so conversion preserves the same
//! value: palette names, binds and the authored influence rows survive without
//! a second projection.

use sculpto_core::mesh::{MeshAsset, PartitionId};

use crate::NifShape;

impl From<&NifShape> for MeshAsset {
    fn from(shape: &NifShape) -> Self {
        Self {
            name: shape.name.clone(),
            positions: shape.positions.clone(),
            uvs: shape.uvs.clone(),
            normals: shape.normals.clone(),
            tangents: shape.tangents.clone(),
            triangles: shape.triangles.clone(),
            // NifShape carries no partition metadata; one partition is the
            // honest default until the reader exposes it.
            triangle_partitions: vec![PartitionId(0); shape.triangles.len()],
            skin: shape.skin.clone(),
            ..Self::default()
        }
    }
}

/// Project the model back to the reader's shape type.
///
/// This is the compatibility path for the existing slice-based consumers:
/// `NifShape -> MeshAsset -> NifShape` preserves every field `NifShape` owns.
/// Materials and morphs have no `NifShape` slot and are dropped by this
/// projection.
impl From<&MeshAsset> for NifShape {
    fn from(mesh: &MeshAsset) -> Self {
        Self {
            name: mesh.name.clone(),
            positions: mesh.positions.clone(),
            uvs: mesh.uvs.clone(),
            normals: mesh.normals.clone(),
            tangents: mesh.tangents.clone(),
            triangles: mesh.triangles.clone(),
            skin: mesh.skin.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use sculpto_core::mesh::Skin;
    use super::*;
    use sculpto_core::skin::SkinWeights;

    fn shape() -> NifShape {
        NifShape {
            name: "body".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2]],
            skin: Some(Skin {
                bones: vec!["root".into(), "spine".into()],
                bind_inverse: vec![[[0.0; 4]; 4]; 2],
                influences: SkinWeights::from_quads(
                    &[[0, 1, 0, 0]; 3],
                    &[[0.75, 0.25, 0.0, 0.0]; 3],
                )
                .expect("a four-lane row fits"),
            }),
        }
    }

    #[test]
    fn builder_maps_arena_uvs_and_skin() {
        let mesh = MeshAsset::from(&shape());
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.uvs.len(), 3);
        assert_eq!(mesh.normals.len(), 3);
        assert_eq!(mesh.tangents.len(), 3);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.triangle_partitions, vec![PartitionId(0)]);
        let skin = mesh.skin.as_ref().expect("skin");
        assert_eq!(skin.bones, vec!["root", "spine"]);
        assert_eq!(skin.influences.rows(), 3);
        assert_eq!(skin.influences.row(0).expect("vertex 0").joints, &[0, 1]);
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
        source.skin = None;
        let mesh = MeshAsset::from(&source);
        assert!(mesh.skin.is_none());
        mesh.validate().expect("valid");
    }
}
