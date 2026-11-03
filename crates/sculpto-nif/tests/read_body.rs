//! Reading a real Skyrim body through the vendored NIF crate.

use sculpto_nif::{NifError, read};

#[test]
fn short_input_is_rejected() {
    let error = read(b"\0\0").expect_err("short input must fail");
    assert_eq!(error, NifError::TooShort);
}

#[test]
fn garbage_is_rejected() {
    let error = read(&[0u8; 64]).expect_err("garbage must fail");
    assert!(
        matches!(error, NifError::TooShort | NifError::Malformed(_)),
        "{error:?}"
    );
}

#[test]
fn foreign_header_is_malformed() {
    let error = read(b"Not a NIF\n\0\0\0\0\0\0\0\0\0").expect_err("foreign header");
    assert!(matches!(error, NifError::Malformed(_)), "{error:?}");
}

#[test]
fn reads_a_real_body() {
    let Some(path) = std::env::var_os("SCULPTO_NIF_FIXTURE") else {
        return;
    };
    let bytes = std::fs::read(&path).expect("fixture is readable");
    let document = read(&bytes).expect("body parses");
    eprintln!(
        "header {:#x} user {} shapes {}",
        document.header.version,
        document.header.user_version,
        document.shapes.len()
    );
    for shape in &document.shapes {
        eprintln!(
            "  {} verts={} tris={} bones={} max_weight_sum={:.4}",
            shape.name,
            shape.positions.len(),
            shape.triangles.len(),
            shape.bones.len(),
            shape.max_weight_sum()
        );
    }
    assert!(!document.shapes.is_empty());
    let body = document
        .shapes
        .iter()
        .find(|shape| shape.positions.len() > 10_000)
        .expect("a body-sized shape");
    assert!(!body.triangles.is_empty());
    assert!(!body.bones.is_empty(), "a body is skinned");
    assert!(
        (body.max_weight_sum() - 1.0).abs() < 0.01,
        "weights normalize"
    );
}
