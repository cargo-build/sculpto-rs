//! argv surface of the coverage flag: the token, the printed state and the
//! refusal reach the command line exactly as the registry contract promises.

use std::path::{Path, PathBuf};
use std::process::Command;

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_core::shape_data::{MorphBlock, MorphContainer, SUPPORTED_VERSION, VertexOffset};

const PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Cover Body">
    <DataFolder>Cover</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Cover Body">Body</Shape>
    <Slider name="NipBGone" small="0" big="0">
      <Data target="Cover Body" local="true">cover.osd\CoverData</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const BARE_PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Bare Body">
    <DataFolder>Cover</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Bare Body">Body</Shape>
    <Slider name="Lean" small="0" big="0">
      <Data target="Bare Body" local="true">cover.osd\LeanData</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const PRESETS: &str = r#"<SliderPresets><Preset name="Neutral" set="Cover Body"></Preset><Preset name="Bare" set="Bare Body"></Preset></SliderPresets>"#;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-cli-coverage-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("ShapeData/Cover")).expect("fixture root");
        Self(path)
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_fixture(scratch: &Scratch) {
    let root = scratch.root();
    std::fs::write(root.join("project.osp"), PROJECT).expect("project");
    std::fs::write(root.join("bare.osp"), BARE_PROJECT).expect("bare project");
    std::fs::write(root.join("presets.xml"), PRESETS).expect("presets");
    let body = MeshAsset {
        name: "Body".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    };
    std::fs::write(
        root.join("ShapeData/Cover/body.nif"),
        sculpto_nif::write::se_shape::write_se_shape(&body).expect("body bytes"),
    )
    .expect("body file");
    let container = MorphContainer {
        version: SUPPORTED_VERSION,
        blocks: vec![
            MorphBlock {
                name: "CoverData".into(),
                offsets: vec![VertexOffset {
                    index: 1,
                    delta: [0.25, 0.0, -0.5],
                }],
            },
            MorphBlock {
                name: "LeanData".into(),
                offsets: vec![VertexOffset {
                    index: 2,
                    delta: [0.0, 0.1, 0.0],
                }],
            },
        ],
    };
    std::fs::write(
        root.join("ShapeData/Cover/cover.osd"),
        container.to_osd_bytes().expect("cover data"),
    )
    .expect("container");
}

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn sculpto_bake(
    scratch: &Scratch,
    project: &str,
    preset: &str,
    coverage: Option<&str>,
    out: &str,
) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command.args([
        "bake",
        "--project",
        &scratch.root().join(project).display().to_string(),
        "--presets",
        &scratch.root().join("presets.xml").display().to_string(),
        "--set",
        if project == "project.osp" {
            "Cover Body"
        } else {
            "Bare Body"
        },
        "--preset",
        preset,
        "--endpoint",
        "big",
        "--shape",
        "Body",
        "--home",
        &scratch.root().display().to_string(),
        "--out",
        &scratch.root().join(out).display().to_string(),
    ]);
    if let Some(token) = coverage {
        command.args(["--coverage", token]);
    }
    let output = command.output().expect("spawn sculpto");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

fn read(scratch: &Scratch, name: &str) -> String {
    std::fs::read_to_string(scratch.root().join(name)).expect("artifact")
}

#[test]
fn the_clothed_default_is_the_flagless_bake_and_nude_is_explicit() {
    let scratch = Scratch::new();
    write_fixture(&scratch);

    let clothed = sculpto_bake(&scratch, "project.osp", "Neutral", None, "clothed.obj");
    assert!(clothed.success, "flagless bake: {}", clothed.stderr);
    assert!(
        clothed.stdout.contains("coverage clothed (NipBGone=100)"),
        "the default reports the applied morph: {}",
        clothed.stdout
    );

    let explicit = sculpto_bake(
        &scratch,
        "project.osp",
        "Neutral",
        Some("clothed"),
        "explicit.obj",
    );
    assert!(explicit.success, "explicit clothed: {}", explicit.stderr);
    assert_eq!(
        read(&scratch, "clothed.obj"),
        read(&scratch, "explicit.obj"),
        "the default and the explicit token produce one body"
    );

    let nude = sculpto_bake(&scratch, "project.osp", "Neutral", Some("nude"), "nude.obj");
    assert!(nude.success, "explicit nude: {}", nude.stderr);
    assert!(
        nude.stdout.contains("coverage nude"),
        "nude reports its state: {}",
        nude.stdout
    );
    assert_ne!(
        read(&scratch, "nude.obj"),
        read(&scratch, "clothed.obj"),
        "nude keeps the authored anatomy; the states are different bodies"
    );
}

#[test]
fn a_set_without_the_morph_refuses_clothed_and_bakes_as_authored_by_default() {
    let scratch = Scratch::new();
    write_fixture(&scratch);

    let refused = sculpto_bake(&scratch, "bare.osp", "Bare", Some("clothed"), "refused.obj");
    assert!(!refused.success, "a bare set cannot honour clothed");
    assert!(
        refused.stderr.contains("coverage_slider_absent"),
        "the typed refusal names itself: {}",
        refused.stderr
    );
    assert!(
        !scratch.root().join("refused.obj").exists(),
        "a refused bake writes nothing"
    );

    let flagless = sculpto_bake(&scratch, "bare.osp", "Bare", None, "unauthored.obj");
    assert!(flagless.success, "flagless bare bake: {}", flagless.stderr);
    assert!(
        flagless.stdout.contains("coverage unauthored"),
        "no morph and no request bakes as authored: {}",
        flagless.stdout
    );
}

#[test]
fn an_unknown_coverage_token_is_rejected_up_front() {
    let scratch = Scratch::new();
    write_fixture(&scratch);
    let bad = sculpto_bake(&scratch, "project.osp", "Neutral", Some("pants"), "bad.obj");
    assert!(!bad.success);
    assert!(
        bad.stderr.contains("`--coverage` expects clothed or nude"),
        "the argv surface keeps the vocabulary closed: {}",
        bad.stderr
    );
}
