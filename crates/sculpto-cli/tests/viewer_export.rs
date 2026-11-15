//! `sculpto viewer export` argv: one dispatch, repeatable `--layer`, exit
//! codes that match the registry error kinds.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

/// A two-triangle OBJ spanning one metre's worth of units.
fn fixture_obj(dir: &Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, "v 0 0 0\nv 71.4286 0 0\nv 0 71.4286 0\nf 1 2 3\n")
        .expect("write fixture");
    path
}

#[test]
fn the_argv_route_writes_the_glb_and_the_report() {
    let dir = std::env::temp_dir().join("sculpto-cli-viewer-export");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = fixture_obj(&dir, "body.obj");
    let bra = fixture_obj(&dir, "bra.obj");
    let out = dir.join("review.glb");
    let report = dir.join("report.json");

    let output = sculpto()
        .args([
            "viewer",
            "export",
            "--body",
            body.to_str().expect("utf-8"),
            "--layer",
            &format!("1={}", bra.to_str().expect("utf-8")),
            "--out",
            out.to_str().expect("utf-8"),
            "--report",
            report.to_str().expect("utf-8"),
        ])
        .output()
        .expect("run sculpto viewer export");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let envelope: Value = serde_json::from_slice(&output.stdout).expect("one JSON envelope");
    assert_eq!(envelope["ok"], true);
    assert_eq!(
        envelope["result"]["statement"],
        "review artifact, not a fit result"
    );
    assert_eq!(
        envelope["result"]["parts"].as_array().expect("parts").len(),
        2
    );
    assert!(out.exists(), "the GLB was written");
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&report).expect("report"))
        .expect("a JSON report");
    assert_eq!(written["parts"][1]["material"], "#2ECC71");
}

#[test]
fn a_palette_rank_outside_the_palette_is_a_usage_error() {
    let dir = std::env::temp_dir().join("sculpto-cli-viewer-refusal");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = fixture_obj(&dir, "body.obj");
    let bra = fixture_obj(&dir, "bra.obj");

    let output = sculpto()
        .args([
            "viewer",
            "export",
            "--body",
            body.to_str().expect("utf-8"),
            "--layer",
            &format!("6={}", bra.to_str().expect("utf-8")),
            "--out",
            dir.join("review.glb").to_str().expect("utf-8"),
        ])
        .output()
        .expect("run sculpto viewer export");
    assert_eq!(
        output.status.code(),
        Some(2),
        "InvalidParams exits 2 like the other usage refusals"
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("one JSON envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "viewer_palette_order");
}

#[test]
fn a_malformed_layer_value_is_rejected_before_dispatch() {
    let dir = std::env::temp_dir().join("sculpto-cli-viewer-malformed");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = fixture_obj(&dir, "body.obj");

    let output = sculpto()
        .args([
            "viewer",
            "export",
            "--body",
            body.to_str().expect("utf-8"),
            "--layer",
            "bra.obj",
            "--out",
            dir.join("review.glb").to_str().expect("utf-8"),
        ])
        .output()
        .expect("run sculpto viewer export");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("<order>=<obj>"),
        "the usage error names the layer syntax"
    );
}
