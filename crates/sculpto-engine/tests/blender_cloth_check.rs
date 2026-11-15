//! `blender.cloth_check`: contact counts read back from a bake result.
//!
//! The result file is the only input, so the check must prove it reads the
//! artifacts the result records: one dirty frame is counted and one re-hashed
//! file that no longer matches is refused. @req DECL0013

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sculpto_engine::{ErrorKind, dispatch};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A body triangle in the `z = 0` plane.
const BODY: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
/// A garment triangle parallel to the body, 0.5 units above it.
const CLEAN: &str = "v 0 0 0.5\nv 1 0 0.5\nv 0 1 0.5\nf 1 2 3\n";
/// A garment triangle crossing the body plane.
const CROSSING: &str = "v -0.5 0.2 -0.2\nv 0.5 0.2 -0.2\nv 0 0.2 0.2\nf 1 2 3\n";

fn work_dir(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "sculpto-cloth-check-{tag}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn artifact(dir: &Path, name: &str) -> Value {
    let bytes = fs::read(dir.join(name)).expect("artifact");
    json!({
        "path": name,
        "sha256": sha256(&bytes),
        "bytes": bytes.len(),
    })
}

fn frame_artifact(dir: &Path, frame: u32, name: &str) -> Value {
    json!({
        "frame": frame,
        "path": name,
        "sha256": artifact(dir, name)["sha256"],
        "bytes": artifact(dir, name)["bytes"],
    })
}

/// Write the two-frame result used by the happy path and the digest case.
fn write_result(dir: &Path, with_body_frames: bool) -> PathBuf {
    fs::write(dir.join("body_0.obj"), BODY).expect("body 0");
    fs::write(dir.join("body_15.obj"), BODY).expect("body 15");
    fs::write(dir.join("frame_0.obj"), CLEAN).expect("frame 0");
    fs::write(dir.join("frame_15.obj"), CROSSING).expect("frame 15");
    fs::write(dir.join("rest.obj"), CLEAN).expect("rest");

    let mut result = json!({
        "schema": "sculpto.blender.cloth_bake.result",
        "version": 2,
        "run_id": "check-fixture",
        "request_sha256": "00",
        "status": "ok",
        "host": {
            "path": "/usr/bin/blender",
            "version": "5.2.3",
            "runner_path": "runner.py",
            "runner_sha256": "00",
        },
        "solver": "legacy-cloth",
        "scene": {
            "body_object": "body",
            "armature": "rig",
            "clip": "walk",
            "frame_start": 0,
            "frame_end": 15,
            "units_per_metre": 71.4286,
        },
        "frames": {
            "start": 0,
            "end": 15,
            "sampled": [0, 15],
            "fps": 24,
        },
        "shells": [{
            "name": "SculptoGarment0",
            "vertices": 3,
            "triangles": 1,
            "rest": artifact(dir, "rest.obj"),
            "frames": [
                frame_artifact(dir, 0, "frame_0.obj"),
                frame_artifact(dir, 15, "frame_15.obj"),
            ],
        }],
        "body_frames": if with_body_frames {
            vec![
                frame_artifact(dir, 0, "body_0.obj"),
                frame_artifact(dir, 15, "body_15.obj"),
            ]
        } else {
            Vec::new()
        },
        "pin": [],
        "warnings": [],
    });
    if !with_body_frames {
        result["body_frames"] = json!([]);
    }
    let path = dir.join("result.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&result).expect("result json"),
    )
    .expect("write result");
    path
}

#[test]
fn counts_body_contacts_per_sampled_frame() {
    let dir = work_dir("counts");
    let result = write_result(&dir, true);
    let report = dispatch("blender.cloth_check", &json!({ "result": result })).expect("check");

    let rows = report["frames"].as_array().expect("frames");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["frame"], 0);
    assert_eq!(rows[0]["body_pairs"], 0);
    assert_eq!(rows[1]["frame"], 15);
    assert_eq!(rows[1]["body_pairs"], 1);
    assert_eq!(report["summary"]["body_pairs"], 1);
    assert_eq!(report["summary"]["self_pairs"], 0);
    assert_eq!(report["summary"]["worst_body_pairs"], 1);
    assert_eq!(report["run_id"], "check-fixture");
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn refuses_an_artifact_that_no_longer_matches_the_result() {
    let dir = work_dir("digest");
    let result = write_result(&dir, true);
    let mut bytes = fs::read(dir.join("frame_15.obj")).expect("frame");
    bytes[0] ^= 0x01;
    fs::write(dir.join("frame_15.obj"), bytes).expect("rewrite frame");

    let error = dispatch("blender.cloth_check", &json!({ "result": result }))
        .expect_err("stale artifact must be refused");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "artifact_digest");
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn refuses_a_result_without_body_frames() {
    let dir = work_dir("bodies");
    let result = write_result(&dir, false);

    let error = dispatch("blender.cloth_check", &json!({ "result": result }))
        .expect_err("missing body frames must be refused");
    assert_eq!(error.kind(), ErrorKind::InvalidRequest);
    assert_eq!(error.code(), "body_frames_missing");
    fs::remove_dir_all(dir).expect("cleanup");
}
