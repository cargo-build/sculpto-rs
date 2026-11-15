//! `export.actor`: one rigged, per-part-coloured actor from an assembled body.
//!
//! `export.glb` writes the shapes a skinned NIF already carries, so the body it
//! writes is whatever the NIF authored — for the standard NeverNude files that
//! is a torso with native underwear and no head, hands or feet. The operator's
//! standard body is the *assembled* one (`body assemble` + `body seal-joints`),
//! which carries every part but no skin at all.
//!
//! This operation binds the assembled parts to the NIF's rig:
//!
//! 1. the NIF body shape supplies the palette authority, the authored weights
//!    and the rest matrices; the family template completes the rig;
//! 2. every declared OBJ part is moved into the body's skin frame with the
//!    shape's own placement;
//! 3. a garment layer's authored seam copies — separate pieces that abut
//!    exactly — are merged by the dress weld ([`weld_layer`]) before any
//!    weight is bound, so skinning cannot slide the panels apart;
//! 4. the engine's weight copy ([`transfer`]) carries the body's authored
//!    weights onto the part wherever the part lies on the body surface;
//! 5. a vertex the copy cannot reach — the head, the hands and the feet of an
//!    assembled body sit 10–19 source units away from the surface the NIF skins
//!    — is bound to the nearest declared extremity bone instead, and the seam
//!    between the copied and the bound region is smoothed over the mesh graph,
//!    so no extremity is a rigid island with a torn boundary;
//! 6. all parts merge into one arena with one palette, one partition per part
//!    and the review palette's colour for each;
//! 7. named clips are mapped by bone name exactly as `export.glb` maps them;
//! 8. the file is written through [`write_partitioned_rigged_glb`]: one skin,
//!    one primitive per part, one material per part.
//!
//! Nothing is glued by hand and no geometry is moved: a part keeps its authored
//! vertices and only gains influences — the one exception is a garment layer's
//! exactly-coincident seam copies, which the weld merges into shared vertices
//! before the bind. The report measures what the copy
//! reached, what the fallback bound, how far the unreached vertices sit and
//! which bone carries them, so "skinned" is a number, not an assertion.
//! @req DECL0008

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use glam::{DMat4, DVec3};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Rig, RigFrame};
use sculpto_gltf::review::{BODY_COLOR, hex, palette as review_palette, smooth_normals};
use sculpto_gltf::{Clip, PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::Actor;
use sculpto_mesh::check::surface_distances;
use sculpto_mesh::dress::{SEAM_WELD_EPSILON, SeamWeldReport, seam_weld};
use sculpto_mesh::garment::{Influences, TransferError, TransferOptions, transfer};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::weight_norm::{MAX_INFLUENCES, SUM_TOLERANCE, pack_influences};
use sculpto_nif::ShapePlacement;

use crate::export_glb::{
    ArenaInfluences, ClipAnchor, ClipPlan, ClipTolerances, DEFAULT_REST_SCALE_TOLERANCE,
    DEFAULT_REST_TRANSLATION_TOLERANCE, DEFAULT_ROLL_TOLERANCE_DEGREES, actor_error, bind_palette,
    bounds, complete_rig, conjugate, conjugate_f64, glb_error, hex as sha_hex, influence_summary,
    palette_of, plan_clip, refused, resolve_shape, rest_error, scale_clip, scale_rig,
    weighted_bones, write_bytes, write_report,
};
use crate::obj::read_obj_surface;
use crate::{CommandError, ErrorKind, errors};

/// Inclusive reach, in source units, inside which a part vertex takes copied
/// body weights. The measured assembled body lies on the NIF surface (largest
/// vertex-to-vertex distance 0.96) while its head, hands and feet sit 10–19
/// units away, so three units (4.2 cm) separates the two populations with a
/// wide margin.
pub const DEFAULT_REACH_RADIUS: f32 = 3.0;
/// Inclusive reach, in source units, for a garment layer: the native weight
/// copy radius. A layer is authored to float, so it is copied at ease distance.
pub const DEFAULT_LAYER_REACH_RADIUS: f32 = sculpto_mesh::garment::DEFAULT_TRANSFER_RADIUS;
/// Graph-smoothing passes over the bound region and its seam.
pub const DEFAULT_SMOOTH_ITERATIONS: usize = 6;
/// Share of its own row a smoothed vertex keeps; the rest is the mean of its
/// neighbours. A half is the largest step that stays monotone across a rigid
/// region: a uniform neighbourhood does not drift.
const SMOOTH_SELF: f32 = 0.5;

/// The command input.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportActorParams {
    /// Source NIF that carries the rig, the palette and the authored body weights.
    pub nif: PathBuf,
    /// Shape name whose weights and rest matrices bind the assembled parts.
    pub body_shape: String,
    /// The assembled body OBJ (every part in one shell).
    pub body: PathBuf,
    /// Ranked garment or underwear OBJs, in the body's frame.
    #[serde(default)]
    pub layers: Vec<ActorLayer>,
    /// Family skeleton template that completes the authored rig.
    #[serde(default)]
    pub skeleton: Option<PathBuf>,
    /// Source actor GLB whose clips are mapped by bone name.
    #[serde(default)]
    pub clips: Option<PathBuf>,
    /// Clip names to write, in written order (index 0 first).
    #[serde(default)]
    pub clip_names: Vec<String>,
    /// Source units per metre; the written file is scaled to metres.
    #[serde(default)]
    pub units_per_metre: Option<f64>,
    /// Admitted rest bone-position residual, relative to the skeleton extent.
    #[serde(default)]
    pub rest_translation_tolerance: Option<f64>,
    /// Admitted spread of the per-bone unit ratios, relative to the median.
    #[serde(default)]
    pub rest_scale_tolerance: Option<f64>,
    /// Admitted excess of a bone-frame difference over its direction change.
    #[serde(default)]
    pub rest_frame_tolerance_degrees: Option<f64>,
    /// Body reach in source units; a body vertex farther from the source body
    /// surface is bound to the nearest fallback bone instead of copied weights.
    #[serde(default)]
    pub reach_radius: Option<f32>,
    /// Layer reach in source units. A garment floats at its authored ease, so it
    /// copies at the native transfer radius rather than the body's coincidence
    /// reach.
    #[serde(default)]
    pub layer_reach_radius: Option<f32>,
    /// Smoothing passes over the bound region and its seam.
    #[serde(default)]
    pub smooth_iterations: Option<usize>,
    /// Bones a vertex beyond the reach may be bound to; empty means every
    /// uniquely named palette bone, nearest rest origin first.
    #[serde(default)]
    pub extremity_bones: Vec<String>,
    /// GLB output path.
    pub out: PathBuf,
    /// Also write the report JSON here.
    #[serde(default)]
    pub report: Option<PathBuf>,
}

/// One ranked part: the palette rank and its OBJ.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorLayer {
    /// Palette rank `1..=5`; the colour comes from the review palette.
    pub order: u32,
    /// Layer OBJ in the body's frame.
    pub obj: PathBuf,
}

/// The command input schema.
pub(crate) fn schema() -> Value {
    json!(schemars::schema_for!(ExportActorParams))
}

/// The registry handler: parse, export, report.
pub(crate) fn export(input: &Value) -> Result<Value, CommandError> {
    let params: ExportActorParams = serde_json::from_value(input.clone()).map_err(|error| {
        CommandError::new(
            ErrorKind::InvalidParams,
            "export_actor_input",
            error.to_string(),
        )
    })?;
    run(&params)
}

/// One part read from disk and moved into the body's skin frame.
struct SourcePart {
    name: String,
    role: &'static str,
    order: u32,
    color: [f32; 4],
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    /// What the seam merge did to an authored garment layer; `None` for the
    /// body, whose joints the seal already owns.
    seam_weld: Option<SeamWeldReport>,
}

/// One part after binding: the rows plus what the copy and the fallback measured.
#[derive(Debug)]
pub struct BoundPart {
    reached: usize,
    bound: usize,
    smoothed: usize,
    reach_median: f32,
    reach_max: f32,
    widest: usize,
    sum_error: f32,
    fallback: Vec<(String, usize)>,
    dominant: Vec<(String, usize)>,
    /// Weighted mass the copy gave this part, per palette slot. The bound mass
    /// comes from the fallback, not from the source, so it is not counted here.
    copied_mass: Vec<f64>,
    rows: Vec<Influences>,
}

impl BoundPart {
    /// The bound rows, one per part vertex.
    pub fn rows(&self) -> &[Influences] {
        &self.rows
    }

    /// Vertices the copy reached.
    pub fn reached(&self) -> usize {
        self.reached
    }

    /// Vertices the fallback bound.
    pub fn bound(&self) -> usize {
        self.bound
    }

    /// Vertices the smoothing pass moved.
    pub fn smoothed(&self) -> usize {
        self.smoothed
    }

    /// The fallback bone that bound each vertex count, heaviest first.
    pub fn fallback(&self) -> &[(String, usize)] {
        &self.fallback
    }
}

/// One merged part: what it came from and its ranges in the merged arena.
struct PartRow {
    name: String,
    role: &'static str,
    order: u32,
    color: [f32; 4],
    vertices: Range<usize>,
    triangles: Range<usize>,
    partition: u16,
    seam_weld: Option<SeamWeldReport>,
}

/// Export one rigged, coloured actor GLB and render its report.
///
/// # Errors
/// Typed refusals for every declared defect class before any output byte, and
/// the measured refusal of a part vertex the copy cannot reach when no
/// extremity bone can carry it.
pub fn run(params: &ExportActorParams) -> Result<Value, CommandError> {
    validate(params)?;

    let nif_bytes = std::fs::read(&params.nif).map_err(|error| errors::io(&params.nif, error))?;
    let document = sculpto_nif::read(&nif_bytes).map_err(errors::nif)?;
    let mut rig = complete_rig(&document, params.skeleton.as_deref())?;
    let shape = resolve_shape(&document, &params.body_shape, "body_shape")?;
    let source = document.shapes.get(shape).ok_or_else(|| {
        errors::invalid(
            "body_shape",
            format!("shape index {shape} is outside the file"),
        )
    })?;
    let authored_skin = source.skin.as_ref().ok_or_else(|| {
        refused(
            "export_actor_unskinned_shape",
            format!(
                "body shape {:?} carries no authored skin; the assembled parts have no weight source",
                source.name
            ),
        )
    })?;
    if source.positions.is_empty() || source.triangles.is_empty() {
        return Err(refused(
            "export_actor_empty_shape",
            format!("body shape {:?} has no geometry to copy from", source.name),
        ));
    }
    let record = document.sources.get(shape).ok_or_else(|| {
        refused(
            "export_actor_incomplete_rig",
            format!("body shape {shape} has no source skin record"),
        )
    })?;
    let mut global_to_skin = record.global_skin.ok_or_else(|| {
        refused(
            "export_actor_incomplete_rig",
            format!(
                "body shape {:?} has no global-to-skin matrix; the parts cannot reach its frame",
                source.name
            ),
        )
    })?;
    let root_name = record.skeleton_root.as_deref().ok_or_else(|| {
        refused(
            "export_actor_incomplete_rig",
            format!(
                "body shape {:?} names no skin skeleton root; the rest proof has nothing to anchor",
                source.name
            ),
        )
    })?;
    let root = rig.joint_by_name(root_name).map_err(|error| {
        refused(
            "export_actor_incomplete_rig",
            format!("body skeleton root {root_name:?}: {error}"),
        )
    })?;
    let placement = document.shape_placement(shape).map_err(|error| {
        refused(
            "export_actor_incomplete_rig",
            format!(
                "body shape {:?} has no usable placement: {error}",
                source.name
            ),
        )
    })?;

    // The palette carries every bone the body skin weights plus the declared
    // extremity bones, then their ancestors: a whole-rig palette is refused by
    // readers above 256 joints and a bone no vertex weights deforms nothing.
    let mut weighted = weighted_bones(&document, shape, &[])?;
    for name in &params.extremity_bones {
        weighted.insert(name.clone());
    }
    let (palette, index) = palette_of(&rig, &weighted)?;
    let body_rows = name_rows(authored_skin);
    let mut authored: Vec<Option<(usize, DMat4)>> = vec![None; palette.len()];
    for (bone, name) in authored_skin.bones.iter().enumerate() {
        let slot = *index.get(name.as_str()).ok_or_else(|| {
            refused(
                "export_actor_palette",
                format!(
                    "body shape {:?} weighs bone {name:?}, which the completed rig does not name \
                     uniquely; declare `skeleton` to complete the family rig",
                    source.name
                ),
            )
        })?;
        let bind = authored_skin.bind_inverse.get(bone).ok_or_else(|| {
            refused(
                "export_actor_palette",
                format!("body shape {:?} has no bind for bone {bone}", source.name),
            )
        })?;
        authored[slot] = Some((
            shape,
            DMat4::from_cols_array_2d(&bind.map(|column| column.map(f64::from))),
        ));
    }

    let (fallback, extremity) = fallback_slots(
        &rig,
        &palette,
        authored_skin,
        &params.extremity_bones,
        &index,
    )?;
    let reach_radius = params.reach_radius.unwrap_or(DEFAULT_REACH_RADIUS);
    let layer_reach = params
        .layer_reach_radius
        .unwrap_or(DEFAULT_LAYER_REACH_RADIUS);
    for (field, value) in [
        ("reach_radius", reach_radius),
        ("layer_reach_radius", layer_reach),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(errors::invalid(
                field,
                format!("{value} is not a positive finite distance"),
            ));
        }
    }
    let iterations = params
        .smooth_iterations
        .unwrap_or(DEFAULT_SMOOTH_ITERATIONS);

    // Read every part in the body's own skin frame, once. A garment layer is
    // authored as separate pieces whose seam copies abut exactly, so the dress
    // weld merges them here, before any weight is bound.
    let mut parts = vec![read_part(&params.body, "body", 0, BODY_COLOR, &placement)?];
    let mut ordered = params.layers.clone();
    ordered.sort_by_key(|layer| layer.order);
    for layer in ordered {
        let rank = u8::try_from(layer.order).expect("the palette gate validated 1..=5");
        let color = review_palette(rank).expect("the palette gate validated 1..=5");
        parts.push(read_layer(&layer.obj, layer.order, color, &placement)?);
    }
    let mut seen = BTreeSet::new();
    for part in &parts {
        if !seen.insert(part.name.clone()) {
            return Err(refused(
                "export_actor_duplicate_part",
                format!(
                    "two parts are named {:?}; the written materials would collide",
                    part.name
                ),
            ));
        }
    }

    // Skin every part against the one body skin, then merge the arena.
    let binder = PartBinder::new(
        &rig,
        &palette,
        &fallback,
        BodySurface {
            positions: &source.positions,
            triangles: &source.triangles,
            rows: &body_rows,
        },
        iterations,
    );
    let mut arena = MeshAsset {
        name: params.body_shape.clone(),
        ..Default::default()
    };
    let mut rows: Vec<Influences> = Vec::new();
    let mut copied_mass = vec![0.0_f64; palette.len()];
    let mut part_skins = Vec::with_capacity(parts.len());
    let mut part_rows = Vec::with_capacity(parts.len());
    for (position, part) in parts.iter().enumerate() {
        let reach = match part.role {
            "body" => reach_radius,
            _ => layer_reach,
        };
        let skin = binder.bind(&part.positions, &part.triangles, reach)?;
        let base = arena.positions.len();
        arena.positions.extend_from_slice(&part.positions);
        arena
            .normals
            .extend(smooth_normals(&part.positions, &part.triangles));
        let first_triangle = arena.triangles.len();
        for triangle in &part.triangles {
            let mut shifted = [0_u32; 3];
            for (lane, vertex) in triangle.iter().enumerate() {
                shifted[lane] = vertex
                    .checked_add(u32::try_from(base).map_err(|_| {
                        refused("export_actor_too_large", "merged vertex count exceeds u32")
                    })?)
                    .ok_or_else(|| {
                        refused(
                            "export_actor_too_large",
                            "merged triangle index exceeds u32",
                        )
                    })?;
            }
            arena.triangles.push(shifted);
        }
        let partition = u16::try_from(position).map_err(|_| {
            refused(
                "export_actor_too_many_parts",
                format!("{} parts exceed the u16 partition id", position + 1),
            )
        })?;
        arena.triangle_partitions.extend(std::iter::repeat_n(
            PartitionId(partition),
            part.triangles.len(),
        ));
        for (slot, mass) in skin.copied_mass.iter().enumerate() {
            copied_mass[slot] += mass;
        }
        part_rows.push(PartRow {
            name: part.name.clone(),
            role: part.role,
            order: part.order,
            color: part.color,
            vertices: base..arena.positions.len(),
            triangles: first_triangle..arena.triangles.len(),
            partition,
            seam_weld: part.seam_weld.clone(),
        });
        rows.extend(skin.rows.iter().cloned());
        part_skins.push(skin);
    }

    // One influence arena over the whole palette.
    let mut joints = Vec::new();
    let mut values = Vec::new();
    let mut offsets = vec![0_u32];
    for row in &rows {
        for (bone, weight) in row {
            let slot = *index.get(bone.as_str()).ok_or_else(|| {
                refused(
                    "export_actor_palette",
                    format!("a copied weight names bone {bone:?}, which the rig does not name"),
                )
            })?;
            joints.push(u16::try_from(slot).map_err(|_| {
                refused(
                    "export_actor_too_large",
                    "palette entry exceeds the u16 lane",
                )
            })?);
            values.push(*weight);
        }
        offsets.push(
            u32::try_from(joints.len())
                .map_err(|_| refused("export_actor_too_large", "influence entries exceed u32"))?,
        );
    }

    let tolerances = ClipTolerances {
        translation_relative: params
            .rest_translation_tolerance
            .unwrap_or(DEFAULT_REST_TRANSLATION_TOLERANCE),
        scale_relative: params
            .rest_scale_tolerance
            .unwrap_or(DEFAULT_REST_SCALE_TOLERANCE),
        roll_degrees: params
            .rest_frame_tolerance_degrees
            .unwrap_or(DEFAULT_ROLL_TOLERANCE_DEGREES),
    };
    let mut clips = plan_clips(params, &rig, &palette, &tolerances)?;

    // The written file is in the declared unit, so the rig the writer proves and
    // drives is the same scaled rig the geometry is converted to.
    let scale = match params.units_per_metre {
        Some(units) if units.is_finite() && units > 0.0 => 1.0 / units,
        Some(units) => {
            return Err(errors::invalid(
                "units_per_metre",
                format!("{units} is not a positive finite unit count"),
            ));
        }
        None => 1.0,
    };
    if (scale - 1.0).abs() > f64::EPSILON {
        let similarity = DMat4::from_scale(DVec3::splat(scale));
        let inverse = similarity.inverse();
        for position in &mut arena.positions {
            for axis in position {
                *axis = (*axis as f64 * scale) as f32;
            }
        }
        for (_, bind) in authored.iter_mut().flatten() {
            *bind = DMat4::from_cols_array_2d(&conjugate_f64(
                bind.to_cols_array_2d(),
                similarity,
                inverse,
            ));
        }
        global_to_skin = conjugate(global_to_skin, similarity, inverse);
        rig = scale_rig(&rig, scale)?;
        for plan in &mut clips {
            scale_clip(&mut plan.clip, scale);
        }
    }

    let bound = bind_palette(
        &rig,
        ArenaInfluences {
            palette: &palette,
            authored: &authored,
            positions: &arena.positions,
            joints,
            values,
            offsets,
            source_mass: Some(&copied_mass),
        },
        global_to_skin,
        root,
        shape,
    )?;
    let skin = bound.skin;
    arena.skin = Some(skin.clone());
    let proof = RestBinding::prove_with_basis(
        &rig,
        RigFrame::SourceZUp,
        &[SkinRestSource {
            shape,
            root,
            positions: &arena.positions,
            skin: &skin,
            global_to_skin,
        }],
        bound.basis,
    )
    .map_err(rest_error)?;
    let measurement = proof
        .skins()
        .first()
        .expect("the actor proof carries its one skin")
        .measurement();
    let materials: Vec<PartMaterial> = part_rows
        .iter()
        .map(|part| PartMaterial {
            partition: part.partition,
            name: part.name.clone(),
            color: part.color,
        })
        .collect();
    let written: Vec<Clip> = clips.iter().map(|plan| plan.clip.clone()).collect();
    let bytes = write_partitioned_rigged_glb(&arena, &proof, shape, &written, &materials)
        .map_err(glb_error)?;
    write_bytes(&params.out, &bytes)?;

    let part_values: Vec<Value> = part_rows
        .iter()
        .zip(&part_skins)
        .map(|(part, skin)| part_value(part, skin))
        .collect();
    let clip_rows: Vec<Value> = clips
        .iter()
        .enumerate()
        .map(|(index, plan)| clip_value(index, plan))
        .collect();
    let report = json!({
        "out": params.out.display().to_string(),
        "bytes": bytes.len(),
        "sha256": sha_hex(&Sha256::digest(&bytes)),
        "source": {
            "nif": params.nif.display().to_string(),
            "nif_sha256": sha_hex(&Sha256::digest(&nif_bytes)),
            "skeleton": params.skeleton.as_ref().map(|path| path.display().to_string()),
            "shapes": document.shapes.len(),
            "rig_joints": rig.joints().len(),
        },
        "body_shape": params.body_shape,
        "body_source": {
            "vertices": source.positions.len(),
            "triangles": source.triangles.len(),
            "bones": authored_skin.bones.len(),
        },
        "parts": part_values,
        "palette": {
            "bones": palette.len(),
            "authored": authored.iter().filter(|entry| entry.is_some()).count(),
            "bind_residual": bound.bind_residual,
            "source_bind_mismatch": bound.source_bind_mismatch,
            "repaired_mass": bound.repaired_mass,
            "bind_repairs": bound
                .repairs
                .iter()
                .map(|repair| json!({
                    "bone": repair.bone,
                    "shape": repair.shape,
                    "before": repair.before,
                    "weight_mass": repair.weight_mass,
                }))
                .collect::<Vec<_>>(),
            "influences": influence_summary(arena.skin.as_ref()),
            "fallback_bones": fallback
                .iter()
                .map(|slot| palette[*slot].clone())
                .collect::<Vec<_>>(),
            "declared_extremity_bones": extremity
                .iter()
                .map(|slot| palette[*slot].clone())
                .collect::<Vec<_>>(),
        },
        "rest": {
            "basis": bound.basis,
            "max_vertex": measurement.max_vertex,
            "max_influence": measurement.max_influence,
            "max_joint": measurement.max_joint,
        },
        "skin": {
            "reach_radius": reach_radius,
            "layer_reach_radius": layer_reach,
            "smooth_iterations": iterations,
            "skins": 1,
            "parts": part_rows.len(),
        },
        "units": {
            "units_per_metre": params.units_per_metre,
            "scale": scale,
            "metres": params.units_per_metre.is_some(),
        },
        "clips": clip_rows,
        "attributes": {
            "normals": !arena.normals.is_empty(),
            "uvs": !arena.uvs.is_empty(),
            "tangents": !arena.tangents.is_empty(),
        },
        "bounds": bounds(&arena.positions),
        "statement": "one skinned actor: the copy reaches the body surface and the fallback binds the \
                      extremities it cannot reach; a per-part colour is a review material, not a fit result",
    });
    if let Some(path) = &params.report {
        write_report(path, &report)?;
    }
    Ok(report)
}

fn part_value(part: &PartRow, skin: &BoundPart) -> Value {
    let pairs = |rows: &[(String, usize)]| {
        rows.iter()
            .map(|(bone, vertices)| json!({"bone": bone, "vertices": vertices}))
            .collect::<Vec<_>>()
    };
    let mut value = json!({
        "name": part.name,
        "role": part.role,
        "order": part.order,
        "partition": part.partition,
        "material": hex(part.color),
        "vertices": part.vertices.len(),
        "triangles": part.triangles.len(),
        "vertex_start": part.vertices.start,
        "vertex_end": part.vertices.end,
        "triangle_start": part.triangles.start,
        "triangle_end": part.triangles.end,
        "reached": skin.reached,
        "bound": skin.bound,
        "smoothed": skin.smoothed,
        "reach_median": skin.reach_median,
        "reach_max": skin.reach_max,
        "influences_widest": skin.widest,
        "influences_sum_error": skin.sum_error,
        "fallback_bones": pairs(&skin.fallback),
        "dominant_bones": pairs(&skin.dominant),
    });
    if let Some(seam) = &part.seam_weld {
        value["seam_weld"] = json!({
            "tolerance": seam.tolerance,
            "source_vertices": seam.welded_vertices + seam.merged_vertices,
            "welded_vertices": seam.welded_vertices,
            "merged_vertices": seam.merged_vertices,
            "merged_pairs": seam.merged_pairs,
            "components_before": seam.components_before,
            "components_after": seam.components_after,
            "boundary_edges_before": seam.boundary_edges_before,
            "boundary_edges_after": seam.boundary_edges_after,
            "dropped_faces": seam.dropped_faces,
        });
    }
    value
}

fn clip_value(index: usize, plan: &ClipPlan) -> Value {
    json!({
        "index": index,
        "name": plan.clip.name,
        "duration": plan.duration,
        "samples": plan.clip.times.len(),
        "channels": plan.clip.channels.len(),
        "mapped_bones": plan.mapped_bones,
        "root_bones": plan.root_bones,
        "moving_channels": plan.moving_channels,
        "unit_scale": plan.scale,
        "max_motion_degrees": plan.max_motion_degrees,
        "max_motion_translation": plan.max_motion_translation,
        "max_rest_rotation_degrees": plan.max_rest_rotation_degrees,
        "max_rest_translation_residual": plan.max_rest_translation_residual,
        "rest_pose_comparable": plan.rest_pose_comparable,
        "rest_pose_outlier": plan.rest_pose_outlier,
        "max_frame_delta_degrees": plan.max_frame_delta_degrees,
        "max_swing_degrees": plan.max_swing_degrees,
        "frame_outlier": plan.frame_outlier,
        "roll_normalization_degrees": plan.roll_normalization_degrees,
    })
}

/// Refuse the declarations that cannot produce one well-defined actor.
fn validate(params: &ExportActorParams) -> Result<(), CommandError> {
    if params.clip_names.is_empty() && params.clips.is_some() {
        return Err(errors::invalid(
            "clip_names",
            "`clips` names a source GLB but no `clip_names` were declared",
        ));
    }
    if !params.clip_names.is_empty() && params.clips.is_none() {
        return Err(errors::invalid(
            "clips",
            format!(
                "{} clip name(s) declared without a source GLB",
                params.clip_names.len()
            ),
        ));
    }
    let mut seen = BTreeMap::new();
    for (position, name) in params.clip_names.iter().enumerate() {
        if let Some(first) = seen.insert(name.as_str(), position) {
            return Err(errors::invalid(
                "clip_names",
                format!("clip `{name}` is declared twice (positions {first} and {position})"),
            ));
        }
    }
    let mut orders = Vec::new();
    for layer in &params.layers {
        if !(1..=5).contains(&layer.order) {
            return Err(errors::invalid(
                "layers",
                format!("layer order {} is outside the palette 1..=5", layer.order),
            ));
        }
        if orders.contains(&layer.order) {
            return Err(errors::invalid(
                "layers",
                format!("two layers declare palette rank {}", layer.order),
            ));
        }
        orders.push(layer.order);
    }
    Ok(())
}

/// The palette slots a vertex beyond the reach may be bound to.
///
/// The deformation vocabulary of the source skin comes first: any point the copy
/// cannot reach but that still lies beside the body — a loose garment's hem
/// inside the leg gap, say — belongs to the bone that already deforms that part
/// of the body. The declared extremity bones are added for the parts the source
/// skin has no influence over at all (the head, hands and feet of an assembled
/// body). With nothing declared and nothing authored, every palette bone is a
/// candidate. Returns `(candidates, declared)`.
#[allow(clippy::type_complexity)]
fn fallback_slots(
    rig: &Rig,
    palette: &[String],
    body_skin: &Skin,
    declared: &[String],
    index: &BTreeMap<String, usize>,
) -> Result<(Vec<usize>, Vec<usize>), CommandError> {
    let mut candidates: BTreeSet<usize> = body_skin
        .bones
        .iter()
        .filter_map(|name| index.get(name.as_str()).copied())
        .collect();
    let mut extremity = Vec::with_capacity(declared.len());
    for name in declared {
        let slot = palette
            .iter()
            .position(|bone| bone == name)
            .ok_or_else(|| {
                refused(
                    "export_actor_extremity_absent",
                    format!(
                        "declared extremity bone {name:?} is not a palette bone; the completed rig \
                     names {} joints and this one is not among the uniquely named ones",
                        rig.joints().len()
                    ),
                )
            })?;
        extremity.push(slot);
        candidates.insert(slot);
    }
    if candidates.is_empty() {
        return Ok(((0..palette.len()).collect(), extremity));
    }
    Ok((candidates.into_iter().collect(), extremity))
}

/// Read one OBJ part and move it into the body's skin frame.
fn read_part(
    path: &Path,
    role: &'static str,
    order: u32,
    color: [f32; 4],
    placement: &ShapePlacement,
) -> Result<SourcePart, CommandError> {
    let (positions, triangles) = read_obj_surface(path)?;
    if positions.is_empty() || triangles.is_empty() {
        return Err(refused(
            "export_actor_empty_part",
            format!("{} has no geometry to write", path.display()),
        ));
    }
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| role.to_string());
    Ok(SourcePart {
        name,
        role,
        order,
        color,
        positions: positions
            .iter()
            .map(|&point| placement.position_in_mesh(point))
            .collect(),
        triangles,
        seam_weld: None,
    })
}

/// Read one garment layer, merge its authored seam copies and move it into the
/// body's skin frame.
fn read_layer(
    path: &Path,
    order: u32,
    color: [f32; 4],
    placement: &ShapePlacement,
) -> Result<SourcePart, CommandError> {
    let (positions, triangles) = read_obj_surface(path)?;
    if positions.is_empty() || triangles.is_empty() {
        return Err(refused(
            "export_actor_empty_part",
            format!("{} has no geometry to write", path.display()),
        ));
    }
    let (welded, welded_triangles, seam_weld) = weld_layer(&positions, &triangles);
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "layer".to_string());
    Ok(SourcePart {
        name,
        role: "layer",
        order,
        color,
        positions: welded
            .iter()
            .map(|&point| placement.position_in_mesh(point))
            .collect(),
        triangles: welded_triangles,
        seam_weld: Some(seam_weld),
    })
}

/// Merge an authored garment's seam copies with the dress weld.
///
/// The authored underwear is printed as separate pieces — the HIMBO boxers are
/// eleven, the CBBE panty five — whose boundary vertices abut exactly. Bound
/// unwelded, skinning slides the panels apart: the waistband ring floats off
/// the shorts and the free gusset ends up between the legs. [`seam_weld`]
/// already owns the rule (merge only coincident groups spanning two pieces,
/// keep an authored dart, drop a collapsed face), so this is the same weld the
/// dress path applies, run on the garment before any weight is bound.
/// @req DECL0013
pub fn weld_layer(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>, SeamWeldReport) {
    seam_weld(positions, triangles, SEAM_WELD_EPSILON)
}

/// The body surface and its authored rows, shared by every part.
pub struct BodySurface<'a> {
    /// Body vertices in the skin frame the copy runs in.
    pub positions: &'a [[f32; 3]],
    /// Body triangles over those vertices.
    pub triangles: &'a [[u32; 3]],
    /// Authored weights per body vertex, by bone name.
    pub rows: &'a [Influences],
}

/// One part's binding context: the rig, the palette, the fallback bones and the
/// surface to copy from.
///
/// The rule is one sentence: a part vertex on the body surface takes the copied
/// weights, a vertex the copy cannot reach takes the nearest fallback bone, and
/// the seam between the two is smoothed. @req DECL0002
pub struct PartBinder<'a> {
    rig: &'a Rig,
    palette: &'a [String],
    fallback: &'a [usize],
    body: BodySurface<'a>,
    iterations: usize,
}

impl<'a> PartBinder<'a> {
    /// Bind one part against one body surface.
    #[must_use]
    pub fn new(
        rig: &'a Rig,
        palette: &'a [String],
        fallback: &'a [usize],
        body: BodySurface<'a>,
        iterations: usize,
    ) -> Self {
        Self {
            rig,
            palette,
            fallback,
            body,
            iterations,
        }
    }

    /// Copy the body skin onto one part and bind what the copy cannot reach.
    ///
    /// # Errors
    /// A part vertex with no comparable body triangle, no fallback bone for an
    /// unreached vertex, a copy the engine refuses, and every violated weight
    /// invariant (a zero-sum row, more than four influences, an off-unit sum).
    pub fn bind(
        &self,
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
        reach_limit: f32,
    ) -> Result<BoundPart, CommandError> {
        let distances = surface_distances(positions, self.body.positions, self.body.triangles);
        let mut values = Vec::with_capacity(positions.len());
        let mut reach: Vec<f32> = Vec::with_capacity(positions.len());
        let mut unreached = Vec::new();
        for (vertex, hit) in distances.iter().enumerate() {
            let Some(distance) = hit else {
                return Err(refused(
                    "export_actor_unreachable",
                    format!(
                        "part vertex {vertex} is outside reach: the body surface has no comparable \
                         triangle, so no weight could be copied and no distance measured"
                    ),
                ));
            };
            reach.push(*distance);
            let beyond = *distance > reach_limit;
            values.push(match beyond {
                true => 1.0,
                false => 0.0,
            });
            if beyond {
                unreached.push(vertex);
            }
        }
        let mask = VertexMask::from_values(values).map_err(|error| {
            refused(
                "export_actor_unreachable",
                format!("the reach mask is invalid: {error}"),
            )
        })?;
        if !unreached.is_empty() && self.fallback.is_empty() {
            return Err(refused(
                "export_actor_unreachable",
                format!(
                    "{} of {} part vertices are beyond the {} reach and no extremity bone is \
                     available to bind them",
                    unreached.len(),
                    positions.len(),
                    reach_limit
                ),
            ));
        }

        // The fallback row: one rigid influence from the nearest declared bone.
        // A palette slot is not a rig index (the palette keeps only the bones a
        // vertex weights and their ancestors), so the origin is resolved through
        // the name the slot carries.
        let origins: Vec<DVec3> = self
            .fallback
            .iter()
            .map(|slot| {
                let joint = self
                    .rig
                    .joint_by_name(&self.palette[*slot])
                    .expect("a palette bone is a rig joint");
                let world = self
                    .rig
                    .rest_world(joint)
                    .expect("a palette joint has a rest world");
                DVec3::new(world[3][0], world[3][1], world[3][2])
            })
            .collect();
        let mut current: Vec<Influences> = vec![Influences::new(); positions.len()];
        let mut fallback: BTreeMap<usize, usize> = BTreeMap::new();
        for &vertex in &unreached {
            let point = DVec3::from_array(positions[vertex].map(f64::from));
            let nearest = origins
                .iter()
                .enumerate()
                .min_by(|left, right| {
                    left.1
                        .distance(point)
                        .total_cmp(&right.1.distance(point))
                        .then_with(|| left.0.cmp(&right.0))
                })
                .map(|(order, _)| order)
                .expect("a declared fallback bone has an origin");
            let slot = self.fallback[nearest];
            *fallback.entry(slot).or_insert(0) += 1;
            current[vertex].insert(self.palette[slot].clone(), 1.0);
        }

        // The copy radius stays above the reach so a vertex the surface test
        // admits always has a cached body vertex to copy from: the cache measures
        // vertex-to-vertex distance, the mask measures surface distance.
        let mut rows = transfer(
            positions,
            self.body.positions,
            self.body.rows,
            &TransferOptions {
                radius: reach_limit * 2.0,
                garment_mask: Some(mask.as_slice().to_vec()),
                current: Some(current),
                ..TransferOptions::default()
            },
        )
        .map_err(copy_error)?;
        let smoothed = smooth(triangles, &mut rows, &unreached, self.iterations);
        let (widest, sum_error) = contract(&rows, positions.len())?;
        let bound: BTreeSet<usize> = unreached.iter().copied().collect();
        let mut copied_mass = vec![0.0_f64; self.palette.len()];
        for (vertex, row) in rows.iter().enumerate() {
            if bound.contains(&vertex) {
                continue;
            }
            for (bone, weight) in row {
                let slot = self
                    .palette
                    .iter()
                    .position(|name| name == bone)
                    .expect("a copied weight names a palette bone");
                copied_mass[slot] += f64::from(*weight);
            }
        }
        let mut fallback: Vec<(String, usize)> = fallback
            .into_iter()
            .map(|(slot, vertices)| (self.palette[slot].clone(), vertices))
            .collect();
        fallback.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        Ok(BoundPart {
            reached: positions.len() - unreached.len(),
            bound: unreached.len(),
            smoothed,
            reach_median: median(&mut reach.clone()),
            reach_max: reach.iter().copied().fold(0.0_f32, f32::max),
            widest,
            sum_error,
            fallback,
            dominant: dominant(&rows),
            copied_mass,
            rows,
        })
    }
}

/// Refuse an empty, over-wide or off-unit row and report the bounds.
fn contract(rows: &[Influences], vertices: usize) -> Result<(usize, f32), CommandError> {
    if rows.len() != vertices {
        return Err(refused(
            "export_actor_skin_contract",
            format!("{} weight rows for {vertices} vertices", rows.len()),
        ));
    }
    let mut widest = 0;
    let mut sum_error = 0.0_f32;
    for (vertex, row) in rows.iter().enumerate() {
        let positive = row.values().filter(|weight| **weight > 0.0).count();
        let sum: f32 = row.values().sum();
        let error = (sum - 1.0).abs();
        if positive == 0 || positive > MAX_INFLUENCES || error > SUM_TOLERANCE {
            return Err(refused(
                "export_actor_skin_contract",
                format!(
                    "part vertex {vertex} carries {positive} influences (at most {MAX_INFLUENCES}) \
                     summing to {sum}; the sum error {error} is above {SUM_TOLERANCE}"
                ),
            ));
        }
        widest = widest.max(positive);
        sum_error = sum_error.max(error);
    }
    Ok((widest, sum_error))
}

/// How many vertices each bone dominates, heaviest first.
fn dominant(rows: &[Influences]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows {
        if let Some((bone, _)) = row
            .iter()
            .max_by(|left, right| left.1.total_cmp(right.1).then_with(|| right.0.cmp(left.0)))
        {
            *counts.entry(bone.as_str()).or_insert(0) += 1;
        }
    }
    let mut rows: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(bone, vertices)| (bone.to_string(), vertices))
        .collect();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.truncate(8);
    rows
}

/// Average the bound region into its seam over the mesh graph.
///
/// Only the bound vertices move: the copied weights keep the exact values the
/// copy gave them, and a rigid region stays rigid (a uniform neighbourhood does
/// not drift), so the pass softens the seam, never the copy. @req DECL0002
fn smooth(
    triangles: &[[u32; 3]],
    rows: &mut [Influences],
    bound: &[usize],
    iterations: usize,
) -> usize {
    if iterations == 0 || bound.is_empty() {
        return 0;
    }
    let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();
    for face in triangles {
        let corners = [face[0] as usize, face[1] as usize, face[2] as usize];
        for (lane, &left) in corners.iter().enumerate() {
            for &right in corners.iter().skip(lane + 1) {
                let key = (left.min(right), left.max(right));
                if seen.insert(key) {
                    adjacent[left].push(right);
                    adjacent[right].push(left);
                }
            }
        }
    }
    let locked: BTreeSet<String> = BTreeSet::new();
    let mut blurred = 0;
    for _ in 0..iterations {
        let mut next = rows.to_vec();
        for &vertex in bound {
            if adjacent[vertex].is_empty() {
                continue;
            }
            let mut blended: Influences = rows[vertex]
                .iter()
                .map(|(bone, weight)| (bone.clone(), weight * SMOOTH_SELF))
                .collect();
            let share = (1.0 - SMOOTH_SELF) / adjacent[vertex].len() as f32;
            for &neighbour in &adjacent[vertex] {
                for (bone, weight) in &rows[neighbour] {
                    *blended.entry(bone.clone()).or_insert(0.0) += weight * share;
                }
            }
            if let Ok(packed) = pack_influences(&blended, &locked) {
                next[vertex] = packed;
                blurred += 1;
            }
        }
        for (slot, blurred_row) in rows.iter_mut().zip(next) {
            *slot = blurred_row;
        }
    }
    bound.len().min(blurred)
}

/// The body's authored rows by bone name.
fn name_rows(skin: &Skin) -> Vec<Influences> {
    (0..skin.influences.rows())
        .map(|vertex| {
            let row = skin
                .influences
                .row(vertex)
                .expect("a body vertex carries a weight row");
            row.joints
                .iter()
                .zip(row.weights)
                .filter(|(_, weight)| **weight > 0.0)
                .map(|(joint, weight)| (skin.bones[usize::from(*joint)].clone(), *weight))
                .collect()
        })
        .collect()
}

/// Build the written clips exactly as `export.glb` builds them.
fn plan_clips(
    params: &ExportActorParams,
    rig: &Rig,
    palette: &[String],
    tolerances: &ClipTolerances,
) -> Result<Vec<ClipPlan>, CommandError> {
    let Some(path) = &params.clips else {
        return Ok(Vec::new());
    };
    let bytes = std::fs::read(path).map_err(|error| errors::io(path, error))?;
    let actor = Actor::from_glb(&bytes).map_err(actor_error)?;
    let first = actor.clip(&params.clip_names[0]).map_err(actor_error)?;
    let anchor = ClipAnchor::of(&actor, &first)?;
    let mut plans = Vec::with_capacity(params.clip_names.len());
    for (index, name) in params.clip_names.iter().enumerate() {
        let source = match index {
            0 => first.clone(),
            _ => actor.clip(name).map_err(actor_error)?,
        };
        plans.push(plan_clip(
            &source, &actor, &anchor, rig, palette, tolerances,
        )?);
    }
    Ok(plans)
}

fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    let middle = values.len() / 2;
    match values.len() % 2 {
        0 => (values[middle - 1] + values[middle]) / 2.0,
        _ => values[middle],
    }
}

fn copy_error(error: TransferError) -> CommandError {
    refused("export_actor_copy", error.to_string())
}
