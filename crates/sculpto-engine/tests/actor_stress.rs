//! Registry-surface stress harness test: `actor.stress` dispatch on the same
//! synthetic skinned pair as the CLI test, plus its typed error contract.

#[path = "../../sculpto-mesh/tests/common/mod.rs"]
#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

#[path = "support/actor_stress_fixture.rs"]
mod fixture;

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sculpto_engine::{CommandId, dispatch, envelope};
use serde_json::{Value, json};

fn scratch(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "sculpto-actor-stress-engine-{tag}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&root).unwrap();
    root
}

fn error_kind(value: Value) -> (String, String) {
    let error = value["error"].clone();
    (
        error["kind"].as_str().expect("error kind").to_owned(),
        error["code"].as_str().expect("error code").to_owned(),
    )
}

#[test]
fn registry_lists_and_dispatches_the_stress_command() {
    let row = sculpto_engine::catalogue()
        .iter()
        .find(|spec| spec.id == CommandId::ActorStress)
        .expect("actor.stress is registered");
    assert_eq!(row.id.as_str(), "actor.stress");
    assert!(row.implemented());
    assert!(row.input_schema().is_object());

    let root = scratch("dispatch");
    let input = root.join("actor.glb");
    fs::write(&input, fixture::fixture()).unwrap();

    let ok = envelope(dispatch(
        "actor.stress",
        &json!({
            "input": input.display().to_string(),
            "body": "Body",
            "garments": ["Shell"],
            "clip": "Walk",
            "frames": 2,
            "tolerance": 0.0021,
            "report": root.join("report.json").display().to_string(),
        }),
    ));
    assert_eq!(ok["ok"], true, "{ok}");
    let report = &ok["result"];
    assert_eq!(report["pose"]["kind"], "clip");
    assert_eq!(report["frames"].as_array().unwrap().len(), 2);
    assert_eq!(report["frames"][0]["visible_max"], json!(0.0));
    assert_eq!(report["frames"][0]["passed"], json!(true));
    assert_eq!(report["frames"][1]["body_pairs"], json!(0));
    assert_eq!(report["frames"][1]["passed"], json!(false));
    assert_eq!(report["passed"], json!(false));
    let written: Value =
        serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    assert_eq!(
        written["frames"][1]["violations"],
        report["frames"][1]["violations"]
    );

    let missing_part = envelope(dispatch(
        "actor.stress",
        &json!({
            "input": input.display().to_string(),
            "body": "Missing",
            "garments": ["Shell"],
            "rest": true,
            "tolerance": 1.0,
        }),
    ));
    let (kind, code) = error_kind(missing_part);
    assert_eq!(kind, "not_found");
    assert_eq!(code, "actor_stress_part_missing");

    let missing_clip = envelope(dispatch(
        "actor.stress",
        &json!({
            "input": input.display().to_string(),
            "body": "Body",
            "garments": ["Shell"],
            "clip": "Missing",
            "frames": 2,
            "tolerance": 1.0,
        }),
    ));
    let (kind, code) = error_kind(missing_clip);
    assert_eq!(kind, "not_found");
    assert_eq!(code, "actor_stress_clip_missing");

    let plan_defect = envelope(dispatch(
        "actor.stress",
        &json!({
            "input": input.display().to_string(),
            "body": "Body",
            "garments": [],
            "rest": true,
            "tolerance": 1.0,
        }),
    ));
    let (kind, code) = error_kind(plan_defect);
    assert_eq!(kind, "invalid_params");
    assert_eq!(code, "invalid_value");

    fs::remove_dir_all(root).unwrap();
}
