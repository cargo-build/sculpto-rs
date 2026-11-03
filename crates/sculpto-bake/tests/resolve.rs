//! Resolving a project + preset into offsets, and applying them.

use std::path::{Path, PathBuf};

use sculpto_bake::{BakeError, BakePlan, DeferredReason, UnresolvedReason};
use sculpto_core::{
    Document, MorphBlock, MorphContainer, Preset, Project, SliderSize, VertexOffset,
};

const PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body">
    <DataFolder>HIMBO</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Body">Body</Shape>
    <Slider name="Lean" small="0" big="100">
      <Data target="Body" local="true">body.osd\BodyLean</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

fn project() -> Project {
    match Document::parse_str(PROJECT, "inline.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    }
}

fn preset(values: &str) -> Preset {
    let xml = format!(
        r#"<SliderPresets><Preset name="Chonky" set="Body">{values}</Preset></SliderPresets>"#
    );
    match Document::parse_str(&xml, "inline.xml") {
        Ok(Document::Presets(presets)) => presets.into_iter().next().expect("one preset"),
        other => panic!("expected presets: {other:?}"),
    }
}

fn container(blocks: &[(&str, Vec<VertexOffset>)]) -> MorphContainer {
    MorphContainer {
        version: 1,
        blocks: blocks
            .iter()
            .map(|(name, offsets)| MorphBlock {
                name: (*name).to_string(),
                offsets: offsets.clone(),
            })
            .collect(),
    }
}

fn offset(index: u16, delta: [f32; 3]) -> VertexOffset {
    VertexOffset { index, delta }
}

/// A home with a real (empty) container file so resolution reaches the loader.
fn home(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("sculpto-bake-{}-{tag}", std::process::id()));
    let dir = root.join("ShapeData/HIMBO");
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("body.osd"), b"").expect("placeholder container");
    root
}

fn resolve(preset: &Preset, home: &Path, blocks: Vec<(&str, Vec<VertexOffset>)>) -> BakePlan {
    BakePlan::resolve_with(
        &project(),
        preset,
        "Body",
        SliderSize::Big,
        home,
        move |_path| Ok(container(&blocks)),
    )
    .expect("plan resolves")
}

#[test]
fn preset_value_scales_the_offset() {
    let plan = resolve(
        &preset(r#"<SetSlider name="Lean" size="big" value="50"/>"#),
        &home("scale"),
        vec![("BodyLean", vec![offset(1, [1.0, 0.0, 0.0])])],
    );
    assert_eq!(plan.sliders.len(), 1);
    assert_eq!(plan.sliders[0].percent, 50.0);
    let out = plan
        .apply("Body", &[[0.0; 3], [0.0; 3], [0.0; 3]])
        .expect("applies");
    assert_eq!(out[1], [0.5, 0.0, 0.0]);
    assert_eq!(out[0], [0.0; 3]);
}

#[test]
fn omitted_preset_value_uses_the_endpoint_default() {
    let plan = resolve(
        &preset(""),
        &home("endpoint"),
        vec![("BodyLean", vec![offset(0, [1.0, 0.0, 0.0])])],
    );
    assert_eq!(plan.sliders[0].percent, 100.0);
    let out = plan.apply("Body", &[[0.0; 3]]).expect("applies");
    assert_eq!(out[0], [1.0, 0.0, 0.0]);
}

#[test]
fn invert_flips_the_direction() {
    let xml = PROJECT.replace(
        r#"<Slider name="Lean""#,
        r#"<Slider name="Lean" invert="true""#,
    );
    let project = match Document::parse_str(&xml, "inline.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    };
    let plan = BakePlan::resolve_with(
        &project,
        &preset(r#"<SetSlider name="Lean" size="big" value="50"/>"#),
        "Body",
        SliderSize::Big,
        &home("invert"),
        |_path| Ok(container(&[("BodyLean", vec![offset(0, [1.0, 0.0, 0.0])])])),
    )
    .expect("plan resolves");
    let out = plan.apply("Body", &[[0.0; 3]]).expect("applies");
    assert_eq!(out[0], [-0.5, 0.0, 0.0]);
}

#[test]
fn uv_and_zap_sliders_are_deferred() {
    let xml = r#"<SliderSetInfo version="1">
  <SliderSet name="Body">
    <DataFolder>HIMBO</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Body">Body</Shape>
    <Slider name="Lean" small="0" big="100">
      <Data target="Body" local="true">body.osd\BodyLean</Data>
    </Slider>
    <Slider name="Paint" small="0" big="100" uv="true"/>
    <Slider name="Cut" small="0" big="100" zap="true"/>
  </SliderSet>
</SliderSetInfo>"#;
    let project = match Document::parse_str(xml, "inline.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    };
    let plan = BakePlan::resolve_with(
        &project,
        &preset(r#"<SetSlider name="Lean" size="big" value="50"/>"#),
        "Body",
        SliderSize::Big,
        &home("deferred"),
        |_path| Ok(container(&[("BodyLean", vec![offset(0, [1.0, 0.0, 0.0])])])),
    )
    .expect("plan resolves");
    let reasons: Vec<DeferredReason> = plan.deferred.iter().map(|item| item.reason).collect();
    assert!(reasons.contains(&DeferredReason::Uv));
    assert!(reasons.contains(&DeferredReason::Zap));
    assert_eq!(plan.sliders.len(), 1, "only the position slider applies");
}

#[test]
fn unrecorded_reference_is_reported() {
    let xml = PROJECT.replace(r"body.osd\BodyLean", "");
    let project = match Document::parse_str(&xml, "inline.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    };
    let plan = BakePlan::resolve_with(
        &project,
        &preset(""),
        "Body",
        SliderSize::Big,
        &home("unrecorded"),
        |_path| Ok(container(&[])),
    )
    .expect("plan resolves");
    assert!(matches!(
        plan.unresolved[0].reason,
        UnresolvedReason::Unrecorded
    ));
}

#[test]
fn missing_file_is_reported() {
    let plan = BakePlan::resolve_with(
        &project(),
        &preset(""),
        "Body",
        SliderSize::Big,
        Path::new("/definitely/not/a/body/slide/home"),
        |_path| Ok(container(&[])),
    )
    .expect("plan resolves");
    assert!(matches!(
        plan.unresolved[0].reason,
        UnresolvedReason::MissingFile(_)
    ));
}

#[test]
fn missing_block_is_reported() {
    let plan = resolve(&preset(""), &home("missing-block"), vec![]);
    assert!(matches!(
        plan.unresolved[0].reason,
        UnresolvedReason::MissingBlock(_)
    ));
}

#[test]
fn out_of_bounds_index_is_an_error() {
    let plan = resolve(
        &preset(""),
        &home("bounds"),
        vec![("BodyLean", vec![offset(9, [1.0, 0.0, 0.0])])],
    );
    let error = plan
        .apply("Body", &[[0.0; 3]])
        .expect_err("index must fail");
    assert!(matches!(
        error,
        BakeError::IndexOutOfBounds { index: 9, .. }
    ));
}

#[test]
fn real_himbo_preset_resolves() {
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
    let chonky = presets
        .iter()
        .find(|preset| preset.name == "HIMBO Chonky")
        .expect("preset exists");
    let plan = BakePlan::resolve(&project, chonky, "HIMBO Body 1st", SliderSize::Big, &home)
        .expect("plan resolves");
    eprintln!(
        "HIMBO Chonky: {} sliders, {} blocks, {} unresolved, {} deferred",
        plan.sliders.len(),
        plan.block_count(),
        plan.unresolved.len(),
        plan.deferred.len()
    );
    assert!(!plan.sliders.is_empty());
    assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);
}

#[test]
fn override_replaces_the_preset_value() {
    let mut overrides = std::collections::BTreeMap::new();
    overrides.insert("Lean".to_string(), 25.0);
    let plan = BakePlan::resolve_with_overrides(
        &project(),
        &preset(r#"<SetSlider name="Lean" size="big" value="50"/>"#),
        "Body",
        SliderSize::Big,
        &home("override"),
        &overrides,
        |_path| Ok(container(&[("BodyLean", vec![offset(0, [1.0, 0.0, 0.0])])])),
    )
    .expect("plan resolves");
    assert_eq!(plan.sliders[0].percent, 25.0);
    let out = plan.apply("Body", &[[0.0; 3]]).expect("applies");
    assert_eq!(out[0], [0.25, 0.0, 0.0]);
}
