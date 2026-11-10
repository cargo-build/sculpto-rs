//! The adapter's raw skin capture, from bytes through the model.
//!
//! Synthetic files are written by this crate's own writers, so the assertions
//! describe the projection: authored values survive a packed SE round trip
//! without the importer's renormalization, and LE sparse values are read exactly.
//! The two real bodies are private host fixtures; without their environment
//! variables the controls print a skip and claim nothing.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::read;
use sculpto_nif::write::header::{NifTarget, NifWriteError};
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::write::{DroppedInfluence, InfluencePolicy, write_shape, write_shape_with_policy};

/// One triangle whose two-influence row deliberately sums to 0.9999, the drift a
/// packed half-float source can carry.
fn drifting_weights() -> [[f32; 4]; 3] {
    [[0.5, 0.2499, 0.25, 0.0]; 3]
}

fn body(quads: [[u16; 4]; 3], weights: [[f32; 4]; 3], bones: usize) -> MeshAsset {
    MeshAsset {
        name: "Body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: (0..bones).map(|bone| format!("bone{bone}")).collect(),
            bind_inverse: Vec::new(),
            influences: SkinWeights::from_quads(&quads, &weights).expect("three four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

#[test]
fn packed_se_lanes_keep_their_quantized_values() {
    let weights = drifting_weights();
    let source = body([[0, 1, 2, 0]; 3], weights, 3);
    let bytes = write_se_shape(&source).expect("a packed body writes");
    let document = read(&bytes).expect("our SE NIF parses");
    let skin = document.shapes[0]
        .skin
        .as_ref()
        .expect("the shape carries one skin");

    assert_eq!(skin.influences.rows(), 3);
    for vertex in 0..3 {
        let row = skin.influences.row(vertex).expect("one row per vertex");
        let stored = row.weights;
        assert_eq!(row.joints, &[0, 1, 2], "packed lane order survives");
        assert_eq!(stored.len(), 3, "a zero lane is padding, not an influence");
        for (slot, value) in stored.iter().enumerate() {
            let expected = half::f16::from_f32(weights[vertex][slot]).to_f32();
            assert_eq!(*value, expected, "half-float value, not a renormalized one");
        }
        let sum: f32 = stored.iter().sum();
        assert!(
            (sum - 1.0).abs() > 1.0e-5,
            "the importer must not rescale the authored row to one"
        );
    }
}

#[test]
fn le_sparse_weights_are_read_exactly() {
    let weights = [[0.5, 0.25, 0.25, 0.0]; 3];
    let source = body([[0, 1, 2, 0]; 3], weights, 3);
    let bytes = write_le_shape(&source).expect("an LE body writes");
    let document = read(&bytes).expect("our LE NIF parses");
    let skin = document.shapes[0]
        .skin
        .as_ref()
        .expect("the shape carries one skin");

    assert_eq!(skin.bones, vec!["bone0", "bone1", "bone2"]);
    assert_eq!(skin.influences.rows(), 3);
    for vertex in 0..3 {
        let row = skin.influences.row(vertex).expect("one row per vertex");
        assert_eq!(row.joints, &[0, 1, 2]);
        for (stored, written) in row.weights.iter().zip(weights[vertex]) {
            assert_eq!(*stored, written, "LE sparse values are f32 and exact");
        }
    }
}

#[test]
fn native_reduction_reports_the_drop_and_le_keeps_the_sparse_row() {
    let mut source = body([[1, 0, 0, 0]; 3], [[1.0, 0.0, 0.0, 0.0]; 3], 5);
    let authored = SkinWeights::new(
        vec![0, 5, 6, 7],
        vec![0, 1, 2, 3, 4, 1, 1],
        vec![0.0625, 0.125, 0.5, 0.25, 0.125, 1.0, 1.0],
        5,
    )
    .expect("a five-influence row and two single-influence rows");
    source.skin.as_mut().expect("skinned").influences = authored.clone();
    for target in [NifTarget::Se, NifTarget::Le] {
        assert_eq!(
            write_shape(&source, target),
            Err(NifWriteError::TooManyInfluences {
                vertex: 0,
                influences: 5,
            }),
        );
        let output = write_shape_with_policy(&source, target, InfluencePolicy::NativeFour)
            .expect("explicit native reduction writes");
        assert_eq!(
            output.dropped_influences,
            [DroppedInfluence {
                vertex: 0,
                bone: "bone0".into(),
                weight: 0.0625,
            }],
        );
        let document = read(&output.bytes).expect("the reduced native file parses");
        let decoded = &document.shapes[0]
            .skin
            .as_ref()
            .expect("skinned")
            .influences;
        match target {
            NifTarget::Le => assert_eq!(decoded, &authored, "LE sparse data retains every value"),
            NifTarget::Se => {
                let row = decoded.row(0).expect("encoded row");
                assert_eq!(row.joints, [2, 3, 1, 4]);
                assert_eq!(row.weights, [0.5, 0.25, 0.125, 0.125]);
            }
        }
    }
    assert_eq!(source.skin.expect("skinned").influences, authored);
}

#[test]
fn se_refuses_a_retained_positive_weight_that_would_encode_as_zero() {
    let tiny = f32::from_bits(1);
    let source = body([[0, 1, 0, 0]; 3], [[1.0, tiny, 0.0, 0.0]; 3], 2);
    assert_eq!(
        write_shape_with_policy(&source, NifTarget::Se, InfluencePolicy::NativeFour),
        Err(NifWriteError::WeightUnrepresentable {
            vertex: 0,
            joint: 1,
            weight: tiny,
        }),
    );
    assert!(write_le_shape(&source).is_ok(), "LE f32 stores the value");
}

#[test]
fn himbo_body_captures_raw_sums() {
    let Ok(path) = std::env::var("SCULPTO_HIMBO_NIF") else {
        eprintln!("SCULPTO_HIMBO_NIF not set; skipping the raw HIMBO weight control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the HIMBO fixture is readable");
    let document = read(&bytes).expect("the HIMBO body reads");
    let mut rows = 0;
    let mut widest = 0;
    let mut max_deviation = 0.0_f32;
    let mut palettes = Vec::new();
    for shape in &document.shapes {
        let skin = shape.skin.as_ref().expect("a body shape is skinned");
        palettes.push(skin.bones.len());
        assert_eq!(
            skin.influences.rows(),
            shape.positions.len(),
            "one row per vertex"
        );
        rows += skin.influences.rows();
        for vertex in 0..skin.influences.rows() {
            let row = skin.influences.row(vertex).expect("one row per vertex");
            widest = widest.max(row.joints.len());
            let deviation = (row.weights.iter().sum::<f32>() - 1.0).abs();
            max_deviation = max_deviation.max(deviation);
        }
    }
    palettes.sort_unstable();
    eprintln!("HIMBO: {rows} rows, palettes {palettes:?}, max |sum-1| {max_deviation:e}");
    assert_eq!(palettes, vec![5, 24], "the configured palettes");
    assert_eq!(rows, 9346 + 917, "the configured vertex counts");
    assert!(
        widest <= 4,
        "a packed SE body has four source lanes, not {widest}"
    );
    assert!(
        max_deviation > 1.0e-5 && max_deviation < 1.0e-3,
        "half-float drift is preserved, not normalized away: {max_deviation:e}"
    );
}

#[test]
fn bijin_body_keeps_five_influences() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        eprintln!("SCULPTO_BIJIN_NIF not set; skipping the raw Bijin weight control");
        return;
    };
    let bytes = std::fs::read(&path).expect("the Bijin fixture is readable");
    let document = read(&bytes).expect("the Bijin body reads");
    let shape = document
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    let skin = shape.skin.as_ref().expect("the body is skinned");
    assert_eq!(skin.bones.len(), 22, "the accepted Bijin palette");
    assert_eq!(skin.influences.rows(), 3471, "the accepted vertex count");

    let wide: Vec<usize> = (0..skin.influences.rows())
        .filter(|vertex| {
            skin.influences
                .row(*vertex)
                .is_some_and(|row| row.joints.len() == 5)
        })
        .collect();
    assert_eq!(
        wide,
        vec![818, 960, 2821, 2849, 2851],
        "the authored five-influence rows, no thresholding"
    );
    let scene = byroredux_nif::parse_nif(&bytes).expect("raw vendor scene");
    let source = scene
        .get_as::<byroredux_nif::blocks::skin::NiSkinData>(4)
        .expect("pinned Bijin skin-data block");
    for vertex in wide {
        let expected: Vec<_> = source
            .bones
            .iter()
            .enumerate()
            .filter_map(|(joint, bone)| {
                bone.vertex_weights
                    .iter()
                    .find(|entry| usize::from(entry.vertex_index) == vertex && entry.weight > 0.0)
                    .map(|entry| {
                        (
                            u16::try_from(joint).expect("22-bone palette"),
                            entry.weight.to_bits(),
                        )
                    })
            })
            .collect();
        let row = skin.influences.row(vertex).expect("authored wide row");
        let stored: Vec<_> = row
            .joints
            .iter()
            .copied()
            .zip(row.weights.iter().map(|value| value.to_bits()))
            .collect();
        assert_eq!(
            stored, expected,
            "vertex {vertex} keeps exact bone/weight pairs"
        );
    }
    let row = skin.influences.row(2821).expect("vertex 2821");
    assert!(
        row.weights
            .iter()
            .any(|&weight| weight > 0.0 && weight < 1.0e-15),
        "a tiny positive fifth influence is data, not padding"
    );
}
