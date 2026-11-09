//! Round-trip properties between the reader shape and the export model.

use sculpto_core::mesh::{MeshAsset, MeshError};
use sculpto_nif::NifShape;

fn shape(seed: u64) -> NifShape {
    let vertices = 3 + (seed as usize % 25);
    let positions = (0..vertices)
        .map(|index| {
            let value = index as f32 + seed as f32;
            [value, value * 0.5, -value]
        })
        .collect();
    let uvs = if seed.is_multiple_of(2) {
        (0..vertices)
            .map(|index| [index as f32 / vertices as f32, 0.25])
            .collect()
    } else {
        Vec::new()
    };
    NifShape {
        name: format!("shape{seed}"),
        positions,
        uvs,
        normals: (0..vertices)
            .map(|index| {
                let value = (index % 3) as f32 + 0.5;
                [value, -value, value]
            })
            .collect(),
        tangents: (0..vertices)
            .map(|index| {
                [
                    1.0,
                    0.0,
                    0.0,
                    if index.is_multiple_of(2) { 1.0 } else { -1.0 },
                ]
            })
            .collect(),
        triangles: (0..1 + seed as usize % 30)
            .map(|index| {
                [
                    (index % vertices) as u32,
                    ((index + 1) % vertices) as u32,
                    ((index + 2) % vertices) as u32,
                ]
            })
            .collect(),
        bones: vec!["root".into(), "spine".into()],
        bone_indices: vec![[0, 1, 0, 0]; vertices],
        weights: vec![[0.75, 0.25, 0.0, 0.0]; vertices],
    }
}

#[test]
fn shape_model_roundtrip_is_stable() {
    for seed in 0..32 {
        let source = shape(seed);
        let model = MeshAsset::from(&source);
        model.validate().expect("generated shape validates");
        let back = NifShape::from(&model);
        assert_eq!(back, source, "seed {seed}: round-trip lost a field");
        let again = MeshAsset::from(&back);
        assert_eq!(again, model, "seed {seed}: second projection differs");
    }
}

#[test]
fn corrupted_shape_is_refused_by_the_model() {
    let mut source = shape(1);
    let vertices = source.positions.len() as u32;
    source.triangles[0][0] = vertices;
    let model = MeshAsset::from(&source);
    assert!(matches!(
        model.validate(),
        Err(MeshError::TriangleVertexOutOfRange { .. })
    ));
}
