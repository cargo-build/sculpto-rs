//! A01 acceptance: the command surface and the argv surface must produce the
//! same bytes.
//!
//! The test needs the operator's private BodySlide home and is therefore
//! env-gated, like the native parity tests. Without `SCULPTO_BODYSLIDE_HOME` it
//! reports the skip and passes; with it, three commands are run twice and the
//! artifacts are compared byte for byte.

use std::path::{Path, PathBuf};
use std::process::Command;

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from)
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn run_ok(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("spawn");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn argv_and_run_produce_identical_artifacts() {
    let Some(home) = home() else {
        eprintln!("skipped: SCULPTO_BODYSLIDE_HOME is not set");
        return;
    };
    let project = home.join("SliderSets/HIMBO.osp");
    let presets = home.join("SliderPresets/HIMBO.xml");
    let set = "HIMBO Body - Vanilla Nevernude";
    let preset = "HIMBO Mike";
    let source = home.join("ShapeData/HIMBO/HIMBO Body - Vanilla.nif");

    let dir = std::env::temp_dir().join(format!("sculpto-a01-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let argv_bake = dir.join("bake-argv.obj");
    let run_bake = dir.join("bake-run.obj");
    let argv_export = dir.join("export-argv.obj");
    let run_export = dir.join("export-run.obj");
    let argv_fit = dir.join("fit-argv.obj");
    let run_fit = dir.join("fit-run.obj");

    run_ok(sculpto().args([
        "bake",
        "--project",
        project.to_str().unwrap(),
        "--presets",
        presets.to_str().unwrap(),
        "--set",
        set,
        "--preset",
        preset,
        "--endpoint",
        "big",
        "--out",
        argv_bake.to_str().unwrap(),
    ]));
    let baked = serde_json::json!({
        "project": project, "presets": presets, "set": set, "preset": preset,
        "endpoint": "big", "out": run_bake,
    });
    run_ok(sculpto().args(["run", "--cmd", "bake", "--input", &baked.to_string()]));
    assert_eq!(
        read(&argv_bake),
        read(&run_bake),
        "bake artifacts differ between argv and command surfaces"
    );

    run_ok(sculpto().args([
        "export-obj",
        "--nif",
        source.to_str().unwrap(),
        "--shape",
        "HIMBO - Body",
        "--out",
        argv_export.to_str().unwrap(),
    ]));
    let exported = serde_json::json!({
        "nif": source, "shape": "HIMBO - Body", "out": run_export,
    });
    run_ok(sculpto().args(["run", "--cmd", "export.obj", "--input", &exported.to_string()]));
    assert_eq!(
        read(&argv_export),
        read(&run_export),
        "export.obj artifacts differ between argv and command surfaces"
    );

    let body = read(&argv_export);
    assert!(!body.is_empty());
    run_ok(sculpto().args([
        "garment",
        "fit",
        "--nif",
        source.to_str().unwrap(),
        "--shape",
        "HIMBO - Boxers",
        "--body-shape",
        "HIMBO - Body",
        "--body",
        argv_export.to_str().unwrap(),
        "--out",
        argv_fit.to_str().unwrap(),
    ]));
    let fitted = serde_json::json!({
        "nif": source, "shape": "HIMBO - Boxers", "body_shape": "HIMBO - Body",
        "body": argv_export, "out": run_fit,
    });
    run_ok(sculpto().args(["run", "--cmd", "outfit.fit", "--input", &fitted.to_string()]));
    assert_eq!(
        read(&argv_fit),
        read(&run_fit),
        "outfit.fit artifacts differ between argv and command surfaces"
    );
}
