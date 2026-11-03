//! Parity against a native BodySlide build of the same preset.
//!
//! Opt-in: `SCULPTO_BODYSLIDE_HOME` points at the BodySlide home, and
//! `SCULPTO_NATIVE_NIF` / `SCULPTO_NATIVE_NIF_SMALL` at the `_1` / `_0` files
//! the native build wrote. The stated tolerance is `1e-6` units per vertex.
//!
//! The negative control uses the same harness against the wrong endpoint: if it
//! did not fail, a passing comparison would prove nothing.

use std::path::PathBuf;

use sculpto_bake::BakePlan;
use sculpto_core::{Document, SliderSize};

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Return the maximum per-vertex deviation, or `None` when the opt-in inputs
/// are absent.
fn max_deviation(size: SliderSize, native_variable: &str) -> Option<Deviation> {
    let home = std::env::var_os("SCULPTO_BODYSLIDE_HOME")?;
    let native = std::env::var_os(native_variable)?;
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
    let plan =
        BakePlan::resolve(&project, preset, "HIMBO Body 1st", size, &home).expect("plan resolves");
    assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);

    let source_bytes = std::fs::read(home.join("ShapeData/HIMBO/HIMBO Body 1st.nif"))
        .expect("source body is readable");
    let source = sculpto_nif::read(&source_bytes).expect("source body parses");
    let source_shape = source
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("source shape");
    let ours = plan
        .apply(&source_shape.name, &source_shape.positions)
        .expect("offsets apply");

    let native_bytes = std::fs::read(&native).expect("native body is readable");
    let native_doc = sculpto_nif::read(&native_bytes).expect("native body parses");
    let native_shape = native_doc
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("native shape");
    assert_eq!(
        native_shape.positions.len(),
        ours.len(),
        "vertex counts differ ({} vs {})",
        native_shape.positions.len(),
        ours.len()
    );

    if std::env::var_os("SCULPTO_PARITY_DUMP").is_some() {
        for (index, (source, (ours, native))) in source_shape
            .positions
            .iter()
            .zip(ours.iter().zip(&native_shape.positions))
            .take(3)
            .enumerate()
        {
            eprintln!("  v{index} source {source:?} ours {ours:?} native {native:?}");
        }
    }
    let mut distances: Vec<f32> = ours
        .iter()
        .zip(&native_shape.positions)
        .map(|(ours, native)| distance(*ours, *native))
        .collect();
    distances.sort_by(|a, b| a.partial_cmp(b).expect("finite distances"));
    let max = *distances.last().expect("vertices");
    let p99 = distances[(distances.len() as f64 * 0.99) as usize];
    let sliders = plan.sliders.len();
    Some(Deviation {
        vertices: distances.len(),
        sliders,
        max,
        p99,
        uvs_equal: source_shape.uvs == native_shape.uvs,
        bones_equal: source_shape.bones == native_shape.bones,
        weight_mismatches: source_shape
            .weights
            .iter()
            .zip(&native_shape.weights)
            .filter(|(a, b)| a != b)
            .count(),
    })
}

struct Deviation {
    vertices: usize,
    sliders: usize,
    max: f32,
    p99: f32,
    uvs_equal: bool,
    bones_equal: bool,
    weight_mismatches: usize,
}

fn report(label: &str, deviation: &Deviation) {
    eprintln!(
        "{label}: {} vertices, {} sliders, max {:.9} u, p99 {:.9} u, \
         uvs_equal {}, bones_equal {}, weight_mismatches {}",
        deviation.vertices,
        deviation.sliders,
        deviation.max,
        deviation.p99,
        deviation.uvs_equal,
        deviation.bones_equal,
        deviation.weight_mismatches
    );
}

#[test]
fn himbo_chonky_big_endpoint() {
    let Some(deviation) = max_deviation(SliderSize::Big, "SCULPTO_NATIVE_NIF") else {
        return;
    };
    report("Big endpoint", &deviation);
    assert!(
        deviation.max <= 1e-6,
        "max deviation {} exceeds the stated tolerance",
        deviation.max
    );
    assert!(deviation.uvs_equal, "UV arrays differ");
    assert!(deviation.weight_mismatches == 0, "weights differ");
}

#[test]
fn himbo_chonky_small_endpoint() {
    let Some(deviation) = max_deviation(SliderSize::Small, "SCULPTO_NATIVE_NIF_SMALL") else {
        return;
    };
    report("Small endpoint", &deviation);
    assert!(
        deviation.max <= 1e-6,
        "max deviation {} exceeds the stated tolerance",
        deviation.max
    );
    assert!(deviation.uvs_equal, "UV arrays differ");
    assert!(deviation.weight_mismatches == 0, "weights differ");
}

#[test]
fn negative_control_rejects_the_wrong_endpoint() {
    let Some(deviation) = max_deviation(SliderSize::Big, "SCULPTO_NATIVE_NIF_SMALL") else {
        return;
    };
    report("Negative control (big plan vs small output)", &deviation);
    assert!(
        deviation.max > 0.1,
        "the harness accepted the wrong endpoint, so a passing comparison proves nothing"
    );
}
