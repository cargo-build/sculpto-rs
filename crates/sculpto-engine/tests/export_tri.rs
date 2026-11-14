//! BLD-10 acceptance: `export.tri` is the product surface over the landed `.tri`
//! writer.
//!
//! The command carries its morph document instead of reading one from a NIF
//! (no reader produces morph targets today), so the portable half proves the
//! registry row, the typed schema and the file our own reader decodes. The
//! native half hands the file to Outfit Studio's `ImportSliderData` step: the
//! step reads `TriFile::Read` and logs one line per imported morph, and it
//! *skips* a file whose shape name the loaded project does not carry, so the
//! import lines are the acceptance and a zero exit code alone is not.

use std::path::{Path, PathBuf};
use std::process::Command;

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::ErrorKind;
use sculpto_engine::ops::{self, ExportTriParams, TriMorphParams, TriOffsetParams};
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{compute_position_multiplier, read_tri};
use serde_json::{Value, json};

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sculpto-export-tri-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn morph(name: &str, offsets: &[(u32, [f32; 3])]) -> TriMorphParams {
    TriMorphParams {
        name: name.into(),
        offsets: offsets
            .iter()
            .map(|&(vertex, position)| TriOffsetParams { vertex, position })
            .collect(),
    }
}

/// One morph set over a 100-vertex shape: a three-lane displacement, a negative
/// lane and an all-zero lane all survive the writer's quantisation.
fn params(out: &Path) -> ExportTriParams {
    ExportTriParams {
        out: out.to_path_buf(),
        shape: "SculptoBody".into(),
        vertex_count: 100,
        morphs: vec![
            morph(
                "BicepsSize",
                &[(12, [1.5, -2.0, 3.25]), (42, [-5.0, 10.0, 0.0])],
            ),
            morph("WaistNarrow", &[(88, [0.25, 0.0, -0.5])]),
        ],
    }
}

fn input(out: &Path) -> Value {
    json!({
        "out": out.display().to_string(),
        "shape": "SculptoBody",
        "vertex_count": 100,
        "morphs": [
            {
                "name": "BicepsSize",
                "offsets": [
                    { "vertex": 12, "position": [1.5, -2.0, 3.25] },
                    { "vertex": 42, "position": [-5.0, 10.0, 0.0] }
                ]
            },
            {
                "name": "WaistNarrow",
                "offsets": [
                    { "vertex": 88, "position": [0.25, 0.0, -0.5] }
                ]
            }
        ]
    })
}

#[test]
fn the_command_writes_a_tri_our_reader_decodes() {
    let scratch = Scratch::new("reader");
    let out = scratch.path().join("body.tri");

    let result = sculpto_engine::dispatch("export.tri", &input(&out)).expect("the command writes");
    assert_eq!(result["shape"], "SculptoBody");
    assert_eq!(result["vertex_count"], 100);
    assert_eq!(result["morphs"], 2);
    assert_eq!(result["offsets"], 3);
    assert_eq!(
        result["bytes"],
        std::fs::metadata(&out).expect("written file").len()
    );

    let bytes = std::fs::read(&out).expect("read the written file");
    assert_eq!(
        &bytes[..4],
        b"PIRT",
        "the file starts with the `.tri` magic"
    );

    let decoded = read_tri(&bytes).expect("our reader decodes the file");
    assert_eq!(decoded.shapes.len(), 1);
    let shape = &decoded.shapes[0];
    assert_eq!(shape.name, "SculptoBody");
    assert_eq!(
        shape.vertex_count, None,
        "the layout stores no vertex count: the command's `vertex_count` bounds the \
         offsets at write time and is not recoverable from the file"
    );
    assert_eq!(
        shape
            .position_morphs
            .iter()
            .map(|morph| morph.name.as_str())
            .collect::<Vec<_>>(),
        ["BicepsSize", "WaistNarrow"],
        "morph order is the caller's order"
    );
    assert!(shape.uv_morphs.is_empty(), "no UV morphs were requested");

    // The format stores one quantisation step per morph, so a decoded offset is
    // exact to half of it. The step is derived from the same offsets the caller
    // declared, not from the decoded values.
    let declared = params(&out);
    for (decoded_morph, source) in shape.position_morphs.iter().zip(&declared.morphs) {
        let step = compute_position_multiplier(
            &source
                .offsets
                .iter()
                .map(|offset| sculpto_nif::PositionOffset {
                    vertex: offset.vertex as u16,
                    offset: offset.position,
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(decoded_morph.offsets.len(), source.offsets.len());
        for (written, expected) in decoded_morph.offsets.iter().zip(&source.offsets) {
            assert_eq!(written.vertex, expected.vertex as u16);
            for lane in 0..3 {
                let error = (written.offset[lane] - expected.position[lane]).abs();
                assert!(
                    error <= step / 2.0 + f32::EPSILON,
                    "`{}` vertex {} lane {lane}: wrote {} for {} (step {step})",
                    source.name,
                    written.vertex,
                    written.offset[lane],
                    expected.position[lane]
                );
            }
        }
    }
}

#[test]
fn the_registry_row_is_typed_and_implemented() {
    let spec = sculpto_engine::catalogue()
        .iter()
        .find(|spec| spec.id.as_str() == "export.tri")
        .expect("the row is registered");
    assert!(spec.implemented(), "the row has a handler");
    assert!(spec.mutates, "the row writes a file");

    let schema = spec.input_schema();
    let required = schema["required"]
        .as_array()
        .expect("required list")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    for field in ["out", "shape", "vertex_count", "morphs"] {
        assert!(required.contains(&field), "`{field}` is required");
        assert!(
            schema["properties"].get(field).is_some(),
            "`{field}` is typed"
        );
    }
    assert_eq!(
        schema["properties"]["morphs"]["items"]["$ref"], "#/$defs/TriMorphParams",
        "the nested morph document is typed, not opaque"
    );
}

/// Every refusal explains itself and leaves no file behind.
#[test]
fn refusals_name_the_defect_and_write_nothing() {
    let scratch = Scratch::new("refusals");

    let cases: Vec<(&str, ExportTriParams, &str)> = vec![
        (
            "vertex outside the declared count",
            ExportTriParams {
                morphs: vec![morph("OutOfRange", &[(100, [1.0, 0.0, 0.0])])],
                ..params(&scratch.path().join("out-of-range.tri"))
            },
            "tri_vertex_out_of_range",
        ),
        (
            "one offset per vertex",
            ExportTriParams {
                morphs: vec![morph(
                    "Duplicate",
                    &[(7, [1.0, 0.0, 0.0]), (7, [0.0, 0.0, 0.0])],
                )],
                ..params(&scratch.path().join("duplicate.tri"))
            },
            "tri_duplicate_vertex",
        ),
        (
            "vertex above the `u16` field",
            ExportTriParams {
                morphs: vec![morph("TooHigh", &[(70_000, [1.0, 0.0, 0.0])])],
                ..params(&scratch.path().join("too-high.tri"))
            },
            "invalid_value",
        ),
        (
            "non-finite displacement",
            ExportTriParams {
                morphs: vec![morph("NotFinite", &[(1, [f32::NAN, 0.0, 0.0])])],
                ..params(&scratch.path().join("not-finite.tri"))
            },
            "invalid_value",
        ),
    ];

    for (case, params, code) in cases {
        let error = ops::export_tri(&params).expect_err(case);
        assert_eq!(error.kind(), ErrorKind::InvalidParams, "{case}");
        assert_eq!(error.code(), code, "{case}: {error}");
        assert!(
            !params.out.exists(),
            "{case} must not write {}",
            params.out.display()
        );
    }
}

/// One quad, the shape name the `.tri` binds to.
fn native_nif(path: &Path) {
    let asset = MeshAsset {
        name: "SculptoNativeTri".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [1.5, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    };
    let bytes = write_se_shape(&asset).expect("the fixture writes as SE");
    std::fs::write(path, bytes).expect("write the native source NIF");
}

/// Native acceptance: Outfit Studio's own `ImportSliderData` step reads the file.
///
/// The gate names a **writable** BodySlide home, staged as an isolated copy: the
/// step is resolved from `<home>/Automations/`, and the tool appends
/// `<home>/Log_OS.txt`. Both inputs are ours — a shape written by the SE writer
/// and the `.tri` from this command — so the run proves the native reader, not a
/// private project fixture. The tool's GTK frame still needs a display; run the
/// test under `xvfb-run -a` (or any private X server).
///
/// The morph names carry the process id, so a stale log line cannot stand in for
/// this import, and the two exact `Imported morph` lines are required.
#[test]
fn native_import_accepts_the_written_tri() {
    let Some(home) = std::env::var_os("SCULPTO_TRI_NATIVE_HOME").map(PathBuf::from) else {
        eprintln!("SCULPTO_TRI_NATIVE_HOME unset: native ImportSliderData acceptance skipped");
        return;
    };
    let binary = home.join("OutfitStudio");
    if !binary.exists() {
        eprintln!(
            "{}: no OutfitStudio binary, native ImportSliderData acceptance skipped",
            home.display()
        );
        return;
    }

    let scratch = Scratch::new("native");
    let nif = scratch.path().join("native-tri.nif");
    native_nif(&nif);
    let tri = scratch.path().join("native-tri.tri");
    let small = format!("SculptoNativeSmall{}", std::process::id());
    let big = format!("SculptoNativeBig{}", std::process::id());
    let params = ExportTriParams {
        out: tri.clone(),
        shape: "SculptoNativeTri".into(),
        vertex_count: 4,
        morphs: vec![
            morph(&small, &[(1, [0.5, 0.0, 0.0]), (2, [0.0, 0.25, 0.0])]),
            morph(&big, &[(3, [0.0, 0.0, -0.75])]),
        ],
    };
    let written = ops::export_tri(&params).expect("the command writes the native input");

    let automations = home.join("Automations");
    std::fs::create_dir_all(&automations).expect("the staged home carries an Automations folder");
    let script = format!("sculpto_tri_accept_{}", std::process::id());
    let script_path = automations.join(format!("{script}.xml"));
    std::fs::write(
        &script_path,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<AutomationScript version="1">
  <Step type="ImportFile">
    <FilePath>{nif}</FilePath>
  </Step>
  <Step type="ImportSliderData">
    <SliderDataFile>{tri}</SliderDataFile>
    <SliderNames>{small},{big}</SliderNames>
  </Step>
</AutomationScript>
"#,
            nif = nif.display(),
            tri = tri.display(),
        ),
    )
    .expect("write the automation script");

    let status = Command::new(&binary)
        .arg("-a")
        .arg(&script)
        .current_dir(&home)
        .status()
        .expect("run Outfit Studio headlessly");
    assert!(
        status.success(),
        "Outfit Studio refused the automation: {status}"
    );

    let log = std::fs::read_to_string(home.join("Log_OS.txt")).expect("the tool writes Log_OS.txt");
    for name in [&small, &big] {
        let line = format!("Imported morph '{name}' for shape 'SculptoNativeTri'");
        assert!(
            log.contains(&line),
            "the native reader did not import `{name}` ({written_bytes} bytes written); \
             Log_OS.txt has no `{line}`",
            written_bytes = written.bytes
        );
    }

    std::fs::remove_file(&script_path).expect("remove the task's automation script");
}
