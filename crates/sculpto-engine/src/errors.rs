//! Domain errors → the stable `kind` + `code` contract.
//!
//! Every conversion is an exhaustive match, so a new variant in a domain crate
//! is a compile error here instead of a silent fallback to `other`. The `kind`
//! is the caller's decision input; `code` names the source variant and is part
//! of the wire contract.

use std::path::Path;

use crate::{CommandError, ErrorKind};
use sculpto_bake::BakeError;
use sculpto_core::mesh::MeshError;
use sculpto_core::{Error as DocumentError, ErrorKind as DocumentKind};
use sculpto_gltf::sanitize::SanitizeError;
use sculpto_mesh::conform::ConformError;
use sculpto_mesh::garment::GarmentRuleError;
use sculpto_mesh::intake_gltf::{DefectOverLimit, IntakeError};
use sculpto_nif::NifError;
use sculpto_nif::write::header::NifWriteError;

/// A file-system failure, with the path in the message.
pub fn io(path: &Path, error: std::io::Error) -> CommandError {
    CommandError::new(
        ErrorKind::Io,
        "io_error",
        format!("{}: {error}", path.display()),
    )
}

/// A document could not be read or does not have the expected shape.
pub fn document(error: DocumentError) -> CommandError {
    let (kind, code) = match error.kind() {
        DocumentKind::Io => (ErrorKind::Io, "document_io"),
        DocumentKind::Malformed => (ErrorKind::InvalidRequest, "document_malformed"),
        DocumentKind::UnsupportedRoot => (ErrorKind::Unsupported, "unsupported_root"),
        DocumentKind::UnsupportedVersion => (ErrorKind::Unsupported, "unsupported_version"),
        DocumentKind::MissingRequired => (ErrorKind::InvalidRequest, "missing_required"),
        // A rejected value inside a parsed document, not a bad parameter of the
        // request: the `document_` prefix keeps the code distinct from
        // `errors::invalid`. @req DECL0007
        DocumentKind::InvalidValue => (ErrorKind::InvalidParams, "document_invalid_value"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A NIF could not be decoded.
pub fn nif(error: NifError) -> CommandError {
    let (kind, code) = match &error {
        NifError::TooShort => (ErrorKind::InvalidRequest, "nif_too_short"),
        NifError::Malformed(_) => (ErrorKind::InvalidRequest, "nif_malformed"),
        NifError::Truncated { .. } => (ErrorKind::Refused, "nif_truncated"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A NIF writer refused the model.
pub fn nif_write(error: NifWriteError) -> CommandError {
    let (kind, code) = match &error {
        NifWriteError::UnsupportedGame { .. } => {
            (ErrorKind::Unsupported, "nif_write_unsupported_game")
        }
        NifWriteError::UnsupportedVersion { .. } => {
            (ErrorKind::Unsupported, "nif_write_unsupported_version")
        }
        NifWriteError::TooShort => (ErrorKind::InvalidRequest, "nif_write_too_short"),
        NifWriteError::NotGamebryo => (ErrorKind::InvalidRequest, "nif_write_not_gamebryo"),
        NifWriteError::UnsupportedEndian { .. } => {
            (ErrorKind::Unsupported, "nif_write_unsupported_endian")
        }
        NifWriteError::InvalidModel(_) => (ErrorKind::Refused, "nif_write_invalid_model"),
        NifWriteError::TooManyVertices { .. } => {
            (ErrorKind::Unsupported, "nif_write_too_many_vertices")
        }
        NifWriteError::TooManyTriangles { .. } => {
            (ErrorKind::Unsupported, "nif_write_too_many_triangles")
        }
        NifWriteError::TangentsWithoutNormals => {
            (ErrorKind::Refused, "nif_write_tangents_without_normals")
        }
        NifWriteError::TangentsUnsupported { .. } => {
            (ErrorKind::Refused, "nif_write_tangents_unsupported")
        }
        NifWriteError::ZeroVector { .. } => (ErrorKind::Refused, "nif_write_zero_vector"),
        NifWriteError::UnnamedBone { .. } => (ErrorKind::Refused, "nif_write_unnamed_bone"),
        NifWriteError::DuplicateBone { .. } => (ErrorKind::Refused, "nif_write_duplicate_bone"),
        NifWriteError::NameTooLong { .. } => (ErrorKind::Refused, "nif_write_name_too_long"),
        NifWriteError::TooManyBones { .. } => (ErrorKind::Refused, "nif_write_too_many_bones"),
        NifWriteError::TooManyInfluences { .. } => {
            (ErrorKind::Refused, "nif_write_too_many_influences")
        }
        NifWriteError::WeightUnrepresentable { .. } => {
            (ErrorKind::Refused, "nif_write_weight_unrepresentable")
        }
        NifWriteError::SingularBind { .. } => (ErrorKind::Refused, "nif_write_singular_bind"),
        NifWriteError::TextureSlotUnsupported { .. } => {
            (ErrorKind::Refused, "nif_write_texture_slot_unsupported")
        }
        NifWriteError::MultipleMaterialSlots { .. } => {
            (ErrorKind::Refused, "nif_write_multiple_material_slots")
        }
        NifWriteError::InlinePhysicsUnsupported => {
            (ErrorKind::Refused, "nif_write_inline_physics_unsupported")
        }
        NifWriteError::GlobalVertexMismatch { .. } => {
            (ErrorKind::Refused, "nif_write_global_vertex_mismatch")
        }
    };
    CommandError::new(kind, code, error.to_string())
}

/// A model failed validation before a writer could produce bytes.
pub fn mesh(error: MeshError) -> CommandError {
    let (kind, code) = match &error {
        MeshError::NoVertices => (ErrorKind::InvalidRequest, "mesh_no_vertices"),
        MeshError::AttributeLengthMismatch { .. } => {
            (ErrorKind::InvalidRequest, "mesh_attribute_length")
        }
        MeshError::PartitionLengthMismatch => (ErrorKind::InvalidRequest, "mesh_partition_length"),
        MeshError::TriangleVertexOutOfRange { .. } => {
            (ErrorKind::InvalidRequest, "mesh_triangle_range")
        }
        MeshError::MorphVertexOutOfRange { .. } => (ErrorKind::InvalidRequest, "mesh_morph_range"),
        MeshError::MorphDeltasNotSorted { .. } => (ErrorKind::InvalidRequest, "mesh_morph_order"),
        MeshError::MorphUnknownPart { .. } => (ErrorKind::InvalidRequest, "mesh_morph_part"),
        MeshError::NonFinite { .. } => (ErrorKind::InvalidRequest, "mesh_non_finite"),
        MeshError::SkinLengthMismatch => (ErrorKind::InvalidRequest, "mesh_skin_length"),
        MeshError::BoneIndexOutOfRange { .. } => (ErrorKind::InvalidRequest, "mesh_bone_range"),
        MeshError::WeightSumOutOfRange { .. } => (ErrorKind::InvalidRequest, "mesh_weight_sum"),
        MeshError::ZeroWeightSum { .. } => (ErrorKind::InvalidRequest, "mesh_zero_weight_sum"),
        MeshError::BindLengthMismatch { .. } => (ErrorKind::InvalidRequest, "mesh_bind_length"),
        MeshError::NonFiniteBind { .. } => (ErrorKind::InvalidRequest, "mesh_non_finite_bind"),
        MeshError::MissingPartitionPalette { .. } => {
            (ErrorKind::InvalidRequest, "mesh_missing_palette")
        }
        MeshError::PaletteMissingBone { .. } => (ErrorKind::InvalidRequest, "mesh_palette_bone"),
        MeshError::DuplicatePartitionPalette { .. } => {
            (ErrorKind::InvalidRequest, "mesh_duplicate_palette")
        }
        MeshError::UnmatchedPartition { .. } => {
            (ErrorKind::InvalidRequest, "mesh_unmatched_partition")
        }
        MeshError::Material { .. } => (ErrorKind::InvalidRequest, "mesh_material"),
        MeshError::Physics { .. } => (ErrorKind::InvalidRequest, "mesh_physics"),
    };
    CommandError::new(kind, code, format!("{error:?}"))
}

/// A bake plan could not be resolved or applied.
pub fn bake(error: BakeError) -> CommandError {
    let (kind, code) = match &error {
        BakeError::SetNotResolved { .. } => (ErrorKind::NotFound, "set_not_resolved"),
        BakeError::Container { .. } => (ErrorKind::InvalidRequest, "slider_container"),
        BakeError::IndexOutOfBounds { .. } => (ErrorKind::InvalidParams, "index_out_of_bounds"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A GLB container could not be repaired.
pub fn gltf(error: SanitizeError) -> CommandError {
    let (kind, code) = match &error {
        SanitizeError::TooShort { .. } => (ErrorKind::InvalidRequest, "glb_too_short"),
        SanitizeError::NotGlb { .. } => (ErrorKind::InvalidRequest, "glb_not_glb"),
        SanitizeError::FirstChunkNotJson { .. } => {
            (ErrorKind::InvalidRequest, "glb_first_chunk_not_json")
        }
        SanitizeError::JsonTruncated { .. } => (ErrorKind::Refused, "glb_json_truncated"),
        SanitizeError::JsonInvalid(_) => (ErrorKind::InvalidRequest, "glb_json_invalid"),
        SanitizeError::JsonNotObject => (ErrorKind::InvalidRequest, "glb_json_not_object"),
        SanitizeError::TooLarge { .. } => (ErrorKind::Unsupported, "glb_too_large"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A conform bind or apply was refused.
pub fn conform(error: ConformError) -> CommandError {
    let (kind, code) = match &error {
        ConformError::EmptyGarment => (ErrorKind::InvalidParams, "empty_garment"),
        ConformError::EmptyReference => (ErrorKind::InvalidParams, "empty_reference"),
        ConformError::Topology { .. } => (ErrorKind::Refused, "topology_mismatch"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A raw garment export could not be read.
pub fn intake(error: IntakeError) -> CommandError {
    let (kind, code) = match &error {
        IntakeError::Io { .. } => (ErrorKind::Io, "intake_io"),
        IntakeError::UnsupportedExtension { .. } => (ErrorKind::InvalidParams, "intake_extension"),
        IntakeError::Empty { .. } => (ErrorKind::InvalidRequest, "intake_empty"),
        IntakeError::Decode { .. } => (ErrorKind::InvalidRequest, "intake_decode"),
        IntakeError::MissingVertices { .. } => (ErrorKind::InvalidRequest, "intake_no_vertices"),
        IntakeError::MissingIndices { .. } => (ErrorKind::InvalidRequest, "intake_no_indices"),
        // The one-garment contract is a domain rule, not a parse failure.
        IntakeError::MultipleMeshes { .. } => (ErrorKind::Refused, "intake_multiple_meshes"),
        IntakeError::Rigged { .. } => (ErrorKind::Refused, "intake_rigged"),
        IntakeError::TooManyMaterials { .. } => (ErrorKind::Refused, "intake_too_many_materials"),
        IntakeError::UnsupportedBufferUri { .. } => (ErrorKind::Unsupported, "intake_buffer_uri"),
        IntakeError::FaceIndexOutOfRange { .. } => (ErrorKind::InvalidRequest, "intake_face_range"),
        IntakeError::Malformed { .. } => (ErrorKind::InvalidRequest, "intake_malformed"),
    };
    CommandError::new(kind, code, error.to_string())
}

/// A garment's measured defects exceed the declared limits.
pub fn intake_defects(error: DefectOverLimit) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        "intake_defects_over_limit",
        error.to_string(),
    )
}

/// A value the caller supplied is outside the accepted set.
pub fn invalid(field: &str, detail: impl Into<String>) -> CommandError {
    CommandError::new(
        ErrorKind::InvalidParams,
        "invalid_value",
        format!("`{field}`: {}", detail.into()),
    )
}

/// A named object does not exist.
pub fn not_found(what: &str, name: &str) -> CommandError {
    CommandError::new(
        ErrorKind::NotFound,
        "not_found",
        format!("{what} `{name}` not found"),
    )
}

/// A garment rule could not answer.
pub fn garment_rule(error: GarmentRuleError) -> CommandError {
    let (kind, code) = match &error {
        GarmentRuleError::EmptyMesh => (ErrorKind::InvalidParams, "garment_empty_mesh"),
        GarmentRuleError::FaceOutOfRange { .. } => {
            (ErrorKind::InvalidParams, "garment_face_out_of_range")
        }
        GarmentRuleError::DegenerateRaw => (ErrorKind::Refused, "garment_degenerate_raw"),
        GarmentRuleError::MissingParameter(_) => (ErrorKind::Refused, "garment_missing_parameter"),
        GarmentRuleError::NoNearestVertex => (ErrorKind::Refused, "garment_no_nearest"),
    };
    CommandError::new(kind, code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_core::mesh::MeshError;
    use sculpto_nif::write::header::NifTarget;

    #[test]
    fn every_bake_variant_has_its_own_code() {
        let cases = [
            BakeError::SetNotResolved { name: "x".into() },
            BakeError::Container {
                path: "p".into(),
                source: DocumentError::new(DocumentKind::Malformed, "e", "d"),
            },
            BakeError::IndexOutOfBounds {
                slider: "s".into(),
                index: 1,
                vertices: 0,
            },
        ];
        let mut codes = Vec::new();
        for error in cases {
            let mapped = bake(error);
            assert!(!mapped.code().is_empty());
            codes.push(mapped.code());
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), 3, "bake variants must map to distinct codes");
    }

    #[test]
    fn every_nif_write_variant_has_its_own_code() {
        let cases = [
            NifWriteError::UnsupportedGame { user_version: 1 },
            NifWriteError::UnsupportedVersion { version: 1 },
            NifWriteError::TooShort,
            NifWriteError::NotGamebryo,
            NifWriteError::UnsupportedEndian { endian: 0 },
            NifWriteError::InvalidModel(MeshError::NoVertices),
            NifWriteError::TooManyVertices { vertices: 1 },
            NifWriteError::TooManyTriangles { triangles: 1 },
            NifWriteError::TangentsWithoutNormals,
            NifWriteError::TangentsUnsupported {
                target: NifTarget::Le,
            },
            NifWriteError::ZeroVector {
                attribute: "normal",
                vertex: 0,
            },
            NifWriteError::UnnamedBone { index: 0 },
            NifWriteError::DuplicateBone {
                name: "b".to_string(),
            },
            NifWriteError::NameTooLong {
                name: "b".to_string(),
            },
            NifWriteError::TooManyBones { bones: 1 },
            NifWriteError::SingularBind { bone: 0 },
            NifWriteError::TextureSlotUnsupported { slot: "specular" },
            NifWriteError::MultipleMaterialSlots { count: 2 },
            NifWriteError::InlinePhysicsUnsupported,
        ];
        let mut codes = Vec::new();
        for error in cases {
            let mapped = nif_write(error);
            assert!(!mapped.code().is_empty());
            codes.push(mapped.code());
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            19,
            "NifWriteError variants must map to distinct codes"
        );
    }

    #[test]
    fn every_conform_variant_has_its_own_code() {
        let cases = [
            ConformError::EmptyGarment,
            ConformError::EmptyReference,
            ConformError::Topology {
                expected: 2,
                actual: 1,
            },
        ];
        let mut codes = Vec::new();
        for error in cases {
            let mapped = conform(error);
            codes.push(mapped.code());
        }
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            codes.len(),
            3,
            "conform variants must map to distinct codes"
        );
    }

    #[test]
    fn every_intake_variant_has_its_own_code() {
        use sculpto_mesh::intake_gltf::IntakeError;
        let cases = [
            IntakeError::Io {
                path: "p".into(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "x"),
            },
            IntakeError::UnsupportedExtension {
                extension: "fbx".into(),
            },
            IntakeError::Empty { path: "p".into() },
            IntakeError::Decode {
                format: "glTF",
                detail: "d".into(),
            },
            IntakeError::MissingVertices { path: "p".into() },
            IntakeError::MissingIndices { mesh: 0 },
            IntakeError::MultipleMeshes { count: 2 },
            IntakeError::Rigged { skins: 1 },
            IntakeError::TooManyMaterials { count: 2 },
            IntakeError::UnsupportedBufferUri {
                uri: "data:".into(),
            },
            IntakeError::FaceIndexOutOfRange { face: 1, vertex: 9 },
            IntakeError::Malformed {
                format: "OBJ",
                line: 1,
                detail: "d".into(),
            },
        ];
        let mut codes = Vec::new();
        for error in cases {
            let mapped = intake(error);
            assert!(!mapped.code().is_empty());
            codes.push(mapped.code());
        }
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(
            count,
            codes.len(),
            "IntakeError variants must stay distinct"
        );
    }

    #[test]
    fn an_over_limit_garment_is_a_refusal() {
        let refused = intake_defects(DefectOverLimit {
            counts: sculpto_mesh::intake_gltf::TopologyDefects {
                degenerate_triangles: 1,
                ..Default::default()
            },
            limits: sculpto_mesh::intake_gltf::DefectLimits::default(),
        });
        assert_eq!(refused.kind(), ErrorKind::Refused);
        assert_eq!(refused.code(), "intake_defects_over_limit");
        assert!(
            refused.message().contains("degenerate_triangles 1 > 0"),
            "the refusal lists the measured count and the limit: {}",
            refused.message()
        );
    }

    #[test]
    fn truncation_is_a_refusal_not_a_parse_error() {
        let refused = nif(NifError::Truncated { dropped: 2 });
        let malformed = nif(NifError::Malformed("x".into()));
        assert_ne!(refused.kind(), malformed.kind());
        assert_eq!(refused.kind(), ErrorKind::Refused);
        assert_eq!(malformed.kind(), ErrorKind::InvalidRequest);
    }

    #[test]
    fn document_kinds_are_not_collapsed() {
        let malformed = document(DocumentError::new(DocumentKind::Malformed, "e", "d"));
        let unsupported = document(DocumentError::new(
            DocumentKind::UnsupportedVersion,
            "e",
            "d",
        ));
        assert_eq!(malformed.kind(), ErrorKind::InvalidRequest);
        assert_eq!(unsupported.kind(), ErrorKind::Unsupported);
        assert_ne!(malformed.code(), unsupported.code());
    }

    #[test]
    fn every_document_kind_has_its_own_code() {
        // `InvalidValue` shared the `invalid_value` code with `errors::invalid`
        // until the CMD_GATE registry test caught it; this pins the split at
        // runtime, next to the mapping that had it. @req DECL0007
        let cases = [
            DocumentKind::Io,
            DocumentKind::Malformed,
            DocumentKind::UnsupportedRoot,
            DocumentKind::UnsupportedVersion,
            DocumentKind::MissingRequired,
            DocumentKind::InvalidValue,
        ];
        let mut codes = Vec::new();
        for kind in cases {
            codes.push(document(DocumentError::new(kind, "e", "d")).code());
        }
        assert_eq!(
            document(DocumentError::new(DocumentKind::InvalidValue, "e", "d")).code(),
            "document_invalid_value"
        );
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(count, codes.len(), "document kinds must stay distinct");
    }

    #[test]
    fn write_refusals_carry_their_own_codes() {
        let invalid = nif_write(NifWriteError::InvalidModel(MeshError::NoVertices));
        let tangents = nif_write(NifWriteError::TangentsUnsupported {
            target: NifTarget::Le,
        });
        assert_eq!(invalid.kind(), ErrorKind::Refused);
        assert_ne!(invalid.code(), tangents.code());
        assert_ne!(invalid.code(), nif(NifError::TooShort).code());
        assert_eq!(invalid.code(), "nif_write_invalid_model");
    }
}
