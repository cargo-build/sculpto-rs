//! `.osp` serialization: attribute coverage, round trips and named refusals.

use sculpto_core::project::Project;
use sculpto_core::{Document, ErrorKind};

/// Every attribute the reader models, written in one document.
const RICH: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<SliderSetInfo version="1">
    <SliderSet name="Body &amp; Soul">
        <ReferenceInfo ProjectFile="CBBE.osp" ProjectName="CBBE &quot;Body&quot;" ShapeName="CBBE"/>
        <DataFolder>HIMBO\Body</DataFolder>
        <SourceFile>body.nif</SourceFile>
        <OutputPath>meshes\actors\character\character assets</OutputPath>
        <OutputFile GenWeights="false" PreventMorphFile="true" KeepZappedShapes="true">malebody</OutputFile>
        <SFMorphPath>morphs\male</SFMorphPath>
        <SFMorphTargetShape>MaleBody</SFMorphTargetShape>
        <Shape target="Body" DataFolder="HIMBO;Other" SmoothSeamNormals="false" SmoothSeamNormalsAngle="45" LockNormals="true">Body</Shape>
        <Slider name="Lean &amp; Trim" invert="true" small="0" big="100" default="50" hidden="true" clamp="true" zap="true" zaptoggles="A;B;" uv="true">
            <Data name="Lean &amp; Trim Lean" target="Body" local="true">Body\Lean.osd\Lean &amp; Trim</Data>
        </Slider>
        <Notes>Authored by &amp; for Ünïcode — "quotes" &lt;angle&gt;</Notes>
    </SliderSet>
</SliderSetInfo>"#;

fn parse(text: &str) -> Project {
    match Document::parse_str(text, "inline.osp") {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project: {other:?}"),
    }
}

fn fixture() -> Project {
    parse(include_str!("fixtures/project.osp"))
}

fn reparse(project: &Project) -> Project {
    let xml = project.to_osp_xml().expect("writes");
    let reparsed = parse(&xml);
    assert_eq!(reparsed, *project, "parse/write/parse must be stable");
    reparsed
}

fn refusal(project: &Project) -> sculpto_core::Error {
    project
        .to_osp_xml()
        .expect_err("the writer must refuse this model")
}

#[test]
fn fixture_project_round_trips() {
    let project = fixture();
    let xml = project.to_osp_xml().expect("writes");
    assert!(
        xml.starts_with(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SliderSetInfo version=\"1\">\n"
        ),
        "{xml}"
    );
    assert!(xml.ends_with("</SliderSetInfo>\n"), "{xml}");
    assert!(xml.contains("<SliderSet name=\"Reference Body\">"), "{xml}");
    // A boolean that already equals the reader default is not written.
    assert!(!xml.contains("invert="), "{xml}");
    assert!(xml.contains(">RefBodyTrim.bsd</Data>"), "{xml}");
    assert!(
        xml.contains("<Data name=\"RefBodyTrimNote\" target=\"RefBodyAlt\"/>"),
        "{xml}"
    );
    reparse(&project);
}

#[test]
fn every_modeled_attribute_round_trips() {
    let project = parse(RICH);
    let set = &project.sets[0];
    assert_eq!(
        set.notes.as_deref(),
        Some("Authored by & for Ünïcode — \"quotes\" <angle>")
    );
    assert_eq!(set.sf_morph_path.as_deref(), Some("morphs\\male"));
    assert_eq!(set.sf_morph_target_shape.as_deref(), Some("MaleBody"));
    let reference = set.reference.as_ref().expect("reference info");
    assert_eq!(reference.project_name.as_deref(), Some("CBBE \"Body\""));
    let slider = &set.sliders[0];
    assert_eq!(slider.zap_toggles, ["A", "B"]);
    assert_eq!(slider.default, Some(50.0));
    assert_eq!(slider.data[0].file.as_deref(), Some("Body\\Lean.osd"));
    assert_eq!(slider.data[0].record.as_deref(), Some("Lean & Trim"));

    let xml = project.to_osp_xml().expect("writes");
    for expected in [
        "<SliderSet name=\"Body &amp; Soul\">",
        "<ReferenceInfo ProjectFile=\"CBBE.osp\" ProjectName=\"CBBE &quot;Body&quot;\" ShapeName=\"CBBE\"/>",
        "<OutputFile GenWeights=\"false\" PreventMorphFile=\"true\" KeepZappedShapes=\"true\">malebody</OutputFile>",
        "<SFMorphPath>morphs\\male</SFMorphPath>",
        "SmoothSeamNormals=\"false\" SmoothSeamNormalsAngle=\"45\" LockNormals=\"true\"",
        "name=\"Lean &amp; Trim\" invert=\"true\" small=\"0\" big=\"100\" default=\"50\"",
        "hidden=\"true\" clamp=\"true\" zap=\"true\" zaptoggles=\"A;B;\" uv=\"true\"",
        ">Body\\Lean.osd\\Lean &amp; Trim</Data>",
        "<Notes>Authored by &amp; for Ünïcode — \"quotes\" &lt;angle&gt;</Notes>",
    ] {
        assert!(xml.contains(expected), "{expected} missing from\n{xml}");
    }
    reparse(&project);
}

#[test]
fn a_reference_without_a_container_survives_and_a_lone_record_is_refused() {
    let project = fixture();
    let trim = project.sets[0]
        .sliders
        .iter()
        .find(|slider| slider.name == "Trim")
        .expect("Trim slider");
    assert_eq!(trim.data[0].file.as_deref(), Some("RefBodyTrim.bsd"));
    assert_eq!(
        trim.data[0].record, None,
        "a bare `.bsd` reference has no record"
    );
    assert_eq!(
        trim.data[2].file, None,
        "a reference with no container stays unrecorded"
    );
    reparse(&project);

    let mut lone_record = project.clone();
    let data = &mut lone_record.sets[0]
        .sliders
        .iter_mut()
        .find(|slider| slider.name == "Trim")
        .expect("Trim slider")
        .data[2];
    data.record = Some("RefBodyTrimNote".to_string());
    let error = refusal(&lone_record);
    assert_eq!(error.kind(), ErrorKind::MissingRequired);
    assert!(error.to_string().contains("file\\record"), "{error}");
}

#[test]
fn unknown_metadata_is_refused_by_name() {
    let cases = [
        (
            "project level",
            r#"<SliderSetInfo version="1"><BuildStamp>2031-01-01</BuildStamp><SliderSet name="S"><DataFolder>D</DataFolder><SourceFile>a.nif</SourceFile><OutputPath>p</OutputPath><OutputFile>o</OutputFile></SliderSet></SliderSetInfo>"#,
            "SliderSetInfo/BuildStamp",
        ),
        (
            "set level",
            r#"<SliderSetInfo version="1"><SliderSet name="S"><DataFolder>D</DataFolder><SourceFile>a.nif</SourceFile><OutputPath>p</OutputPath><OutputFile>o</OutputFile><NormalsGeneration><Layer name="X"/></NormalsGeneration></SliderSet></SliderSetInfo>"#,
            "SliderSetInfo/SliderSet[name=S]/NormalsGeneration",
        ),
        (
            "slider level",
            r#"<SliderSetInfo version="1"><SliderSet name="S"><DataFolder>D</DataFolder><SourceFile>a.nif</SourceFile><OutputPath>p</OutputPath><OutputFile>o</OutputFile><Slider name="A"><Data target="Body">x.osd</Data><Falloff>1</Falloff></Slider></SliderSet></SliderSetInfo>"#,
            "SliderSetInfo/SliderSet[name=S]/Slider[name=A]/Falloff",
        ),
    ];
    for (what, xml, path) in cases {
        let project = parse(xml);
        let error = refusal(&project);
        assert_eq!(error.kind(), ErrorKind::InvalidValue, "{what}");
        assert!(
            error.to_string().contains(path),
            "{what}: expected `{path}` in {error}"
        );
    }
}

#[test]
fn duplicate_and_empty_declarations_are_refused() {
    let project = fixture();

    let mut duplicate_set = project.clone();
    let set = duplicate_set.sets[0].clone();
    duplicate_set.sets.push(set);
    assert_eq!(refusal(&duplicate_set).kind(), ErrorKind::InvalidValue);

    let mut duplicate_shape = project.clone();
    let shape = duplicate_shape.sets[0].shapes[0].clone();
    duplicate_shape.sets[0].shapes.push(shape);
    assert_eq!(refusal(&duplicate_shape).kind(), ErrorKind::InvalidValue);

    let mut duplicate_slider = project.clone();
    let slider = duplicate_slider.sets[0].sliders[0].clone();
    duplicate_slider.sets[0].sliders.push(slider);
    assert_eq!(refusal(&duplicate_slider).kind(), ErrorKind::InvalidValue);

    let mut duplicate_record = project.clone();
    let record = duplicate_record.sets[0].sliders[0].data[0].clone();
    duplicate_record.sets[0].sliders[0].data.push(record);
    assert_eq!(refusal(&duplicate_record).kind(), ErrorKind::InvalidValue);

    let mut empty_output = project.clone();
    empty_output.sets[0].output_file.name = String::new();
    let error = refusal(&empty_output);
    assert_eq!(error.kind(), ErrorKind::MissingRequired);
    assert!(error.to_string().contains("OutputFile"), "{error}");
}

#[test]
fn values_the_reader_cannot_restore_are_refused() {
    let project = fixture();

    let mut bad_version = project.clone();
    bad_version.version = 2;
    assert_eq!(refusal(&bad_version).kind(), ErrorKind::UnsupportedVersion);

    let mut non_finite = project.clone();
    non_finite.sets[0].sliders[0].small = f32::NAN;
    let error = refusal(&non_finite);
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("small"), "{error}");

    let mut padded = project.clone();
    padded.sets[0].source_file = " body.nif ".to_string();
    let error = refusal(&padded);
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("trim"), "{error}");

    let mut illegal = project.clone();
    illegal.sets[0].name = "Body\u{1}".to_string();
    let error = refusal(&illegal);
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("U+0001"), "{error}");

    let mut separator_in_record = project.clone();
    separator_in_record.sets[0].sliders[0].data[0].record = Some("a\\b".to_string());
    let error = refusal(&separator_in_record);
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("path separator"), "{error}");

    let mut empty_record = project.clone();
    empty_record.sets[0].sliders[0].data[0].record = Some(String::new());
    assert_eq!(refusal(&empty_record).kind(), ErrorKind::MissingRequired);

    let mut toggles_without_zap = project.clone();
    toggles_without_zap.sets[0].sliders[0].zap_toggles = vec!["A".to_string()];
    let error = refusal(&toggles_without_zap);
    assert_eq!(error.kind(), ErrorKind::InvalidValue);
    assert!(error.to_string().contains("non-zap"), "{error}");

    let mut unexpressible_toggle = project.clone();
    unexpressible_toggle.sets[0].sliders[0].zap = true;
    unexpressible_toggle.sets[0].sliders[0].zap_toggles = vec!["A;B".to_string()];
    assert_eq!(
        refusal(&unexpressible_toggle).kind(),
        ErrorKind::InvalidValue
    );
}

#[test]
fn file_only_references_refuse_ambiguous_separators() {
    for file in ["ShapeData/HIMBO", "ShapeData\\HIMBO", "foo\\", "\\x"] {
        let mut project = fixture();
        project.sets[0].sliders[0].data[0].file = Some(file.to_string());
        project.sets[0].sliders[0].data[0].record = None;
        assert_eq!(refusal(&project).kind(), ErrorKind::InvalidValue, "{file}");
    }
}

#[test]
fn carriage_returns_in_element_text_round_trip() {
    let mut project = fixture();
    project.sets[0].source_file = "body\rsource.nif".to_string();
    project.sets[0].notes = Some("line1\r\nline2\rline3".to_string());
    reparse(&project);
}

#[test]
fn an_empty_project_round_trips() {
    let project = Project {
        version: 1,
        sets: Vec::new(),
        unknown: Vec::new(),
    };
    let xml = project.to_osp_xml().expect("writes");
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<SliderSetInfo version=\"1\">\n</SliderSetInfo>\n"
    );
    assert_eq!(parse(&xml), project);
}
