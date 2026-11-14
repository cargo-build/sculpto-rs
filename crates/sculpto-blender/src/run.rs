//! One complete run: resolve, invoke, parse and validate.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::digest::sha256_bytes;
use crate::error::{BlenderError, BlenderErrorKind, Result};
use crate::exec::{Blender, run_host};
use crate::request::ClothBakeRequest;
use crate::result::{ClothBakeResult, RunStatus};
use crate::validate::validate_result;

/// Everything a caller needs after a completed host process.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    /// Canonical request written to the output directory.
    pub request_path: PathBuf,
    /// Result written by the runner.
    pub result_path: PathBuf,
    /// Captured stdout.
    pub stdout_path: PathBuf,
    /// Captured stderr.
    pub stderr_path: PathBuf,
    /// Parsed and structurally validated result.
    pub result: ClothBakeResult,
}

impl RunOutcome {
    /// True when the host completed the bake.
    pub fn accepted(&self) -> bool {
        self.result.status == RunStatus::Ok
    }
}

/// Resolve a request against the current directory and run it.
///
/// # Errors
/// Any classified preflight, spawn, timeout or result-validation failure.
pub fn run(request: ClothBakeRequest) -> Result<RunOutcome> {
    let base = std::env::current_dir().map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "cwd",
            format!("cannot read the current directory: {error}"),
        )
    })?;
    run_with_base(request, &base)
}

/// Resolve relative paths against `base`, then run.
///
/// # Errors
/// See [`run`].
pub fn run_with_base(mut request: ClothBakeRequest, base: &Path) -> Result<RunOutcome> {
    request.resolve_with_base(base)?;

    let blender = Blender::discover(request.blender.executable.as_deref())?;
    if !blender.satisfies(&request.blender.version) {
        return Err(BlenderError::new(
            BlenderErrorKind::Environment,
            "blender_version",
            format!(
                "{} reports {}, request requires {}",
                blender.path().display(),
                blender.version(),
                request.blender.version
            ),
        ));
    }

    let output = &request.output.directory;
    let request_path = output.join("request.json");
    let result_path = output.join("result.json");
    let stdout_path = output.join("blender.stdout.log");
    let stderr_path = output.join("blender.stderr.log");

    let canonical = serde_json::to_vec_pretty(&request).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::InvalidRequest,
            "request_serialize",
            format!("cannot serialize the resolved request: {error}"),
        )
    })?;
    std::fs::write(&request_path, &canonical).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "request_write",
            format!("{}: {error}", request_path.display()),
        )
    })?;
    let request_sha256 = sha256_bytes(&canonical);

    let host = run_host(
        &blender,
        &request.scene.path,
        &request.runner.path,
        &request_path,
        &result_path,
        &stdout_path,
        &stderr_path,
        Duration::from_secs(request.timeout_seconds),
    )?;

    let bytes = std::fs::read(&result_path).map_err(|error| {
        let exit = host
            .exit_code
            .map_or_else(|| "signal".to_string(), |code| code.to_string());
        BlenderError::new(
            BlenderErrorKind::Environment,
            "missing_result",
            format!(
                "Blender exited with {exit} and wrote no {}; logs: {} and {} ({error})",
                result_path.display(),
                stdout_path.display(),
                stderr_path.display()
            ),
        )
    })?;
    let result: ClothBakeResult = serde_json::from_slice(&bytes).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Refused,
            "result_json",
            format!(
                "{} does not match the result schema: {error}",
                result_path.display()
            ),
        )
    })?;
    validate_result(&request, &request_sha256, &result, output)?;

    if let Some(timings) = &result.timings
        && timings.total_seconds <= 0.0
    {
        return Err(BlenderError::new(
            BlenderErrorKind::Refused,
            "result_timing",
            "result timings must be positive",
        ));
    }
    Ok(RunOutcome {
        request_path,
        result_path,
        stdout_path,
        stderr_path,
        result,
    })
}
