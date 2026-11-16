//! The shared rig measures: stance-foot slide, edge stretch and palette
//! reconstruction, in the `0134` forms.
//!
//! One owner per measurement: `actor adapt` gates its conversions with these
//! and [`crate::regression`] re-measures any written artifact with the same
//! functions, so the two verbs can never disagree about what a number means.

use std::collections::BTreeSet;

use glam::{DMat4, DVec3};
use serde_json::{Value, json};

use sculpto_mesh::actor::{Actor, Pose};

use crate::{CommandError, ErrorKind};

/// Admitted stance-foot speed while in contact, metres per second: 0.1 over one
/// 33 ms frame is 3.3 mm of slip, under the 5 mm the eye resolves at gait speed.
pub const CONTACT_SPEED: f64 = 0.1;
/// A foot is in contact while within this height of its own minimum, metres.
pub const CONTACT_HEIGHT: f64 = 0.010;
/// Admitted *median* stance speed over the contact frames, metres per second.
///
/// The maximum fires at push-off, where the toe rolls while the sole is still
/// down: the reference clip's own toe rolls 0.023 m during that phase, so a maxima
/// bar measures the roll, not sliding. The median over contact frames measures
/// whether the foot is planted for the stance. The bar is twice the reference
/// clip's own median measured with this check (0.084 m/s), and the measured value
/// is reported next to it. A zeroed-hips trajectory slides at ~1.5 m/s for the
/// whole stance and still fails.
pub const STANCE_MEDIAN_BAR: f64 = 2.0 * 0.084;
/// Admitted body edge stretch, as a multiple of the rest length. Reported, not
/// gated on its own: on these bodies the same edges exceed it with no adapted clip
/// at all, so the gate is the relative one below.
pub const EDGE_STRETCH_CAP: f64 = 2.0;
/// Admitted stretch of the adapted clip against the body's own baseline pose at
/// the same joint angles.
pub const RELATIVE_STRETCH: f64 = 1.10;

/// The mesh parts this actor can be sampled on with the palette's own skin.
///
/// A multi-skin file (a body plus garments bound to their own joint sets) samples
/// per skin; the measures are about the body the palette drives, so a part whose
/// skin is not the palette's is left out and named in the report instead of failing
/// the measurement.
pub(crate) fn measurable_parts(actor: &Actor) -> Vec<String> {
    actor
        .mesh_parts()
        .into_iter()
        .filter(|name| actor.sample(&[*name], Pose::Rest).is_ok())
        .map(str::to_owned)
        .collect()
}

/// One toe joint and its world position per sampled frame.
#[derive(Debug, Clone)]
struct ToeTrack {
    name: String,
    positions: Vec<DVec3>,
}

/// Measure the stance-foot slide of one clip on the actor that carries it.
///
/// A foot is in contact while it is within [`CONTACT_HEIGHT`] of its own lowest
/// point over the clip; while in contact its horizontal speed must stay under
/// [`CONTACT_SPEED`]. A clip with no contact sample is not a pass: it is reported
/// with `contact_samples: 0`, which is how a float or a tiptoe gets caught.
pub(crate) fn toe_slide(
    actor: &Actor,
    clip: &str,
    palette: &[String],
    toes: &[String],
    frames: usize,
) -> Result<Value, CommandError> {
    let parts = measurable_parts(actor);
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let (start, end) = actor.clip_span(clip).map_err(actor_error)?;
    let count = frames.max(2);
    let mut tracks: Vec<ToeTrack> = toes
        .iter()
        .map(|name| ToeTrack {
            name: name.clone(),
            positions: Vec::with_capacity(count),
        })
        .collect();
    let mut times = Vec::with_capacity(count);
    for index in 0..count {
        let time = start + (end - start) * index as f64 / (count - 1) as f64;
        times.push(time);
        // The whole palette is requested, not only the toes: a retained rig keeps
        // the joints a requested bone's vertices bind to, and a body vertex is
        // weighted across the whole skeleton.
        let sample = actor
            .rig_sample(
                &parts,
                Pose::Clip { name: clip, time },
                &palette.iter().map(String::as_str).collect::<Vec<_>>(),
            )
            .map_err(actor_error)?;
        for (track, name) in tracks.iter_mut().zip(toes) {
            let node = sample
                .rig
                .iter()
                .find(|node| node.name.as_deref() == Some(name.as_str()))
                .ok_or_else(|| {
                    refused(
                        "measures_palette",
                        format!("the actor's rig sample does not carry toe joint {name:?}"),
                    )
                })?;
            track
                .positions
                .push(DMat4::from_cols_array_2d(&node.world).w_axis.truncate());
        }
    }
    let mut rows = Vec::with_capacity(tracks.len());
    let mut worst = 0.0_f64;
    for track in &tracks {
        let lowest = track
            .positions
            .iter()
            .map(|position| position.y)
            .fold(f64::INFINITY, f64::min);
        let mut contact = 0;
        let mut peak = 0.0_f64;
        let mut speeds: Vec<f64> = Vec::new();
        for index in 1..track.positions.len() {
            let (previous, current) = (track.positions[index - 1], track.positions[index]);
            if previous.y > lowest + CONTACT_HEIGHT || current.y > lowest + CONTACT_HEIGHT {
                continue;
            }
            contact += 1;
            let delta = times[index] - times[index - 1];
            if delta <= 0.0 {
                continue;
            }
            let speed =
                DVec3::new(current.x - previous.x, 0.0, current.z - previous.z).length() / delta;
            peak = peak.max(speed);
            speeds.push(speed);
        }
        speeds.sort_by(f64::total_cmp);
        let median = match speeds.len() {
            0 => 0.0,
            odd if odd % 2 == 1 => speeds[odd / 2],
            even => (speeds[even / 2 - 1] + speeds[even / 2]) / 2.0,
        };
        worst = worst.max(median);
        // The table the verdict rests on: height and speed per frame, so a refusal
        // names the frames instead of the clip.
        let mut table = Vec::with_capacity(track.positions.len());
        for index in 0..track.positions.len() {
            let speed = match index {
                0 => 0.0,
                _ => {
                    let delta = times[index] - times[index - 1];
                    match delta > 0.0 {
                        true => {
                            DVec3::new(
                                track.positions[index].x - track.positions[index - 1].x,
                                0.0,
                                track.positions[index].z - track.positions[index - 1].z,
                            )
                            .length()
                                / delta
                        }
                        false => 0.0,
                    }
                }
            };
            table.push(json!({
                "time": times[index],
                "height": track.positions[index].y,
                "speed": speed,
                "contact": track.positions[index].y <= lowest + CONTACT_HEIGHT,
            }));
        }
        rows.push(json!({
            "joint": track.name,
            "lowest_height": lowest,
            "contact_samples": contact,
            "max_horizontal_speed": peak,
            "median_horizontal_speed": median,
            "frames": table,
        }));
    }
    Ok(json!({
        "frames": count,
        "contact_height": CONTACT_HEIGHT,
        "admitted_median_speed": STANCE_MEDIAN_BAR,
        "reported_max_speed_bar": CONTACT_SPEED,
        "max_horizontal_speed": rows
            .iter()
            .filter_map(|row| row["max_horizontal_speed"].as_f64())
            .fold(0.0_f64, f64::max),
        "worst_median_speed": worst,
        "pass": worst <= STANCE_MEDIAN_BAR
            && rows.iter().all(|row| row["contact_samples"].as_u64().unwrap_or(0) > 0),
        "toes": rows,
    }))
}

/// Largest body edge stretch of one clip, as a rest-length multiple.
pub(crate) fn edge_stretch(
    actor: &Actor,
    clip: &str,
    frames: usize,
) -> Result<Value, CommandError> {
    let named = measurable_parts(actor);
    let parts: Vec<&str> = named.iter().map(String::as_str).collect();
    let rest = actor.sample(&parts, Pose::Rest).map_err(actor_error)?;
    // Unique edges per part, so a garment seam is not counted twice and a part
    // boundary is never treated as an edge.
    let mut edges: Vec<(usize, usize, usize, f64)> = Vec::new();
    let mut skipped = 0_usize;
    for (index, part) in rest.parts.iter().enumerate() {
        let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
        for triangle in &rest.triangles[part.triangles.clone()] {
            for pair in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let ordered = match pair.0 <= pair.1 {
                    true => pair,
                    false => (pair.1, pair.0),
                };
                if !seen.insert(ordered) {
                    continue;
                }
                let length = DVec3::from_array(rest.vertices[ordered.0])
                    .distance(DVec3::from_array(rest.vertices[ordered.1]));
                // A welded seam can carry coincident vertices: a rest length of a
                // micrometre makes every ratio infinite and says nothing about
                // the deformation, so the ratio is only measured where the edge
                // has a length to stretch.
                if length < 1e-4 {
                    skipped += 1;
                    continue;
                }
                edges.push((index, ordered.0, ordered.1, length));
            }
        }
    }
    let (start, end) = actor.clip_span(clip).map_err(actor_error)?;
    let count = frames.max(2);
    let mut worst = 0.0_f64;
    let mut worst_part = String::new();
    let mut worst_time = 0.0;
    // A ratio on a sub-millimetre sliver is not a visible tear: report the ratio
    // per rest-length class and the absolute displacement, so the verdict names
    // the geometry it is about.
    let mut class_one_mm = 0.0_f64;
    let mut class_five_mm = 0.0_f64;
    let mut absolute = 0.0_f64;
    for index in 0..count {
        let time = start + (end - start) * index as f64 / (count - 1) as f64;
        let sample = actor
            .sample(&parts, Pose::Clip { name: clip, time })
            .map_err(actor_error)?;
        for (part, first, second, length) in &edges {
            if *length < 1e-9 {
                continue;
            }
            let posed = DVec3::from_array(sample.vertices[*first])
                .distance(DVec3::from_array(sample.vertices[*second]));
            let ratio = posed / length;
            if ratio > worst {
                worst = ratio;
                worst_part.clone_from(&rest.parts[*part].name);
                worst_time = time;
            }
            if *length >= 1e-3 {
                class_one_mm = class_one_mm.max(ratio);
            }
            if *length >= 5e-3 {
                class_five_mm = class_five_mm.max(ratio);
            }
            absolute = absolute.max(posed - length);
        }
    }
    Ok(json!({
        "frames": count,
        "parts": named,
        "edges": edges.len(),
        "degenerate_edges_skipped": skipped,
        "max_ratio": worst,
        "max_ratio_edges_at_least_1mm": class_one_mm,
        "max_ratio_edges_at_least_5mm": class_five_mm,
        "max_absolute_stretch": absolute,
        "cap": EDGE_STRETCH_CAP,
        "pass": worst < EDGE_STRETCH_CAP,
        "worst_part": worst_part,
        "worst_time": worst_time,
    }))
}

/// Palette reconstruction: one written palette's rest must be the other's.
pub(crate) fn palette_reconstruction(base: &Actor, written: &Actor) -> Result<Value, CommandError> {
    let base_joints = base.skin_joints().map_err(actor_error)?;
    let written_joints = written.skin_joints().map_err(actor_error)?;
    let mut max = 0.0_f64;
    let mut height = 0.0_f64;
    let count = base_joints.len().min(written_joints.len());
    for index in 0..count {
        let left = base
            .rest_local_matrix(base_joints[index])
            .map_err(actor_error)?;
        let right = written
            .rest_local_matrix(written_joints[index])
            .map_err(actor_error)?;
        for column in 0..4 {
            for row in 0..4 {
                max = max.max((left[column][row] - right[column][row]).abs());
            }
        }
    }
    for node in &base_joints {
        let point = DMat4::from_cols_array_2d(&base.rest_local_matrix(*node).map_err(actor_error)?)
            .w_axis
            .truncate();
        height = height.max(point.length());
    }
    Ok(json!({
        "palette_bones": base_joints.len(),
        "written_palette_bones": written_joints.len(),
        "names_identical": base_joints
            .iter()
            .zip(&written_joints)
            .all(|(left, right)| base.node_name(*left) == written.node_name(*right)),
        "max_rest_delta": max,
        "extent": height,
        "pass": max <= 1e-5 * height.max(1.0) && base_joints.len() == written_joints.len(),
    }))
}

pub(crate) fn actor_error(error: sculpto_mesh::actor::ActorError) -> CommandError {
    crate::export_glb::actor_error(error)
}

/// Where a shoulder joint sits relative to the top of the spine, over a clip.
///
/// The operator judges the girdle by its *level*, not by its swing (`0154`): a
/// shoulder that moves the source's distance but sits 75 mm further out reads as
/// a frame over the chest, while a shoulder that swings back and forth at the
/// body's own radius is natural. Radius is the three-dimensional distance from
/// the spine top; height is its component along the body's own up axis, which is
/// read from the rest chain rather than assumed to be a world axis.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ShoulderLevel {
    /// Palette bone the row measures.
    pub bone: String,
    /// Palette bone it is measured from.
    pub spine: String,
    /// Mean distance from the spine top, mm.
    pub mean_radius_mm: f64,
    /// Peak-to-peak radial excursion over the clip, mm.
    pub radius_excursion_mm: f64,
    /// Radial excursion as a fraction of the mean radius.
    pub radius_excursion_fraction: f64,
    /// Mean height above the spine top along the body's up axis, mm.
    pub mean_height_mm: f64,
    /// Peak-to-peak height excursion, mm.
    pub height_excursion_mm: f64,
    /// Frames sampled.
    pub frames: usize,
}

/// The pure metric behind [`ShoulderLevel`]: offsets of the shoulder from the
/// spine top, world metres, with `up` the body's own up direction.
pub fn shoulder_level_from_offsets(
    bone: &str,
    spine: &str,
    up: DVec3,
    offsets: &[DVec3],
) -> ShoulderLevel {
    let up = up.normalize_or_zero();
    let radii: Vec<f64> = offsets.iter().map(|offset| offset.length()).collect();
    let heights: Vec<f64> = offsets.iter().map(|offset| offset.dot(up)).collect();
    let mean = |values: &[f64]| match values.is_empty() {
        true => 0.0,
        false => values.iter().sum::<f64>() / values.len() as f64,
    };
    let spread = |values: &[f64]| match (
        values.iter().copied().reduce(f64::min),
        values.iter().copied().reduce(f64::max),
    ) {
        (Some(low), Some(high)) => high - low,
        _ => 0.0,
    };
    let mean_radius = mean(&radii);
    let radius_excursion = spread(&radii);
    ShoulderLevel {
        bone: bone.to_owned(),
        spine: spine.to_owned(),
        mean_radius_mm: mean_radius * 1000.0,
        radius_excursion_mm: radius_excursion * 1000.0,
        radius_excursion_fraction: match mean_radius > 1e-9 {
            true => radius_excursion / mean_radius,
            false => 0.0,
        },
        mean_height_mm: mean(&heights) * 1000.0,
        height_excursion_mm: spread(&heights) * 1000.0,
        frames: offsets.len(),
    }
}

/// The palette in skin order: one name per skin joint, the names clips address.
pub(crate) fn palette_names(actor: &Actor) -> Result<Vec<String>, CommandError> {
    let joints = actor.skin_joints().map_err(actor_error)?;
    joints
        .iter()
        .map(|node| {
            actor.node_name(*node).map(str::to_owned).ok_or_else(|| {
                refused(
                    "measures_palette",
                    format!("skin joint node {node} is unnamed, so a clip cannot name it"),
                )
            })
        })
        .collect()
}

fn refused(context: &'static str, message: impl Into<String>) -> CommandError {
    CommandError::new(ErrorKind::Refused, context, message.into())
}
