//! The clip, scene-layout and physics surface of the GLB writer (E05+E06),
//! re-read through the `gltf` crate.
//!
//! Every assertion reads the parsed document back, never the input struct, so
//! the tests see what a consumer would see.

use gltf::Gltf;
use gltf::animation::{Interpolation, Property};
use sculpto_core::mesh::{MeshAsset, PartitionId, PhysicsRef, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::{Clip, ClipChannel, GltfError, write_glb, write_glb_with};

/// A two-bone skinned triangle, so clips target real joints.
fn two_bone_triangle() -> MeshAsset {
    MeshAsset {
        name: "body".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: vec!["root".into(), "spine".into()],
            bind_inverse: vec![],
            influences: SkinWeights::from_quads(&[[0, 0, 0, 0]; 3], &[[1.0, 0.0, 0.0, 0.0]; 3])
                .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

/// A translation+rotation clip over three samples, driving both bones.
fn swing_clip() -> Clip {
    Clip {
        name: "swing".into(),
        times: vec![0.0, 0.5, 1.0],
        channels: vec![
            ClipChannel::Translation {
                bone: 0,
                values: vec![[0.0, 0.0, 0.0], [0.0, 1.5, 0.0], [0.0, 3.0, 0.0]],
            },
            ClipChannel::Rotation {
                bone: 1,
                values: vec![
                    [0.0, 0.0, 0.0, 1.0],
                    [0.0, 0.6, 0.0, 0.8],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
        ],
    }
}

/// A one-channel scale clip over two samples.
fn bob_clip() -> Clip {
    Clip {
        name: "bob".into(),
        times: vec![0.25, 0.75],
        channels: vec![ClipChannel::Scale {
            bone: 1,
            values: vec![[1.0, 1.0, 1.0], [2.0, 2.0, 2.0]],
        }],
    }
}

/// Raw accessor payload from the embedded buffer, at accessor coordinates.
fn accessor_bytes<'doc>(parsed: &'doc Gltf, accessor: &gltf::Accessor<'doc>) -> &'doc [u8] {
    let view = accessor.view().expect("accessor has a buffer view");
    let blob = parsed.blob.as_deref().expect("GLB carries the buffer");
    let start = view.offset() + accessor.offset();
    let end = start + accessor.size() * accessor.count();
    &blob[start..end]
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

/// Scalar `f32` values, little-endian.
fn scalars(bytes: &[u8]) -> Vec<f32> {
    floats::<1>(bytes).iter().map(|row| row[0]).collect()
}

/// A JSON number array read back as `f32` cells.
fn floats_from_json(value: &serde_json::Value) -> Vec<f32> {
    value
        .as_array()
        .expect("a JSON number array")
        .iter()
        .map(|cell| cell.as_f64().expect("a JSON number") as f32)
        .collect()
}

#[test]
fn clips_write_animations_that_parse_back() {
    let asset = two_bone_triangle();
    let parsed = Gltf::from_slice(
        &write_glb_with(&asset, &[swing_clip(), bob_clip()])
            .expect("a valid skinned triangle with clips writes"),
    )
    .expect("the writer output parses");

    let animations: Vec<_> = parsed.document.animations().collect();
    assert_eq!(animations.len(), 2, "one animation per clip");
    let names: Vec<_> = animations
        .iter()
        .map(|animation| animation.name().expect("clips are named"))
        .collect();
    assert_eq!(names, ["swing", "bob"], "animation names follow the clips");
    let channel_counts: Vec<usize> = animations
        .iter()
        .map(|animation| animation.channels().count())
        .collect();
    assert_eq!(
        channel_counts,
        [2, 1],
        "one channel per clip channel, in order"
    );

    for (animation, samples) in animations.iter().zip([3_usize, 2]) {
        for channel in animation.channels() {
            let sampler = channel.sampler();
            assert_eq!(
                sampler.interpolation(),
                Interpolation::Linear,
                "samplers interpolate LINEAR"
            );
            let input = sampler.input();
            assert_eq!(
                input.data_type(),
                gltf::accessor::DataType::F32,
                "sampler input is f32"
            );
            assert_eq!(
                input.count(),
                samples,
                "sampler input pairs every clip sample"
            );
            assert!(input.min().is_some(), "sampler input carries min");
            assert!(input.max().is_some(), "sampler input carries max");
        }
    }

    let swing = &animations[0];
    let swing_channels: Vec<_> = swing.channels().collect();
    let (translation, rotation) = (&swing_channels[0], &swing_channels[1]);
    assert_eq!(
        translation.target().property(),
        Property::Translation,
        "the first swing channel drives translation"
    );
    assert_eq!(
        translation.target().node().index(),
        1,
        "bone 0 targets its joint node"
    );
    assert_eq!(
        translation.target().node().name(),
        Some("root"),
        "the targeted node is the bone's joint"
    );
    assert_eq!(
        rotation.target().property(),
        Property::Rotation,
        "the second swing channel drives rotation"
    );
    assert_eq!(
        rotation.target().node().index(),
        2,
        "bone 1 targets its joint node"
    );
    assert_eq!(rotation.target().node().name(), Some("spine"));

    let times = scalars(accessor_bytes(&parsed, &translation.sampler().input()));
    assert_eq!(times, swing_clip().times, "clip times are bit-exact");
    assert_eq!(
        floats_from_json(&translation.sampler().input().min().expect("min")),
        [0.0],
        "sampler input min is the first time"
    );
    assert_eq!(
        floats_from_json(&translation.sampler().input().max().expect("max")),
        [1.0],
        "sampler input max is the last time"
    );

    let translations = floats::<3>(accessor_bytes(&parsed, &translation.sampler().output()));
    let ClipChannel::Translation { values, .. } = &swing_clip().channels[0] else {
        unreachable!("the first swing channel is a translation");
    };
    assert_eq!(
        translations,
        values.as_slice(),
        "translations are bit-exact"
    );

    let rotations = floats::<4>(accessor_bytes(&parsed, &rotation.sampler().output()));
    let ClipChannel::Rotation { values, .. } = &swing_clip().channels[1] else {
        unreachable!("the second swing channel is a rotation");
    };
    assert_eq!(rotations, values.as_slice(), "rotations are bit-exact");

    let bob = &animations[1];
    let bob_channel = bob.channels().next().expect("one channel");
    assert_eq!(
        bob_channel.target().property(),
        Property::Scale,
        "the bob clip drives scale"
    );
    assert_eq!(
        bob_channel.target().node().name(),
        Some("spine"),
        "bone 1 is the scale target"
    );
    assert_eq!(
        floats_from_json(&bob_channel.sampler().input().min().expect("min")),
        [0.25],
        "the bob sampler input min"
    );
    assert_eq!(
        floats_from_json(&bob_channel.sampler().input().max().expect("max")),
        [0.75],
        "the bob sampler input max"
    );
    let scales = floats::<3>(accessor_bytes(&parsed, &bob_channel.sampler().output()));
    let ClipChannel::Scale { values, .. } = &bob_clip().channels[0] else {
        unreachable!("the bob channel is a scale");
    };
    assert_eq!(scales, values.as_slice(), "scales are bit-exact");
}

#[test]
fn write_glb_is_the_no_clip_spelling() {
    let asset = two_bone_triangle();
    let plain = write_glb(&asset).expect("a valid skinned triangle writes");
    let parsed = Gltf::from_slice(&plain).expect("the writer output parses");
    assert_eq!(
        parsed.document.animations().count(),
        0,
        "write_glb emits no animation"
    );
    assert_eq!(
        plain,
        write_glb_with(&asset, &[]).expect("an empty clip slice writes"),
        "write_glb is write_glb_with over no clips"
    );
}

#[test]
fn a_constant_clip_is_refused_before_any_bytes() {
    let asset = two_bone_triangle();
    let empty = Clip {
        name: "still".into(),
        times: vec![],
        channels: vec![],
    };
    assert_eq!(
        write_glb_with(&asset, &[empty]),
        Err(GltfError::ConstantClip {
            clip: 0,
            name: "still".into(),
            samples: 0
        }),
        "a zero-length clip is a constant clip"
    );

    let single = Clip {
        name: "still".into(),
        times: vec![1.0],
        channels: vec![],
    };
    assert_eq!(
        write_glb_with(&asset, &[single]),
        Err(GltfError::ConstantClip {
            clip: 0,
            name: "still".into(),
            samples: 1
        }),
        "a single-sample clip is a constant clip"
    );

    let good = Clip {
        name: "good".into(),
        times: vec![0.0, 1.0],
        channels: vec![],
    };
    let single = Clip {
        name: "still".into(),
        times: vec![1.0],
        channels: vec![],
    };
    assert_eq!(
        write_glb_with(&asset, &[good, single]),
        Err(GltfError::ConstantClip {
            clip: 1,
            name: "still".into(),
            samples: 1
        }),
        "the error names the offending clip's position"
    );
}

#[test]
fn malformed_clip_channels_are_refused() {
    let asset = two_bone_triangle();
    let short_channel = Clip {
        name: "short".into(),
        times: vec![0.0, 0.5, 1.0],
        channels: vec![ClipChannel::Translation {
            bone: 0,
            values: vec![[0.0; 3]; 2],
        }],
    };
    assert_eq!(
        write_glb_with(&asset, &[short_channel]),
        Err(GltfError::ClipChannelSampleMismatch {
            clip: 0,
            channel: 0,
            samples: 3,
            values: 2
        }),
        "a sampler cannot pair two values with three samples"
    );

    let stray_bone = Clip {
        name: "stray".into(),
        times: vec![0.0, 1.0],
        channels: vec![ClipChannel::Scale {
            bone: 5,
            values: vec![[1.0; 3]; 2],
        }],
    };
    assert_eq!(
        write_glb_with(&asset, &[stray_bone]),
        Err(GltfError::ClipBoneOutOfRange {
            clip: 0,
            channel: 0,
            bone: 5,
            joints: 2
        }),
        "a channel cannot target a bone without a joint node"
    );

    let mut skinless = two_bone_triangle();
    skinless.skin = None;
    let clip = Clip {
        name: "unskinned".into(),
        times: vec![0.0, 1.0],
        channels: vec![ClipChannel::Translation {
            bone: 0,
            values: vec![[0.0; 3]; 2],
        }],
    };
    assert_eq!(
        write_glb_with(&skinless, &[clip]),
        Err(GltfError::ClipBoneOutOfRange {
            clip: 0,
            channel: 0,
            bone: 0,
            joints: 0
        }),
        "clips need a skin with joints"
    );
}

#[test]
fn the_scene_roots_one_named_node_over_joints_and_mesh() {
    let asset = two_bone_triangle();
    let parsed = Gltf::from_slice(
        &write_glb_with(&asset, &[swing_clip()]).expect("a valid skinned triangle writes"),
    )
    .expect("the writer output parses");

    let scene = parsed
        .document
        .default_scene()
        .expect("the writer names a scene");
    let roots: Vec<_> = scene.nodes().collect();
    assert_eq!(roots.len(), 1, "the scene has exactly one root node");
    let root = &roots[0];
    assert_eq!(root.index(), 0, "the root node is node 0");
    assert_eq!(root.name(), Some("body"), "the root carries the asset name");
    assert!(root.mesh().is_some(), "the root carries the mesh");
    assert!(
        parsed.document.as_json().nodes[root.index()].skin.is_some(),
        "the mesh node references the skin"
    );
    let children: Vec<_> = root.children().collect();
    let child_names: Vec<_> = children
        .iter()
        .map(|node| node.name().expect("joint nodes are named"))
        .collect();
    assert_eq!(
        child_names,
        ["root", "spine"],
        "the joints hang under the root in model bone order"
    );

    let mut skinless = two_bone_triangle();
    skinless.skin = None;
    let parsed = Gltf::from_slice(&write_glb(&skinless).expect("a skinless mesh writes"))
        .expect("the writer output parses");
    let roots: Vec<_> = parsed
        .document
        .default_scene()
        .expect("the writer names a scene")
        .nodes()
        .collect();
    assert_eq!(roots.len(), 1, "a skinless mesh still roots one node");
    assert_eq!(roots[0].name(), Some("body"));
    assert_eq!(
        roots[0].children().count(),
        0,
        "no joints hang under a skinless root"
    );
    assert!(
        parsed.document.as_json().nodes[roots[0].index()]
            .skin
            .is_none()
    );
}

#[test]
fn a_physics_path_travels_in_extras_and_an_inline_document_is_refused() {
    let mut asset = two_bone_triangle();
    asset.physics = Some(PhysicsRef::Path("robe.sculpto-phys.json".into()));
    let parsed = Gltf::from_slice(&write_glb(&asset).expect("a referenced sidecar writes"))
        .expect("the writer output parses");
    let extras = parsed.document.as_json().nodes[0]
        .extras
        .as_ref()
        .expect("the root node carries the sidecar reference");
    let value: serde_json::Value = serde_json::from_str(extras.get()).expect("extras hold JSON");
    assert_eq!(
        value,
        serde_json::json!({ "sculpto": { "physics": "robe.sculpto-phys.json" } }),
        "the sidecar path is the documented extras.sculpto.physics string"
    );

    asset.physics = Some(PhysicsRef::Inline("{\"constraints\":[]}".into()));
    let error =
        write_glb_with(&asset, &[swing_clip()]).expect_err("an inline physics document is refused");
    assert_eq!(error, GltfError::PhysicsInlineUnsupported);
    assert!(
        error.to_string().contains("extras.sculpto.physics"),
        "the refusal records the documented slot: {error}"
    );
}
