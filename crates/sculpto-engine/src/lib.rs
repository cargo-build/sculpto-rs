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

pub mod errors;
mod handlers;
pub mod obj;
pub mod ops;

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
    /// Bake one shape at both weight endpoints and write them as NIFs.
    ExportPair,
    /// Audit a raw garment export and report its topology defect counts.
    IntakeCheck,
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
            CommandId::ExportPair => "export.pair",
            CommandId::IntakeCheck => "intake.check",
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
        Self {
            kind,
            code,
            message: message.into(),
        }
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

    fn json(&self) -> Value {
        json!({
            "kind": self.kind.as_str(),
            "code": self.code,
            "message": self.message,
        })
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CommandError {}

/// A command implementation.
pub type Handler = fn(&Value) -> Result<Value, CommandError>;

/// Produces the JSON schema derived from a command's typed parameter struct.
pub type Schema = fn() -> Value;

/// One registry row.
///
/// `stub` is the single source of the `implemented` flag: a row either points at
/// a real handler or is a stub the surfaces report as unimplemented. Nothing
/// outside this table may decide whether an id is implemented.
#[derive(Clone, Copy)]
pub struct Spec {
    /// The id this row describes.
    pub id: CommandId,
    /// One line for the reference and `tools/list`.
    pub summary: &'static str,
    /// Does the command write files?
    pub mutates: bool,
    stub: bool,
    schema: Schema,
    handler: Handler,
}

impl Spec {
    /// The command has a real handler (the inverse of the `stub` flag).
    pub fn implemented(&self) -> bool {
        !self.stub
    }

    /// The input schema derived from the typed parameter struct.
    pub fn input_schema(&self) -> Value {
        (self.schema)()
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id.as_str(),
            "summary": self.summary,
            "mutates": self.mutates,
            "implemented": self.implemented(),
            "input_schema": self.input_schema(),
        })
    }
}

const COMMANDS: &[Spec] = &[
    Spec {
        id: CommandId::Version,
        summary: "Report the engine build version.",
        mutates: false,
        stub: false,
        schema: handlers::empty_schema,
        handler: handlers::version,
    },
    Spec {
        id: CommandId::CommandsList,
        summary: "List every command with its schema and mutation class.",
        mutates: false,
        stub: false,
        schema: handlers::empty_schema,
        handler: commands_list,
    },
    Spec {
        id: CommandId::Bake,
        summary: "Apply a slider preset to a source shape and write an OBJ.",
        mutates: true,
        stub: false,
        schema: handlers::bake_schema,
        handler: handlers::bake,
    },
    Spec {
        id: CommandId::OutfitFit,
        summary: "Bind a garment to its authored body and fit it to a bake.",
        mutates: true,
        stub: false,
        schema: handlers::fit_schema,
        handler: handlers::fit,
    },
    Spec {
        id: CommandId::ExportObj,
        summary: "Read one shape from a NIF and write it as an OBJ.",
        mutates: true,
        stub: false,
        schema: handlers::export_obj_schema,
        handler: handlers::export_obj,
    },
    Spec {
        id: CommandId::ExportPair,
        summary: "Bake one shape at both weight endpoints and write the `_0`/`_1` NIF pair.",
        mutates: true,
        stub: false,
        schema: handlers::export_pair_schema,
        handler: handlers::export_pair,
    },
    Spec {
        id: CommandId::IntakeCheck,
        summary: "Audit a raw garment export (.glb/.gltf/.obj) and report its topology defect counts against the declared limits.",
        mutates: false,
        stub: false,
        schema: handlers::intake_check_schema,
        handler: handlers::intake_check,
    },
    Spec {
        id: CommandId::ExportNif,
        summary: "Export an asset as a Skyrim NIF.",
        mutates: true,
        stub: false,
        schema: handlers::export_nif_schema,
        handler: handlers::export_nif,
    },
];

/// Every registry row, in declaration order.
pub fn catalogue() -> &'static [Spec] {
    COMMANDS
}

/// Dispatch one command through the registry.
///
/// The returned value is the `result` member of a successful envelope; failures
/// are [`CommandError`] and are rendered by [`envelope`].
pub fn dispatch(name: &str, input: &Value) -> Result<Value, CommandError> {
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
    (spec.handler)(input)
}

/// Render the one envelope shape every surface uses.
pub fn envelope(result: Result<Value, CommandError>) -> Value {
    match result {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(error) => json!({ "ok": false, "error": error.json() }),
    }
}

fn commands_list(_input: &Value) -> Result<Value, CommandError> {
    let rows: Vec<Value> = COMMANDS.iter().map(Spec::json).collect();
    Ok(json!({ "commands": rows }))
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
    fn every_row_has_a_schema_and_consistent_flag() {
        for spec in COMMANDS {
            assert!(
                spec.input_schema().is_object(),
                "{} has no schema",
                spec.id.as_str()
            );
            let value = dispatch(CommandId::CommandsList.as_str(), &Value::Null).expect("list");
            let row = value["commands"]
                .as_array()
                .expect("array")
                .iter()
                .find(|row| row["id"] == spec.id.as_str())
                .expect("row");
            assert_eq!(row["implemented"], spec.implemented());
        }
    }

    #[test]
    fn dispatch_wires_the_documented_commands() {
        assert!(dispatch("version", &Value::Null).is_ok());
        for id in [
            "bake",
            "outfit.fit",
            "export.obj",
            "export.pair",
            "export.nif",
            "intake.check",
        ] {
            let error = dispatch(id, &Value::Null).expect_err("missing params are an error");
            assert_ne!(error.kind(), ErrorKind::NotImplemented, "{id} is not wired");
        }
    }

    #[test]
    fn unknown_command_is_not_found() {
        let error = dispatch("no.such", &Value::Null).expect_err("must fail");
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(error.code(), "unknown_command");
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
