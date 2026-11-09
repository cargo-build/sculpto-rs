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
    let output = sculpto().args(["commands", "--json"]).output().expect("run");
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
    assert!(output.stdout.is_empty());
}
