//! The GLB validator gate: the writer output must survive both the `gltf`
//! crate's structural validation and the glTF 2.0 accessor rules that crate
//! does not implement, and must fail the same gate after an in-place
//! corruption.
//!
//! `gltf` 1.4.1 exposes no `validate::Validator`; `Gltf::from_slice` runs
//! `gltf-json`'s `Validate` derive, which checks index bounds, field sizes and
//! required fields, but not accessor extents, buffer-view alignment, POSITION
//! `min`/`max` or index ranges. [`validate_glb`] therefore layers those
//! specification checks over the crate's parser and is the single path both
//! the clean and the corrupted assertions use.

use gltf::Gltf;
use gltf::accessor::DataType;
use gltf::mesh::{Mode, Semantic};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::write_glb;
use serde_json::Value;

/// One synthetic skinned model: two partitions, normals, UVs and asymmetric
/// extents, so `min`/`max` and the alignment checks cannot pass by accident.
fn synthetic_model() -> MeshAsset {
    let quads: Vec<[u16; 4]> = (0..9_u16)
        .map(|vertex| [vertex % 2, (vertex + 1) % 2, 0, 0])
        .collect();
    let weights: Vec<[f32; 4]> = (0..9)
        .map(|vertex| match vertex % 2 {
            0 => [0.75, 0.25, 0.0, 0.0],
            _ => [0.5, 0.5, 0.0, 0.0],
        })
        .collect();
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [-0.5, 0.25, 2.0],
        [2.0, -0.5, 0.75],
        [0.5, 1.5, -3.0],
        [3.5, 3.0, 3.0],
        [-2.0, -1.25, -2.0],
        [0.125, 0.0, 0.5],
    ];
    MeshAsset {
        name: "synthetic".into(),
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        uvs: (0..positions.len())
            .map(|vertex| [vertex as f32 / 8.0, 0.25])
            .collect(),
        positions,
        triangles: vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
        // Partition 0 holds two triangles, partition 7 one, so their index
        // accessor counts differ and the negative control can target one.
        triangle_partitions: vec![PartitionId(0), PartitionId(0), PartitionId(7)],
        skin: Some(Skin {
            bones: vec!["root".into(), "spine".into()],
            bind_inverse: vec![
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [1.0, 2.0, 3.0, 1.0],
                ],
                [
                    [2.0, 0.0, 0.0, 0.0],
                    [0.0, 0.5, 0.0, 0.0],
                    [0.0, 0.0, 4.0, 0.0],
                    [5.0, 0.0, -1.0, 1.0],
                ],
            ],
            influences: SkinWeights::from_quads(&quads, &weights).expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

/// The gate: the crate's structural validation plus the glTF 2.0 accessor
/// rules it does not implement. Every returned entry is an error; the `gltf`
/// crate has no warning severity to report.
fn validate_glb(bytes: &[u8]) -> Vec<String> {
    let mut errors = Vec::new();
    let parsed = match Gltf::from_slice(bytes) {
        Ok(parsed) => parsed,
        Err(error) => {
            errors.push(format!("gltf structural validation: {error}"));
            return errors;
        }
    };
    let Some(blob) = parsed.blob.as_deref() else {
        errors.push("GLB: the BIN chunk is missing".to_owned());
        return errors;
    };
    let document = &parsed.document;

    for (index, view) in document.views().enumerate() {
        if view.offset() % 4 != 0 {
            errors.push(format!(
                "bufferView {index}: byteOffset {} is not 4-byte aligned",
                view.offset()
            ));
        }
        if view.offset() + view.length() > blob.len() {
            errors.push(format!(
                "bufferView {index}: range {}..{} exceeds the {} byte binary chunk",
                view.offset(),
                view.offset() + view.length(),
                blob.len()
            ));
        }
        if view.stride().is_some() {
            errors.push(format!("bufferView {index}: unexpectedly interleaved"));
        }
    }

    for (index, accessor) in document.accessors().enumerate() {
        let Some(view) = accessor.view() else {
            errors.push(format!("accessor {index}: has no bufferView"));
            continue;
        };
        let component = accessor.data_type().size();
        let start = view.offset() + accessor.offset();
        let length = accessor.size() * accessor.count();
        if start % component != 0 {
            errors.push(format!(
                "accessor {index}: byte {start} is not {component}-byte aligned"
            ));
        }
        if accessor.offset() + length > view.length() {
            errors.push(format!(
                "accessor {index}: {length} bytes at offset {} exceed bufferView {} ({} bytes)",
                accessor.offset(),
                view.index(),
                view.length()
            ));
        }
    }

    for (mesh_index, mesh) in document.meshes().enumerate() {
        for (primitive_index, primitive) in mesh.primitives().enumerate() {
            let label = format!("mesh {mesh_index} primitive {primitive_index}");
            let Some(position) = primitive.get(&Semantic::Positions) else {
                errors.push(format!("{label}: POSITION is missing"));
                continue;
            };
            let vertex_count = position.count();
            if position.min().is_none() || position.max().is_none() {
                errors.push(format!("{label}: POSITION is missing min/max"));
            }
            for (semantic, accessor) in primitive.attributes() {
                if accessor.count() != vertex_count {
                    errors.push(format!(
                        "{label}: {semantic:?} count {} differs from POSITION count {vertex_count}",
                        accessor.count()
                    ));
                }
            }
            if primitive.mode() != Mode::Triangles {
                continue;
            }
            let Some(indices) = primitive.indices() else {
                errors.push(format!("{label}: TRIANGLES primitive has no indices"));
                continue;
            };
            if indices.count() % 3 != 0 {
                errors.push(format!(
                    "{label}: index count {} is not a multiple of 3",
                    indices.count()
                ));
                continue;
            }
            let Some(values) = index_values(&parsed, &indices) else {
                errors.push(format!(
                    "{label}: index accessor does not fit the binary chunk"
                ));
                continue;
            };
            if let Some(vertex) = values
                .iter()
                .copied()
                .find(|vertex| *vertex as usize >= vertex_count)
            {
                errors.push(format!(
                    "{label}: index {vertex} is outside {vertex_count} vertices"
                ));
            }
        }
    }

    errors
}

/// The bytes one accessor addresses, or `None` when that range is outside its
/// buffer view or the binary chunk.
fn accessor_slice<'a>(parsed: &'a Gltf, accessor: &gltf::Accessor) -> Option<&'a [u8]> {
    let view = accessor.view()?;
    let blob = parsed.blob.as_deref()?;
    let length = accessor.size() * accessor.count();
    let start = view.offset() + accessor.offset();
    let end = start + length;
    if end > blob.len() || accessor.offset() + length > view.length() {
        return None;
    }
    Some(&blob[start..end])
}

/// Index values of an unsigned integer accessor, or `None` when its range does
/// not fit.
fn index_values(parsed: &Gltf, accessor: &gltf::Accessor) -> Option<Vec<u32>> {
    let bytes = accessor_slice(parsed, accessor)?;
    let values = match accessor.data_type() {
        DataType::U8 => bytes.iter().map(|byte| u32::from(*byte)).collect(),
        DataType::U16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|cell| u32::from(u16::from_le_bytes(*cell)))
            .collect(),
        DataType::U32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|cell| u32::from_le_bytes(*cell))
            .collect(),
        _ => return None,
    };
    Some(values)
}

/// Patch one equal-length slice inside the JSON chunk, leaving the GLB frames
/// and the chunk length untouched.
fn replace_in_json(glb: &mut [u8], needle: &[u8], replacement: &[u8]) {
    assert_eq!(
        needle.len(),
        replacement.len(),
        "an in-place patch keeps the JSON length"
    );
    let json_length = u32::from_le_bytes(glb[12..16].try_into().expect("chunk length")) as usize;
    // 12-byte header plus the 8-byte JSON chunk header.
    let chunk = &mut glb[20..20 + json_length];
    let positions: Vec<usize> = chunk
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect();
    assert_eq!(
        positions.len(),
        1,
        "expected one {:?} in the JSON chunk, found {}",
        String::from_utf8_lossy(needle),
        positions.len()
    );
    let at = positions[0];
    chunk[at..at + needle.len()].copy_from_slice(replacement);
}

/// A JSON number array read back as `f32` cells.
fn floats_from_json(value: &Value) -> Vec<f32> {
    value
        .as_array()
        .expect("a JSON number array")
        .iter()
        .map(|cell| cell.as_f64().expect("a JSON number") as f32)
        .collect()
}

/// Fixed-width `f32` rows, little-endian.
fn floats<const WIDTH: usize>(bytes: &[u8]) -> Vec<[f32; WIDTH]> {
    bytes
        .chunks_exact(4 * WIDTH)
        .map(|row| {
            let mut values = [0.0_f32; WIDTH];
            for (axis, cell) in values.iter_mut().enumerate() {
                let start = axis * 4;
                *cell = f32::from_le_bytes(row[start..start + 4].try_into().expect("f32 cell"));
            }
            values
        })
        .collect()
}

#[test]
fn the_synthetic_model_passes_the_validator_gate() {
    let bytes = write_glb(&synthetic_model()).expect("the synthetic model writes");
    let errors = validate_glb(&bytes);
    assert!(
        errors.is_empty(),
        "the GLB validator gate reported errors: {errors:#?}"
    );
}

#[test]
fn a_corrupted_index_accessor_count_fails_the_gate() {
    let mut bytes = write_glb(&synthetic_model()).expect("the synthetic model writes");
    // The partition-7 index accessor is the only `u32` accessor with three
    // elements; raising its count to nine keeps the JSON length and the GLB
    // frames intact while making the accessor nine times its buffer view.
    replace_in_json(
        &mut bytes,
        b"\"componentType\":5125,\"count\":3",
        b"\"componentType\":5125,\"count\":9",
    );
    assert!(
        Gltf::from_slice(&bytes).is_ok(),
        "the crate's structural layer does not read accessor extents, so the gate must"
    );
    let errors = validate_glb(&bytes);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("accessor") && error.contains("exceed bufferView")),
        "a corrupted accessor count must fail the gate: {errors:#?}"
    );
}

#[test]
fn a_misaligned_buffer_view_fails_the_gate() {
    let mut bytes = write_glb(&synthetic_model()).expect("the synthetic model writes");
    // Only the first view starts at byte zero, so this needle is unique; the
    // sorted JSON keys put `byteOffset` last in each view object.
    replace_in_json(&mut bytes, b"\"byteOffset\":0}", b"\"byteOffset\":2}");
    let errors = validate_glb(&bytes);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("is not 4-byte aligned")),
        "a misaligned buffer view must fail the gate: {errors:#?}"
    );
}

#[test]
fn buffer_views_are_four_byte_aligned_and_tightly_packed() {
    let bytes = write_glb(&synthetic_model()).expect("writes");
    let parsed = Gltf::from_slice(&bytes).expect("parses");
    let blob = parsed
        .blob
        .as_deref()
        .expect("the GLB carries the BIN chunk");
    let views: Vec<_> = parsed.document.views().collect();
    assert!(!views.is_empty(), "the writer produced buffer views");
    assert_eq!(views[0].offset(), 0, "the first view starts the buffer");
    for (index, view) in views.iter().enumerate() {
        assert_eq!(view.offset() % 4, 0, "bufferView {index} byteOffset");
        assert!(
            view.offset() + view.length() <= blob.len(),
            "bufferView {index} stays inside the binary chunk"
        );
        assert!(view.stride().is_none(), "bufferView {index} is packed");
    }
    for (index, accessor) in parsed.document.accessors().enumerate() {
        assert_eq!(accessor.offset(), 0, "accessor {index} starts its view");
        assert!(
            accessor_slice(&parsed, &accessor).is_some(),
            "accessor {index} fits"
        );
    }
}

#[test]
fn position_accessors_carry_spec_required_min_and_max() {
    let asset = synthetic_model();
    let parsed = Gltf::from_slice(&write_glb(&asset).expect("writes")).expect("parses");
    let mesh = parsed.document.meshes().next().expect("one mesh");
    let expected_min = asset
        .positions
        .iter()
        .fold([f32::INFINITY; 3], |mut min, row| {
            for (axis, value) in row.iter().enumerate() {
                min[axis] = min[axis].min(*value);
            }
            min
        });
    let expected_max = asset
        .positions
        .iter()
        .fold([f32::NEG_INFINITY; 3], |mut max, row| {
            for (axis, value) in row.iter().enumerate() {
                max[axis] = max[axis].max(*value);
            }
            max
        });

    for primitive in mesh.primitives() {
        let position = primitive
            .get(&Semantic::Positions)
            .expect("every primitive has POSITION");
        let min = floats_from_json(&position.min().expect("POSITION min is required"));
        let max = floats_from_json(&position.max().expect("POSITION max is required"));
        assert_eq!(min, expected_min, "POSITION min matches the vertices");
        assert_eq!(max, expected_max, "POSITION max matches the vertices");

        let read_back = floats::<3>(accessor_slice(&parsed, &position).expect("POSITION fits"));
        assert_eq!(read_back, asset.positions, "POSITION is bit-exact");
    }
}

#[test]
fn index_accessors_are_u32_at_every_vertex_count() {
    let parsed = Gltf::from_slice(&write_glb(&synthetic_model()).expect("writes")).expect("parses");
    let mesh = parsed.document.meshes().next().expect("one mesh");
    let partitions: Vec<_> = mesh
        .primitives()
        .map(|primitive| primitive.indices().expect("indices are written"))
        .collect();
    assert_eq!(partitions.len(), 2, "one primitive per partition");
    for (index, accessor) in partitions.iter().enumerate() {
        assert_eq!(
            accessor.data_type(),
            DataType::U32,
            "partition {index} indices are u32"
        );
    }

    // A vertex count above the `u16` range must not narrow the index width.
    let positions: Vec<[f32; 3]> = (0..70_000)
        .map(|vertex| [vertex as f32, 0.0, 0.0])
        .collect();
    let large = MeshAsset {
        name: "large".into(),
        positions,
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    };
    let parsed = Gltf::from_slice(&write_glb(&large).expect("writes")).expect("parses");
    let indices = parsed
        .document
        .meshes()
        .next()
        .expect("one mesh")
        .primitives()
        .next()
        .expect("one primitive")
        .indices()
        .expect("indices are written");
    assert_eq!(
        indices.data_type(),
        DataType::U32,
        "70 000 vertices still use u32 indices"
    );
    assert_eq!(indices.count(), 3);
}
