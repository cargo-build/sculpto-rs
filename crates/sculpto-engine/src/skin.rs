//! `actor skin`: author the skin for an actor's own deform skeleton from the
//! flesh, replacing whatever weights the input carried.
//!
//! The transfer route ([`crate::rebind`]) moves somebody else's authored weights
//! onto another palette and folds what the palette cannot name. This verb does
//! the opposite: it reads the rig and the geometry, decides which bone owns
//! which flesh on the surface, and encodes a fresh skin. The mesh, the skeleton,
//! the partitions and the part colours are untouched.
//!
//! Everything the encoded skin must satisfy is measured on the output, not on
//! the intermediate field: unit rows, no empty row, the four-slot cap, no
//! positive weight dropped above `0.05`, no weight on a leaf bone, and no
//! opposite-side influence on a vertex clear of the midline. @req DECL0008
//! @req DECL0013

use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::DMat4;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{JointId, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_gltf::review::smooth_normals;
use sculpto_gltf::write_partitioned_rigged_glb;
use sculpto_mesh::actor::Actor;
use sculpto_mesh::rig::author::{AuthorSettings, BodySide, DeformBone, author_weights};

use crate::CommandError;
use crate::actor_io::{f32_of, identity_f32, part_materials, read_pool, validate_skin};
use crate::adapt::actor_rig;
use crate::errors;
use crate::export_glb::{
    actor_error, bounds, glb_error, hex as sha_hex, refused, rest_error, write_report,
};

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkinParams {
    /// Actor GLB whose mesh and skeleton the authored skin is built on.
    pub actor: PathBuf,
    /// Write the re-skinned actor GLB here.
    #[serde(default)]
    pub out: Option<PathBuf>,
    /// Write the JSON report here.
    #[serde(default)]
    pub report: Option<PathBuf>,
    /// Refuse the actor unless its `sha256` starts with this hex prefix.
    ///
    /// A chain that consumes a file by name silently re-authors whatever sits at
    /// that path; the stale seat that produced the M2 audit was exactly that. The
    /// expectation travels with the command, so a superseded input is refused
    /// before any weight is authored. At least eight hex digits.
    #[serde(default)]
    pub expect_input_sha: Option<String>,
}

/// Author a skin for one actor's own skeleton.
///
/// # Errors
/// A missing or unreadable file, an actor that is not one shared skinned pool,
/// a rig without one root, an empty deform-bone set, a refused authoring step,
/// or an encoded skin that fails its own proof.
pub fn run(params: &SkinParams) -> Result<Value, CommandError> {
    let actor_bytes =
        std::fs::read(&params.actor).map_err(|error| errors::io(&params.actor, error))?;
    let actor_sha = sha_hex(&Sha256::digest(&actor_bytes));
    if let Some(expected) = &params.expect_input_sha {
        verify_input_sha(&actor_sha, expected)?;
    }
    let actor = Actor::from_glb(&actor_bytes).map_err(actor_error)?;
    let (rig, palette) = actor_rig(&actor)?;
    if rig.roots().len() != 1 {
        return Err(refused(
            "skin_roots",
            format!(
                "the palette has {} roots; authoring needs one skeleton root",
                rig.roots().len()
            ),
        ));
    }
    let (mesh_node, mesh_name) = actor.skinned_mesh_node().map_err(actor_error)?;
    let pool = read_pool(&actor, mesh_node, &mesh_name, &palette)?;

    // Partition 0 is the body; every later primitive is a layer that inherits
    // the flesh it lies on instead of authoring a second, independent field.
    let mut flesh = vec![false; pool.vertices.len()];
    for (triangle, partition) in pool.triangles.iter().zip(&pool.partitions) {
        if partition.0 != 0 {
            continue;
        }
        for index in triangle {
            flesh[*index as usize] = true;
        }
    }
    if !flesh.iter().any(|flag| *flag) {
        return Err(refused(
            "skin_parts",
            "no primitive of the actor is the body".to_owned(),
        ));
    }
    let mut input_mass = vec![0.0_f64; palette.len()];
    for row in &pool.rows {
        for &(slot, weight) in row {
            input_mass[usize::from(slot)] += f64::from(weight);
        }
    }
    let palette_bones = deform_bones(&rig, &palette, &input_mass)?;
    let PaletteBones {
        bones,
        leaves,
        dropped: duplicates,
    } = palette_bones;
    if bones.is_empty() {
        return Err(refused(
            "skin_bones",
            "every bone in the palette is a leaf; nothing can carry flesh".to_owned(),
        ));
    }
    let settings = AuthorSettings::default();
    let authored = author_weights(&pool.vertices, &pool.triangles, &bones, &flesh, &settings)
        .map_err(|error| refused("skin_author", error.to_string()))?;

    let slot_of: BTreeMap<&str, usize> = palette
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.as_str(), slot))
        .collect();
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for (vertex, row) in authored.rows.iter().enumerate() {
        for (name, weight) in row {
            let slot = slot_of.get(name.as_str()).ok_or_else(|| {
                refused(
                    "skin_author",
                    format!("vertex {vertex}: bone {name:?} is not in the palette"),
                )
            })?;
            joints.push(
                u16::try_from(*slot).map_err(|_| {
                    refused("skin_author", format!("palette slot {slot} exceeds u16"))
                })?,
            );
            values.push(*weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| refused("skin_author", "influence entries exceed u32"))?,
        );
    }
    let influences = SkinWeights::new(offsets, joints, values, palette.len())
        .map_err(|error| refused("skin_author", format!("encoded rows: {error}")))?;
    let bind_inverse = (0..palette.len())
        .map(|slot| {
            let world =
                DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(slot as u32)).map_err(
                    |error| errors::invalid("skin_encode", format!("rest world: {error}")),
                )?);
            Ok(f32_of(world.inverse()))
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let skin = Skin {
        bones: palette.clone(),
        bind_inverse,
        influences,
    };
    let validation = validate_skin(&skin, pool.vertices.len())?;
    let positions: Vec<[f32; 3]> = pool
        .vertices
        .iter()
        .map(|point| point.map(|value| value as f32))
        .collect();
    let root = rig.roots()[0];
    let binding = RestBinding::prove_with_basis(
        &rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root,
            positions: &positions,
            skin: &skin,
            global_to_skin: identity_f32(),
        }],
        identity_f64(),
    )
    .map_err(rest_error)?;
    let measurement = binding.skins()[0].measurement();

    let gates = gates(&authored, &leaves, &pool.vertices);
    let normals = smooth_normals(&positions, &pool.triangles);
    let asset = MeshAsset {
        name: mesh_name.clone(),
        positions: positions.clone(),
        normals,
        triangles: pool.triangles.clone(),
        triangle_partitions: pool.partitions.clone(),
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let materials = part_materials(&actor, mesh_node)?;
    let bytes =
        write_partitioned_rigged_glb(&asset, &binding, 0, &[], &materials).map_err(glb_error)?;
    if let Some(path) = &params.out {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| errors::io(parent, error))?;
        }
        std::fs::write(path, &bytes).map_err(|error| errors::io(path, error))?;
    }

    let report = build_report(ReportInputs {
        params,
        actor_bytes: &actor_bytes,
        palette: &palette,
        bones: &bones,
        leaves: &leaves,
        duplicates: &duplicates,
        authored: &authored,
        validation: &validation,
        gates: &gates,
        measurement,
        bounds: bounds(&positions),
        bytes: &bytes,
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

/// Refuse an input whose digest is not the one the caller pinned.
///
/// The prefix form exists because the reports quote sixteen hex digits; a shorter
/// expectation would match too much to be evidence, so it is refused outright.
fn verify_input_sha(actual: &str, expected: &str) -> Result<(), CommandError> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() < 8 {
        return Err(refused(
            "skin_input_sha",
            format!("expected input sha {expected:?} is shorter than eight hex digits"),
        ));
    }
    if actual.starts_with(&expected) {
        return Ok(());
    }
    Err(refused(
        "skin_input_sha",
        format!("input sha256 {actual} does not match the expected {expected}"),
    ))
}

/// The one-line verdict the CLI prints.
pub fn verdict(report: &Value) -> String {
    let bones = report["palette"]["deform_bones"]
        .as_u64()
        .unwrap_or_default();
    let vertices = report["authoring"]["vertices"].as_u64().unwrap_or_default();
    let widest = report["validation"]["widest_row"]
        .as_u64()
        .unwrap_or_default();
    let sum = report["validation"]["max_sum_error"]
        .as_f64()
        .unwrap_or_default();
    format!(
        "authored {vertices} vertices over {bones} deform bones; widest row {widest}, \
         row sum error {sum:.3e}"
    )
}

/// Every bone that can carry flesh, and the leaves that cannot.
fn deform_bones(
    rig: &Rig,
    palette: &[String],
    input_mass: &[f64],
) -> Result<PaletteBones, CommandError> {
    let children = children_of(rig, palette.len());
    let mut candidates: Vec<(DeformBone, f64)> = Vec::new();
    let mut leaves = Vec::new();
    for (slot, name) in palette.iter().enumerate() {
        let head = world_origin(rig, JointId::new(slot as u32))?;
        let tails = effective_tails(rig, &children, slot as u32, head, 0)?;
        if tails.is_empty() {
            leaves.push(name.clone());
            continue;
        }
        candidates.push((
            DeformBone {
                name: name.clone(),
                side: side_of(name),
                segments: tails.into_iter().map(|tail| (head, tail)).collect(),
            },
            input_mass[slot],
        ));
    }
    // A weightless bone whose head sits on a joint that does carry weight is a
    // ghost link: the Skyrim palette threads a zero-length `CME` twin in front of
    // every `NPC` bone, and two live twins would split a limb between them. The
    // input skin says which twin the body was authored against, so the tie-break
    // reads its mass — an identity rule, never a weight source. A bone with mass
    // is never dropped (two twist bones share a head and both are real), and a
    // weightless bone with no weighted twin stays (a neck need not carry weight).
    let mut dropped = Vec::new();
    let mut kept: Vec<(DeformBone, f64)> = Vec::new();
    for (bone, mass) in candidates {
        let head = bone
            .segments
            .first()
            .map(|(head, _)| *head)
            .unwrap_or_default();
        let twin = kept.iter().position(|(known, _)| {
            known
                .segments
                .first()
                .is_some_and(|(known_head, _)| known_head.distance(head) <= 1e-3)
        });
        match (mass > 0.0, twin) {
            // A weighted bone never loses its identity; it displaces a weightless
            // twin declared before it.
            (true, Some(index)) if kept[index].1 <= 0.0 => {
                dropped.push(kept[index].0.name.clone());
                kept[index] = (bone, mass);
            }
            (true, Some(_)) | (true, None) => kept.push((bone, mass)),
            // A weightless bone keeps the identity only if nothing coincident was
            // declared before it.
            (false, Some(_)) => dropped.push(bone.name),
            (false, None) => kept.push((bone, mass)),
        }
    }
    Ok(PaletteBones {
        bones: kept.into_iter().map(|(bone, _)| bone).collect(),
        leaves,
        dropped,
    })
}

/// The bones the authoring step may reach, and the palette members it excludes.
struct PaletteBones {
    bones: Vec<DeformBone>,
    /// Joints with no lever of their own: a leaf deforms nothing.
    leaves: Vec<String>,
    /// Coincident twins that lost the identity tie-break.
    dropped: Vec<String>,
}

/// Each palette joint's children, by slot.
fn children_of(rig: &Rig, palette: usize) -> Vec<Vec<u32>> {
    let mut children = vec![Vec::new(); palette];
    for slot in 0..palette {
        let joint = JointId::new(slot as u32);
        if let Some(parent) = rig.joint(joint).and_then(|joint| joint.parent) {
            children[parent.index()].push(slot as u32);
        }
    }
    children
}

/// The joints a bone's lever arm actually reaches.
///
/// A child sitting on its parent's own origin is a pass-through link: the bone's
/// rotation still carries it, but it has no lever of its own, so the arm runs on
/// to that link's own distinct children. A chain of only coincident links is a
/// tip and yields nothing.
fn effective_tails(
    rig: &Rig,
    children: &[Vec<u32>],
    slot: u32,
    head: glam::DVec3,
    depth: usize,
) -> Result<Vec<glam::DVec3>, CommandError> {
    const COINCIDENT_M: f64 = 1e-3;
    const MAX_DEPTH: usize = 8;
    let mut tails = Vec::new();
    for &child in &children[slot as usize] {
        let child_head = world_origin(rig, JointId::new(child))?;
        if child_head.distance(head) > COINCIDENT_M {
            tails.push(child_head);
            continue;
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        tails.extend(effective_tails(rig, children, child, head, depth + 1)?);
    }
    Ok(tails)
}

/// One joint's rest origin in world metres.
fn world_origin(rig: &Rig, joint: JointId) -> Result<glam::DVec3, CommandError> {
    let world = DMat4::from_cols_array_2d(
        &rig.rest_world(joint)
            .map_err(|error| errors::invalid("skin_bones", format!("rest world: {error}")))?,
    );
    Ok(world.to_scale_rotation_translation().2)
}

/// The side a bone's name declares, if it declares one.
///
/// Mixamo spells the side out (`mixamorig:LeftArm`); the Skyrim palette marks it
/// as a standalone token (`NPC L UpperArm [LUar]`, `CME R Clavicle [RClv]`), where
/// a bare `L`/`R` glued to a word (`CME LBody`) is a body part, not a side.
fn side_of(name: &str) -> Option<BodySide> {
    if name.contains("Left") || name.contains(" L ") || name.starts_with("L ") {
        return Some(BodySide::Left);
    }
    if name.contains("Right") || name.contains(" R ") || name.starts_with("R ") {
        return Some(BodySide::Right);
    }
    let lower = name.to_lowercase();
    for (suffix, side) in [
        ("_l", BodySide::Left),
        ("_r", BodySide::Right),
        (".l", BodySide::Left),
        (".r", BodySide::Right),
        (" l", BodySide::Left),
        (" r", BodySide::Right),
    ] {
        if lower.ends_with(suffix) {
            return Some(side);
        }
    }
    None
}

/// The identity basis the actor pipeline works in.
fn identity_f64() -> [[f64; 4]; 4] {
    DMat4::IDENTITY.to_cols_array_2d()
}

/// What the encoded skin must prove about ownership, measured on the output.
struct Gates {
    /// Vertices clear of the midline, split by the side they sit on.
    left_vertices: usize,
    right_vertices: usize,
    /// Vertices carrying an opposite-side influence despite being clear of the midline.
    left_violations: usize,
    right_violations: usize,
    /// Mass on leaf bones, which must be zero.
    leaf_mass: f64,
    /// Positive weights over `0.05` the strict pack dropped.
    discarded_over_005: usize,
}

/// The product gates, measured from the encoded rows and the author's own frame.
fn gates(
    authored: &sculpto_mesh::rig::author::AuthoredSkin,
    leaves: &[String],
    positions: &[[f64; 3]],
) -> Gates {
    let axis = glam::DVec3::from_array(authored.report.lateral_axis);
    let origin = glam::DVec3::from_array(authored.report.midline_origin);
    let dead_zone = AuthorSettings::default().midline_dead_zone;
    let leaf_names: std::collections::BTreeSet<&str> = leaves.iter().map(String::as_str).collect();
    let mut gate = Gates {
        left_vertices: 0,
        right_vertices: 0,
        left_violations: 0,
        right_violations: 0,
        leaf_mass: 0.0,
        discarded_over_005: authored
            .report
            .discarded
            .iter()
            .filter(|(_, weight)| *weight > 0.05)
            .count(),
    };
    for (vertex, row) in authored.rows.iter().enumerate() {
        let lateral = (glam::DVec3::from_array(positions[vertex]) - origin).dot(axis);
        let side = match lateral {
            value if value > dead_zone => Some(BodySide::Left),
            value if value < -dead_zone => Some(BodySide::Right),
            _ => None,
        };
        match side {
            Some(BodySide::Left) => gate.left_vertices += 1,
            Some(BodySide::Right) => gate.right_vertices += 1,
            None => {}
        }
        for (name, weight) in row {
            if leaf_names.contains(name.as_str()) {
                gate.leaf_mass += f64::from(*weight);
            }
            let opposite =
                side_of(name).is_some_and(|bone_side| Some(bone_side) != side && side.is_some());
            if opposite {
                match side {
                    Some(BodySide::Left) => gate.left_violations += 1,
                    Some(BodySide::Right) => gate.right_violations += 1,
                    None => {}
                }
            }
        }
    }
    gate
}

/// Everything the report folds together.
struct ReportInputs<'a> {
    params: &'a SkinParams,
    actor_bytes: &'a [u8],
    palette: &'a [String],
    bones: &'a [DeformBone],
    leaves: &'a [String],
    duplicates: &'a [String],
    authored: &'a sculpto_mesh::rig::author::AuthoredSkin,
    validation: &'a crate::actor_io::Validation,
    gates: &'a Gates,
    measurement: sculpto_core::rig::rest::RestMeasurement,
    bounds: Value,
    bytes: &'a [u8],
}

/// The durable evidence of one authored skin.
fn build_report(input: ReportInputs<'_>) -> Value {
    let settings = AuthorSettings::default();
    let total_mass: f64 = input
        .authored
        .report
        .footprints
        .iter()
        .map(|footprint| footprint.mass)
        .sum();
    let footprints: Vec<Value> = input
        .authored
        .report
        .footprints
        .iter()
        .map(|footprint| {
            json!({
                "bone": footprint.name,
                "section_mm": (footprint.section * 1000.0 * 1000.0).round() / 1000.0,
                "reach_mm": (footprint.reach * 1000.0 * 1000.0).round() / 1000.0,
                "seeds": footprint.seeds,
                "dominated": footprint.dominated,
                "dominance": (footprint.dominance * 10000.0).round() / 10000.0,
                "mass_share": (footprint.mass / total_mass * 10000.0).round() / 10000.0,
            })
        })
        .collect();
    let unweighted: Vec<&str> = input
        .authored
        .report
        .footprints
        .iter()
        .filter(|footprint| footprint.mass <= 0.0)
        .map(|footprint| footprint.name.as_str())
        .collect();
    let discarded: Vec<Value> = input
        .authored
        .report
        .discarded
        .iter()
        .map(|(bone, weight)| json!({"bone": bone, "weight": weight}))
        .collect();
    json!({
        "command": "actor skin",
        "input": {
            "actor": input.params.actor.to_string_lossy(),
            "sha256": sha_hex(&Sha256::digest(input.actor_bytes)),
            "expect_sha256": input.params.expect_input_sha,
        },
        "output": {
            "path": input.params.out.as_ref().map(|path| path.to_string_lossy().to_string()),
            "sha256": sha_hex(&Sha256::digest(input.bytes)),
            "bytes": input.bytes.len(),
        },
        "method": {
            "name": "geodesic bone envelopes",
            "description": "flesh-section capsules per bone, surface-geodesic inverse-distance \
                            falloff in bone sections, a hard reach bound at the armpit crease, \
                            side isolation, relative prune, strict four-slot pack",
            "settings": {
                "exponent": settings.exponent,
                "blend_in_flesh_radii": settings.blend_in_flesh_radii,
                "reach_band": settings.reach_band,
                "ownership_band": settings.ownership_band,
                "falloff_band": settings.falloff_band,
                "prune_ratio": settings.prune_ratio,
                "smoothing": settings.smoothing,
                "midline_dead_zone": settings.midline_dead_zone,
            },
        },
        "palette": {
            "bones": input.palette.len(),
            "deform_bones": input.bones.len(),
            "leaves_excluded": input.leaves,
            "coincident_bones_dropped": input.duplicates,
        },
        "authoring": {
            "vertices": input.authored.report.vertices,
            "components": input.authored.report.components,
            "unreached_pairs": input.authored.report.unreached_pairs,
            "flesh_vertices": input.authored.report.flesh_vertices,
            "inherited_vertices": input.authored.report.inherited_vertices,
            "side_masked_vertices": input.authored.report.side_masked_vertices,
            "side_masked_mass": input.authored.report.side_masked_mass,
            "side_mask_restored": input.authored.report.side_mask_restored,
            "off_nearest_bone": input.authored.report.off_nearest,
            "envelope_cut_vertices": input.authored.report.envelope_cut_vertices,
            "envelope_cut_mass": input.authored.report.envelope_cut_mass,
            "envelope_restored": input.authored.report.envelope_restored,
            "pruned": input.authored.report.pruned,
            "discarded": discarded,
            "lateral_axis": input.authored.report.lateral_axis,
            "midline_origin": input.authored.report.midline_origin,
        },
        "footprints": footprints,
        "unweighted_deform_bones": unweighted,
        "validation": {
            "max_sum_error": input.validation.max_sum_error,
            "empty_rows": input.validation.empty_rows,
            "widest_row": input.validation.widest,
            "per_bone_mass": input.validation.per_bone,
        },
        "gates": {
            "side_isolation": {
                "left_vertices": input.gates.left_vertices,
                "right_vertices": input.gates.right_vertices,
                "left_violations": input.gates.left_violations,
                "right_violations": input.gates.right_violations,
                "dead_zone_m": settings.midline_dead_zone,
            },
            "leaf_weight_mass": input.gates.leaf_mass,
            "discarded_over_0_05": input.gates.discarded_over_005,
        },
        "rest": {
            "max_vertex": input.measurement.max_vertex,
            "max_influence": input.measurement.max_influence,
            "max_joint": input.measurement.max_joint,
        },
        "bounds": input.bounds,
    })
}
