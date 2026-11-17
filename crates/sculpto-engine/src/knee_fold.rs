//! Author bounded knee weights and standard glTF bend correctives on an adapted actor.

use glam::{DVec3, Vec3};
use sculpto_gltf::{pack::PackWriter, review::smooth_normals};
use sculpto_mesh::actor::{Actor, Pose};
use sculpto_mesh::rig::author::knee::{
    FLEXION_DEGREES, KneeChain, KneeTransition, flexion, knee_transitions, shape_weights,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::CommandError;
use crate::actor_io::read_pool;
use crate::export_glb::{actor_error, hex, refused};
use crate::retarget::{BoneRole, Part, Side, palette_role};

fn failure(message: impl ToString) -> CommandError {
    refused("knee_fold", message.to_string())
}

fn landmarks(palette: &[String], side: Side) -> Result<[usize; 4], CommandError> {
    let mut result = [0; 4];
    for (i, part) in [Part::Thigh, Part::Calf, Part::Foot, Part::Toe]
        .into_iter()
        .enumerate()
    {
        let role = BoneRole { side, part };
        let mut hits = palette
            .iter()
            .enumerate()
            .filter(|(_, name)| palette_role(name) == Some(role));
        match (hits.next(), hits.next()) {
            (Some((slot, _)), None) => result[i] = slot,
            _ => {
                return Err(failure(format!(
                    "expected one {} in the palette",
                    role.label()
                )));
            }
        }
    }
    Ok(result)
}

/// Preserve the original clip and mesh, replacing the knee influence pairs and optionally adding angle-driven targets. @req DECL0013
pub fn prepare(bytes: &[u8], with_shapes: bool) -> Result<(Vec<u8>, Value), CommandError> {
    let actor = Actor::from_glb(bytes).map_err(actor_error)?;
    let (node, name) = actor.skinned_mesh_node().map_err(actor_error)?;
    let mesh = actor
        .document()
        .nodes()
        .nth(node)
        .and_then(|n| n.mesh())
        .ok_or_else(|| failure("missing mesh"))?;
    let parts = actor.shared_pool_parts(node).map_err(actor_error)?;
    let frame = actor.shared_skin_frame(node).map_err(actor_error)?;
    let joints = actor.skin_joints().map_err(actor_error)?;
    let palette: Vec<_> = joints
        .iter()
        .map(|&n| {
            actor
                .node_name(n)
                .map(str::to_owned)
                .ok_or_else(|| failure("unnamed palette joint"))
        })
        .collect::<Result<_, _>>()?;
    let mut pool = read_pool(&actor, node, &name, &palette)?;
    let slots = [
        landmarks(&palette, Side::Left)?,
        landmarks(&palette, Side::Right)?,
    ];
    let rest = actor.skeleton_worlds(Pose::Rest).map_err(actor_error)?;
    for side in slots {
        if side
            .windows(2)
            .any(|pair| actor.node_parents()[joints[pair[1]]] != Some(joints[pair[0]]))
        {
            return Err(failure(
                "the knee requires a direct thigh/shin/foot/toe chain",
            ));
        }
    }
    let chains = slots.map(|s| KneeChain {
        thigh: s[0] as u16,
        shin: s[1] as u16,
        landmarks: s.map(|i| rest[joints[i]].w_axis.truncate()),
    });
    let mut flesh = vec![false; pool.vertices.len()];
    for (face, part) in pool.triangles.iter().zip(&pool.partitions) {
        if part.0 == 0 {
            face.iter().for_each(|&i| flesh[i as usize] = true);
        }
    }
    let folds = knee_transitions(
        &pool.vertices,
        &pool.triangles,
        &flesh,
        &mut pool.rows,
        &chains,
        with_shapes,
    )
    .map_err(failure)?;
    let mut writer = PackWriter::from_glb(bytes).map_err(failure)?;
    writer
        .set_shared_skin(mesh.index(), parts, &pool.rows)
        .map_err(failure)?;
    let mut tracks = Vec::new();
    if with_shapes {
        let inverse = frame.inverse();
        let base: Vec<_> = pool
            .vertices
            .iter()
            .map(|&v| {
                inverse
                    .transform_point3(DVec3::from_array(v))
                    .as_vec3()
                    .to_array()
            })
            .collect();
        let normals = smooth_normals(&base, &pool.triangles);
        let mut targets = Vec::new();
        let mut names = Vec::new();
        for (side, fold) in ["Left", "Right"].into_iter().zip(&folds) {
            for (angle, target) in FLEXION_DEGREES.into_iter().zip(&fold.targets) {
                let delta: Vec<_> = target
                    .iter()
                    .map(|&v| inverse.transform_vector3(v).as_vec3().to_array())
                    .collect();
                let moved: Vec<_> = base
                    .iter()
                    .zip(&delta)
                    .map(|(&p, &d)| (Vec3::from_array(p) + Vec3::from_array(d)).to_array())
                    .collect();
                let normal_delta: Vec<_> = smooth_normals(&moved, &pool.triangles)
                    .iter()
                    .zip(&normals)
                    .map(|(&m, &n)| (Vec3::from_array(m) - Vec3::from_array(n)).to_array())
                    .collect();
                let pos = writer.morph_vectors(&delta).map_err(failure)?;
                let norm = writer.morph_vectors(&normal_delta).map_err(failure)?;
                targets.push(vec![
                    ("POSITION".to_owned(), pos),
                    ("NORMAL".to_owned(), norm),
                ]);
                names.push(format!("{side}KneeFlex{angle}"));
            }
        }
        for part in 0..parts {
            writer
                .set_targets(mesh.index(), part, &targets)
                .map_err(failure)?;
        }
        writer
            .set_target_names(mesh.index(), Some(&names))
            .map_err(failure)?;
        writer
            .set_mesh_weights(mesh.index(), names.len())
            .map_err(failure)?;
        writer
            .set_node_weights(node, names.len())
            .map_err(failure)?;
        let nodes: Vec<_> = slots
            .iter()
            .flat_map(|s| s[..3].iter().map(|&i| joints[i]))
            .collect();
        for clip in actor.document().animations() {
            let keys = actor
                .joint_position_keys(clip.index(), &nodes)
                .map_err(actor_error)?;
            let (times, weights) = driver(&keys, &folds)?;
            writer
                .write_morph_track(clip.index(), None, node, &times, &weights)
                .map_err(failure)?;
            let peaks: Vec<_> = (0..names.len())
                .map(|k| {
                    weights
                        .iter()
                        .skip(k)
                        .step_by(names.len())
                        .copied()
                        .fold(0.0_f32, f32::max)
                })
                .collect();
            tracks.push(json!({
                "clip": clip.name(), "keys": times.len(), "peak_weights": peaks
            }));
        }
    }
    let encoded = writer.finish().map_err(failure)?;
    let written = Actor::from_glb(&encoded).map_err(actor_error)?;
    let reconstructed = written
        .sample(&[name.as_str()], Pose::Rest)
        .map_err(actor_error)?;
    let residual = pool
        .vertices
        .iter()
        .zip(&reconstructed.vertices)
        .map(|(&a, &b)| DVec3::from_array(a).distance(DVec3::from_array(b)))
        .fold(0.0, f64::max);
    if residual > 1e-5 {
        return Err(failure(format!("bind shape drifted {residual} m")));
    }
    let sides: Vec<_> = ["Left", "Right"]
        .into_iter()
        .zip(&folds)
        .map(|(side, fold)| {
            json!({
                "side": side,
                "rest_angle_deg": fold.rest_angle,
                "changed_rows": fold.changed_rows,
                "free_vertices": fold.free_vertices,
                "core_rims_m": fold.core_rims,
                "pair_mix_10_90_m": fold.mixed_extent,
                "pair_mix_45_55_m": fold.equal_extent
            })
        })
        .collect();
    let report = json!({
        "command": "actor knee-fold",
        "with_shapes": with_shapes,
        "input_sha256": hex(&Sha256::digest(bytes)),
        "output_sha256": hex(&Sha256::digest(&encoded)),
        "rest_max_m": residual,
        "target_angles_deg": FLEXION_DEGREES,
        "sides": sides,
        "morph_tracks": tracks
    });
    Ok((encoded, report))
}

fn driver(
    keys: &[(f32, Vec<DVec3>)],
    folds: &[KneeTransition],
) -> Result<(Vec<f32>, Vec<f32>), CommandError> {
    let mut times = Vec::with_capacity(keys.len());
    let mut weights = Vec::with_capacity(keys.len() * 4);
    for (time, p) in keys {
        times.push(*time);
        for (leg, fold) in p.as_chunks::<3>().0.iter().zip(folds) {
            let angle = flexion(leg[0], leg[1], leg[2]).map_err(failure)?;
            weights.extend(shape_weights(fold.rest_angle, angle));
        }
    }
    Ok((times, weights))
}
