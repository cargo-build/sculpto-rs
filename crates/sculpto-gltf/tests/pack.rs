//! The packer appends to a GLB and the result parses again through `gltf`.
//!
//! One adapter-level round trip: copy a validated accessor out of a geometry
//! GLB, replace the primitive attribute and morph target, zero the default
//! weights and write the weights channel, then re-read the container and
//! compare the accessor bookkeeping.

use gltf::Gltf;
use gltf::animation::Property;
use gltf::mesh::Semantic;
use sculpto_core::mesh::{MeshAsset, MorphDelta, MorphScope, MorphTarget, PartitionId};
use sculpto_gltf::Clip;
use sculpto_gltf::pack::{AccessorCopy, PackWriter};
use sculpto_gltf::write_glb_with;
use serde_json::{Value, json};

/// One triangle with normals and UVs, the smallest mesh the writer accepts.
fn triangle() -> MeshAsset {
    MeshAsset {
        name: "cloth".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

/// One accessor copy read out of a parsed geometry GLB.
fn copy_of<'input>(
    parsed: &'input Gltf,
    json: &'input Value,
    index: usize,
) -> AccessorCopy<'input> {
    let accessor = parsed
        .document
        .accessors()
        .nth(index)
        .expect("source accessor");
    let view = accessor.view().expect("source buffer view");
    let blob = parsed.blob.as_deref().expect("source BIN chunk");
    AccessorCopy {
        accessor: index,
        view: view.index(),
        description: &json["accessors"][index],
        view_description: &json["bufferViews"][view.index()],
        bytes: &blob[view.offset()..view.offset() + view.length()],
    }
}

/// The stored bytes of one accessor, at the accessor's own coordinates.
fn accessor_bytes<'doc>(parsed: &'doc Gltf, accessor: &gltf::Accessor<'doc>) -> &'doc [u8] {
    let view = accessor.view().expect("accessor view");
    let blob = parsed.blob.as_deref().expect("BIN chunk");
    let start = view.offset() + accessor.offset();
    &blob[start..start + accessor.size() * accessor.count()]
}

#[test]
fn a_packed_glb_parses_and_its_accessor_counts_match() {
    let clip = Clip {
        name: "Run".into(),
        times: vec![0.0, 1.0],
        channels: Vec::new(),
    };
    let base = write_glb_with(&triangle(), std::slice::from_ref(&clip)).expect("base GLB");
    let mut displaced = triangle();
    displaced.positions = vec![[0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [0.0, 1.0, 0.5]];
    displaced.morphs = vec![MorphTarget {
        name: "baked0".into(),
        deltas: vec![MorphDelta {
            vertex: 2,
            position: [0.0, 0.0, 0.25],
        }],
        scope: MorphScope::Whole,
    }];
    let targets = write_glb_with(&displaced, &[]).expect("targets GLB");

    let base_document = Gltf::from_slice(&base).expect("base parses");
    let (base_accessors, base_views) = (
        base_document.document.accessors().count(),
        base_document.document.views().count(),
    );
    let target_document = Gltf::from_slice(&targets).expect("targets parse");
    let target_json: Value = serde_json::from_slice(
        &gltf::binary::Glb::from_slice(&targets)
            .expect("targets framing")
            .json,
    )
    .expect("targets JSON");
    let target_primitive = target_document
        .document
        .meshes()
        .next()
        .and_then(|mesh| mesh.primitives().next())
        .expect("target primitive");
    let position = target_primitive
        .get(&Semantic::Positions)
        .expect("target POSITION");
    let morph = target_json["meshes"][0]["primitives"][0]["targets"][0]["POSITION"]
        .as_u64()
        .expect("target morph accessor") as usize;

    let mut writer = PackWriter::from_glb(&base).expect("base packer");
    let copied_position = writer
        .copy_accessor(copy_of(&target_document, &target_json, position.index()))
        .expect("copy POSITION");
    writer
        .set_attribute(0, 0, "POSITION", copied_position)
        .expect("replace POSITION");
    let copied_morph = writer
        .copy_accessor(copy_of(&target_document, &target_json, morph))
        .expect("copy morph");
    writer
        .set_targets(0, 0, &[vec![("POSITION".to_owned(), copied_morph)]])
        .expect("replace morph targets");
    writer.set_mesh_weights(0, 1).expect("zero weights");
    writer
        .set_target_names(0, Some(&["baked0".to_owned()]))
        .expect("target names");
    writer
        .set_weights_channel(0, None, 0, &[0.0, 1.0])
        .expect("weights channel");

    let packed = writer.finish().expect("packed GLB");
    let parsed = Gltf::from_slice(&packed).expect("packed GLB parses");
    assert_eq!(
        parsed.document.accessors().count(),
        base_accessors + 4,
        "POSITION and morph copies plus the two weights accessors"
    );
    assert_eq!(
        parsed.document.views().count(),
        base_views + 4,
        "one view per copied accessor and per weights payload"
    );
    let packed_primitive = parsed
        .document
        .meshes()
        .next()
        .and_then(|mesh| mesh.primitives().next())
        .expect("packed primitive");
    let packed_position = packed_primitive
        .get(&Semantic::Positions)
        .expect("packed POSITION");
    assert_eq!(packed_position.count(), position.count());
    assert_eq!(
        accessor_bytes(&parsed, &packed_position),
        accessor_bytes(&target_document, &position)
    );
    let animation = parsed.document.animations().next().expect("clip");
    assert_eq!(
        animation
            .channels()
            .filter(|channel| channel.target().property() == Property::MorphTargetWeights)
            .count(),
        1,
        "the packer appends one weights channel"
    );
    let json: Value = serde_json::from_slice(
        &gltf::binary::Glb::from_slice(&packed)
            .expect("framing")
            .json,
    )
    .expect("packed JSON");
    assert_eq!(json["meshes"][0]["weights"], json!([0.0]));
    assert_eq!(
        json["meshes"][0]["extras"]["targetNames"],
        json!(["baked0"])
    );
}

/// A minimal skeleton GLB with one named rotation clip, for insertion tests.
fn skeleton_base(clip_name: &str) -> Vec<u8> {
    let mut bin = Vec::new();
    let mut push = |values: &[f32]| -> usize {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let offset = bin.len();
        bin.extend(values.iter().flat_map(|v| v.to_le_bytes()));
        offset
    };
    let times_off = push(&[0.0, 1.0]);
    let rot_off = push(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    let json = json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": times_off, "byteLength": 8},
            {"buffer": 0, "byteOffset": rot_off, "byteLength": 32},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "type": "SCALAR", "count": 2,
             "min": [0.0], "max": [1.0]},
            {"bufferView": 1, "componentType": 5126, "type": "VEC4", "count": 2},
        ],
        "nodes": [
            {"name": "Root", "children": [1]},
            {"name": "Joint", "translation": [0.0, 1.0, 0.0]},
        ],
        "animations": [{
            "name": clip_name,
            "channels": [{"sampler": 0, "target": {"node": 1, "path": "rotation"}}],
            "samplers": [{"input": 0, "output": 1, "interpolation": "LINEAR"}],
        }],
    });
    gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: std::borrow::Cow::Owned(serde_json::to_vec(&json).unwrap()),
        bin: Some(std::borrow::Cow::Owned(bin)),
    }
    .to_vec()
    .unwrap()
}

#[test]
fn insert_animation_lands_at_the_walk_slot_and_keeps_existing_clips() {
    use sculpto_gltf::pack::Track;
    let base = skeleton_base("Idle");
    let base_accessors = 2;
    let mut writer = PackWriter::from_glb(&base).expect("base packer");
    let tracks = [
        Track::Rotation {
            node: 1,
            values: vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.3826834, 0.9238795]],
        },
        Track::Translation {
            node: 1,
            values: vec![[0.0, 1.0, 0.0], [0.0, 1.03, 0.0]],
        },
    ];
    let index = writer
        .insert_animation(1, "Gait", &[0.0, 1.0], &tracks)
        .expect("insert");
    assert_eq!(index, 1);
    let packed = writer.finish().expect("packed GLB");
    let parsed = Gltf::from_slice(&packed).expect("packed GLB parses");
    let names: Vec<_> = parsed
        .document
        .animations()
        .map(|clip| clip.name().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["Idle", "Gait"]);
    // One shared time accessor plus one output accessor per track.
    assert_eq!(parsed.document.accessors().count(), base_accessors + 3);
    let gait = parsed.document.animations().nth(1).expect("gait clip");
    assert_eq!(gait.channels().count(), 2);
    let paths: Vec<_> = gait
        .channels()
        .map(|channel| channel.target().property())
        .collect();
    assert!(paths.contains(&Property::Rotation));
    assert!(paths.contains(&Property::Translation));
    // Every sampler is LINEAR and shares the input accessor.
    let input = gait.samplers().next().expect("sampler").input().index();
    assert!(
        gait.samplers()
            .all(|sampler| sampler.input().index() == input)
    );
}

#[test]
fn insert_animation_clamps_to_zero_on_a_clipless_actor_and_refuses_short_input() {
    use sculpto_gltf::pack::Track;
    // A clipless base: drop the animations array from the skeleton base.
    let base = skeleton_base("Idle");
    let framing = gltf::binary::Glb::from_slice(&base).unwrap();
    let mut json: Value = serde_json::from_slice(&framing.json).unwrap();
    json.as_object_mut().unwrap().remove("animations");
    let clipless = gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: std::borrow::Cow::Owned(serde_json::to_vec(&json).unwrap()),
        bin: Some(framing.bin.unwrap()),
    }
    .to_vec()
    .unwrap();
    let mut writer = PackWriter::from_glb(&clipless).expect("packer");
    let tracks = [Track::Rotation {
        node: 1,
        values: vec![[0.0, 0.0, 0.0, 1.0]; 2],
    }];
    let index = writer
        .insert_animation(1, "Pose", &[0.0, 1.0], &tracks)
        .expect("insert");
    assert_eq!(index, 0, "clamped to the first slot when no clip exists");

    let mut writer = PackWriter::from_glb(&base).expect("packer");
    assert!(writer.insert_animation(1, "Bad", &[0.0], &tracks).is_err());
    assert!(writer.insert_animation(1, "Bad", &[0.0, 1.0], &[]).is_err());
}
