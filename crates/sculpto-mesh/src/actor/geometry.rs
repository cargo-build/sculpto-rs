//! Primitive identity stays attached to its original ordered geometry.

use std::collections::HashSet;

use glam::{DMat4, DVec3, DVec4};
use gltf::accessor::{DataType, Dimensions};
use gltf::mesh::{Mode, Semantic, util::ReadWeights};

use super::{Actor, ActorError, Part, Sample, decode, invalid, unsupported};

pub(super) fn select(document: &gltf::Document, names: &[&str]) -> Result<Vec<usize>, ActorError> {
    if names.is_empty() {
        return Err(invalid(
            "actor selection",
            "at least one explicit part is required",
        ));
    }
    let mut seen = HashSet::new();
    names
        .iter()
        .map(|&name| {
            match seen.insert(name) {
                true => {}
                false => {
                    return Err(ActorError::DuplicatePart {
                        name: name.to_owned(),
                    });
                }
            }
            let candidates: Vec<_> = document
                .nodes()
                .filter(|node| node.name() == Some(name))
                .map(|node| node.index())
                .collect();
            match candidates.as_slice() {
                [index]
                    if document
                        .nodes()
                        .nth(*index)
                        .and_then(|node| node.mesh())
                        .is_some() =>
                {
                    Ok(*index)
                }
                [_] => Err(ActorError::Selection {
                    name: name.to_owned(),
                    matches: 0,
                }),
                _ => Err(ActorError::Selection {
                    name: name.to_owned(),
                    matches: candidates.len(),
                }),
            }
        })
        .collect()
}

pub(super) fn sample(
    actor: &Actor,
    selected: &[usize],
    worlds: &[DMat4],
) -> Result<Sample, ActorError> {
    let mut sample = Sample {
        vertices: Vec::new(),
        triangles: Vec::new(),
        parts: Vec::new(),
    };
    selected.iter().try_for_each(|&index| {
        let node = actor
            .document
            .nodes()
            .nth(index)
            .ok_or_else(|| invalid(format!("node {index}"), "missing selected node"))?;
        let mesh = node
            .mesh()
            .ok_or_else(|| invalid(format!("node {index}"), "missing selected mesh"))?;
        if mesh.primitives().len() == 0 {
            return Err(invalid(
                format!("node {index} mesh {}", mesh.index()),
                "mesh has no primitives",
            ));
        }
        let name = node
            .name()
            .ok_or_else(|| invalid(format!("node {index}"), "missing selected name"))?;
        let joints = match node.skin() {
            Some(skin) => Some(joint_matrices(actor, skin, worlds)?),
            None => None,
        };
        mesh.primitives().try_for_each(|primitive| {
            let context = format!(
                "part {name:?} node {index} mesh {} primitive {}",
                mesh.index(),
                primitive.index()
            );
            let start = sample.vertices.len();
            let triangle_start = sample.triangles.len();
            let vertices = primitive_vertices(
                actor,
                &primitive,
                joints.as_deref(),
                worlds[index],
                &context,
            )?;
            let triangles = primitive_triangles(actor, &primitive, vertices.len(), &context)?;
            sample.vertices.extend(vertices);
            sample.triangles.extend(
                triangles
                    .into_iter()
                    .map(|triangle| triangle.map(|i| i + start)),
            );
            sample.parts.push(Part {
                name: name.to_owned(),
                node: index,
                mesh: mesh.index(),
                primitive: primitive.index(),
                vertices: start..sample.vertices.len(),
                triangles: triangle_start..sample.triangles.len(),
            });
            Ok(())
        })
    })?;
    Ok(sample)
}

fn joint_matrices(
    actor: &Actor,
    skin: gltf::Skin<'_>,
    worlds: &[DMat4],
) -> Result<Vec<DMat4>, ActorError> {
    let context = format!("skin {} inverse binds", skin.index());
    let joints: Vec<_> = skin.joints().map(|joint| joint.index()).collect();
    if joints.is_empty() {
        return Err(invalid(&context, "empty joint list"));
    }
    let mut seen = HashSet::new();
    match joints.iter().all(|joint| seen.insert(joint)) {
        true => {}
        false => return Err(invalid(&context, "duplicate joint")),
    }
    let inverse_binds: Vec<DMat4> = match skin.inverse_bind_matrices() {
        Some(accessor) => {
            decode::accessor(&accessor, Dimensions::Mat4, &[DataType::F32], &context)?;
            match accessor.count() == joints.len() && !accessor.normalized() {
                true => {}
                false => {
                    return Err(invalid(
                        &context,
                        "inverse bind count or normalization mismatch",
                    ));
                }
            }
            skin.reader(|buffer| actor.buffer(buffer))
                .read_inverse_bind_matrices()
                .ok_or_else(|| invalid(&context, "unreadable inverse binds"))?
                .map(|matrix| {
                    let matrix =
                        DMat4::from_cols_array_2d(&matrix.map(|column| column.map(f64::from)));
                    decode::affine(matrix, &context)?;
                    Ok(matrix)
                })
                .collect::<Result<_, _>>()?
        }
        None => vec![DMat4::IDENTITY; joints.len()],
    };
    joints
        .into_iter()
        .zip(inverse_binds)
        .map(|(joint, inverse)| {
            let matrix = worlds[joint] * inverse;
            decode::affine(matrix, &context)?;
            Ok(matrix)
        })
        .collect()
}

/// One source influence row: skin-local joint indices and original weights.
pub(super) struct SkinRow {
    pub(super) joints: [u16; 4],
    pub(super) weights: [f64; 4],
}

/// Decode and validate JOINTS_0/WEIGHTS_0 shared by sampling and rig output.
/// @req DECL0013
pub(super) fn skin_rows(
    actor: &Actor,
    primitive: &gltf::Primitive<'_>,
    joint_count: usize,
    vertex_count: usize,
    context: &str,
) -> Result<Vec<SkinRow>, ActorError> {
    let joint_accessor = primitive
        .get(&Semantic::Joints(0))
        .ok_or_else(|| invalid(context, "missing JOINTS_0"))?;
    let weight_accessor = primitive
        .get(&Semantic::Weights(0))
        .ok_or_else(|| invalid(context, "missing WEIGHTS_0"))?;
    decode::accessor(
        &joint_accessor,
        Dimensions::Vec4,
        &[DataType::U8, DataType::U16],
        context,
    )?;
    decode::accessor(
        &weight_accessor,
        Dimensions::Vec4,
        &[DataType::F32, DataType::U8, DataType::U16],
        context,
    )?;
    match joint_accessor.count() == vertex_count
        && weight_accessor.count() == vertex_count
        && !joint_accessor.normalized()
        && (weight_accessor.normalized() == (weight_accessor.data_type() != DataType::F32))
    {
        true => {}
        false => {
            return Err(invalid(
                context,
                "skin attribute count or normalization mismatch",
            ));
        }
    }
    let reader = primitive.reader(|buffer| actor.buffer(buffer));
    let indices = reader
        .read_joints(0)
        .ok_or_else(|| invalid(context, "unreadable JOINTS_0"))?
        .into_u16();
    let weights: Vec<[f64; 4]> = match reader
        .read_weights(0)
        .ok_or_else(|| invalid(context, "unreadable WEIGHTS_0"))?
    {
        ReadWeights::F32(values) => values.map(|value| value.map(f64::from)).collect(),
        ReadWeights::U8(values) => values
            .map(|value| value.map(|v| f64::from(v) / 255.0))
            .collect(),
        ReadWeights::U16(values) => values
            .map(|value| value.map(|v| f64::from(v) / 65535.0))
            .collect(),
    };
    indices
        .into_iter()
        .zip(weights)
        .enumerate()
        .map(|(vertex, (joints, weights))| {
            let location = format!("{context} vertex {vertex}");
            match weights
                .iter()
                .all(|weight| weight.is_finite() && *weight >= 0.0 && *weight <= 1.0)
                && (weights.iter().sum::<f64>() - 1.0).abs() <= 2e-6
            {
                true => {}
                false => {
                    return Err(invalid(&location, "invalid skin weight total or value"));
                }
            }
            match joints
                .iter()
                .find(|&&joint| usize::from(joint) >= joint_count)
            {
                Some(&joint) => Err(invalid(
                    &location,
                    format!("joint index {joint} exceeds skin"),
                )),
                None => Ok(SkinRow { joints, weights }),
            }
        })
        .collect()
}

fn primitive_vertices(
    actor: &Actor,
    primitive: &gltf::Primitive<'_>,
    joints: Option<&[DMat4]>,
    world: DMat4,
    context: &str,
) -> Result<Vec<[f64; 3]>, ActorError> {
    match primitive.mode() {
        Mode::Triangles => {}
        mode => return Err(unsupported(context, format!("primitive mode {mode:?}"))),
    }
    if primitive.morph_targets().len() > 0 {
        return Err(unsupported(context, "morph geometry"));
    }
    if primitive.attributes().any(|(semantic, _)| matches!(semantic, Semantic::Joints(set) | Semantic::Weights(set) if set > 0)) {
        return Err(unsupported(context, "additional skin influence sets"));
    }
    let positions = primitive
        .get(&Semantic::Positions)
        .ok_or_else(|| invalid(context, "missing POSITION"))?;
    decode::accessor(&positions, Dimensions::Vec3, &[DataType::F32], context)?;
    if positions.normalized() {
        return Err(invalid(context, "normalized floating-point POSITION"));
    }
    let reader = primitive.reader(|buffer| actor.buffer(buffer));
    let points: Vec<_> = reader
        .read_positions()
        .ok_or_else(|| invalid(context, "unreadable POSITION"))?
        .map(|point| DVec3::from_array(point.map(f64::from)).extend(1.0))
        .collect();
    let transformed: Vec<_> = match joints {
        Some(matrices) => {
            let rows = skin_rows(actor, primitive, matrices.len(), points.len(), context)?;
            points
                .into_iter()
                .zip(rows)
                .map(|(point, row)| {
                    row.joints.into_iter().zip(row.weights).try_fold(
                        DVec4::ZERO,
                        |sum, (joint, weight)| {
                            // skin_rows validated each index against this skin.
                            Ok(sum + (matrices[usize::from(joint)] * point) * weight)
                        },
                    )
                })
                .collect::<Result<_, ActorError>>()?
        }
        None => points.into_iter().map(|point| world * point).collect(),
    };
    transformed
        .into_iter()
        .enumerate()
        .map(|(index, point)| match point.is_finite() {
            true => Ok(point.truncate().to_array()),
            false => Err(invalid(
                format!("{context} vertex {index}"),
                "nonfinite position",
            )),
        })
        .collect()
}

fn primitive_triangles(
    actor: &Actor,
    primitive: &gltf::Primitive<'_>,
    count: usize,
    context: &str,
) -> Result<Vec<[usize; 3]>, ActorError> {
    let indices: Vec<usize> = match primitive.indices() {
        Some(accessor) => {
            decode::accessor(
                &accessor,
                Dimensions::Scalar,
                &[DataType::U8, DataType::U16, DataType::U32],
                context,
            )?;
            if accessor.normalized() {
                return Err(invalid(context, "normalized indices"));
            }
            primitive
                .reader(|buffer| actor.buffer(buffer))
                .read_indices()
                .ok_or_else(|| invalid(context, "unreadable indices"))?
                .into_u32()
                .map(|index| index as usize)
                .collect()
        }
        None => (0..count).collect(),
    };
    match indices.len().is_multiple_of(3) && indices.iter().all(|&index| index < count) {
        true => Ok(indices.as_chunks::<3>().0.to_vec()),
        false => Err(invalid(
            context,
            "triangle index count or vertex index is invalid",
        )),
    }
}
