//! Structural validation of a host result.
//!
//! This module proves that the runner produced the requested artifacts and
//! that every declared artifact exists with the declared identity. It does not
//! prove fit, contact or appearance; those checks belong to the command layer.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::digest::sha256_file;
use crate::error::{BlenderError, BlenderErrorKind, Result};
use crate::exec::version_matches;
use crate::request::{ClothBakeRequest, PinPolicy, Solver};
use crate::result::{Artifact, ClothBakeResult, RunStatus};

/// Validate a parsed result against the resolved request.
///
/// # Errors
/// Any schema, identity, frame-coverage or artifact mismatch is a classified
/// refusal. A refused result is valid when it carries an error code.
pub fn validate_result(
    request: &ClothBakeRequest,
    request_sha256: &str,
    result: &ClothBakeResult,
    run_dir: &Path,
) -> Result<()> {
    if result.schema != crate::result::RESULT_SCHEMA {
        return Err(invalid(
            "result_schema",
            format!(
                "result schema must be {}, got {}",
                crate::result::RESULT_SCHEMA,
                result.schema
            ),
        ));
    }
    if result.version != crate::result::RESULT_VERSION {
        return Err(invalid(
            "result_version",
            format!(
                "result version must be {}, got {}",
                crate::result::RESULT_VERSION,
                result.version
            ),
        ));
    }
    if result.run_id != request.run_id {
        return Err(invalid(
            "result_run_id",
            format!(
                "result run_id {:?} does not match request {:?}",
                result.run_id, request.run_id
            ),
        ));
    }
    if !result.request_sha256.eq_ignore_ascii_case(request_sha256) {
        return Err(invalid(
            "result_request_digest",
            format!(
                "result request digest {} does not match {}",
                result.request_sha256, request_sha256
            ),
        ));
    }
    if !result.host.version.is_empty()
        && !version_matches(&result.host.version, &request.blender.version)
    {
        return Err(invalid(
            "result_host_version",
            format!(
                "host reported {}, request requires {}",
                result.host.version, request.blender.version
            ),
        ));
    }
    if let Some(expected) = &request.runner.sha256
        && !result.host.runner_sha256.eq_ignore_ascii_case(expected)
    {
        return Err(invalid(
            "result_runner_digest",
            format!(
                "result runner digest {} does not match {}",
                result.host.runner_sha256, expected
            ),
        ));
    }

    match result.status {
        RunStatus::Refused => {
            if result
                .error
                .as_ref()
                .is_none_or(|error| error.code.trim().is_empty())
            {
                return Err(invalid(
                    "result_error",
                    "a refused result must carry a non-empty error code",
                ));
            }
            return Ok(());
        }
        RunStatus::Ok => {}
    }

    let frames = result
        .frames
        .as_ref()
        .ok_or_else(|| invalid("result_frames", "an ok result must carry frame information"))?;
    if frames.fps != request.output.fps {
        return Err(invalid(
            "result_fps",
            format!(
                "result fps {} does not match request {}",
                frames.fps, request.output.fps
            ),
        ));
    }
    if frames.end > request.simulation.frame_end {
        return Err(invalid(
            "result_frame_end",
            format!(
                "result end {} exceeds request frame_end {}",
                frames.end, request.simulation.frame_end
            ),
        ));
    }
    if frames.end < request.simulation.settle_frames {
        return Err(invalid(
            "result_frame_end",
            format!(
                "result end {} is before settle_frames {}; the clip is too short",
                frames.end, request.simulation.settle_frames
            ),
        ));
    }
    validate_samples(request, frames)?;

    if result.shells.is_empty() {
        return Err(invalid("result_shells", "an ok result must carry shells"));
    }
    let mut names = BTreeSet::new();
    for shell in &result.shells {
        if shell.name.trim().is_empty() {
            return Err(invalid("result_shell_name", "shell name must not be empty"));
        }
        if !names.insert(shell.name.clone()) {
            return Err(invalid(
                "result_shell_name",
                format!("duplicate shell name {:?}", shell.name),
            ));
        }
        if shell.vertices == 0 || shell.triangles == 0 {
            return Err(invalid(
                "result_shell_counts",
                format!("shell {:?} has zero vertices or triangles", shell.name),
            ));
        }
        check_artifact(run_dir, &shell.rest, "rest")?;
        if shell.frames.len() != frames.sampled.len() {
            return Err(invalid(
                "result_shell_frames",
                format!(
                    "shell {:?} has {} frames, expected {}",
                    shell.name,
                    shell.frames.len(),
                    frames.sampled.len()
                ),
            ));
        }
        for (position, frame) in shell.frames.iter().enumerate() {
            if frame.frame != frames.sampled[position] {
                return Err(invalid(
                    "result_shell_frames",
                    format!(
                        "shell {:?} frame {:?} does not match sample {:?}",
                        shell.name, frame.frame, frames.sampled[position]
                    ),
                ));
            }
            check_artifact(
                run_dir,
                &Artifact {
                    path: frame.path.clone(),
                    sha256: frame.sha256.clone(),
                    bytes: frame.bytes,
                },
                "frame",
            )?;
        }
    }
    let expected_solver = match request.simulation.solver {
        Solver::Xpbd => "xpbd",
        Solver::Legacy => "legacy-cloth",
    };
    if result.solver.as_deref() != Some(expected_solver) {
        return Err(invalid(
            "result_solver",
            format!(
                "result solver {:?} does not match the requested {expected_solver}",
                result.solver
            ),
        ));
    }
    let scene = result
        .scene
        .as_ref()
        .ok_or_else(|| invalid("result_scene", "an ok result must carry the scene identity"))?;
    if scene.body_object != request.scene.body_object || scene.clip != request.scene.clip {
        return Err(invalid(
            "result_scene",
            format!(
                "result scene {}/{} does not match the request {}/{}",
                scene.body_object, scene.clip, request.scene.body_object, request.scene.clip
            ),
        ));
    }
    if scene.frame_end != frames.end {
        return Err(invalid(
            "result_scene",
            format!(
                "result scene end {} does not match the sampled end {}",
                scene.frame_end, frames.end
            ),
        ));
    }
    if (scene.units_per_metre - request.scene.units_per_metre).abs() > 1e-9 {
        return Err(invalid(
            "result_scene_units",
            format!(
                "result units {} do not match the request {}",
                scene.units_per_metre, request.scene.units_per_metre
            ),
        ));
    }

    if request.output.body_frames {
        if result.body_frames.len() != frames.sampled.len() {
            return Err(invalid(
                "result_body_frames",
                format!(
                    "{} body frames, expected {}",
                    result.body_frames.len(),
                    frames.sampled.len()
                ),
            ));
        }
        for (position, frame) in result.body_frames.iter().enumerate() {
            if frame.frame != frames.sampled[position] {
                return Err(invalid(
                    "result_body_frames",
                    format!(
                        "body frame {:?} does not match sample {:?}",
                        frame.frame, frames.sampled[position]
                    ),
                ));
            }
            check_artifact(
                run_dir,
                &Artifact {
                    path: frame.path.clone(),
                    sha256: frame.sha256.clone(),
                    bytes: frame.bytes,
                },
                "body frame",
            )?;
        }
    }

    if result.pin.len() != result.shells.len() {
        return Err(invalid(
            "result_pin",
            format!(
                "{} pin records, expected one per shell ({})",
                result.pin.len(),
                result.shells.len()
            ),
        ));
    }
    for pin in &result.pin {
        if !names.contains(&pin.shell) {
            return Err(invalid(
                "result_pin",
                format!("pin record names unknown shell {:?}", pin.shell),
            ));
        }
        if pin.pinned == 0 {
            return Err(invalid(
                "result_pin",
                format!("shell {:?} has no pinned vertices", pin.shell),
            ));
        }
        if pin.policy != policy_name(request.simulation.pin.policy) {
            return Err(invalid(
                "result_pin",
                format!(
                    "pin policy {:?} does not match the requested {:?}",
                    pin.policy,
                    policy_name(request.simulation.pin.policy)
                ),
            ));
        }
    }
    if let Some(report) = &result.report {
        check_artifact(run_dir, report, "report")?;
    }
    Ok(())
}

fn policy_name(policy: PinPolicy) -> &'static str {
    match policy {
        PinPolicy::Waist => "waist",
        PinPolicy::Shoulder => "shoulder",
        PinPolicy::Neck => "neck",
        PinPolicy::ComponentTop => "component_top",
        PinPolicy::ExplicitZ => "explicit_z",
    }
}

fn validate_samples(request: &ClothBakeRequest, frames: &crate::result::FrameInfo) -> Result<()> {
    if frames.sampled.is_empty() {
        return Err(invalid(
            "result_samples",
            "sampled frames must not be empty",
        ));
    }
    if frames.sampled.len() > 100_000 {
        return Err(invalid(
            "result_samples",
            "sampled frames exceed the supported count",
        ));
    }
    if frames.sampled[0] != request.simulation.settle_frames {
        return Err(invalid(
            "result_samples",
            format!(
                "first sample {} does not equal settle_frames {}",
                frames.sampled[0], request.simulation.settle_frames
            ),
        ));
    }
    let stride = request.simulation.sample_stride;
    for window in frames.sampled.windows(2) {
        let (previous, next) = (window[0], window[1]);
        if next <= previous {
            return Err(invalid(
                "result_samples",
                "sampled frames must be strictly increasing",
            ));
        }
        let gap = next - previous;
        let is_last = next == frames.end;
        if gap > stride || (gap != stride && !is_last) {
            return Err(invalid(
                "result_samples",
                format!(
                    "sample gap {gap} between {previous} and {next} is neither the stride {stride} nor the final jump"
                ),
            ));
        }
    }
    if frames.sampled.last().copied() != Some(frames.end) {
        return Err(invalid(
            "result_samples",
            "the last sample must equal the simulated end frame",
        ));
    }
    Ok(())
}

fn check_artifact(run_dir: &Path, artifact: &Artifact, label: &str) -> Result<()> {
    let relative = relative_artifact(&artifact.path)?;
    let path = run_dir.join(&relative);
    if !path.is_file() {
        return Err(BlenderError::new(
            BlenderErrorKind::Refused,
            "artifact_missing",
            format!("{label} artifact is missing: {}", path.display()),
        ));
    }
    let canonical_root = std::fs::canonicalize(run_dir).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "artifact_root",
            format!("{}: {error}", run_dir.display()),
        )
    })?;
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "artifact_resolve",
            format!("{}: {error}", path.display()),
        )
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(invalid(
            "artifact_path",
            format!(
                "{label} artifact escapes the output directory: {}",
                artifact.path
            ),
        ));
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "artifact_metadata",
            format!("{}: {error}", path.display()),
        )
    })?;
    if metadata.len() != artifact.bytes {
        return Err(invalid(
            "artifact_bytes",
            format!(
                "{label} artifact {} is {} bytes, expected {}",
                artifact.path,
                metadata.len(),
                artifact.bytes
            ),
        ));
    }
    let actual = sha256_file(&path)?;
    if !artifact.sha256.eq_ignore_ascii_case(&actual) {
        return Err(invalid(
            "artifact_digest",
            format!(
                "{label} artifact {} digest mismatch: {} vs {}",
                artifact.path, artifact.sha256, actual
            ),
        ));
    }
    Ok(())
}

fn relative_artifact(path: &str) -> Result<PathBuf> {
    let candidate = Path::new(path);
    if path.is_empty() || candidate.is_absolute() {
        return Err(invalid(
            "artifact_path",
            format!("artifact path must be relative: {path:?}"),
        ));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(invalid(
                    "artifact_path",
                    format!("artifact path must stay below the output directory: {path:?}"),
                ));
            }
        }
    }
    Ok(candidate.to_path_buf())
}

fn invalid(code: &'static str, message: impl Into<String>) -> BlenderError {
    BlenderError::new(BlenderErrorKind::Refused, code, message)
}
