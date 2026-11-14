//! Headless Blender host adapter for cloth bakes.
//!
//! The crate owns the versioned request/result contract, Blender discovery and
//! the pinned-version check, process execution with a timeout, and structural
//! validation of the produced artifacts. It does not link Blender or Python and
//! owns no geometry rule: body identity, fitting and cloth selection stay in
//! the engine crates.
//!
//! Build and tests must pass without Blender installed (DECL0007). A missing
//! executable is a classified runtime error.

mod digest;
pub mod error;
pub mod exec;
pub mod request;
pub mod result;
pub mod run;
pub mod validate;

pub use error::{BlenderError, BlenderErrorKind};
pub use request::{
    BlenderRef, ClothBakeRequest, FileRef, FrameEndMode, LegacySettings, OutputRef, PinPolicy,
    PinSettings, REQUEST_SCHEMA, REQUEST_VERSION, SceneRef, Simulation, Solver, XpbdSettings,
};
pub use result::{
    Artifact, ClothBakeResult, FrameArtifact, FrameInfo, HostInfo, PinInfo, RESULT_SCHEMA,
    RESULT_VERSION, RunError, RunStatus, SceneInfo, ShellResult, Timings,
};
pub use run::{RunOutcome, run, run_with_base};

/// JSON schema of [`ClothBakeRequest`] for the engine registry.
///
/// # Panics
/// Never at runtime: the schema is a fixed serializable value. The panic message
/// names the contract if a future derive change breaks serialization.
pub fn request_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ClothBakeRequest))
        .expect("the cloth-bake request schema is serializable")
}
