use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sculpto_blender::error::BlenderErrorKind;
use sculpto_blender::{ClothBakeRequest, run_with_base};

const FAKE_BLENDER: &str = r#"#!/usr/bin/env python3
import hashlib
import json
import os
import sys


def digest(data):
    return hashlib.sha256(data).hexdigest()


def artifact(out, name, data):
    path = os.path.join(out, name)
    with open(path, "wb") as sink:
        sink.write(data)
    return {"path": name, "sha256": digest(data), "bytes": len(data)}


def main():
    if "--version" in sys.argv:
        print("Blender 5.2.3")
        return 0
    args = sys.argv[sys.argv.index("--") + 1 :]
    request_path = args[args.index("--request") + 1]
    result_path = args[args.index("--result") + 1]
    with open(request_path, "rb") as source:
        raw = source.read()
    request = json.loads(raw)
    out = request["output"]["directory"]
    mode = request.get("profile") or "ok"
    host = {
        "path": "fake-blender",
        "version": "5.2.3",
        "runner_path": request["runner"]["path"],
        "runner_sha256": request["runner"]["sha256"],
    }
    result = {
        "schema": "sculpto.blender.cloth_bake.result",
        "version": 2,
        "run_id": request["run_id"],
        "request_sha256": digest(raw),
        "status": "ok",
        "host": host,
        "shells": [],
        "warnings": [],
    }
    if mode == "crash":
        return 3
    if mode == "refused":
        result["status"] = "refused"
        result["error"] = {"code": "simulation_failed", "message": "stub refusal"}
    else:
        simulation = request["simulation"]
        settle = simulation["settle_frames"]
        end = simulation["frame_end"]
        stride = simulation["sample_stride"]
        sampled = list(range(settle, end + 1, stride))
        if sampled[-1] != end:
            sampled.append(end)
        rest = artifact(out, "rest_stub.obj", b"v 0 0 0\n")
        frames = []
        for frame in sampled:
            item = artifact(out, "frame_%d_stub.obj" % frame, ("v 0 0 %d\n" % frame).encode())
            frames.append(
                {
                    "frame": frame,
                    "path": item["path"],
                    "sha256": item["sha256"],
                    "bytes": item["bytes"],
                }
            )
        result["frames"] = {
            "start": 0,
            "end": end,
            "sampled": sampled,
            "fps": request["output"]["fps"],
        }
        body_frames = []
        for frame in sampled:
            item = artifact(out, "body_%d.obj" % frame, ("v 0 0 %d\n" % frame).encode())
            body_frames.append(
                {
                    "frame": frame,
                    "path": item["path"],
                    "sha256": item["sha256"],
                    "bytes": item["bytes"],
                }
            )
        result["body_frames"] = body_frames
        result["solver"] = "xpbd"
        result["scene"] = {
            "body_object": request["scene"]["body_object"],
            "armature": "Armature",
            "clip": request["scene"]["clip"],
            "frame_start": 0,
            "frame_end": end,
            "units_per_metre": request["scene"]["units_per_metre"],
        }
        result["shells"] = [
            {
                "name": "stub",
                "vertices": 1,
                "triangles": 1,
                "rest": rest,
                "frames": frames,
            }
        ]
        result["pin"] = [
            {
                "shell": "stub",
                "policy": simulation["pin"]["policy"],
                "world_z": 1.0,
                "pinned": 1,
            }
        ]
        result["timings"] = {"simulation_seconds": 0.1, "total_seconds": 0.2}
        result["report"] = artifact(out, "report.json", b"{}")
        if mode == "bad_digest":
            result["shells"][0]["frames"][0]["sha256"] = "0" * 64
    with open(result_path, "w", encoding="utf-8") as sink:
        json.dump(result, sink)
    return 0


sys.exit(main())
"#;

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-blender-run-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, bytes).expect("write");
}

fn fixture(tag: &str, mode: Option<&str>) -> (Temp, ClothBakeRequest) {
    let temp = Temp::new(tag);
    let root = temp.path();
    write(&root.join("runner.py"), b"# runner placeholder\n");
    write(&root.join("scene.blend"), b"scene");
    write(&root.join("garment.obj"), b"v 0 0 0\n");
    let blender = root.join("fake-blender");
    write(&blender, FAKE_BLENDER.as_bytes());
    let mut permissions = fs::metadata(&blender).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&blender, permissions).expect("chmod");

    let request: ClothBakeRequest = serde_json::from_value(serde_json::json!({
        "run_id": format!("stub-{tag}"),
        "blender": {"executable": blender, "version": "5.2.3"},
        "runner": {"path": root.join("runner.py")},
        "scene": {"path": root.join("scene.blend"), "body_object": "BaseShape", "clip": "Idle"},
        "garment": {"path": root.join("garment.obj")},
        "profile": mode,
        "simulation": {"settle_frames": 0, "frame_end": 10, "sample_stride": 5},
        "output": {"directory": root.join("out")},
        "timeout_seconds": 10
    }))
    .expect("request");
    (temp, request)
}

#[test]
fn stub_host_produces_an_accepted_run() {
    let (temp, request) = fixture("ok", None);
    let outcome = run_with_base(request, temp.path()).expect("run");
    assert!(outcome.accepted());
    assert_eq!(
        outcome.result.frames.expect("frames").sampled,
        vec![0, 5, 10]
    );
    assert_eq!(outcome.result.shells.len(), 1);
    assert!(outcome.request_path.is_file());
    assert!(outcome.result_path.is_file());
    assert!(outcome.stdout_path.is_file());
}

#[test]
fn stub_refusal_is_a_valid_result() {
    let (temp, request) = fixture("refused", Some("refused"));
    let outcome = run_with_base(request, temp.path()).expect("run");
    assert!(!outcome.accepted());
    assert_eq!(
        outcome.result.error.as_ref().expect("error").code,
        "simulation_failed"
    );
}

#[test]
fn host_crash_without_result_is_an_environment_error() {
    let (temp, request) = fixture("crash", Some("crash"));
    let error = run_with_base(request, temp.path()).expect_err("crash");
    assert_eq!(error.kind(), BlenderErrorKind::Environment);
    assert_eq!(error.code(), "missing_result");
}

#[test]
fn tampered_artifact_is_refused() {
    let (temp, request) = fixture("digest", Some("bad_digest"));
    let error = run_with_base(request, temp.path()).expect_err("digest");
    assert_eq!(error.kind(), BlenderErrorKind::Refused);
    assert_eq!(error.code(), "artifact_digest");
}
