//! Domain errors → the stable `kind` + `code` contract.
//!
//! Every conversion is an exhaustive match, so a new variant in a domain crate
//! is a compile error here instead of a silent fallback to `other`. The `kind`
//! is the caller's decision input; `code` names the source variant and is part
//! of the wire contract.

use std::path::Path;

use crate::{CommandError, ErrorKind};
use sculpto_bake::BakeError;
use sculpto_core::{Error as DocumentError, ErrorKind as DocumentKind};
use sculpto_mesh::conform::ConformError;
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
        DocumentKind::InvalidValue => (ErrorKind::InvalidParams, "invalid_value"),
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

/// A NIF could not be written: the target or the model refused it.
pub fn nif_write(error: NifWriteError) -> CommandError {
    let (kind, code) = match &error {
        NifWriteError::UnsupportedGame { .. } => (ErrorKind::Unsupported, "nif_game_unsupported"),
        NifWriteError::UnsupportedVersion { .. } => {
            (ErrorKind::Unsupported, "nif_version_unsupported")
        }
        NifWriteError::TooShort => (ErrorKind::InvalidRequest, "nif_too_short"),
        NifWriteError::NotGamebryo => (ErrorKind::InvalidRequest, "nif_not_gamebryo"),
        NifWriteError::UnsupportedEndian { .. } => {
            (ErrorKind::Unsupported, "nif_endian_unsupported")
        }
        NifWriteError::InvalidModel(_) => (ErrorKind::InvalidParams, "nif_model_invalid"),
        NifWriteError::TooManyVertices { .. } => {
            (ErrorKind::InvalidParams, "nif_too_many_vertices")
        }
        NifWriteError::TooManyTriangles { .. } => {
            (ErrorKind::InvalidParams, "nif_too_many_triangles")
        }
        NifWriteError::TangentsWithoutNormals => {
            (ErrorKind::InvalidParams, "nif_tangents_without_normals")
        }
        NifWriteError::TangentsUnsupported { .. } => {
            (ErrorKind::Unsupported, "nif_tangents_unsupported")
        }
        NifWriteError::ZeroVector { .. } => (ErrorKind::InvalidParams, "nif_zero_vector"),
        NifWriteError::UnnamedBone { .. } => (ErrorKind::InvalidParams, "nif_unnamed_bone"),
        NifWriteError::DuplicateBone { .. } => (ErrorKind::InvalidParams, "nif_duplicate_bone"),
        NifWriteError::NameTooLong { .. } => (ErrorKind::InvalidParams, "nif_name_too_long"),
        NifWriteError::TooManyBones { .. } => (ErrorKind::InvalidParams, "nif_too_many_bones"),
        NifWriteError::SingularBind { .. } => (ErrorKind::InvalidParams, "nif_singular_bind"),
        NifWriteError::TextureSlotUnsupported { .. } => {
            (ErrorKind::Unsupported, "nif_texture_slot")
        }
        NifWriteError::MultipleMaterialSlots { .. } => {
            (ErrorKind::Unsupported, "nif_material_slots")
        }
        NifWriteError::InlinePhysicsUnsupported => (ErrorKind::Unsupported, "nif_inline_physics"),
    };
    CommandError::new(kind, code, error.to_string())
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

/// A conform bind or apply was refused.
pub fn conform(error: ConformError) -> CommandError {
    let (kind, code) = match &error {
        ConformError::EmptyGarment => (ErrorKind::InvalidParams, "empty_garment"),
        ConformError::EmptyReference => (ErrorKind::InvalidParams, "empty_reference"),
        ConformError::Topology { .. } => (ErrorKind::Refused, "topology_mismatch"),
    };
    CommandError::new(kind, code, error.to_string())
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
}
