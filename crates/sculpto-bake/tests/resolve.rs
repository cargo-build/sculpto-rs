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

#[test]
fn nonlocal_reference_uses_target_shape_folder() {
    let root = home("nonlocal-target-folder");
    [
        ("ShapeData/HIMBO", -20.0),
        ("Data", -10.0),
        ("ShapeData/Shared/Body", 3.0),
    ]
    .into_iter()
    .for_each(|(folder, delta)| {
        let folder = root.join(folder);
        std::fs::create_dir_all(&folder).expect("fixture folder");
        let data = container(&[("BodyLean", vec![offset(0, [delta, 0.0, 0.0])])]);
        std::fs::write(
            folder.join("body.osd"),
            data.to_osd_bytes().expect("OSD bytes"),
        )
        .expect("fixture container");
    });
    let xml = PROJECT
        .replace(
            r#"<Shape target="Body">Body</Shape>"#,
            r#"<Shape target="Body" DataFolder="Shared\Body">RenderBody</Shape>"#,
        )
        .replace(r#"local="true""#, r#"local="false""#);
    let project = match Document::parse_str(&xml, "nonlocal.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    };
    let plan = BakePlan::resolve(&project, &preset(""), "Body", SliderSize::Big, &root)
        .expect("production morph loader");
    let output = plan.apply("Body", &[[0.0; 3]]).expect("applies offsets");
    std::fs::remove_dir_all(&root).expect("remove fixture");

    assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);
    assert_eq!(output, [[3.0, 0.0, 0.0]], "authored target folder must win");
}

struct FolderFixture(PathBuf);

impl FolderFixture {
    fn new(tag: &str) -> Self {
        Self(home(tag))
    }

    fn write_osd(&self, relative: &str, blocks: &[(&str, Vec<VertexOffset>)]) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture folder");
        std::fs::write(&path, container(blocks).to_osd_bytes().expect("OSD bytes"))
            .expect("fixture OSD");
        path
    }

    fn resolve(&self, project: &Project) -> BakePlan {
        BakePlan::resolve(project, &preset(""), "Body", SliderSize::Big, &self.0)
            .expect("production morph loader")
    }
}

impl Drop for FolderFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn folder_project(folders: Option<&str>, local: bool) -> Project {
    let mut project = project();
    let set = &mut project.sets[0];
    set.shapes[0].name = "RenderBody".into();
    set.shapes[0].data_folder = folders.map(str::to_owned);
    set.sliders[0].data[0].local = local;
    project
}

#[test]
fn local_records_ignore_external_shape_folders() {
    let fixture = FolderFixture::new("local-override");
    fixture.write_osd(
        "ShapeData/HIMBO/body.osd",
        &[("BodyLean", vec![offset(0, [2.0, 0.0, 0.0])])],
    );
    fixture.write_osd(
        "ShapeData/Shared/Body/body.osd",
        &[("BodyLean", vec![offset(0, [9.0, 0.0, 0.0])])],
    );
    let plan = fixture.resolve(&folder_project(Some(r"Shared\Body"), true));
    assert!(plan.unresolved.is_empty());
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("applies"),
        [[2.0, 0.0, 0.0]]
    );
}

#[test]
fn external_candidates_select_the_first_existing_file() {
    let fixture = FolderFixture::new("ordered-folders");
    std::fs::create_dir_all(fixture.0.join("ShapeData/Directory/nested/body.osd"))
        .expect("a directory is not a container file");
    fixture.write_osd(
        "ShapeData/First/nested/body.osd",
        &[("BodyLean", vec![offset(0, [3.0, 0.0, 0.0])])],
    );
    fixture.write_osd(
        "ShapeData/Second/nested/body.osd",
        &[("BodyLean", vec![offset(0, [9.0, 0.0, 0.0])])],
    );
    let mut project = folder_project(Some("Missing;Directory;First;Second;"), false);
    project.sets[0].sliders[0].data[0].file = Some(r"nested\body.osd".into());
    let plan = fixture.resolve(&project);
    assert!(plan.unresolved.is_empty());
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("applies"),
        [[3.0, 0.0, 0.0]]
    );
}

#[test]
fn a_missing_block_does_not_select_a_later_container() {
    let fixture = FolderFixture::new("no-block-fallback");
    fixture.write_osd("ShapeData/First/body.osd", &[]);
    fixture.write_osd(
        "ShapeData/Second/body.osd",
        &[("BodyLean", vec![offset(0, [9.0, 0.0, 0.0])])],
    );
    let plan = fixture.resolve(&folder_project(Some("First;Second"), false));
    assert_eq!(plan.block_count(), 0);
    assert_eq!(plan.unresolved.len(), 1);
    assert_eq!(plan.unresolved[0].target, "Body");
    assert_eq!(
        plan.unresolved[0].reason,
        UnresolvedReason::MissingBlock("BodyLean".into())
    );
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("applies"),
        [[0.0; 3]]
    );
}

#[test]
fn external_references_without_an_effective_override_use_the_set_folder() {
    let fixture = FolderFixture::new("external-default-folder");
    fixture.write_osd(
        "ShapeData/HIMBO/body.osd",
        &[("BodyLean", vec![offset(0, [2.0, 0.0, 0.0])])],
    );
    [None, Some("")].into_iter().for_each(|folders| {
        let plan = fixture.resolve(&folder_project(folders, false));
        assert!(plan.unresolved.is_empty(), "{folders:?}");
        assert_eq!(
            plan.apply("Body", &[[0.0; 3]]).expect("applies"),
            [[2.0, 0.0, 0.0]]
        );
    });
    let mut project = folder_project(Some("Missing"), false);
    project.sets[0].shapes[0].target = "AnotherTarget".into();
    let plan = fixture.resolve(&project);
    assert!(plan.unresolved.is_empty());
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("applies"),
        [[2.0, 0.0, 0.0]]
    );
}

#[test]
fn missing_external_files_report_the_first_candidate_without_using_the_set_folder() {
    let fixture = FolderFixture::new("external-missing-file");
    fixture.write_osd(
        "ShapeData/HIMBO/body.osd",
        &[("BodyLean", vec![offset(0, [2.0, 0.0, 0.0])])],
    );
    let plan = fixture.resolve(&folder_project(
        Some(r"Missing\First;Missing\Second"),
        false,
    ));
    assert_eq!(plan.block_count(), 0);
    assert_eq!(plan.unresolved.len(), 1);
    assert_eq!(plan.unresolved[0].slider, "Lean");
    assert_eq!(
        plan.unresolved[0].reason,
        UnresolvedReason::MissingFile(fixture.0.join("ShapeData/Missing/First/body.osd"))
    );
}

#[test]
fn folder_lists_preserve_native_empty_components_and_whitespace() {
    let fixture = FolderFixture::new("native-folder-components");
    fixture.write_osd(
        "ShapeData/body.osd",
        &[("BodyLean", vec![offset(0, [3.0, 0.0, 0.0])])],
    );
    fixture.write_osd(
        "ShapeData/ Later/body.osd",
        &[("BodyLean", vec![offset(0, [7.0, 0.0, 0.0])])],
    );
    fixture.write_osd(
        "ShapeData/Later/body.osd",
        &[("BodyLean", vec![offset(0, [9.0, 0.0, 0.0])])],
    );
    [
        (";", 3.0),
        (";Later", 3.0),
        ("Missing;;Later", 3.0),
        (" Later", 7.0),
        ("Missing; Later", 7.0),
    ]
    .into_iter()
    .for_each(|(folders, expected)| {
        let plan = fixture.resolve(&folder_project(Some(folders), false));
        assert!(plan.unresolved.is_empty(), "{folders:?}");
        assert_eq!(
            plan.apply("Body", &[[0.0; 3]]).expect("applies"),
            [[expected, 0.0, 0.0]],
            "{folders:?}"
        );
    });
    let trailing = fixture.resolve(&folder_project(Some("Missing;"), false));
    assert_eq!(
        trailing.unresolved[0].reason,
        UnresolvedReason::MissingFile(fixture.0.join("ShapeData/Missing/body.osd"))
    );
}

#[test]
fn cache_identity_uses_the_selected_path_and_reuses_blocks_from_one_file() {
    let fixture = FolderFixture::new("external-cache");
    let body = fixture.write_osd(
        "ShapeData/Body/body.osd",
        &[
            ("First", vec![offset(0, [1.0, 0.0, 0.0])]),
            ("Second", vec![offset(0, [2.0, 0.0, 0.0])]),
        ],
    );
    let boots = fixture.write_osd(
        "ShapeData/Boots/body.osd",
        &[("First", vec![offset(0, [9.0, 0.0, 0.0])])],
    );
    let mut project = folder_project(Some("Body"), false);
    let set = &mut project.sets[0];
    let mut shape = set.shapes[0].clone();
    shape.name = "RenderBoots".into();
    shape.target = "Boots".into();
    shape.data_folder = Some("Boots".into());
    set.shapes.push(shape);
    let mut reference = set.sliders[0].data[0].clone();
    reference.record = Some("First".into());
    set.sliders[0].data = vec![reference.clone()];
    reference.target = "Boots".into();
    set.sliders[0].data.push(reference.clone());
    reference.target = "Body".into();
    reference.record = Some("Second".into());
    set.sliders[0].data.push(reference);
    let mut loaded = Vec::new();
    let plan = BakePlan::resolve_with(
        &project,
        &preset(""),
        "Body",
        SliderSize::Big,
        &fixture.0,
        |path| {
            loaded.push(path.to_path_buf());
            sculpto_core::shape_data::MorphData::from_file(path).map_err(|source| {
                BakeError::Container {
                    path: path.to_path_buf(),
                    source,
                }
            })
        },
    )
    .expect("production morph loader");
    assert!(plan.unresolved.is_empty());
    assert_eq!(loaded, [body, boots]);
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("body"),
        [[3.0, 0.0, 0.0]]
    );
    assert_eq!(
        plan.apply("Boots", &[[0.0; 3]]).expect("boots"),
        [[9.0, 0.0, 0.0]]
    );
}

#[test]
fn external_bare_bsd_uses_the_selected_shape_folder() {
    let fixture = FolderFixture::new("external-bsd");
    let directory = fixture.0.join("ShapeData/Shared/Body/nested");
    std::fs::create_dir_all(&directory).expect("BSD folder");
    std::fs::write(
        directory.join("body.bsd"),
        sculpto_core::shape_data::to_bsd_bytes(&[offset(0, [4.0, 0.0, 0.0])]).expect("BSD bytes"),
    )
    .expect("BSD file");
    let mut project = folder_project(Some(r"Shared\Body"), false);
    let data = &mut project.sets[0].sliders[0].data[0];
    data.file = Some(r"nested\body.bsd".into());
    data.record = None;
    let plan = fixture.resolve(&project);
    assert!(plan.unresolved.is_empty());
    assert_eq!(plan.sliders[0].shapes[0].block, "BodyLean");
    assert_eq!(
        plan.apply("Body", &[[0.0; 3]]).expect("applies"),
        [[4.0, 0.0, 0.0]]
    );
}
