//! `outfit.dress`: the command's refusals and the supplied-underwear acceptance.
//!
//! The tier-2 case reads a private host fixture through
//! `SCULPTO_UNDERWEAR_FIXTURE`: a directory holding `bra.obj`, `panty.obj`, the
//! exact Bijin body `body.obj` and the grading surface `collision.obj`. It
//! quotes the numbers it measures, asserts the authored seam merge on the raw
//! garment, and grades the written garment with the same depth rule
//! `garment depth` applies at the declared tolerance.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sculpto_engine::obj::read_obj_mesh;
use sculpto_engine::{ErrorKind, dispatch};
use sculpto_mesh::check::garment_depth_report;
use sculpto_mesh::dress::{SEAM_WELD_EPSILON, seam_weld};
use serde_json::json;

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-engine-dress-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn square_patch(cells: usize, z: f32, half: f32) -> String {
    let step = 2.0 * half / cells as f32;
    let mut text = String::new();
    for row in 0..=cells {
        for column in 0..=cells {
            let x = -half + column as f32 * step;
            let y = -half + row as f32 * step;
            text.push_str(&format!("v {x} {y} {z}\n"));
        }
    }
    let stride = cells + 1;
    for row in 0..cells {
        for column in 0..cells {
            let corner = row * stride + column + 1;
            let next = corner + stride;
            text.push_str(&format!("f {corner} {} {next}\n", corner + 1));
            text.push_str(&format!("f {corner} {next} {}\n", next + 1));
        }
    }
    text
}

#[test]
fn missing_parameters_are_refused_before_any_read() {
    let error = dispatch(
        "outfit.dress",
        &json!({ "garment": "a.obj", "out": "b.obj" }),
    )
    .expect_err("a dress needs a body");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
    assert_eq!(error.code(), "missing_field");
}

#[test]
fn an_unreadable_input_is_an_io_failure() {
    let error = dispatch(
        "outfit.dress",
        &json!({ "body": "/nonexistent/body.obj", "garment": "/nonexistent/g.obj", "out": "/tmp/x.obj" }),
    )
    .expect_err("an absent body cannot be dressed");
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "obj_unreadable");
}

#[test]
fn a_malformed_parameter_is_a_parameter_refusal() {
    let error = dispatch(
        "outfit.dress",
        &json!({
            "body": "/nonexistent/body.obj",
            "garment": "/nonexistent/g.obj",
            "out": "/tmp/x.obj",
            "clearance": "thick",
        }),
    )
    .expect_err("a non-numeric clearance is invalid");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
    assert_eq!(error.code(), "invalid_value");
}

#[test]
fn a_synthetic_pair_dresses_through_the_command() {
    let directory = fixture_directory("synthetic");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, square_patch(4, 0.0, 2.0)).unwrap();
    fs::write(&garment, square_patch(2, 1.5, 0.5)).unwrap();
    let out = directory.join("dressed.obj");
    let result = dispatch(
        "outfit.dress",
        &json!({
            "body": body.display().to_string(),
            "garment": garment.display().to_string(),
            "out": out.display().to_string(),
            "clearance": 0.3,
        }),
    )
    .expect("a flat pair dresses");
    assert_eq!(result["source_vertices"], 9);
    assert_eq!(result["vertices"], 27, "the default one-cut subdivision");
    assert_eq!(result["triangles"], 32);
    assert_eq!(result["max_floating_gap"], 0.0);
    assert_eq!(result["max_penetration"], 0.0);
    assert_eq!(result["converged"], true);
    assert!(out.is_file(), "the dressed OBJ is written");
}

/// One supplied garment's measured seam and dress expectations.
///
/// The numbers are the measured run at this revision, not predictions: the
/// merge counts come from `seam_weld` on the raw OBJ, the dress counts from
/// `outfit.dress` with the parameters the acceptance uses.
struct Underwear {
    name: &'static str,
    source: (usize, usize),
    dressed: usize,
    welded: usize,
    merged: usize,
    pairs: usize,
    groups: usize,
    components: usize,
    boundary: usize,
    merged_boundary: usize,
    /// Authored darts of one piece: coincident lips that must stay separate.
    kept: &'static [(usize, usize)],
    /// One cross-piece seam that must become a single vertex.
    merged_seam: (usize, usize),
}

const UNDERWEAR: [Underwear; 2] = [
    Underwear {
        name: "bra",
        source: (500, 926),
        dressed: 1856,
        welded: 463,
        merged: 37,
        pairs: 41,
        groups: 33,
        components: 6,
        boundary: 80,
        merged_boundary: 8,
        kept: &[(52, 144), (375, 379)],
        merged_seam: (102, 154),
    },
    Underwear {
        name: "panty",
        source: (664, 1056),
        dressed: 2222,
        welded: 582,
        merged: 82,
        pairs: 90,
        groups: 76,
        components: 5,
        boundary: 264,
        merged_boundary: 112,
        kept: &[(113, 656), (114, 659), (115, 657), (652, 660), (658, 662)],
        merged_seam: (104, 90),
    },
];

/// The supplied underwear on the exact Bijin body: the acceptance this task
/// exists for.
#[test]
fn configured_underwear_dresses_onto_the_exact_bijin_body() {
    let Some(root) = std::env::var_os("SCULPTO_UNDERWEAR_FIXTURE") else {
        eprintln!(
            "SKIP configured_underwear_dresses_onto_the_exact_bijin_body: SCULPTO_UNDERWEAR_FIXTURE is unset"
        );
        return;
    };
    let root = PathBuf::from(root);
    let body = root.join("body.obj");
    let collision = root.join("collision.obj");
    for path in [&body, &collision] {
        assert!(
            path.is_file(),
            "configured fixture {} must exist",
            path.display()
        );
    }
    let (body_positions, body_triangles) =
        read_obj_mesh(&body).expect("the configured body must parse");
    assert_eq!(
        (body_positions.len(), body_triangles.len()),
        (3471, 6516),
        "the fixture is the exact Bijin CBBE base body"
    );
    let (collision_positions, collision_triangles) =
        read_obj_mesh(&collision).expect("the configured collision surface must parse");
    assert_eq!(
        (collision_positions.len(), collision_triangles.len()),
        (19717, 36286),
        "the fixture is the declared grading surface"
    );

    let output = std::env::var_os("SCULPTO_UNDERWEAR_OUT").map(PathBuf::from);
    for expected in &UNDERWEAR {
        let name = expected.name;
        let garment = root.join(format!("{name}.obj"));
        assert!(
            garment.is_file(),
            "configured fixture {} must exist",
            garment.display()
        );
        let (source_positions, source_triangles) =
            read_obj_mesh(&garment).expect("the configured garment must parse");
        assert_eq!(
            (source_positions.len(), source_triangles.len()),
            expected.source,
            "the fixture is the supplied {name}"
        );

        // The authored pieces are separate: the merge joins their coincident
        // seam copies before subdivision, which is what stops the solve from
        // tearing them apart.
        let (welded, welded_triangles, seam) =
            seam_weld(&source_positions, &source_triangles, SEAM_WELD_EPSILON);
        assert_eq!(
            welded.len(),
            expected.welded,
            "{name} merges its seam copies"
        );
        assert_eq!(seam.merged_vertices, expected.merged);
        assert_eq!(seam.merged_pairs, expected.pairs, "{name} coincident pairs");
        assert_eq!(seam.groups.len(), expected.groups, "{name} merged groups");
        assert_eq!(seam.components_before, expected.components);
        assert_eq!(
            seam.components_after, 1,
            "{name} is one closed piece after the merge"
        );
        assert_eq!(seam.boundary_edges_before, expected.boundary);
        assert_eq!(seam.boundary_edges_after, expected.merged_boundary);
        assert_eq!(seam.dropped_faces, 0, "no face collapsed");
        assert_eq!(seam.vertex_map.len(), expected.source.0);
        assert_eq!(welded_triangles.len(), expected.source.1);
        assert!(
            welded_triangles
                .iter()
                .all(|face| face[0] != face[1] && face[1] != face[2] && face[0] != face[2]),
            "{name} keeps no degenerate face"
        );
        assert_eq!(
            seam_weld(&source_positions, &source_triangles, SEAM_WELD_EPSILON).2,
            seam,
            "{name} merges the same way twice"
        );
        // An authored dart is a coincidence inside one piece: it stays open,
        // while a cross-piece seam becomes one vertex.
        for &(a, b) in expected.kept {
            assert_ne!(
                seam.vertex_map[a], seam.vertex_map[b],
                "{name} source {a}={b} is an authored opening, not a seam"
            );
        }
        let (a, b) = expected.merged_seam;
        assert_eq!(
            seam.vertex_map[a], seam.vertex_map[b],
            "{name} source {a}={b} is a cross-piece seam"
        );

        let out = output
            .clone()
            .unwrap_or_else(|| fixture_directory(&format!("real-{name}")))
            .join(format!("{name}.fitted.obj"));
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let report = dispatch(
            "outfit.dress",
            &json!({
                "body": body.display().to_string(),
                "garment": garment.display().to_string(),
                "out": out.display().to_string(),
                "clearance": 0.2,
                "contact": 0.05,
                "cuts": 1,
                "iterations": 16,
                "rounds": 2,
                "relax": 0.2,
            }),
        )
        .unwrap_or_else(|error| panic!("{name} must dress: {error}"));

        // Subdivision fixes the geometry the report describes: one cut keeps
        // every merged vertex and adds one midpoint per merged edge.
        assert_eq!(report["vertices"], expected.dressed as u64);
        assert_eq!(report["source_vertices"], expected.source.0 as u64);
        assert_eq!(report["converged"], true, "{name} converges");
        assert!(
            report["max_floating_gap"].as_f64().unwrap() <= 0.2,
            "{name} floats within the declared clearance: {}",
            report["max_floating_gap"]
        );
        assert_eq!(
            report["max_penetration"], 0.0,
            "{name} leaves no penetration"
        );
        assert_eq!(
            report["pokes_before"],
            if name == "panty" { 25 } else { 99 }
        );
        assert!(
            report["pokes_after"].as_u64().unwrap() == 0,
            "{name} must leave no body poke: {}",
            report["pokes_after"]
        );

        // The grading rule `garment depth --tolerance` applies: the written
        // garment against the declared collision surface.
        let (fitted, fitted_triangles) = read_obj_mesh(&out).expect("the dressed garment parses");
        assert_eq!(fitted.len(), expected.dressed);
        let depth = garment_depth_report(
            (&fitted, &fitted_triangles),
            (&collision_positions, &collision_triangles),
        )
        .expect("the dressed garment grades");
        let verdict = depth.visible_verdict(0.15);
        assert!(
            verdict.passed,
            "{name} must pass `garment depth --tolerance 0.15`: {:?}",
            verdict.violations
        );
        assert_eq!(depth.max, 0.0, "{name} leaves no penetrating vertex");
    }
}
