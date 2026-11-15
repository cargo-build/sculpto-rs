//! Locate and drive the Blender executable.
//!
//! Nothing here links Blender or Python. A missing executable is a classified
//! runtime error, so the crate builds and tests on a host without Blender.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::error::{BlenderError, BlenderErrorKind, Result};

/// A resolved Blender host.
#[derive(Debug, Clone)]
pub struct Blender {
    path: PathBuf,
    version: String,
}

impl Blender {
    /// Resolve the executable and read its version.
    ///
    /// Order: explicit path, `SCULPTO_BLENDER`, `blender` on `PATH`.
    ///
    /// # Errors
    /// `blender_missing` when no executable is found; `blender_version` when it
    /// does not answer `--version`.
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        let path = match explicit {
            Some(path) => {
                if !path.is_file() {
                    return Err(BlenderError::new(
                        BlenderErrorKind::NotFound,
                        "blender_missing",
                        format!("Blender executable does not exist: {}", path.display()),
                    ));
                }
                path.to_path_buf()
            }
            None => {
                let from_env = std::env::var_os("SCULPTO_BLENDER").map(PathBuf::from);
                match from_env {
                    Some(path) if path.is_file() => path,
                    Some(path) => {
                        return Err(BlenderError::new(
                            BlenderErrorKind::NotFound,
                            "blender_missing",
                            format!("SCULPTO_BLENDER does not name a file: {}", path.display()),
                        ));
                    }
                    None => find_on_path("blender").ok_or_else(|| {
                        BlenderError::new(
                            BlenderErrorKind::NotFound,
                            "blender_missing",
                            "no Blender executable: set SCULPTO_BLENDER or put blender on PATH",
                        )
                    })?,
                }
            }
        };
        let version = read_version(&path)?;
        Ok(Self { path, version })
    }

    /// Resolved executable.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Version reported by `blender --version`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Does the host satisfy `required` (`5.2.3` exact, `5.2` prefix)?
    pub fn satisfies(&self, required: &str) -> bool {
        version_matches(&self.version, required)
    }
}

/// Run `blender --version` and return the token after `Blender`.
///
/// # Errors
/// A failed spawn, a non-zero exit or an unparsable first line is
/// `blender_version`.
pub fn read_version(path: &Path) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            BlenderError::new(
                BlenderErrorKind::Environment,
                "blender_version",
                format!("cannot run {} --version: {error}", path.display()),
            )
        })?;
    if !output.status.success() {
        return Err(BlenderError::new(
            BlenderErrorKind::Environment,
            "blender_version",
            format!("{} --version exited with {}", path.display(), output.status),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or_default();
    first
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
        .filter(|version| version.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .ok_or_else(|| {
            BlenderError::new(
                BlenderErrorKind::Environment,
                "blender_version",
                format!("unrecognized Blender version line: {first:?}"),
            )
        })
}

/// Compare dotted versions. `required` with two components accepts any patch;
/// three components are exact.
pub fn version_matches(actual: &str, required: &str) -> bool {
    let actual = numeric_parts(actual);
    let required = numeric_parts(required);
    if required.is_empty() || required.len() > 3 || actual.len() < required.len() {
        return false;
    }
    actual[..required.len()] == required[..]
}

fn numeric_parts(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map_while(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .collect()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// One finished host process.
#[derive(Debug, Clone)]
pub struct HostOutput {
    /// Captured stdout.
    pub stdout_path: PathBuf,
    /// Captured stderr.
    pub stderr_path: PathBuf,
    /// Exit code, `None` when killed by a signal.
    pub exit_code: Option<i32>,
}

/// Run the pinned Blender over one request.
///
/// # Errors
/// Spawn failures are `environment`; an exceeded timeout is `timeout`. A
/// non-zero host exit is returned in [`HostOutput`] because the runner writes a
/// refusal result before exiting.
#[allow(clippy::too_many_arguments)]
pub fn run_host(
    blender: &Blender,
    scene: &Path,
    runner: &Path,
    request: &Path,
    result: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout: Duration,
) -> Result<HostOutput> {
    let stdout = File::create(stdout_path).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "log_create",
            format!("{}: {error}", stdout_path.display()),
        )
    })?;
    let stderr = File::create(stderr_path).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "log_create",
            format!("{}: {error}", stderr_path.display()),
        )
    })?;
    let mut child = Command::new(blender.path())
        .arg("--background")
        .arg("--factory-startup")
        .arg(scene)
        .arg("--python-exit-code")
        .arg("1")
        .arg("--python")
        .arg(runner)
        .arg("--")
        .arg("--request")
        .arg(request)
        .arg("--result")
        .arg(result)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            BlenderError::new(
                BlenderErrorKind::Environment,
                "blender_spawn",
                format!("cannot start {}: {error}", blender.path().display()),
            )
        })?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(HostOutput {
                    stdout_path: stdout_path.to_path_buf(),
                    stderr_path: stderr_path.to_path_buf(),
                    exit_code: status.code(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(BlenderError::new(
                        BlenderErrorKind::Timeout,
                        "timeout",
                        format!(
                            "Blender exceeded {:.0}s; logs: {} and {}",
                            timeout.as_secs_f64(),
                            stdout_path.display(),
                            stderr_path.display()
                        ),
                    ));
                }
                sleep(Duration::from_millis(100));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(BlenderError::new(
                    BlenderErrorKind::Environment,
                    "blender_wait",
                    format!("cannot wait for Blender: {error}"),
                ));
            }
        }
    }
}

#[cfg(test)]
#[test]
#[ignore = "needs a real Blender executable; set SCULPTO_BLENDER"]
fn real_blender_reports_a_version() {
    let Some(path) = std::env::var_os("SCULPTO_BLENDER") else {
        return;
    };
    let version = read_version(std::path::Path::new(&path)).expect("version");
    assert!(!version.is_empty());
}
