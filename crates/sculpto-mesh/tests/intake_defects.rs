//! G04 defect report and G07 thresholds: measured counts on synthetic reads,
//! never repaired, and the typed refusal that names every exceeded count.
//! Fixtures are inline OBJ text or hand-built [`RawGarment`] values; no
//! private payload is consulted.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::MaterialSlot;
use sculpto_mesh::intake_gltf::{
    DefectLimits, DefectOverLimit, RawGarment, ScaleReport, ScaleRule, TopologyDefects, read_raw,
};

/// A closed axis-aligned unit cube with one UV per vertex: the quad-sphere-ish
/// clean fixture. Six quads fan-triangulate into 12 triangles, every edge is
/// carried twice, nothing degenerates, nothing duplicates.
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

/// A flat square sheet plus one collinear triangle and one repeated position:
/// the sheet's four outer edges form the single open boundary loop, the
/// collinear triangle is the one degenerate, the repeated `v` line is the one
/// duplicate.
const MIXED_SHEET: &str = "\
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

/// A square sheet that authors no `vt` at all.
const UNTEXTURED_SHEET: &str = "\
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3
f 1 3 4
";

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "sculpto-intake-defects-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("temp home");
    root
}

fn write_obj(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, text).expect("write fixture");
    path
}

/// A hand-built intake read, for shapes the readers cannot produce.
fn garment(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>, uvs: Vec<[f32; 2]>) -> RawGarment {
    RawGarment {
        name: "robe".into(),
        positions,
        normals: Vec::new(),
        uvs,
        triangles,
        material: MaterialSlot::default(),
        scale: ScaleReport {
            rule: ScaleRule::Metres,
            source_extent: 1.0,
            factor: 1.0,
        },
    }
}

#[test]
fn the_clean_cube_reports_zero_defects() {
    let root = temp_root("clean");
    let path = write_obj(&root, "cube.obj", CLEAN_CUBE);
    let read = read_raw(&path).expect("intake reads the cube");
    assert_eq!(read.defects(), TopologyDefects::default(), "0 defects");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_mixed_sheet_reports_each_defect_exactly() {
    let root = temp_root("mixed");
    let path = write_obj(&root, "mixed.obj", MIXED_SHEET);
    let read = read_raw(&path).expect("intake reads the sheet");
    assert_eq!(
        read.defects(),
        TopologyDefects {
            non_manifold_edges: 0,
            degenerate_triangles: 1,
            duplicate_positions: 1,
            missing_uvs: 0,
            // The sheet's one boundary loop: four edges. The degenerate
            // triangle's edges enter no census, so the count stays exact.
            open_boundary_edges: 4,
        }
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_fin_counts_one_non_manifold_edge() {
    // Three triangles over the shared edge 0-1: that edge is carried three
    // times, every other edge once.
    let fin = garment(
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.5, 0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.0, 0.5],
        ],
        vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]],
        vec![[0.0, 0.0]; 5],
    );
    assert_eq!(
        fin.defects(),
        TopologyDefects {
            non_manifold_edges: 1,
            degenerate_triangles: 0,
            duplicate_positions: 0,
            missing_uvs: 0,
            open_boundary_edges: 6,
        }
    );
}

#[test]
fn without_a_uv_layer_every_vertex_is_missing() {
    let root = temp_root("untextured");
    let path = write_obj(&root, "plain.obj", UNTEXTURED_SHEET);
    let read = read_raw(&path).expect("intake reads the sheet");
    assert!(
        read.uvs.is_empty(),
        "an unauthored OBJ layer stays empty, not zero-padded"
    );
    assert_eq!(
        read.defects(),
        TopologyDefects {
            non_manifold_edges: 0,
            degenerate_triangles: 0,
            duplicate_positions: 0,
            missing_uvs: 4,
            open_boundary_edges: 4,
        },
        "every vertex lacks a UV; the sheet's own loop is the only other count"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_partial_uv_layer_counts_its_uncovered_tail() {
    let sheet = garment(
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
        vec![[0.0, 0.0]; 2],
    );
    assert_eq!(sheet.defects().missing_uvs, 2);
}

#[test]
fn negative_zero_counts_as_a_duplicate_position() {
    let twin = garment(vec![[1.0, 0.0, -0.0], [1.0, 0.0, 0.0]], vec![], vec![]);
    assert_eq!(twin.defects().duplicate_positions, 1);
}

#[test]
fn the_refusal_names_every_exceeded_count_and_its_limit() {
    let limits = DefectLimits::default();
    let counts = TopologyDefects {
        non_manifold_edges: 3,
        degenerate_triangles: 2,
        duplicate_positions: 1,
        missing_uvs: 8,
        open_boundary_edges: 513,
    };
    let error = limits.check(counts).expect_err("every count is over");
    assert_eq!(
        error,
        DefectOverLimit { counts, limits },
        "the refusal carries the measured counts and the applied limits"
    );
    let message = error.to_string();
    for (name, measured, limit) in [
        ("non_manifold_edges", 3, 0),
        ("degenerate_triangles", 2, 0),
        ("duplicate_positions", 1, 0),
        ("missing_uvs", 8, 0),
        ("open_boundary_edges", 513, 512),
    ] {
        assert!(
            message.contains(&format!("{name} {measured} > {limit}")),
            "`{message}` must name {name} {measured} > {limit}"
        );
    }
}

#[test]
fn the_default_limits_accept_a_cloth_open_boundary() {
    let limits = DefectLimits::default();
    let hemmed = TopologyDefects {
        open_boundary_edges: 100,
        ..TopologyDefects::default()
    };
    assert_eq!(limits.check(hemmed), Ok(()));
    assert_eq!(limits.check(TopologyDefects::default()), Ok(()));
}
