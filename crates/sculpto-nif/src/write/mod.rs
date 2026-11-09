//! NIF writing. The reader and the writer live in one crate so the header and
//! block gates cannot drift apart.

pub mod header;
pub mod le_shape;
pub mod se_shape;
pub mod skin;
pub mod strings;
