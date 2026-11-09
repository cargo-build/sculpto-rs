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

/// How far the most displaced vertex of `moved` is from the same vertex in
/// `source`.
fn max_displacement(moved: &Path, source: &[[f32; 3]]) -> f32 {
    let positions = sculpto_engine::obj::read_obj_positions(moved).expect("moved OBJ positions");
    assert_eq!(
        positions.len(),
        source.len(),
        "{}: vertex count",
        moved.display()
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

/// The Tier-1 `outfit.fit` request; only `out` differs between the surfaces.
///
/// `shape` and `body_shape` name the fixture's one shape: `garment_fit` picks
/// both from the same document (`ops.rs:565-566`), and the NIF writer emits one
/// `BSTriShape` per file (`write/se_shape.rs:34`), so a distinct authored
/// reference cannot be committed. See the coverage limit on
/// [`fit_tool_matches_the_command_surface`].
fn tier1_fit(body: &Path, out: &Path) -> Value {
    json!({
        "nif": fixture_home().join("ShapeData/Tier1/Body.nif"),
        "shape": "Tier1 Body",
        "body_shape": "Tier1 Body",
        "body": body,
        "out": out,
    })
}

/// One surface's answer to the same request.
struct FitRun {
    surface: &'static str,
    result: Value,
    out: PathBuf,
}

/// Run one `outfit.fit` request over the three transports: in-process
/// `dispatch`, `sculpto run` and one stdio tool call.
fn fit_everywhere(id: u64, body: &Path, dir: &Path, tag: &str) -> Vec<FitRun> {
    let direct_out = dir.join(format!("{tag}-direct.obj"));
    let direct = sculpto_engine::dispatch("outfit.fit", &tier1_fit(body, &direct_out))
        .expect("fit dispatches");

    let cli_out = dir.join(format!("{tag}-run.obj"));
    let cli = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args([
            "run",
            "--cmd",
            "outfit.fit",
            "--input",
            &tier1_fit(body, &cli_out).to_string(),
        ])
        .output()
        .expect("run outfit.fit");
    assert!(
        cli.status.success(),
        "the command failed: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let envelope: Value = serde_json::from_slice(&cli.stdout).expect("one JSON envelope");
    assert_eq!(envelope["ok"], true, "the command failed: {envelope}");

    let mcp_out = dir.join(format!("{tag}-tool.obj"));
    let mut session = Session::start();
    initialize(&mut session);
    let response = session.call(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "outfit.fit", "arguments": tier1_fit(body, &mcp_out) },
    }));
    session.finish();
    let tool = &response["result"];
    assert_eq!(tool["isError"], false, "the tool failed: {response}");
    let from_tool: Value =
        serde_json::from_str(tool["content"][0]["text"].as_str().expect("text content"))
            .expect("the text content is the result JSON");

    vec![
        FitRun {
            surface: "dispatch",
            result: direct,
            out: direct_out,
        },
        FitRun {
            surface: "run",
            result: envelope["result"].clone(),
            out: cli_out,
        },
        FitRun {
            surface: "tool",
            result: from_tool,
            out: mcp_out,
        },
    ]
}

/// A07: the `outfit.fit` tool, `sculpto run` and an in-process `dispatch` are
/// one command and write one OBJ.
///
/// Coverage limit, asserted instead of assumed: the garment and the authored
/// reference body must come from one NIF (`ops.rs:565-566`), and the writer
/// emits one `BSTriShape` per file (`write/se_shape.rs:34`), so no committed
/// two-shape fixture exists and the Tier-1 shape is bound to itself. The
/// authored offsets are therefore zero and this fit is a transport
/// pass-through; the authored-reference case stays host-gated
/// (`run_matches_argv.rs:267`) until a multi-shape writer exists. The
/// zero-offset assertions below fail the moment this test is pointed at a
/// two-shape fixture, so it cannot widen into a claim it does not make.
#[test]
fn fit_tool_matches_the_command_surface() {
    let home = fixture_home();
    let dir = work_dir("fit-parity");
    // The body the Tier-1 shape was authored on, exported by the product.
    let authored = dir.join("authored.obj");
    sculpto_engine::dispatch(
        "export.obj",
        &json!({
            "nif": home.join("ShapeData/Tier1/Body.nif"),
            "shape": "Tier1 Body",
            "out": &authored,
        }),
    )
    .expect("export the authored body");
    // A body it was not authored on: the fixture preset moves vertex 1 by +0.25
    // and vertex 2 by +0.5 on x at Widen=50%, so the fit has to carry a movement
    // it did not invent.
    let morphed = dir.join("morphed.obj");
    sculpto_engine::dispatch("bake", &tier1_bake(&morphed)).expect("bake the morphed body");

    let authored_runs = fit_everywhere(9, &authored, &dir, "authored");
    let morphed_runs = fit_everywhere(10, &morphed, &dir, "morphed");

    // One result object on every surface; only the output path differs.
    for run in &authored_runs[1..] {
        for field in [
            "garment",
            "reference",
            "vertices",
            "offset_mean",
            "offset_max",
            "follow",
        ] {
            assert_eq!(
                run.result[field], authored_runs[0].result[field],
                "`outfit.fit` `{field}` differs between `dispatch` and {}",
                run.surface
            );
        }
    }
    // Same output bytes on every surface: the OBJ checksum is one value.
    for run in &authored_runs[1..] {
        assert_eq!(
            read(&run.out),
            read(&authored_runs[0].out),
            "`outfit.fit` wrote different artifacts over {}",
            run.surface
        );
    }
    // The fixture's own degeneracy, pinned: the garment and the reference are
    // the same shape, so the authored offset is exactly zero and no follow
    // measurement was requested.
    assert_eq!(authored_runs[0].result["garment"], "Tier1 Body");
    assert_eq!(authored_runs[0].result["reference"], "Tier1 Body");
    assert_eq!(authored_runs[0].result["vertices"], 4);
    assert_eq!(authored_runs[0].result["offset_mean"], 0.0);
    assert_eq!(authored_runs[0].result["offset_max"], 0.0);
    assert_eq!(authored_runs[0].result["follow"], Value::Null);

    // A zero-offset fit places the garment exactly on the body it is given, so
    // the positions are the body's and the triangles are the shape's.
    let body = sculpto_engine::obj::read_obj_positions(&authored).expect("body positions");
    let fitted =
        sculpto_engine::obj::read_obj_positions(&authored_runs[0].out).expect("fitted positions");
    assert_eq!(
        fitted, body,
        "a zero-offset fit must place every garment vertex on its bound body vertex"
    );

    // Negative control: the checksum is a function of the body, so a differently
    // morphed body cannot produce the same OBJ on any surface. (The command has
    // no mask or mode parameter — `ops.rs:523-540` — so the body is its fit
    // input.)
    let authored_bytes = read(&authored_runs[0].out);
    for run in &morphed_runs {
        assert_eq!(
            read(&run.out),
            read(&morphed_runs[0].out),
            "`outfit.fit` wrote different artifacts over {} for a morphed body",
            run.surface
        );
        assert_ne!(
            read(&run.out),
            authored_bytes,
            "a differently morphed body produced the same OBJ over {}",
            run.surface
        );
    }
    let movement = max_displacement(&morphed_runs[0].out, &body);
    assert!(
        (movement - 0.5).abs() < 1e-6,
        "the fit must carry the fixture's Widen=50% movement, got {movement}"
    );
}

/// A07: one typed envelope for a refused `outfit.fit`, on every transport.
///
/// The command accepts no mask or mode (`ops.rs:523-540`), so the refusals a
/// call can reach are a name the NIF does not carry and a body whose vertex
/// count differs from the reference shape's.
#[test]
fn fit_tool_error_codes_match_the_command_surface() {
    let home = fixture_home();
    let dir = work_dir("fit-errors");
    let nif = home.join("ShapeData/Tier1/Body.nif");
    let body = dir.join("body.obj");
    sculpto_engine::dispatch(
        "export.obj",
        &json!({ "nif": &nif, "shape": "Tier1 Body", "out": &body }),
    )
    .expect("export the authored body");
    // Three vertices against the four-vertex reference.
    let triangle_body = dir.join("triangle.obj");
    std::fs::write(&triangle_body, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("write body");
    let never = dir.join("never.obj");

    let unknown_shape = json!({
        "nif": &nif, "shape": "Tier1 Skirt", "body_shape": "Tier1 Body",
        "body": &body, "out": &never,
    });
    let wrong_topology = json!({
        "nif": &nif, "shape": "Tier1 Body", "body_shape": "Tier1 Body",
        "body": &triangle_body, "out": &never,
    });
    // Request, expected code, expected kind, the `sculpto run` exit code.
    for (arguments, code, kind, exit) in [
        (unknown_shape, "not_found", "not_found", 2),
        (wrong_topology, "topology_mismatch", "refused", 1),
    ] {
        let cli = Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args([
                "run",
                "--cmd",
                "outfit.fit",
                "--input",
                &arguments.to_string(),
            ])
            .output()
            .expect("run outfit.fit");
        assert_eq!(cli.status.code(), Some(exit), "exit code for {arguments}");
        let envelope: Value = serde_json::from_slice(&cli.stdout).expect("one JSON envelope");
        assert_eq!(envelope["ok"], false, "{envelope}");
        assert_eq!(envelope["error"]["code"], code, "{envelope}");
        assert_eq!(envelope["error"]["kind"], kind, "{envelope}");

        let direct = sculpto_engine::dispatch("outfit.fit", &arguments)
            .expect_err("the same request is refused in process");
        assert_eq!(direct.code(), code, "`dispatch` code for {arguments}");
        assert_eq!(
            direct.kind().as_str(),
            kind,
            "`dispatch` kind for {arguments}"
        );

        let mut session = Session::start();
        initialize(&mut session);
        let response = session.call(&json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": { "name": "outfit.fit", "arguments": &arguments },
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
                .is_some_and(|text| text.starts_with(code)),
            "the message must keep the code: {response}"
        );
        assert!(!never.exists(), "a refused fit left a file behind");
    }
}
