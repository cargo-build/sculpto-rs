//! The MCP surface is a contract: one registry, one dispatch path, one
//! envelope. These tests drive the real binary over its stdio session.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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

/// The committed Tier-1 fixture root; its layout is a BodySlide home.
fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tier1")
}

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sculpto-mcp-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The Tier-1 `bake` arguments; only `out` differs between the surfaces.
fn tier1_bake(out: &Path) -> Value {
    let home = fixture_home();
    json!({
        "project": home.join("SliderSets/Tier1.osp"),
        "presets": home.join("SliderPresets/Tier1.xml"),
        "set": "Tier1 Body",
        "preset": "Tier1 Default",
        "endpoint": "big",
        "shape": "Tier1 Body",
        "out": out,
    })
}

/// How far the most displaced baked vertex moved from the unbaked source.
fn max_displacement(baked: &Path, source: &[[f32; 3]]) -> f32 {
    let positions = sculpto_engine::obj::read_obj_positions(baked).expect("baked OBJ positions");
    assert_eq!(
        positions.len(),
        source.len(),
        "{}: vertex count",
        baked.display()
    );
    positions
        .iter()
        .zip(source)
        .map(|(baked, source)| {
            let [bx, by, bz] = *baked;
            let [sx, sy, sz] = *source;
            ((bx - sx).powi(2) + (by - sy).powi(2) + (bz - sz).powi(2)).sqrt()
        })
        .fold(0.0_f32, f32::max)
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
        "the tool stays catalogued"
    );
    session.finish();
}

#[test]
fn export_nif_tool_is_implemented() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
    }));
    let tools = response["result"]["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "export.nif")
        .expect("export.nif row");
    let description = tool["description"].as_str().expect("description");
    assert!(
        !description.contains("Not implemented"),
        "the wired tool must not claim a stub: {description}"
    );
    let schema = &tool["inputSchema"];
    assert_eq!(schema["required"].as_array().map(Vec::len), Some(3));
    let targets: Vec<&str> = schema["$defs"]["ExportTarget"]["oneOf"]
        .as_array()
        .expect("target variants")
        .iter()
        .map(|variant| variant["const"].as_str().expect("const target"))
        .collect();
    assert_eq!(targets, ["se", "le"], "the target must be a closed enum");
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
fn export_nif_call_with_missing_params_is_a_typed_error() {
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": { "name": "export.nif", "arguments": {} },
    }));
    assert!(
        response.get("error").is_none(),
        "no protocol error: {response}"
    );
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["_meta"]["sculpto"]["kind"], "invalid_params");
    assert_eq!(result["_meta"]["sculpto"]["code"], "missing_field");
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

/// A06: the tool, `sculpto run` and an in-process `dispatch` are one command.
///
/// They differ only in transport, so the fixture's numbers, the written
/// artifact and the displacement it encodes must all agree.
#[test]
fn bake_tool_matches_the_command_surface() {
    let home = fixture_home();
    let dir = work_dir("bake-parity");
    let direct_out = dir.join("direct.obj");
    let cli_out = dir.join("cli.obj");
    let mcp_out = dir.join("mcp.obj");

    let direct = sculpto_engine::dispatch("bake", &tier1_bake(&direct_out))
        .expect("bake dispatches without arguments it rejects");

    let cli = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args([
            "run",
            "--cmd",
            "bake",
            "--input",
            &tier1_bake(&cli_out).to_string(),
        ])
        .output()
        .expect("run bake");
    assert!(
        cli.status.success(),
        "the command failed: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let envelope: Value = serde_json::from_slice(&cli.stdout).expect("one JSON envelope");
    assert_eq!(envelope["ok"], true, "the command failed: {envelope}");
    let cli = envelope["result"].clone();

    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": { "name": "bake", "arguments": tier1_bake(&mcp_out) },
    }));
    session.finish();
    let result = &response["result"];
    assert_eq!(result["isError"], false, "the tool failed: {response}");
    let mcp: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().expect("text content"))
            .expect("the text content is the result JSON");

    // Every numeric field agrees, and the fixture's own plan is pinned: a
    // silently empty plan would make the counts trivially equal.
    for field in ["vertices", "triangles", "sliders", "unresolved", "deferred"] {
        assert_eq!(
            direct[field], cli[field],
            "`bake` `{field}` differs between `dispatch` and `run`"
        );
        assert_eq!(
            direct[field], mcp[field],
            "`bake` `{field}` differs between `dispatch` and the tool"
        );
    }
    assert_eq!(mcp["vertices"], 4);
    assert_eq!(mcp["triangles"], 2);
    assert_eq!(mcp["sliders"], 1);
    assert_eq!(mcp["unresolved"], 0);
    assert_eq!(mcp["deferred"], 0);

    // Same fixture and the same output-path semantics: the bytes are identical.
    assert_eq!(
        read(&cli_out),
        read(&mcp_out),
        "the tool and `run` wrote different artifacts"
    );
    assert_eq!(
        read(&direct_out),
        read(&mcp_out),
        "`dispatch` and the tool wrote different artifacts"
    );

    // The artifact carries the morph, at half the fixture's largest authored
    // offset (1.0) because the preset sets Widen to 50%, so an identical
    // artifact is not two copies of the source shape.
    let source_obj = dir.join("source.obj");
    sculpto_engine::dispatch(
        "export.obj",
        &json!({
            "nif": home.join("ShapeData/Tier1/Body.nif"),
            "shape": "Tier1 Body",
            "out": source_obj,
        }),
    )
    .expect("export the unbaked source");
    let source = sculpto_engine::obj::read_obj_positions(&source_obj).expect("source positions");
    let cli_displacement = max_displacement(&cli_out, &source);
    let mcp_displacement = max_displacement(&mcp_out, &source);
    assert_eq!(
        cli_displacement, mcp_displacement,
        "the surfaces displaced the shape differently"
    );
    assert!(
        (mcp_displacement - 0.5).abs() < 1e-6,
        "the fixture's Widen=50% moves the largest vertex by 0.5, got {mcp_displacement}"
    );
}

/// A malformed `bake` keeps one typed code per failure on both transports.
#[test]
fn bake_tool_error_codes_match_the_command_surface() {
    let home = fixture_home();
    let dir = work_dir("bake-errors");
    let unset_endpoint = {
        let mut arguments = tier1_bake(&dir.join("never.obj"));
        arguments["endpoint"] = json!("huge");
        arguments
    };
    // One missing required field, one value outside a closed set.
    let malformed = [
        json!({ "project": home.join("SliderSets/Tier1.osp") }),
        unset_endpoint,
    ];

    for arguments in malformed {
        let cli = Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["run", "--cmd", "bake", "--input", &arguments.to_string()])
            .output()
            .expect("run bake");
        assert_eq!(cli.status.code(), Some(2), "a parameter error exits 2");
        let envelope: Value = serde_json::from_slice(&cli.stdout).expect("one JSON envelope");
        assert_eq!(envelope["ok"], false, "{envelope}");
        let code = envelope["error"]["code"]
            .as_str()
            .expect("typed code")
            .to_string();
        let kind = envelope["error"]["kind"]
            .as_str()
            .expect("typed kind")
            .to_string();

        let mut session = Session::start();
        initialize(&mut session);
        let response = session.call(&json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": { "name": "bake", "arguments": arguments },
        }));
        session.finish();
        let result = &response["result"];
        assert_eq!(result["isError"], true, "the tool accepted {arguments}");
        assert_eq!(
            result["_meta"]["sculpto"]["code"], code,
            "the tool and `run` disagree on the code for {arguments}"
        );
        assert_eq!(
            result["_meta"]["sculpto"]["kind"], kind,
            "the tool and `run` disagree on the kind for {arguments}"
        );
        assert!(
            result["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.starts_with(&code)),
            "the message must keep the code: {response}"
        );
    }
}
