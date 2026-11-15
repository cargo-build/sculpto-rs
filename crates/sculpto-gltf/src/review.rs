//! The review GLB: one palette-coloured mesh per part, in metres, no skin.
//!
//! This is the writer behind `viewer.export`: the interactive Bevy viewer is
//! the standard preview, so the engine must produce the file it orbits without
//! a Blender/Python detour. The layout is deliberately simple — every part is
//! its own mesh, primitive, material and root node, so a consumer's report
//! (the viewer's `--report` mode) counts one mesh per part and the palette
//! makes the layers distinguishable the moment the file opens.
//!
//! Units are the caller's: the handler scales BodySlide's centimetre-like
//! frame (71.4286 units per metre) to metres before calling, because the
//! viewer's camera, lights and orbit speeds are tuned for a human-sized
//! model. The writer never rescales; it refuses empty or triangle-free parts
//! instead of writing an invisible mesh.

use std::borrow::Cow;

use serde_json::{Map, Value, json};

use crate::{FLOAT, GltfError, TRIANGLES, UNSIGNED_INT, Writer};

/// The body part's palette colour: warm skin, `#e8d5c4`.
pub const BODY_COLOR: [f32; 4] = hex_rgba(0xE8, 0xD5, 0xC4, 0xFF);

/// The palette colour for one layer rank, or `None` outside `1..=5`.
///
/// The ranks and colours are the `quality-review.md` palette: rank 1
/// underwear green, 2 trousers blue, 3 tunic teal, 4 robe red, 5 cloak purple.
#[must_use]
pub fn palette(rank: u8) -> Option<[f32; 4]> {
    let (r, g, b) = match rank {
        1 => (0x2E, 0xCC, 0x71),
        2 => (0x34, 0x98, 0xDB),
        3 => (0x1A, 0xBC, 0x9C),
        4 => (0xE7, 0x4C, 0x3C),
        5 => (0x9B, 0x59, 0xB6),
        _ => return None,
    };
    Some(hex_rgba(r, g, b, 0xFF))
}

/// The `#rrggbb` spelling of a palette colour, as the report prints it.
#[must_use]
pub fn hex(color: [f32; 4]) -> String {
    let channel = |lane: f32| {
        let value = (lane.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("{value:02X}")
    };
    format!(
        "#{}{}{}",
        channel(color[0]),
        channel(color[1]),
        channel(color[2])
    )
}

/// Normalised RGBA from 8-bit channels.
const fn hex_rgba(r: u8, g: u8, b: u8, a: u8) -> [f32; 4] {
    [
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ]
}

/// One part of a review GLB: a name, its geometry and its palette colour.
///
/// Positions are already in metres and Y-up; the writer frames them as-is.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewPart {
    /// Mesh, primitive, material and node name in the written file.
    pub name: String,
    /// Vertex positions in metres.
    pub positions: Vec<[f32; 3]>,
    /// Triangles over [`Self::positions`].
    pub triangles: Vec<[u32; 3]>,
    /// The material's `baseColorFactor`.
    pub color: [f32; 4],
}

/// Write the parts as one GLB: one mesh + one coloured material per part.
///
/// Every refusal check runs before any byte is produced, so a refused parts
/// list returns a typed [`GltfError`] and nothing else. Normals are computed
/// (area-weighted, per vertex) because the OBJ sources carry none and a
/// review file must shade on open.
///
/// # Errors
/// [`GltfError::ReviewNoParts`] for an empty list, and
/// [`GltfError::ReviewPartEmpty`] for a part without vertices or triangles.
pub fn write_review_glb(parts: &[ReviewPart]) -> Result<Vec<u8>, GltfError> {
    if parts.is_empty() {
        return Err(GltfError::ReviewNoParts);
    }
    for part in parts {
        if part.positions.is_empty() {
            return Err(GltfError::ReviewPartEmpty {
                part: part.name.clone(),
                what: "vertices",
            });
        }
        if part.triangles.is_empty() {
            return Err(GltfError::ReviewPartEmpty {
                part: part.name.clone(),
                what: "triangles",
            });
        }
        u32::try_from(part.positions.len()).map_err(|_| GltfError::TooLarge {
            field: "vertex count",
            size: part.positions.len(),
        })?;
    }

    let mut writer = Writer::default();
    let mut meshes = Vec::with_capacity(parts.len());
    let mut materials = Vec::with_capacity(parts.len());
    let mut nodes = Vec::with_capacity(parts.len());

    for (index, part) in parts.iter().enumerate() {
        let position_view = writer.view(&bytes_f32(
            part.positions.iter().flat_map(|row| row.iter().copied()),
        ))?;
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for position in &part.positions {
            for (axis, value) in position.iter().enumerate() {
                min[axis] = min[axis].min(*value);
                max[axis] = max[axis].max(*value);
            }
        }
        let mut attributes = Map::new();
        attributes.insert(
            "POSITION".into(),
            json!(writer.accessor_with_bounds(
                position_view,
                FLOAT,
                "VEC3",
                part.positions.len(),
                min,
                max,
            )),
        );
        let normals = smooth_normals(&part.positions, &part.triangles);
        let normal_view = writer.view(&bytes_f32(
            normals.iter().flat_map(|row| row.iter().copied()),
        ))?;
        attributes.insert(
            "NORMAL".into(),
            json!(writer.accessor(normal_view, FLOAT, "VEC3", normals.len())),
        );
        let index_view = writer.view(&bytes_u32(part.triangles.iter().flatten().copied()))?;
        let primitive = json!({
            "attributes": attributes,
            "indices": writer.accessor(index_view, UNSIGNED_INT, "SCALAR", part.triangles.len() * 3),
            "mode": TRIANGLES,
            "material": index,
        });
        meshes.push(json!({
            "name": part.name,
            "primitives": [primitive],
        }));
        materials.push(json!({
            "name": part.name,
            "pbrMetallicRoughness": { "baseColorFactor": part.color },
            "doubleSided": true,
        }));
        nodes.push(json!({
            "name": part.name,
            "mesh": index,
        }));
    }

    let scene_nodes: Vec<usize> = (0..nodes.len()).collect();
    let mut root = Map::new();
    root.insert(
        "asset".into(),
        json!({"version": "2.0", "generator": "sculpto-gltf review"}),
    );
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{ "nodes": scene_nodes }]));
    root.insert("nodes".into(), json!(nodes));
    root.insert("meshes".into(), json!(meshes));
    root.insert("materials".into(), json!(materials));
    root.insert("accessors".into(), json!(writer.accessors));
    root.insert("bufferViews".into(), json!(writer.views));
    root.insert("buffers".into(), json!([{"byteLength": writer.bin.len()}]));

    let json = serde_json::to_vec(&Value::Object(root))
        .map_err(|error| GltfError::Serialize(error.to_string()))?;
    let json_padded = json.len() + (4 - json.len() % 4) % 4;
    let bin_padded = writer.bin.len() + (4 - writer.bin.len() % 4) % 4;
    let total = 12 + 8 + json_padded + 8 + bin_padded;
    if u32::try_from(total).is_err() {
        return Err(GltfError::TooLarge {
            field: "GLB total length",
            size: total,
        });
    }
    gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(json),
        bin: Some(Cow::Owned(writer.bin)),
    }
    .to_vec()
    .map_err(|error| GltfError::Encode(error.to_string()))
}

/// Area-weighted per-vertex normals; degenerate corners contribute nothing.
///
/// The review writer and the rigged actor writer share it: an OBJ part carries
/// no normals, and a file without them shades flat or black in the viewer.
#[must_use]
pub fn smooth_normals(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0_f32; 3]; positions.len()];
    for [a, b, c] in triangles {
        let (pa, pb, pc) = (
            positions[*a as usize],
            positions[*b as usize],
            positions[*c as usize],
        );
        let edge =
            |from: [f32; 3], to: [f32; 3]| [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let (u, v) = (edge(pa, pb), edge(pa, pc));
        let face = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        for corner in [a, b, c] {
            let normal = &mut normals[*corner as usize];
            for (axis, value) in face.iter().enumerate() {
                normal[axis] += value;
            }
        }
    }
    normals
        .into_iter()
        .map(|mut normal| {
            let length = normal.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
            if length > f32::EPSILON {
                for axis in &mut normal {
                    *axis /= length;
                }
                normal
            } else {
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

/// Little-endian bytes of a flat `f32` payload.
fn bytes_f32(values: impl Iterator<Item = f32>) -> Vec<u8> {
    values.flat_map(f32::to_le_bytes).collect()
}

/// Little-endian bytes of a flat `u32` payload.
fn bytes_u32(values: impl Iterator<Item = u32>) -> Vec<u8> {
    values.flat_map(u32::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::{BODY_COLOR, bytes_f32, bytes_u32, hex, palette, smooth_normals};

    #[test]
    fn palette_covers_the_review_ranks_and_refuses_the_rest() {
        assert_eq!(palette(1).map(hex).as_deref(), Some("#2ECC71"));
        assert_eq!(palette(2).map(hex).as_deref(), Some("#3498DB"));
        assert_eq!(palette(3).map(hex).as_deref(), Some("#1ABC9C"));
        assert_eq!(palette(4).map(hex).as_deref(), Some("#E74C3C"));
        assert_eq!(palette(5).map(hex).as_deref(), Some("#9B59B6"));
        assert_eq!(hex(BODY_COLOR), "#E8D5C4");
        for rank in [0u8, 6, 200, 255] {
            assert!(palette(rank).is_none(), "rank {rank} has no palette slot");
        }
    }

    #[test]
    fn normals_are_unit_length_and_face_away_from_the_surface() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let normals = smooth_normals(&positions, &[[0, 1, 2]]);
        for normal in &normals {
            let length = normal.iter().map(|axis| axis * axis).sum::<f32>().sqrt();
            assert!((length - 1.0).abs() < 1e-6, "normal {normal:?} is not unit");
            assert!(normal[2] > 0.0, "counter-clockwise face points +Z");
        }
    }

    #[test]
    fn a_degenerate_triangle_leaves_a_fallback_normal() {
        let positions = vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        let normals = smooth_normals(&positions, &[[0, 1, 2]]);
        assert_eq!(normals, vec![[0.0, 0.0, 1.0]; 3]);
    }

    #[test]
    fn byte_helpers_are_little_endian() {
        assert_eq!(bytes_f32([1.0].into_iter()), 1.0_f32.to_le_bytes());
        assert_eq!(bytes_u32([7u32].into_iter()), 7u32.to_le_bytes());
    }
}
