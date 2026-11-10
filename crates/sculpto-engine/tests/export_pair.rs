//! C12 acceptance: `export.pair` writes the `_0`/`_1` weight pair.
//!
//! Every fixture is synthetic: the input NIF is written with this crate's own
//! writers, the project and presets are inline XML on disk, and the morph
//! container is a hand-packed `\0DSO` block. No private fixture is consulted.
//!
//! "Differ only where the endpoint changes positions" is asserted twice: at the
//! model level (a reader pass sees identical shapes except positions, which
//! equal the endpoint plans exactly) and at the byte level (all differing
//! offsets fall inside one window no larger than the vertex stream).

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::header::NifTarget;
use sculpto_nif::write::write_shape;

/// Eight-vertex cube; vertices 0 and 1 are the ones the slider moves.
const BASE: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 1.0, 1.0],
];

const TRIANGLES: [[u32; 3]; 12] = [
    [0, 2, 1],
    [0, 3, 2],
    [4, 5, 6],
    [4, 6, 7],
    [0, 1, 5],
    [0, 5, 4],
    [3, 7, 6],
    [3, 6, 2],
    [0, 4, 7],
    [0, 7, 3],
    [1, 2, 6],
    [1, 6, 5],
];

/// The `Lean` slider record: sparse deltas on the two front vertices.
const DELTAS: [(u16, [f32; 3]); 2] = [(0, [2.0, 0.0, 0.0]), (1, [0.0, 2.0, 0.0])];

fn asset() -> MeshAsset {
    MeshAsset {
        name: "Body".to_string(),
        positions: BASE.to_vec(),
        triangles: TRIANGLES.to_vec(),
        triangle_partitions: vec![PartitionId(0); TRIANGLES.len()],
        ..MeshAsset::default()
    }
}

/// Hand-packed `\0DSO`: magic, version, block count, then
/// `u8` name length, name, `u16` record count, `u16` index + three `f32`s.
fn container_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x00DSO");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.push("BodyLean".len() as u8);
    bytes.extend_from_slice(b"BodyLean");
    bytes.extend_from_slice(&(DELTAS.len() as u16).to_le_bytes());
    for (index, delta) in DELTAS {
        bytes.extend_from_slice(&index.to_le_bytes());
        for value in delta {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

const PROJECT: &str = r#"<SliderSetInfo version="1">
  <SliderSet name="Body">
    <DataFolder>Test</DataFolder>
    <SourceFile>body.nif</SourceFile>
    <OutputPath>meshes</OutputPath>
    <OutputFile>body</OutputFile>
    <Shape target="Body">Body</Shape>
    <Slider name="Lean" small="0" big="100">
      <Data target="Body" local="true">body.osd\BodyLean</Data>
    </Slider>
  </SliderSet>
</SliderSetInfo>"#;

const PRESETS: &str = r#"<SliderPresets>
  <Preset name="Pair" set="Body">
    <SetSlider name="Lean" size="small" value="25"/>
  </Preset>
</SliderPresets>"#;

/// A BodySlide home with project, presets, container and the input NIF.
fn home(tag: &str, target: NifTarget) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("sculpto-c12-{tag}-{}", std::process::id()));
    let shape_data = root.join("ShapeData/Test");
    for dir in [
        shape_data.clone(),
        root.join("SliderSets"),
        root.join("SliderPresets"),
    ] {
        std::fs::create_dir_all(dir).expect("temp home");
    }
    std::fs::write(root.join("SliderSets/body.osp"), PROJECT).expect("project");
    std::fs::write(root.join("SliderPresets/presets.xml"), PRESETS).expect("presets");
    std::fs::write(shape_data.join("body.osd"), container_bytes()).expect("container");
    let nif = shape_data.join("body.nif");
    std::fs::write(&nif, write_shape(&asset(), target).expect("input writes")).expect("input nif");
    (root, nif)
}

fn export(root: &Path, nif: &Path, out: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut input = serde_json::json!({
        "nif": nif,
        "project": root.join("SliderSets/body.osp"),
        "presets": root.join("SliderPresets/presets.xml"),
        "preset": "Pair",
        "set": "Body",
        "out": root.join(out),
    });
    for (key, value) in extra.as_object().expect("object") {
        input[key.clone()] = value.clone();
    }
    sculpto_engine::envelope(sculpto_engine::dispatch("export.pair", &input))
}

/// `BASE` plus the `Lean` deltas at `percent` of the record.
fn expected(percent: f32) -> Vec<[f32; 3]> {
    let mut positions = BASE.to_vec();
    let scale = percent / 100.0;
    for (index, delta) in DELTAS {
        for (slot, value) in positions[index as usize].iter_mut().zip(delta) {
            *slot += scale * value;
        }
    }
    positions
}

fn differing_offsets(small: &[u8], big: &[u8]) -> Vec<usize> {
    assert_eq!(small.len(), big.len(), "pair files must share a length");
    small
        .iter()
        .zip(big)
        .enumerate()
        .filter_map(|(index, (a, b))| (a != b).then_some(index))
        .collect()
}

fn shapes_of(path: &Path) -> sculpto_nif::NifShape {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let document = sculpto_nif::read(&bytes).expect("written NIF reads");
    assert_eq!(document.shapes.len(), 1);
    document.shapes.into_iter().next().expect("one shape")
}

/// The pair differs only where the endpoint moved vertices: every other model
/// field matches exactly, and all byte diffs sit inside the vertex stream
/// (at most `positions * 16` bytes apart on either target).
fn assert_pair_differs_only_in_positions(small_path: &Path, big_path: &Path) {
    let small = std::fs::read(small_path).expect("_0 reads");
    let big = std::fs::read(big_path).expect("_1 reads");
    let diffs = differing_offsets(&small, &big);
    assert!(!diffs.is_empty(), "the endpoints produced identical bytes");
    let span = diffs[diffs.len() - 1] - diffs[0] + 1;
    assert!(
        span <= BASE.len() * 16,
        "diffs span {span} bytes, beyond the vertex stream"
    );

    let small_shape = shapes_of(small_path);
    let big_shape = shapes_of(big_path);
    assert_eq!(small_shape.name, big_shape.name);
    assert_eq!(small_shape.triangles, big_shape.triangles);
    assert_eq!(small_shape.uvs, big_shape.uvs);
    assert_eq!(small_shape.normals, big_shape.normals);
    assert_eq!(small_shape.tangents, big_shape.tangents);
    assert_eq!(
        small_shape.skin.as_ref().map(|skin| &skin.bones),
        big_shape.skin.as_ref().map(|skin| &skin.bones)
    );
    assert_eq!(
        small_shape.skin.as_ref().map(|skin| &skin.influences),
        big_shape.skin.as_ref().map(|skin| &skin.influences)
    );
    assert_eq!(
        small_shape.positions,
        expected(25.0),
        "_0 must carry the small endpoint"
    );
    assert_eq!(
        big_shape.positions,
        expected(100.0),
        "_1 must carry the big endpoint"
    );
}

#[test]
fn pair_writes_two_files_that_differ_only_in_the_endpoint() {
    let (root, nif) = home("pair", NifTarget::Se);
    let value = export(&root, &nif, "pair", serde_json::json!({}));
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["result"]["target"], "se");
    assert_eq!(value["result"]["vertices"], 8);
    assert_eq!(value["result"]["triangles"], 12);
    let small_path = PathBuf::from(value["result"]["out_0"].as_str().expect("out_0"));
    let big_path = PathBuf::from(value["result"]["out_1"].as_str().expect("out_1"));
    assert_eq!(small_path, root.join("pair_0.nif"));
    assert_eq!(big_path, root.join("pair_1.nif"));
    assert!(small_path.is_file(), "_0 is missing");
    assert!(big_path.is_file(), "_1 is missing");
    assert_pair_differs_only_in_positions(&small_path, &big_path);
}

#[test]
fn an_le_input_pair_is_written_for_le() {
    let (root, nif) = home("le", NifTarget::Le);
    let value = export(&root, &nif, "le-pair", serde_json::json!({}));
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["result"]["target"], "le");
    assert_pair_differs_only_in_positions(&root.join("le-pair_0.nif"), &root.join("le-pair_1.nif"));
}

#[test]
fn the_naming_rule_accepts_previous_outputs_back() {
    let (root, nif) = home("naming", NifTarget::Se);
    // Each form must resolve to the same pair, including the `_0` output of
    // the run above: the rule is total, nothing is ambiguous enough to refuse.
    for out in ["naming", "naming_0", "naming_1", "naming_0.nif"] {
        let value = export(&root, &nif, out, serde_json::json!({}));
        assert_eq!(value["ok"], true, "{out}: {value}");
        assert_eq!(
            value["result"]["out_0"],
            root.join("naming_0.nif").display().to_string(),
            "{out}"
        );
        assert_eq!(
            value["result"]["out_1"],
            root.join("naming_1.nif").display().to_string(),
            "{out}"
        );
    }
}

#[test]
fn the_size_pair_swaps_the_endpoints() {
    let (root, nif) = home("sizes", NifTarget::Se);
    let value = export(
        &root,
        &nif,
        "sizes",
        serde_json::json!({ "size": ["big", "small"] }),
    );
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["result"]["endpoint_0"], "big");
    assert_eq!(value["result"]["endpoint_1"], "small");
    let big_shape = shapes_of(&root.join("sizes_0.nif"));
    let small_shape = shapes_of(&root.join("sizes_1.nif"));
    assert_eq!(big_shape.positions, expected(100.0));
    assert_eq!(small_shape.positions, expected(25.0));
}

#[test]
fn a_refused_pair_leaves_no_file_behind() {
    let (root, nif) = home("refused", NifTarget::Se);
    let value = export(
        &root,
        &nif,
        "refused",
        serde_json::json!({ "set": "Missing" }),
    );
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["kind"], "not_found");
    assert!(!root.join("refused_0.nif").exists());
    assert!(!root.join("refused_1.nif").exists());

    let value = export(
        &root,
        &nif,
        "refused",
        serde_json::json!({ "size": ["small"] }),
    );
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["error"]["kind"], "invalid_params");
    assert!(!root.join("refused_0.nif").exists());
    assert!(!root.join("refused_1.nif").exists());
}
