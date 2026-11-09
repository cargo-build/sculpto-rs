//! Skyrim SE `BSTriShape` writer for one position-only shape.
//!
//! C03 writes the minimum real file: header, block table, string table and one
//! `BSTriShape` with packed positions and triangles. UVs, normals, skin and
//! shader blocks arrive in C05–C10; the reader's frame conversion is undone by
//! `sculpto_nif::read`, so positions round-trip exactly.

use sculpto_core::mesh::MeshAsset;

use super::header::{NifTarget, NifWriteError, write_header};

const VF_VERTEX: u64 = 0x001;
const VF_UVS: u64 = 0x002;
const BLOCK_TYPE: &str = "BSTriShape";

/// Write a complete SE NIF containing one position-only `BSTriShape`.
///
/// The model is validated first; a refused model leaves no bytes. UVs, morphs
/// and skin are deliberately not written yet (C05–C07).
pub fn write_se_shape(asset: &MeshAsset) -> Result<Vec<u8>, NifWriteError> {
    asset.validate().map_err(NifWriteError::InvalidModel)?;
    let vertices =
        u16::try_from(asset.positions.len()).map_err(|_| NifWriteError::TooManyVertices {
            vertices: asset.positions.len(),
        })?;
    let triangles =
        u16::try_from(asset.triangles.len()).map_err(|_| NifWriteError::TooManyTriangles {
            triangles: asset.triangles.len(),
        })?;

    let has_uvs = !asset.uvs.is_empty();
    let vertex_size = if has_uvs { 20 } else { 16 };
    let vertex_desc = ((VF_VERTEX | if has_uvs { VF_UVS } else { 0 }) << 44) | (vertex_size / 4);

    let mut body = Vec::new();
    // NiObjectNET: name=-1, no extra data, no controller.
    push_i32(&mut body, -1);
    push_u32(&mut body, 0);
    push_i32(&mut body, -1);
    // NiAVObject: flags, identity transform, no collision.
    push_u32(&mut body, 0);
    for value in [0.0f32, 0.0, 0.0] {
        push_f32(&mut body, value);
    }
    for row in 0..3 {
        for column in 0..3 {
            push_f32(&mut body, if row == column { 1.0 } else { 0.0 });
        }
    }
    push_f32(&mut body, 1.0);
    push_i32(&mut body, -1);
    // BSTriShape: bounding sphere, refs, vertex descriptor.
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        push_f32(&mut body, value);
    }
    push_i32(&mut body, -1); // skin
    push_i32(&mut body, -1); // shader
    push_i32(&mut body, -1); // alpha
    push_u64(&mut body, vertex_desc);
    push_u16(&mut body, triangles);
    push_u16(&mut body, vertices);
    let data_size = asset.positions.len() as u64 * vertex_size + asset.triangles.len() as u64 * 6;
    push_u32(&mut body, data_size as u32);
    for (index, position) in asset.positions.iter().enumerate() {
        push_f32(&mut body, position[0]);
        push_f32(&mut body, position[1]);
        push_f32(&mut body, position[2]);
        push_f32(&mut body, 0.0); // unused W / bitangent X
        if has_uvs {
            let uv = asset.uvs[index];
            push_u16(&mut body, half::f16::from_f32(uv[0]).to_bits());
            push_u16(&mut body, half::f16::from_f32(uv[1]).to_bits());
        }
    }
    for triangle in &asset.triangles {
        push_u16(&mut body, triangle[0] as u16);
        push_u16(&mut body, triangle[1] as u16);
        push_u16(&mut body, triangle[2] as u16);
    }
    push_u32(&mut body, 0); // SSE particle_data_size

    let mut file = write_header(NifTarget::Se);
    // Block count precedes BSStreamHeader in the parsed layout.
    push_u32(&mut file, 1);
    // BSStreamHeader: SE bsver, empty author/process/export strings.
    push_u32(&mut file, 100);
    file.push(0); // author
    file.push(0); // process script
    file.push(0); // export script
    // Block type table.
    push_u16(&mut file, 1);
    push_u32(&mut file, BLOCK_TYPE.len() as u32);
    file.extend_from_slice(BLOCK_TYPE.as_bytes());
    push_u16(&mut file, 0); // block type index
    push_u32(&mut file, body.len() as u32);
    // Empty string table.
    push_u32(&mut file, 0);
    push_u32(&mut file, 0);
    push_u32(&mut file, 0); // num_groups
    file.extend_from_slice(&body);
    Ok(file)
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

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_core::mesh::PartitionId;

    fn triangle() -> MeshAsset {
        MeshAsset {
            name: "Body".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            ..MeshAsset::default()
        }
    }

    #[test]
    fn se_shape_roundtrips_positions() {
        let asset = triangle();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("our SE NIF parses");
        assert_eq!(document.shapes.len(), 1);
        let shape = &document.shapes[0];
        assert_eq!(shape.triangles, asset.triangles);
        for (actual, expected) in shape.positions.iter().zip(&asset.positions) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-6, "position drift");
            }
        }
    }

    #[test]
    fn se_uv_seam_roundtrips() {
        let mut asset = triangle();
        asset.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.uvs.len(), asset.uvs.len());
        for (actual, expected) in shape.uvs.iter().zip(&asset.uvs) {
            for (left, right) in actual.iter().zip(expected) {
                assert!((left - right).abs() <= 1e-3, "UV drift at the seam");
            }
        }
    }

    #[test]
    fn invalid_model_is_refused_before_bytes() {
        let mut asset = triangle();
        asset.triangles[0] = [0, 1, 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::InvalidModel(_))
        ));
    }
}
