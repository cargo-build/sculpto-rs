//! Mesh operations shared by Sculpto adapters and surfaces.
//!
//! The package owns the mesh model and every transformation that has to agree
//! across surfaces (DECL0002).
//! The first slice is garment conforming: binding a garment to the body shape
//! it was authored on and re-applying that binding to a morphed body. The
//! mesh model lives in `sculpto-core`; garment operations include weight transfer.
//! [`actor`] reads and evaluates the embedded glTF actor-motion subset.
//!
//! [`aura`](aura) adds the second slice: a cross-section envelope of the body
//! that garment vertices are pressed onto, in the same frame as the body,
//! with band rules for merged volumes (a skirt around both legs) and
//! collision proxies. [`classify`](classify) adds the third slice: measuring
//! a garment into its [`classify::GarmentKind`](classify::GarmentKind) and
//! resolving its corset-aura profile. [`intake`](intake) audits a garment
//! mesh's arrival state — components, openings, interior chains, rest
//! self-intersections — and names the fit/repair/split route. [`intake_gltf`]
//! reads the raw generated export (`.glb`, `.gltf`, `.obj`) behind that audit
//! and normalises it to the declared metre scale. [`check`](check)
//! adds the measured gates: triangle-pair intersections that vertex-inside
//! counting cannot see.

pub mod actor;
pub mod aura;
pub mod check;
pub mod classify;
pub mod clip_fix;
pub mod conform;
pub mod contact_morph;
pub mod control;
pub mod delta_conform;
pub mod garment;
pub mod intake;
pub mod intake_gltf;
pub mod mask;
mod point_query;
pub mod silhouette;
pub mod surface;
mod surface_util;
mod topology;
pub mod uv;
pub mod weight_norm;
pub mod wrap;
