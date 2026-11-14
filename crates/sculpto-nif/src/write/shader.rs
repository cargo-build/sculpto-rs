//! Shader, texture-set and alpha blocks.
//!
//! The model carries neutral [`MaterialSlot`] values, and an authored slot table
//! when the source had one; both Skyrim targets use
//! `BSLightingShaderProperty` + `BSShaderTextureSet` + `NiAlphaProperty`. An
//! authored table is written verbatim — count and slot order included, empty
//! slots keeping their position — because a source's slot vocabulary is wider
//! than the neutral roles. A slot authored through the roles projects them onto
//! the positions the source itself uses: diffuse (0), normal (1), glow or
//! lighting mask (2), environment (4), environment mask (5) and specular (7).
//!
//! Slot 7 is the alternate specular position: the reader only routes it to
//! [`sculpto_core::mesh::MaterialSlot::specular`] when the material carries the
//! model-space-normal gate, so a projected specular map is meaningful together
//! with that flag and is inert without it. That is the source's own rule,
//! quoted from the vendored slot table
//! (`vendor/byroredux/nif/src/import/material/slot_role.rs:536-546`), not a
//! writer convention.

use byroredux_nif::shader_flags::{skyrim_slsf1, skyrim_slsf2};
use sculpto_core::mesh::{AlphaProperty, MaterialSlot, MeshAsset, ShaderFlags, TextureSlot};

use super::header::NifWriteError;

/// Block type table name for the lighting shader property.
pub(super) const SHADER_TYPE: &str = "BSLightingShaderProperty";
/// Block type table name for the texture set.
pub(super) const TEXTURE_TYPE: &str = "BSShaderTextureSet";
/// Block type table name for the alpha property.
pub(super) const ALPHA_TYPE: &str = "NiAlphaProperty";
/// Skyrim `BSLightingShaderProperty` shader type used for a neutral surface.
const SHADER_TYPE_DEFAULT: u32 = 0;
/// Skyrim `BSLightingShaderProperty` shader type that carries a skin-tint colour.
const SHADER_TYPE_SKIN_TINT: u32 = 5;
/// Skyrim `SkyrimShaderPropertyFlags1` bit 0: specular term enabled.
///
/// The vendored flag module declares only the SLSF1 bits its importers read;
/// nif.xml's `SkyrimShaderPropertyFlags1` is the authority for this position.
const SLSF1_SPECULAR: u32 = 0x0000_0001;
/// `SkyrimShaderPropertyFlags1` bit 3: per-vertex alpha modulates the surface.
const SLSF1_VERTEX_ALPHA: u32 = 0x0000_0008;
/// `SkyrimShaderPropertyFlags1` bit 7: environment reflection map applies.
const SLSF1_ENVIRONMENT_MAPPING: u32 = 0x0000_0080;
/// `NiAlphaProperty.flags` bit 0: colour blending enabled.
const ALPHA_BLEND: u16 = 0x0001;
/// `NiAlphaProperty.flags` bit 9: alpha test enabled.
const ALPHA_TEST: u16 = 0x0200;
/// Texture-set slot that carries the glow or lighting mask.
const SLOT_GLOW: usize = 2;
/// Texture-set slot that carries the environment reflection map.
const SLOT_ENVIRONMENT: usize = 4;
/// Texture-set slot that carries the environment reflection mask.
const SLOT_ENVIRONMENT_MASK: usize = 5;
/// Texture-set slot that carries the alternate specular map.
const SLOT_SPECULAR: usize = 7;
/// Highest texture-set slot this projection writes.
const SLOT_COUNT: usize = SLOT_SPECULAR + 1;

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
/// An authored table is written exactly: every slot keeps its position and the
/// count is the table's length, so a later consumer reads the same slot
/// vocabulary back. Without one the role view is written, trimmed to the highest
/// present role; an absent earlier role is written as an empty path, which the
/// reader treats as "no texture".
pub fn texture_set(material: &MaterialSlot) -> Vec<u8> {
    if !material.texture_set.is_empty() {
        if material.texture_set.iter().all(String::is_empty) {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        push_u32(&mut bytes, material.texture_set.len() as u32);
        for value in &material.texture_set {
            push_u32(&mut bytes, value.len() as u32);
            bytes.extend_from_slice(value.as_bytes());
        }
        return bytes;
    }
    let mut slots: [&str; SLOT_COUNT] = [""; SLOT_COUNT];
    for (slot, value) in [
        (0, material.texture(TextureSlot::Diffuse)),
        (1, material.texture(TextureSlot::Normal)),
        (SLOT_GLOW, material.texture(TextureSlot::Glow)),
        (SLOT_ENVIRONMENT, material.texture(TextureSlot::Environment)),
        (
            SLOT_ENVIRONMENT_MASK,
            material.texture(TextureSlot::EnvironmentMask),
        ),
        (SLOT_SPECULAR, material.texture(TextureSlot::Specular)),
    ] {
        slots[slot] = value.unwrap_or("");
    }
    // Trim to the highest present role: the source's own tables are ragged, and
    // an absent inner role still has to keep its position.
    let Some(count) = slots.iter().rposition(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();
    push_u32(&mut bytes, count as u32 + 1);
    for value in &slots[..=count] {
        push_u32(&mut bytes, value.len() as u32);
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

/// Write a `NiAlphaProperty` body, or `None` when the slot authors no alpha.
///
/// The flags word is the inverse of the reader's decode
/// (`import/material/mod.rs`: blend bit 0, source factor bits 1-4, destination
/// factor bits 5-8, test bit 9); a threshold authored outside `0.0..=1.0` is
/// clamped to the byte the format stores.
pub fn alpha(property: &AlphaProperty) -> Vec<u8> {
    let mut flags =
        (u16::from(property.src_blend & 0xF) << 1) | (u16::from(property.dst_blend & 0xF) << 5);
    if property.blend {
        flags |= ALPHA_BLEND;
    }
    if property.test {
        flags |= ALPHA_TEST;
    }
    let threshold = (property.threshold.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut bytes = Vec::new();
    // NiObjectNET: no name, no extra data, no controller.
    push_i32(&mut bytes, -1);
    push_u32(&mut bytes, 0);
    push_i32(&mut bytes, -1);
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.push(threshold);
    bytes
}

/// Write a `BSLightingShaderProperty` body.
///
/// `texture_set` is the block index of the matching `BSShaderTextureSet`, or
/// `-1` when the slot declares no textures. The shader block stays unnamed: the
/// model's slot name is a slot label, not the block's `NiObjectNET` name, and
/// the shipped Skyrim shader blocks carry none.
///
/// `shader_type` follows the payload: a slot carrying a skin-tint colour writes
/// the skin-tint type and its trailing colour, every other slot the neutral
/// type. Specular colour, glossiness and the lighting-effect scalars have no
/// model field and stay at the source's neutral defaults.
pub fn shader(material: &MaterialSlot, texture_set: i32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let shader_type = match material.skin_tint {
        Some(_) => SHADER_TYPE_SKIN_TINT,
        None => SHADER_TYPE_DEFAULT,
    };
    push_u32(&mut bytes, shader_type);
    // NiObjectNET: no name, no extra data, no controller.
    push_i32(&mut bytes, -1);
    push_u32(&mut bytes, 0);
    push_i32(&mut bytes, -1);
    push_u32(&mut bytes, shader_flags_1(material.flags));
    push_u32(&mut bytes, shader_flags_2(material.flags));
    for value in [0.0f32, 0.0, 1.0, 1.0] {
        push_f32(&mut bytes, value); // UV offset, UV scale
    }
    push_i32(&mut bytes, texture_set);
    for value in [
        material.emissive_color[0],
        material.emissive_color[1],
        material.emissive_color[2],
        material.emissive_multiple,
    ] {
        push_f32(&mut bytes, value); // emissive colour, emissive multiple
    }
    push_u32(&mut bytes, 3); // clamp mode: wrap S / wrap T
    for value in [1.0f32, 0.0, 0.0] {
        push_f32(&mut bytes, value); // alpha, refraction, glossiness
    }
    for value in [0.0f32, 0.0, 0.0, material.specular_power] {
        push_f32(&mut bytes, value); // specular colour, specular strength
    }
    for value in [0.0f32, 0.0] {
        push_f32(&mut bytes, value); // lighting effect 1/2
    }
    if let Some(tint) = material.skin_tint {
        for value in tint {
            push_f32(&mut bytes, value); // skin-tint colour (type 5 trailer)
        }
    }
    bytes
}

/// `SkyrimShaderPropertyFlags1` word for the model's gates.
fn shader_flags_1(flags: ShaderFlags) -> u32 {
    word(&[
        (flags.specular, SLSF1_SPECULAR),
        (flags.vertex_alpha, SLSF1_VERTEX_ALPHA),
        (flags.environment_mapping, SLSF1_ENVIRONMENT_MAPPING),
        (flags.model_space_normals, skyrim_slsf1::MODEL_SPACE_NORMALS),
        (flags.emissive, skyrim_slsf1::OWN_EMIT),
    ])
}

/// `SkyrimShaderPropertyFlags2` word for the model's gates.
fn shader_flags_2(flags: ShaderFlags) -> u32 {
    word(&[
        (flags.double_sided, skyrim_slsf2::DOUBLE_SIDED),
        (flags.glow_map, skyrim_slsf2::GLOW_MAP),
        (flags.soft_lighting, skyrim_slsf2::SOFT_LIGHTING),
        (flags.rim_lighting, skyrim_slsf2::RIM_LIGHTING),
        (flags.back_lighting, skyrim_slsf2::BACK_LIGHTING),
    ])
}

/// The flag word for every set gate.
fn word(gates: &[(bool, u32)]) -> u32 {
    gates
        .iter()
        .filter(|(set, _)| *set)
        .fold(0, |word, (_, bit)| word | bit)
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
