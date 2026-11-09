//! Golden fixture for the accepted Bijin CBBE female body.
//!
//! The NIF is a private host fixture reached through `SCULPTO_BIJIN_NIF`; the
//! committed descriptor carries the counts and position hash. Without the env
//! variable the test skips, so `just gate` stays portable.

use std::path::PathBuf;

use sculpto_core::mesh::MeshAsset;

fn fnv1a64(positions: &[[f32; 3]]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for position in positions {
        for value in position {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    format!("{hash:016x}")
}

fn descriptor_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bijin-cbbe-golden.txt")
}

#[test]
fn bijin_cbbe_golden() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        return;
    };
    let bytes = std::fs::read(&path).expect("Bijin NIF is readable");
    let document = sculpto_nif::read(&bytes).expect("Bijin NIF parses");
    let shape = document
        .shapes
        .into_iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    let mesh = MeshAsset::from(&shape);
    mesh.validate().expect("golden body validates");
    let actual = format!(
        "shape={}\nvertices={}\ntriangles={}\nbones={}\nmorphs={}\npositions_fnv1a64={}\n",
        mesh.name,
        mesh.positions.len(),
        mesh.triangles.len(),
        mesh.skin.as_ref().map_or(0, |skin| skin.bones.len()),
        mesh.morphs.len(),
        fnv1a64(&mesh.positions),
    );
    let descriptor = descriptor_path();
    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        std::fs::create_dir_all(descriptor.parent().expect("parent")).expect("fixture directory");
        std::fs::write(descriptor, &actual).expect("write descriptor");
        return;
    }
    let expected = std::fs::read_to_string(descriptor).expect("golden descriptor is committed");
    assert_eq!(actual, expected, "Bijin CBBE body drifted");
}
