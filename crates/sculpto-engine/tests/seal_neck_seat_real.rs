//! BLD-13 computed neck seat, real half: `body seal-joints
//! --seat-by-ring-centroid` alone seats the authored head on the neck ring —
//! no manifest translate, no explicit drop.
//!
//! Env-gated on two private fixture directories, each holding `body.obj` and
//! the authoring head `head-before-0.obj` (sha256 `0da86aea8ef7c2e3…`, authored
//! Z-up and turned −90° about source X):
//!
//! - `SCULPTO_NECK_SEAT_FIXTURES` — the Bijin prepared body
//!   (`bijin0-body-feet-yup.obj`, sha256 `af75623c0c127c44…`, 5,825 v);
//! - `SCULPTO_NECK_SEAT_3BA_FIXTURES` — one gallery 3BA body
//!   (`…/3ba__Natural_and_Realistic_Bodies_-_<name>/body.obj`, 18,436 v).
//!
//! The assembly is built by `body.assemble` with no joints, so the head's
//! declared source rotation stays a product rule and nothing seats the part
//! before the seal. The asserted seam is the BLD-13 measurement: 26↔29 → 54
//! bridge triangles with both rim counts at 0, gap 0.321/0.500/0.884, and the
//! reported centroid seat (BODY-01 0050 §3) — display-Y-up (−0.0001, +3.5498,
//! +2.5868) on the five 3BA bodies, (−0.0001, +0.2619, +0.0422) on Bijin. The
//! engine reports the source Z-up frame, so those vectors read
//! (−0.0001, −2.5868, +3.5498) and (−0.0001, −0.0422, +0.2619) here.

use std::path::{Path, PathBuf};

use sculpto_engine::dispatch;
use serde_json::{Value, json};

fn fixtures(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sculpto-neck-seat-real-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("main")
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Assemble `body.obj` + the authoring head with no joints, then seal the neck
/// through the computed centroid seat. Returns the seal report.
fn seat_the_neck(root: &Path) -> Value {
    let body = root.join("body.obj");
    let head = root.join("head-before-0.obj");
    assert!(
        body.is_file() && head.is_file(),
        "fixture directory {} must hold body.obj and head-before-0.obj",
        root.display()
    );
    let dir = work_dir();
    let manifest = dir.join("neck-seat.json");
    std::fs::write(
        &manifest,
        json!({
            "version": 1,
            "parts": [
                { "path": body, "role": "body" },
                { "path": head, "role": "head", "rotate": { "axis": "x", "degrees": -90.0 } },
            ],
            "joints": [],
        })
        .to_string(),
    )
    .expect("manifest written");
    let assembled = dir.join("assembled.obj");
    let assembled_report = dir.join("assembled.json");
    let assembly = dispatch(
        "body.assemble",
        &json!({
            "manifest": manifest.display().to_string(),
            "out": assembled.display().to_string(),
            "report": assembled_report.display().to_string(),
        }),
    )
    .expect("the body and head concatenate unsealed");
    assert_eq!(
        assembly["joints"].as_array().expect("joint rows").len(),
        0,
        "an empty joint list leaves the concatenation unsealed"
    );
    dispatch(
        "body.seal_joints",
        &json!({
            "body": assembled.display().to_string(),
            "out": dir.join("sealed.obj").display().to_string(),
            "joints": [{ "part": "head", "socket": "torso" }],
            "seat_by_ring_centroid": true,
        }),
    )
    .unwrap_or_else(|error| {
        panic!(
            "the computed seat must seal the neck: {error}; shells after assembly: {}",
            assembly["parts"]
                .as_array()
                .map(|parts| parts
                    .iter()
                    .map(|part| format!("{}={}", part["role"], part["vertices"]))
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        )
    })
}

/// The measured seam every body must report, plus the reported seat.
fn assert_measured_seam(report: &Value, expected_translation: [f64; 3]) {
    let joints = report["joints"].as_array().expect("joint rows");
    assert_eq!(joints.len(), 1, "one neck joint");
    let joint = &joints[0];
    assert_eq!(joint["joint"], "head:torso");
    assert_eq!(
        joint["correspondence"], "monotone_dp",
        "the arc path is kept"
    );
    assert_eq!(joint["bridge_triangles"], 54, "26 ring + 28 arc edges");
    assert_eq!(joint["edges_consumed"], json!([26, 28]));
    assert_eq!(joint["boundary_ring_after"], 0, "torso neck ring closed");
    assert_eq!(joint["boundary_arc_after"], 0, "head collar closed");
    assert_eq!(joint["non_manifold_after"], 0);
    assert_eq!(joint["winding_conflicts"], 0);
    for (field, value) in [("gap_min", 0.321), ("gap_mean", 0.500), ("gap_max", 0.884)] {
        let measured = joint[field].as_f64().expect("gap lane");
        assert!(
            (measured - value).abs() < 1e-3,
            "{field} = {measured}, recorded {value}"
        );
    }

    let seat = &joint["seat"];
    assert!(
        (seat["scale"].as_f64().expect("scale") - 1.0).abs() < 1e-9,
        "the neck seat never scales: {seat}"
    );
    assert_eq!(
        seat["rotation"],
        json!([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
        "translation only"
    );
    assert_eq!(seat["head_drop"], 0.0, "no explicit drop was declared");
    let ring_seat = &seat["ring_seat"];
    assert!(
        ring_seat.is_object(),
        "the computed seat is reported: {seat}"
    );
    assert_eq!(ring_seat["collar_vertices"], 26, "the merged collar cycle");
    let translation: Vec<f64> = ring_seat["translation"]
        .as_array()
        .expect("translation lanes")
        .iter()
        .map(|lane| lane.as_f64().expect("lane"))
        .collect();
    for (lane, expected) in translation.iter().zip(expected_translation) {
        assert!(
            (lane - expected).abs() < 5e-3,
            "computed translation {translation:?}, recorded {expected_translation:?}"
        );
    }
    let total: Vec<f64> = seat["translation"]
        .as_array()
        .expect("total translation")
        .iter()
        .map(|lane| lane.as_f64().expect("lane"))
        .collect();
    assert_eq!(
        total, translation,
        "with no drop the total translation is the computed seat"
    );
    let socket_centre: Vec<f64> = ring_seat["socket_centre"]
        .as_array()
        .expect("socket centre")
        .iter()
        .map(|lane| lane.as_f64().expect("lane"))
        .collect();
    let collar_centre: Vec<f64> = ring_seat["collar_centre"]
        .as_array()
        .expect("collar centre")
        .iter()
        .map(|lane| lane.as_f64().expect("lane"))
        .collect();
    for lane in 0..3 {
        assert!(
            (socket_centre[lane] - collar_centre[lane] - translation[lane]).abs() < 5e-3,
            "the translation is the centroid delta: {socket_centre:?} - {collar_centre:?}"
        );
    }

    // Smoothing stays inside its declared bound and outside nothing it owns;
    // the constrained seam's dihedral is report-only (35° proxy suspended).
    let smoothing = &joint["smoothing"];
    assert_eq!(smoothing["enabled"], true);
    assert_eq!(smoothing["changed_outside_band"], 0);
    assert!(
        smoothing["max_displacement"]
            .as_f64()
            .expect("displacement")
            <= smoothing["max_shift"].as_f64().expect("bound"),
        "displacement over bound: {smoothing}"
    );
    let dihedral = smoothing["max_dihedral_after_deg"]
        .as_f64()
        .expect("dihedral");
    assert!(dihedral <= 150.0, "fold-back edge {dihedral}° over 150°");
}

#[test]
fn bijin_neck_seats_by_ring_centroid_alone() {
    let Some(root) = fixtures("SCULPTO_NECK_SEAT_FIXTURES") else {
        eprintln!(
            "SKIP bijin_neck_seats_by_ring_centroid_alone: SCULPTO_NECK_SEAT_FIXTURES is unset"
        );
        return;
    };
    let report = seat_the_neck(&root);
    // Display Y-up (−0.0001, +0.2619, +0.0422) in the engine's Z-up frame.
    assert_measured_seam(&report, [-0.0001, -0.0422, 0.2619]);
}

#[test]
fn three_ba_neck_seats_by_ring_centroid_alone() {
    let Some(root) = fixtures("SCULPTO_NECK_SEAT_3BA_FIXTURES") else {
        eprintln!(
            "SKIP three_ba_neck_seats_by_ring_centroid_alone: SCULPTO_NECK_SEAT_3BA_FIXTURES is unset"
        );
        return;
    };
    let report = seat_the_neck(&root);
    // Display Y-up (−0.0001, +3.5498, +2.5868) in the engine's Z-up frame.
    assert_measured_seam(&report, [-0.0001, -2.5868, 3.5498]);
}
