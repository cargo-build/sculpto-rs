//! Named source clips read as palette-free node-local channels. @req DECL0013
//!
//! The evaluator applies one clip to this actor's own nodes ([`super::Pose`]).
//! Exporting a clip onto a *different* rig needs the same validated samples
//! without the application, so a caller can map them onto its own palette and
//! judge the rest compatibility itself. This module is that read: node
//! identity, property, samples and the authored sample times, nothing applied.

use super::{Actor, ActorError, Transform, animation, invalid, unsupported};

/// Which node-local property a source channel drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceProperty {
    /// Local translation.
    Translation,
    /// Local rotation quaternion (`x, y, z, w`).
    Rotation,
    /// Local scale.
    Scale,
}

/// One channel's samples, one per shared clip time.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceValues {
    /// Translation or scale samples.
    Vector(Vec<[f64; 3]>),
    /// Rotation quaternion samples.
    Rotation(Vec<[f64; 4]>),
}

/// One node-local track of a source clip.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceChannel {
    /// The targeted glTF node.
    pub node: usize,
    /// That node's authored name.
    pub node_name: Option<String>,
    /// The targeted node's parent, when the graph has one.
    pub parent: Option<usize>,
    /// The parent node's authored name.
    pub parent_name: Option<String>,
    /// The property this channel drives.
    pub property: SourceProperty,
    /// One sample per clip time.
    pub values: SourceValues,
}

/// A named clip read out of an actor GLB.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceClip {
    /// Clip name as authored.
    pub name: String,
    /// Shared sample times in seconds.
    pub times: Vec<f32>,
    /// Node-local channels; a caller maps them by [`SourceChannel::node_name`].
    pub channels: Vec<SourceChannel>,
}

/// One node's rest local transform, as a clip channel would replace it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalRest {
    /// Local translation.
    pub translation: [f64; 3],
    /// Local rotation quaternion (`x, y, z, w`).
    pub rotation: [f64; 4],
    /// Local scale.
    pub scale: [f64; 3],
}

impl Actor {
    /// Every animation name in file order, `None` when unnamed.
    pub fn clip_names(&self) -> Vec<Option<&str>> {
        self.document.animations().map(|clip| clip.name()).collect()
    }

    /// One named clip as node-local channels, without applying it.
    ///
    /// # Errors
    /// A name that matches no clip or more than one, every channel defect
    /// [`super::Pose`] evaluation also refuses, a `STEP` sampler (a written
    /// clip is `LINEAR`), and channels whose sample times disagree.
    pub fn clip(&self, name: &str) -> Result<SourceClip, ActorError> {
        let clips: Vec<_> = self
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
        let read = animation::read(self, clip, &[])?;
        let mut times: Option<Vec<f32>> = None;
        let mut channels = Vec::with_capacity(read.channels.len());
        read.channels.into_iter().try_for_each(|channel| {
            let context = format!("clip {} channel node {}", clip.index(), channel.node);
            if channel.step {
                return Err(unsupported(&context, "STEP clip interpolation"));
            }
            let samples: Vec<f32> = channel.times.iter().map(|time| *time as f32).collect();
            match &times {
                None => times = Some(samples),
                Some(shared) if *shared == samples => {}
                Some(_) => {
                    return Err(invalid(
                        &context,
                        "channels of one clip must share their sample times",
                    ));
                }
            }
            let property = match channel.property {
                gltf::animation::Property::Translation => SourceProperty::Translation,
                gltf::animation::Property::Rotation => SourceProperty::Rotation,
                gltf::animation::Property::Scale => SourceProperty::Scale,
                gltf::animation::Property::MorphTargetWeights => {
                    return Err(unsupported(&context, "morph weight clip channel"));
                }
            };
            let values = match (property, channel.values) {
                (SourceProperty::Rotation, animation::Values::Rotation(values)) => {
                    SourceValues::Rotation(
                        values.into_iter().map(|value| value.to_array()).collect(),
                    )
                }
                (SourceProperty::Translation, animation::Values::Vector(values))
                | (SourceProperty::Scale, animation::Values::Vector(values)) => {
                    SourceValues::Vector(values.into_iter().map(|value| value.to_array()).collect())
                }
                _ => {
                    return Err(invalid(&context, "animation property/output mismatch"));
                }
            };
            channels.push(SourceChannel {
                node: channel.node,
                node_name: self.node_name(channel.node).map(str::to_owned),
                parent: self.parents[channel.node],
                parent_name: self.parents[channel.node]
                    .and_then(|parent| self.node_name(parent))
                    .map(str::to_owned),
                property,
                values,
            });
            Ok(())
        })?;
        Ok(SourceClip {
            name: name.to_owned(),
            times: times.unwrap_or_default(),
            channels,
        })
    }

    /// One named clip with every channel resampled onto one shared time grid.
    ///
    /// A tool that keys only the bones that move leaves the constants on two
    /// samples while the motion carries thirty, and the strict [`Actor::clip`]
    /// refuses that mix. A converter still has to read such a file: this read
    /// keeps the densest channel's own times and interpolates the rest onto
    /// them (linear for vectors, shortest-path slerp for rotations), clamping
    /// outside a channel's own span.
    ///
    /// # Errors
    /// The same refusals [`Actor::clip`] states, except the shared-time one.
    pub fn clip_shared(&self, name: &str) -> Result<SourceClip, ActorError> {
        let clips: Vec<_> = self
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
        let read = animation::read(self, clip, &[])?;
        let grid: Vec<f64> = read
            .channels
            .iter()
            .map(|channel| (channel.times.len(), channel.times.clone()))
            .max_by(|left, right| left.0.cmp(&right.0))
            .map(|row| row.1)
            .unwrap_or_default();
        let mut channels = Vec::with_capacity(read.channels.len());
        for channel in read.channels {
            let context = format!("clip {} channel node {}", clip.index(), channel.node);
            // A `STEP` channel holds its previous sample until the next one; a
            // resample reproduces that exactly, which is why a constant channel
            // taken from a tool that writes two `STEP` keys still reads.
            let hold = channel.step;
            let property = match channel.property {
                gltf::animation::Property::Translation => SourceProperty::Translation,
                gltf::animation::Property::Rotation => SourceProperty::Rotation,
                gltf::animation::Property::Scale => SourceProperty::Scale,
                gltf::animation::Property::MorphTargetWeights => {
                    return Err(unsupported(&context, "morph weight clip channel"));
                }
            };
            let values = match (property, channel.values) {
                (SourceProperty::Rotation, animation::Values::Rotation(values)) => {
                    SourceValues::Rotation(
                        sample(&grid, &channel.times, &values, hold)
                            .into_iter()
                            .map(|value| value.to_array())
                            .collect(),
                    )
                }
                (SourceProperty::Translation, animation::Values::Vector(values))
                | (SourceProperty::Scale, animation::Values::Vector(values)) => {
                    SourceValues::Vector(
                        sample(&grid, &channel.times, &values, hold)
                            .into_iter()
                            .map(|value| value.to_array())
                            .collect(),
                    )
                }
                _ => {
                    return Err(invalid(&context, "animation property/output mismatch"));
                }
            };
            channels.push(SourceChannel {
                node: channel.node,
                node_name: self.node_name(channel.node).map(str::to_owned),
                parent: self.parents[channel.node],
                parent_name: self.parents[channel.node]
                    .and_then(|parent| self.node_name(parent))
                    .map(str::to_owned),
                property,
                values,
            });
        }
        Ok(SourceClip {
            name: name.to_owned(),
            times: grid.iter().map(|time| *time as f32).collect(),
            channels,
        })
    }

    /// One node's rest local transform.
    ///
    /// # Errors
    /// A node index outside the document, or a node authored as a static
    /// matrix (a clip cannot replace a matrix node, so there is no local TRS).
    pub fn rest_local(&self, node: usize) -> Result<LocalRest, ActorError> {
        match self.transforms.get(node) {
            Some(Transform::Trs {
                translation,
                rotation,
                scale,
            }) => Ok(LocalRest {
                translation: translation.to_array(),
                rotation: rotation.to_array(),
                scale: scale.to_array(),
            }),
            Some(Transform::Matrix(_)) => Err(invalid(
                format!("node {node}"),
                "node is a static matrix; a local TRS rest does not exist",
            )),
            None => Err(invalid(
                format!("node {node}"),
                "node index is outside the actor document",
            )),
        }
    }

    /// One parent index per node; `None` for a graph root.
    pub fn node_parents(&self) -> &[Option<usize>] {
        &self.parents
    }

    /// The palette: node indices of the first skin's joint array, in authored
    /// order.
    ///
    /// A caller retargeting a clip onto an existing actor needs the palette the
    /// skin actually binds, not every node the file carries; the joint array is
    /// that list, and its order is the order the writer indexes.
    ///
    /// # Errors
    /// A file with no skin: without one there is no palette to drive.
    pub fn skin_joints(&self) -> Result<Vec<usize>, ActorError> {
        self.document
            .skins()
            .next()
            .map(|skin| skin.joints().map(|joint| joint.index()).collect())
            .ok_or_else(|| {
                invalid(
                    "actor skin",
                    "the file carries no skin, so it has no joint palette",
                )
            })
    }

    /// One node's rest local transform as a matrix, static matrix nodes included.
    ///
    /// # Errors
    /// A node index outside the document.
    pub fn rest_local_matrix(&self, node: usize) -> Result<[[f64; 4]; 4], ActorError> {
        match self.transforms.get(node) {
            Some(transform) => Ok(transform.matrix().to_cols_array_2d()),
            None => Err(invalid(
                format!("node {node}"),
                "node index is outside the actor document",
            )),
        }
    }

    /// One node's authored name, when the file names it.
    pub fn node_name(&self, node: usize) -> Option<&str> {
        self.document.nodes().nth(node).and_then(|node| node.name())
    }

    /// Unique names of every mesh-bearing node, for sampling the whole actor.
    pub fn mesh_parts(&self) -> Vec<&str> {
        let mut seen = std::collections::HashSet::new();
        self.document
            .nodes()
            .filter(|node| node.mesh().is_some())
            .filter_map(|node| node.name())
            .filter(|name| seen.insert(*name))
            .collect()
    }
}

/// One channel's values sampled at the shared grid.
fn sample<T>(grid: &[f64], times: &[f64], values: &[T], hold: bool) -> Vec<T>
where
    T: SampleInterpolate,
{
    debug_assert_eq!(times.len(), values.len());
    match (times.first(), values.first()) {
        (Some(_), Some(first)) if times.len() == 1 => vec![*first; grid.len()],
        _ => grid
            .iter()
            .map(|time| match hold {
                true => T::held(*time, times, values),
                false => T::at(*time, times, values),
            })
            .collect(),
    }
}

/// One channel value interpolated between two authored samples.
trait SampleInterpolate: Copy {
    fn between(first: Self, second: Self, alpha: f64) -> Self;

    /// The previous authored sample, which is what `STEP` means.
    fn held(time: f64, times: &[f64], values: &[Self]) -> Self {
        let after = times.partition_point(|sample| *sample <= time);
        values[after.saturating_sub(1).min(values.len() - 1)]
    }

    fn at(time: f64, times: &[f64], values: &[Self]) -> Self {
        let Some(first) = times.first() else {
            unreachable!("a channel carries samples");
        };
        if time <= *first {
            return values[0];
        }
        let Some(last) = times.last() else {
            unreachable!("a channel carries samples");
        };
        if time >= *last {
            return values[values.len() - 1];
        }
        let after = times
            .partition_point(|sample| *sample <= time)
            .min(times.len() - 1);
        let before = after.saturating_sub(1);
        let span = times[after] - times[before];
        let alpha = match span > 0.0 {
            true => (time - times[before]) / span,
            false => 0.0,
        };
        Self::between(values[before], values[after], alpha)
    }
}

impl SampleInterpolate for glam::DVec3 {
    fn between(first: Self, second: Self, alpha: f64) -> Self {
        first.lerp(second, alpha)
    }
}

impl SampleInterpolate for glam::DQuat {
    fn between(first: Self, second: Self, alpha: f64) -> Self {
        animation::interpolate(first, second, alpha)
    }
}
