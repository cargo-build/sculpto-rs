//! A real skinned body through the SE writer and back.
//!
//! Private-host tier: the body NIF is reached through `SCULPTO_BIJIN_NIF`; the
//! test skips without it, so `just gate` stays portable.

use sculpto_core::mesh::MeshAsset;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{NifShape, read};

#[test]
fn bijin_body_skin_roundtrips_through_the_se_writer() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        return;
    };
    let bytes = std::fs::read(&path).expect("Bijin NIF is readable");
    let document = read(&bytes).expect("Bijin NIF parses");
    let source = document
        .shapes
        .into_iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    let mesh = MeshAsset::from(&source);
    mesh.validate().expect("the body is a valid model");

    let written = write_se_shape(&mesh).expect("the body writes");
    let reread = read(&written).expect("our SE NIF parses");
    let back = NifShape::from(&MeshAsset::from(&reread.shapes[0]));

    assert_eq!(back.bones, source.bones);
    assert_eq!(back.triangles, source.triangles);
    assert_eq!(back.bone_indices, source.bone_indices);
    for (actual, expected) in back.weights.iter().zip(&source.weights) {
        for (left, right) in actual.iter().zip(expected) {
            assert!((left - right).abs() <= 1e-3, "f16 weight drift");
        }
    }
    for (actual, expected) in back.bind_inverse.iter().zip(&source.bind_inverse) {
        for column in 0..4 {
            for row in 0..4 {
                assert!(
                    (actual[column][row] - expected[column][row]).abs() <= 1e-4,
                    "bind matrix drift"
                );
            }
        }
    }
    assert!(
        !source.bones.is_empty(),
        "the body must carry a skin for this test to mean anything"
    );
}
