//! Deterministic body/outfit morph model for Sculpto.
//!
//! Sculpto is a Rust reimplementation of the authoring half of BodySlide and
//! Outfit Studio: it reads slider projects and presets, applies them to body
//! and garment meshes, and produces portable assets with runtime physics
//! metadata. This crate owns the format model and the pure transformation
//! rules; host adapters (Blender add-on, Bevy viewer) stay outside it.
//!
//! Layers:
//! - [`document`] detects a supported file by its XML root element.
//! - [`preset`] models `SliderPresets` files (named slider value sets).
//! - [`project`] models `.osp` `SliderSetInfo` projects (buildable slider sets).
//! - [`shape_data`] decodes `.osd`/`.bsd` morph containers into sparse offsets.
//! - [`report`] records children the readers do not model, instead of failing.
//!
//! Everything here is a pure parse or transformation step: no UI, no file
//! writes, no hidden global state, so results are reproducible and testable.

pub mod document;
pub mod error;
pub mod mesh;
pub mod preset;
pub mod project;
pub mod report;
pub mod rig;
pub mod shape_data;
mod xml_util;

pub use document::Document;
pub use error::{Error, ErrorKind, Result};
pub use preset::{Preset, PresetValue, SliderSize};
pub use project::{
    OutputFile, Project, ReferenceInfo, ShapeRef, SliderDataRef, SliderDef, SliderSet,
};
pub use report::UnknownElement;
pub use shape_data::{MorphBlock, MorphContainer, MorphData, VertexOffset};
