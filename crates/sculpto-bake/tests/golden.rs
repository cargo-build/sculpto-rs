//! Golden fixture for the accepted bake.
//!
//! The descriptor is committed; the body NIF stays a private host fixture.
//! Without `SCULPTO_BODYSLIDE_HOME` the test skips, so `just gate` is portable.
//! `GOLDEN_UPDATE=1` rewrites the descriptor when the accepted fixture changes
//! on purpose.

use std::path::PathBuf;

use sculpto_bake::BakePlan;
use sculpto_core::mesh::MeshAsset;
use sculpto_core::{Document, SliderSize};

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/himbo-chonky-golden.txt")
}

#[test]
fn himbo_chonky_golden() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME") else {
        return;
    };
    let home = PathBuf::from(home);
    let project = match Document::parse_file(home.join("SliderSets/HIMBO.osp")) {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    };
    let presets = match Document::parse_file(home.join("SliderPresets/HIMBO.xml")) {
        Ok(Document::Presets(presets)) => presets,
        other => panic!("expected presets: {other:?}"),
    };
    let preset = presets
        .iter()
        .find(|preset| preset.name == "HIMBO Chonky")
        .expect("preset exists");
    let plan = BakePlan::resolve(&project, preset, "HIMBO Body 1st", SliderSize::Big, &home)
        .expect("plan resolves");
    assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);

    let bytes = std::fs::read(home.join("ShapeData/HIMBO/HIMBO Body 1st.nif"))
        .expect("body NIF is readable");
    let document = sculpto_nif::read(&bytes).expect("body NIF parses");
    let mut baked = document
        .shapes
        .into_iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    baked.positions = plan
        .apply(&baked.name, &baked.positions)
        .expect("offsets apply");

    let mesh = MeshAsset::from(&baked);
    mesh.validate().expect("golden bake validates");
    let actual = format!(
        "preset=HIMBO Chonky\nset=HIMBO Body 1st\nshape={}\nvertices={}\ntriangles={}\nbones={}\nmorphs={}\npositions_fnv1a64={}\n",
        mesh.name,
        mesh.positions.len(),
        mesh.triangles.len(),
        mesh.skin.as_ref().map_or(0, |skin| skin.bones.len()),
        mesh.morphs.len(),
        fnv1a64(&mesh.positions),
    );

    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        let path = descriptor_path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
        std::fs::write(path, &actual).expect("write descriptor");
        return;
    }
    let expected =
        std::fs::read_to_string(descriptor_path()).expect("golden descriptor is committed");
    assert_eq!(
        actual, expected,
        "accepted bake drifted from the golden fixture"
    );
}
