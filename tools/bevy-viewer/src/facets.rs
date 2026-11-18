//! A display-only mesh variant for inspecting skinned triangle boundaries.

use bevy::mesh::PrimitiveTopology;
use bevy::mesh::{morph::MorphAttributes, VertexAttributeValues};
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
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    let mut flat = mesh.clone();
    flat.try_duplicate_vertices().ok()?;
    let folded = folded_normals(&flat);
    flat.try_compute_flat_normals().ok()?;
    if !folded.is_empty() {
        let VertexAttributeValues::Float32x3(normals) =
            flat.attribute_mut(Mesh::ATTRIBUTE_NORMAL)?
        else {
            return None;
        };
        for (index, original) in folded {
            normals[index..index + 3].copy_from_slice(&original);
        }
    }
    if mesh.try_has_morph_targets().ok()? {
        let count = mesh.count_vertices();
        let source: Vec<_> = match mesh.indices() {
            Some(indices) => indices.iter().collect(),
            None => (0..count).collect(),
        };
        let VertexAttributeValues::Float32x3(positions) =
            flat.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        let VertexAttributeValues::Float32x3(normals) = flat.attribute(Mesh::ATTRIBUTE_NORMAL)?
        else {
            return None;
        };
        let original = mesh.try_morph_targets().ok()?;
        if count == 0 || !original.len().is_multiple_of(count) {
            return None;
        }
        let mut remapped = Vec::with_capacity(source.len().checked_mul(original.len() / count)?);
        for shape in original.chunks_exact(count) {
            for (i, corners) in source.as_chunks::<3>().0.iter().enumerate() {
                let tri = [0, 1, 2]
                    .map(|k| Vec3::from_array(positions[3 * i + k]) + shape[corners[k]].position);
                let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or_zero()
                    - Vec3::from_array(normals[3 * i]);
                for &v in corners {
                    remapped.push(MorphAttributes::new(
                        shape[v].position,
                        normal,
                        shape[v].tangent,
                    ));
                }
            }
        }
        flat.set_morph_targets(remapped);
    }
    Some(flat)
}

/// A bound membrane can open after posing. Its collapsed rest triangle has no geometric normal.
fn folded_normals(flat: &Mesh) -> Vec<(usize, [[f32; 3]; 3])> {
    let (
        Some(_),
        Some(VertexAttributeValues::Float32x3(p)),
        Some(VertexAttributeValues::Float32x3(n)),
    ) = (
        flat.attribute(Mesh::ATTRIBUTE_JOINT_INDEX),
        flat.attribute(Mesh::ATTRIBUTE_POSITION),
        flat.attribute(Mesh::ATTRIBUTE_NORMAL),
    )
    else {
        return Vec::new();
    };
    p.as_chunks::<3>()
        .0
        .iter()
        .zip(n.as_chunks::<3>().0)
        .enumerate()
        .filter_map(|(i, (tri, normals))| {
            let [a, b, c] = tri.map(Vec3::from_array);
            let u = b - a;
            let v = c - a;
            let tol = 4. * f32::EPSILON * u.length_squared().max(v.length_squared());
            (u.cross(v).length_squared() <= tol * tol).then_some((i * 3, *normals))
        })
        .collect()
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
    fn closed_skinned_seam_keeps_authored_normals_for_its_animated_opening() {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::all());
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 0.]],
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0., 0., 1.]; 3]);
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(vec![[0; 4], [0; 4], [1, 0, 0, 0]]),
        );
        let fixed = face_mesh(&mesh).expect("skinned seam");
        assert_eq!(
            fixed.attribute(Mesh::ATTRIBUTE_NORMAL),
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        );
    }

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
        mesh.set_morph_targets(vec![
            MorphAttributes::default(),
            MorphAttributes::default(),
            MorphAttributes::new(Vec3::Z, Vec3::ZERO, Vec3::ZERO),
            MorphAttributes::default(),
        ]);

        let faceted = face_mesh(&mesh).expect("faceted skinned mesh");
        assert_eq!(mesh.count_vertices(), 4);
        assert_eq!(faceted.count_vertices(), 6);
        let Some(VertexAttributeValues::Float32x3(normals)) =
            faceted.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("triangle normals are missing");
        };
        assert_ne!(normals[0], normals[3]);
        let morph = faceted.try_morph_targets().expect("faceted target");
        assert_eq!(morph.len(), 6);
        assert_eq!(morph[2].position, Vec3::Z);
        assert_eq!(morph[3].position, Vec3::ZERO);
        assert!(morph[0].normal.length() > 0.5);
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
