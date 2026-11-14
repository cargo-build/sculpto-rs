//! `export.bundle` re-encodes a multi-shape NIF without dropping shapes.
//!
//! Each source is built with the real SE bundle writer, so the input is a file
//! the reader already accepts. The round trip reads the source, exports every
//! shape and re-reads the output: shape count, names, per-shape counts and the
//! shared skin must survive. No private payload or byte fixture is needed for
//! those synthetic controls.
//!
//! One control is the private-host tier: `SCULPTO_BUNDLE_NIF` names a real
//! authored outfit whose shapes are skinned to different subsets of one
//! skeleton. It skips with a message when unset, like the other fixture-gated
//! tests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sculpto_core::mesh::validate_bundle;
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::ErrorKind;
use sculpto_engine::ops::{self, ExportBundleParams, ExportNifParams, ExportTarget, ShapeCount};
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::skin::identity;

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-export-multi-{}-{name}",
            std::process::id()
        ));
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

/// One skinned shape on the shared two-bone palette.
fn skinned(name: &str, base: f32, vertices: usize) -> MeshAsset {
    let positions = (0..vertices)
        .map(|vertex| [base + vertex as f32, f32::from(vertex as u8 % 2), 0.0])
        .collect::<Vec<_>>();
    let triangles = match vertices > 3 {
        true => vec![[0, 1, 2], [1, 2, 3]],
        false => vec![[0, 1, 2]],
    };
    MeshAsset {
        name: name.into(),
        normals: vec![[0.0, 0.0, 1.0]; vertices],
        uvs: (0..vertices)
            .map(|vertex| [vertex as f32, f32::from(vertex as u8 % 2)])
            .collect(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: vec![identity(); 2],
            influences: SkinWeights::from_quads(
                &vec![[0, 1, 0, 0]; vertices],
                &vec![[0.75, 0.25, 0.0, 0.0]; vertices],
            )
            .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

/// Two shapes in scene order: the smaller one first, so "the largest" and
/// "the first" are different shapes and a first-shape-only export is visible.
fn bundle() -> Vec<MeshAsset> {
    vec![skinned("Hands", 4.0, 3), skinned("Body", 0.0, 4)]
}

fn source_nif(directory: &Path, shapes: &[MeshAsset]) -> PathBuf {
    let path = directory.join("source-se.nif");
    let bytes = write_se_bundle(shapes).expect("the fixture writes as an SE bundle");
    std::fs::write(&path, bytes).expect("source bytes");
    path
}

fn params(nif: PathBuf, out: PathBuf, target: ExportTarget) -> ExportBundleParams {
    ExportBundleParams {
        nif,
        out,
        target,
        shapes: None,
    }
}

/// The per-shape facts the round trip must preserve, in scene order.
fn expected_counts() -> Vec<ShapeCount> {
    vec![
        ShapeCount {
            name: "Hands".into(),
            vertices: 3,
            triangles: 1,
        },
        ShapeCount {
            name: "Body".into(),
            vertices: 4,
            triangles: 2,
        },
    ]
}

fn assert_roundtrip(out: &Path, outcome: &ops::ExportBundleOutcome, target: ExportTarget) {
    let bytes = std::fs::read(out).expect("output bytes");
    let document = sculpto_nif::read(&bytes).expect("the export re-reads");
    assert_eq!(document.shapes.len(), 2, "no shape was dropped");
    let counts: Vec<ShapeCount> = document
        .shapes
        .iter()
        .map(|shape| ShapeCount {
            name: shape.name.clone(),
            vertices: shape.positions.len(),
            triangles: shape.triangles.len(),
        })
        .collect();
    assert_eq!(
        counts,
        expected_counts(),
        "names and per-shape counts survive"
    );
    assert_eq!(
        outcome.shapes,
        expected_counts(),
        "the outcome reports each shape"
    );
    assert_eq!(outcome.vertices, 7);
    assert_eq!(outcome.triangles, 3);
    assert_eq!(outcome.bytes, bytes.len());
    assert_eq!(outcome.target, target);
    for shape in &document.shapes {
        let skin = shape.skin.as_ref().expect("each shape keeps its skin");
        assert_eq!(skin.bones, vec!["Root", "Spine"], "shared palette order");
        assert_eq!(skin.influences.rows(), shape.positions.len());
        for vertex in 0..skin.influences.rows() {
            let row = skin.influences.row(vertex).expect("row");
            assert_eq!(row.joints, &[0, 1], "bone lanes survive");
            for (left, right) in row.weights.iter().zip([0.75, 0.25]) {
                assert!((left - right).abs() <= 1e-3, "weight drifted (f16)");
            }
        }
    }
}

#[test]
fn two_shape_roundtrip_se() {
    let scratch = Scratch::new("se");
    let shapes = bundle();
    let source = source_nif(scratch.path(), &shapes);
    let read_back = sculpto_nif::read(&std::fs::read(&source).expect("source")).expect("read");
    assert_eq!(
        read_back.shapes.len(),
        2,
        "the source itself is multi-shape"
    );

    let out = scratch.path().join("out-se.nif");
    let outcome = ops::export_nif_bundle(&params(source, out.clone(), ExportTarget::Se))
        .expect("SE bundle export");
    assert_roundtrip(&out, &outcome, ExportTarget::Se);
}

#[test]
fn two_shape_roundtrip_le() {
    let scratch = Scratch::new("le");
    let source = source_nif(scratch.path(), &bundle());
    let out = scratch.path().join("out-le.nif");
    let outcome = ops::export_nif_bundle(&params(source, out.clone(), ExportTarget::Le))
        .expect("LE bundle export");
    assert_roundtrip(&out, &outcome, ExportTarget::Le);
}

#[test]
fn unskinned_shapes_roundtrip() {
    let scratch = Scratch::new("unskinned");
    let shapes = vec![
        MeshAsset {
            skin: None,
            ..skinned("Cape", 0.0, 3)
        },
        MeshAsset {
            skin: None,
            ..skinned("Boot", 4.0, 4)
        },
    ];
    let source = source_nif(scratch.path(), &shapes);
    let out = scratch.path().join("out.nif");
    let outcome = ops::export_nif_bundle(&params(source, out.clone(), ExportTarget::Se))
        .expect("unskinned bundle export");
    let document = sculpto_nif::read(&std::fs::read(&out).expect("bytes")).expect("re-read");
    assert_eq!(document.shapes.len(), 2);
    assert!(document.shapes.iter().all(|shape| shape.skin.is_none()));
    assert_eq!(
        outcome.shapes,
        vec![
            ShapeCount {
                name: "Cape".into(),
                vertices: 3,
                triangles: 1
            },
            ShapeCount {
                name: "Boot".into(),
                vertices: 4,
                triangles: 2
            },
        ]
    );
}

#[test]
fn an_explicit_list_selects_and_orders() {
    let scratch = Scratch::new("list");
    let source = source_nif(scratch.path(), &bundle());
    let out = scratch.path().join("out.nif");
    let mut params = params(source, out.clone(), ExportTarget::Se);
    params.shapes = Some(vec!["Body".into(), "Hands".into()]);
    let outcome = ops::export_nif_bundle(&params).expect("explicit list export");
    assert_eq!(
        outcome.shapes,
        expected_counts().into_iter().rev().collect::<Vec<_>>(),
        "the list order is the write order"
    );
    let document = sculpto_nif::read(&std::fs::read(&out).expect("bytes")).expect("re-read");
    let names: Vec<&str> = document
        .shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    assert_eq!(names, vec!["Body", "Hands"]);
}

#[test]
fn an_unknown_name_lists_the_available_shapes() {
    let scratch = Scratch::new("unknown");
    let source = source_nif(scratch.path(), &bundle());
    let out = scratch.path().join("out.nif");
    let mut params = params(source, out.clone(), ExportTarget::Se);
    params.shapes = Some(vec!["Cape".into()]);
    let error = ops::export_nif_bundle(&params).expect_err("an unknown shape must fail");
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.code(), "shape_not_found");
    assert!(error.message().contains("`Cape`"), "{}", error.message());
    assert!(error.message().contains("Body"), "{}", error.message());
    assert!(!out.exists(), "a failed export must write no file");
}

#[test]
fn an_empty_list_is_a_bundle_refusal() {
    let scratch = Scratch::new("empty");
    let source = source_nif(scratch.path(), &bundle());
    let out = scratch.path().join("out.nif");
    let mut params = params(source, out.clone(), ExportTarget::Se);
    params.shapes = Some(Vec::new());
    let error = ops::export_nif_bundle(&params).expect_err("an empty bundle must fail");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "nif_write_invalid_bundle");
    assert!(!out.exists(), "a failed export must write no file");
}

#[test]
fn repeated_names_are_a_bundle_refusal() {
    let scratch = Scratch::new("duplicate");
    let source = source_nif(scratch.path(), &bundle());
    let out = scratch.path().join("out.nif");
    let mut params = params(source, out.clone(), ExportTarget::Se);
    params.shapes = Some(vec!["Body".into(), "Body".into()]);
    let error = ops::export_nif_bundle(&params).expect_err("repeated names must fail");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "nif_write_invalid_bundle");
    assert!(!out.exists(), "a failed export must write no file");
}

#[test]
fn the_default_export_stays_single_shape() {
    let scratch = Scratch::new("compat");
    // Unskinned: the checked-rig single-shape path needs a reader-captured
    // skeleton root, which the writers' flat bone nodes do not provide. That
    // pre-existing gap is independent of the bundle surface; the compatibility
    // contract here is "default writes the largest shape, one shape only".
    let shapes = bundle()
        .into_iter()
        .map(|shape| MeshAsset {
            skin: None,
            ..shape
        })
        .collect::<Vec<_>>();
    let source = source_nif(scratch.path(), &shapes);
    let out = scratch.path().join("out.nif");
    let outcome = ops::export_nif(&ExportNifParams {
        nif: source,
        out: out.clone(),
        target: ExportTarget::Se,
        shape: None,
        rig_template: None,
    })
    .expect("the single-shape default still works");
    assert_eq!(outcome.vertices, 4, "the largest shape, not the first");
    assert_eq!(outcome.triangles, 2);
    let document = sculpto_nif::read(&std::fs::read(&out).expect("bytes")).expect("re-read");
    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].name, "Body");
}

/// The private-host control: a real authored outfit whose shapes are skinned to
/// different bone subsets of one skeleton.
///
/// The bundle gate must accept the shared union (a common root is the only
/// skeleton requirement), and `--all-shapes` must re-encode every shape with its
/// own palette, in its authored order.
#[test]
fn real_subset_outfit_bundle_roundtrip() {
    let Ok(path) = std::env::var("SCULPTO_BUNDLE_NIF") else {
        eprintln!("SCULPTO_BUNDLE_NIF not set; skipping the real subset outfit control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the fixture NIF is readable");
    let source = sculpto_nif::read(&bytes).expect("the real outfit parses");
    let target = match source.bsver {
        83 => ExportTarget::Le,
        _ => ExportTarget::Se,
    };
    let expected: Vec<ShapeCount> = source
        .shapes
        .iter()
        .map(|shape| ShapeCount {
            name: shape.name.clone(),
            vertices: shape.positions.len(),
            triangles: shape.triangles.len(),
        })
        .collect();
    let palettes: Vec<Vec<String>> = source
        .shapes
        .iter()
        .map(|shape| {
            shape
                .skin
                .as_ref()
                .map(|skin| skin.bones.clone())
                .unwrap_or_default()
        })
        .collect();
    let union: BTreeSet<&str> = palettes
        .iter()
        .flat_map(|palette| palette.iter().map(String::as_str))
        .collect();
    for (index, palette) in palettes.iter().enumerate() {
        eprintln!(
            "source shape {index}: `{}` {} vertices / {} triangles, {} bones",
            expected[index].name,
            expected[index].vertices,
            expected[index].triangles,
            palette.len()
        );
    }
    eprintln!(
        "{:?} carries {} shapes over a {} bone union (bsver {})",
        path,
        expected.len(),
        union.len(),
        source.bsver
    );
    assert!(
        palettes.iter().any(|palette| *palette != palettes[0]),
        "the control needs shapes whose palettes are not all identical"
    );
    let mut shared = union.clone();
    for palette in &palettes {
        shared.retain(|bone| palette.iter().any(|name| name == bone));
    }
    eprintln!("common skeleton root: {shared:?}");

    let assets = source
        .shapes
        .iter()
        .map(MeshAsset::from)
        .collect::<Vec<_>>();
    validate_bundle(&assets).expect("one skeleton with per-shape subsets is a bundle");

    let scratch = Scratch::new("real");
    let out = scratch.path().join("out.nif");
    let outcome = ops::export_nif_bundle(&params(PathBuf::from(&path), out.clone(), target))
        .expect("--all-shapes re-encodes the authored outfit");
    assert_eq!(
        outcome.shapes, expected,
        "the outcome reports every source shape"
    );
    assert_eq!(
        outcome.vertices,
        expected.iter().map(|s| s.vertices).sum::<usize>()
    );
    assert_eq!(
        outcome.triangles,
        expected.iter().map(|s| s.triangles).sum::<usize>()
    );

    let written = std::fs::read(&out).expect("output bytes");
    let document = sculpto_nif::read(&written).expect("the export re-reads");
    assert_eq!(
        document.shapes.len(),
        expected.len(),
        "no authored shape was dropped"
    );
    let counts: Vec<ShapeCount> = document
        .shapes
        .iter()
        .map(|shape| ShapeCount {
            name: shape.name.clone(),
            vertices: shape.positions.len(),
            triangles: shape.triangles.len(),
        })
        .collect();
    assert_eq!(counts, expected, "names and per-shape counts survive");
    let written_palettes: Vec<Vec<String>> = document
        .shapes
        .iter()
        .map(|shape| {
            shape
                .skin
                .as_ref()
                .map(|skin| skin.bones.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(
        written_palettes, palettes,
        "each shape keeps its own bone subset, in authored order"
    );
    assert_eq!(outcome.bytes, written.len());
    eprintln!(
        "re-encoded {} bytes: {} shapes, {} palettes distinct, {} bone union",
        written.len(),
        counts.len(),
        palettes.iter().collect::<BTreeSet<&Vec<String>>>().len(),
        union.len()
    );
}
