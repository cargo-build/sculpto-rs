//! Versioned cloth-bake request.
//!
//! The request is the only thing the Python runner reads. It carries pinned
//! input identities, the Blender version, the host profile reference and the
//! simulation knobs; it never carries solver values invented by the engine.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::sha256_file;
use crate::error::{BlenderError, BlenderErrorKind, Result};

/// Wire schema name. Changing it is a migration.
pub const REQUEST_SCHEMA: &str = "sculpto.blender.cloth_bake";
/// Contract version.
pub const REQUEST_VERSION: u32 = 2;

fn default_schema() -> String {
    REQUEST_SCHEMA.to_string()
}

fn default_request_version() -> u32 {
    REQUEST_VERSION
}

fn default_settle_frames() -> u32 {
    24
}

fn default_frame_end() -> u32 {
    140
}

fn default_sample_stride() -> u32 {
    10
}

fn default_substeps() -> u32 {
    5
}

fn default_constraint_steps() -> u32 {
    15
}

fn default_pin_height() -> f64 {
    0.85
}

fn default_pin_band_m() -> f64 {
    0.10
}

fn default_pin_contact_m() -> f64 {
    0.02
}

fn default_units_per_metre() -> f64 {
    1.0
}

fn default_solver() -> Solver {
    Solver::Xpbd
}

fn default_frame_end_mode() -> FrameEndMode {
    FrameEndMode::Action
}

fn default_legacy_bending() -> f64 {
    20.0
}

fn default_legacy_structural() -> f64 {
    200.0
}

fn default_legacy_mass() -> f64 {
    0.006
}

fn default_legacy_self_min() -> f64 {
    0.0015
}

fn default_legacy_body_min() -> f64 {
    0.002
}

fn default_legacy_body_thickness() -> f64 {
    0.005
}

fn default_legacy_collision_quality() -> u32 {
    4
}

fn default_legacy_gravity() -> f64 {
    -9.81
}

fn default_legacy_air_damping() -> f64 {
    1.0
}

fn default_legacy_quality() -> u32 {
    10
}

fn default_true() -> bool {
    true
}

fn default_linear_damping() -> f64 {
    1.0
}

fn default_gravity() -> bool {
    true
}

fn default_collide() -> bool {
    true
}

fn default_collider_friction() -> f64 {
    0.2
}

fn default_collision_radius() -> f64 {
    0.004
}

fn default_fps() -> u32 {
    24
}

fn default_timeout_seconds() -> u64 {
    600
}

/// A file plus its optional digest.
///
/// A missing digest is computed when the request is resolved and then written
/// into the canonical `request.json`; a supplied digest is verified before the
/// host starts.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    /// Absolute after resolution; relative paths resolve against the request
    /// file's directory (or the process CWD for an inline command).
    pub path: PathBuf,
    /// Lowercase SHA-256, or `null` to compute and record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Which Blender executable and version the run requires.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct BlenderRef {
    /// Explicit executable; `null` uses `SCULPTO_BLENDER`, then `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    /// Required version. `5.2.3` is exact; `5.2` accepts any `5.2.x`.
    pub version: String,
}

/// The Blender scene and the actor the bake consumes.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SceneRef {
    /// Path to the `.blend` scene.
    pub path: PathBuf,
    /// Lowercase SHA-256, or `null` to compute and record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Body mesh object that receives the collider.
    pub body_object: String,
    /// Armature object; `null` selects the scene's only armature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armature_object: Option<String>,
    /// Action name driven over the sampled range.
    pub clip: String,
    /// Scene units per metre. The legacy bake assumes metres; the preserved
    /// November 8 walk scene is ~71.4 units/m. `1.0` means metres.
    #[serde(default = "default_units_per_metre")]
    pub units_per_metre: f64,
}

/// Solver selected for the bake.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Solver {
    /// Geometry-nodes XPBD asset; pins, colliders, no self-collision.
    Xpbd,
    /// Legacy mass-spring cloth; self-collision and bending on open flaps.
    Legacy,
}

/// How `frame_end` is resolved against the action's keyframe range.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrameEndMode {
    /// Clamp to the action's own range (the shipped scripts' default).
    Action,
    /// Honour the requested end and let cycle modifiers extend the action.
    Scene,
}

/// Where the garment is anchored. The policy names the anchor; its parameters
/// carry the numbers, and no universal height exists.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinPolicy {
    /// Waist band; XPBD auto, legacy top ring.
    Waist,
    /// Shoulder line at a body-height fraction.
    Shoulder,
    /// Neck line with optional back yoke.
    Neck,
    /// Each connected component's own top band.
    ComponentTop,
    /// Explicit world z in scene units, for reproductions.
    ExplicitZ,
}

/// Pin policy plus its own parameters.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PinSettings {
    /// Which anchor rule applies.
    pub policy: PinPolicy,
    /// Shoulder-line fraction of body height, used by [`PinPolicy::Shoulder`].
    #[serde(default = "default_pin_height")]
    pub height_fraction: f64,
    /// Pin band height in scene units, used by waist/component policies.
    #[serde(default = "default_pin_band_m")]
    pub band_m: f64,
    /// Contact tolerance in scene units, used by the neck policy.
    #[serde(default = "default_pin_contact_m")]
    pub contact_m: f64,
    /// Pin the back yoke above the neck line as well.
    #[serde(default)]
    pub back_yoke: bool,
    /// Pin every vertex above the line instead of a boundary ring (XPBD).
    #[serde(default)]
    pub pin_all: bool,
    /// World z in scene units, required by [`PinPolicy::ExplicitZ`].
    #[serde(default)]
    pub explicit_z: Option<f64>,
}

impl Default for PinSettings {
    fn default() -> Self {
        Self {
            policy: PinPolicy::Shoulder,
            height_fraction: default_pin_height(),
            band_m: default_pin_band_m(),
            contact_m: default_pin_contact_m(),
            back_yoke: false,
            pin_all: false,
            explicit_z: None,
        }
    }
}

/// Geometry-nodes XPBD settings.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct XpbdSettings {
    /// XPBD substeps.
    #[serde(default = "default_substeps")]
    pub substeps: u32,
    /// XPBD constraint iterations per substep.
    #[serde(default = "default_constraint_steps")]
    pub constraint_steps: u32,
    /// Stretchiness, passed to the XPBD asset.
    #[serde(default)]
    pub stretchiness: f64,
    /// Bendiness, passed to the XPBD asset.
    #[serde(default)]
    pub bendiness: f64,
    /// Linear damping, passed to the XPBD asset.
    #[serde(default = "default_linear_damping")]
    pub linear_damping: f64,
    /// Whether gravity is enabled.
    #[serde(default = "default_gravity")]
    pub gravity: bool,
    /// Whether body collision is enabled.
    #[serde(default = "default_collide")]
    pub collide: bool,
    /// Collider friction.
    #[serde(default = "default_collider_friction")]
    pub collider_friction: f64,
    /// Collider margin, in scene units.
    #[serde(default)]
    pub collider_margin: f64,
    /// Enable edge contacts on the collider.
    #[serde(default)]
    pub edge_contacts: bool,
    /// XPBD collision radius, in scene units.
    #[serde(default = "default_collision_radius")]
    pub collision_radius: f64,
}

impl Default for XpbdSettings {
    fn default() -> Self {
        Self {
            substeps: default_substeps(),
            constraint_steps: default_constraint_steps(),
            stretchiness: 0.0,
            bendiness: 0.0,
            linear_damping: default_linear_damping(),
            gravity: default_gravity(),
            collide: default_collide(),
            collider_friction: default_collider_friction(),
            collider_margin: 0.0,
            edge_contacts: false,
            collision_radius: default_collision_radius(),
        }
    }
}

/// Legacy mass-spring settings. Different parameters and units from XPBD;
/// never reuse the XPBD names for these.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct LegacySettings {
    /// Bending stiffness.
    #[serde(default = "default_legacy_bending")]
    pub bending: f64,
    /// Structural (tension and compression) stiffness.
    #[serde(default = "default_legacy_structural")]
    pub structural: f64,
    /// Shear stiffness; defaults to `structural`.
    #[serde(default)]
    pub shear: Option<f64>,
    /// Per-vertex mass, scaled down with vertex count.
    #[serde(default = "default_legacy_mass")]
    pub mass: f64,
    /// Enable cloth self-collision.
    #[serde(default = "default_true")]
    pub self_collision: bool,
    /// Self-collision minimum distance, scene units.
    #[serde(default = "default_legacy_self_min")]
    pub self_min: f64,
    /// Body-collision minimum distance, scene units.
    #[serde(default = "default_legacy_body_min")]
    pub body_min: f64,
    /// Body-collision thickness, scene units. A negative value means
    /// `body_min + 0.2`, the lab-harness default.
    #[serde(default = "default_legacy_body_thickness")]
    pub body_thickness: f64,
    /// Collision quality steps.
    #[serde(default = "default_legacy_collision_quality")]
    pub collision_quality: u32,
    /// Scene gravity, units/s². Lab-unit scenes need their own value.
    #[serde(default = "default_legacy_gravity")]
    pub gravity: f64,
    /// Air damping applied to cloth motion.
    #[serde(default = "default_legacy_air_damping")]
    pub air_damping: f64,
    /// Solver quality steps.
    #[serde(default = "default_legacy_quality")]
    pub quality: u32,
    /// Add a floor plane at the body's lowest point.
    #[serde(default)]
    pub ground: bool,
}

impl Default for LegacySettings {
    fn default() -> Self {
        Self {
            bending: default_legacy_bending(),
            structural: default_legacy_structural(),
            shear: None,
            mass: default_legacy_mass(),
            self_collision: default_true(),
            self_min: default_legacy_self_min(),
            body_min: default_legacy_body_min(),
            body_thickness: default_legacy_body_thickness(),
            collision_quality: default_legacy_collision_quality(),
            gravity: default_legacy_gravity(),
            air_damping: default_legacy_air_damping(),
            quality: default_legacy_quality(),
            ground: false,
        }
    }
}

/// Simulation settings. These are host knobs, not universal garment facts.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Simulation {
    /// Solver script the runner adopts.
    #[serde(default = "default_solver")]
    pub solver: Solver,
    /// Frames simulated before the first recorded sample. Legacy has no
    /// pre-roll and requires `0`.
    #[serde(default = "default_settle_frames")]
    pub settle_frames: u32,
    /// Last simulated frame.
    #[serde(default = "default_frame_end")]
    pub frame_end: u32,
    /// Whether `frame_end` is clamped to the action range.
    #[serde(default = "default_frame_end_mode")]
    pub frame_end_mode: FrameEndMode,
    /// Recorded frame interval; the solver still steps every frame.
    #[serde(default = "default_sample_stride")]
    pub sample_stride: u32,
    /// Pin policy and parameters.
    #[serde(default = "PinSettings::default")]
    pub pin: PinSettings,
    /// XPBD settings; used when `solver` is `xpbd`.
    #[serde(default = "XpbdSettings::default")]
    pub xpbd: XpbdSettings,
    /// Legacy settings; used when `solver` is `legacy`.
    #[serde(default = "LegacySettings::default")]
    pub legacy: LegacySettings,
}

/// Where the run writes its artifacts.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct OutputRef {
    /// Output directory; it must not exist or must be empty.
    pub directory: PathBuf,
    /// Frames per second recorded in the result.
    #[serde(default = "default_fps")]
    pub fps: u32,
    /// Export the posed body at every sampled frame for slot-independent
    /// contact checks. Defaults to `true`.
    #[serde(default = "default_true")]
    pub body_frames: bool,
}

/// One complete cloth-bake request.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ClothBakeRequest {
    /// Wire schema name.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// Contract version.
    #[serde(default = "default_request_version")]
    pub version: u32,
    /// Caller-chosen identity; the result must echo it.
    pub run_id: String,
    /// Blender pin.
    pub blender: BlenderRef,
    /// The runner script.
    pub runner: FileRef,
    /// Scene and actor.
    pub scene: SceneRef,
    /// Fitted garment OBJ.
    pub garment: FileRef,
    /// Simulation knobs.
    #[serde(default = "Simulation::default")]
    pub simulation: Simulation,
    /// Host-owned garment profile reference, not solver values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Output location.
    pub output: OutputRef,
    /// Wall-clock limit for the host process.
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
}

impl Default for Simulation {
    fn default() -> Self {
        Self {
            solver: default_solver(),
            settle_frames: default_settle_frames(),
            frame_end: default_frame_end(),
            frame_end_mode: default_frame_end_mode(),
            sample_stride: default_sample_stride(),
            pin: PinSettings::default(),
            xpbd: XpbdSettings::default(),
            legacy: LegacySettings::default(),
        }
    }
}

impl ClothBakeRequest {
    /// Parse a request from JSON bytes.
    ///
    /// # Errors
    /// Returns `invalid_request` when the document is not this schema.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).map_err(|error| {
            BlenderError::new(
                BlenderErrorKind::InvalidRequest,
                "request_json",
                format!("request does not match {REQUEST_SCHEMA} v{REQUEST_VERSION}: {error}"),
            )
        })
    }

    /// Validate scalar rules without touching the filesystem.
    ///
    /// # Errors
    /// Returns `invalid_params` with a stable code for the first broken rule.
    pub fn validate(&self) -> Result<()> {
        if self.schema != REQUEST_SCHEMA {
            return Err(invalid(
                "request_schema",
                format!("schema must be {REQUEST_SCHEMA}, got {}", self.schema),
            ));
        }
        if self.version != REQUEST_VERSION {
            return Err(invalid(
                "request_version",
                format!("version must be {REQUEST_VERSION}, got {}", self.version),
            ));
        }
        if self.run_id.trim().is_empty() || self.run_id.len() > 200 {
            return Err(invalid("run_id", "run_id must be 1..=200 characters"));
        }
        if self.blender.version.trim().is_empty() {
            return Err(invalid(
                "blender_version",
                "blender.version must not be empty",
            ));
        }
        if self.scene.body_object.trim().is_empty() {
            return Err(invalid(
                "body_object",
                "scene.body_object must not be empty",
            ));
        }
        if self.scene.clip.trim().is_empty() {
            return Err(invalid("clip", "scene.clip must not be empty"));
        }
        let simulation = &self.simulation;
        if simulation.settle_frames >= simulation.frame_end {
            return Err(invalid(
                "frame_range",
                "simulation.settle_frames must be smaller than frame_end",
            ));
        }
        if simulation.sample_stride == 0 {
            return Err(invalid("sample_stride", "sample_stride must be at least 1"));
        }
        if !(self.scene.units_per_metre.is_finite() && self.scene.units_per_metre > 0.0) {
            return Err(invalid(
                "units_per_metre",
                "scene.units_per_metre must be finite and positive",
            ));
        }
        match simulation.solver {
            Solver::Xpbd => {
                if matches!(
                    simulation.pin.policy,
                    PinPolicy::ComponentTop | PinPolicy::ExplicitZ
                ) {
                    return Err(invalid(
                        "pin_policy",
                        "the XPBD solver cannot express component_top or explicit_z pinning",
                    ));
                }
                if simulation.xpbd.substeps == 0 {
                    return Err(invalid("substeps", "substeps must be at least 1"));
                }
                if simulation.xpbd.constraint_steps == 0 {
                    return Err(invalid(
                        "constraint_steps",
                        "constraint_steps must be at least 1",
                    ));
                }
                if simulation.xpbd.collision_radius < 0.0
                    || simulation.xpbd.collider_friction < 0.0
                    || simulation.xpbd.collider_margin < 0.0
                {
                    return Err(invalid(
                        "contact_settings",
                        "collision radius, friction and margin must be non-negative",
                    ));
                }
            }
            Solver::Legacy => {
                if simulation.settle_frames != 0 {
                    return Err(invalid(
                        "legacy_settle",
                        "the legacy solver has no pre-roll; settle_frames must be 0",
                    ));
                }
                if simulation.pin.policy == PinPolicy::Neck {
                    return Err(invalid(
                        "pin_policy",
                        "legacy neck pinning is not implemented; use waist, shoulder, component_top or explicit_z",
                    ));
                }
                if simulation.legacy.bending <= 0.0
                    || simulation.legacy.structural <= 0.0
                    || simulation.legacy.mass <= 0.0
                    || simulation.legacy.quality == 0
                    || simulation.legacy.self_min < 0.0
                    || simulation.legacy.body_min < 0.0
                {
                    return Err(invalid(
                        "legacy_settings",
                        "bending, structural, mass and quality must be positive; distances non-negative",
                    ));
                }
            }
        }
        if simulation.pin.policy == PinPolicy::Shoulder
            && (!(0.0..=1.0).contains(&simulation.pin.height_fraction)
                || simulation.pin.height_fraction == 0.0)
        {
            return Err(invalid(
                "pin_height",
                "pin.height_fraction must be in (0, 1]",
            ));
        }
        if simulation.pin.band_m <= 0.0 || simulation.pin.contact_m < 0.0 {
            return Err(invalid(
                "pin_tolerance",
                "pin.band_m must be positive and pin.contact_m non-negative",
            ));
        }
        if simulation.pin.policy == PinPolicy::ExplicitZ {
            let Some(z) = simulation.pin.explicit_z else {
                return Err(invalid(
                    "explicit_z",
                    "pin.explicit_z is required for the explicit_z policy",
                ));
            };
            if !z.is_finite() {
                return Err(invalid("explicit_z", "pin.explicit_z must be finite"));
            }
        }
        if self.output.fps == 0 || self.output.fps > 240 {
            return Err(invalid("fps", "output.fps must be in 1..=240"));
        }
        if self.timeout_seconds == 0 || self.timeout_seconds > 86_400 {
            return Err(invalid("timeout", "timeout_seconds must be in 1..=86400"));
        }
        Ok(())
    }

    /// Resolve paths against `base`, verify or fill digests, and prepare the
    /// output directory.
    ///
    /// # Errors
    /// Missing inputs, digest mismatches and a non-empty output directory are
    /// classified refusals.
    pub fn resolve_with_base(&mut self, base: &Path) -> Result<()> {
        self.validate()?;
        self.runner.path = absolute(base, &self.runner.path);
        self.scene.path = absolute(base, &self.scene.path);
        self.garment.path = absolute(base, &self.garment.path);
        self.output.directory = absolute(base, &self.output.directory);
        if let Some(executable) = &self.blender.executable {
            self.blender.executable = Some(absolute(base, executable));
        }

        ensure_file(&self.runner.path, "runner_missing", "runner")?;
        ensure_file(&self.scene.path, "scene_missing", "scene")?;
        ensure_file(&self.garment.path, "garment_missing", "garment")?;
        if let Some(executable) = &self.blender.executable {
            ensure_file(executable, "blender_missing", "Blender executable")?;
        }

        ensure_digest(&self.runner.path, &mut self.runner.sha256, "runner")?;
        ensure_digest(&self.scene.path, &mut self.scene.sha256, "scene")?;
        ensure_digest(&self.garment.path, &mut self.garment.sha256, "garment")?;
        prepare_output(&self.output.directory)?;
        Ok(())
    }
}

fn invalid(code: &'static str, message: impl Into<String>) -> BlenderError {
    BlenderError::new(BlenderErrorKind::InvalidParams, code, message)
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn ensure_file(path: &Path, code: &'static str, label: &str) -> Result<()> {
    if !path.is_file() {
        return Err(BlenderError::new(
            BlenderErrorKind::NotFound,
            code,
            format!("{label} is not a readable file: {}", path.display()),
        ));
    }
    Ok(())
}

fn ensure_digest(path: &Path, slot: &mut Option<String>, label: &str) -> Result<()> {
    match slot {
        Some(claimed) => {
            let actual = sha256_file(path)?;
            if !claimed.eq_ignore_ascii_case(&actual) {
                return Err(BlenderError::new(
                    BlenderErrorKind::Refused,
                    "digest_mismatch",
                    format!(
                        "{label} digest mismatch: claimed {claimed}, actual {actual} ({})",
                        path.display()
                    ),
                ));
            }
        }
        None => {
            *slot = Some(sha256_file(path)?);
        }
    }
    Ok(())
}

fn prepare_output(directory: &Path) -> Result<()> {
    if directory.exists() {
        if !directory.is_dir() {
            return Err(BlenderError::new(
                BlenderErrorKind::Refused,
                "output_not_directory",
                format!("output is not a directory: {}", directory.display()),
            ));
        }
        let mut entries = std::fs::read_dir(directory).map_err(|error| {
            BlenderError::new(
                BlenderErrorKind::Io,
                "output_read",
                format!("{}: {error}", directory.display()),
            )
        })?;
        if entries.next().is_some() {
            return Err(BlenderError::new(
                BlenderErrorKind::Refused,
                "output_not_empty",
                format!(
                    "output directory must be empty before a run: {}",
                    directory.display()
                ),
            ));
        }
        return Ok(());
    }
    std::fs::create_dir_all(directory).map_err(|error| {
        BlenderError::new(
            BlenderErrorKind::Io,
            "output_create",
            format!("{}: {error}", directory.display()),
        )
    })
}
