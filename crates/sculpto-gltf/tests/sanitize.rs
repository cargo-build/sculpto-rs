//! The two GLB repairs on a synthetic container: what the `gltf` crate reads
//! back, the untouched binary chunk, and the byte-level no-op on a re-run.

mod common;

use sculpto_gltf::sanitize::{SanitizeError, clamp_texcoord, reset_mesh_weights};
use serde_json::json;

#[test]
fn reset_weights_zeroes_every_default_and_keeps_the_target_count() {
    let bytes = common::defective();
    let repaired = reset_mesh_weights(&bytes).expect("the container is a GLB");
    assert_eq!(repaired.changed, 1, "one mesh carries defaults");
    let rewritten = repaired.bytes.expect("the weights changed");

    let before = common::json_chunk(&bytes);
    let after = common::json_chunk(&rewritten);
    assert_eq!(before["meshes"][0]["weights"], json!([0.4, 0.25]));
    assert_eq!(after["meshes"][0]["weights"], json!([0.0, 0.0]));
    assert_eq!(
        after["meshes"][0]["weights"]
            .as_array()
            .expect("an array")
            .len(),
        before["meshes"][0]["weights"]
            .as_array()
            .expect("an array")
            .len(),
        "the target count is preserved"
    );

    // The binary chunk is copied, not rebuilt, and the length field is honest.
    assert_eq!(
        common::binary_chunk(&bytes),
        common::binary_chunk(&rewritten)
    );
    assert_eq!(
        gltf::binary::Glb::from_slice(&rewritten)
            .expect("container")
            .header
            .length as usize,
        rewritten.len()
    );
}

#[test]
fn a_second_reset_changes_nothing() {
    let once = reset_mesh_weights(&common::defective())
        .expect("repair")
        .bytes
        .expect("the first pass changed the document");
    let twice = reset_mesh_weights(&once).expect("the repaired container is a GLB");
    assert_eq!(twice.changed, 0);
    assert_eq!(twice.bytes, None, "an untouched document keeps its bytes");
}

#[test]
fn clamp_moves_both_negative_sets_to_zero_and_leaves_the_legal_one() {
    let bytes = common::defective();
    // The defect is real: a reader refuses a negative set index.
    assert!(
        gltf::Gltf::from_slice(&bytes).is_err(),
        "`texCoord: -1` is not loadable"
    );

    let repaired = clamp_texcoord(&bytes).expect("the container is a GLB");
    assert_eq!(repaired.changed, 2, "base color and normal are clamped");
    let rewritten = repaired.bytes.expect("the indices changed");

    let document = gltf::Gltf::from_slice(&rewritten).expect("the repaired GLB loads");
    let material = document.materials().next().expect("one material");
    for (channel, set) in [
        (
            "base color",
            material
                .pbr_metallic_roughness()
                .base_color_texture()
                .expect("a base color texture")
                .tex_coord(),
        ),
        (
            "normal",
            material
                .normal_texture()
                .expect("a normal texture")
                .tex_coord(),
        ),
        (
            "occlusion",
            material
                .occlusion_texture()
                .expect("an occlusion texture")
                .tex_coord(),
        ),
    ] {
        assert!(
            set <= 1,
            "{channel} set {set} is outside the accepted range"
        );
    }
    let after = common::json_chunk(&rewritten);
    assert_eq!(
        after["materials"][0]["occlusionTexture"]["texCoord"],
        json!(1)
    );
    assert_eq!(
        after["materials"][0]["normalTexture"]["scale"],
        json!(1.5),
        "clamping the set index leaves the channel's other values alone"
    );
}

#[test]
fn a_second_clamp_changes_nothing() {
    let once = clamp_texcoord(&common::defective())
        .expect("repair")
        .bytes
        .expect("the first pass changed the document");
    let twice = clamp_texcoord(&once).expect("the repaired container is a GLB");
    assert_eq!(twice.changed, 0);
    assert_eq!(twice.bytes, None);
}

#[test]
fn a_file_that_is_not_a_glb_is_refused_before_anything_is_written() {
    assert_eq!(
        reset_mesh_weights(b"not a glb, just some ordinary bytes").unwrap_err(),
        SanitizeError::NotGlb { magic: *b"not " }
    );
    assert_eq!(
        clamp_texcoord(b"glTF").unwrap_err(),
        SanitizeError::TooShort { len: 4 }
    );
}

#[test]
fn a_json_chunk_that_overruns_the_file_is_refused() {
    let mut bytes = common::defective();
    let overrun = u32::try_from(bytes.len()).expect("a short fixture");
    bytes[12..16].copy_from_slice(&overrun.to_le_bytes());
    assert!(matches!(
        clamp_texcoord(&bytes),
        Err(SanitizeError::JsonTruncated { .. })
    ));
}
