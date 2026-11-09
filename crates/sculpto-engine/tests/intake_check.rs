//! `intake.check` audits a raw garment export and reports its topology defect
//! counts against the declared limits (G04 + G07).
//!
//! Each case writes its fixture inline as OBJ text, so the audit is exercised
//! through the real reader; no private payload is involved. The clean fixture
//! is a closed unit cube with one UV per vertex; the bad fixture carries one
//! degenerate triangle, one duplicate position and one open boundary loop.

use std::path::{Path, PathBuf};

use sculpto_engine::{ErrorKind, dispatch, envelope};

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-intake-check-{}-{name}",
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

fn write_obj(directory: &Path, name: &str, text: &str) -> PathBuf {
    let path = directory.join(name);
    std::fs::write(&path, text).expect("write fixture");
    path
}

/// A closed axis-aligned unit cube: every edge carried twice, full UV layer.
const CLEAN_CUBE: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0 0 1
v 1 0 1
v 1 1 1
v 0 1 1
vt 0 0
vt 1 0
vt 1 1
vt 0 1
vt 0 0
vt 1 0
vt 1 1
vt 0 1
f 1/1 2/2 3/3 4/4
f 5/5 8/8 7/7 6/6
f 1/1 5/5 6/6 2/2
f 4/4 3/3 7/7 8/8
f 2/2 6/6 7/7 3/3
f 1/1 4/4 8/8 5/5
";

/// A square sheet plus one collinear triangle and one repeated position.
const DEFECTED_SHEET: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0 0 0
v 2 0 0
v 3 0 0
v 4 0 0
vt 0 0
vt 1 0
vt 1 1
vt 0 1
f 1/1 2/2 3/3
f 1/1 3/3 4/4
f 6/1 7/2 8/3
";

#[test]
fn the_catalogue_lists_intake_check_with_a_schema() {
    let listed = dispatch("commands.list", &serde_json::Value::Null).expect("list");
    let row = listed["commands"]
        .as_array()
        .expect("array")
        .iter()
        .find(|row| row["id"] == "intake.check")
        .expect("intake.check row");
    assert_eq!(row["implemented"], true);
    assert_eq!(row["mutates"], false);
    assert_eq!(
        row["input_schema"]["properties"]["file"]["type"], "string",
        "the schema is derived from the typed parameters"
    );
}

#[test]
fn a_clean_cube_is_accepted_with_zero_counts() {
    let scratch = Scratch::new("clean");
    let path = write_obj(scratch.path(), "cube.obj", CLEAN_CUBE);
    let result = dispatch(
        "intake.check",
        &serde_json::json!({ "file": path.display().to_string() }),
    )
    .expect("the cube is accepted");

    assert_eq!(result["verdict"], "accept");
    assert_eq!(result["vertices"], 8);
    assert_eq!(result["triangles"], 12);
    assert_eq!(result["counts"]["non_manifold_edges"], 0);
    assert_eq!(result["counts"]["degenerate_triangles"], 0);
    assert_eq!(result["counts"]["duplicate_positions"], 0);
    assert_eq!(result["counts"]["missing_uvs"], 0);
    assert_eq!(result["counts"]["open_boundary_edges"], 0);
    assert_eq!(result["limits"]["non_manifold_edges"], 0);
    assert_eq!(result["limits"]["open_boundary_edges"], 512);

    let printed = envelope(Ok(result));
    assert_eq!(printed["ok"], true);
    assert!(printed["result"]["counts"].is_object());
    assert!(printed["result"]["verdict"].is_string());
}

#[test]
fn a_sheet_with_cloth_openings_is_accepted_with_exact_counts() {
    // A UV-covered square sheet: its four hem edges are the only counts, and
    // they stay inside the default boundary budget.
    let scratch = Scratch::new("sheet");
    let hemmed = write_obj(
        scratch.path(),
        "hemmed.obj",
        "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
         vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
         f 1/1 2/2 3/3\nf 1/1 3/3 4/4\n",
    );
    let result = dispatch(
        "intake.check",
        &serde_json::json!({ "file": hemmed.display().to_string() }),
    )
    .expect("the hemmed sheet stays inside the default limits");

    assert_eq!(result["verdict"], "accept");
    assert_eq!(result["vertices"], 4);
    assert_eq!(result["triangles"], 2);
    assert_eq!(result["counts"]["non_manifold_edges"], 0);
    assert_eq!(result["counts"]["degenerate_triangles"], 0);
    assert_eq!(result["counts"]["duplicate_positions"], 0);
    assert_eq!(result["counts"]["missing_uvs"], 0);
    assert_eq!(result["counts"]["open_boundary_edges"], 4);
    assert_eq!(result["limits"]["open_boundary_edges"], 512);
}

#[test]
fn a_defected_sheet_is_refused_naming_each_exceeded_count() {
    let scratch = Scratch::new("defected");
    let path = write_obj(scratch.path(), "sheet.obj", DEFECTED_SHEET);
    let error = dispatch(
        "intake.check",
        &serde_json::json!({ "file": path.display().to_string() }),
    )
    .expect_err("the zero defaults refuse a degenerate and a duplicate");

    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "intake_defects_over_limit");
    assert!(
        error
            .message()
            .contains("degenerate_triangles 1 > 0; duplicate_positions 1 > 0"),
        "the refusal lists each measured count and its limit: {}",
        error.message()
    );

    let printed = envelope(Err(error));
    assert_eq!(printed["ok"], false);
    assert_eq!(printed["error"]["kind"], "refused");
}

#[test]
fn an_over_limit_garment_is_refused_with_the_measured_counts() {
    // The defected sheet stays inside the defaults; a refusal needs an asset
    // that crosses one. A sheet with no UV layer leaves every vertex without
    // a UV, which is over the zero default.
    let scratch = Scratch::new("refused");
    let plain = write_obj(
        scratch.path(),
        "plain.obj",
        "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3\nf 1 3 4\n",
    );
    let error = dispatch(
        "intake.check",
        &serde_json::json!({ "file": plain.display().to_string() }),
    )
    .expect_err("the untextured sheet is refused");

    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "intake_defects_over_limit");
    assert!(
        error.message().contains("missing_uvs 4 > 0"),
        "the refusal lists the measured count and the limit: {}",
        error.message()
    );

    let printed = envelope(Err(error));
    assert_eq!(printed["ok"], false);
    assert_eq!(printed["error"]["kind"], "refused");
}

#[test]
fn a_missing_file_is_an_io_error() {
    let error = dispatch(
        "intake.check",
        &serde_json::json!({ "file": "/nonexistent/garment.obj" }),
    )
    .expect_err("a missing file cannot be audited");
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "intake_io");
}

#[test]
fn a_missing_file_parameter_is_invalid_params() {
    let error = dispatch("intake.check", &serde_json::Value::Null).expect_err("`file` is required");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
    assert_eq!(error.code(), "missing_field");
}
