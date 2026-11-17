//! A display-only mesh variant for inspecting skinned triangle boundaries.

use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::{SceneMeshFilter, ViewerConfig};

#[derive(Resource)]
pub(super) struct FaceNormals(pub bool);

#[derive(Component)]
pub(super) struct FaceMesh {
    smooth: Handle<Mesh>,
    faceted: Handle<Mesh>,
}

fn face_mesh(mesh: &Mesh) -> Option<Mesh> {
    // All vertex attributes, including joint indices and weights, duplicate with the face.
    // Morph targets are indexed separately and need a different remap.
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList
        || mesh.try_has_morph_targets().ok()?
    {
        return None;
    }
    let mut mesh = mesh.clone();
    mesh.try_duplicate_vertices().ok()?;
    mesh.try_compute_flat_normals().ok()?;
    Some(mesh)
}

pub(super) fn update(
    mut commands: Commands,
    config: Res<ViewerConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<FaceNormals>,
    mut assets: ResMut<Assets<Mesh>>,
    mut models: Query<(Entity, &mut Mesh3d, Option<&FaceMesh>), SceneMeshFilter>,
) {
    if config.interactive && keys.just_pressed(KeyCode::KeyN) {
        mode.0 = !mode.0;
    }
    for (entity, mut handle, copies) in &mut models {
        if let Some(copies) = copies {
            let wanted = if mode.0 {
                &copies.faceted
            } else {
                &copies.smooth
            };
            if &handle.0 != wanted {
                handle.0 = wanted.clone();
            }
        } else if mode.0 {
            let Some(mesh) = assets.get(&handle.0).and_then(face_mesh) else {
                continue;
            };
            let original = handle.0.clone();
            let faceted = assets.add(mesh);
            handle.0 = faceted.clone();
            commands.entity(entity).insert(FaceMesh {
                smooth: original,
                faceted,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, VertexAttributeValues};

    use super::*;

    #[test]
    fn splits_normals_at_a_crease_without_changing_vertex_skinning() {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(vec![
                [1, 0, 0, 0],
                [2, 0, 0, 0],
                [3, 0, 0, 0],
                [4, 0, 0, 0],
            ]),
        );
        let weights = [
            [1., 0., 0., 0.],
            [0.75, 0.25, 0., 0.],
            [1., 0., 0., 0.],
            [1., 0., 0., 0.],
        ];
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights.to_vec());
        mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 3, 1]));

        let faceted = face_mesh(&mesh).expect("faceted skinned mesh");
        assert_eq!(mesh.count_vertices(), 4);
        assert_eq!(faceted.count_vertices(), 6);
        let Some(VertexAttributeValues::Float32x3(normals)) =
            faceted.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("triangle normals are missing");
        };
        assert_ne!(normals[0], normals[3]);
        assert_eq!(
            faceted.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT),
            Some(&VertexAttributeValues::Float32x4(
                [0, 1, 2, 0, 3, 1].map(|v| weights[v]).to_vec()
            ))
        );
        let Some(VertexAttributeValues::Uint16x4(joints)) =
            faceted.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)
        else {
            panic!("joint indices are missing");
        };
        assert_eq!(
            joints.iter().map(|row| row[0]).collect::<Vec<_>>(),
            [1, 2, 3, 1, 4, 2]
        );
    }
}
