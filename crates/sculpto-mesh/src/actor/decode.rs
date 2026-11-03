//! Validate before invoking decoder utilities with unchecked range arithmetic.

use std::collections::VecDeque;

use glam::{DMat4, DQuat, DVec3};
use gltf::accessor::{DataType, Dimensions};

use super::{Actor, ActorError, Transform, invalid, unsupported};

pub(super) fn read(bytes: &[u8]) -> Result<Actor, ActorError> {
    let header = bytes
        .get(..12)
        .ok_or_else(|| invalid("GLB", "truncated header"))?;
    let length = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    match (&header[..4], length) {
        (b"glTF", n) if n == bytes.len() && n >= 20 && n.is_multiple_of(4) => {}
        _ => return Err(invalid("GLB", "invalid magic, total length or alignment")),
    }
    let framing = gltf::binary::Glb::from_slice(bytes).map_err(ActorError::Decode)?;
    let consumed = 20 + framing.json.len() + framing.bin.as_ref().map_or(0, |bin| 8 + bin.len());
    match consumed == bytes.len()
        && framing.json.len().is_multiple_of(4)
        && framing
            .bin
            .as_ref()
            .is_none_or(|bin| bin.len().is_multiple_of(4))
    {
        true => {}
        false => return Err(invalid("GLB", "trailing bytes or unaligned chunk")),
    }
    let decoded = gltf::Gltf::from_slice(bytes).map_err(ActorError::Decode)?;
    let document = decoded.document;
    if let Some(extension) = document.extensions_required().next() {
        return Err(unsupported(
            "GLB",
            format!("required extension {extension}"),
        ));
    }
    let blob = decoded
        .blob
        .ok_or_else(|| invalid("GLB", "missing embedded BIN chunk"))?;
    let buffers: Vec<_> = document.buffers().collect();
    match buffers.as_slice() {
        [buffer]
            if matches!(buffer.source(), gltf::buffer::Source::Bin)
                && buffer.length() <= blob.len()
                && blob.len() - buffer.length() <= 3 => {}
        _ => {
            return Err(unsupported(
                "GLB",
                "buffer layout (expected one embedded BIN buffer)",
            ));
        }
    }
    document
        .views()
        .try_for_each(|view| match view.offset().checked_add(view.length()) {
            Some(end) if end <= buffers[0].length() => Ok(()),
            _ => Err(invalid(
                format!("buffer view {}", view.index()),
                "range exceeds buffer",
            )),
        })?;
    let (parents, order) = hierarchy(&document)?;
    let transforms = document
        .nodes()
        .map(|node| {
            let source = &document.as_json().nodes[node.index()];
            if source.matrix.is_some()
                && (source.translation.is_some()
                    || source.rotation.is_some()
                    || source.scale.is_some())
            {
                return Err(invalid(
                    format!("node {}", node.index()),
                    "matrix and TRS are mutually exclusive",
                ));
            }
            transform(node)
        })
        .collect::<Result<_, _>>()?;
    Ok(Actor {
        document,
        blob,
        transforms,
        parents,
        order,
    })
}

fn hierarchy(document: &gltf::Document) -> Result<(Vec<Option<usize>>, Vec<usize>), ActorError> {
    let mut parents = vec![None; document.nodes().len()];
    document.nodes().try_for_each(|node| {
        node.children()
            .try_for_each(|child| match parents[child.index()].replace(node.index()) {
                None => Ok(()),
                Some(_) => Err(invalid(
                    format!("node {}", child.index()),
                    "multiple parents",
                )),
            })
    })?;
    let children: Vec<Vec<usize>> = document
        .nodes()
        .map(|node| node.children().map(|child| child.index()).collect())
        .collect();
    let mut pending: VecDeque<_> = parents
        .iter()
        .enumerate()
        .filter_map(|(index, parent)| parent.is_none().then_some(index))
        .collect();
    let mut order = Vec::with_capacity(parents.len());
    while let Some(index) = pending.pop_front() {
        order.push(index);
        pending.extend(children[index].iter().copied());
    }
    match order.len() == parents.len() {
        true => Ok((parents, order)),
        false => Err(invalid("node hierarchy", "cycle")),
    }
}

fn transform(node: gltf::Node<'_>) -> Result<Transform, ActorError> {
    let context = format!("node {} transform", node.index());
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => {
            let matrix = DMat4::from_cols_array_2d(&matrix.map(|column| column.map(f64::from)));
            affine(matrix, &context)?;
            Ok(Transform::Matrix(matrix))
        }
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => {
            let translation = DVec3::from_array(translation.map(f64::from));
            let scale = DVec3::from_array(scale.map(f64::from));
            match translation.is_finite() && scale.is_finite() {
                true => Ok(Transform::Trs {
                    translation,
                    scale,
                    rotation: quaternion(rotation, &context)?,
                }),
                false => Err(invalid(context, "nonfinite translation or scale")),
            }
        }
    }
}

pub(super) fn quaternion(value: [f32; 4], context: &str) -> Result<DQuat, ActorError> {
    let value = DQuat::from_array(value.map(f64::from));
    match value.is_finite() && value.length_squared() > 0.0 {
        true => Ok(value.normalize()),
        false => Err(invalid(context, "nonfinite or degenerate quaternion")),
    }
}

pub(super) fn affine(matrix: DMat4, context: &str) -> Result<(), ActorError> {
    match matrix.is_finite() && matrix.row(3).to_array() == [0.0, 0.0, 0.0, 1.0] {
        true => Ok(()),
        false => Err(invalid(context, "nonfinite or non-affine matrix")),
    }
}

pub(super) fn accessor(
    accessor: &gltf::Accessor<'_>,
    dimensions: Dimensions,
    types: &[DataType],
    context: &str,
) -> Result<(), ActorError> {
    match accessor.dimensions() == dimensions && types.contains(&accessor.data_type()) {
        true => {}
        false => {
            return Err(invalid(
                context,
                format!("accessor {} has invalid type", accessor.index()),
            ));
        }
    }
    if accessor.sparse().is_some() {
        return Err(unsupported(context, "sparse accessor"));
    }
    let view = accessor
        .view()
        .ok_or_else(|| unsupported(context, "accessor without buffer view"))?;
    let size = accessor.size();
    let stride = view.stride().unwrap_or(size);
    let end = accessor
        .count()
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| n.checked_add(size))
        .and_then(|n| n.checked_add(accessor.offset()));
    match (stride >= size, end) {
        (true, Some(end))
            if end <= view.length()
                && accessor
                    .offset()
                    .is_multiple_of(accessor.data_type().size())
                && stride.is_multiple_of(accessor.data_type().size()) =>
        {
            Ok(())
        }
        _ => Err(invalid(
            context,
            format!(
                "accessor {} has invalid count, stride or range",
                accessor.index()
            ),
        )),
    }
}
