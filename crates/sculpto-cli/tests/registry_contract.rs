//! The registry contract, as a gate: one catalogue, three surfaces, one code
//! per failure.
//!
//! DoD v2 criterion 5 claims that the CLI and MCP read one registry, that no row
//! is a placeholder, and that `CommandError` codes are unique. Each claim is
//! machine-checked here instead of asserted in prose:
//!
//! - every [`sculpto_engine::catalogue`] row has a handler, a derived input
//!   schema, a catalogue row, and an `implemented` flag that agrees with what
//!   `dispatch` actually does;
//! - `sculpto commands --json`, the registry and MCP `tools/list` list the same
//!   ids with the same schemas;
//! - the literals in `sculpto-engine`'s error mappings are unique, so two domain
//!   failures cannot answer with one code.
//!
//! |DECL0007|

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sculpto_engine::{ErrorKind, catalogue, dispatch};
use serde_json::{Value, json};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

/// The ids `sculpto commands --json` prints, in output order.
fn cli_catalogue() -> Value {
    let output = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run sculpto commands --json");
    assert!(output.status.success(), "sculpto commands --json failed");
    serde_json::from_slice(&output.stdout).expect("one JSON object on stdout")
}

#[test]
fn every_row_has_a_handler_a_schema_and_an_agreeing_flag() {
    let rows = catalogue();
    assert!(!rows.is_empty(), "the registry is empty");

    let mut names: Vec<&str> = rows.iter().map(|spec| spec.id.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), rows.len(), "duplicate command id");

    let listed = dispatch("commands.list", &Value::Null).expect("commands.list");
    let listed = listed["commands"].as_array().expect("catalogue rows");

    for spec in rows {
        let id = spec.id.as_str();

        // The handler table is private, so wiring is proven behaviorally: an id
        // without a handler is `unknown_command`, and a stub is the typed
        // `not_implemented` refusal the row promises.
        match dispatch(id, &Value::Null) {
            Ok(_) => assert!(
                spec.implemented(),
                "`{id}` dispatches but is flagged unimplemented"
            ),
            Err(error) => {
                assert_ne!(
                    (error.kind(), error.code()),
                    (ErrorKind::NotFound, "unknown_command"),
                    "`{id}` has no dispatch handler"
                );
                if spec.implemented() {
                    assert_ne!(
                        error.kind(),
                        ErrorKind::NotImplemented,
                        "`{id}` is flagged implemented but answers `not_implemented`"
                    );
                } else {
                    assert_eq!(
                        error.kind(),
                        ErrorKind::NotImplemented,
                        "`{id}` is flagged unimplemented but fails differently"
                    );
                    assert_eq!(
                        error.code(),
                        "not_implemented",
                        "`{id}` is flagged unimplemented without the typed code"
                    );
                }
            }
        }

        // A derived schema carries the `schemars` title; the placeholder
        // `{ "type": "object" }` does not. An unimplemented row may keep the
        // placeholder until its parameters are decided; an implemented row may
        // not, which is the gate that fails when a handler lands without a
        // schema.
        let schema = spec.input_schema();
        assert!(schema.is_object(), "`{id}` has no input schema object");
        if spec.implemented() {
            assert!(
                schema.get("title").is_some(),
                "`{id}` is implemented but its input schema is a placeholder"
            );
        }

        let row = listed
            .iter()
            .find(|row| row["id"] == id)
            .unwrap_or_else(|| panic!("`{id}` has no catalogue row"));
        assert_eq!(row["summary"], spec.summary, "`{id}` summary drifts");
        assert_eq!(row["mutates"], spec.mutates, "`{id}` mutation class drifts");
        assert_eq!(
            row["implemented"],
            spec.implemented(),
            "`{id}` implemented flag drifts"
        );
        assert_eq!(
            row["input_schema"], schema,
            "`{id}` schema drifts from the registry"
        );
    }

    assert_eq!(
        listed.len(),
        rows.len(),
        "the catalogue lists rows the registry does not have"
    );
}

/// One `sculpto mcp` session, spawned once for the whole test.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl McpSession {
    fn start() -> Self {
        let mut child = sculpto()
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

    fn call(&mut self, request: &Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush");
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).expect("read response");
        assert!(read > 0, "the server closed before answering {request}");
        serde_json::from_str(&line).expect("response is JSON")
    }

    fn finish(self) {
        let Self {
            mut child,
            stdin,
            stdout,
        } = self;
        drop(stdin);
        drop(stdout);
        child.wait().expect("server exits on end of input");
    }
}

#[test]
fn the_three_surfaces_list_one_id_set() {
    let registry: Vec<String> = catalogue()
        .iter()
        .map(|spec| spec.id.as_str().to_string())
        .collect();

    let printed = cli_catalogue();
    let cli: Vec<String> = printed["commands"]
        .as_array()
        .expect("catalogue rows")
        .iter()
        .map(|row| row["id"].as_str().expect("id").to_string())
        .collect();

    let mut session = McpSession::start();
    session.call(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "sculpto-registry-gate", "version": "0" },
        },
    }));
    let listed = session.call(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
    }));
    session.finish();
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list result");
    let mcp: Vec<String> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name").to_string())
        .collect();

    let sorted = |mut ids: Vec<String>| {
        ids.sort();
        ids
    };
    assert_eq!(
        sorted(cli.clone()),
        sorted(registry.clone()),
        "`sculpto commands --json` and the registry disagree"
    );
    assert_eq!(
        sorted(mcp.clone()),
        sorted(registry.clone()),
        "MCP `tools/list` and the registry disagree"
    );

    // Names are not the whole contract: the MCP tool schema must be the
    // registry schema, or a client validates against a stale shape.
    for spec in catalogue() {
        let id = spec.id.as_str();
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == id)
            .unwrap_or_else(|| panic!("`{id}` is not an MCP tool"));
        assert_eq!(
            tool["inputSchema"],
            spec.input_schema(),
            "`{id}` schema differs between MCP and the registry"
        );
        assert_eq!(
            tool["annotations"]["readOnlyHint"], !spec.mutates,
            "`{id}` mutation class differs between MCP and the registry"
        );
    }
}

/// The production half of `sculpto-engine`'s error mappings.
fn errors_source() -> (PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives under crates/")
        .join("sculpto-engine/src/errors.rs");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    // Codes reach a caller only through the mappings; the test module builds
    // errors through those same functions, so its literals are out of scope.
    let production = match text.find("#[cfg(test)]") {
        Some(index) => text[..index].to_string(),
        None => text,
    };
    (path, production)
}

/// Every code literal that immediately follows an `ErrorKind` in the mappings,
/// with its 1-based line.
fn mapped_codes(source: &str) -> Vec<(String, usize)> {
    let mut codes = Vec::new();
    let mut rest = 0;
    while let Some(found) = source[rest..].find("ErrorKind::") {
        let after_kind = rest + found + "ErrorKind::".len();
        let tail = &source[after_kind..];
        let variant_len = tail
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(tail.len());
        let search_from = after_kind + variant_len;
        // The code is the next string literal: always the first one on the
        // argument list of a match arm or a `CommandError::new`.
        let Some(open) = source[search_from..].find('"') else {
            break;
        };
        let start = search_from + open + 1;
        let Some(close) = source[start..].find('"') else {
            break;
        };
        let end = start + close;
        let literal = &source[start..end];
        let line = source[..start].matches('\n').count() + 1;
        if is_code(literal) {
            codes.push((literal.to_string(), line));
        }
        rest = end;
    }
    codes
}

/// A stable code: snake_case, so a message literal cannot pass as one.
fn is_code(literal: &str) -> bool {
    !literal.is_empty()
        && literal
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[test]
fn every_command_error_code_is_unique() {
    let (path, source) = errors_source();
    let codes = mapped_codes(&source);
    assert!(
        codes.len() >= 15,
        "{}: the scan found {} codes, so it stopped reading the mappings",
        path.display(),
        codes.len()
    );

    let mut by_code: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (code, line) in &codes {
        by_code.entry(code.as_str()).or_default().push(*line);
    }
    let duplicated: Vec<String> = by_code
        .iter()
        .filter(|(_, lines)| lines.len() > 1)
        .map(|(code, lines)| {
            let lines: Vec<String> = lines.iter().map(usize::to_string).collect();
            format!("`{code}` at line {}", lines.join(" and "))
        })
        .collect();
    assert!(
        duplicated.is_empty(),
        "{}: one code answers two different failures: {}",
        path.display(),
        duplicated.join(", ")
    );
}
