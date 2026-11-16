//! A native Skyrim walk mapped onto Mixamo's bones must stand upright (0159).
//!
//! Mixamo's `Hips` carries a −90° turn *in its own local* while its parent node
//! (`Armature`) carries the opposite turn, so the body is upright in world and a
//! quarter turn out in the node frame. The palette the converter builds folds that
//! armature out of the joint; the glTF player puts it back. A local written in the
//! palette's frame therefore drops the −90° and the whole figure lies on its side
//! while every limb still articulates — which is exactly what `0155` rendered.
//!
//! This test keeps `0155`'s numbers (52 mapped tracks, 47 named refusals) and
//! states the bar the root has to hold: it must stay inside its rest frame for the
//! whole clip, with the pelvis held at its rest height under a zeroed root motion.
//!
//! Skipped when the private host fixtures are absent. @req DECL0013

use std::collections::BTreeSet;
use std::path::PathBuf;

use glam::{DQuat, DVec3};
use sculpto_engine::adapt::actor_rig;
use sculpto_engine::export_glb::{
    ClipAnchor, ClipSemantics, ClipTolerances, RootMotion, plan_clip_with,
};
use sculpto_engine::retarget::Correspondence;
use sculpto_mesh::actor::Actor;

/// Admitted tilt of the root's own up axis away from its rest up axis, degrees.
///
/// This is the measure the operator's complaint is about: a body laid on its side
/// reads 90° here. The native walk's *full* root orientation travels 17.3–32.1°
/// because `NPC COM` yaws the body about the vertical — honest motion, which the
/// bar must not refuse — and that same yaw leaves the up axis within 9.3°. The bar
/// is therefore stated on the tilt, with the full deviation reported beside it.
const ROOT_UP_TILT_BAR_DEGREES: f64 = 25.0;

/// Admitted pelvis height change under a zeroed root motion, metres.
const PELVIS_HEIGHT_BAR: f64 = 5e-3;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

fn angle(first: DQuat, second: DQuat) -> f64 {
    2.0 * first.dot(second).abs().clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn native_walk_stands_upright_on_mixamo_bones() {
    let native = home().join("Models/sculpto-rig-matrix/native-rig-0156.glb");
    let native = match native.exists() {
        true => native,
        false => home().join("Models/sculpto-rig-matrix/native-rig.glb"),
    };
    let ybot = home().join("Models/sculpto-jokes-20261115/ybot_walking.glb");
    if !native.exists() || !ybot.exists() {
        eprintln!("skip: the rig-matrix fixtures are absent");
        return;
    }
    let target =
        Actor::from_glb(&std::fs::read(&ybot).expect("the target fixture")).expect("decodes");
    let source =
        Actor::from_glb(&std::fs::read(&native).expect("the source fixture")).expect("decodes");

    let (rig, palette) = actor_rig(&target).expect("the Mixamo palette");
    let frames = sculpture_frames(&target);
    let correspondence = Correspondence::build(&source, &rig, &palette).expect("a correspondence");
    let clip = source.clip("1hm_walkforward").expect("the native walk");
    let anchor = ClipAnchor::of(&source, &clip).expect("an anchor");
    let plan = plan_clip_with(
        &clip,
        &source,
        &anchor,
        &rig,
        &palette,
        &ClipTolerances::default(),
        Some(&correspondence),
        Some(&frames),
        ClipSemantics::Pose,
        RootMotion::Zero,
    )
    .expect("the native walk plans onto Mixamo's bones");

    // 0155's numbers, unchanged: 52 mapped tracks and 47 named refusals.
    assert_eq!(plan.mapped_bones, 52, "mapped tracks");
    eprintln!(
        "mapped {} channels, {} dropped channel(s) from this decode",
        plan.mapped_bones,
        plan.dropped_bones.len()
    );
    assert!(
        plan.moving_channels > 0,
        "the clip has to carry motion to be worth a frame check"
    );

    // The fold is measured, never assumed.
    assert!(
        (plan.palette_fold_degrees - 90.0).abs() < 1.0,
        "the armature turn is measured: got {:.2}° on {:?}",
        plan.palette_fold_degrees,
        plan.palette_fold_bone
    );
    assert_eq!(
        plan.palette_fold_slots, 1,
        "only the pelvis sits under a fold"
    );

    let frame = plan
        .root_frame
        .as_ref()
        .expect("the pelvis carries the body");
    assert_eq!(frame.bone, palette[frame.slot]);

    let rest_up = frame.rest_rotation * DVec3::Y;
    let mut tilt_min = f64::INFINITY;
    let mut tilt_max = 0.0_f64;
    let mut deviation_max = 0.0_f64;
    for rotation in &frame.rotations {
        let tilt = (*rotation * DVec3::Y)
            .dot(rest_up)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        tilt_min = tilt_min.min(tilt);
        tilt_max = tilt_max.max(tilt);
        deviation_max = deviation_max.max(angle(*rotation, frame.rest_rotation));
    }
    eprintln!(
        "root {} up tilt {:.2}..{:.2}° (bar {:.1}°), full orientation {:.2}° (reported), \
         rest height {:.4} m",
        frame.bone,
        tilt_min,
        tilt_max,
        ROOT_UP_TILT_BAR_DEGREES,
        deviation_max,
        frame.rest_translation.y
    );
    assert!(
        tilt_max <= ROOT_UP_TILT_BAR_DEGREES,
        "the root tilts {tilt_max:.2}° off its rest up axis, bar {ROOT_UP_TILT_BAR_DEGREES:.1}°"
    );

    // A zeroed root motion means the clip plays in place: the pelvis holds its
    // rest height instead of being carried off it.
    let worst_delta = frame
        .deltas
        .iter()
        .fold(0.0_f64, |worst, delta| worst.max(delta.length()));
    eprintln!("worst pelvis displacement under root-motion zero: {worst_delta:.6} m");
    assert!(
        worst_delta <= PELVIS_HEIGHT_BAR,
        "the pelvis moves {worst_delta:.4} m with the root motion zeroed"
    );

    // Every native track that found no Mixamo target is still named, not silently
    // dropped: the refusal list is the evidence `0155` published.
    let claimed: BTreeSet<&str> = correspondence
        .pairs
        .iter()
        .filter_map(|pair| pair.source_name.as_deref())
        .collect();
    let havok = home().join("Models/sculpto-rig-matrix/native-havok.json");
    if havok.exists() {
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&havok).expect("the probe report"))
                .expect("valid json");
        let tracks = report["track_bindings"].as_array().expect("track bindings");
        let hits = tracks
            .iter()
            .filter(|track| claimed.contains(track["bone"].as_str().unwrap_or("")))
            .count();
        let misses = tracks.len() - hits;
        eprintln!(
            "native track inventory: {} tracks, {hits} with a Mixamo target, {misses} named refusals",
            tracks.len()
        );
        assert_eq!(hits, 52, "native tracks finding a Mixamo target");
        assert_eq!(misses, 47, "native tracks with no Mixamo target");
    }
}

/// The target file's own frames, through the public adapter.
fn sculpture_frames(actor: &Actor) -> sculpto_engine::export_glb::PaletteFrames {
    sculpto_engine::export_glb::PaletteFrames::of(actor).expect("the file's frames measure")
}
