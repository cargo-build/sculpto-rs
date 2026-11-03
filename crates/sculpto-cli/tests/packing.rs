#[path = "../../sculpto-mesh/tests/common/packing.rs"]
mod fixtures;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sculpto-pack-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let (base, target) = fixtures::pair();
        fs::write(root.join("base.glb"), base.bytes()).unwrap();
        fs::write(root.join("targets.glb"), target.bytes()).unwrap();
        fs::write(
            root.join("schedule.json"),
            r#"{"time_seconds": [0, 1], "unrelated_provenance": "allowed"}"#,
        )
        .unwrap();
        Self { root }
    }

    fn run(&self, output: &Path, extra: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["actor", "pack-cloth", "--input"])
            .arg(self.root.join("base.glb"))
            .arg("--targets")
            .arg(self.root.join("targets.glb"))
            .args(["--node", "Cloth", "--clip", "Run", "--schedule"])
            .arg(self.root.join("schedule.json"))
            .arg("--out")
            .arg(output)
            .args(extra)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn packs_and_reports_encoded_schedule_without_overwriting_existing_files() {
    let fixture = Fixture::new();
    let output = fixture.root.join("result.glb");
    let report = fixture.root.join("result.json");
    let before = fs::read(fixture.root.join("base.glb")).unwrap();
    let result = fixture.run(&output, &["--report", report.to_str().unwrap()]);
    assert!(result.status.success(), "{result:?}");
    let bytes = fs::read(&output).unwrap();
    gltf::Gltf::from_slice(&bytes).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(report["targets_count"], 2);
    assert_eq!(report["time_seconds"], serde_json::json!([0., 1.]));
    assert_eq!(fs::read(fixture.root.join("base.glb")).unwrap(), before);
    assert!(!fixture.run(&output, &[]).status.success());
    assert_eq!(fs::read(&output).unwrap(), bytes);
    assert!(
        !fixture
            .run(&fixture.root.join("base.glb"), &[])
            .status
            .success()
    );
    assert_eq!(fs::read(fixture.root.join("base.glb")).unwrap(), before);
}

#[test]
fn validation_and_output_errors_leave_no_new_result() {
    let fixture = Fixture::new();
    let output = fixture.root.join("absent.glb");
    let result = fixture.run(&output, &["--node", "missing"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(!output.exists());
    [
        r#"{}"#,
        r#"{"time_seconds": [0, "bad"]}"#,
        r#"{"time_seconds": [0, 0]}"#,
        r#"{"time_seconds": [0, 2]}"#,
    ]
    .into_iter()
    .for_each(|schedule| {
        fs::write(fixture.root.join("schedule.json"), schedule).unwrap();
        assert_eq!(fixture.run(&output, &[]).status.code(), Some(1));
        assert!(!output.exists());
    });
    fs::write(
        fixture.root.join("schedule.json"),
        r#"{"time_seconds": [0, 1]}"#,
    )
    .unwrap();
    let result = fixture.run(&fixture.root.join("missing/result.glb"), &[]);
    assert_eq!(result.status.code(), Some(1));
    let existing_report = fixture.root.join("existing.json");
    fs::write(&existing_report, b"preserve me").unwrap();
    let result = fixture.run(&output, &["--report", existing_report.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(1));
    assert!(!output.exists());
    assert_eq!(fs::read(existing_report).unwrap(), b"preserve me");
    let result = fixture.run(&output, &["--report", "/dev/full"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(!output.exists());
    fs::write(fixture.root.join("targets.glb"), b"malformed").unwrap();
    assert_eq!(fixture.run(&output, &[]).status.code(), Some(1));
    assert!(!output.exists());
}

#[test]
fn usage_rejects_missing_unknown_duplicate_and_conflicting_output_arguments() {
    let fixture = Fixture::new();
    let output = fixture.root.join("never.glb");
    [
        vec!["--unknown"],
        vec!["--report"],
        vec!["--clip", "Run"],
        vec!["--report", output.to_str().unwrap()],
    ]
    .into_iter()
    .for_each(|extra| {
        assert_eq!(fixture.run(&output, &extra).status.code(), Some(2));
        assert!(!output.exists());
    });
    assert_eq!(
        Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["actor", "pack-cloth"])
            .output()
            .unwrap()
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn relocating_actor_with_relative_base_texture_fails_without_output() {
    let fixture = Fixture::new();
    let (mut base, _) = fixtures::pair();
    base.json["images"] = serde_json::json!([{"uri": "fabric.png"}]);
    fs::write(fixture.root.join("base.glb"), base.bytes()).unwrap();
    fs::write(fixture.root.join("fabric.png"), b"private source texture").unwrap();
    let output_dir = fixture.root.join("relocated");
    fs::create_dir(&output_dir).unwrap();
    let output = output_dir.join("actor.glb");
    let report = output_dir.join("report.json");
    let result = fixture.run(&output, &["--report", report.to_str().unwrap()]);
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(
        stderr.contains("base image 0") && stderr.contains("external image URI"),
        "{stderr}"
    );
    assert!(!output.exists());
    assert!(!report.exists());
    assert!(fixture.root.join("fabric.png").exists());
}
