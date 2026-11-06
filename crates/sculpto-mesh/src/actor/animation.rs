//! LINEAR channels use f64 interpolation and exact trigonometric SLERP.

use std::collections::HashSet;

use glam::{DMat4, DQuat, DVec3};
use gltf::accessor::{DataType, Dimensions};
use gltf::animation::{Interpolation, Property};

use super::{Actor, ActorError, Pose, Transform, decode, invalid, unsupported};

#[derive(Debug)]
struct Channel {
    node: usize,
    property: Property,
    times: Vec<f64>,
    values: Values,
    step: bool,
}

#[derive(Debug)]
enum Values {
    Vector(Vec<DVec3>),
    Rotation(Vec<DQuat>),
}

pub(super) fn worlds(
    actor: &Actor,
    selected: &[usize],
    pose: Pose<'_>,
) -> Result<Vec<DMat4>, ActorError> {
    let mut transforms = actor.transforms.clone();
    match pose {
        Pose::Rest => {}
        Pose::Clip { name, time } => {
            let clips: Vec<_> = actor
                .document
                .animations()
                .filter(|clip| clip.name() == Some(name))
                .collect();
            let clip = match clips.as_slice() {
                [clip] => clip,
                _ => {
                    return Err(ActorError::Clip {
                        name: name.to_owned(),
                        matches: clips.len(),
                    });
                }
            };
            apply(actor, clip, selected, time, &mut transforms)?;
        }
    }
    let mut worlds = vec![DMat4::IDENTITY; transforms.len()];
    actor.order.iter().try_for_each(|&index| {
        let local = transforms[index].matrix();
        worlds[index] = match actor.parents[index] {
            Some(parent) => worlds[parent] * local,
            None => local,
        };
        decode::affine(worlds[index], &format!("node {index} world transform"))
    })?;
    Ok(worlds)
}

fn apply(
    actor: &Actor,
    clip: &gltf::Animation<'_>,
    selected: &[usize],
    time: f64,
    transforms: &mut [Transform],
) -> Result<(), ActorError> {
    let mut targets = HashSet::new();
    let mut start = f64::INFINITY;
    let mut end = f64::NEG_INFINITY;
    let channels = clip
        .channels()
        .map(|channel| {
            let node = channel.target().node().index();
            let property = channel.target().property();
            let context = format!(
                "clip {} channel {} node {node} {property:?}",
                clip.index(),
                channel.index()
            );
            match targets.insert((node, property as u8)) {
                true => {}
                false => return Err(invalid(&context, "duplicate target channel")),
            }
            let input = channel.sampler().input();
            decode::accessor(&input, Dimensions::Scalar, &[DataType::F32], &context)?;
            let times: Vec<f64> = channel
                .reader(|buffer| actor.buffer(buffer))
                .read_inputs()
                .ok_or_else(|| invalid(&context, "unreadable animation times"))?
                .map(f64::from)
                .collect();
            match times.iter().all(|value| value.is_finite() && *value >= 0.0)
                && times.windows(2).all(|pair| pair[0] < pair[1])
            {
                true => {}
                false => {
                    return Err(invalid(
                        &context,
                        "times must be finite, nonnegative and strictly increasing",
                    ));
                }
            }
            start = start.min(times[0]);
            end = end.max(times[times.len() - 1]);
            match property {
                Property::MorphTargetWeights if !selected.contains(&node) => return Ok(None),
                Property::MorphTargetWeights => {
                    return Err(unsupported(&context, "selected morph weight animation"));
                }
                _ => {}
            }
            let step = match channel.sampler().interpolation() {
                Interpolation::Linear => false,
                Interpolation::Step => true,
                value => return Err(unsupported(&context, format!("{value:?} interpolation"))),
            };
            match transforms[node] {
                Transform::Matrix(_) => {
                    return Err(invalid(&context, "animation targets a static matrix node"));
                }
                Transform::Trs { .. } => {}
            }
            let output = channel.sampler().output();
            let dimensions = match property {
                Property::Rotation => Dimensions::Vec4,
                _ => Dimensions::Vec3,
            };
            decode::accessor(&output, dimensions, &[DataType::F32], &context)?;
            match output.count() == times.len() && !output.normalized() && !input.normalized() {
                true => {}
                false => {
                    return Err(invalid(
                        &context,
                        "animation input/output count or normalization mismatch",
                    ));
                }
            }
            let values = match property {
                Property::Rotation => {
                    let values = gltf::accessor::Iter::<[f32; 4]>::new(output, |buffer| {
                        actor.buffer(buffer)
                    })
                    .ok_or_else(|| invalid(&context, "unreadable rotation output"))?
                    .map(|value| decode::quaternion(value, &context))
                    .collect::<Result<_, _>>()?;
                    Values::Rotation(values)
                }
                _ => {
                    let values: Vec<_> = gltf::accessor::Iter::<[f32; 3]>::new(output, |buffer| {
                        actor.buffer(buffer)
                    })
                    .ok_or_else(|| invalid(&context, "unreadable vector output"))?
                    .map(|value| DVec3::from_array(value.map(f64::from)))
                    .collect();
                    match values.iter().all(|value| value.is_finite()) {
                        true => Values::Vector(values),
                        false => return Err(invalid(&context, "nonfinite animation output")),
                    }
                }
            };
            Ok(Some(Channel {
                node,
                property,
                times,
                values,
                step,
            }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    match time.is_finite() && time >= start && time <= end {
        true => {}
        false => return Err(ActorError::Time { time, start, end }),
    }
    channels
        .into_iter()
        .flatten()
        .try_for_each(|channel| evaluate(channel, time, transforms))
}

fn evaluate(channel: Channel, time: f64, transforms: &mut [Transform]) -> Result<(), ActorError> {
    let last = channel.times.len() - 1;
    let (lower, upper, alpha) = match channel.step {
        true => {
            let index = channel
                .times
                .partition_point(|&value| value <= time)
                .saturating_sub(1)
                .min(last);
            (index, index, 0.0)
        }
        false => {
            let upper = channel
                .times
                .partition_point(|&value| value <= time)
                .min(last);
            let lower = upper.saturating_sub(1);
            let alpha = match lower == upper {
                true => 0.0,
                false => ((time - channel.times[lower])
                    / (channel.times[upper] - channel.times[lower]))
                    .clamp(0.0, 1.0),
            };
            (lower, upper, alpha)
        }
    };
    let Transform::Trs {
        translation,
        rotation,
        scale,
    } = &mut transforms[channel.node]
    else {
        return Err(invalid(
            format!("node {}", channel.node),
            "animation targets static matrix",
        ));
    };
    match (channel.property, channel.values) {
        (Property::Translation, Values::Vector(values)) => {
            *translation = values[lower].lerp(values[upper], alpha)
        }
        (Property::Scale, Values::Vector(values)) => {
            *scale = values[lower].lerp(values[upper], alpha)
        }
        (Property::Rotation, Values::Rotation(values)) => {
            *rotation = slerp(values[lower], values[upper], alpha)
        }
        _ => {
            return Err(invalid(
                format!("node {}", channel.node),
                "animation property/output mismatch",
            ));
        }
    }
    Ok(())
}

fn slerp(a: DQuat, mut b: DQuat, alpha: f64) -> DQuat {
    if a.dot(b) < 0.0 {
        b = -b;
    }
    // The half-angle identity remains accurate when the dot product rounds to one.
    let theta = 2.0 * (a - b).length().atan2((a + b).length());
    let value = match theta > 0.0 {
        true => (a * ((1.0 - alpha) * theta).sin() + b * (alpha * theta).sin()) / theta.sin(),
        false => a,
    };
    value.normalize()
}
