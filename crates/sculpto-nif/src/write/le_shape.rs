//! Skyrim LE `NiTriShape` + `NiTriShapeData` writer for one position-only shape.
//!
//! The LE wire format keeps geometry in a separate data block. C04 writes the
//! two blocks with positions and triangles only; UVs, normals and skin follow in
//! C05–C07.

use sculpto_core::mesh::MeshAsset;

use super::header::{NifTarget, NifWriteError, write_header};

/// Write a complete LE NIF containing one position-only `NiTriShape`.
pub fn write_le_shape(asset: &MeshAsset) -> Result<Vec<u8>, NifWriteError> {
    asset.validate().map_err(NifWriteError::InvalidModel)?;
    let vertices =
        u16::try_from(asset.positions.len()).map_err(|_| NifWriteError::TooManyVertices {
            vertices: asset.positions.len(),
        })?;
    let triangles =
        u16::try_from(asset.triangles.len()).map_err(|_| NifWriteError::TooManyTriangles {
            triangles: asset.triangles.len(),
        })?;

    let mut shape = Vec::new();
    push_i32(&mut shape, -1); // name
    push_u32(&mut shape, 0); // extra data
    push_i32(&mut shape, -1); // controller
    push_u32(&mut shape, 0); // flags
    for value in [0.0f32, 0.0, 0.0] {
        push_f32(&mut shape, value);
    }
    for row in 0..3 {
        for column in 0..3 {
            push_f32(&mut shape, if row == column { 1.0 } else { 0.0 });
        }
    }
    push_f32(&mut shape, 1.0);
    push_i32(&mut shape, -1); // collision
    push_i32(&mut shape, 1); // data block
    push_i32(&mut shape, -1); // skin
    push_u32(&mut shape, 0); // num_materials
    push_u32(&mut shape, 0); // active_material_index
    shape.push(0); // dirty flag
    push_i32(&mut shape, -1); // shader
    push_i32(&mut shape, -1); // alpha

    let mut data = Vec::new();
    push_i32(&mut data, 0); // group id
    push_u16(&mut data, vertices);
    data.push(0); // keep flags
    data.push(0); // compress flags
    data.push(1); // has vertices
    for position in &asset.positions {
        push_f32(&mut data, position[0]);
        push_f32(&mut data, position[1]);
        push_f32(&mut data, position[2]);
    }
    push_u16(&mut data, 0); // data flags: no normals, no UVs
    push_u32(&mut data, 0); // material CRC
    data.push(0); // has normals
    for value in [0.0f32, 0.0, 0.0, 0.0] {
        push_f32(&mut data, value); // center + radius
    }
    data.push(0); // has vertex colors
    // data_flags bit 0 clear -> no UV sets, has UV false
    push_u16(&mut data, 0); // consistency flags
    push_i32(&mut data, -1); // additional data
    push_u16(&mut data, triangles);
    push_u32(&mut data, u32::from(triangles) * 3);
    data.push(1); // has triangles
    for triangle in &asset.triangles {
        push_u16(&mut data, triangle[0] as u16);
        push_u16(&mut data, triangle[1] as u16);
        push_u16(&mut data, triangle[2] as u16);
    }
    push_u16(&mut data, 0); // match groups

    let mut file = write_header(NifTarget::Le);
    push_u32(&mut file, 2); // block count
    push_u32(&mut file, 83); // LE bsver
    file.push(0); // author
    file.push(0); // process script
    file.push(0); // export script
    push_u16(&mut file, 2); // block types
    for name in ["NiTriShape", "NiTriShapeData"] {
        push_u32(&mut file, name.len() as u32);
        file.extend_from_slice(name.as_bytes());
    }
    push_u16(&mut file, 0);
    push_u16(&mut file, 1);
    push_u32(&mut file, shape.len() as u32);
    push_u32(&mut file, data.len() as u32);
    push_u32(&mut file, 0); // string count
    push_u32(&mut file, 0); // max string length
    push_u32(&mut file, 0); // num_groups
    file.extend_from_slice(&shape);
    file.extend_from_slice(&data);
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
    fn le_shape_roundtrips_positions() {
        let asset = triangle();
        let bytes = write_le_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("our LE NIF parses");
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
    fn invalid_model_is_refused_before_bytes() {
        let mut asset = triangle();
        asset.triangles[0] = [0, 1, 3];
        assert!(matches!(
            write_le_shape(&asset),
            Err(NifWriteError::InvalidModel(_))
        ));
    }
}
