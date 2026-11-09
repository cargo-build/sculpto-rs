//! Consumer profiles: caller-selected, measured export caps.
//!
//! The caller selects a [`ConsumerProfile`] — name, targeted requirement,
//! resolved version, measured caps — and passes it to the adapter; the model
//! and writers contain no consumer numbers. Each cap is a [`Measured<T>`]
//! carrying the command and date that measured it, so a profile is versioned
//! data and is re-measured when the resolved version changes. A resolved
//! version mismatch between callers is a data problem, not a writer guess:
//! [`crate::write_glb_with_profile`] only ever reads the measured value it
//! is handed.
//!
//! Format limits (NIF `u16` field widths, `.tri` index width) remain
//! adapter-enforced `FormatLimits`, not profile data: they are format facts
//! of the written container, stable per target variant, while profile caps
//! are consumer measurements that change with the consumer's version.
//!
//! # Example
//!
//! The A00B measurement of the Bevy `bevy_mesh` morph-weight constant on
//! the resolved 0.20.1 build:
//!
//! ```
//! use sculpto_gltf::profile::{ConsumerProfile, Measured};
//!
//! let bevy = ConsumerProfile {
//!     name: "bevy".into(),
//!     requirement: "0.20".into(),
//!     resolved: "0.20.1".into(),
//!     max_morph_targets: Measured {
//!         value: 256,
//!         command: "rg MAX_MORPH_WEIGHTS bevy_mesh-0.20.1/src/morph.rs".into(),
//!         date: "2026-11-10".into(),
//!     },
//! };
//! assert_eq!(bevy.max_morph_targets.value, 256);
//! ```

/// One measured cap or capability, with the provenance of the measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured<T> {
    /// The measured limit or capability value.
    pub value: T,
    /// The command that produced the measurement.
    pub command: String,
    /// The date of the measurement (`YYYY-MM-DD`).
    pub date: String,
}

/// One consumer's measured export profile, selected by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsumerProfile {
    /// Profile name, quoted verbatim in a cap refusal.
    pub name: String,
    /// The consumer version requirement the profile targets.
    pub requirement: String,
    /// The exact consumer version that was measured.
    pub resolved: String,
    /// The measured morph-target cap of this consumer build.
    pub max_morph_targets: Measured<usize>,
}
