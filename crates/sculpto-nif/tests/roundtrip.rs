//! C14 round-trip suite: write a synthetic model, read it back, compare.
//!
//! Every fixture is procedural — no committed binary payload and no
//! private-host path, so no test may early-return (unlike
//! `skin_roundtrip.rs`, whose real-body tier skips without
//! `SCULPTO_BIJIN_NIF`). Each test drives both targets through
//! write → `sculpto_nif::read` → compare with the per-target tolerances the
//! formats impose: SE packs UVs/weights as `f16` and normals as bytes; LE
//! stores `f32` throughout.
//!
//! Coverage gaps, named: `.tri` morphs stay out because the crate has no
//! `.tri` writer yet (`write` carries only `se_shape`/`le_shape`), and
//! partition ids go through parsed `NiSkinPartition` blocks because the
//! reader's `NifShape` carries no partition metadata
//! (`src/mesh.rs:25-27`). Tangents follow the task's compare list: SE
//! in-module tests own them (`src/write/se_shape.rs`), LE refuses them.
//! Normal absence is not round-trippable: the importer synthesizes
//! geometry normals when a file carries none, so the suite compares
//! normals only for models that author them.

use std::collections::BTreeSet;

use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet};
use byroredux_nif::blocks::skin::NiSkinPartition;
use byroredux_nif::parse_nif;
use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, ShaderKind, Skin};
use sculpto_nif::write::header::{NifTarget, VERSION};
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{NifDocument, read};

/// Per-attribute drift allowances, tighter on LE because its layout is `f32`.
#[derive(Clone, Copy)]
struct Tolerance {
    uv: f32,
    normal: f32,
    weight: f32,
}

impl Tolerance {
    fn for_target(target: NifTarget) -> Self {
        match target {
            NifTarget::Se => Self {
                uv: 1e-3,
                normal: 0.02,
                weight: 1e-3,
            },
            NifTarget::Le => Self {
                uv: 1e-6,
                normal: 1e-6,
                weight: 1e-4,
            },
        }
    }
}

// ── Fixtures ─────────────────────────────────────────────────────────

fn triangle() -> MeshAsset {
    MeshAsset {
        name: "Triangle".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    }
}

/// Flat-shaded cube: 24 vertices, six per-face unit normals.
fn cube() -> MeshAsset {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut triangles = Vec::new();
    for face in 0..6_u32 {
        let axis = (face / 2) as usize;
        let sign = if face.is_multiple_of(2) { 1.0 } else { -1.0 };
        let mut normal = [0.0f32; 3];
        normal[axis] = sign;
        let mut u = [0.0f32; 3];
        u[(axis + 1) % 3] = 1.0;
        let mut v = [0.0f32; 3];
        v[(axis + 2) % 3] = 1.0;
        let base = positions.len() as u32;
        for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let mut position = [0.0f32; 3];
            position[axis] = sign;
            for k in 0..3 {
                position[k] += su * u[k] + sv * v[k];
            }
            positions.push(position);
            normals.push(normal);
        }
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);
    }
    let partitions = vec![PartitionId(0); triangles.len()];
    MeshAsset {
        name: "Cube".into(),
        positions,
        normals,
        triangles,
        triangle_partitions: partitions,
        ..MeshAsset::default()
    }
}

/// A quad whose UVs span the 0/1 seam: every corner sits on a seam edge.
fn uv_seam() -> MeshAsset {
    MeshAsset {
        name: "Seam".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    }
}

/// Two bone partitions: partition 0 deforms by Root/Spine, partition 1 by
/// Root/Arm, so the writer must emit two palettes.
///
/// Slots are authored in ascending bone order: the SE stream preserves the
/// authored order verbatim, while the LE reader rebuilds slots by walking
/// `NiSkinData` bones in order, so ascending bone order is the only
/// authoring both targets round-trip exactly. Weights sum to 1 so the LE
/// renormalization is a no-op. Zero-weight slots name bone 0, which both
/// palettes contain, because the partition writer maps all four authored
/// slots to palette positions regardless of weight
/// (`src/write/skin.rs`, palette-local indices).
fn skinned_two_partitions() -> MeshAsset {
    let mut bind = vec![identity(), identity(), identity()];
    bind[1][3] = [0.0, 0.0, 1.0, 1.0];
    bind[2][3] = [1.0, 0.0, 0.0, 1.0];
    MeshAsset {
        name: "Skinned".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 2.0],
            [1.0, 0.0, 2.0],
            [1.0, 1.0, 2.0],
            [0.0, 1.0, 2.0],
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
        triangle_partitions: vec![
            PartitionId(0),
            PartitionId(0),
            PartitionId(1),
            PartitionId(1),
        ],
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into(), "Arm".into()],
            bind_inverse: bind,
            indices: vec![
                [0, 1, 0, 0],
                [0, 1, 0, 0],
                [1, 0, 0, 0],
                [0, 1, 0, 0],
                [0, 2, 0, 0],
                [0, 2, 0, 0],
                [0, 2, 0, 0],
                [2, 0, 0, 0],
            ],
            weights: vec![
                [0.75, 0.25, 0.0, 0.0],
                [0.5, 0.5, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.25, 0.75, 0.0, 0.0],
                [0.75, 0.25, 0.0, 0.0],
                [0.4, 0.6, 0.0, 0.0],
                [0.5, 0.5, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
            ],
        }),
        ..MeshAsset::default()
    }
}

fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0f32; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

/// One material slot with both measured texture roles.
fn textured() -> MeshAsset {
    let mut asset = triangle();
    asset.name = "Textured".into();
    asset.materials = vec![MaterialSlot {
        name: "robe".into(),
        shader: ShaderKind::Cloth,
        diffuse: Some("textures/robe_d.dds".into()),
        normal: Some("textures/robe_n.dds".into()),
        specular: None,
        double_sided: true,
    }];
    asset
}

// ── Round-trip helpers ───────────────────────────────────────────────

fn write_model(model: &MeshAsset, target: NifTarget) -> Vec<u8> {
    match target {
        NifTarget::Se => write_se_shape(model),
        NifTarget::Le => write_le_shape(model),
    }
    .expect("the fixture must write")
}

fn roundtrip(model: &MeshAsset, target: NifTarget) -> NifDocument {
    read(&write_model(model, target)).expect("our writer's bytes must parse")
}

/// Compare the read-back document against the authored model.
///
/// Every divergence is named, so a failed round trip points at the drifted
/// attribute. Returns `Err` rather than panicking so the negative control can
/// hold the failure inside an `assert_ne!`.
fn compare(document: &NifDocument, model: &MeshAsset, target: NifTarget) -> Result<(), String> {
    let tolerance = Tolerance::for_target(target);
    if document.header.version != VERSION {
        return Err(format!(
            "header version {:?} is not our {VERSION:#010x}",
            document.header.version
        ));
    }
    if document.header.user_version != target.user_version() {
        return Err(format!(
            "user version {} does not match the {:?} target",
            document.header.user_version, target
        ));
    }
    if document.shapes.len() != 1 {
        return Err(format!(
            "expected one shape, the document carries {}",
            document.shapes.len()
        ));
    }
    let shape = &document.shapes[0];
    if shape.name != model.name {
        return Err(format!("name `{}` is not `{}`", shape.name, model.name));
    }

    if shape.positions.len() != model.positions.len() {
        return Err(format!(
            "{} vertices read back, the model authors {}",
            shape.positions.len(),
            model.positions.len()
        ));
    }
    for (vertex, (actual, expected)) in shape.positions.iter().zip(&model.positions).enumerate() {
        for (lane, (left, right)) in actual.iter().zip(expected).enumerate() {
            if (left - right).abs() > 1e-6 {
                return Err(format!(
                    "position[{vertex}] lane {lane} drifted: {left} vs {right}"
                ));
            }
        }
    }

    if shape.triangles != model.triangles {
        return Err(format!(
            "triangles differ: {:?} vs authored {:?}",
            shape.triangles, model.triangles
        ));
    }

    // UV absence round-trips: neither writer emits a UV stream for a
    // model that authors none. Normal absence does not — the shared
    // importer synthesizes geometry normals when the file carries none
    // (`byroredux-nif` import/mesh/normal.rs), so normals are compared
    // only when the model authors them.
    if shape.uvs.is_empty() != model.uvs.is_empty() {
        return Err(format!(
            "uv presence changed: model authors {}, {} read back",
            model.uvs.len(),
            shape.uvs.len()
        ));
    }
    if !model.normals.is_empty() && shape.normals.len() != model.normals.len() {
        return Err(format!(
            "{} normals read back, the model authors {}",
            shape.normals.len(),
            model.normals.len()
        ));
    }
    for (vertex, (actual, expected)) in shape.uvs.iter().zip(&model.uvs).enumerate() {
        for (lane, (left, right)) in actual.iter().zip(expected).enumerate() {
            if (left - right).abs() > tolerance.uv {
                return Err(format!(
                    "uv[{vertex}] lane {lane} drifted beyond {}: {left} vs {right}",
                    tolerance.uv
                ));
            }
        }
    }
    for (vertex, (actual, expected)) in shape.normals.iter().zip(&model.normals).enumerate() {
        for (lane, (left, right)) in actual.iter().zip(expected).enumerate() {
            if (left - right).abs() > tolerance.normal {
                return Err(format!(
                    "normal[{vertex}] lane {lane} drifted beyond {}: {left} vs {right}",
                    tolerance.normal
                ));
            }
        }
    }

    let authored_skin = model.skin.as_ref();
    if authored_skin.is_none() && !shape.bones.is_empty() {
        return Err(format!(
            "the model has no skin but {} bones read back",
            shape.bones.len()
        ));
    }
    if let Some(skin) = authored_skin {
        if shape.bones != skin.bones {
            return Err(format!(
                "bone names {:?} differ from authored {:?}",
                shape.bones, skin.bones
            ));
        }
        if shape.bone_indices != skin.indices {
            return Err(format!(
                "bone indices differ: {:?} vs authored {:?}",
                shape.bone_indices, skin.indices
            ));
        }
        for (vertex, (actual, expected)) in shape.weights.iter().zip(&skin.weights).enumerate() {
            for (slot, (left, right)) in actual.iter().zip(expected).enumerate() {
                if (left - right).abs() > tolerance.weight {
                    return Err(format!(
                        "weight[{vertex}] slot {slot} drifted beyond {}: {left} vs {right}",
                        tolerance.weight
                    ));
                }
            }
        }
        if shape.bind_inverse.len() != skin.bind_inverse.len() {
            return Err(format!(
                "{} bind matrices read back, the model authors {}",
                shape.bind_inverse.len(),
                skin.bind_inverse.len()
            ));
        }
        for (bone, (actual, expected)) in shape
            .bind_inverse
            .iter()
            .zip(&skin.bind_inverse)
            .enumerate()
        {
            for (column, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                for (row, (left, right)) in actual.iter().zip(expected).enumerate() {
                    if (left - right).abs() > 1e-5 {
                        return Err(format!(
                            "bind[{bone}] [{column}][{row}] drifted: {left} vs {right}"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// The `NiSkinPartition` block index each target's writer layout produces.
///
/// SE: shape 0, instance 1, data 2, partition 3 (`src/write/se_shape.rs`,
/// block-layout comment). LE: shape 0, data 1, instance 2, data 3,
/// partition 4 (`src/write/le_shape.rs`).
fn partition_block(target: NifTarget) -> usize {
    match target {
        NifTarget::Se => 3,
        NifTarget::Le => 4,
    }
}

/// The `BSLightingShaderProperty` block index each target's writer layout
/// produces: after the geometry/skin group, one block per material slot.
/// SE geometry group is one block (skinned: 4 + bones); LE is two
/// (skinned: 5 + bones).
fn shader_block(model: &MeshAsset, target: NifTarget) -> usize {
    let bones = model.skin.as_ref().map_or(0, |skin| skin.bones.len());
    match (target, model.skin.is_some()) {
        (NifTarget::Se, false) => 1,
        (NifTarget::Se, true) => 5 + bones,
        (NifTarget::Le, false) => 2,
        (NifTarget::Le, true) => 6 + bones,
    }
}

/// Compare the written skin partitions against the model's derived palettes.
///
/// The wire format stores no partition id: `NiSkinPartition` is an ordered
/// entry list, and the writer derives entries from
/// `MeshAsset::partition_palettes` — a `BTreeMap` keyed by ascending
/// `PartitionId`. Id identity is therefore checked as entry order plus the
/// exact palette of each entry.
fn compare_partitions(model: &MeshAsset, target: NifTarget) -> Result<(), String> {
    let Some(_skin) = model.skin.as_ref() else {
        return Ok(());
    };
    let scene = parse_nif(&write_model(model, target))
        .map_err(|error| format!("the written NIF must parse: {error}"))?;
    let partition = scene
        .get_as::<NiSkinPartition>(partition_block(target))
        .ok_or_else(|| format!("no NiSkinPartition at block {}", partition_block(target)))?;
    let palettes = model.partition_palettes();
    if partition.partitions.len() != palettes.len() {
        return Err(format!(
            "{} skin partitions read back, the model derives {} from ids {:?}",
            partition.partitions.len(),
            palettes.len(),
            palettes.iter().map(|p| p.partition).collect::<Vec<_>>()
        ));
    }
    for (index, (entry, palette)) in partition.partitions.iter().zip(&palettes).enumerate() {
        if entry.bones != palette.bones {
            return Err(format!(
                "partition {index} palette {:?} differs from derived {:?}",
                entry.bones, palette.bones
            ));
        }
        let triangles: Vec<&[u32; 3]> = model
            .triangles
            .iter()
            .zip(&model.triangle_partitions)
            .filter(|(_, id)| **id == palette.partition)
            .map(|(triangle, _)| triangle)
            .collect();
        if entry.num_triangles as usize != triangles.len() {
            return Err(format!(
                "partition {index} carries {} triangles, the model assigns {}",
                entry.num_triangles,
                triangles.len()
            ));
        }
        let vertices: BTreeSet<u32> = triangles
            .iter()
            .flat_map(|triangle| triangle.iter().copied())
            .collect();
        if entry.num_vertices as usize != vertices.len() {
            return Err(format!(
                "partition {index} carries {} vertices, its triangles touch {}",
                entry.num_vertices,
                vertices.len()
            ));
        }
        let mapped: BTreeSet<u32> = entry.vertex_map.iter().map(|v| u32::from(*v)).collect();
        if mapped != vertices {
            return Err(format!(
                "partition {index} vertex map {mapped:?} differs from touched {vertices:?}"
            ));
        }
        if entry.num_weights_per_vertex != 4 {
            return Err(format!(
                "partition {index} carries {} weights per vertex",
                entry.num_weights_per_vertex
            ));
        }
    }
    Ok(())
}

/// The texture paths `write::shader::texture_set` projects for a slot:
/// diffuse then normal, trimmed to the highest present role.
fn expected_texture_paths(material: &MaterialSlot) -> Vec<String> {
    let diffuse = material.diffuse.as_deref().unwrap_or("");
    let normal = material.normal.as_deref().unwrap_or("");
    match normal.is_empty() {
        false => vec![diffuse.into(), normal.into()],
        true if !diffuse.is_empty() => vec![diffuse.into()],
        true => Vec::new(),
    }
}

/// Compare texture paths through the parsed shader blocks, as the task
/// specifies: `BSLightingShaderProperty` → `BSShaderTextureSet`.
fn compare_textures(model: &MeshAsset, target: NifTarget) -> Result<(), String> {
    let Some(material) = model.materials.first() else {
        return Ok(());
    };
    let scene = parse_nif(&write_model(model, target))
        .map_err(|error| format!("the written NIF must parse: {error}"))?;
    let block = shader_block(model, target);
    let shader = scene
        .get_as::<BSLightingShaderProperty>(block)
        .ok_or_else(|| format!("no BSLightingShaderProperty at block {block}"))?;
    let texture_index = shader
        .texture_set_ref
        .index()
        .ok_or("the shader carries no texture set reference")?;
    let textures = scene
        .get_as::<BSShaderTextureSet>(texture_index)
        .ok_or_else(|| format!("no BSShaderTextureSet at block {texture_index}"))?;
    let expected = expected_texture_paths(material);
    if textures.textures != expected {
        return Err(format!(
            "texture paths {:?} differ from authored {expected:?}",
            textures.textures
        ));
    }
    Ok(())
}

// ── The suite: one test per fixture, both targets each ───────────────

#[test]
fn triangle_roundtrips_on_se_and_le() {
    for target in [NifTarget::Se, NifTarget::Le] {
        let model = triangle();
        let document = roundtrip(&model, target);
        compare(&document, &model, target)
            .unwrap_or_else(|error| panic!("{target:?} triangle round trip: {error}"));
    }
}

#[test]
fn flat_shaded_cube_roundtrips_on_se_and_le() {
    for target in [NifTarget::Se, NifTarget::Le] {
        let model = cube();
        let document = roundtrip(&model, target);
        compare(&document, &model, target)
            .unwrap_or_else(|error| panic!("{target:?} cube round trip: {error}"));
    }
}

#[test]
fn uv_seam_roundtrips_on_se_and_le() {
    for target in [NifTarget::Se, NifTarget::Le] {
        let model = uv_seam();
        let document = roundtrip(&model, target);
        compare(&document, &model, target)
            .unwrap_or_else(|error| panic!("{target:?} uv seam round trip: {error}"));
    }
}

#[test]
fn two_partition_skin_roundtrips_on_se_and_le() {
    for target in [NifTarget::Se, NifTarget::Le] {
        let model = skinned_two_partitions();
        let document = roundtrip(&model, target);
        compare(&document, &model, target)
            .unwrap_or_else(|error| panic!("{target:?} skin round trip: {error}"));
        compare_partitions(&model, target)
            .unwrap_or_else(|error| panic!("{target:?} skin partitions: {error}"));
    }
}

#[test]
fn textured_mesh_roundtrips_on_se_and_le() {
    for target in [NifTarget::Se, NifTarget::Le] {
        let model = textured();
        let document = roundtrip(&model, target);
        compare(&document, &model, target)
            .unwrap_or_else(|error| panic!("{target:?} textured round trip: {error}"));
        compare_textures(&model, target)
            .unwrap_or_else(|error| panic!("{target:?} texture paths: {error}"));
    }
}

/// Negative control: one altered field must trip the comparison.
///
/// The file is written from the original model; the comparison then faces a
/// model whose vertex 1 moved. `assert_ne!` on the comparison result proves
/// the comparator detects divergence instead of passing everything.
#[test]
fn an_altered_field_must_fail_the_comparison() {
    let original = triangle();
    let mut altered = original.clone();
    altered.positions[1] = [9.0, 9.0, 9.0];
    for target in [NifTarget::Se, NifTarget::Le] {
        let document = roundtrip(&original, target);
        assert_ne!(
            compare(&document, &altered, target),
            Ok(()),
            "{target:?}: moving a vertex must fail the comparison"
        );
    }
}
