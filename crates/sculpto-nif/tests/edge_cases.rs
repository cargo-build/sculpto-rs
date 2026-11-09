//! Edge case tests for NIF writers (epic task C16).
//!
//! Covers:
//! 1. Empty mesh refused (`NoVertices`).
//! 2. Single triangle legal round-trip.
//! 3. Repeated and degenerate triangle indices.
//! 4. Vertex count at 65 535 (legal) and 65 536 (refused as `TooManyVertices`).
//! 5. Triangle count above 65 535 (refused as `TooManyTriangles`).
//! 6. Shape names empty vs non-ASCII UTF-8 string table encoding.
//! 7. Shared shape and bone name deduplication in string table.
//! 8. Mesh without UVs.
//! 9. Mesh without normals (and negative control tangents without normals).
//! 10. Zero triangles and partition length validation.
//! 11. PhysicsRef empty values refused.

use sculpto_core::mesh::{MeshAsset, MeshError, PartitionId, PhysicsDefect, PhysicsRef, Skin};
use sculpto_nif::read;
use sculpto_nif::write::header::NifWriteError;
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;

fn minimal_valid_mesh() -> MeshAsset {
    MeshAsset {
        name: "TestShape".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

#[test]
fn empty_mesh_refused_as_no_vertices() {
    let empty = MeshAsset::default();
    assert_eq!(
        write_se_shape(&empty),
        Err(NifWriteError::InvalidModel(MeshError::NoVertices))
    );
    assert_eq!(
        write_le_shape(&empty),
        Err(NifWriteError::InvalidModel(MeshError::NoVertices))
    );

    // Negative control: adding a vertex moves past NoVertices.
    let single_vertex = MeshAsset {
        positions: vec![[0.0, 0.0, 0.0]],
        ..MeshAsset::default()
    };
    assert_ne!(
        write_se_shape(&single_vertex),
        Err(NifWriteError::InvalidModel(MeshError::NoVertices))
    );
    assert_ne!(
        write_le_shape(&single_vertex),
        Err(NifWriteError::InvalidModel(MeshError::NoVertices))
    );
}

#[test]
fn one_triangle_legal_roundtrip() {
    let mesh = minimal_valid_mesh();

    // SE writer
    let se_bytes = write_se_shape(&mesh).expect("SE single triangle writes");
    let se_doc = read(&se_bytes).expect("SE single triangle parses");
    assert_eq!(se_doc.shapes.len(), 1);
    assert_eq!(se_doc.shapes[0].positions, mesh.positions);
    assert_eq!(se_doc.shapes[0].triangles, vec![[0, 1, 2]]);
    assert_eq!(se_doc.shapes[0].name, "TestShape");

    // LE writer
    let le_bytes = write_le_shape(&mesh).expect("LE single triangle writes");
    let le_doc = read(&le_bytes).expect("LE single triangle parses");
    assert_eq!(le_doc.shapes.len(), 1);
    assert_eq!(le_doc.shapes[0].positions, mesh.positions);
    assert_eq!(le_doc.shapes[0].triangles, vec![[0, 1, 2]]);
    assert_eq!(le_doc.shapes[0].name, "TestShape");
}

#[test]
fn repeated_and_degenerate_indices_preserve_exact_topology() {
    let mut mesh = minimal_valid_mesh();
    // Degenerate triangle [0, 0, 1] followed by duplicate triangles [0, 1, 2]
    mesh.triangles = vec![[0, 0, 1], [0, 1, 2], [0, 1, 2]];
    mesh.triangle_partitions = vec![PartitionId(0); 3];

    let se_bytes = write_se_shape(&mesh).expect("SE writes degenerate indices");
    let se_doc = read(&se_bytes).expect("SE parses degenerate indices");
    assert_eq!(
        se_doc.shapes[0].triangles,
        vec![[0, 0, 1], [0, 1, 2], [0, 1, 2]]
    );

    let le_bytes = write_le_shape(&mesh).expect("LE writes degenerate indices");
    let le_doc = read(&le_bytes).expect("LE parses degenerate indices");
    assert_eq!(
        le_doc.shapes[0].triangles,
        vec![[0, 0, 1], [0, 1, 2], [0, 1, 2]]
    );
}

#[test]
fn vertex_count_boundary_at_65535_and_65536() {
    let mesh_65535 = MeshAsset {
        name: "BoundaryVertices".into(),
        positions: vec![[1.0, 2.0, 3.0]; 65_535],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    };

    // 65 535 vertices is legal for u16 vertex indices
    let se_bytes = write_se_shape(&mesh_65535).expect("SE writes 65535 vertices");
    let se_doc = read(&se_bytes).expect("SE parses 65535 vertices");
    assert_eq!(se_doc.shapes[0].positions.len(), 65_535);

    let le_bytes = write_le_shape(&mesh_65535).expect("LE writes 65535 vertices");
    let le_doc = read(&le_bytes).expect("LE parses 65535 vertices");
    assert_eq!(le_doc.shapes[0].positions.len(), 65_535);

    // 65 536 vertices exceeds u16::MAX
    let mut mesh_65536 = mesh_65535.clone();
    mesh_65536.positions.push([4.0, 5.0, 6.0]);
    assert_eq!(
        write_se_shape(&mesh_65536),
        Err(NifWriteError::TooManyVertices { vertices: 65_536 })
    );
    assert_eq!(
        write_le_shape(&mesh_65536),
        Err(NifWriteError::TooManyVertices { vertices: 65_536 })
    );

    // Negative control: ensure it fails for TooManyVertices, not an unaligned attribute error
    mesh_65536.normals = vec![[0.0, 0.0, 1.0]; 65_536];
    assert_eq!(
        write_se_shape(&mesh_65536),
        Err(NifWriteError::TooManyVertices { vertices: 65_536 })
    );
}

#[test]
fn triangle_count_boundary_above_65535_refused() {
    let mut mesh = MeshAsset {
        name: "BoundaryTriangles".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]; 65_536],
        triangle_partitions: vec![PartitionId(0); 65_536],
        ..MeshAsset::default()
    };

    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::TooManyTriangles { triangles: 65_536 })
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::TooManyTriangles { triangles: 65_536 })
    );

    // Negative control: 65 535 triangles fits within u16
    mesh.triangles.pop();
    mesh.triangle_partitions.pop();
    assert_eq!(mesh.triangles.len(), 65_535);
    assert!(write_se_shape(&mesh).is_ok());
    assert!(write_le_shape(&mesh).is_ok());
}

#[test]
fn shape_names_empty_vs_non_ascii_utf8_string_table_bytes() {
    // Case A: empty shape name
    let mut empty_name_mesh = minimal_valid_mesh();
    empty_name_mesh.name = String::new();

    let se_empty = write_se_shape(&empty_name_mesh).expect("SE empty name writes");
    let se_doc = read(&se_empty).expect("SE empty name parses");
    assert_eq!(se_doc.shapes[0].name, "");

    let le_empty = write_le_shape(&empty_name_mesh).expect("LE empty name writes");
    let le_doc = read(&le_empty).expect("LE empty name parses");
    assert_eq!(le_doc.shapes[0].name, "");

    // When shape name is empty, string table contains only index 0 ("")
    // Check header string table: num_strings = 1, max_string_length = 0
    // String table layout: u32 num_strings, u32 max_string_length, then each string (u32 len + bytes)
    let string_table_empty: [u8; 12] = [
        1, 0, 0, 0, // num_strings: 1
        0, 0, 0, 0, // max_string_length: 0
        0, 0, 0, 0, // string 0 len: 0
    ];
    assert!(
        se_empty.windows(12).any(|w| w == string_table_empty),
        "SE empty string table marker missing"
    );
    assert!(
        le_empty.windows(12).any(|w| w == string_table_empty),
        "LE empty string table marker missing"
    );

    // Case B: non-ASCII UTF-8 name (multi-byte UTF-8 bytes)
    let non_ascii_name = "Schöne_Тело_キャラクター";
    let mut utf8_mesh = minimal_valid_mesh();
    utf8_mesh.name = non_ascii_name.into();

    let se_utf8 = write_se_shape(&utf8_mesh).expect("SE UTF-8 name writes");
    let se_doc = read(&se_utf8).expect("SE UTF-8 name parses");
    assert_eq!(se_doc.shapes[0].name, non_ascii_name);

    let le_utf8 = write_le_shape(&utf8_mesh).expect("LE UTF-8 name writes");
    let le_doc = read(&le_utf8).expect("LE UTF-8 name parses");
    assert_eq!(le_doc.shapes[0].name, non_ascii_name);

    // Verify raw UTF-8 bytes in the wire byte stream
    let raw_bytes = non_ascii_name.as_bytes();
    assert!(
        se_utf8.windows(raw_bytes.len()).any(|w| w == raw_bytes),
        "SE missing UTF-8 string bytes"
    );
    assert!(
        le_utf8.windows(raw_bytes.len()).any(|w| w == raw_bytes),
        "LE missing UTF-8 string bytes"
    );
}

#[test]
fn shared_shape_and_bone_name_deduplication() {
    let mut mesh = minimal_valid_mesh();
    mesh.name = "Root".into();
    mesh.skin = Some(Skin {
        bones: vec!["Root".into(), "Spine".into()],
        bind_inverse: vec![
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        ],
        indices: vec![[0, 1, 0, 0]; 3],
        weights: vec![[0.75, 0.25, 0.0, 0.0]; 3],
    });

    let se_bytes = write_se_shape(&mesh).expect("SE writes shared name");
    let se_doc = read(&se_bytes).expect("SE parses shared name");
    assert_eq!(se_doc.shapes[0].name, "Root");
    assert_eq!(se_doc.shapes[0].bones, vec!["Root", "Spine"]);

    let le_bytes = write_le_shape(&mesh).expect("LE writes shared name");
    let le_doc = read(&le_bytes).expect("LE parses shared name");
    assert_eq!(le_doc.shapes[0].name, "Root");
    assert_eq!(le_doc.shapes[0].bones, vec!["Root", "Spine"]);

    // Deduplication check: string table contains ["", "Root", "Spine"], total 3 entries
    // Without dedup, it would contain ["", "Root", "Root", "Spine"] (4 entries).
    // String table header has num_strings = 3 (0x03, 0x00, 0x00, 0x00)
    let string_table_header: [u8; 8] = [
        3, 0, 0, 0, // num_strings = 3
        5, 0, 0, 0, // max_string_length = 5 ("Spine")
    ];
    assert!(
        se_bytes.windows(8).any(|w| w == string_table_header),
        "SE string table does not deduplicate shared name"
    );
    assert!(
        le_bytes.windows(8).any(|w| w == string_table_header),
        "LE string table does not deduplicate shared name"
    );

    // Negative control: duplicate bone within the skin palette itself is refused
    let mut dup_bone_mesh = mesh.clone();
    dup_bone_mesh.skin.as_mut().unwrap().bones = vec!["Spine".into(), "Spine".into()];
    assert_eq!(
        write_se_shape(&dup_bone_mesh),
        Err(NifWriteError::DuplicateBone {
            name: "Spine".into()
        })
    );
    assert_eq!(
        write_le_shape(&dup_bone_mesh),
        Err(NifWriteError::DuplicateBone {
            name: "Spine".into()
        })
    );
}

#[test]
fn mesh_without_uvs_roundtrips_empty() {
    let mut mesh = minimal_valid_mesh();
    mesh.uvs.clear();

    let se_bytes = write_se_shape(&mesh).expect("SE writes without UVs");
    let se_doc = read(&se_bytes).expect("SE parses without UVs");
    assert!(se_doc.shapes[0].uvs.is_empty());
    assert_eq!(se_doc.shapes[0].positions, mesh.positions);
    assert_eq!(se_doc.shapes[0].triangles, vec![[0, 1, 2]]);

    let le_bytes = write_le_shape(&mesh).expect("LE writes without UVs");
    let le_doc = read(&le_bytes).expect("LE parses without UVs");
    assert!(le_doc.shapes[0].uvs.is_empty());
    assert_eq!(le_doc.shapes[0].positions, mesh.positions);
    assert_eq!(le_doc.shapes[0].triangles, vec![[0, 1, 2]]);

    // Negative control: UV length mismatch is refused
    let mut bad_uvs = mesh.clone();
    bad_uvs.uvs = vec![[0.0, 0.0]]; // 1 UV for 3 positions
    assert_eq!(
        write_se_shape(&bad_uvs),
        Err(NifWriteError::InvalidModel(
            MeshError::AttributeLengthMismatch { attribute: "uvs" }
        ))
    );
    assert_eq!(
        write_le_shape(&bad_uvs),
        Err(NifWriteError::InvalidModel(
            MeshError::AttributeLengthMismatch { attribute: "uvs" }
        ))
    );
}

#[test]
fn mesh_without_normals_roundtrips_empty() {
    let mut mesh = minimal_valid_mesh();
    mesh.normals.clear();
    mesh.tangents.clear();

    // LE writer explicitly serializes has_normals = 0 in NiTriShapeData.
    // The downstream byroredux importer synthesizes unit normals from triangle geometry (#3541)
    // when normals are absent on disk, ensuring renderer safety without authored normals.
    let le_bytes = write_le_shape(&mesh).expect("LE writes without normals");
    let le_doc = read(&le_bytes).expect("LE parses without normals");
    assert_eq!(le_doc.shapes[0].normals.len(), 3);
    for normal in &le_doc.shapes[0].normals {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!(
            (length - 1.0).abs() <= 1e-4,
            "LE synthesized normal is unit length"
        );
    }
    assert_eq!(le_doc.shapes[0].positions, mesh.positions);

    // SE writer clears VF_NORMALS (0x008) from vertex_desc so the normal lane is not stored on disk.
    // The downstream byroredux importer synthesizes unit normals from triangle geometry (#3541)
    // when VF_NORMALS is absent.
    let se_bytes = write_se_shape(&mesh).expect("SE writes without normals");
    let se_doc = read(&se_bytes).expect("SE parses without normals");
    assert_eq!(se_doc.shapes[0].normals.len(), 3);
    for normal in &se_doc.shapes[0].normals {
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        assert!(
            (length - 1.0).abs() <= 1e-4,
            "SE synthesized normal is unit length"
        );
    }
    assert_eq!(se_doc.shapes[0].positions, mesh.positions);

    // Negative control: tangents without normals is refused by SE writer
    let mut bad_tangents = mesh.clone();
    bad_tangents.tangents = vec![[1.0, 0.0, 0.0, 1.0]; 3];
    assert_eq!(
        write_se_shape(&bad_tangents),
        Err(NifWriteError::TangentsWithoutNormals)
    );

    // Negative control: normal length mismatch is refused
    let mut bad_normals = mesh.clone();
    bad_normals.normals = vec![[0.0, 0.0, 1.0]; 1]; // 1 normal for 3 positions
    assert_eq!(
        write_se_shape(&bad_normals),
        Err(NifWriteError::InvalidModel(
            MeshError::AttributeLengthMismatch {
                attribute: "normals"
            }
        ))
    );
}

#[test]
fn zero_triangles_validation_and_refusal() {
    let mut mesh = minimal_valid_mesh();
    mesh.triangles.clear();
    // Leaving triangle_partitions unaligned triggers PartitionLengthMismatch in model validation
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::InvalidModel(
            MeshError::PartitionLengthMismatch
        ))
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::InvalidModel(
            MeshError::PartitionLengthMismatch
        ))
    );

    // When both triangles and partitions are empty, the writers emit the zero-triangle block.
    // The reader intentionally drops zero-triangle shapes (extract_bs_tri_shape / extract_mesh return None)
    // because a triangle-free shape has no renderable surface.
    mesh.triangle_partitions.clear();
    let se_bytes = write_se_shape(&mesh).expect("SE writes zero triangles");
    let se_doc = read(&se_bytes).expect("SE parses zero triangles");
    assert!(
        se_doc.shapes.is_empty(),
        "SE reader drops triangle-free shape"
    );

    let le_bytes = write_le_shape(&mesh).expect("LE writes zero triangles");
    let le_doc = read(&le_bytes).expect("LE parses zero triangles");
    assert!(
        le_doc.shapes.is_empty(),
        "LE reader drops triangle-free shape"
    );
}

#[test]
fn physics_ref_empty_values_refused() {
    let mut mesh = minimal_valid_mesh();

    // Empty path
    mesh.physics = Some(PhysicsRef::Path("".into()));
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyPath,
        }))
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyPath,
        }))
    );

    // Whitespace path
    mesh.physics = Some(PhysicsRef::Path("   \t\n  ".into()));
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyPath,
        }))
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyPath,
        }))
    );

    // Empty inline document
    mesh.physics = Some(PhysicsRef::Inline("".into()));
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyInline,
        }))
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyInline,
        }))
    );

    // Whitespace inline document
    mesh.physics = Some(PhysicsRef::Inline("  \r\n \t ".into()));
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyInline,
        }))
    );
    assert_eq!(
        write_le_shape(&mesh),
        Err(NifWriteError::InvalidModel(MeshError::Physics {
            defect: PhysicsDefect::EmptyInline,
        }))
    );

    // Negative control: valid non-empty physics references pass validation and write
    mesh.physics = Some(PhysicsRef::Path("physics/body.xml".into()));
    assert!(write_se_shape(&mesh).is_ok());
    assert!(write_le_shape(&mesh).is_ok());

    // C11 contract: an inline physics document has no file link in a NIF.
    mesh.physics = Some(PhysicsRef::Inline("<xml>valid</xml>".into()));
    assert!(matches!(
        write_se_shape(&mesh),
        Err(NifWriteError::InlinePhysicsUnsupported)
    ));
    assert!(matches!(
        write_le_shape(&mesh),
        Err(NifWriteError::InlinePhysicsUnsupported)
    ));
}
