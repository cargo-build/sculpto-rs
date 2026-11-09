//! E09: the constant-clip gate runs on bytes our own actor path produces.
//!
//! The Python checker is the project's existing gate; this test proves it is not
//! vacuous by feeding it one moving clip (must pass) and one constant clip (must
//! fail) written through the same `Fixture` builder the actor tests use.

#[allow(dead_code)]
mod common;

use std::process::Command;

use common::Fixture;

/// Walk up until the wrapper with `scripts/check_glb_animation.py` is found.
///
/// The test must work from the canonical checkout and from a jj workspace, which
/// sit at different depths.
fn wrapper_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|candidate| candidate.join("scripts/check_glb_animation.py").is_file())
        .expect("wrapper root with scripts/check_glb_animation.py")
        .to_path_buf()
}

fn check(path: &std::path::Path) -> (bool, String) {
    let output = Command::new("python3")
        .arg("-B")
        .arg(wrapper_root().join("scripts/check_glb_animation.py"))
        .arg("--check")
        .arg(path)
        .output()
        .expect("python3 runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn moving_clip_passes_and_constant_clip_fails() {
    let directory = std::env::temp_dir().join(format!("sculpto-e09-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp dir");

    let mut moving = Fixture::triangle();
    moving.channel(
        0,
        "translation",
        &[0.0, 1.0],
        &[0.0, 0.0, 0.0, 1.0, 2.0, 3.0],
    );
    let moving_path = directory.join("moving.glb");
    std::fs::write(&moving_path, moving.bytes()).expect("write moving");

    let mut constant = Fixture::triangle();
    constant.channel(
        0,
        "translation",
        &[0.0, 1.0],
        &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
    );
    let constant_path = directory.join("constant.glb");
    std::fs::write(&constant_path, constant.bytes()).expect("write constant");

    let (ok, report) = check(&moving_path);
    assert!(ok, "a moving clip must pass the gate:\n{report}");
    assert!(
        report.contains("moving samplers="),
        "the gate must report sampler spread:\n{report}"
    );

    let (ok, report) = check(&constant_path);
    assert!(!ok, "a constant clip must fail the gate:\n{report}");
    assert!(
        report.contains("constant") || report.contains("CONSTANT"),
        "the failure must name the constant clip:\n{report}"
    );

    std::fs::remove_dir_all(&directory).ok();
}
