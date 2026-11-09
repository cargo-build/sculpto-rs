//! Correspondence, frame and clip admission for the bounded append writer.

use std::collections::HashSet;

use super::{Source, index_of, reject_extensions};
use crate::actor::{ActorError, Pose, animation, decode, invalid, unsupported};
use gltf::accessor::{DataType, Dimensions};
use gltf::animation::Property;
use gltf::mesh::Mode;

pub(super) fn images(base: &Source) -> Result<(), ActorError> {
    base.actor.document.images().try_for_each(|image| {
        match base.json["images"][image.index()]
            .get("uri")
            .and_then(|uri| uri.as_str())
        {
            Some(uri)
                if !uri
                    .get(..5)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:")) =>
            {
                Err(unsupported(
                    format!("base image {}", image.index()),
                    "external image URI (expected embedded bufferView or data URI)",
                ))
            }
            _ => Ok(()),
        }
    })
}

pub(super) fn schedule(times: &[f64]) -> Result<Vec<f32>, ActorError> {
    match (2..=64).contains(&times.len()) {
        true => {}
        false => {
            return Err(unsupported(
                "cloth schedule",
                "target count outside current Bevy consumer bound 2–64",
            ));
        }
    }
    let encoded: Vec<_> = times.iter().map(|&time| time as f32).collect();
    match times.iter().all(|time| time.is_finite() && *time >= 0.0)
        && times.windows(2).all(|pair| pair[0] < pair[1])
        && encoded.iter().all(|time| time.is_finite())
        && encoded.windows(2).all(|pair| pair[0] < pair[1])
    {
        true => Ok(encoded),
        false => Err(invalid(
            "cloth schedule",
            "times must remain finite, nonnegative and strictly increasing after f32 encoding",
        )),
    }
}

pub(super) fn frames(
    base: &Source,
    targets: &Source,
    selected: usize,
) -> Result<usize, ActorError> {
    let document = &base.actor.document;
    let node = document
        .nodes()
        .nth(selected)
        .ok_or_else(|| invalid("cloth node", "missing selected node"))?;
    let mesh = node
        .mesh()
        .ok_or_else(|| invalid("cloth node", "missing mesh"))?
        .index();
    match document
        .nodes()
        .filter(|node| {
            node.mesh()
                .is_some_and(|candidate| candidate.index() == mesh)
        })
        .count()
    {
        1 => {}
        _ => return Err(unsupported("cloth node", "instanced/shared mesh")),
    }
    let target_nodes: Vec<_> = targets
        .actor
        .document
        .nodes()
        .filter(|node| node.mesh().is_some())
        .collect();
    let target_node = match (
        targets.actor.document.meshes().len(),
        target_nodes.as_slice(),
    ) {
        (1, [node]) => node.index(),
        _ => {
            return Err(invalid(
                "cloth targets",
                "expected one mesh and one mesh node",
            ));
        }
    };
    match targets.actor.document.animations().len() {
        0 => {}
        _ => {
            return Err(unsupported(
                "cloth targets",
                "animation in geometry-only input",
            ));
        }
    }
    [(base, selected), (targets, target_node)]
        .into_iter()
        .try_for_each(|(source, index)| {
            let mut cursor = Some(index);
            while let Some(index) = cursor {
                let node = source
                    .actor
                    .document
                    .nodes()
                    .nth(index)
                    .ok_or_else(|| invalid("cloth hierarchy", "missing node"))?;
                if node.skin().is_some() {
                    return Err(unsupported(
                        "cloth hierarchy",
                        "skin on selected node or ancestor",
                    ));
                }
                reject_extensions(&source.json["nodes"][index], "cloth node/ancestor")?;
                cursor = source.actor.parents[index];
            }
            Ok::<_, ActorError>(())
        })?;
    reject_extensions(&base.json["meshes"][mesh], "selected mesh")?;
    reject_extensions(&targets.json["meshes"][0], "target mesh")?;
    let base_world = animation::worlds(&base.actor, &[selected], Pose::Rest)?[selected];
    let target_world = animation::worlds(&targets.actor, &[target_node], Pose::Rest)?[target_node];
    match base_world == target_world && base_world.determinant().abs() > 0.0 {
        true => Ok(mesh),
        false => Err(invalid(
            "cloth frame",
            "default world transforms differ or are singular",
        )),
    }
}

pub(super) fn animation(
    base: &Source,
    node: usize,
    name: &str,
    times: &[f32],
) -> Result<(usize, Option<usize>), ActorError> {
    let clips: Vec<_> = base
        .actor
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
    let mut ancestors = HashSet::from([node]);
    let mut cursor = base.actor.parents[node];
    while let Some(index) = cursor {
        ancestors.insert(index);
        cursor = base.actor.parents[index];
    }
    base.actor.document.animations().try_for_each(|candidate| {
        candidate.channels().try_for_each(|channel| {
            let target = channel.target();
            let index = target.node().index();
            match target.property() {
                Property::MorphTargetWeights
                    if index == node && candidate.index() != clip.index() =>
                {
                    Err(unsupported(
                        "cloth animation",
                        "another clip animates the same cloth bank",
                    ))
                }
                Property::Translation | Property::Rotation | Property::Scale
                    if ancestors.contains(&index) =>
                {
                    Err(unsupported(
                        "cloth animation",
                        "animated selected node or ancestor would transform baked geometry",
                    ))
                }
                _ => Ok(()),
            }
        })
    })?;
    let mut start = f32::INFINITY;
    let mut end = f32::NEG_INFINITY;
    let mut selected = None;
    let mut seen = HashSet::new();
    clip.channels().try_for_each(|channel| {
        let target = channel.target();
        match seen.insert((target.node().index(), target.property() as u8)) {
            true => {}
            false => return Err(invalid("selected clip", "duplicate target channel")),
        }
        let input = base.accessor(channel.sampler().input().index())?;
        decode::accessor(&input, Dimensions::Scalar, &[DataType::F32], "clip times")?;
        if input.normalized() {
            return Err(invalid("clip times", "normalized floating-point input"));
        }
        let channel_times: Vec<_> =
            gltf::accessor::Iter::<f32>::new(input, |buffer| base.actor.buffer(buffer))
                .ok_or_else(|| invalid("clip times", "unreadable input"))?
                .collect();
        match channel_times
            .iter()
            .all(|time| time.is_finite() && *time >= 0.0)
            && channel_times.windows(2).all(|pair| pair[0] < pair[1])
        {
            true => {}
            false => return Err(invalid("clip times", "invalid time sequence")),
        }
        start = start.min(channel_times[0]);
        end = end.max(channel_times[channel_times.len() - 1]);
        if target.node().index() == node && target.property() == Property::MorphTargetWeights {
            let sampler = channel.sampler().index();
            match clip
                .channels()
                .filter(|channel| channel.sampler().index() == sampler)
                .count()
            {
                1 => {}
                _ => return Err(unsupported("cloth weights", "shared animation sampler")),
            }
            reject_extensions(
                &base.json["animations"][clip.index()]["channels"][channel.index()],
                "cloth channel",
            )?;
            reject_extensions(
                &base.json["animations"][clip.index()]["channels"][channel.index()]["target"],
                "cloth channel target",
            )?;
            reject_extensions(
                &base.json["animations"][clip.index()]["samplers"][sampler],
                "cloth sampler",
            )?;
            selected = Some(channel.index());
        }
        Ok::<_, ActorError>(())
    })?;
    match times.first() == Some(&start) && times.last() == Some(&end) {
        true => Ok((clip.index(), selected)),
        false => Err(invalid(
            "cloth schedule",
            format!("encoded endpoints must equal existing clip interval [{start}, {end}]"),
        )),
    }
}

pub(super) fn primitives(
    base: &Source,
    targets: &Source,
    mesh: usize,
    count: usize,
) -> Result<(), ActorError> {
    let base_mesh = base
        .actor
        .document
        .meshes()
        .nth(mesh)
        .ok_or_else(|| invalid("cloth mesh", "missing mesh"))?;
    let target_mesh = targets
        .actor
        .document
        .meshes()
        .next()
        .ok_or_else(|| invalid("cloth targets", "missing mesh"))?;
    match base_mesh.primitives().len() == target_mesh.primitives().len()
        && base_mesh.primitives().len() > 0
    {
        true => {}
        false => {
            return Err(invalid(
                "cloth primitives",
                "primitive count mismatch or empty mesh",
            ));
        }
    }
    base_mesh
        .primitives()
        .zip(target_mesh.primitives())
        .try_for_each(|(original, replacement)| {
            let context = format!("cloth primitive {}", original.index());
            let base_json = &base.json["meshes"][mesh]["primitives"][original.index()];
            let target_json = &targets.json["meshes"][0]["primitives"][replacement.index()];
            reject_extensions(base_json, &context)?;
            reject_extensions(target_json, &context)?;
            match original.mode() == Mode::Triangles && replacement.mode() == Mode::Triangles {
                true => {}
                false => return Err(unsupported(&context, "non-triangle primitive")),
            }
            let attributes = base_json["attributes"]
                .as_object()
                .ok_or_else(|| invalid(&context, "missing attributes"))?;
            let target_attributes = target_json["attributes"]
                .as_object()
                .ok_or_else(|| invalid(&context, "missing target attributes"))?;
            match attributes.keys().eq(target_attributes.keys())
                && attributes.contains_key("POSITION")
                && attributes.contains_key("NORMAL")
            {
                true => {}
                false => {
                    return Err(invalid(
                        &context,
                        "attribute set mismatch or missing POSITION/NORMAL",
                    ));
                }
            }
            let positions = base.accessor(index_of(&attributes["POSITION"], &context)?)?;
            let vertices = positions.count();
            attributes.iter().try_for_each(|(name, index)| {
                let original = base.accessor(index_of(index, &context)?)?;
                let replacement =
                    targets.accessor(index_of(&target_attributes[name], &context)?)?;
                match original.count() == vertices && compatible(&original, &replacement) {
                    true => {}
                    false => {
                        return Err(invalid(
                            &context,
                            format!("{name} accessor shape/count mismatch"),
                        ));
                    }
                }
                match name.as_str() {
                    "POSITION" | "NORMAL" | "TANGENT" => {
                        geometric(base, &original, name, false, vertices, &context)?;
                        geometric(targets, &replacement, name, false, vertices, &context)
                    }
                    _ => match base.elements(&original)? == targets.elements(&replacement)? {
                        true => Ok(()),
                        false => Err(invalid(
                            &context,
                            format!("non-geometric attribute {name} differs"),
                        )),
                    },
                }
            })?;
            indices(base, targets, &original, &replacement, vertices, &context)?;
            let morphs = target_json["targets"]
                .as_array()
                .ok_or_else(|| invalid(&context, "missing morph targets"))?;
            match morphs.len() == count {
                true => {}
                false => return Err(invalid(&context, "target count differs from schedule")),
            }
            morphs.iter().try_for_each(|target| {
                let attributes = target
                    .as_object()
                    .ok_or_else(|| invalid(&context, "expected morph attribute map"))?;
                if attributes.is_empty() {
                    return Err(invalid(&context, "empty morph target"));
                }
                attributes.iter().try_for_each(|(name, index)| {
                    match name.as_str() {
                        "POSITION" | "NORMAL" | "TANGENT"
                            if target_attributes.contains_key(name) => {}
                        _ => return Err(unsupported(&context, format!("morph attribute {name}"))),
                    }
                    let accessor = targets.accessor(index_of(index, &context)?)?;
                    geometric(targets, &accessor, name, true, vertices, &context)
                })
            })
        })
}

fn compatible(a: &gltf::Accessor<'_>, b: &gltf::Accessor<'_>) -> bool {
    a.count() == b.count()
        && a.dimensions() == b.dimensions()
        && a.data_type() == b.data_type()
        && a.normalized() == b.normalized()
}

fn geometric(
    source: &Source,
    accessor: &gltf::Accessor<'_>,
    name: &str,
    morph: bool,
    vertices: usize,
    context: &str,
) -> Result<(), ActorError> {
    let dimensions = match (name, morph) {
        ("TANGENT", false) => Dimensions::Vec4,
        _ => Dimensions::Vec3,
    };
    decode::accessor(accessor, dimensions, &[DataType::F32], context)?;
    match !accessor.normalized()
        && accessor.count() == vertices
        && source
            .elements(accessor)?
            .as_chunks::<4>()
            .0
            .iter()
            .all(|bytes| f32::from_le_bytes(*bytes).is_finite())
    {
        true => Ok(()),
        false => Err(invalid(
            context,
            format!("invalid {name} geometry count/normalization or nonfinite component"),
        )),
    }
}

fn indices(
    base: &Source,
    targets: &Source,
    original: &gltf::Primitive<'_>,
    replacement: &gltf::Primitive<'_>,
    vertices: usize,
    context: &str,
) -> Result<(), ActorError> {
    match (original.indices(), replacement.indices()) {
        (Some(a), Some(b)) => {
            let a = base.accessor(a.index())?;
            let b = targets.accessor(b.index())?;
            decode::accessor(
                &a,
                Dimensions::Scalar,
                &[DataType::U8, DataType::U16, DataType::U32],
                context,
            )?;
            decode::accessor(
                &b,
                Dimensions::Scalar,
                &[DataType::U8, DataType::U16, DataType::U32],
                context,
            )?;
            match compatible(&a, &b)
                && !a.normalized()
                && a.count().is_multiple_of(3)
                && base.elements(&a)? == targets.elements(&b)?
            {
                true => {}
                false => {
                    return Err(invalid(
                        context,
                        "ordered index layout/payload differs or is invalid",
                    ));
                }
            }
            let reader = original.reader(|buffer| base.actor.buffer(buffer));
            match reader
                .read_indices()
                .ok_or_else(|| invalid(context, "unreadable indices"))?
                .into_u32()
                .all(|index| (index as usize) < vertices)
            {
                true => Ok(()),
                false => Err(invalid(context, "index exceeds vertex count")),
            }
        }
        (None, None) if vertices.is_multiple_of(3) => Ok(()),
        _ => Err(invalid(
            context,
            "index presence differs or implicit triangle count is invalid",
        )),
    }
}
