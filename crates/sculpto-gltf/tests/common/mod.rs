//! A synthetic GLB carrying both defects the post-write repairs address.
//!
//! The container is framed by the `gltf` crate itself, so the tests exercise
//! the same reader a consumer uses rather than a hand-rolled byte layout.

use std::borrow::Cow;

use gltf::binary::Glb;
use serde_json::{Map, Value, json};

/// One mesh with two morph defaults, and one material whose base-color and
/// normal textures both point at UV set `-1` while occlusion keeps a legal 1.
///
/// The single POSITION accessor is a complete one-vertex primitive, so the
/// repaired document validates with the `gltf` crate instead of only parsing.
pub fn defective() -> Vec<u8> {
    let positions: Vec<u8> = [0.0_f32, 0.0, 0.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    frame(
        json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": positions.len()}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": positions.len()}],
            "accessors": [{
                "bufferView": 0,
                "componentType": 5126,
                "type": "VEC3",
                "count": 1,
                "min": [0.0, 0.0, 0.0],
                "max": [0.0, 0.0, 0.0],
            }],
            "images": [{"uri": "cloth.png"}],
            "textures": [{"source": 0}],
            "materials": [{
                "name": "cloth",
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0, "texCoord": -1}},
                "normalTexture": {"index": 0, "texCoord": -1, "scale": 1.5},
                "occlusionTexture": {"index": 0, "texCoord": 1},
            }],
            "meshes": [{
                "name": "Cloth",
                "weights": [0.4, 0.25],
                "primitives": [{"attributes": {"POSITION": 0}, "material": 0}],
            }],
            "nodes": [{"name": "Cloth", "mesh": 0}],
            "scenes": [{"nodes": [0]}],
            "scene": 0,
        }),
        positions,
    )
}

/// Frame a document as a GLB with the binary chunk it references.
pub fn frame(document: Value, bin: Vec<u8>) -> Vec<u8> {
    Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Owned(serde_json::to_vec(&document).expect("a document serialises")),
        bin: Some(Cow::Owned(bin)),
    }
    .to_vec()
    .expect("a framed container")
}

/// The JSON chunk of a GLB, before any reader applies its own rules.
pub fn json_chunk(bytes: &[u8]) -> Map<String, Value> {
    let container = Glb::from_slice(bytes).expect("a GLB container");
    serde_json::from_slice(&container.json).expect("a JSON object chunk")
}

/// The binary chunk of a GLB.
// The adapter tests compare chunks; the CLI test links this module without it.
#[allow(dead_code)]
pub fn binary_chunk(bytes: &[u8]) -> Option<Vec<u8>> {
    Glb::from_slice(bytes)
        .expect("a GLB container")
        .bin
        .map(|bin| bin.to_vec())
}
