//! Typed writer refusals: every `NifWriteError` variant reachable from the
//! public writers, asserted against the exact variant.
//!
//! Both shape writers build the file in local `Vec<u8>` buffers and hand bytes
//! back only through `Ok`, so `Err(<exact variant>)` is itself the no-bytes
//! guarantee; each test additionally proves the neighbouring valid fixture
//! writes. Read-side header refusals (`TooShort`, `NotGamebryo`,
//! `UnsupportedEndian`) belong to `read_header` and are pinned in
//! `src/write/header.rs`, not here.

use sculpto_core::mesh::{MaterialSlot, MeshAsset, MeshError, PartitionId, ShaderKind, Skin};
use sculpto_nif::NifHeader;
use sculpto_nif::write::header::{NifTarget, NifWriteError, VERSION, write_header_for};
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;

fn triangle() -> MeshAsset {
    MeshAsset {
        name: "Body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

fn skinned(bones: Vec<String>) -> MeshAsset {
    let mut asset = triangle();
    asset.name = "Skinned".into();
    asset.skin = Some(Skin {
        bones,
        // Empty means the identity fallback for every bone.
        bind_inverse: Vec::new(),
        indices: vec![[0, 1, 0, 0]; 3],
        weights: vec![[0.75, 0.25, 0.0, 0.0]; 3],
    });
    asset
}

fn textured() -> MeshAsset {
    let mut asset = triangle();
    asset.name = "Robe".into();
    asset.materials = vec![MaterialSlot {
        name: "robe".into(),
        shader: ShaderKind::Cloth,
        diffuse: Some("textures/robe_d.dds".into()),
        normal: None,
        specular: None,
        double_sided: false,
    }];
    asset
}

/// Negative control: meshes without the defects below write bytes.
#[test]
fn valid_meshes_write_bytes() {
    let plain = triangle();
    assert!(
        !write_se_shape(&plain)
            .expect("SE triangle writes")
            .is_empty()
    );
    assert!(
        !write_le_shape(&plain)
            .expect("LE triangle writes")
            .is_empty()
    );
    let skinned = skinned(vec!["Root".into(), "Spine".into()]);
    assert!(!write_se_shape(&skinned).expect("SE skin writes").is_empty());
    assert!(!write_le_shape(&skinned).expect("LE skin writes").is_empty());
    let textured = textured();
    assert!(
        !write_se_shape(&textured)
            .expect("SE material writes")
            .is_empty()
    );
    assert!(
        !write_le_shape(&textured)
            .expect("LE material writes")
            .is_empty()
    );
}

#[test]
fn unsupported_games_and_versions_are_refused() {
    assert_eq!(
        write_header_for(NifHeader {
            version: VERSION,
            user_version: 20
        }),
        Err(NifWriteError::UnsupportedGame { user_version: 20 })
    );
    assert_eq!(
        write_header_for(NifHeader {
            version: 0x1402_0008,
            user_version: 12
        }),
        Err(NifWriteError::UnsupportedVersion {
            version: 0x1402_0008
        })
    );
}

#[test]
fn invalid_models_are_refused() {
    let mut asset = triangle();
    asset.triangles[0] = [0, 1, 3];
    let expected = NifWriteError::InvalidModel(MeshError::TriangleVertexOutOfRange {
        triangle: 0,
        vertex: 3,
    });
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn too_many_vertices_at_the_u16_boundary_are_refused() {
    let mut asset = triangle();
    asset.positions = vec![[0.0; 3]; u16::MAX as usize + 1];
    let expected = NifWriteError::TooManyVertices {
        vertices: u16::MAX as usize + 1,
    };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn too_many_triangles_at_the_u16_boundary_are_refused() {
    let mut asset = triangle();
    let count = u16::MAX as usize + 1;
    asset.triangles = vec![[0, 1, 2]; count];
    asset.triangle_partitions = vec![PartitionId(0); count];
    let expected = NifWriteError::TooManyTriangles { triangles: count };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn se_tangents_without_normals_are_refused() {
    let mut asset = triangle();
    asset.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
    assert_eq!(
        write_se_shape(&asset),
        Err(NifWriteError::TangentsWithoutNormals)
    );
}

#[test]
fn se_zero_normals_are_refused() {
    let mut asset = triangle();
    asset.normals = vec![[0.0; 3]; 3];
    assert_eq!(
        write_se_shape(&asset),
        Err(NifWriteError::ZeroVector {
            attribute: "normal",
            vertex: 0
        })
    );
}

#[test]
fn se_zero_tangents_are_refused() {
    let mut asset = triangle();
    asset.normals = vec![[0.0, 0.0, 1.0]; 3];
    asset.tangents = vec![[0.0, 0.0, 0.0, 1.0]; 3];
    assert_eq!(
        write_se_shape(&asset),
        Err(NifWriteError::ZeroVector {
            attribute: "tangent",
            vertex: 0
        })
    );
}

/// The on-disk tangent is `cross(normal, bitangent)`; a tangent parallel to
/// its normal makes that cross product zero.
#[test]
fn se_tangents_parallel_to_their_normals_are_refused() {
    let mut asset = triangle();
    asset.normals = vec![[0.0, 0.0, 1.0]; 3];
    asset.tangents = vec![[0.0, 0.0, 1.0, 1.0]; 3];
    assert_eq!(
        write_se_shape(&asset),
        Err(NifWriteError::ZeroVector {
            attribute: "tangent",
            vertex: 0
        })
    );
}

#[test]
fn le_tangents_are_refused() {
    let mut asset = triangle();
    asset.normals = vec![[0.0, 0.0, 1.0]; 3];
    asset.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
    assert_eq!(
        write_le_shape(&asset),
        Err(NifWriteError::TangentsUnsupported {
            target: NifTarget::Le
        })
    );
}

#[test]
fn unnamed_bones_are_refused() {
    let asset = skinned(vec!["Root".into(), "  ".into()]);
    let expected = NifWriteError::UnnamedBone { index: 1 };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn duplicate_bones_are_refused() {
    let asset = skinned(vec!["Root".into(), "Root".into()]);
    let expected = NifWriteError::DuplicateBone {
        name: "Root".into(),
    };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn bone_names_beyond_the_string_limit_are_refused() {
    let long = "x".repeat(u8::MAX as usize + 1);
    let asset = skinned(vec!["Root".into(), long.clone()]);
    let expected = NifWriteError::NameTooLong { name: long };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn too_many_bones_are_refused() {
    let bones = (0..=u8::MAX as usize)
        .map(|index| format!("Bone{index}"))
        .collect();
    let asset = skinned(bones);
    let expected = NifWriteError::TooManyBones {
        bones: u8::MAX as usize + 1,
    };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn singular_bind_matrices_are_refused() {
    let mut asset = skinned(vec!["Root".into(), "Spine".into()]);
    let skin = asset.skin.as_mut().expect("skin");
    let mut bind = vec![[[0.0f32; 4]; 4]; 2];
    bind[0][0][0] = 1.0;
    bind[0][1][1] = 1.0;
    bind[0][2][2] = 1.0;
    bind[0][3][3] = 1.0;
    skin.bind_inverse = bind;
    let expected = NifWriteError::SingularBind { bone: 1 };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}

#[test]
fn specular_textures_are_refused() {
    let expected = NifWriteError::TextureSlotUnsupported { slot: "specular" };
    let mut se = textured();
    se.materials[0].specular = Some("textures/robe_s.dds".into());
    assert_eq!(write_se_shape(&se), Err(expected.clone()));
    let mut le = textured();
    le.materials[0].specular = Some("textures/robe_s.dds".into());
    assert_eq!(write_le_shape(&le), Err(expected));
}

#[test]
fn multiple_material_slots_are_refused() {
    let mut asset = textured();
    let mut second = asset.materials[0].clone();
    second.name = "robe2".into();
    asset.materials.push(second);
    let expected = NifWriteError::MultipleMaterialSlots { count: 2 };
    assert_eq!(write_se_shape(&asset), Err(expected.clone()));
    assert_eq!(write_le_shape(&asset), Err(expected));
}
