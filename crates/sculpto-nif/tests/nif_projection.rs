//! BLD-09 preservation measurement: does the `export-nif --all-shapes`
//! projection carry triangle partitions, materials and morphs through?
//!
//! The projection under test is exactly the engine's SE export path
//! (`crates/sculpto-engine/src/ops.rs` `export_nif_bundle`):
//! `read` -> `MeshAsset::from(&NifShape)` -> `write_se_bundle`. The test
//! re-encodes in-process and compares raw block facts — the dismember
//! partition table, the `NiSkinPartition` entries, the shader property and
//! its texture set, and the morph targets — between the source file and the
//! re-encode, so a dropped property is named, not just counted.
//!
//! Host-gated on `SCULPTO_BUNDLE_NIF`; without it the test skips.

use byroredux_nif::blocks::controller::{NiGeomMorpherController, NiMorphData};
use byroredux_nif::blocks::shader::{BSLightingShaderProperty, BSShaderTextureSet};
use byroredux_nif::blocks::skin::{BsDismemberSkinInstance, NiSkinInstance, NiSkinPartition};
use byroredux_nif::blocks::tri_shape::BsTriShape;
use byroredux_nif::parse_nif;
use byroredux_nif::scene::NifScene;
use byroredux_nif::types::BlockRef;
use sculpto_core::mesh::MeshAsset;
use sculpto_nif::write::se_bundle::write_se_bundle;

/// Raw per-shape facts the projection must preserve.
#[derive(Debug, Clone)]
struct ShapeFacts {
    name: String,
    /// `(part_flag, body_part)` per `BSDismemberSkinInstance` slot; empty when
    /// the skin instance is a plain `NiSkinInstance`.
    dismember: Vec<(u16, u16)>,
    /// One row per `NiSkinPartition` entry: `(triangles, vertices, bones)`.
    partition_entries: Vec<(usize, usize, usize)>,
    /// Shader property facts: `(block type, editor/material name, textures)`.
    shader: Option<ShaderFacts>,
    /// Morph target names reached from the shape's own controller ref.
    morphs: Vec<String>,
}

#[derive(Debug, Clone)]
struct ShaderFacts {
    /// Block type name, e.g. `BSLightingShaderProperty`.
    block: String,
    /// `NiObjectNET` name (often empty for Skyrim garments).
    name: Option<String>,
    /// Authored material path when the shader names one (`root_material_path`).
    material_path: Option<String>,
    /// Texture paths in slot order from the linked `BSShaderTextureSet`.
    textures: Vec<String>,
}

fn owned(value: Option<&str>) -> Option<String> {
    value.map(str::to_string)
}

/// Per-shape facts of every `BSTriShape` block, in scene order.
fn shape_facts(scene: &NifScene) -> Vec<ShapeFacts> {
    (0..scene.len())
        .filter_map(|index| {
            let shape = scene.get_as::<BsTriShape>(index)?;
            Some(facts_of(scene, shape))
        })
        .collect()
}

fn facts_of(scene: &NifScene, shape: &BsTriShape) -> ShapeFacts {
    let mut dismember = Vec::new();
    let mut partition_entries = Vec::new();
    if let Some(instance) = scene
        .get(shape.skin_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| block.as_any().downcast_ref::<BsDismemberSkinInstance>())
    {
        dismember = instance
            .partitions
            .iter()
            .map(|slot| (slot.part_flag, slot.body_part))
            .collect();
        collect_partition_entries(
            scene,
            instance.base.skin_partition_ref,
            &mut partition_entries,
        );
    } else if let Some(instance) = scene
        .get(shape.skin_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| block.as_any().downcast_ref::<NiSkinInstance>())
    {
        collect_partition_entries(scene, instance.skin_partition_ref, &mut partition_entries);
    }

    let shader = scene
        .get(shape.shader_property_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| {
            let block = block.as_any();
            block
                .downcast_ref::<BSLightingShaderProperty>()
                .map(|lighting| ShaderFacts {
                    block: "BSLightingShaderProperty".to_string(),
                    name: owned(lighting.net.name.as_deref()),
                    material_path: owned(lighting.root_material_path.as_deref()),
                    textures: texture_set(scene, lighting.texture_set_ref),
                })
        });

    let mut morphs = Vec::new();
    let morpher = scene
        .get(shape.av.net.controller_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| block.as_any().downcast_ref::<NiGeomMorpherController>());
    let data = morpher.and_then(|morpher| {
        scene
            .get(morpher.data_ref.index().unwrap_or(usize::MAX))
            .and_then(|block| block.as_any().downcast_ref::<NiMorphData>())
    });
    if let Some(data) = data {
        morphs = data
            .morphs
            .iter()
            .map(|target| owned(target.name.as_deref()).unwrap_or_default())
            .collect();
    }

    ShapeFacts {
        name: owned(shape.av.net.name.as_deref()).unwrap_or_default(),
        dismember,
        partition_entries,
        shader,
        morphs,
    }
}

fn collect_partition_entries(
    scene: &NifScene,
    partition_ref: BlockRef,
    entries: &mut Vec<(usize, usize, usize)>,
) {
    let Some(partition) = scene
        .get(partition_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| block.as_any().downcast_ref::<NiSkinPartition>())
    else {
        return;
    };
    for entry in &partition.partitions {
        entries.push((
            entry.num_triangles as usize,
            entry.num_vertices as usize,
            entry.bones.len(),
        ));
    }
}

fn texture_set(scene: &NifScene, set_ref: BlockRef) -> Vec<String> {
    scene
        .get(set_ref.index().unwrap_or(usize::MAX))
        .and_then(|block| block.as_any().downcast_ref::<BSShaderTextureSet>())
        .map(|set| set.textures.clone())
        .unwrap_or_default()
}

/// File-scope morph census: controller and data blocks the link walk above
/// might have missed.
fn morph_census(scene: &NifScene) -> (usize, usize) {
    let mut morphers = 0;
    let mut datas = 0;
    for index in 0..scene.len() {
        match scene.get(index).map(|block| block.block_type_name()) {
            Some("NiGeomMorpherController") => morphers += 1,
            Some("NiMorphData") => datas += 1,
            _ => {}
        }
    }
    (morphers, datas)
}

fn facts_lines(label: &str, facts: &[ShapeFacts], census: (usize, usize)) -> Vec<String> {
    let mut lines = vec![format!(
        "{label}: {} shapes, morpher/data census {:?}",
        facts.len(),
        census
    )];
    for shape in facts {
        lines.push(format!(
            "  {:?}: dismember {:?}, partition entries {:?}, shader {:?}, morphs {:?}",
            shape.name, shape.dismember, shape.partition_entries, shape.shader, shape.morphs
        ));
    }
    lines
}

/// The BLD-09 contract on the real garment: the re-encode carries the source's
/// partitions, materials and morphs per shape. Fails naming every lost
/// property, so one run is the complete measurement.
#[test]
fn real_garment_preserves_partitions_materials_and_morphs() {
    let Ok(path) = std::env::var("SCULPTO_BUNDLE_NIF") else {
        eprintln!("SCULPTO_BUNDLE_NIF not set; skipping the BLD-09 preservation measurement");
        return;
    };
    let source_bytes = std::fs::read(&path).expect("the fixture NIF is readable");
    let source = sculpto_nif::read(&source_bytes).expect("the real garment parses");

    // The engine's SE export path, verbatim: project every shape, write the
    // bundle (`crates/sculpto-engine/src/ops.rs` `export_nif_bundle`).
    let assets: Vec<MeshAsset> = source.shapes.iter().map(MeshAsset::from).collect();
    let reencoded_bytes = write_se_bundle(&assets).expect("the re-encode writes");

    let source_scene = parse_nif(&source_bytes).expect("the source parses raw");
    let reencoded_scene = parse_nif(&reencoded_bytes).expect("the re-encode parses raw");
    assert!(
        !source_scene.truncated,
        "the source parses without dropped blocks"
    );
    assert!(
        !reencoded_scene.truncated,
        "the re-encode parses without dropped blocks"
    );

    let source_facts = shape_facts(&source_scene);
    let reencoded_facts = shape_facts(&reencoded_scene);
    let source_census = morph_census(&source_scene);
    let reencoded_census = morph_census(&reencoded_scene);

    for line in facts_lines("source", &source_facts, source_census) {
        eprintln!("{line}");
    }
    for line in facts_lines("reencode", &reencoded_facts, reencoded_census) {
        eprintln!("{line}");
    }

    assert_eq!(
        source_facts.len(),
        reencoded_facts.len(),
        "the re-encode keeps every shape",
    );
    let mut lost: Vec<String> = Vec::new();
    for (source, reencoded) in source_facts.iter().zip(&reencoded_facts) {
        assert_eq!(source.name, reencoded.name, "shape names survive in order");
        if source.dismember != reencoded.dismember {
            lost.push(format!(
                "partitions: `{}` dismember slots {:?} -> {:?}",
                source.name, source.dismember, reencoded.dismember
            ));
        }
        if source.partition_entries != reencoded.partition_entries {
            lost.push(format!(
                "partitions: `{}` skin-partition entries {:?} -> {:?}",
                source.name, source.partition_entries, reencoded.partition_entries
            ));
        }
        if source.shader.as_ref().map(|shader| {
            (
                &shader.block,
                &shader.name,
                &shader.material_path,
                &shader.textures,
            )
        }) != reencoded.shader.as_ref().map(|shader| {
            (
                &shader.block,
                &shader.name,
                &shader.material_path,
                &shader.textures,
            )
        }) {
            lost.push(format!(
                "materials: `{}` shader {:?} -> {:?}",
                source.name, source.shader, reencoded.shader
            ));
        }
        if source.morphs != reencoded.morphs {
            lost.push(format!(
                "morphs: `{}` targets {:?} -> {:?}",
                source.name, source.morphs, reencoded.morphs
            ));
        }
    }
    if source_census != reencoded_census {
        lost.push(format!(
            "morphs: file census {source_census:?} -> {reencoded_census:?}"
        ));
    }
    assert!(
        lost.is_empty(),
        "the export projection lost {} property group(s): {}",
        lost.len(),
        lost.join("; "),
    );
}
