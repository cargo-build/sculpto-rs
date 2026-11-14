//! `body assemble` on both surfaces: the argv wrapper and `sculpto run` must
//! produce the same OBJ bytes, the same report file and the same envelope.
//!
//! The fixture is synthetic (two open tubes and a head collar), so the case runs
//! unconditionally; the real Bijin numbers live in
//! `sculpto-engine/tests/body_assemble_real.rs` (tier 2).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sculpto_engine::{ErrorKind, dispatch};
use serde_json::{Value, json};

const SIDES: usize = 8;

fn ring(radius: f64, z: f64) -> Vec<[f64; 3]> {
    (0..SIDES)
        .map(|k| {
            let angle = k as f64 * std::f64::consts::FRAC_PI_4;
            [radius * angle.cos(), radius * angle.sin(), z]
        })
        .collect()
}

/// Torso tube (open at the ankle and the neck) plus a head collar 0.8 above the
/// neck ring, written in the OBJ display frame like every engine input.
fn parts_obj() -> String {
    let rings = [
        ring(2.0, 4.0),
        ring(2.0, 11.0),
        ring(2.0, 11.8),
        ring(1.4, 13.6),
    ];
    let mut text = String::from("o BaseShape\n");
    for ring in &rings {
        for &[x, y, z] in ring {
            text.push_str(&format!("v {x:.6} {z:.6} {:.6}\n", -y));
        }
    }
    for (bottom, top) in [(0usize, 8usize), (16, 24)] {
        for k in 0..SIDES {
            let next = (k + 1) % SIDES;
            let (b0, b1) = (bottom + k + 1, bottom + next + 1);
            let (t0, t1) = (top + k + 1, top + next + 1);
            text.push_str(&format!("f {b0} {b1} {t1}\nf {b0} {t1} {t0}\n"));
        }
    }
    for k in 1..SIDES - 1 {
        text.push_str(&format!("f {} {} {}\n", 24 + 1, 24 + k + 1, 24 + k + 2));
    }
    text
}

fn work_dir() -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sculpto-cli-body-assemble-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

fn run(command: &mut Command, what: &str) -> Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn envelope(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("one JSON envelope on stdout")
}

fn manifest(dir: &Path, body: &Path) -> PathBuf {
    let path = dir.join("manifest.json");
    std::fs::write(
        &path,
        json!({
            "version": 1,
            "parts": [{ "path": body, "role": "body", "object": "BaseShape" }],
            "joints": [],
        })
        .to_string(),
    )
    .expect("manifest written");
    path
}

#[test]
fn argv_and_run_agree_on_the_assembled_body() {
    let dir = work_dir();
    let body = dir.join("parts.obj");
    std::fs::write(&body, parts_obj()).expect("fixture written");
    let manifest = manifest(&dir, &body);
    let out = dir.join("argv.obj");
    let report = dir.join("argv.json");

    let argv = run(
        sculpto().args([
            "body",
            "assemble",
            "--manifest",
            manifest.to_str().expect("utf8"),
            "--out",
            out.to_str().expect("utf8"),
            "--report",
            report.to_str().expect("utf8"),
        ]),
        "body assemble",
    );
    let argv_result = envelope(&argv);
    assert_eq!(argv_result["ok"], true, "{argv_result}");
    let argv_bytes = std::fs::read(&out).expect("argv output");

    let run_out = dir.join("run.obj");
    let run_report = dir.join("run.json");
    let dispatched = run(
        sculpto().args([
            "run",
            "--cmd",
            "body.assemble",
            "--input",
            &json!({
                "manifest": manifest.display().to_string(),
                "out": run_out.display().to_string(),
                "report": run_report.display().to_string(),
            })
            .to_string(),
        ]),
        "run body.assemble",
    );
    let run_result = envelope(&dispatched);
    assert_eq!(run_result["ok"], true, "{run_result}");
    assert_eq!(
        std::fs::read(&run_out).expect("run output"),
        argv_bytes,
        "both surfaces write the same OBJ"
    );
    let argv_seal = &argv_result["result"]["seal"];
    let run_seal = &run_result["result"]["seal"];
    assert_eq!(argv_seal, run_seal, "both surfaces report the same seal");

    // The report file is the envelope's result, and a missing manifest is a
    // usage error on the argv surface.
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report")).expect("report");
    assert_eq!(written["parts"], argv_result["result"]["parts"]);
    let missing = sculpto()
        .args([
            "body",
            "assemble",
            "--manifest",
            "/no/such/manifest.json",
            "--out",
            out.to_str().expect("utf8"),
        ])
        .output()
        .expect("spawn");
    assert!(!missing.status.success());
    let refused = envelope(&missing);
    assert_eq!(refused["error"]["code"], "assemble_manifest_io");
    assert_eq!(refused["error"]["kind"], ErrorKind::Io.as_str());
}

#[test]
fn the_registry_lists_the_command_with_a_schema() {
    let rows = dispatch("commands.list", &Value::Null).expect("catalogue");
    let row = rows["commands"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "body.assemble")
        .expect("body.assemble is registered");
    assert_eq!(row["implemented"], true);
    assert_eq!(row["mutates"], true);
    assert!(row["input_schema"]["properties"]["manifest"].is_object());
}
