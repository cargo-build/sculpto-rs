//! The MCP surface is a contract: one registry, one dispatch path, one
//! envelope. These tests drive the real binary over its stdio session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Session {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn sculpto mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Send one request line and read its one response line.
    fn call(&mut self, request: &Value) -> Value {
        self.send(request);
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "the server closed before answering {request}");
        serde_json::from_str(&line).expect("response is JSON")
    }

    /// Send one line without waiting, for notifications and malformed input.
    fn send(&mut self, request: &Value) {
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush");
    }

    fn send_raw(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write request");
        self.stdin.flush().expect("flush");
    }

    fn finish(self) {
        let Session {
            mut child,
            stdin,
            stdout,
        } = self;
        drop(stdin);
        drop(stdout);
        child.wait().expect("server exits on EOF");
    }
}

fn initialize(session: &mut Session) {
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "sculpto-tests", "version": "0" },
        },
    }));
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    session.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
}

#[test]
fn initialize_reports_protocol_and_server_info() {
    let mut session = Session::start();
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {} },
    }));
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(response["result"]["serverInfo"]["name"], "sculpto");
    assert!(
        response["result"]["serverInfo"]["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty())
    );
    assert_eq!(
        response["result"]["capabilities"]["tools"]["listChanged"],
        false
    );
    session.finish();
}

#[test]
fn tools_list_ids_equal_the_command_catalogue() {
    let catalogue = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["commands", "--json"])
        .output()
        .expect("run commands");
    assert!(catalogue.status.success());
    let catalogue: Value = serde_json::from_slice(&catalogue.stdout).expect("catalogue json");
    let expected: Vec<String> = catalogue["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .map(|row| row["id"].as_str().expect("id").to_string())
        .collect();

    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
    }));
    let tools = response["result"]["tools"].as_array().expect("tools array");
    let listed: Vec<String> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect();
    assert_eq!(listed, expected, "tools/list must project the registry");

    for tool in tools {
        assert!(
            tool["inputSchema"].is_object(),
            "{} has no input schema",
            tool["name"]
        );
        assert!(
            tool["annotations"]["readOnlyHint"].is_boolean(),
            "{} has no read-only annotation",
            tool["name"]
        );
    }
    assert!(
        listed.contains(&"export.nif".to_string()),
        "the stub stays catalogued"
    );
    session.finish();
}

#[test]
fn stub_tool_is_marked_unimplemented_in_its_description() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
    }));
    let tools = response["result"]["tools"].as_array().expect("tools array");
    let stub = tools
        .iter()
        .find(|tool| tool["name"] == "export.nif")
        .expect("export.nif row");
    let description = stub["description"].as_str().expect("description");
    assert!(
        description.contains("Not implemented"),
        "the stub description must say so: {description}"
    );
    session.finish();
}

#[test]
fn tools_call_version_returns_the_envelope_result() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "version", "arguments": {} },
    }));
    let result = &response["result"];
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().expect("text content");
    let payload: Value = serde_json::from_str(text).expect("text is the result JSON");
    assert!(payload["version"].as_str().is_some_and(|v| !v.is_empty()));

    let from_cli = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["run", "--cmd", "version"])
        .output()
        .expect("run version");
    let from_cli: Value = serde_json::from_slice(&from_cli.stdout).expect("cli json");
    assert_eq!(payload, from_cli["result"]);
    session.finish();
}

#[test]
fn tools_call_bake_with_empty_arguments_is_a_typed_error() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": { "name": "bake", "arguments": {} },
    }));
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["_meta"]["sculpto"]["kind"], "invalid_params");
    assert_eq!(result["_meta"]["sculpto"]["code"], "missing_field");
    let text = result["content"][0]["text"].as_str().expect("text content");
    assert!(text.contains("missing_field"), "typed code missing: {text}");
    session.finish();
}

#[test]
fn stub_tool_call_reports_not_implemented() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": { "name": "export.nif", "arguments": {} },
    }));
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["_meta"]["sculpto"]["kind"], "not_implemented");
    assert_eq!(result["_meta"]["sculpto"]["code"], "not_implemented");
    session.finish();
}

#[test]
fn unknown_tool_is_an_invalid_params_error() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": { "name": "no.such", "arguments": {} },
    }));
    assert_eq!(response["error"]["code"], -32602);
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("no.such"))
    );
    assert!(
        response.get("result").is_none(),
        "no tool result on refusal"
    );
    session.finish();
}

#[test]
fn unknown_method_and_malformed_line_are_protocol_errors() {
    let mut session = Session::start();
    let unknown = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "resources/list",
    }));
    assert_eq!(unknown["error"]["code"], -32601);

    session.send_raw("{not json");
    let mut line = String::new();
    assert!(session.stdout.read_line(&mut line).expect("read") > 0);
    let malformed: Value = serde_json::from_str(&line).expect("error is JSON");
    assert_eq!(malformed["error"]["code"], -32700);
    assert_eq!(malformed["id"], Value::Null);
    session.finish();
}

#[test]
fn notifications_get_no_response() {
    let mut session = Session::start();
    session.send(&json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
    }));
    // The next response must belong to this ping, so no answer was queued.
    let pong = session.call(&json!({ "jsonrpc": "2.0", "id": 8, "method": "ping" }));
    assert_eq!(pong["id"], 8);
    assert!(pong["result"].is_object());
    session.finish();
}
