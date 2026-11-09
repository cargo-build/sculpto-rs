//! Skin blocks shared by the SE and LE writers.
//!
//! The model carries bone names, per-bone bind matrices and four influences per
//! vertex. Both targets write `NiSkinInstance` + `NiSkinData`; SE additionally
//! packs the influences into the `BSTriShape` vertex stream (`VF_SKINNED`) while
//! LE reads them from the sparse `NiSkinData` arrays. Bones are `NiNode` blocks
//! so the reader can resolve each palette entry to its name.

use std::collections::BTreeSet;

use sculpto_core::mesh::MeshAsset;

use super::header::NifWriteError;

/// Validated palette, bind matrices and per-bone sparse weights.
pub struct SkinPlan {
    /// Bone names in palette order.
    pub names: Vec<String>,
    /// Per-bone bind matrix, column-major (mesh -> bone at bind).
    pub bind: Vec<[[f32; 4]; 4]>,
    /// Per bone: `(vertex, weight)` entries with a non-zero influence.
    pub weights: Vec<Vec<(u16, f32)>>,
}

/// Build the skin plan, refusing a palette the file format cannot express.
pub fn plan(asset: &MeshAsset) -> Result<Option<SkinPlan>, NifWriteError> {
    let Some(skin) = &asset.skin else {
        return Ok(None);
    };
    if skin.bones.is_empty() {
        return Err(NifWriteError::UnnamedBone { index: 0 });
    }
    if skin.bones.len() > u8::MAX as usize {
        return Err(NifWriteError::TooManyBones {
            bones: skin.bones.len(),
        });
    }
    let mut seen = BTreeSet::new();
    for (index, name) in skin.bones.iter().enumerate() {
        if name.trim().is_empty() {
            return Err(NifWriteError::UnnamedBone { index });
        }
        if name.len() > u8::MAX as usize {
            return Err(NifWriteError::NameTooLong { name: name.clone() });
        }
        if !seen.insert(name.as_str()) {
            return Err(NifWriteError::DuplicateBone { name: name.clone() });
        }
    }
    let bind = match skin.bind_inverse.is_empty() {
        true => vec![identity(); skin.bones.len()],
        false => skin.bind_inverse.clone(),
    };
    let mut weights = vec![Vec::new(); skin.bones.len()];
    for (vertex, (indices, quad)) in skin.indices.iter().zip(&skin.weights).enumerate() {
        let vertex = u16::try_from(vertex).map_err(|_| NifWriteError::TooManyVertices {
            vertices: skin.indices.len(),
        })?;
        for (slot, bone) in indices.iter().enumerate() {
            let weight = quad[slot];
            if weight > 0.0 {
                weights[*bone as usize].push((vertex, weight));
            }
        }
    }
    Ok(Some(SkinPlan {
        names: skin.bones.clone(),
        bind,
        weights,
    }))
}

/// A named `NiNode` body with an identity transform and the given children.
pub fn node(name: i32, children: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_i32(&mut bytes, name);
    push_u32(&mut bytes, 0); // extra data
    push_i32(&mut bytes, -1); // controller
    push_u32(&mut bytes, 0); // flags
    for value in [0.0f32, 0.0, 0.0] {
        push_f32(&mut bytes, value);
    }
    for row in 0..3 {
        for column in 0..3 {
            push_f32(&mut bytes, if row == column { 1.0 } else { 0.0 });
        }
    }
    push_f32(&mut bytes, 1.0);
    push_i32(&mut bytes, -1); // collision
    push_u32(&mut bytes, children.len() as u32);
    for child in children {
        push_i32(&mut bytes, *child);
    }
    push_u32(&mut bytes, 0); // effects
    bytes
}

/// A `NiSkinInstance` body pointing at `NiSkinData` and the bone nodes.
pub fn instance(data: i32, skeleton_root: i32, bones: &[i32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    push_i32(&mut bytes, data);
    push_i32(&mut bytes, -1); // skin partition
    push_i32(&mut bytes, skeleton_root);
    push_u32(&mut bytes, bones.len() as u32);
    for bone in bones {
        push_i32(&mut bytes, *bone);
    }
    bytes
}

/// A `NiSkinData` body with an identity global transform and sparse weights.
pub fn data(plan: &SkinPlan) -> Result<Vec<u8>, NifWriteError> {
    let mut bytes = Vec::new();
    push_transform(&mut bytes, &identity(), 0)?;
    push_u32(&mut bytes, plan.names.len() as u32);
    push_u8(&mut bytes, 1); // has vertex weights
    for (bone, entries) in plan.weights.iter().enumerate() {
        push_transform(&mut bytes, &plan.bind[bone], bone)?;
        for _ in 0..4 {
            push_f32(&mut bytes, 0.0); // bounding sphere
        }
        push_u16(&mut bytes, entries.len() as u16);
        for (vertex, weight) in entries {
            push_u16(&mut bytes, *vertex);
            push_f32(&mut bytes, *weight);
        }
    }
    Ok(bytes)
}

fn push_transform(
    bytes: &mut Vec<u8>,
    matrix: &[[f32; 4]; 4],
    bone: usize,
) -> Result<(), NifWriteError> {
    let scale =
        (matrix[0][0] * matrix[0][0] + matrix[0][1] * matrix[0][1] + matrix[0][2] * matrix[0][2])
            .sqrt();
    if scale == 0.0 || !scale.is_finite() {
        return Err(NifWriteError::SingularBind { bone });
    }
    for row in &[0, 1, 2] {
        for column in &[0, 1, 2] {
            push_f32(bytes, matrix[*column][*row] / scale);
        }
    }
    push_f32(bytes, matrix[3][0]);
    push_f32(bytes, matrix[3][1]);
    push_f32(bytes, matrix[3][2]);
    push_f32(bytes, scale);
    Ok(())
}

/// The identity bind matrix.
pub fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0f32; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

fn push_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
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
