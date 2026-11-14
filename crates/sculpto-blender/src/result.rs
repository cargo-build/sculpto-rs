//! Versioned cloth-bake result written by the Python runner.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Wire schema name of the result.
pub const RESULT_SCHEMA: &str = "sculpto.blender.cloth_bake.result";
/// Result contract version; it follows the request version.
pub const RESULT_VERSION: u32 = 2;

/// Whether the host completed the bake or refused it.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Every requested artifact was produced.
    Ok,
    /// The host refused the job; `error` names the reason.
    Refused,
}

/// A stable refusal or environment failure reported by the runner.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct RunError {
    /// Stable code, for example `clip_missing` or `simulation_failed`.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

/// The exact host that produced the result.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct HostInfo {
    /// Resolved executable path.
    pub path: String,
    /// Version reported by the executable.
    pub version: String,
    /// Runner path as resolved by the request.
    pub runner_path: String,
    /// Runner digest as resolved by the request.
    pub runner_sha256: String,
}

/// One file produced under the output directory.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// Path relative to the output directory; `..` and absolute paths are
    /// invalid.
    pub path: String,
    /// Lowercase SHA-256.
    pub sha256: String,
    /// Byte size.
    pub bytes: u64,
}

/// One recorded frame of one shell.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FrameArtifact {
    /// Scene frame number.
    pub frame: u32,
    /// Path relative to the output directory.
    pub path: String,
    /// Lowercase SHA-256.
    pub sha256: String,
    /// Byte size.
    pub bytes: u64,
}

/// One simulated garment shell.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ShellResult {
    /// Shell object name in the scene.
    pub name: String,
    /// Vertex count of the rest mesh.
    pub vertices: u64,
    /// Triangle count of the rest mesh.
    pub triangles: u64,
    /// Rest mesh.
    pub rest: Artifact,
    /// Recorded frames, one per sampled frame.
    pub frames: Vec<FrameArtifact>,
}

/// Sampled frame identity.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FrameInfo {
    /// First simulated frame.
    pub start: u32,
    /// Last simulated frame.
    pub end: u32,
    /// Frames recorded to disk, strictly increasing.
    pub sampled: Vec<u32>,
    /// Frames per second recorded in the result.
    pub fps: u32,
}

/// Wall-clock measurements from the host.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Timings {
    /// Simulation time reported by the bake.
    pub simulation_seconds: f64,
    /// Total host time.
    pub total_seconds: f64,
}

/// Scene identity as the host saw it.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SceneInfo {
    /// Body mesh object.
    pub body_object: String,
    /// Armature object.
    pub armature: String,
    /// Action name.
    pub clip: String,
    /// First simulated scene frame.
    pub frame_start: u32,
    /// Last simulated scene frame after any clamp.
    pub frame_end: u32,
    /// Scene units per metre, echoed from the request.
    pub units_per_metre: f64,
}

/// Resolved pin identity for one shell.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PinInfo {
    /// Shell object name.
    pub shell: String,
    /// Policy name from the request.
    pub policy: String,
    /// Resolved world z in scene units, when the host reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_z: Option<f64>,
    /// Pinned vertex count.
    pub pinned: u64,
}

/// The bake result. A refusal still carries the request identity and host.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
pub struct ClothBakeResult {
    /// Wire schema name.
    pub schema: String,
    /// Contract version.
    pub version: u32,
    /// Echo of the request identity.
    pub run_id: String,
    /// Digest of the canonical `request.json`.
    pub request_sha256: String,
    /// Outcome.
    pub status: RunStatus,
    /// Present when `status` is `refused`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    /// Host identity; present for every result.
    pub host: HostInfo,
    /// Solver reported by the bake (`xpbd` or `legacy-cloth`); present when ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
    /// Scene identity; present when ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneInfo>,
    /// Frame identity; present when `status` is `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<FrameInfo>,
    /// Shells; empty for a refusal.
    #[serde(default)]
    pub shells: Vec<ShellResult>,
    /// Body frames exported in the same `frame_set` pass as the garment.
    #[serde(default)]
    pub body_frames: Vec<FrameArtifact>,
    /// Resolved pin identity per shell.
    #[serde(default)]
    pub pin: Vec<PinInfo>,
    /// Timings; present when the simulation ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timings: Option<Timings>,
    /// The bake's own `report.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Artifact>,
    /// Non-fatal observations.
    #[serde(default)]
    pub warnings: Vec<String>,
}
