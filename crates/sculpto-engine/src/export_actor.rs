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
//! 4. the engine's weight copy ([`transfer`]) carries authored weights onto the
//!    part from the **nearest declared source shape**: the head, hands and feet
//!    of an assembled body are their own NIF shapes in this collection, not part
//!    of the body shape, so a single body surface cannot weight them and their
//!    authored Calf↔Foot↔Toe0, Hand↔Forearm and Head↔Spine2 blends would be
//!    discarded;
//! 5. a vertex no declared source can reach is bound to the nearest declared
//!    extremity bone instead, and the seam between the copied and the bound
//!    region is smoothed over the mesh graph, so an extremity is not a rigid
//!    island with a torn boundary;
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
use sculpto_mesh::strand::{DropStrands, StrandOptions, drop_unsupported_strands};
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
/// Largest median surface distance, in source units, a declared weight source may
/// sit from the vertices it owns. One unit is 1.4 cm; a source further away than
/// that is not in the part's frame, and its weights would describe another surface.
pub const DEFAULT_SOURCE_REACH_TOLERANCE: f32 = 1.0;
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
    /// Additional authored weight sources: the shapes an assembled body's own
    /// parts came from. A part vertex copies from whichever declared source is
    /// nearest, so the head, hands and feet keep the blend their own shape
    /// authored instead of one rigid bone.
    #[serde(default)]
    pub sources: Vec<ActorSource>,
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
    /// Admitted median distance, in source units, between a declared source and
    /// the vertices it owns. A source further away is refused as misaligned.
    #[serde(default)]
    pub source_reach_tolerance: Option<f32>,
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
    /// Remove thin pieces this layer hangs off the body unsupported — a
    /// drawstring end, a loose strap tail — before the weld and the bind.
    /// Opt-in per layer because the rule deletes authored geometry.
    #[serde(default)]
    pub drop_strands: bool,
}

/// One authored weight source: a shape whose skin the parts may copy.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActorSource {
    /// NIF that carries the shape; absent means the command's own `nif`.
    #[serde(default)]
    pub nif: Option<PathBuf>,
    /// Shape name inside that file.
    pub shape: String,
    /// The transform the assembly applied to this part, in the source frame.
    ///
    /// The assembled head of an authored body is rotated, scaled or seated off
    /// the position its NIF authors, so the source surface has to receive the
    /// same transform before a nearest-surface copy means anything. This mirrors
    /// the `body assemble` manifest's own `scale`/`rotate`/`translate`, applied in
    /// that order; absent means the part was placed exactly as authored.
    #[serde(default)]
    pub placement: Option<SourcePlacement>,
}

/// The assembly transform one part received, in the source frame.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourcePlacement {
    /// Signed per-axis scale about the origin, applied first.
    #[serde(default)]
    pub scale: Option<[f64; 3]>,
    /// Rotation about one source axis through the origin, applied after the scale.
    #[serde(default)]
    pub rotate: Option<SourceRotate>,
    /// Translation in source units, applied last.
    #[serde(default)]
    pub translate: Option<[f64; 3]>,
}

/// One axis rotation of a declared source.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRotate {
    /// Source axis the part turns about.
    pub axis: SourceAxis,
    /// Degrees, right-handed about that axis.
    pub degrees: f64,
}

/// Source axis a declared part rotation turns about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SourceAxis {
    /// The source X axis.
    X,
    /// The source Y axis.
    Y,
    /// The source Z axis.
    Z,
}

impl SourcePlacement {
    /// The part's placement matrix: translation after rotation after scale.
    fn matrix(&self) -> DMat4 {
        let scale = self.scale.unwrap_or([1.0; 3]);
        let mut matrix = DMat4::from_scale(DVec3::from(scale));
        if let Some(rotate) = &self.rotate {
            let (sin, cos) = rotate.degrees.to_radians().sin_cos();
            let rotation = match rotate.axis {
                SourceAxis::X => DMat4::from_cols_array_2d(&[
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, cos, sin, 0.0],
                    [0.0, -sin, cos, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]),
                SourceAxis::Y => DMat4::from_cols_array_2d(&[
                    [cos, 0.0, -sin, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [sin, 0.0, cos, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]),
                SourceAxis::Z => DMat4::from_cols_array_2d(&[
                    [cos, sin, 0.0, 0.0],
                    [-sin, cos, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]),
            };
            matrix = rotation * matrix;
        }
        if let Some(translate) = self.translate {
            matrix = DMat4::from_translation(DVec3::from(translate)) * matrix;
        }
        matrix
    }
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
    /// What the opt-in strand rule removed from this layer, if it ran.
    strands: Option<DropStrands>,
}

/// What one part copied from one weight source.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceBind {
    /// Shape name the vertices copied from.
    pub shape: String,
    /// Vertices whose nearest source is this one.
    pub vertices: usize,
    /// Median surface distance to this source.
    pub reach_median: f32,
    /// Largest surface distance to this source.
    pub reach_max: f32,
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
    /// Per-source attribution, in declaration order.
    sources: Vec<SourceBind>,
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
    strands: Option<DropStrands>,
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

    // Every declared source, placed in the body's own skin frame. The body shape
    // is always the first source: a part on the body surface copies from it.
    let (sources, source_bounds) = read_sources(
        &document,
        &params.nif,
        &params.body_shape,
        shape,
        global_to_skin,
        &params.sources,
    )?;

    // The palette carries every bone any source weights plus the declared
    // extremity bones, then their ancestors: a whole-rig palette is refused by
    // readers above 256 joints and a bone no vertex weights deforms nothing.
    let mut weighted = weighted_bones(&document, shape, &[])?;
    for source in &sources {
        for row in &source.rows {
            for (bone, weight) in row {
                if *weight > 0.0 {
                    weighted.insert(bone.clone());
                }
            }
        }
    }
    for name in &params.extremity_bones {
        weighted.insert(name.clone());
    }
    let (palette, index) = palette_of(&rig, &weighted)?;
    // A bone an extra source also weights takes the derived bind: the body shape's
    // authored inverse would reconstruct the body's rest, not the part's, and the
    // repair gate would charge the difference as a source defect.
    let shared: BTreeSet<String> = sources
        .iter()
        .skip(1)
        .flat_map(|source| {
            source
                .rows
                .iter()
                .flat_map(|row| row.iter().map(|(bone, weight)| (*weight, bone.as_str())))
        })
        .filter(|(weight, _)| *weight > 0.0)
        .map(|(_, bone)| bone.to_owned())
        .collect();
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
        if shared.contains(name.as_str()) {
            continue;
        }
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
    let body_surface = read_obj_surface(&params.body)?;
    let mut parts = vec![read_part(
        &params.body,
        "body",
        0,
        BODY_COLOR,
        &body_surface.0,
        &body_surface.1,
        &placement,
    )?];
    let mut ordered = params.layers.clone();
    ordered.sort_by_key(|layer| layer.order);
    for layer in ordered {
        let rank = u8::try_from(layer.order).expect("the palette gate validated 1..=5");
        let color = review_palette(rank).expect("the palette gate validated 1..=5");
        parts.push(read_layer(&layer, color, &body_surface, &placement)?);
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

    // Skin every part against the declared weight sources, then merge the arena.
    let binder = PartBinder::new(&rig, &palette, &fallback, &sources, iterations);
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
            strands: part.strands.clone(),
        });
        rows.extend(skin.rows.iter().cloned());
        part_skins.push(skin);
    }
    let source_tolerance = params
        .source_reach_tolerance
        .unwrap_or(DEFAULT_SOURCE_REACH_TOLERANCE);
    if !source_tolerance.is_finite() || source_tolerance <= 0.0 {
        return Err(errors::invalid(
            "source_reach_tolerance",
            format!("{source_tolerance} is not a positive finite distance"),
        ));
    }
    for (part, skin) in part_rows.iter().zip(&part_skins) {
        // Only the assembled body has to sit on its own source: a garment layer is
        // authored to float at its ease, and `layer_reach_radius` already bounds it.
        if part.role != "body" {
            continue;
        }
        for source in &skin.sources {
            if source.vertices == 0 || source.reach_median <= source_tolerance {
                continue;
            }
            return Err(refused(
                "export_actor_source_misaligned",
                format!(
                    "source {:?} owns {} vertices of part {:?} at a median distance of {:.4} \
                     source units (admitted {source_tolerance}); declare the part `placement` the \
                     assembly applied, or the copied weights describe a surface the part is not",
                    source.shape, source.vertices, part.name, source.reach_median
                ),
            ));
        }
    }

    // One influence arena over the whole palette.
    let mut joints = Vec::new();
    let mut values = Vec::new();
    let mut offsets = vec![0_u32];
    for row in &rows {
        // Influence weights are stored heaviest first, the order every importer
        // reads a bone row in (Unity `BoneWeight.html:244`); a `BTreeMap` row is
        // otherwise in bone-name order.
        let mut ordered: Vec<(&String, &f32)> = row.iter().collect();
        ordered.sort_by(|left, right| right.1.total_cmp(left.1).then_with(|| left.0.cmp(right.0)));
        for (bone, weight) in ordered {
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
            "sources": sources
                .iter()
                .zip(&source_bounds)
                .map(|(source, (shape, lo, hi))| json!({
                    "shape": shape,
                    "declared": source.shape,
                    "vertices": source.positions.len(),
                    "triangles": source.triangles.len(),
                    "bounds_in_body_frame": {"min": lo, "max": hi},
                }))
                .collect::<Vec<_>>(),
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
        "statement": "one skinned actor: every part vertex copies the nearest declared source shape and \
                      the fallback binds only what no source reaches; a per-part colour is a review \
                      material, not a fit result",
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
        "sources": skin
            .sources
            .iter()
            .map(|source| json!({
                "shape": source.shape,
                "vertices": source.vertices,
                "reach_median": source.reach_median,
                "reach_max": source.reach_max,
            }))
            .collect::<Vec<_>>(),
    });
    if let Some(strands) = &part.strands {
        value["strand_drop"] = json!({
            "source_vertices": strands.source_vertices,
            "source_triangles": strands.source_triangles,
            "source_pieces": strands.source_pieces,
            "kept_pieces": strands.kept_pieces,
            "source_area": strands.source_area,
            "kept_area": strands.kept_area,
            "removed_vertices": strands.removed_vertices(),
            "removed_triangles": strands.removed_triangles(),
            "removed_area": strands.removed_area(),
            "boundary_edges_before": strands.boundary_edges_before,
            "boundary_edges_after": strands.boundary_edges_after,
            "removed": strands.removed.iter().map(|strand| json!({
                "pieces": strand.pieces,
                "triangles": strand.triangles,
                "vertices": strand.vertices,
                "area": strand.area,
                "area_share": strand.area_share,
                "thickness": strand.thickness,
                "extent": strand.extent,
                "unsupported_share": strand.unsupported_share,
                "tip_distance": strand.tip_distance,
                "attachment_span_share": strand.attachment_span_share,
            })).collect::<Vec<_>>(),
            "kept": strands.kept.iter().map(|kept| json!({
                "pieces": kept.pieces,
                "area_share": kept.area_share,
                "thickness": kept.thickness,
                "unsupported_share": kept.unsupported_share,
                "refusal": kept.refusal.as_str(),
            })).collect::<Vec<_>>(),
        });
    }
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

/// Move one already-read OBJ part into the body's skin frame.
fn read_part(
    path: &Path,
    role: &'static str,
    order: u32,
    color: [f32; 4],
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    placement: &ShapePlacement,
) -> Result<SourcePart, CommandError> {
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
        triangles: triangles.to_vec(),
        seam_weld: None,
        strands: None,
    })
}

/// Read one garment layer, merge its authored seam copies and move it into the
/// body's skin frame.
fn read_layer(
    layer: &ActorLayer,
    color: [f32; 4],
    body: &(Vec<[f32; 3]>, Vec<[u32; 3]>),
    placement: &ShapePlacement,
) -> Result<SourcePart, CommandError> {
    let path = &layer.obj;
    let (positions, triangles) = read_obj_surface(path)?;
    if positions.is_empty() || triangles.is_empty() {
        return Err(refused(
            "export_actor_empty_part",
            format!("{} has no geometry to write", path.display()),
        ));
    }
    // The strand rule runs on the authored pieces, before the weld merges them,
    // and against the assembled body in the same frame: only a body surface can
    // say whether a piece hangs unsupported. A layer the rule leaves alone keeps
    // its authored vertex order, so opting in changes no byte when the layer
    // carries no strand.
    let (mut positions, mut triangles) = (positions, triangles);
    let mut strands = None;
    if layer.drop_strands {
        let dropped = drop_unsupported_strands(
            &positions,
            &triangles,
            &body.0,
            &body.1,
            &StrandOptions::default(),
        )
        .map_err(|error| {
            refused(
                "export_actor_strand_rule",
                format!("{}: {error}", path.display()),
            )
        })?;
        if dropped.removed_triangles() > 0 {
            positions.clone_from(&dropped.positions);
            triangles.clone_from(&dropped.triangles);
        }
        strands = Some(dropped);
    }
    let (welded, welded_triangles, seam_weld) = weld_layer(&positions, &triangles);
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "layer".to_string());
    Ok(SourcePart {
        name,
        role: "layer",
        order: layer.order,
        color,
        positions: welded
            .iter()
            .map(|&point| placement.position_in_mesh(point))
            .collect(),
        triangles: welded_triangles,
        seam_weld: Some(seam_weld),
        strands,
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

/// A source's axis-aligned bounds in the body skin frame, for the report.
type SourceBounds = (String, [f32; 3], [f32; 3]);

/// One authored weight source placed in the body's own skin frame.
///
/// The head, hands and feet of this collection are their own NIF shapes, so a
/// part vertex has to copy from the shape it actually is; the body surface alone
/// leaves every extremity vertex beyond reach. @req DECL0002
pub struct WeightSource {
    /// Shape name, as the report names it.
    pub shape: String,
    /// Source vertices in the body skin frame.
    pub positions: Vec<[f32; 3]>,
    /// Source triangles over those vertices.
    pub triangles: Vec<[u32; 3]>,
    /// Authored weights per source vertex, by bone name.
    pub rows: Vec<Influences>,
}

/// Read the body shape and every declared source into the body's skin frame.
///
/// A part vertex copies from the nearest source, so the sources must share one
/// frame: each shape's stored position is taken to the source scene with its own
/// global-to-skin inverse and then into the body skin with the body's matrix.
fn read_sources(
    body_document: &sculpto_nif::NifDocument,
    body_path: &Path,
    body_shape_name: &str,
    body_shape: usize,
    body_global_skin: [[f32; 4]; 4],
    declared: &[ActorSource],
) -> Result<(Vec<WeightSource>, Vec<SourceBounds>), CommandError> {
    let body = body_document.shapes.get(body_shape).ok_or_else(|| {
        errors::invalid(
            "body_shape",
            format!("shape index {body_shape} is outside the file"),
        )
    })?;
    let body_skin = body
        .skin
        .as_ref()
        .expect("the body shape's skin was resolved");
    let bounds_of = |positions: &[[f32; 3]]| {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for point in positions {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(point[axis]);
                hi[axis] = hi[axis].max(point[axis]);
            }
        }
        (lo, hi)
    };
    let (body_lo, body_hi) = bounds_of(&body.positions);
    let mut source_bounds: Vec<SourceBounds> = vec![(body_shape_name.to_owned(), body_lo, body_hi)];
    let mut sources = vec![WeightSource {
        shape: body_shape_name.to_owned(),
        positions: body.positions.clone(),
        triangles: body.triangles.clone(),
        rows: name_rows(body_skin),
    }];
    let to_body = DMat4::from_cols_array_2d(&body_global_skin.map(|column| column.map(f64::from)));
    let mut cache: BTreeMap<PathBuf, sculpto_nif::NifDocument> = BTreeMap::new();
    for entry in declared {
        let path = entry.nif.clone().unwrap_or_else(|| body_path.to_path_buf());
        let document = match cache.remove(&path) {
            Some(document) => document,
            None => {
                let bytes = std::fs::read(&path).map_err(|error| errors::io(&path, error))?;
                sculpto_nif::read(&bytes).map_err(errors::nif)?
            }
        };
        let index = resolve_shape(&document, &entry.shape, "sources")?;
        let shape = document.shapes.get(index).expect("a resolved shape exists");
        let skin = shape.skin.as_ref().ok_or_else(|| {
            refused(
                "export_actor_unskinned_shape",
                format!("source shape {:?} carries no authored skin", shape.name),
            )
        })?;
        if shape.positions.is_empty() || shape.triangles.is_empty() {
            return Err(refused(
                "export_actor_empty_shape",
                format!("source shape {:?} has no geometry to copy from", shape.name),
            ));
        }
        if path == body_path && index == body_shape {
            return Err(errors::invalid(
                "sources",
                format!(
                    "shape {:?} is declared as a source and is already the body shape; its weights \
                     would be counted twice",
                    entry.shape
                ),
            ));
        }
        let record = document.sources.get(index).ok_or_else(|| {
            refused(
                "export_actor_incomplete_rig",
                format!("source shape {index} has no source skin record"),
            )
        })?;
        let source_global = record.global_skin.ok_or_else(|| {
            refused(
                "export_actor_incomplete_rig",
                format!(
                    "source shape {:?} has no global-to-skin matrix; it cannot reach the body frame",
                    shape.name
                ),
            )
        })?;
        // G_body · M · G_source⁻¹ brings a stored source vertex into the body
        // skin frame: M is the placement the assembly gave the part in the source
        // scene, and the two global-to-skin matrices differ whenever the files
        // were exported from different skeleton roots.
        let to_source =
            DMat4::from_cols_array_2d(&source_global.map(|column| column.map(f64::from)));
        let placement = entry
            .placement
            .as_ref()
            .map_or(DMat4::IDENTITY, SourcePlacement::matrix);
        let frame = to_body * placement * to_source.inverse();
        let positions = shape
            .positions
            .iter()
            .map(|&point| {
                frame
                    .transform_point3(DVec3::from(point.map(f64::from)))
                    .to_array()
                    .map(|value| value as f32)
            })
            .collect::<Vec<[f32; 3]>>();
        if let Some(bad) = positions
            .iter()
            .position(|point| point.iter().any(|v| !v.is_finite()))
        {
            return Err(refused(
                "export_actor_source_frame",
                format!(
                    "source shape {:?} vertex {bad} has no finite position in the body frame; its \
                     global-to-skin matrix cannot be inverted into this scene",
                    shape.name
                ),
            ));
        }
        let triangles = shape.triangles.clone();
        let rows = name_rows(skin);
        cache.insert(path, document);
        let (lo, hi) = bounds_of(&positions);
        source_bounds.push((entry.shape.clone(), lo, hi));
        sources.push(WeightSource {
            shape: entry.shape.clone(),
            positions,
            triangles,
            rows,
        });
    }
    Ok((sources, source_bounds))
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
    sources: &'a [WeightSource],
    iterations: usize,
}

impl<'a> PartBinder<'a> {
    /// Bind one part against the declared weight sources.
    #[must_use]
    pub fn new(
        rig: &'a Rig,
        palette: &'a [String],
        fallback: &'a [usize],
        sources: &'a [WeightSource],
        iterations: usize,
    ) -> Self {
        Self {
            rig,
            palette,
            fallback,
            sources,
            iterations,
        }
    }

    /// Copy the authored weights onto one part and bind what no source reaches.
    ///
    /// Every part vertex copies from the declared source nearest to it, so the
    /// assembled head, hands and feet keep the blend their own shape authored.
    ///
    /// # Errors
    /// A part vertex with no comparable triangle in any source, no fallback bone
    /// for an unreached vertex, a copy the engine refuses, and every violated
    /// weight invariant (a zero-sum row, more than four influences, an off-unit
    /// sum).
    pub fn bind(
        &self,
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
        reach_limit: f32,
    ) -> Result<BoundPart, CommandError> {
        if self.sources.is_empty() {
            return Err(refused(
                "export_actor_unreachable",
                "no weight source is declared; a part vertex has nothing to copy from".to_owned(),
            ));
        }
        // One surface-distance field per source; the nearest source owns the
        // vertex, and the smallest of those distances is what the reach tests.
        let mut nearest = vec![0_usize; positions.len()];
        let mut reach: Vec<f32> = vec![f32::INFINITY; positions.len()];
        let mut per_source: Vec<Vec<f32>> = vec![Vec::new(); self.sources.len()];
        let mut measured = vec![false; positions.len()];
        for (index, source) in self.sources.iter().enumerate() {
            let distances = surface_distances(positions, &source.positions, &source.triangles);
            for (vertex, hit) in distances.iter().enumerate() {
                let Some(distance) = hit else {
                    continue;
                };
                measured[vertex] = true;
                if *distance < reach[vertex] {
                    reach[vertex] = *distance;
                    nearest[vertex] = index;
                }
            }
        }
        let mut unreached = Vec::new();
        for (vertex, distance) in reach.iter().enumerate() {
            if !measured[vertex] {
                return Err(refused(
                    "export_actor_unreachable",
                    format!(
                        "part vertex {vertex} is outside reach: no declared source has a \
                         comparable triangle, so no weight could be copied"
                    ),
                ));
            }
            if *distance > reach_limit {
                unreached.push(vertex);
            } else {
                per_source[nearest[vertex]].push(*distance);
            }
        }
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
        let mut current: Vec<Influences> = Vec::with_capacity(positions.len());
        let mut fallback: BTreeMap<usize, usize> = BTreeMap::new();
        for (vertex, position) in positions.iter().enumerate() {
            let point = DVec3::from_array(position.map(f64::from));
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
            let unreached_here = unreached.binary_search(&vertex).is_ok();
            if unreached_here {
                *fallback.entry(slot).or_insert(0) += 1;
            }
            let mut row = Influences::new();
            row.insert(self.palette[slot].clone(), 1.0);
            current.push(row);
        }

        // One copy per source. A vertex every source but its own keeps masked, so
        // the copy leaves that row exactly as it stands; only the source that owns
        // a vertex writes it. Rows a source has not written yet carry their
        // fallback bone as a legal placeholder and are replaced when their source
        // runs, so a masked row is always a complete weight set.
        let mut rows = current.clone();
        for (index, source) in self.sources.iter().enumerate() {
            let values: Vec<f32> = (0..positions.len())
                .map(|vertex| {
                    match unreached.binary_search(&vertex).is_ok() || nearest[vertex] != index {
                        true => 1.0,
                        false => 0.0,
                    }
                })
                .collect();
            let mask = VertexMask::from_values(values).map_err(|error| {
                refused(
                    "export_actor_unreachable",
                    format!("the reach mask is invalid: {error}"),
                )
            })?;
            // A masked row keeps its current weights, so it has to be complete;
            // the vertices this source owns start empty so the copy writes the
            // row outright instead of leaving a seed bone at weight one.
            let mut base: Vec<Influences> = rows.clone();
            for (vertex, row) in base.iter_mut().enumerate() {
                if mask.as_slice()[vertex] == 0.0 {
                    *row = Influences::new();
                }
            }
            let copied = transfer(
                positions,
                &source.positions,
                &source.rows,
                &TransferOptions {
                    radius: reach_limit * 2.0,
                    garment_mask: Some(mask.as_slice().to_vec()),
                    current: Some(base),
                    ..TransferOptions::default()
                },
            )
            .map_err(copy_error)?;
            for (vertex, row) in copied.into_iter().enumerate() {
                if mask.as_slice()[vertex] == 0.0 {
                    rows[vertex] = row;
                }
            }
        }
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
        let sources = self
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| SourceBind {
                shape: source.shape.clone(),
                vertices: per_source[index].len(),
                reach_median: median(&mut per_source[index].clone()),
                reach_max: per_source[index].iter().copied().fold(0.0_f32, f32::max),
            })
            .collect();
        Ok(BoundPart {
            reached: positions.len() - unreached.len(),
            bound: unreached.len(),
            smoothed,
            reach_median: median(&mut per_source.iter().flatten().copied().collect::<Vec<_>>()),
            reach_max: per_source.iter().flatten().copied().fold(0.0_f32, f32::max),
            widest,
            sum_error,
            fallback,
            dominant: dominant(&rows),
            copied_mass,
            sources,
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
