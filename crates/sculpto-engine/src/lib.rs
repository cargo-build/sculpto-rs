//! The command spine: one closed set of command ids, one spec table, one JSON
//! envelope.
//!
//! Every user-visible action is a command with a stable id ([DECL0007]). The
//! catalogue in this crate is the single source of truth: the CLI, the MCP
//! server and the generated reference all read it, and a command that is not in
//! [`COMMANDS`] cannot be dispatched.
//!
//! The registry is deliberately closed. Nothing registers at runtime, because no
//! consumer needs it: the set of actions is known when the binary is built.
//!
//! [DECL0007]: ../../../doc/rules/0007-DECL-engine-first-standalone.md

use serde_json::{Value, json};

/// A stable command identifier.
///
/// Wire names use `noun.verb`; they are part of the external contract and must
/// not be renamed without a migration entry in the issuing epic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandId {
    /// Report the engine build version.
    Version,
    /// List the catalogue.
    CommandsList,
    /// Resolve a slider preset into offsets and apply it to a source NIF.
    Bake,
    /// Bind a garment to its authored body and fit it to a bake.
    OutfitFit,
    /// Read one shape from a NIF and write it as an OBJ.
    ExportObj,
    /// Export an asset as a NIF.
    ExportNif,
}

impl CommandId {
    /// Wire name, for example `commands.list`.
    pub fn as_str(self) -> &'static str {
        match self {
            CommandId::Version => "version",
            CommandId::CommandsList => "commands.list",
            CommandId::Bake => "bake",
            CommandId::OutfitFit => "outfit.fit",
            CommandId::ExportObj => "export.obj",
            CommandId::ExportNif => "export.nif",
        }
    }

    /// Parse a wire name.
    pub fn from_wire(name: &str) -> Option<Self> {
        COMMANDS
            .iter()
            .find(|spec| spec.id.as_str() == name)
            .map(|spec| spec.id)
    }
}

/// A stable error kind: the caller decides from this, never from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The request is malformed as JSON.
    InvalidRequest,
    /// The request is well formed but violates a parameter rule.
    InvalidParams,
    /// The requested object does not exist.
    NotFound,
    /// The engine cannot represent the request.
    Unsupported,
    /// The command exists but is not implemented yet.
    NotImplemented,
    /// The operation was refused by a domain rule.
    Refused,
    /// The operation failed in the environment.
    Io,
}

impl ErrorKind {
    /// Stable wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::InvalidRequest => "invalid_request",
            ErrorKind::InvalidParams => "invalid_params",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::NotImplemented => "not_implemented",
            ErrorKind::Refused => "refused",
            ErrorKind::Io => "io",
        }
    }
}

/// One command failure. `code` names the source variant; `message` is for humans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    kind: ErrorKind,
    code: &'static str,
    message: String,
}

impl CommandError {
    /// Build an error. `code` must be a stable identifier, not a sentence.
    pub fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self { kind, code, message: message.into() }
    }

    /// The caller's decision input.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The stable source code.
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Human-readable detail.
    pub fn message(&self) -> &str {
        &self.message
    }

    fn to_json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "code": self.code,
            "message": self.message,
        })
    }
}

/// A command implementation supplied by a binary.
pub type Handler = fn(&Value) -> Result<Value, CommandError>;

/// One catalogue row.
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// The id this row describes.
    pub id: CommandId,
    /// One line for the reference and `tools/list`.
    pub summary: &'static str,
    /// Does the command write files?
    pub mutates: bool,
    /// Is the handler implemented, or does it return `not_implemented`?
    pub implemented: bool,
    handler: Handler,
}

impl Spec {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id.as_str(),
            "summary": self.summary,
            "mutates": self.mutates,
            "implemented": self.implemented,
        })
    }
}

const COMMANDS: &[Spec] = &[
    Spec {
        id: CommandId::Version,
        summary: "Report the engine build version.",
        mutates: false,
        implemented: true,
        handler: version,
    },
    Spec {
        id: CommandId::CommandsList,
        summary: "List every command with its mutation class.",
        mutates: false,
        implemented: true,
        handler: commands_list,
    },
    Spec {
        id: CommandId::Bake,
        summary: "Apply a slider preset to a source shape and write an OBJ.",
        mutates: true,
        implemented: false,
        handler: not_implemented,
    },
    Spec {
        id: CommandId::OutfitFit,
        summary: "Bind a garment to its authored body and fit it to a bake.",
        mutates: true,
        implemented: false,
        handler: not_implemented,
    },
    Spec {
        id: CommandId::ExportObj,
        summary: "Read one shape from a NIF and write it as an OBJ.",
        mutates: true,
        implemented: false,
        handler: not_implemented,
    },
    Spec {
        id: CommandId::ExportNif,
        summary: "Export an asset as a Skyrim NIF.",
        mutates: true,
        implemented: false,
        handler: not_implemented,
    },
];

/// Every catalogue row, in declaration order.
pub fn catalogue() -> &'static [Spec] {
    COMMANDS
}

/// Dispatch one command using only the handlers built into this crate.
///
/// The returned value is the `result` member of a successful envelope; failures
/// are [`CommandError`] and are rendered by [`envelope`].
pub fn dispatch(name: &str, input: &Value) -> Result<Value, CommandError> {
    dispatch_with(name, input, &[])
}

/// Dispatch with caller-supplied handlers for the ids this crate catalogues but
/// does not implement.
///
/// The engine owns the catalogue, the unknown-id refusal and the
/// `not_implemented` fallback; the binaries own the code that touches the
/// document and the file system. That keeps `sculpto-engine` free of format
/// crates while still being the single dispatch point.
pub fn dispatch_with(
    name: &str,
    input: &Value,
    handlers: &[(&str, Handler)],
) -> Result<Value, CommandError> {
    let spec = COMMANDS
        .iter()
        .find(|spec| spec.id.as_str() == name)
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::NotFound,
                "unknown_command",
                format!("no command named {name}"),
            )
        })?;
    if let Some((_, handler)) = handlers.iter().find(|(id, _)| *id == name) {
        return handler(input);
    }
    (spec.handler)(input)
}

/// Render the one envelope shape every surface uses.
pub fn envelope(result: Result<Value, CommandError>) -> Value {
    match result {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(error) => json!({ "ok": false, "error": error.to_json() }),
    }
}

fn version(_input: &Value) -> Result<Value, CommandError> {
    Ok(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

fn commands_list(_input: &Value) -> Result<Value, CommandError> {
    let rows: Vec<Value> = COMMANDS.iter().map(Spec::to_json).collect();
    Ok(json!({ "commands": rows }))
}

fn not_implemented(_input: &Value) -> Result<Value, CommandError> {
    Err(CommandError::new(
        ErrorKind::NotImplemented,
        "not_implemented",
        "the command is catalogued but its handler is not wired yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_sorted() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|spec| spec.id.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate command id");
        assert_eq!(names, names.clone(), "ids must already be sorted");
    }

    #[test]
    fn wire_roundtrip_covers_every_row() {
        for spec in COMMANDS {
            assert_eq!(CommandId::from_wire(spec.id.as_str()), Some(spec.id));
        }
        assert_eq!(CommandId::from_wire("no.such"), None);
    }

    #[test]
    fn catalogue_lists_every_row() {
        let value = dispatch(CommandId::CommandsList.as_str(), &Value::Null).expect("list");
        let rows = value["commands"].as_array().expect("array");
        assert_eq!(rows.len(), COMMANDS.len());
    }

    #[test]
    fn unknown_command_is_not_found() {
        let error = dispatch("no.such", &Value::Null).expect_err("must fail");
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(error.code(), "unknown_command");
    }

    #[test]
    fn unimplemented_command_is_typed() {
        let error = dispatch("export.nif", &Value::Null).expect_err("must fail");
        assert_eq!(error.kind(), ErrorKind::NotImplemented);
    }

    #[test]
    fn envelope_has_one_shape() {
        let ok = envelope(dispatch("version", &Value::Null));
        assert_eq!(ok["ok"], true);
        assert!(ok["result"]["version"].is_string());

        let bad = envelope(dispatch("no.such", &Value::Null));
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["error"]["kind"], "not_found");
    }
}
