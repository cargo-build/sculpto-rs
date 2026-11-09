//! The command surface is a contract: one envelope, documented exit codes.
//!
//! These tests run the real binary, because the contract is what a caller sees
//! on the other side of the process boundary.

use std::process::Command;

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

#[test]
fn catalogue_is_machine_readable() {
    let output = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let rows = value["commands"].as_array().expect("array");
    assert!(rows.iter().any(|row| row["id"] == "version"));
    for row in rows {
        assert!(row["id"].is_string());
        assert!(row["summary"].is_string());
        assert!(row["mutates"].is_boolean());
        assert!(row["implemented"].is_boolean());
    }
}

#[test]
fn run_wraps_a_result_in_the_envelope() {
    let output = sculpto()
        .args(["run", "--cmd", "version"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["ok"], true);
    assert!(value["result"]["version"].is_string());
}

#[test]
fn unknown_command_is_a_request_error() {
    let output = sculpto()
        .args(["run", "--cmd", "no.such"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "not_found");
    assert_eq!(value["error"]["code"], "unknown_command");
}

#[test]
fn unimplemented_command_is_a_domain_error() {
    let output = sculpto()
        .args(["run", "--cmd", "export.nif"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["error"]["kind"], "not_implemented");
}

#[test]
fn malformed_input_is_a_request_error() {
    let output = sculpto()
        .args(["run", "--cmd", "version", "--input", "{not json"])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_request");
    assert_eq!(value["error"]["code"], "invalid_json");
}

#[test]
fn dispatch_and_catalogue_agree_on_implemented() {
    let catalogue = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run");
    let listed: serde_json::Value = serde_json::from_slice(&catalogue.stdout).expect("json");
    let dispatched = sculpto()
        .args(["run", "--cmd", "commands.list"])
        .output()
        .expect("run");
    let inner: serde_json::Value = serde_json::from_slice(&dispatched.stdout).expect("json");
    assert_eq!(listed["commands"], inner["result"]["commands"]);
}

#[test]
fn domain_failures_and_request_failures_have_distinct_kinds() {
    // Request failure: a required field is absent.
    let missing = sculpto()
        .args(["run", "--cmd", "export.obj", "--input", "{}"])
        .output()
        .expect("run");
    assert_eq!(missing.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&missing.stdout).expect("json");
    assert_eq!(value["error"]["kind"], "invalid_params");
    assert_eq!(value["error"]["code"], "missing_field");

    // Domain failure: the file is absent, the request is well formed.
    let absent = sculpto()
        .args([
            "run",
            "--cmd",
            "export.obj",
            "--input",
            r#"{"nif":"/definitely/absent.nif"}"#,
        ])
        .output()
        .expect("run");
    assert_eq!(absent.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&absent.stdout).expect("json");
    assert_eq!(value["error"]["kind"], "io");
    assert_eq!(value["error"]["code"], "io_error");
}

#[test]
fn every_catalogue_row_carries_a_schema() {
    let output = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    for row in value["commands"].as_array().expect("array") {
        assert!(
            row["input_schema"].is_object(),
            "{} has no input schema",
            row["id"]
        );
    }
}

#[test]
fn dispatch_and_run_share_one_handler() {
    for id in ["bake", "outfit.fit", "export.obj", "export.pair"] {
        let output = sculpto()
            .args(["run", "--cmd", id, "--input", "{}"])
            .output()
            .expect("run");
        let from_run: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        let from_dispatch =
            sculpto_engine::envelope(sculpto_engine::dispatch(id, &serde_json::json!({})));
        assert_eq!(
            from_run, from_dispatch,
            "{id} diverges between run and dispatch"
        );
    }
}

#[test]
fn catalogue_and_dispatch_list_agree() {
    let output = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run");
    let listed: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let dispatched =
        sculpto_engine::dispatch("commands.list", &serde_json::Value::Null).expect("commands.list");
    assert_eq!(listed["commands"], dispatched["commands"]);
}
