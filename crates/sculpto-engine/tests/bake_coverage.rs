//! Coverage-state contract of `bake` against a synthetic authored set.
//!
//! The fixture mirrors the authored CBBE/3BA shape of the coverage story: an
//! ordinary body slider (`Lift`) plus the non-destructive coverage morph
//! (`NipBGone`) in one local `.osd`. The assertions are the product contract:
//!
//! - the implicit default is clothed — the coverage morph applies at 100 and
//!   moves exactly its own block, leaving every other vertex bit-identical;
//! - `coverage=nude` keeps the preset and other sliders untouched;
//! - an explicit clothed request on a set that authors no coverage morph is a
//!   typed refusal, never a silently visible body;
//! - a set without the morph bakes as authored by default (`unauthored`), so
//!   male and outfit sets stay bakeable;
//! - a coverage morph that never reaches the plan (deferred class) is refused
//!   rather than reported clothed.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_core::shape_data::{MorphBlock, MorphContainer, SUPPORTED_VERSION, VertexOffset};
use sculpto_engine::obj;
use sculpto_engine::{CommandError, ErrorKind, dispatch};

const COVERED_PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body Set">
    <DataFolder>Test</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="BodyData">Body</Shape>
    <Slider name="Lift" small="0" big="0">
      <Data target="BodyData" local="true">source.osd\LiftData</Data>
    </Slider>
    <Slider name="NipBGone" small="0" big="0">
      <Data target="BodyData" local="true">source.osd\CoverData</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const BARE_PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body Set">
    <DataFolder>Test</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="BodyData">Body</Shape>
    <Slider name="Lift" small="0" big="0">
      <Data target="BodyData" local="true">source.osd\LiftData</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const DEFERRED_PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body Set">
    <DataFolder>Test</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="BodyData">Body</Shape>
    <Slider name="Lift" small="0" big="0">
      <Data target="BodyData" local="true">source.osd\LiftData</Data>
    </Slider>
    <Slider name="NipBGone" small="0" big="0" uv="true">
      <Data target="BodyData" local="true">source.osd\CoverData</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const PRESETS: &str =
    r#"<SliderPresets><Preset name="Neutral" set="Body Set"></Preset></SliderPresets>"#;

/// The coverage block's authored deltas: vertices 1 and 3 move, 0 and 2 do not.
const COVER_DELTAS: [(usize, [f32; 3]); 2] = [(1, [0.3, 0.0, 0.4]), (3, [0.0, -0.2, 0.0])];

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-coverage-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir(&path).expect("unique fixture root");
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("fixture file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn quad() -> MeshAsset {
    MeshAsset {
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
    }
}

/// One home: three project files (authored, bare, deferred-class coverage)
/// sharing the morph container and source body. `Lift` is inert in the preset
/// and set at 50 by every bake here, so the two slider paths are always
/// exercised together.
fn home() -> Scratch {
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.0.join("ShapeData/Test")).expect("local data directory");
    scratch.write("project.osp", COVERED_PROJECT.as_bytes());
    scratch.write("bare.osp", BARE_PROJECT.as_bytes());
    scratch.write("deferred.osp", DEFERRED_PROJECT.as_bytes());
    scratch.write("presets.xml", PRESETS.as_bytes());
    scratch.write(
        "body.nif",
        &sculpto_nif::write::se_shape::write_se_shape(&quad()).expect("body bytes"),
    );
    let container = MorphContainer {
        version: SUPPORTED_VERSION,
        blocks: vec![
            MorphBlock {
                name: "LiftData".into(),
                offsets: (0..4)
                    .map(|index| VertexOffset {
                        index,
                        delta: [0.0, 0.0, 1.0],
                    })
                    .collect(),
            },
            MorphBlock {
                name: "CoverData".into(),
                offsets: COVER_DELTAS
                    .iter()
                    .map(|&(index, delta)| VertexOffset {
                        index: index as u16,
                        delta,
                    })
                    .collect(),
            },
        ],
    };
    scratch.write(
        "ShapeData/Test/source.osd",
        &container.to_osd_bytes().expect("source data"),
    );
    scratch
}

/// Bake the fixture at `Lift=50` plus the requested extras. `project` selects
/// one of the three `.osp` files (default: the authored one).
fn bake(
    scratch: &Scratch,
    tag: &str,
    project: Option<&str>,
    coverage: Option<&str>,
    sliders: serde_json::Value,
) -> Result<serde_json::Value, CommandError> {
    let root: &Path = &scratch.0;
    let mut sliders_map = serde_json::Map::new();
    if let Some(map) = sliders.as_object() {
        for (name, value) in map {
            sliders_map.insert(name.clone(), value.clone());
        }
    }
    sliders_map
        .entry("Lift".to_string())
        .or_insert(serde_json::json!(50));
    dispatch(
        "bake",
        &serde_json::json!({
            "project": root.join(project.unwrap_or("project.osp")),
            "presets": root.join("presets.xml"),
            "set": "Body Set", "preset": "Neutral", "endpoint": "big",
            "home": root, "source": root.join("body.nif"), "shape": "Body",
            "sliders": sliders_map,
            "coverage": coverage,
            "out": root.join(format!("{tag}.obj")),
        }),
    )
}

fn positions(scratch: &Scratch, tag: &str) -> Vec<[f32; 3]> {
    obj::read_obj_positions(&scratch.0.join(format!("{tag}.obj"))).expect("baked OBJ")
}

/// The plain preset build: `Lift` at 50 moves every vertex by z += 0.5 and the
/// coverage morph contributes nothing.
fn uncovered() -> Vec<[f32; 3]> {
    quad()
        .positions
        .iter()
        .map(|&[x, y, z]| [x, y, z + 0.5])
        .collect()
}

fn covered(scale: f32) -> Vec<[f32; 3]> {
    uncovered()
        .iter()
        .enumerate()
        .map(
            |(index, &[x, y, z])| match COVER_DELTAS.iter().find(|&&(i, _)| i == index) {
                Some(&(_, [dx, dy, dz])) => [x + scale * dx, y + scale * dy, z + scale * dz],
                None => [x, y, z],
            },
        )
        .collect()
}

#[test]
fn default_bake_is_clothed_and_moves_only_the_coverage_block() {
    let scratch = home();
    let result = bake(&scratch, "default", None, None, serde_json::json!({}))
        .expect("the implicit default is the covered body");
    assert_eq!(result["coverage"]["state"], "clothed");
    assert_eq!(result["coverage"]["slider"], "NipBGone");
    assert_eq!(result["coverage"]["percent"], 100.0);
    assert_eq!(result["sliders"], 2, "Lift plus the coverage morph");
    assert_eq!(result["deferred"], 0);
    assert_eq!(positions(&scratch, "default"), covered(1.0));
}

#[test]
fn explicit_nude_equals_the_plain_preset_build() {
    let scratch = home();
    let result = bake(&scratch, "nude", None, Some("nude"), serde_json::json!({}))
        .expect("explicit nude is always satisfiable");
    assert_eq!(result["coverage"]["state"], "nude");
    assert_eq!(result["coverage"]["percent"], serde_json::Value::Null);
    assert_eq!(result["sliders"], 1, "only Lift applies");

    // The covered/uncovered difference is confined to the authored block: the
    // default bake equals this build plus exactly the authored deltas at full
    // weight (`default_bake_is_clothed_and_moves_only_the_coverage_block`),
    // so every vertex outside the block is bit-identical between the states.
    let nude = positions(&scratch, "nude");
    assert_eq!(nude, uncovered(), "nude must equal the plain preset build");
}

#[test]
fn explicit_clothed_without_the_morph_is_a_typed_refusal() {
    let scratch = home();
    let error = bake(
        &scratch,
        "refused",
        Some("bare.osp"),
        Some("clothed"),
        serde_json::json!({}),
    )
    .expect_err("the bare set authors no coverage morph");
    assert_eq!(error.code(), "coverage_slider_absent");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
    assert!(
        !scratch.0.join("refused.obj").exists(),
        "a refused bake writes nothing"
    );
}

#[test]
fn default_without_the_morph_bakes_as_authored() {
    let scratch = home();
    let result = bake(
        &scratch,
        "unauthored",
        Some("bare.osp"),
        None,
        serde_json::json!({}),
    )
    .expect("male and outfit sets stay bakeable");
    assert_eq!(result["coverage"]["state"], "unauthored");
    assert_eq!(result["coverage"]["percent"], serde_json::Value::Null);
    assert_eq!(positions(&scratch, "unauthored"), uncovered());
}

#[test]
fn deferred_coverage_morph_is_refused_not_reported_clothed() {
    let scratch = home();
    let error = bake(
        &scratch,
        "deferred",
        Some("deferred.osp"),
        None,
        serde_json::json!({}),
    )
    .expect_err("a UV-class coverage morph never reaches positions");
    assert_eq!(error.code(), "coverage_unapplied");
    assert_eq!(error.kind(), ErrorKind::Refused);
}

#[test]
fn explicit_slider_value_wins_over_the_clothed_default() {
    let scratch = home();
    let result = bake(
        &scratch,
        "partial",
        None,
        None,
        serde_json::json!({"NipBGone": 40}),
    )
    .expect("partial coverage through the plain slider surface");
    assert_eq!(result["coverage"]["state"], "clothed");
    assert_eq!(result["coverage"]["percent"], 40.0);
    assert_eq!(positions(&scratch, "partial"), covered(0.4));
}

#[test]
fn a_clothed_bake_at_explicit_zero_is_the_callers_choice() {
    let scratch = home();
    let result = bake(
        &scratch,
        "zero",
        None,
        None,
        serde_json::json!({"NipBGone": 0}),
    )
    .expect("an explicit zero override is not a silent miss");
    assert_eq!(result["coverage"]["state"], "clothed");
    assert_eq!(result["coverage"]["percent"], 0.0);
    assert_eq!(positions(&scratch, "zero"), uncovered());
}

#[test]
fn a_bad_coverage_token_is_invalid_params() {
    let scratch = home();
    let error = bake(
        &scratch,
        "token",
        None,
        Some("pants"),
        serde_json::json!({}),
    )
    .expect_err("the token vocabulary is clothed and nude");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
}
