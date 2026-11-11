//! Supplied body/garment skins exercise target bind precision. @req DECL0008

use std::path::Path;

use glam::DVec3;
use sculpto_core::mesh::{MeshAsset, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::template::{TemplateRoots, complete_family_rig};
use sculpto_core::rig::{Rig, RigFrame};
use sculpto_gltf::write_rigged_glb;
use sculpto_mesh::actor::{Actor, Pose};
use sculpto_mesh::rig::map::{BodyFamily, BoneMap};
use sculpto_mesh::rig::remap::{RemapSettings, remap_skin};
use sculpto_nif::NifDocument;

fn load(root: &Path, body: &str, template: &str) -> (NifDocument, Rig) {
    let read = |name: &str| sculpto_nif::read(&std::fs::read(root.join(name)).expect("configured NIF")).expect("fixture decodes");
    let document = read(body);
    let family = read(template);
    let authored = document.rig.as_ref().expect("source rig");
    let template = family.rig.as_ref().expect("family template rig");
    let rig = complete_family_rig(authored, template, TemplateRoots {
        authored: authored.roots()[0], template: template.roots()[0],
    }).expect("compatible family template");
    (document, rig)
}

fn raw_weight(skin: &Skin, vertex: usize, name: &str) -> f32 {
    let row = skin.influences.row(vertex).expect("weight row");
    row.joints.iter().zip(row.weights).filter(|(slot,_)| skin.bones[usize::from(**slot)] == name).map(|(_,weight)| *weight).sum()
}

#[test]
fn configured_family_remaps_keep_placement_and_locked_weights() {
    let Some(root) = std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES") else {
        eprintln!("SKIP configured_family_remaps_keep_placement_and_locked_weights: SCULPTO_RIG_TEMPLATE_FIXTURES is unset");
        return;
    };
    let root = Path::new(&root);
    let bodies = [
        load(root, "himbo.nif", "skeleton_male_sse.nif"),
        load(root, "bijin.nif", "skeleton_female_sse.nif"),
    ];
    let families = [BodyFamily::Himbo, BodyFamily::Cbbe];
    let lock_name = "NPC Pelvis [Pelv]";
    for (source_index, target_index) in [(0,1),(1,0)] {
        let (source, source_rig) = &bodies[source_index];
        let (target, target_rig) = &bodies[target_index];
        let source_binding = source.bind_rest_on(source_rig).expect("source body/garment proof");
        let target_binding = target.bind_rest_on(target_rig).expect("target reference proof");
        let target_shape = target.shapes.iter().enumerate().max_by_key(|(_,shape)|shape.positions.len()).expect("reference body").0;
        let map = BoneMap::build(families[source_index], &source_binding, families[target_index], target_rig, &[]).expect("supported direct family map");
        for source_skin in source_binding.skins() {
            let original = source_skin.source();
            let settings = RemapSettings {
                source_shape: original.shape, target_shape,
                locked_source_joints: vec![source_rig.joint_by_name(lock_name).expect("source pelvis")],
            };
            let output = remap_skin(&source_binding, &map, &target_binding, &settings).expect("real family skin transfer");
            let mut max_sum_error = 0.0_f32;
            for vertex in 0..output.skin.influences.rows() {
                let row = output.skin.influences.row(vertex).expect("output row");
                assert!(row.weights.iter().filter(|&&w|w>0.0).count() <= 4);
                max_sum_error = max_sum_error.max((row.weights.iter().sum::<f32>()-1.0).abs());
                assert_eq!(raw_weight(original.skin, vertex, lock_name).to_bits(),raw_weight(&output.skin, vertex, lock_name).to_bits(),"vertex{vertex} lock");
            }
            assert!(max_sum_error <= 1e-4);
            let proof = RestBinding::prove_with_basis(target_rig, RigFrame::SourceZUp, &[SkinRestSource {
                shape: original.shape, root: output.root, positions: original.positions,
                skin: &output.skin, global_to_skin: output.global_to_skin,
            }],target_binding.basis()).expect("encoded transfer at exact target basis");
            let mut asset = MeshAsset::from(&source.shapes[original.shape]);
            asset.skin = Some(output.skin.clone());
            let bytes = write_rigged_glb(&asset, &proof, original.shape, &[]).expect("real remapped GLB");
            let actor = Actor::from_glb(&bytes).expect("real Actor import");
            let sample = actor.sample(&[&asset.name],Pose::Rest).expect("rest reconstruction");
            assert_eq!(sample.vertices.len(), original.positions.len());
            let placement = source.shape_placement(original.shape).expect("source placement");
            let max_actor_error = original.positions.iter().zip(&sample.vertices).map(|(&point,actual)| {
                let point = placement.position_in_source(point);
                DVec3::new(f64::from(point[0]),f64::from(point[2]),-f64::from(point[1])).distance(DVec3::from_array(*actual))
            }).fold(0.0_f64,f64::max);
            assert!(max_actor_error <= 1e-4,"{} rest error {max_actor_error}",asset.name);
            eprintln!("{:?}->{:?} {} vertices={} sum_error={max_sum_error:.9e} actor_rest={max_actor_error:.9e} dropped={} source_G_preserved={}",families[source_index],families[target_index],asset.name,asset.positions.len(),output.report.discarded.len(),output.global_to_skin==original.global_to_skin);
            assert_eq!(output.global_to_skin, original.global_to_skin);
        }
    }
}
