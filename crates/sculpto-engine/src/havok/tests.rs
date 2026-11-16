//! Unit tests for the Havok decoder: masks, quaternion quantization, splines and
//! the source-GLB round trip through the actor loader. The real-archive proof runs
//! against the private fixture, not here.

use super::{
    ChannelType, RigSkeleton, SplineClip, SplineTrack, TrackMask, de_boor_weights,
    read_quant_scalar, write_source_glb,
};
use sculpto_mesh::actor::Actor;

/// Three stored components of zero, `w` reconstructed at slot 3, sign clear.
#[test]
fn a_48bit_identity_decodes_to_unit_w() {
    let raw = 0xBFFFu64 | 0xBFFFu64 << 16 | 0x3FFFu64 << 32;
    assert_eq!(super::quat48(raw), [0.0, 0.0, 0.0, 1.0]);
}

/// The shift places the reconstructed component: `x` largest lands in slot 1.
#[test]
fn the_48bit_shift_places_the_fourth_component() {
    let raw = 0xBFFFu64 | 0x3FFFu64 << 16 | 0x3FFFu64 << 32;
    assert_eq!(super::quat48(raw), [0.0, 1.0, 0.0, 0.0]);
}

/// The `z` sign bit negates the reconstructed component.
#[test]
fn the_48bit_sign_negates_the_fourth_component() {
    let raw = 0xBFFFu64 | 0xBFFFu64 << 16 | 0xBFFFu64 << 32;
    assert_eq!(super::quat48(raw), [0.0, 0.0, 0.0, -1.0]);
}

/// Quantized scalars recover the range ends and the midpoint.
#[test]
fn quantized_scalars_recover_their_range() {
    let blob = [0u8; 4];
    let mut cursor = 0;
    let low = read_quant_scalar(&blob, &mut cursor, -1.0, 1.0, 16).expect("16-bit");
    assert!((low + 1.0).abs() < 1e-3, "got {low}");
    let high_blob = [0xFF, 0xFF, 0, 0];
    let mut cursor = 0;
    let high = read_quant_scalar(&high_blob, &mut cursor, -1.0, 1.0, 16).expect("16-bit");
    assert!((high - 1.0).abs() < 1e-3, "got {high}");
    let mid_blob = [128, 0, 0, 0];
    let mut cursor = 0;
    let mid = read_quant_scalar(&mid_blob, &mut cursor, -1.0, 1.0, 8).expect("8-bit");
    assert!((mid - 0.0039).abs() < 1e-3, "got {mid}");
}

/// The mask nibbles route each channel to identity, static or spline.
#[test]
fn masks_classify_their_channels() {
    let mask = TrackMask::new(0b1000_1001, 0b0010_0001, 0b0001_0000, 0);
    assert_eq!(mask.pos_quant, 1);
    assert_eq!(mask.rot_quant, 2);
    assert_eq!(mask.scale_quant, 2);
    assert_eq!(mask.pos_type(0), ChannelType::Static);
    assert_eq!(mask.pos_type(1), ChannelType::Spline);
    assert_eq!(mask.pos_type(2), ChannelType::Identity);
    assert_eq!(mask.rot_type(), ChannelType::Spline);
    assert_eq!(mask.scale_type(0), ChannelType::Identity);
    let static_rot = TrackMask::new(0, 0, 0b0000_0001, 0);
    assert_eq!(static_rot.rot_type(), ChannelType::Static);
    let identity_rot = TrackMask::new(0, 0, 0, 0);
    assert_eq!(identity_rot.rot_type(), ChannelType::Identity);
}

/// Degree-1 de Boor reduces to linear interpolation at the clamped uniform knots.
#[test]
fn degree1_weights_interpolate_linearly() {
    let knots = [0.0, 0.0, 1.0, 2.0, 2.0];
    let weights_at = |t: f64| {
        let span = super::knot_span(1, t, 3, &knots);
        (span, de_boor_weights(1, span, t, &knots))
    };
    let value = |(span, weights): (usize, Vec<f64>), points: &[f64]| -> f64 {
        weights
            .iter()
            .enumerate()
            .filter(|(_, w)| **w != 0.0)
            .map(|(offset, w)| points[span - offset] * w)
            .sum()
    };
    let points = [0.0, 4.0, 8.0];
    assert!((value(weights_at(0.0), &points) - 0.0).abs() < 1e-12);
    assert!((value(weights_at(0.5), &points) - 2.0).abs() < 1e-12);
    assert!((value(weights_at(1.5), &points) - 6.0).abs() < 1e-12);
    assert!((value(weights_at(2.0), &points) - 8.0).abs() < 1e-12);
}

/// A two-bone rig with a two-track clip round-trips through the actor loader.
#[test]
fn the_source_glb_round_trips_through_the_actor_loader() {
    let skeleton = RigSkeleton {
        name: "NPC Root [Root]".to_owned(),
        bones: vec!["NPC Root [Root]".to_owned(), "NPC Spine1 [Spn1]".to_owned()],
        parents: vec![-1, 0],
        rest: vec![
            ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
            ([7.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]),
        ],
    };
    let clip = SplineClip {
        duration: 2.0,
        frame_duration: 1.0,
        block_duration: 2.0,
        num_frames: 3,
        num_tracks: 2,
        num_blocks: 1,
        max_frames_per_block: 256,
        class: "hkaSplineCompressedAnimation".to_owned(),
        tracks: vec![
            SplineTrack {
                translations: vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
                rotations: vec![[0.0, 0.0, 0.0, 1.0]; 3],
            },
            SplineTrack {
                translations: vec![[7.0, 0.0, 0.0]; 3],
                rotations: vec![
                    [0.0, 0.0, 0.0, 1.0],
                    [
                        0.0,
                        0.0,
                        std::f64::consts::FRAC_1_SQRT_2,
                        std::f64::consts::FRAC_1_SQRT_2,
                    ],
                    [0.0, 0.0, 1.0, 0.0],
                ],
            },
        ],
        ..SplineClip::default()
    };
    let bytes = write_source_glb(&skeleton, &clip, &[Some(0), Some(1)], 0.014).expect("glb");
    let actor = Actor::from_glb(&bytes).expect("the written GLB must load");
    assert_eq!(actor.clip_names(), vec![Some("attack")]);
    let clip_read = actor.clip("attack").expect("clip read");
    assert_eq!(clip_read.times.len(), 3);
    assert_eq!(
        clip_read.channels.len(),
        4,
        "rotation + translation per track"
    );
    let spine = actor.rest_local(1).expect("rest");
    assert!(
        (spine.translation[0] - 0.098).abs() < 1e-3,
        "got {:?}",
        spine.translation
    );
    let names_of_nodes: Vec<_> = (0..2).filter_map(|node| actor.node_name(node)).collect();
    assert_eq!(names_of_nodes, vec!["NPC Root [Root]", "NPC Spine1 [Spn1]"]);
}

/// A clip whose tracks bind to nothing writes no GLB: a named refusal, not an
/// empty file.
#[test]
fn a_fully_unbound_clip_is_a_named_refusal() {
    let skeleton = RigSkeleton {
        name: "rig".to_owned(),
        bones: vec!["a".to_owned()],
        parents: vec![-1],
        rest: vec![([0.0; 3], [0.0, 0.0, 0.0, 1.0])],
    };
    let clip = SplineClip {
        num_frames: 3,
        num_tracks: 1,
        frame_duration: 0.5,
        duration: 1.0,
        tracks: vec![SplineTrack::default()],
        ..SplineClip::default()
    };
    let error = write_source_glb(&skeleton, &clip, &[None], 0.014).expect_err("refused");
    assert!(error.to_string().contains("no track binds to a rig bone"));
}

/// A one-frame clip cannot become a GLB clip: named refusal.
#[test]
fn a_one_frame_clip_is_refused() {
    let skeleton = RigSkeleton {
        name: "rig".to_owned(),
        bones: vec!["a".to_owned()],
        parents: vec![-1],
        rest: vec![([0.0; 3], [0.0, 0.0, 0.0, 1.0])],
    };
    let clip = SplineClip {
        num_frames: 1,
        num_tracks: 1,
        tracks: vec![SplineTrack::default()],
        ..SplineClip::default()
    };
    let error = write_source_glb(&skeleton, &clip, &[Some(0)], 0.014).expect_err("refused");
    assert!(error.to_string().contains("at least two"));
}
