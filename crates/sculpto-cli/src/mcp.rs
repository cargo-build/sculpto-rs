//! `sculpto mcp`: newline-delimited JSON-RPC 2.0 over stdio.
//!
//! One request object per line in, one response object per line out; a
//! notification is answered with silence. The surface has no handler table:
//! `tools/list` projects [`sculpto_engine::catalogue`] and `tools/call`
//! forwards to [`sculpto_engine::dispatch`], so the MCP tools cannot drift from
//! `sculpto commands --json` or `sculpto run --cmd`.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use sculpto_engine::CommandError;
use serde_json::{Value, json};

/// The protocol revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Name reported in `serverInfo`.
const SERVER_NAME: &str = "sculpto";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serve one stdio session until end of input.
pub fn serve() -> ExitCode {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("sculpto mcp: cannot read stdin: {error}");
                return ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request),
            Err(error) => Some(rpc_error(
                Value::Null,
                PARSE_ERROR,
                format!("invalid JSON: {error}"),
            )),
        };
        let Some(response) = response else {
            continue;
        };
        if let Err(error) = writeln!(stdout, "{response}").and_then(|()| stdout.flush()) {
            eprintln!("sculpto mcp: cannot write stdout: {error}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// Answer one request, or nothing when it is a notification.
fn handle(request: &Value) -> Option<Value> {
    if !request.is_object() {
        return Some(rpc_error(
            Value::Null,
            INVALID_REQUEST,
            "a request must be a JSON object",
        ));
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(rpc_error(
            request.get("id").cloned().unwrap_or(Value::Null),
            INVALID_REQUEST,
            "`method` is required",
        ));
    };
    // JSON-RPC: a request without an id is a notification and never answers.
    let id = request.get("id").cloned()?;
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let outcome = match method {
        "initialize" => Ok(initialize()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(&params),
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method `{other}`"),
        )),
    };
    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => rpc_error(id, error.code, error.message),
    })
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
    })
}

/// One tool per registry row; the id is the tool name and the schema is the
/// row's derived input schema.
fn tools_list() -> Value {
    let tools: Vec<Value> = sculpto_engine::catalogue()
        .iter()
        .map(|spec| {
            let mut description = spec.summary.to_string();
            if !spec.implemented() {
                description.push_str(
                    " Not implemented: the command is catalogued but its handler is not wired \
                     yet; a call returns a typed `not_implemented` error.",
                );
            }
            json!({
                "name": spec.id.as_str(),
                "description": description,
                "inputSchema": spec.input_schema(),
                "annotations": { "readOnlyHint": !spec.mutates },
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn tools_call(params: &Value) -> Result<Value, RpcError> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "`tools/call` needs a tool `name`",
        ));
    };
    if sculpto_engine::catalogue()
        .iter()
        .all(|spec| spec.id.as_str() != name)
    {
        return Err(RpcError::new(
            INVALID_PARAMS,
            format!("no tool named `{name}`"),
        ));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    Ok(match sculpto_engine::dispatch(name, &arguments) {
        Ok(value) => json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&value).expect("a command result is JSON"),
            }],
            "isError": false,
        }),
        Err(error) => tool_error(&error),
    })
}

/// A command failure keeps its typed kind and code next to the human message.
fn tool_error(error: &CommandError) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": format!("{}: {}", error.code(), error.message()),
        }],
        "isError": true,
        "_meta": {
            "sculpto": { "kind": error.kind().as_str(), "code": error.code() },
        },
    })
}

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    let message = message.into();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}
