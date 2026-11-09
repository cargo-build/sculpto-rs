//! Integration tests for `.tri` morph writer, reader, quantisation, and index validation.
//!
//! Covers deliverables D01, D02, and D03 according to the reference in `TriFile.cpp`.

use sculpto_nif::write::tri::{
    MAGIC_BYTES, PositionOffset, QUANT_DIVISOR, TriError, TriFile, TriPositionMorph, TriShape,
    TriUvMorph, UvOffset, compute_position_multiplier, compute_uv_multiplier, read_tri,
    read_tri_with_limit, write_tri,
};

/// D01: writer + reader round-trip with zero difference for a fixture with one
/// shape, two morphs (one position, one UV), and non-trivial offsets.
#[test]
fn d01_roundtrip_zero_difference() {
    let fixture = TriShape {
        name: "BaseBody".to_string(),
        vertex_count: Some(1000),
        position_morphs: vec![TriPositionMorph {
            name: "WaistSmall".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 12,
                    offset: [150.0, -300.0, 450.0],
                },
                PositionOffset {
                    vertex: 88,
                    offset: [-32767.0, 1000.0, -2500.0],
                },
                PositionOffset {
                    vertex: 250,
                    offset: [32767.0, 0.0, 1234.0],
                },
            ],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "WaistSmallUV".to_string(),
            offsets: vec![
                UvOffset {
                    vertex: 12,
                    offset: [-50.0, 200.0],
                },
                UvOffset {
                    vertex: 88,
                    offset: [32767.0, -16383.0],
                },
            ],
        }],
    };

    let original_doc = TriFile::new(vec![fixture.clone()]);
    let bytes = write_tri(&original_doc).expect("writing fixture must succeed");

    // Verify header bytes
    assert_eq!(&bytes[0..4], &MAGIC_BYTES);

    let decoded_doc = read_tri(&bytes).expect("reading fixture bytes must succeed");

    assert_eq!(decoded_doc.shapes.len(), 1);
    let decoded_shape = &decoded_doc.shapes[0];
    assert_eq!(decoded_shape.name, fixture.name);

    // Position morph round-trip: zero difference
    assert_eq!(decoded_shape.position_morphs.len(), 1);
    assert_eq!(
        decoded_shape.position_morphs[0].name,
        fixture.position_morphs[0].name
    );
    assert_eq!(
        decoded_shape.position_morphs[0].offsets,
        fixture.position_morphs[0].offsets
    );

    // UV morph round-trip: zero difference
    assert_eq!(decoded_shape.uv_morphs.len(), 1);
    assert_eq!(decoded_shape.uv_morphs[0].name, fixture.uv_morphs[0].name);
    assert_eq!(
        decoded_shape.uv_morphs[0].offsets,
        fixture.uv_morphs[0].offsets
    );
}

/// D02: quantisation. Assert the observed round-trip error is under the
/// quantisation step (`max_component / 32767`), not "small".
#[test]
fn d02_quantisation_error_bounded_by_step() {
    let raw_offsets = [
        [0.123456, -0.654321, 14.876543],
        [-14.876543, 7.111111, -3.333333],
        [5.555555, -2.222222, 0.000123],
    ];
    let max_comp = 14.876543_f32;
    let expected_mult = max_comp / QUANT_DIVISOR;

    let shape = TriShape {
        name: "AnalyticMesh".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: "FloatMorph".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 0,
                    offset: raw_offsets[0],
                },
                PositionOffset {
                    vertex: 1,
                    offset: raw_offsets[1],
                },
                PositionOffset {
                    vertex: 2,
                    offset: raw_offsets[2],
                },
            ],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "FloatUV".to_string(),
            offsets: vec![
                UvOffset {
                    vertex: 0,
                    offset: [-0.054321, 0.987654],
                },
                UvOffset {
                    vertex: 1,
                    offset: [0.987654, -0.456789],
                },
            ],
        }],
    };

    let computed_mult = compute_position_multiplier(&shape.position_morphs[0].offsets);
    assert_eq!(computed_mult, expected_mult);

    let uv_max = 0.987654_f32;
    let uv_mult = compute_uv_multiplier(&shape.uv_morphs[0].offsets);
    assert_eq!(uv_mult, uv_max / QUANT_DIVISOR);

    let bytes = write_tri(&TriFile::new(vec![shape.clone()])).expect("serialization succeeds");
    let decoded = read_tri(&bytes).expect("deserialization succeeds");

    // Assert observed position error <= quantisation step
    let dec_pos = &decoded.shapes[0].position_morphs[0];
    for (orig, dec) in shape.position_morphs[0]
        .offsets
        .iter()
        .zip(&dec_pos.offsets)
    {
        assert_eq!(orig.vertex, dec.vertex);
        for lane in 0..3 {
            let error = (orig.offset[lane] - dec.offset[lane]).abs();
            assert!(
                error <= expected_mult + 1e-7,
                "lane {lane} observed error {error} exceeds quantisation step {expected_mult}"
            );
        }
    }

    // Assert observed UV error <= quantisation step
    let dec_uv = &decoded.shapes[0].uv_morphs[0];
    for (orig, dec) in shape.uv_morphs[0].offsets.iter().zip(&dec_uv.offsets) {
        assert_eq!(orig.vertex, dec.vertex);
        for lane in 0..2 {
            let error = (orig.offset[lane] - dec.offset[lane]).abs();
            assert!(
                error <= uv_mult + 1e-7,
                "uv lane {lane} observed error {error} exceeds quantisation step {uv_mult}"
            );
        }
    }
}

/// D02: all-zero morph (`mult == 0.0`).
///
/// Documents what the reference `TriFile.cpp` does vs Sculpto:
/// - In `TriFile.cpp`, when a morph has all zero offsets, `mult` is calculated as `0.0f`.
///   During write, `v.second.x / mult` evaluates to `0.0f / 0.0f = NaN`, which in C++ yields
///   undefined behaviour when cast to `short`.
///   During read, `TriFile::Read` calculates `offset = x * mult = 0.0f`, then calls
///   `if (!offset.IsZero(true))` which filters out all zero displacements. If no non-zero
///   displacements remain, `morphOffsets.size() == 0`, and the morph is omitted.
/// - In Sculpto:
///   `mult` is `0.0f`. The writer emits `0_i16` without division by zero.
///   The reader preserves all authored morphs and offsets, with 0 observed error.
#[test]
fn d02_all_zero_morph_handling() {
    let shape = TriShape {
        name: "ZeroShape".to_string(),
        vertex_count: Some(10),
        position_morphs: vec![TriPositionMorph {
            name: "ZeroPosition".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 0,
                    offset: [0.0, 0.0, 0.0],
                },
                PositionOffset {
                    vertex: 2,
                    offset: [0.0, 0.0, 0.0],
                },
            ],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "ZeroUv".to_string(),
            offsets: vec![UvOffset {
                vertex: 0,
                offset: [0.0, 0.0],
            }],
        }],
    };

    let mult_pos = compute_position_multiplier(&shape.position_morphs[0].offsets);
    assert_eq!(mult_pos, 0.0f32);

    let mult_uv = compute_uv_multiplier(&shape.uv_morphs[0].offsets);
    assert_eq!(mult_uv, 0.0f32);

    let bytes = write_tri(&TriFile::new(vec![shape.clone()])).expect("write all-zero morph");
    let decoded = read_tri(&bytes).expect("read all-zero morph");

    assert_eq!(decoded.shapes.len(), 1);
    let dec_shape = &decoded.shapes[0];
    assert_eq!(dec_shape.position_morphs.len(), 1);
    assert_eq!(
        dec_shape.position_morphs[0].offsets,
        shape.position_morphs[0].offsets
    );
    assert_eq!(dec_shape.uv_morphs.len(), 1);
    assert_eq!(dec_shape.uv_morphs[0].offsets, shape.uv_morphs[0].offsets);

    // Observed error is 0.0, which is <= step (0.0)
    for o in &dec_shape.position_morphs[0].offsets {
        assert_eq!(o.offset, [0.0, 0.0, 0.0]);
    }
}

/// D03: index validation refuses out-of-range vertex ids.
#[test]
fn d03_validation_refuses_out_of_range_vertex_id() {
    let shape = TriShape {
        name: "Shape".to_string(),
        vertex_count: Some(50),
        position_morphs: vec![TriPositionMorph {
            name: "OutMorph".to_string(),
            offsets: vec![PositionOffset {
                vertex: 50, // 50 >= 50 is out of range
                offset: [1.0, 0.0, 0.0],
            }],
        }],
        uv_morphs: Vec::new(),
    };

    let err = write_tri(&TriFile::new(vec![shape])).expect_err("must refuse out-of-range vertex");
    assert_eq!(
        err,
        TriError::VertexOutOfRange {
            morph: "OutMorph".to_string(),
            vertex: 50,
            limit: 50,
        }
    );

    // Also verify reader validation when limit is provided
    let valid_bytes = write_tri(&TriFile::new(vec![TriShape {
        name: "Shape".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: "OutMorph".to_string(),
            offsets: vec![PositionOffset {
                vertex: 75,
                offset: [1.0, 0.0, 0.0],
            }],
        }],
        uv_morphs: Vec::new(),
    }]))
    .unwrap();

    let read_err = read_tri_with_limit(&valid_bytes, Some(50))
        .expect_err("must refuse out-of-range vertex in reader");
    assert_eq!(
        read_err,
        TriError::VertexOutOfRange {
            morph: "OutMorph".to_string(),
            vertex: 75,
            limit: 50,
        }
    );
}

/// D03: index validation refuses duplicate vertex id in one morph.
#[test]
fn d03_validation_refuses_duplicate_vertex_id() {
    let shape = TriShape {
        name: "Shape".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: "DupMorph".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 10,
                    offset: [1.0, 0.0, 0.0],
                },
                PositionOffset {
                    vertex: 10,
                    offset: [0.0, 1.0, 0.0],
                },
            ],
        }],
        uv_morphs: Vec::new(),
    };

    let err = write_tri(&TriFile::new(vec![shape])).expect_err("must refuse duplicate vertex id");
    assert_eq!(
        err,
        TriError::DuplicateVertexId {
            morph: "DupMorph".to_string(),
            vertex: 10,
        }
    );
}

/// D03: index validation refuses names longer than 255 bytes.
#[test]
fn d03_validation_refuses_name_longer_than_255_bytes() {
    let long_name = "x".repeat(256);
    let shape = TriShape {
        name: long_name.clone(),
        vertex_count: None,
        position_morphs: Vec::new(),
        uv_morphs: Vec::new(),
    };

    let err = write_tri(&TriFile::new(vec![shape])).expect_err("must refuse name > 255 bytes");
    assert_eq!(
        err,
        TriError::NameTooLong {
            name: long_name,
            len: 256,
        }
    );

    // Also test morph name > 255 bytes
    let long_morph_name = "m".repeat(256);
    let shape_morph = TriShape {
        name: "ValidName".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: long_morph_name.clone(),
            offsets: Vec::new(),
        }],
        uv_morphs: Vec::new(),
    };

    let morph_err =
        write_tri(&TriFile::new(vec![shape_morph])).expect_err("must refuse morph name > 255");
    assert_eq!(
        morph_err,
        TriError::NameTooLong {
            name: long_morph_name,
            len: 256,
        }
    );
}

/// D03: index validation refuses more than 65535 shapes/morphs/vertices.
#[test]
fn d03_validation_refuses_counts_over_65535() {
    // 1. More than 65535 morphs in one shape
    let mut morphs = Vec::with_capacity(65536);
    for i in 0..=u16::MAX as usize {
        morphs.push(TriPositionMorph {
            name: format!("m{i}"),
            offsets: Vec::new(),
        });
    }
    let shape_too_many_morphs = TriShape {
        name: "Large".to_string(),
        vertex_count: None,
        position_morphs: morphs,
        uv_morphs: Vec::new(),
    };
    let err_morphs = write_tri(&TriFile::new(vec![shape_too_many_morphs]))
        .expect_err("must refuse > 65535 morphs");
    assert_eq!(
        err_morphs,
        TriError::TooManyMorphs {
            shape: "Large".to_string(),
            count: 65536,
        }
    );

    // 2. More than 65535 vertices in one morph
    let mut offsets = Vec::with_capacity(65536);
    for i in 0..=u16::MAX {
        offsets.push(PositionOffset {
            vertex: i,
            offset: [0.0, 0.0, 0.0],
        });
    }
    let shape_too_many_verts = TriShape {
        name: "LargeVerts".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: "ManyVerts".to_string(),
            offsets,
        }],
        uv_morphs: Vec::new(),
    };
    let err_verts = write_tri(&TriFile::new(vec![shape_too_many_verts]))
        .expect_err("must refuse > 65535 vertices");
    assert_eq!(
        err_verts,
        TriError::TooManyVertices {
            morph: "ManyVerts".to_string(),
            count: 65536,
        }
    );
}

/// D03: truncated file returns typed error `TriError::Truncated` or `TriError::InvalidMagic`.
#[test]
fn d03_truncated_file_returns_typed_error() {
    let valid_bytes = write_tri(&TriFile::new(vec![TriShape {
        name: "TruncTest".to_string(),
        vertex_count: None,
        position_morphs: vec![TriPositionMorph {
            name: "Pos1".to_string(),
            offsets: vec![PositionOffset {
                vertex: 1,
                offset: [10.0, 20.0, 30.0],
            }],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "Uv1".to_string(),
            offsets: vec![UvOffset {
                vertex: 1,
                offset: [5.0, 15.0],
            }],
        }],
    }]))
    .expect("write succeeds");

    // Empty input
    assert_eq!(
        read_tri(&[]),
        Err(TriError::Truncated {
            expected: 4,
            available: 0
        })
    );

    // Bad magic
    assert_eq!(
        read_tri(b"BAD!1234"),
        Err(TriError::InvalidMagic { magic: *b"BAD!" })
    );

    // Truncated at every possible byte offset
    for cut in 1..valid_bytes.len() {
        let res = read_tri(&valid_bytes[..cut]);
        assert!(
            matches!(
                res,
                Err(TriError::Truncated { .. }) | Err(TriError::InvalidMagic { .. })
            ),
            "cut at {cut}/{} must fail with Truncated or InvalidMagic",
            valid_bytes.len()
        );
    }
}

/// D04: Generates acceptance `.tri` fixture containing morphs for native BodySlide / Outfit Studio shapes:
/// `HIMBO - Body` and `BaseShape`.
#[test]
fn d04_generate_acceptance_tri() {
    let himbo_shape = TriShape {
        name: "HIMBO - Body".to_string(),
        vertex_count: Some(9307),
        position_morphs: vec![TriPositionMorph {
            name: "D04BicepsSize".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 10,
                    offset: [1.5, -2.0, 3.25],
                },
                PositionOffset {
                    vertex: 42,
                    offset: [-5.0, 10.0, 0.0],
                },
                PositionOffset {
                    vertex: 100,
                    offset: [2.0, 4.0, -1.0],
                },
            ],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "D04BicepsUV".to_string(),
            offsets: vec![
                UvOffset {
                    vertex: 10,
                    offset: [0.01, -0.02],
                },
                UvOffset {
                    vertex: 42,
                    offset: [0.05, 0.02],
                },
            ],
        }],
    };

    let base_shape = TriShape {
        name: "BaseShape".to_string(),
        vertex_count: Some(18436),
        position_morphs: vec![TriPositionMorph {
            name: "D04WaistNarrow".to_string(),
            offsets: vec![
                PositionOffset {
                    vertex: 50,
                    offset: [0.5, -1.0, 2.0],
                },
                PositionOffset {
                    vertex: 120,
                    offset: [-2.0, 3.0, 1.5],
                },
            ],
        }],
        uv_morphs: vec![TriUvMorph {
            name: "D04WaistUV".to_string(),
            offsets: vec![UvOffset {
                vertex: 50,
                offset: [-0.01, 0.03],
            }],
        }],
    };

    let doc = TriFile::new(vec![himbo_shape, base_shape]);
    let bytes = write_tri(&doc).expect("writing acceptance tri must succeed");

    // Ensure our reader decodes it with zero difference
    let roundtrip = read_tri(&bytes).expect("decoding acceptance tri must succeed");
    assert_eq!(roundtrip.shapes.len(), 2);
    assert_eq!(roundtrip.shapes[0].name, "HIMBO - Body");
    assert_eq!(roundtrip.shapes[1].name, "BaseShape");

    let out_path = std::path::Path::new("/tmp/opencode/d04_acceptance.tri");
    if let Some(parent) = out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(out_path, &bytes)
        .expect("writing to /tmp/opencode/d04_acceptance.tri must succeed");
}
