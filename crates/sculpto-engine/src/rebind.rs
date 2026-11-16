//! `actor rebind`: seat an actor's mesh onto another skeleton, transferring the
//! authored weights onto the target palette.
//!
//! The mesh keeps its geometry, partitions and part colours; only the skeleton
//! and the skin change. The transfer is the neutral remap kernel
//! ([`remap_skin`]) through an explicitly checked [`BoneMap::resolved`] table:
//! the correspondence layer ([`crate::retarget`]) resolves each target bone onto
//! a source bone, the target hierarchy is seated at the source's rest, and every
//! source weight the target cannot name folds to its nearest mapped source
//! ancestor. The fold is reported per family (breast, twist, helpers) with its
//! raw weight mass, so the soft tissue a pure target rig trades away is named
//! before a render shows it, not after.
//!
//! Everything is measured in world glTF Y-up metres with an identity basis and
//! global-to-skin: the source rest is proved, the seat is proved, and the
//! encoded output is proved again under the seated rig's own basis before a byte
//! is written. Every output vertex sums to one and carries at least one
//! influence, or the verb refuses. @req DECL0008 @req DECL0013

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use glam::{DMat4, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::review::smooth_normals;
use sculpto_gltf::{PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::Actor;
use sculpto_mesh::rig::map::{BoneMap, Destination};
use sculpto_mesh::rig::remap::{RemapSettings, remap_skin};

use crate::actor_io::{
    Pool, Validation, f32_of, identity_f32, part_materials, pool_weights, read_pool, validate_skin,
};
use crate::adapt::{actor_rest_worlds, actor_rig};
use crate::export_glb::{
    actor_error, bounds, glb_error, hex as sha_hex, refused, rest_error, write_report,
};
use crate::retarget::{
    Correspondence, Part, compact, correspondence_report, require_mapped, role_of,
};
use crate::{CommandError, errors};

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebindParams {
    /// Actor GLB whose mesh and authored weights are transferred.
    pub actor: PathBuf,
    /// Skeleton GLB whose skin palette names the target rig.
    pub skeleton: PathBuf,
    /// Write the re-bound actor GLB here.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Write the JSON report here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// One folded source bone: its raw mass and the target that carries it.
struct FoldRow {
    name: String,
    group: &'static str,
    mass: f64,
    target: String,
}

/// Seat the target hierarchy at the source rest and report the per-bone shift.
///
/// A mapped target bone takes its source bone's rest world; an unmapped tip
/// (the fourth finger segment, `Toe_End`, `HeadTop_End`) keeps its authored
/// offset from the seated parent. Locals are recomposed from the seated worlds.
fn seat_rig(
    target_rig: &Rig,
    target_palette: &[String],
    seat_world: &BTreeMap<JointId, DMat4>,
) -> Result<(Rig, Value), CommandError> {
    let authored: Vec<DMat4> = target_palette
        .iter()
        .enumerate()
        .map(|(slot, _)| -> Result<DMat4, CommandError> {
            Ok(DMat4::from_cols_array_2d(
                &target_rig
                    .rest_world(JointId::new(slot as u32))
                    .map_err(|error| {
                        errors::invalid("rebind_seat", format!("target rest world: {error}"))
                    })?,
            ))
        })
        .collect::<Result<_, CommandError>>()?;
    let mut seated: Vec<Option<DMat4>> = vec![None; target_palette.len()];
    fn resolve(
        slot: usize,
        target_rig: &Rig,
        seat_world: &BTreeMap<JointId, DMat4>,
        authored: &[DMat4],
        seated: &mut Vec<Option<DMat4>>,
    ) -> Result<DMat4, CommandError> {
        if let Some(world) = seated[slot] {
            return Ok(world);
        }
        let joint = JointId::new(slot as u32);
        let world = match seat_world.get(&joint) {
            Some(world) => *world,
            None => {
                let parent = target_rig.joint(joint).and_then(|joint| joint.parent);
                let Some(parent) = parent else {
                    return Err(refused(
                        "rebind_seat",
                        format!(
                            "skeleton root {:?} has no source bone to seat it; the correspondence \
                             mapped no root",
                            target_rig.joint(joint).and_then(|joint| joint.name.clone())
                        ),
                    ));
                };
                let parent_world =
                    resolve(parent.index(), target_rig, seat_world, authored, seated)?;
                // The authored offset is extracted scale-free: a target skeleton
                // exported with a unit scale (Blender's 0.01 armature) would
                // otherwise amplify the tip offset by the inverse scale and throw
                // the leaf metres away. The seated parent's own scale still applies.
                let relative =
                    scale_free(authored[parent.index()]).inverse() * scale_free(authored[slot]);
                parent_world * relative
            }
        };
        seated[slot] = Some(world);
        Ok(world)
    }
    let mut worlds = Vec::with_capacity(target_palette.len());
    for slot in 0..target_palette.len() {
        worlds.push(resolve(
            slot,
            target_rig,
            seat_world,
            &authored,
            &mut seated,
        )?);
    }
    let mut joints = Vec::with_capacity(target_palette.len());
    let mut shifts = Vec::new();
    let mut max_shift = 0.0_f64;
    for (slot, name) in target_palette.iter().enumerate() {
        let parent = target_rig
            .joint(JointId::new(slot as u32))
            .and_then(|joint| joint.parent);
        let parent_world = match parent {
            Some(parent) => worlds[parent.index()],
            None => DMat4::IDENTITY,
        };
        let rest = local_of(parent_world.inverse() * worlds[slot], name)?;
        let shift = worlds[slot]
            .w_axis
            .truncate()
            .distance(authored[slot].w_axis.truncate());
        max_shift = max_shift.max(shift);
        shifts.push(json!({
            "bone": name,
            "seated": seat_world.contains_key(&JointId::new(slot as u32)),
            "rest_shift_metres": shift,
        }));
        joints.push(Joint {
            name: Some(name.clone()),
            parent,
            rest,
        });
    }
    let rig = Rig::new(joints, RigFrame::YUp).map_err(|error| {
        errors::invalid(
            "rebind_seat",
            format!("the seated skeleton is not a rig: {error}"),
        )
    })?;
    let report = json!({
        "bones": target_palette.len(),
        "seated": seat_world.len(),
        "tips_kept": target_palette.len() - seat_world.len(),
        "max_rest_shift_metres": max_shift,
        "per_bone": shifts,
    });
    Ok((rig, report))
}

/// Decompose a local matrix into the rig model's uniform-scale TRS.
fn local_of(local: DMat4, name: &str) -> Result<LocalTransform, CommandError> {
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    if (scale.x - scale.y).abs() > 1e-6 || (scale.x - scale.z).abs() > 1e-6 {
        return Err(refused(
            "rebind_seat",
            format!("bone {name:?} carries a non-uniform rest scale ({scale:?})"),
        ));
    }
    Ok(LocalTransform {
        translation: translation.to_array().map(|value| value as f32),
        rotation: rotation.normalize().to_array().map(|value| value as f32),
        scale: scale.x as f32,
    })
}

/// One affine with its scale divided out: rotation and translation only.
fn scale_free(value: DMat4) -> DMat4 {
    let (_, rotation, translation) = value.to_scale_rotation_translation();
    DMat4::from_scale_rotation_translation(DVec3::ONE, rotation, translation)
}

/// The family a folded bone belongs to, for the named soft-tissue census.
fn fold_group(name: &str) -> &'static str {
    if compact(name).contains("breast") {
        "breast"
    } else if matches!(
        role_of(name).map(|role| role.part),
        Some(Part::Twist { .. })
    ) {
        "twist"
    } else {
        "helpers"
    }
}

/// Rebind one actor's mesh onto another skeleton.
///
/// # Errors
/// A missing or unreadable file, an actor whose mesh is not one shared skinned
/// pool, a correspondence that resolves nothing, a weighted source bone with no
/// mapped ancestor, a rest proof that fails, or an output row that is empty or
/// does not sum to one.
pub fn run(params: &RebindParams) -> Result<Value, CommandError> {
    let actor_bytes =
        std::fs::read(&params.actor).map_err(|error| errors::io(&params.actor, error))?;
    let actor = Actor::from_glb(&actor_bytes).map_err(actor_error)?;
    let skeleton_bytes =
        std::fs::read(&params.skeleton).map_err(|error| errors::io(&params.skeleton, error))?;
    let skeleton = Actor::from_glb(&skeleton_bytes).map_err(actor_error)?;

    let (source_rig, source_palette) = actor_rig(&actor)?;
    let (target_rig, target_palette) = actor_rig(&skeleton)?;
    if source_rig.roots().len() != 1 {
        return Err(refused(
            "rebind_source",
            format!(
                "the source palette has {} roots; rebind needs one skeleton root",
                source_rig.roots().len()
            ),
        ));
    }

    // One skinned mesh node, one shared vertex pool across its primitives: the
    // engine actor layout. A different layout is a named refusal, not a guess.
    let (mesh_node, mesh_name) = actor.skinned_mesh_node().map_err(actor_error)?;
    let pool = read_pool(&actor, mesh_node, &mesh_name, &source_palette)?;

    // The correspondence resolves each target bone onto a source node; invert it
    // into a source-palette -> target-joint map and seat the target at the source
    // rest.
    let correspondence = Correspondence::build(&actor, &target_rig, &target_palette)?;
    require_mapped(&correspondence, &params.skeleton.to_string_lossy())?;
    let source_worlds = actor_rest_worlds(&actor)?;
    let source_joints = actor.skin_joints().map_err(actor_error)?;
    let slot_of_node: BTreeMap<usize, usize> = source_joints
        .iter()
        .enumerate()
        .map(|(slot, node)| (*node, slot))
        .collect();
    let mut direct: Vec<Option<JointId>> = vec![None; source_palette.len()];
    let mut seat_world: BTreeMap<JointId, DMat4> = BTreeMap::new();
    for pair in &correspondence.pairs {
        let Some(node) = pair.source else { continue };
        seat_world.insert(pair.joint, source_worlds[node]);
        if let Some(&slot) = slot_of_node.get(&node) {
            direct[slot] = Some(pair.joint);
        }
    }
    let (seated_rig, seat_report) = seat_rig(&target_rig, &target_palette, &seat_world)?;

    // Resolve every source joint: direct, folded to its nearest mapped ancestor,
    // or unsupported. A weighted bone with no mapped ancestor is a refusal.
    let mut mass = vec![0.0_f64; source_palette.len()];
    for row in &pool.rows {
        for &(slot, weight) in row {
            mass[usize::from(slot)] += f64::from(weight);
        }
    }
    let total_mass: f64 = mass.iter().sum();
    let mut destinations = Vec::with_capacity(source_palette.len());
    let mut folds: Vec<FoldRow> = Vec::new();
    for (slot, name) in source_palette.iter().enumerate() {
        let joint = JointId::new(slot as u32);
        if let Some(target) = direct[slot] {
            destinations.push(Destination::Direct(target));
            continue;
        }
        let ancestor = nearest_mapped_ancestor(&source_rig, joint, &direct);
        match ancestor {
            Some(target) => {
                destinations.push(Destination::Collapsed(target));
                if mass[slot] > 0.0 {
                    folds.push(FoldRow {
                        name: name.clone(),
                        group: fold_group(name),
                        mass: mass[slot],
                        target: target_palette[target.index()].clone(),
                    });
                }
            }
            None if mass[slot] > 0.0 => {
                return Err(refused(
                    "rebind_fold",
                    format!(
                        "source bone {name:?} carries weight mass {:.4} but no mapped ancestor; \
                         the target skeleton cannot carry it",
                        mass[slot]
                    ),
                ));
            }
            None => destinations.push(Destination::Unsupported),
        }
    }

    // The source rest, proved in world Y-up with an identity basis.
    let positions: Vec<[f32; 3]> = pool
        .vertices
        .iter()
        .map(|point| point.map(|value| value as f32))
        .collect();
    let source_skin = Skin {
        bones: source_palette.clone(),
        bind_inverse: (0..source_palette.len())
            .map(|slot| {
                f32_of(
                    DMat4::from_cols_array_2d(
                        &source_rig
                            .rest_world(JointId::new(slot as u32))
                            .expect("source palette joint resolves"),
                    )
                    .inverse(),
                )
            })
            .collect(),
        influences: pool_weights(&pool.rows, source_palette.len())?,
    };
    let source_root = source_rig.roots()[0];
    let source_binding = RestBinding::prove_with_basis(
        &source_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: source_root,
            positions: &positions,
            skin: &source_skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;

    let map = BoneMap::resolved(&source_binding, &seated_rig, &destinations)
        .map_err(|error| errors::invalid("rebind_map", error.to_string()))?;

    // The seat carrier: the mesh rigidly bound to the seated root. It proves the
    // seated rig carries the geometry at rest and supplies the kernel its target
    // root and basis; the transferred output is proved again below.
    let seat_root = seated_rig.roots()[0];
    let seat_skin = rigid_root_skin(&seated_rig, seat_root, positions.len())?;
    let seat_binding = RestBinding::prove_with_basis(
        &seated_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: seat_root,
            positions: &positions,
            skin: &seat_skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;

    let remapped = remap_skin(
        &source_binding,
        &map,
        &seat_binding,
        &RemapSettings {
            source_shape: 0,
            target_shape: 0,
            locked_source_joints: Vec::new(),
        },
    )
    .map_err(|error| errors::invalid("rebind_transfer", error.to_string()))?;

    // The palette is the skeleton's, not the weight census's: keep every target
    // bone so a clip authored on the full skeleton drives it without a conversion
    // layer. Unreached tips carry zero weight.
    let skin = complete_palette(&remapped.skin, &seated_rig, &target_palette)?;
    let validation = validate_skin(&skin, positions.len())?;

    let normals = smooth_normals(&positions, &pool.triangles);
    let asset = MeshAsset {
        name: mesh_name.to_owned(),
        positions: positions.clone(),
        normals,
        triangles: pool.triangles.clone(),
        triangle_partitions: pool.partitions.clone(),
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let final_binding = RestBinding::prove_with_basis(
        &seated_rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: seat_root,
            positions: &asset.positions,
            skin: &skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;
    let measurement = final_binding.skins()[0].measurement();
    let materials = part_materials(&actor, mesh_node)?;
    let bytes = write_partitioned_rigged_glb(&asset, &final_binding, 0, &[], &materials)
        .map_err(glb_error)?;
    if let Some(path) = &params.out {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        std::fs::write(path, &bytes).map_err(|error| errors::io(path, error))?;
    }

    let report = build_report(ReportInputs {
        params,
        actor_bytes: &actor_bytes,
        skeleton_bytes: &skeleton_bytes,
        correspondence: &correspondence,
        seat_report,
        source_palette: &source_palette,
        target_palette: &target_palette,
        folds: &folds,
        total_mass,
        mass: &mass,
        direct: &direct,
        validation: &validation,
        measurement,
        remap_discarded: remapped.report.discarded.len(),
        pool: &pool,
        materials: &materials,
        bytes: &bytes,
        positions: &positions,
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

fn identity_f64() -> [[f64; 4]; 4] {
    DMat4::IDENTITY.to_cols_array_2d()
}

/// The nearest ancestor of `joint` that maps directly to a target bone.
fn nearest_mapped_ancestor(
    rig: &Rig,
    joint: JointId,
    direct: &[Option<JointId>],
) -> Option<JointId> {
    let mut cursor = rig.joint(joint).and_then(|joint| joint.parent);
    while let Some(parent) = cursor {
        if let Some(target) = direct[parent.index()] {
            return Some(target);
        }
        cursor = rig.joint(parent).and_then(|joint| joint.parent);
    }
    None
}

/// One skin row per pool vertex: palette slots and raw weights.
/// Every vertex rigidly bound to one root: a valid seat carrier skin.
fn rigid_root_skin(rig: &Rig, root: JointId, vertices: usize) -> Result<Skin, CommandError> {
    let name = rig
        .joint(root)
        .and_then(|joint| joint.name.clone())
        .ok_or_else(|| refused("rebind_seat", "the seated root is unnamed".to_owned()))?;
    let world = DMat4::from_cols_array_2d(
        &rig.rest_world(root)
            .map_err(|error| errors::invalid("rebind_seat", format!("seat root: {error}")))?,
    );
    let offsets: Vec<u32> = (0..=vertices as u32).collect();
    let joints = vec![0_u16; vertices];
    let values = vec![1.0_f32; vertices];
    let influences = SkinWeights::new(offsets, joints, values, 1)
        .map_err(|error| errors::invalid("rebind_seat", format!("seat skin: {error}")))?;
    Ok(Skin {
        bones: vec![name],
        bind_inverse: vec![f32_of(world.inverse())],
        influences,
    })
}

/// Extend the transferred skin to the full target palette, zero-weight tips kept.
fn complete_palette(
    transferred: &Skin,
    seated_rig: &Rig,
    target_palette: &[String],
) -> Result<Skin, CommandError> {
    let slot_of_name: BTreeMap<&str, usize> = target_palette
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.as_str(), slot))
        .collect();
    let remap_slot: Vec<usize> = transferred
        .bones
        .iter()
        .map(|name| {
            slot_of_name.get(name.as_str()).copied().ok_or_else(|| {
                refused(
                    "rebind_transfer",
                    format!("transferred bone {name:?} is outside the target palette"),
                )
            })
        })
        .collect::<Result<_, CommandError>>()?;
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for vertex in 0..transferred.influences.rows() {
        let row = transferred
            .influences
            .row(vertex)
            .expect("row below the checked count");
        for (&slot, &weight) in row.joints.iter().zip(row.weights) {
            if weight <= 0.0 {
                continue;
            }
            joints.push(
                u16::try_from(remap_slot[usize::from(slot)])
                    .map_err(|_| refused("rebind_transfer", "target palette exceeds u16"))?,
            );
            values.push(weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| refused("rebind_transfer", "influence entries exceed u32"))?,
        );
    }
    let influences = SkinWeights::new(offsets, joints, values, target_palette.len())
        .map_err(|error| errors::invalid("rebind_transfer", format!("output weights: {error}")))?;
    let bind_inverse = (0..target_palette.len())
        .map(|slot| {
            f32_of(
                DMat4::from_cols_array_2d(
                    &seated_rig
                        .rest_world(JointId::new(slot as u32))
                        .expect("target palette joint resolves"),
                )
                .inverse(),
            )
        })
        .collect();
    Ok(Skin {
        bones: target_palette.to_vec(),
        bind_inverse,
        influences,
    })
}

/// Everything the report folds together.
struct ReportInputs<'a> {
    params: &'a RebindParams,
    actor_bytes: &'a [u8],
    skeleton_bytes: &'a [u8],
    correspondence: &'a Correspondence,
    seat_report: Value,
    source_palette: &'a [String],
    target_palette: &'a [String],
    folds: &'a [FoldRow],
    total_mass: f64,
    mass: &'a [f64],
    direct: &'a [Option<JointId>],
    validation: &'a Validation,
    measurement: sculpto_core::rig::rest::RestMeasurement,
    remap_discarded: usize,
    pool: &'a Pool,
    materials: &'a [PartMaterial],
    bytes: &'a [u8],
    positions: &'a [[f32; 3]],
}

fn build_report(input: ReportInputs<'_>) -> Value {
    let mut groups: BTreeMap<&str, (f64, BTreeSet<String>, Vec<Value>)> = BTreeMap::new();
    for fold in input.folds {
        let entry = groups.entry(fold.group).or_default();
        entry.0 += fold.mass;
        entry.1.insert(fold.target.clone());
        entry.2.push(json!({
            "bone": fold.name,
            "mass": fold.mass,
            "share": fold.mass / input.total_mass,
            "target": fold.target,
        }));
    }
    let folded_mass: f64 = input.folds.iter().map(|fold| fold.mass).sum();
    let group_rows: Vec<Value> = groups
        .into_iter()
        .map(|(group, (mass, targets, bones))| {
            json!({
                "group": group,
                "mass": mass,
                "share": mass / input.total_mass,
                "targets": targets.into_iter().collect::<Vec<_>>(),
                "bones": bones,
            })
        })
        .collect();
    let mapped_mass: f64 = input
        .mass
        .iter()
        .zip(input.direct)
        .filter(|(_, direct)| direct.is_some())
        .map(|(mass, _)| *mass)
        .sum();
    let per_bone: Vec<Value> = input
        .target_palette
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            json!({
                "bone": name,
                "mass": input.validation.per_bone[slot],
                "share": input.validation.per_bone[slot] / input.positions.len() as f64,
            })
        })
        .collect();
    json!({
        "actor": input.params.actor.to_string_lossy(),
        "actor_sha256": sha_hex(&Sha256::digest(input.actor_bytes)),
        "skeleton": input.params.skeleton.to_string_lossy(),
        "skeleton_sha256": sha_hex(&Sha256::digest(input.skeleton_bytes)),
        "out": input.params.out.as_ref().map(|path| path.to_string_lossy().into_owned()),
        "bytes": input.bytes.len(),
        "sha256": sha_hex(&Sha256::digest(input.bytes)),
        "correspondence": correspondence_report(input.correspondence),
        "seat": input.seat_report,
        "palette": {
            "source_bones": input.source_palette.len(),
            "target_bones": input.target_palette.len(),
            "mapped_source_bones": input.direct.iter().filter(|d| d.is_some()).count(),
            "weighted_target_bones": input.validation.per_bone.iter().filter(|m| **m > 0.0).count(),
        },
        "weight_census": {
            "total_raw_mass": input.total_mass,
            "mapped_share": mapped_mass / input.total_mass,
            "folded_share": folded_mass / input.total_mass,
            "groups": group_rows,
        },
        "validation": {
            "vertices": input.positions.len(),
            "max_sum_error": input.validation.max_sum_error,
            "empty_rows": input.validation.empty_rows,
            "widest_row": input.validation.widest,
            "discarded_weights": input.remap_discarded,
            "rest": {
                "max_vertex": input.measurement.max_vertex,
                "max_influence": input.measurement.max_influence,
                "max_joint": input.measurement.max_joint,
            },
        },
        "per_bone_mass": per_bone,
        "parts": input.pool.parts.iter().zip(input.materials).map(|(part, material)| json!({
            "name": material.name,
            "color": material.color,
            "triangles": part.triangles,
        })).collect::<Vec<_>>(),
        "bounds": bounds(input.positions),
        "statement": "the mesh keeps its geometry, partitions and part colours; the skeleton is \
                      the target's, seated at the source rest, and every source weight the target \
                      cannot name folds to its nearest mapped ancestor, reported per family",
    })
}

/// The report's one-line verdict, for the caller that prints it.
pub fn verdict(report: &Value) -> String {
    let validation = &report["validation"];
    format!(
        "rebound · {} target bones · {:.2}% mapped / {:.2}% folded · sum error {:.2e} · rest {:.2e}",
        report["palette"]["target_bones"],
        report["weight_census"]["mapped_share"]
            .as_f64()
            .unwrap_or(0.0)
            * 100.0,
        report["weight_census"]["folded_share"]
            .as_f64()
            .unwrap_or(0.0)
            * 100.0,
        validation["max_sum_error"].as_f64().unwrap_or(0.0),
        validation["rest"]["max_vertex"].as_f64().unwrap_or(0.0),
    )
}
