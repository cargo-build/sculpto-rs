//! `export.nif` re-encodes a readable shape for a Skyrim target.
//!
//! Each file case builds its source with the real SE writer, so the input is a
//! file the reader already accepts; the output is re-read instead of compared to
//! a golden, and no private payload or byte fixture is needed. The validation
//! gate is driven through [`ops::write_nif`], the seam `export_nif` uses, because
//! every NIF the readers accept converts back into a valid model.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::ErrorKind;
use sculpto_engine::ops::{self, ExportNifParams, ExportTarget};
use sculpto_nif::write::se_shape::write_se_shape;

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sculpto-export-nif-{}-{name}", std::process::id()));
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

fn quad() -> MeshAsset {
    MeshAsset {
        name: "Body".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    }
}

fn source_nif(directory: &Path, asset: &MeshAsset) -> PathBuf {
    let path = directory.join("source-se.nif");
    let bytes = write_se_shape(asset).expect("the fixture writes as SE");
    std::fs::write(&path, bytes).expect("source bytes");
    path
}

fn params(nif: PathBuf, out: PathBuf, target: ExportTarget) -> ExportNifParams {
    ExportNifParams {
        nif,
        out,
        target,
        shape: None,
        rig_template: None,
    }
}

#[test]
fn se_source_exports_to_le_with_the_same_counts() {
    let scratch = Scratch::new("le");
    let source = source_nif(scratch.path(), &quad());
    let out = scratch.path().join("out-le.nif");
    let outcome =
        ops::export_nif(&params(source, out.clone(), ExportTarget::Le)).expect("LE export");

    let bytes = std::fs::read(&out).expect("output bytes");
    let document = sculpto_nif::read(&bytes).expect("the export re-reads");
    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].positions.len(), 4);
    assert_eq!(document.shapes[0].triangles.len(), 2);
    assert_eq!(outcome.vertices, 4);
    assert_eq!(outcome.triangles, 2);
    assert_eq!(outcome.bytes, bytes.len());
    assert_eq!(outcome.target, ExportTarget::Le);
}

#[test]
fn se_source_exports_to_se_with_the_same_counts() {
    let scratch = Scratch::new("se");
    let source = source_nif(scratch.path(), &quad());
    let out = scratch.path().join("out-se.nif");
    let outcome =
        ops::export_nif(&params(source, out.clone(), ExportTarget::Se)).expect("SE export");

    let bytes = std::fs::read(&out).expect("output bytes");
    let document = sculpto_nif::read(&bytes).expect("the export re-reads");
    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].positions.len(), 4);
    assert_eq!(document.shapes[0].triangles.len(), 2);
    assert_eq!(outcome.vertices, 4);
    assert_eq!(outcome.triangles, 2);
    assert_eq!(outcome.bytes, bytes.len());
    assert_eq!(outcome.target, ExportTarget::Se);
}

#[test]
fn a_missing_source_is_a_typed_io_error() {
    let scratch = Scratch::new("missing");
    let out = scratch.path().join("out.nif");
    let error = ops::export_nif(&params(
        scratch.path().join("absent.nif"),
        out.clone(),
        ExportTarget::Se,
    ))
    .expect_err("an absent source must fail");

    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "io_error");
    assert!(!out.exists(), "a failed export must write no file");
}

#[test]
fn an_unknown_shape_lists_the_available_names() {
    let scratch = Scratch::new("shape");
    let source = source_nif(scratch.path(), &quad());
    let out = scratch.path().join("out.nif");
    let mut params = params(source, out.clone(), ExportTarget::Se);
    params.shape = Some("Cape".into());
    let error = ops::export_nif(&params).expect_err("an unknown shape must fail");

    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.code(), "shape_not_found");
    assert!(error.message().contains("`Cape`"), "{}", error.message());
    assert!(error.message().contains("Body"), "{}", error.message());
    assert!(!out.exists(), "a failed export must write no file");
}

#[test]
fn a_model_that_fails_validation_produces_no_bytes() {
    let mut asset = quad();
    asset.triangles[0] = [0, 1, 9];
    asset.validate().expect_err("the fixture must be invalid");
    let error = ops::write_nif(&asset, ExportTarget::Se)
        .expect_err("an invalid model must not reach the writer");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert_eq!(error.code(), "mesh_triangle_range");
}
