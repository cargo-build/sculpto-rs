//! Shader and texture-set blocks.
//!
//! The model carries neutral [`MaterialSlot`] values; both Skyrim targets use
//! `BSLightingShaderProperty` + `BSShaderTextureSet`. Only the roles with a
//! measured Skyrim slot are projected: diffuse (0) and normal (1). A specular
//! path has no confirmed slot in this layout, so it is refused instead of being
//! written into the glow slot and misread by every consumer.

use sculpto_core::mesh::{MaterialSlot, MeshAsset, TextureSlot};

use super::header::NifWriteError;

/// Block type table name for the lighting shader property.
pub(super) const SHADER_TYPE: &str = "BSLightingShaderProperty";
/// Block type table name for the texture set.
pub(super) const TEXTURE_TYPE: &str = "BSShaderTextureSet";
/// Skyrim `BSLightingShaderProperty` shader type used for a neutral surface.
const SHADER_TYPE_DEFAULT: u32 = 0;
/// `SLSF2` bit 4: back-face rendering enabled.
const DOUBLE_SIDED: u32 = 0x0000_0010;

/// The single material slot a shape may carry, or `None`.
///
/// Two slots need per-triangle material authoring, which neither Skyrim target
/// writes yet, so they are refused by name instead of collapsing to one.
pub(super) fn single_slot(asset: &MeshAsset) -> Result<Option<&MaterialSlot>, NifWriteError> {
    match asset.materials.len() {
        0 => Ok(None),
        1 => Ok(Some(&asset.materials[0])),
        count => Err(NifWriteError::MultipleMaterialSlots { count }),
    }
}

/// Write a `BSShaderTextureSet` body for a material slot.
///
/// The list is trimmed to the highest present role; an absent earlier role is
/// written as an empty path, which the reader treats as "no texture".
pub fn texture_set(material: &MaterialSlot) -> Result<Vec<u8>, NifWriteError> {
    if material.texture(TextureSlot::Specular).is_some() {
        return Err(NifWriteError::TextureSlotUnsupported { slot: "specular" });
    }
    let diffuse = material.texture(TextureSlot::Diffuse).unwrap_or("");
    let normal = material.texture(TextureSlot::Normal).unwrap_or("");
    let count = if !normal.is_empty() {
        2
    } else if !diffuse.is_empty() {
        1
    } else {
        0
    };
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::new();
    push_u32(&mut bytes, count);
    for value in [diffuse, normal].into_iter().take(count as usize) {
        push_u32(&mut bytes, value.len() as u32);
        bytes.extend_from_slice(value.as_bytes());
    }
    Ok(bytes)
}

/// Write a `BSLightingShaderProperty` body.
///
/// `texture_set` is the block index of the matching `BSShaderTextureSet`, or
/// `-1` when the slot declares no textures. Flags carry only the measured
/// double-sided bit; tint and gloss data have no model field and stay neutral.
pub fn shader(material: &MaterialSlot, texture_set: i32, name: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_u32(&mut bytes, SHADER_TYPE_DEFAULT);
    // NiObjectNET: name, no extra data, no controller.
    push_i32(&mut bytes, name);
    push_u32(&mut bytes, 0);
    push_i32(&mut bytes, -1);
    push_u32(&mut bytes, 0); // shader flags 1
    push_u32(
        &mut bytes,
        if material.double_sided {
            DOUBLE_SIDED
        } else {
            0
        },
    );
    for value in [0.0f32, 0.0, 1.0, 1.0] {
        push_f32(&mut bytes, value); // UV offset, UV scale
    }
    push_i32(&mut bytes, texture_set);
    for value in [0.0f32, 0.0, 0.0, 1.0] {
        push_f32(&mut bytes, value); // emissive colour, emissive multiple
    }
    push_u32(&mut bytes, 3); // clamp mode: wrap S / wrap T
    for value in [1.0f32, 0.0, 0.0] {
        push_f32(&mut bytes, value); // alpha, refraction, glossiness
    }
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        push_f32(&mut bytes, value); // specular colour, specular strength
    }
    for value in [0.0f32, 0.0] {
        push_f32(&mut bytes, value); // lighting effect 1/2
    }
    bytes
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
