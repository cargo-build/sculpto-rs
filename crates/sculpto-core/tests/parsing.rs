//! Document-level parsing behaviour against checked-in fixtures.

use std::path::PathBuf;

use sculpto_core::{Document, ErrorKind, SliderSize};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn preset_document_keeps_author_order_and_endpoints() {
    let document = Document::parse_file(fixture("presets.xml")).expect("fixture parses");
    let Document::Presets(presets) = document else {
        panic!("expected a presets document");
    };
    assert_eq!(presets.len(), 2);
    let male = &presets[0];
    assert_eq!(male.name, "Reference Male");
    assert_eq!(male.slider_set.as_deref(), Some("Reference HIMBO"));
    assert_eq!(male.groups, ["Reference"]);
    assert_eq!(male.values.len(), 3);
    assert_eq!(male.values[0].slider, "ArmsBiceps");
    assert_eq!(male.values[0].size, SliderSize::Big);
    assert!((male.values[0].fraction() - 1.0).abs() < f32::EPSILON);
    assert_eq!(male.values[1].size, SliderSize::Small);
    assert!((male.values[1].fraction() - 0.55).abs() < 1e-6);
    assert_eq!(presets[1].slider_set, None);
    assert_eq!(presets[1].values[0].percent, 0.0);
}

#[test]
fn project_document_reads_set_metadata_shape_defaults_and_data_refs() {
    let document = Document::parse_file(fixture("project.osp")).expect("fixture parses");
    let Document::Project(project) = document else {
        panic!("expected a project document");
    };
    assert_eq!(project.version, 1);
    let set = &project.sets[0];
    assert_eq!(set.name, "Reference Body");
    assert_eq!(set.data_folder, "RefBody");
    assert_eq!(set.source_file, "RefBody.nif");
    assert_eq!(set.output_path, r"meshes\actors\character\character assets");
    assert_eq!(set.output_file.name, "refbody");
    assert!(set.output_file.gen_weights);
    assert!(!set.output_file.prevent_morph_file);
    assert!(!set.output_file.keep_zapped_shapes);

    let shape = &set.shapes[0];
    assert_eq!(shape.name, "RefBody");
    assert_eq!(shape.target, "RefBody");
    assert_eq!(shape.data_folder.as_deref(), Some("RefBody"));
    assert!(shape.smooth_seam_normals);
    assert!((shape.smooth_seam_normals_angle - 60.0).abs() < f32::EPSILON);
    assert!(!shape.lock_normals);

    let weight = &set.sliders[0];
    assert_eq!(weight.name, "Weight 0 to 1");
    assert!(weight.hidden);
    assert!(!weight.zap);
    assert!(!weight.uv);
    assert_eq!(weight.big, 100.0);
    let record = &weight.data[0];
    assert_eq!(record.file.as_deref(), Some("RefBody.osd"));
    assert_eq!(record.record.as_deref(), Some("RefBodyWeight"));
    assert!(record.local);

    let trim = &set.sliders[2];
    assert!(trim.zap);
    assert_eq!(trim.data.len(), 3);
    assert_eq!(trim.data[0].file.as_deref(), Some("RefBodyTrim.bsd"));
    assert_eq!(
        trim.data[0].record, None,
        "single-record .bsd has no inner name"
    );
    assert_eq!(trim.data[1].target, "RefBodyAlt");
    assert_eq!(trim.data[2].file, None, "unrecorded reference is preserved");
}

#[test]
fn unknown_size_token_is_rejected() {
    let error = Document::parse_str(
        r#"<SliderPresets><Preset name="P"><SetSlider name="A" size="medium" value="1"/></Preset></SliderPresets>"#,
        "inline.xml",
    )
    .expect_err("unknown endpoint must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("medium"));
}

#[test]
fn out_of_range_percentage_is_rejected() {
    let error = Document::parse_str(
        r#"<SliderPresets><Preset name="P"><SetSlider name="A" size="big" value="101"/></Preset></SliderPresets>"#,
        "inline.xml",
    )
    .expect_err("percent above 100 must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}

#[test]
fn duplicate_preset_name_is_rejected() {
    let error = Document::parse_str(
        r#"<SliderPresets><Preset name="P"/><Preset name="P"/></SliderPresets>"#,
        "inline.xml",
    )
    .expect_err("duplicate preset must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}

#[test]
fn duplicate_slider_in_one_set_is_rejected() {
    let error = Document::parse_str(
        r#"<SliderSetInfo version="1"><SliderSet name="S"><DataFolder>D</DataFolder><SourceFile>a.nif</SourceFile><OutputPath>p</OutputPath><OutputFile>o</OutputFile><Slider name="A"/><Slider name="A"/></SliderSet></SliderSetInfo>"#,
        "inline.osp",
    )
    .expect_err("duplicate slider must fail");
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
}

#[test]
fn unsupported_version_is_rejected() {
    let error = Document::parse_str(
        r#"<SliderSetInfo version="2"><SliderSet name="S"/></SliderSetInfo>"#,
        "inline.osp",
    )
    .expect_err("version 2 must fail");
    assert_eq!(error.kind(), ErrorKind::UnsupportedVersion);
}

#[test]
fn missing_required_element_names_the_set() {
    let error = Document::parse_str(
        r#"<SliderSetInfo version="1"><SliderSet name="NoSource"><DataFolder>D</DataFolder><OutputPath>p</OutputPath><OutputFile>o</OutputFile></SliderSet></SliderSetInfo>"#,
        "inline.osp",
    )
    .expect_err("missing SourceFile must fail");
    assert_eq!(error.kind(), ErrorKind::MissingRequired);
    let message = error.to_string();
    assert!(message.contains("SourceFile"), "{message}");
    assert!(message.contains("NoSource"), "{message}");
}

#[test]
fn missing_file_is_reported_as_io() {
    let error =
        Document::parse_file("/definitely/not/a/document.osp").expect_err("missing file must fail");
    assert_eq!(error.kind(), ErrorKind::Io);
}

#[test]
fn unrelated_root_is_rejected() {
    let error = Document::parse_str("<Config/>", "inline.xml").expect_err("root must be known");
    assert_eq!(error.kind(), ErrorKind::UnsupportedRoot);
}

#[test]
fn unknown_children_are_reported_not_fatal() {
    let document = Document::parse_str(
        r#"<SliderSetInfo version="1"><BuildStamp>2031-01-01</BuildStamp><SliderSet name="S"><DataFolder>D</DataFolder><SourceFile>a.nif</SourceFile><OutputPath>p</OutputPath><OutputFile>o</OutputFile><Slider name="A"><Data target="Body">x.osd</Data><Falloff>1</Falloff></Slider><Unused/></SliderSet></SliderSetInfo>"#,
        "inline.osp",
    )
    .expect("unknown children must not fail the document");
    let Document::Project(project) = document else {
        panic!("expected a project")
    };
    let paths: Vec<&str> = project
        .unknown
        .iter()
        .map(|item| item.path.as_str())
        .collect();
    assert_eq!(paths, ["SliderSetInfo/BuildStamp"]);
    let set = &project.sets[0];
    assert_eq!(set.unknown.len(), 1, "set-level child is reported");
    assert_eq!(
        set.sliders[0].unknown.len(),
        1,
        "slider-level child is reported"
    );
    assert_eq!(set.sliders[0].data.len(), 1, "known children still parse");
}

#[test]
fn unknown_preset_child_is_reported_not_fatal() {
    let document = Document::parse_str(
        r#"<SliderPresets><Preset name="P"><Group name="G"/><LockedBy>Editor</LockedBy><SetSlider name="A" size="big" value="50"/></Preset></SliderPresets>"#,
        "inline.xml",
    )
    .expect("unknown preset child must not fail the document");
    let Document::Presets(presets) = document else {
        panic!("expected presets")
    };
    assert_eq!(presets[0].values.len(), 1);
    assert_eq!(
        presets[0].unknown[0].path,
        "SliderPresets/Preset[name=P]/LockedBy"
    );
}

#[test]
fn negative_percentage_is_accepted() {
    let document = Document::parse_str(
        r#"<SliderPresets><Preset name="P"><SetSlider name="A" size="big" value="-25"/></Preset></SliderPresets>"#,
        "inline.xml",
    )
    .expect("a negative preset value is valid");
    let Document::Presets(presets) = document else {
        panic!("expected presets");
    };
    assert_eq!(presets[0].values[0].percent, -25.0);
    assert_eq!(presets[0].values[0].fraction(), -0.25);
}
