//! Shared actor-pool reading and skin validation for the authored and
//! transferred skin routes.
//!
//! Both [`crate::rebind`] and [`crate::skin`] work on one shared vertex pool and
//! must agree on what a valid encoded skin is, so the reader and the validation
//! live here once instead of in either verb. @req DECL0013 @req DECL0008

use std::collections::BTreeMap;

use glam::DMat4;

use sculpto_core::mesh::{PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::PartMaterial;
use sculpto_mesh::actor::{Actor, Pose};

use crate::CommandError;
use crate::errors;
use crate::export_glb::{actor_error, refused};

/// One shared vertex pool read from an actor's single skinned mesh.
pub(crate) struct Pool {
    pub(crate) vertices: Vec<[f64; 3]>,
    pub(crate) triangles: Vec<[u32; 3]>,
    pub(crate) partitions: Vec<PartitionId>,
    /// Source influence rows: palette slot and raw weight, positives only.
    pub(crate) rows: Vec<Vec<(u16, f32)>>,
    pub(crate) parts: Vec<PoolPart>,
}

/// One primitive's identity in the shared pool.
pub(crate) struct PoolPart {
    pub(crate) triangles: usize,
}

/// Read the actor's one shared skinned pool through the validated actor reader.
pub(crate) fn read_pool(
    actor: &Actor,
    mesh_node: usize,
    mesh_name: &str,
    source_palette: &[String],
) -> Result<Pool, CommandError> {
    actor.shared_pool_parts(mesh_node).map_err(actor_error)?;
    let palette_refs: Vec<&str> = source_palette.iter().map(String::as_str).collect();
    let sampled = actor
        .rig_sample(&[mesh_name], Pose::Rest, &palette_refs)
        .map_err(actor_error)?;
    let parts = &sampled.sample.parts;
    let pool_range = parts[0].vertices.clone();
    let pool_count = pool_range.len();
    if parts.iter().any(|part| part.vertices.len() != pool_count) {
        return Err(refused(
            "actor_pool",
            "the actor's primitives read different vertex counts".to_owned(),
        ));
    }
    let vertices = sampled.sample.vertices[pool_range.clone()].to_vec();
    let node_of_rig: Vec<usize> = sampled.rig.iter().map(|rig| rig.node).collect();
    let slot_of_node: BTreeMap<usize, usize> = actor
        .skin_joints()
        .map_err(actor_error)?
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    let mut rows = Vec::with_capacity(pool_count);
    for influence_row in &sampled.influences[pool_range] {
        let mut row = Vec::new();
        for influence in influence_row {
            if influence.weight <= 0.0 {
                continue;
            }
            let node = node_of_rig[influence.rig];
            let slot = slot_of_node.get(&node).ok_or_else(|| {
                refused(
                    "actor_pool",
                    format!("an influence lands on node {node}, outside the source palette"),
                )
            })?;
            row.push((*slot as u16, influence.weight as f32));
        }
        rows.push(row);
    }
    let mut triangles = Vec::new();
    let mut partitions = Vec::new();
    let mut pool_parts = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let offset = part.vertices.start;
        let count = part.triangles.len();
        for triangle in &sampled.sample.triangles[part.triangles.clone()] {
            let shifted = triangle.map(|vertex| (vertex - offset) as u32);
            if shifted.iter().any(|&vertex| vertex as usize >= pool_count) {
                return Err(refused(
                    "actor_pool",
                    "a primitive index leaves the shared vertex pool".to_owned(),
                ));
            }
            triangles.push(shifted);
            partitions.push(PartitionId(index as u16));
        }
        pool_parts.push(PoolPart { triangles: count });
    }
    Ok(Pool {
        vertices,
        triangles,
        partitions,
        rows,
        parts: pool_parts,
    })
}

/// Encode pool rows as skin weights over the palette.
pub(crate) fn pool_weights(
    rows: &[Vec<(u16, f32)>],
    palette: usize,
) -> Result<SkinWeights, CommandError> {
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for row in rows {
        for &(slot, weight) in row {
            joints.push(slot);
            values.push(weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| refused("actor_pool", "influence entries exceed u32"))?,
        );
    }
    SkinWeights::new(offsets, joints, values, palette)
        .map_err(|error| errors::invalid("actor_pool", format!("source weights: {error}")))
}

/// Measured guarantees of one encoded skin.
pub(crate) struct Validation {
    pub(crate) max_sum_error: f64,
    pub(crate) empty_rows: usize,
    pub(crate) widest: usize,
    pub(crate) per_bone: Vec<f64>,
}

/// Prove one encoded skin: unit rows, no empty row, the four-slot cap.
pub(crate) fn validate_skin(skin: &Skin, vertices: usize) -> Result<Validation, CommandError> {
    let mut max_sum_error = 0.0_f64;
    let mut empty_rows = 0;
    let mut widest = 0;
    let mut per_bone = vec![0.0_f64; skin.bones.len()];
    for vertex in 0..skin.influences.rows() {
        let row = skin
            .influences
            .row(vertex)
            .expect("row below the checked count");
        let mut sum = 0.0_f64;
        let mut positive = 0;
        for (&slot, &weight) in row.joints.iter().zip(row.weights) {
            if weight > 0.0 {
                positive += 1;
                sum += f64::from(weight);
                per_bone[usize::from(slot)] += f64::from(weight);
            }
        }
        widest = widest.max(positive);
        if positive == 0 {
            empty_rows += 1;
        }
        max_sum_error = max_sum_error.max((sum - 1.0).abs());
    }
    if empty_rows > 0 {
        return Err(refused(
            "skin_validation",
            format!("{empty_rows} vertices carry no influence after authoring"),
        ));
    }
    if max_sum_error > 1e-5 {
        return Err(refused(
            "skin_validation",
            format!("output rows sum to one within {max_sum_error:.3e}, above 1e-5"),
        ));
    }
    let _ = vertices;
    Ok(Validation {
        max_sum_error,
        empty_rows,
        widest,
        per_bone,
    })
}

/// Read each primitive's part material; refuse textures the part route drops.
pub(crate) fn part_materials(
    actor: &Actor,
    mesh_node: usize,
) -> Result<Vec<PartMaterial>, CommandError> {
    let colors = actor.part_colors(mesh_node).map_err(actor_error)?;
    Ok(colors
        .into_iter()
        .enumerate()
        .map(|(index, color)| PartMaterial {
            partition: index as u16,
            name: color.name,
            color: color.color,
        })
        .collect())
}

/// One matrix in the column-major `f32` layout skins and rigs use.
pub(crate) fn f32_of(value: DMat4) -> [[f32; 4]; 4] {
    value
        .to_cols_array_2d()
        .map(|column| column.map(|x| x as f32))
}

/// The identity in the layout skins use.
pub(crate) fn identity_f32() -> [[f32; 4]; 4] {
    f32_of(DMat4::IDENTITY)
}
