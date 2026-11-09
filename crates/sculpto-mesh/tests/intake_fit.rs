//! G05 hand-off and G09 end-to-end: a synthetic garment GLB written by
//! `sculpto_gltf::write_glb` is read by `read_raw`, narrowed into the fit input,
//! bound to a synthetic body `MeshAsset` and applied to an unchanged and a
//! morphed body. No private fixture is involved.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId};
use sculpto_gltf::write_glb;
use sculpto_mesh::conform::{Conform, ConformError};
use sculpto_mesh::intake_gltf::{HandoffError, RawGarment, ScaleReport, ScaleRule, read_raw};

/// The conform suite's numeric tolerance: positions are `f32` and the authored
/// offset is added once, so the result stays within this bound
/// (`crates/sculpto-mesh/tests/conform.rs:22`).
const CONFORM_TOLERANCE: f32 = 1e-6;

/// Grid side of both synthetic meshes.
const GRID: usize = 5;
/// Vertex spacing in metres.
const SPACING: f32 = 0.25;
/// Authored garment thickness over the body surface.
const THICKNESS: f32 = 0.02;
/// Authored garment shrink against the body surface.
const SHRINK: f32 = 0.98;
/// The morphed body is the reference lifted by this vector.
const MORPH: [f32; 3] = [0.0, 0.0, 0.05];

/// A `GRID × GRID` sheet on `z = 0`, row-major.
fn grid_positions() -> Vec<[f32; 3]> {
    (0..GRID)
        .flat_map(|row| {
            (0..GRID).map(move |column| [column as f32 * SPACING, row as f32 * SPACING, 0.0])
        })
        .collect()
}

/// Two triangles per grid cell, row-major.
fn grid_triangles(base: u32) -> Vec<[u32; 3]> {
    let cells = (0..GRID - 1).flat_map(|row| (0..GRID - 1).map(move |column| (row, column)));
    cells
        .flat_map(|(row, column)| {
            let vertex = |row: usize, column: usize| base + (row * GRID + column) as u32;
            [
                [
                    vertex(row, column),
                    vertex(row + 1, column),
                    vertex(row + 1, column + 1),
                ],
                [
                    vertex(row, column),
                    vertex(row + 1, column + 1),
                    vertex(row, column + 1),
                ],
            ]
        })
        .collect()
}

/// The synthetic body: a flat sheet under the garment.
fn body_asset() -> MeshAsset {
    let triangles = grid_triangles(0);
    MeshAsset {
        name: "body".into(),
        positions: grid_positions(),
        normals: vec![[0.0, 0.0, 1.0]; GRID * GRID],
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    }
}

/// The synthetic garment: the body grid, shrunk and lifted by the authored
/// thickness, with one outward normal per vertex.
fn garment_asset() -> MeshAsset {
    let triangles = grid_triangles(0);
    MeshAsset {
        name: "robes".into(),
        positions: grid_positions()
            .into_iter()
            .map(|[x, y, _]| [x * SHRINK, y * SHRINK, THICKNESS])
            .collect(),
        normals: vec![[0.0, 0.0, 1.0]; GRID * GRID],
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("sculpto-intake-fit-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp home");
    root
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::write(path, bytes).expect("write fixture");
    path.to_owned()
}

fn length(vector: [f32; 3]) -> f32 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

/// Mean distance between two equally sized vertex arenas.
fn mean_displacement(actual: &[[f32; 3]], expected: &[[f32; 3]]) -> f32 {
    assert_eq!(
        actual.len(),
        expected.len(),
        "mean displacement needs pairs"
    );
    let sum: f32 = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| {
            length([
                actual[0] - expected[0],
                actual[1] - expected[1],
                actual[2] - expected[2],
            ])
        })
        .sum();
    sum / actual.len() as f32
}

fn assert_arena(actual: &[[f32; 3]], expected: &[[f32; 3]]) {
    assert_eq!(actual.len(), expected.len(), "vertex count");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        for (axis, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual - expected).abs() <= CONFORM_TOLERANCE,
                "vertex {index} axis {axis}: {actual} vs {expected}"
            );
        }
    }
}

/// A hand-built intake read, for the refusal cases the reader cannot produce.
fn raw(positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> RawGarment {
    RawGarment {
        name: "robe".into(),
        positions,
        normals,
        uvs: Vec::new(),
        triangles,
        material: MaterialSlot {
            name: "default".into(),
            ..MaterialSlot::default()
        },
        scale: ScaleReport {
            rule: ScaleRule::Metres,
            source_extent: 1.0,
            factor: 1.0,
        },
    }
}

#[test]
fn generated_glb_reaches_the_fit_path_inside_the_conform_tolerance() {
    let root = temp_root("end-to-end");
    let body = body_asset();
    let garment = garment_asset();
    let bytes = write_glb(&garment).expect("writes the garment GLB");
    let path = write(&root.join("robes.glb"), &bytes);

    let read = read_raw(&path).expect("intake reads the written GLB");
    let rule = read.scale.rule;
    let fit = read
        .into_fit_input()
        .expect("hands the garment to the fit path");

    assert_eq!(rule, ScaleRule::Metres);
    assert_eq!(fit.positions.len(), garment.positions.len(), "vertex count");
    assert_arena(&fit.positions, &garment.positions);
    assert_arena(&fit.normals, &garment.normals);
    assert_eq!(
        fit.normals.len(),
        fit.positions.len(),
        "normals stay per vertex"
    );
    assert_eq!(fit.triangles, garment.triangles);

    let conform = Conform::bind(&fit.positions, &body.positions).expect("binds to the body");
    let pinned: Vec<usize> = conform
        .bindings()
        .iter()
        .map(|binding| binding.vertex)
        .collect();
    assert_eq!(
        pinned,
        (0..fit.positions.len()).collect::<Vec<usize>>(),
        "intake order survives: garment vertex i binds to body vertex i"
    );

    let rest = conform
        .apply(&body.positions)
        .expect("applies the unchanged body");
    assert_eq!(
        rest.len(),
        fit.positions.len(),
        "the fitted vertex count is unchanged"
    );
    let rest_displacement = mean_displacement(&rest, &fit.positions);

    let morphed: Vec<[f32; 3]> = body
        .positions
        .iter()
        .map(|[x, y, z]| [x + MORPH[0], y + MORPH[1], z + MORPH[2]])
        .collect();
    let moved = conform.apply(&morphed).expect("applies the morphed body");
    assert_eq!(
        moved.len(),
        fit.positions.len(),
        "the fitted vertex count is unchanged on the morphed body"
    );
    let morph_displacement = mean_displacement(&moved, &fit.positions);
    let follow = conform
        .follow(&body.positions, &morphed)
        .expect("follows the morph");

    println!(
        "vertices={} triangles={} offset_mean={:.6} offset_max={:.6} rest_displacement={rest_displacement:.9} morph_displacement={morph_displacement:.9} follow_body={:.6} follow_garment={:.6} follow_difference={:.9}",
        fit.positions.len(),
        fit.triangles.len(),
        conform.offset_mean(),
        conform.offset_max(),
        follow.body,
        follow.garment,
        follow.difference,
    );

    assert!(
        rest_displacement <= CONFORM_TOLERANCE,
        "the unchanged body must reproduce the authored garment: {rest_displacement}"
    );
    assert!(
        (morph_displacement - length(MORPH)).abs() <= CONFORM_TOLERANCE,
        "the morphed fit must move with the body ({morph_displacement} vs {})",
        length(MORPH)
    );
    assert!(
        follow.difference <= CONFORM_TOLERANCE,
        "the garment moved exactly with its body vertices: {}",
        follow.difference
    );
    assert!(
        conform.offset_mean() > 0.0,
        "the authored thickness survived the hand-off"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_hand_off_refuses_an_empty_garment_like_the_conform_binding() {
    let empty = raw(Vec::new(), Vec::new(), Vec::new());
    assert_eq!(
        empty.into_fit_input().unwrap_err(),
        HandoffError::EmptyGarment
    );
    assert_eq!(
        Conform::bind(&[], &[[0.0, 0.0, 0.0]]).unwrap_err(),
        ConformError::EmptyGarment,
        "the hand-off refusal is conform's own rule"
    );
}

#[test]
fn the_hand_off_refuses_normals_that_do_not_cover_the_arena() {
    let mismatched = raw(
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        vec![[0.0, 0.0, 1.0]],
        Vec::new(),
    );
    assert_eq!(
        mismatched.into_fit_input().unwrap_err(),
        HandoffError::NormalCount {
            positions: 2,
            normals: 1,
        }
    );
}

#[test]
fn the_hand_off_refuses_a_triangle_outside_the_arena() {
    let stray = raw(
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        Vec::new(),
        vec![[0, 1, 3]],
    );
    assert_eq!(
        stray.into_fit_input().unwrap_err(),
        HandoffError::TriangleVertexOutOfRange {
            triangle: 0,
            vertex: 3,
        }
    );
}

#[test]
fn an_intake_triangle_outside_the_arena_is_refused_at_the_hand_off() {
    let root = temp_root("stray-index");
    let mut bin = Vec::new();
    for [x, y, z] in [[0.0f32, 0.0, 0.0], [1.2, 0.0, 0.0], [0.0, 1.5, 0.0]] {
        bin.extend_from_slice(&x.to_le_bytes());
        bin.extend_from_slice(&y.to_le_bytes());
        bin.extend_from_slice(&z.to_le_bytes());
    }
    for index in [0u32, 1, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    write(&root.join("raw.bin"), &bin);
    let json = serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "mode": 4}]}],
        "buffers": [{"uri": "raw.bin", "byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 12},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [0.0, 0.0, 0.0], "max": [1.2, 1.5, 0.0]},
            {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"},
        ],
    });
    let path = write(
        &root.join("stray.gltf"),
        serde_json::to_string_pretty(&json).unwrap().as_bytes(),
    );

    let read = read_raw(&path).expect("intake reads indices without a range check");
    assert_eq!(read.triangles, vec![[0, 1, 3]]);
    assert_eq!(
        read.into_fit_input().unwrap_err(),
        HandoffError::TriangleVertexOutOfRange {
            triangle: 0,
            vertex: 3,
        }
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn open_boundary_and_non_manifold_intake_still_reaches_the_fit_path() {
    let root = temp_root("defects");
    // A fin: three triangles over the shared edge 0-1 (non-manifold) with every
    // other edge a boundary edge (open). Conform binds vertices, so neither is a
    // fit refusal; they stay with the intake defect report.
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.5, 0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.0, 0.5],
    ];
    let triangles = vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]];
    let fin = MeshAsset {
        name: "fin".into(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    };
    let bytes = write_glb(&fin).expect("writes the fin GLB");
    let path = write(&root.join("fin.glb"), &bytes);

    let read = read_raw(&path).expect("intake reads the defect without repairing it");
    let fit = read
        .into_fit_input()
        .expect("an open, non-manifold garment is still fittable");
    assert_eq!(fit.positions.len(), 5);
    assert_eq!(fit.triangles, vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]]);

    let body = body_asset();
    let conform = Conform::bind(&fit.positions, &body.positions).expect("binds to the body");
    let fitted = conform
        .apply(&body.positions)
        .expect("applies the unchanged body");
    assert_eq!(
        fitted.len(),
        fit.positions.len(),
        "the fitted vertex count is unchanged"
    );
    let _ = std::fs::remove_dir_all(&root);
}
