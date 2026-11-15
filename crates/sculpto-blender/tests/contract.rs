use std::fs;
use std::path::{Path, PathBuf};

use sculpto_blender::error::BlenderErrorKind;
use sculpto_blender::exec::version_matches;
use sculpto_blender::{
    Artifact, ClothBakeRequest, ClothBakeResult, FileRef, FrameArtifact, FrameInfo, HostInfo,
    PinInfo, RESULT_SCHEMA, RESULT_VERSION, RunStatus, SceneInfo, ShellResult, request_schema,
};

struct Temp(PathBuf);

impl Temp {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "sculpto-blender-{tag}-{}-{}",
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

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let value = hasher.finalize();
    let mut text = String::new();
    for byte in value {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn minimal_request(root: &Path) -> ClothBakeRequest {
    let request: ClothBakeRequest = serde_json::from_value(serde_json::json!({
        "run_id": "test-run",
        "blender": {"version": "5.2.3"},
        "runner": {"path": root.join("runner.py")},
        "scene": {"path": root.join("scene.blend"), "body_object": "BaseShape", "clip": "Idle"},
        "garment": {"path": root.join("garment.obj")},
        "simulation": {"settle_frames": 0, "frame_end": 10, "sample_stride": 5},
        "output": {"directory": root.join("out")},
        "timeout_seconds": 10
    }))
    .expect("request");
    request
}

#[test]
fn schema_is_an_object() {
    let schema = request_schema();
    assert!(schema.is_object());
    assert_eq!(schema["title"], "ClothBakeRequest");
}

#[test]
fn legacy_lab_settings_roundtrip_and_keep_metre_defaults() {
    let settings: sculpto_blender::LegacySettings = serde_json::from_value(serde_json::json!({
        "gravity": -981.0,
        "air_damping": 4.0,
        "body_thickness": -1.0,
        "shear": 20.0,
        "collision_quality": 6
    }))
    .expect("lab settings");
    assert_eq!(settings.gravity, -981.0);
    assert_eq!(settings.air_damping, 4.0);
    assert_eq!(settings.body_thickness, -1.0);
    assert_eq!(settings.shear, Some(20.0));
    assert_eq!(settings.collision_quality, 6);

    let defaults: sculpto_blender::LegacySettings =
        serde_json::from_value(serde_json::json!({})).expect("defaults");
    assert_eq!(defaults.gravity, -9.81);
    assert_eq!(defaults.air_damping, 1.0);
    assert_eq!(defaults.body_thickness, 0.005);
    assert_eq!(defaults.shear, None);
    assert_eq!(defaults.collision_quality, 4);
}

#[test]
fn unknown_request_field_is_rejected() {
    let value = serde_json::json!({
        "run_id": "x",
        "blender": {"version": "5.2.3"},
        "runner": {"path": "r.py"},
        "scene": {"path": "s.blend", "body_object": "B", "clip": "C"},
        "garment": {"path": "g.obj"},
        "output": {"directory": "out"},
        "surprise": true
    });
    let bytes = serde_json::to_vec(&value).expect("json");
    let error = ClothBakeRequest::from_json(&bytes).expect_err("unknown field");
    assert_eq!(error.kind(), BlenderErrorKind::InvalidRequest);
    assert_eq!(error.code(), "request_json");
}

#[test]
fn resolve_fills_digests_and_rejects_tampering() {
    let temp = Temp::new("digest");
    write(&temp.path().join("runner.py"), b"print('runner')");
    write(&temp.path().join("scene.blend"), b"scene");
    write(&temp.path().join("garment.obj"), b"v 0 0 0\n");

    let mut request = minimal_request(temp.path());
    request.resolve_with_base(temp.path()).expect("resolve");
    assert_eq!(
        request.garment.sha256.as_deref(),
        Some(digest(b"v 0 0 0\n").as_str())
    );
    assert!(request.output.directory.is_dir());
    assert!(request.runner.path.is_absolute());

    // Tamper with the garment and resolve again. The output directory is still
    // empty, so the only failure is the digest.
    write(&temp.path().join("garment.obj"), b"tampered");
    let error = request
        .resolve_with_base(temp.path())
        .expect_err("digest mismatch");
    assert_eq!(error.kind(), BlenderErrorKind::Refused);
    assert_eq!(error.code(), "digest_mismatch");
}

#[test]
fn non_empty_output_is_refused_before_the_host() {
    let temp = Temp::new("output");
    write(&temp.path().join("runner.py"), b"r");
    write(&temp.path().join("scene.blend"), b"s");
    write(&temp.path().join("garment.obj"), b"g");
    fs::create_dir_all(temp.path().join("out")).expect("out");
    write(&temp.path().join("out/stale.txt"), b"stale");

    let mut request = minimal_request(temp.path());
    let error = request
        .resolve_with_base(temp.path())
        .expect_err("non-empty output");
    assert_eq!(error.kind(), BlenderErrorKind::Refused);
    assert_eq!(error.code(), "output_not_empty");
}

#[test]
fn scalar_rules_name_the_broken_field() {
    let temp = Temp::new("rules");
    let mut request = minimal_request(temp.path());

    request.simulation.settle_frames = 10;
    assert_eq!(request.validate().expect_err("range").code(), "frame_range");
    request.simulation.settle_frames = 0;

    request.simulation.sample_stride = 0;
    assert_eq!(
        request.validate().expect_err("stride").code(),
        "sample_stride"
    );
    request.simulation.sample_stride = 5;

    request.simulation.pin.height_fraction = 0.0;
    assert_eq!(request.validate().expect_err("pin").code(), "pin_height");
    request.simulation.pin.height_fraction = 0.85;

    request.output.fps = 0;
    assert_eq!(request.validate().expect_err("fps").code(), "fps");
    request.output.fps = 24;

    request.version = 1;
    assert_eq!(
        request.validate().expect_err("version").code(),
        "request_version"
    );
}

#[test]
fn solver_and_pin_rules_are_explicit() {
    let temp = Temp::new("solver-pin");
    let mut request = minimal_request(temp.path());

    request.simulation.frame_end = 100;
    request.simulation.solver = sculpto_blender::Solver::Legacy;
    request.simulation.settle_frames = 24;
    assert_eq!(
        request.validate().expect_err("legacy settle").code(),
        "legacy_settle"
    );
    request.simulation.settle_frames = 0;

    request.simulation.pin.policy = sculpto_blender::PinPolicy::Neck;
    assert_eq!(
        request.validate().expect_err("legacy neck").code(),
        "pin_policy"
    );

    request.simulation.solver = sculpto_blender::Solver::Xpbd;
    request.simulation.pin.policy = sculpto_blender::PinPolicy::ComponentTop;
    assert_eq!(
        request.validate().expect_err("xpbd component").code(),
        "pin_policy"
    );

    request.simulation.solver = sculpto_blender::Solver::Legacy;
    request.simulation.pin.policy = sculpto_blender::PinPolicy::ExplicitZ;
    assert_eq!(
        request.validate().expect_err("explicit z").code(),
        "explicit_z"
    );
    request.simulation.pin.explicit_z = Some(76.0);
    request.validate().expect("explicit z value");
}

#[test]
fn version_matching_is_exact_or_prefix() {
    assert!(version_matches("5.2.3", "5.2.3"));
    assert!(version_matches("5.2.3 LTS", "5.2"));
    assert!(version_matches("5.2.4", "5.2"));
    assert!(!version_matches("5.2.3", "5.2.4"));
    assert!(!version_matches("5.2", "5.2.3"));
    assert!(!version_matches("5.2.3", ""));
    assert!(!version_matches("5.2.3", "5.2.3.1"));
}

#[test]
fn artifacts_are_resolved_below_the_run_directory() {
    let temp = Temp::new("artifacts");
    let run = temp.path().join("run");
    fs::create_dir_all(&run).expect("run");
    let rest_bytes = b"v 0 0 0\n";
    let frame_bytes = b"v 0 0 1\n";
    write(&run.join("rest.obj"), rest_bytes);
    write(&run.join("frame_0.obj"), frame_bytes);
    write(&run.join("report.json"), b"{}");
    let rest = Artifact {
        path: "rest.obj".into(),
        sha256: digest(rest_bytes),
        bytes: rest_bytes.len() as u64,
    };
    let frame = FrameArtifact {
        frame: 0,
        path: "frame_0.obj".into(),
        sha256: digest(frame_bytes),
        bytes: frame_bytes.len() as u64,
    };
    let result = ClothBakeResult {
        schema: RESULT_SCHEMA.into(),
        version: RESULT_VERSION,
        run_id: "test-run".into(),
        request_sha256: "abc".into(),
        status: RunStatus::Ok,
        error: None,
        host: HostInfo {
            path: "blender".into(),
            version: "5.2.3".into(),
            runner_path: "runner.py".into(),
            runner_sha256: "def".into(),
        },
        solver: Some("xpbd".into()),
        scene: Some(SceneInfo {
            body_object: "BaseShape".into(),
            armature: "Armature".into(),
            clip: "Idle".into(),
            frame_start: 0,
            frame_end: 0,
            units_per_metre: 1.0,
        }),
        frames: Some(FrameInfo {
            start: 0,
            end: 0,
            sampled: vec![0],
            fps: 24,
        }),
        shells: vec![ShellResult {
            name: "shell".into(),
            vertices: 1,
            triangles: 1,
            rest: rest.clone(),
            frames: vec![frame.clone()],
        }],
        body_frames: Vec::new(),
        pin: vec![PinInfo {
            shell: "shell".into(),
            policy: "shoulder".into(),
            world_z: Some(1.0),
            pinned: 1,
        }],
        timings: None,
        report: Some(Artifact {
            path: "report.json".into(),
            sha256: digest(b"{}"),
            bytes: 2,
        }),
        warnings: Vec::new(),
    };
    let mut request = minimal_request(temp.path());
    request.simulation.settle_frames = 0;
    request.simulation.frame_end = 0;
    request.simulation.sample_stride = 1;
    request.output.body_frames = false;

    sculpto_blender::validate::validate_result(&request, "abc", &result, &run).expect("valid");

    let mut bad_digest = result.clone();
    bad_digest.shells[0].frames[0].sha256 = "0".repeat(64);
    let error = sculpto_blender::validate::validate_result(&request, "abc", &bad_digest, &run)
        .expect_err("digest");
    assert_eq!(error.code(), "artifact_digest");

    let mut escape = result.clone();
    escape.shells[0].frames[0].path = "../frame.obj".into();
    let error = sculpto_blender::validate::validate_result(&request, "abc", &escape, &run)
        .expect_err("path");
    assert_eq!(error.code(), "artifact_path");

    let mut short = result;
    short.shells[0].frames.clear();
    let error = sculpto_blender::validate::validate_result(&request, "abc", &short, &run)
        .expect_err("frame count");
    assert_eq!(error.code(), "result_shell_frames");
}

#[test]
fn file_ref_roundtrips_without_a_digest() {
    let value = serde_json::json!({"path": "a.obj"});
    let reference: FileRef = serde_json::from_value(value).expect("file ref");
    assert_eq!(reference.path, PathBuf::from("a.obj"));
    assert!(reference.sha256.is_none());
}
