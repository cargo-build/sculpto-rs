//! Skyrim SE `BSTriShape` writer for one shape.
//!
//! C03 wrote positions and triangles, C05 added UVs and C06 adds byte normals
//! and, when the model carries them, SE tangents/bitangents. Skin, shader and
//! morph blocks arrive later (C07–C12). `sculpto_nif::read` undoes the reader's
//! frame conversion, so positions, normals and tangents round-trip.

use sculpto_core::mesh::MeshAsset;

use super::header::{NifTarget, NifWriteError, write_header};

const VF_VERTEX: u64 = 0x001;
const VF_UVS: u64 = 0x002;
const VF_NORMALS: u64 = 0x008;
const VF_TANGENTS: u64 = 0x010;
const BLOCK_TYPE: &str = "BSTriShape";

/// Write a complete SE NIF containing one `BSTriShape`.
///
/// The model is validated first; a refused model leaves no bytes. Normals are
/// byte-encoded, so they round-trip within the format's 8-bit precision;
/// tangents are only written when the model carries both normals and tangents.
/// Morphs and skin are deliberately not written yet (C07 onward).
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
    let has_normals = !asset.normals.is_empty();
    let has_tangents = !asset.tangents.is_empty();
    if has_tangents && !has_normals {
        return Err(NifWriteError::TangentsWithoutNormals);
    }
    let mut flags = VF_VERTEX;
    let mut vertex_size = 16;
    if has_uvs {
        flags |= VF_UVS;
        vertex_size += 4;
    }
    if has_normals {
        flags |= VF_NORMALS;
        vertex_size += 4;
    }
    if has_tangents {
        flags |= VF_TANGENTS;
        vertex_size += 4;
    }
    let vertex_desc = (flags << 44) | (vertex_size / 4);

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
        let normal = match has_normals {
            true => Some(unit(asset.normals[index], "normal", index)?),
            false => None,
        };
        let tangent = match has_tangents {
            true => {
                let raw = asset.tangents[index];
                let direction = unit([raw[0], raw[1], raw[2]], "tangent", index)?;
                let sign = if raw[3].is_sign_negative() { -1.0 } else { 1.0 };
                Some((direction, sign))
            }
            false => None,
        };

        push_f32(&mut body, position[0]);
        push_f32(&mut body, position[1]);
        push_f32(&mut body, position[2]);
        // Position W carries Bitangent X when VF_TANGENTS is set.
        push_f32(&mut body, tangent.map_or(0.0, |(b, _)| b[0]));
        if has_uvs {
            let uv = asset.uvs[index];
            push_u16(&mut body, half::f16::from_f32(uv[0]).to_bits());
            push_u16(&mut body, half::f16::from_f32(uv[1]).to_bits());
        }
        if let Some(normal) = normal {
            push_u8(&mut body, encode_normal(normal[0]));
            push_u8(&mut body, encode_normal(normal[1]));
            push_u8(&mut body, encode_normal(normal[2]));
            // Normal W carries Bitangent Y when VF_TANGENTS is set.
            push_u8(&mut body, encode_normal(tangent.map_or(0.0, |(b, _)| b[1])));
        }
        if let Some((bitangent, sign)) = tangent {
            // On-disk Tangent is Bethesda's dP/dV; the reader keeps it only to
            // derive the sign of `dot(B, cross(N, T))`. `sign * cross(B, N)`
            // reproduces the requested sign exactly.
            let normal = normal.expect("tangents require normals");
            let on_disk = normalize(cross(normal, bitangent), "tangent", index)?;
            push_u8(&mut body, encode_normal(sign * on_disk[0]));
            push_u8(&mut body, encode_normal(sign * on_disk[1]));
            push_u8(&mut body, encode_normal(sign * on_disk[2]));
            // Tangent W carries Bitangent Z.
            push_u8(&mut body, encode_normal(bitangent[2]));
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

fn unit(
    vector: [f32; 3],
    attribute: &'static str,
    vertex: usize,
) -> Result<[f32; 3], NifWriteError> {
    normalize(vector, attribute, vertex)
}

fn normalize(
    vector: [f32; 3],
    attribute: &'static str,
    vertex: usize,
) -> Result<[f32; 3], NifWriteError> {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length == 0.0 || !length.is_finite() {
        return Err(NifWriteError::ZeroVector { attribute, vertex });
    }
    Ok([vector[0] / length, vector[1] / length, vector[2] / length])
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn encode_normal(value: f32) -> u8 {
    ((value.clamp(-1.0, 1.0) + 1.0) * 127.5)
        .round()
        .clamp(0.0, 255.0) as u8
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

    /// A flat-shaded cube: 24 vertices, six per-face unit normals, per-face
    /// tangents with alternating bitangent signs.
    fn cube() -> (MeshAsset, Vec<[f32; 3]>, Vec<[f32; 4]>) {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut tangents = Vec::new();
        let mut triangles = Vec::new();
        let mut partitions = Vec::new();
        for face in 0..6_u32 {
            let axis = (face / 2) as usize;
            let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
            let mut normal = [0.0f32; 3];
            normal[axis] = sign;
            let mut u = [0.0f32; 3];
            u[(axis + 1) % 3] = 1.0;
            let mut v = [0.0f32; 3];
            v[(axis + 2) % 3] = 1.0;
            let base = positions.len() as u32;
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let mut position = [0.0f32; 3];
                position[axis] = sign;
                for k in 0..3 {
                    position[k] += su * u[k] + sv * v[k];
                }
                positions.push(position);
                normals.push(normal);
                tangents.push([u[0], u[1], u[2], sign]);
            }
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
            partitions.push(PartitionId(0));
            partitions.push(PartitionId(0));
        }
        let asset = MeshAsset {
            name: "Cube".into(),
            positions,
            normals: normals.clone(),
            tangents: tangents.clone(),
            triangles,
            triangle_partitions: partitions,
            ..MeshAsset::default()
        };
        (asset, normals, tangents)
    }

    #[test]
    fn flat_shaded_cube_roundtrips_unit_normals() {
        let (asset, expected, _) = cube();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.normals.len(), expected.len());
        for (actual, expected) in shape.normals.iter().zip(&expected) {
            let length =
                (actual[0] * actual[0] + actual[1] * actual[1] + actual[2] * actual[2]).sqrt();
            assert!((length - 1.0).abs() <= 0.02, "normal not unit: {length}");
            let dot = actual[0] * expected[0] + actual[1] * expected[1] + actual[2] * expected[2];
            assert!(dot > 0.999, "normal direction drifted: {dot}");
        }
    }

    #[test]
    fn se_tangents_roundtrip_with_their_sign() {
        let (asset, _, expected) = cube();
        let bytes = write_se_shape(&asset).expect("writes");
        let document = crate::read(&bytes).expect("parses");
        let shape = &document.shapes[0];
        assert_eq!(shape.tangents.len(), expected.len());
        for (actual, expected) in shape.tangents.iter().zip(&expected) {
            assert_eq!(actual[3].signum(), expected[3].signum(), "sign flipped");
            let dot = actual[0] * expected[0] + actual[1] * expected[1] + actual[2] * expected[2];
            assert!(dot > 0.99, "tangent direction drifted: {dot}");
        }
    }

    #[test]
    fn zero_normals_are_refused() {
        let mut asset = triangle();
        asset.normals = vec![[0.0; 3]; 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::ZeroVector {
                attribute: "normal",
                vertex: 0
            })
        ));
    }

    #[test]
    fn tangents_without_normals_are_refused() {
        let mut asset = triangle();
        asset.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
        assert!(matches!(
            write_se_shape(&asset),
            Err(NifWriteError::TangentsWithoutNormals)
        ));
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
