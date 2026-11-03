//! First end-to-end bake: project + preset + real body NIF -> moved vertices.

use std::path::PathBuf;

use sculpto_bake::BakePlan;
use sculpto_core::{Document, SliderSize};

#[test]
fn himbo_chonky_moves_the_real_body() {
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

    let nif_path = home.join("ShapeData/HIMBO/HIMBO Body 1st.nif");
    let bytes = std::fs::read(&nif_path).expect("body NIF is readable");
    let document = sculpto_nif::read(&bytes).expect("body NIF parses");
    let body = document
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    let before = body.positions.clone();
    let after = plan.apply(&body.name, &before).expect("offsets apply");

    let moved = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    let max = before
        .iter()
        .zip(&after)
        .map(|(a, b)| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        })
        .fold(0.0f32, f32::max);
    eprintln!(
        "shape {}: {} vertices, {} moved, max displacement {:.4}, target `{}`",
        body.name,
        before.len(),
        moved,
        max,
        plan.sliders
            .first()
            .map(|slider| slider.shapes[0].target.as_str())
            .unwrap_or("-")
    );
    assert!(!plan.sliders.is_empty());
    assert!(moved > 0, "the preset must move something");
    assert!(max.is_finite());
}
