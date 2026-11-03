//! The `AttachLight` node a light-bearing NIF authors to say where its
//! light sits.
//!
//! A LIGH record with a model (barrel fires, torches, lamps) places the REFR
//! at the model's base; the light itself belongs at the model's
//! `AttachLight` NiNode — the Construction Set's own light-authoring
//! workflow copies `AttachLight` and `FlameNode` into a new light NIF and
//! moves them to position the light and the flame (CS wiki, *Making A New
//! Light*). Census over the vanilla mesh archives: FO3 10 NIFs, FNV 9,
//! Skyrim SE 74, Oblivion 65 author one, never more than one per file; 21
//! Skyrim and 3 Oblivion ones sit under a transformed parent, and 15
//! Oblivion roots carry a non-identity transform, so the offset must be the
//! composed root→node chain, not the node's local translation.
//!
//! The match is the exact node name. The earlier substring rule
//! (`flame` / `fire` / `attachlight`) picked the wrong node in 2 FO3, 1 FNV,
//! 16 Skyrim and 5 Oblivion files — the root of `barrel02firelight.nif`
//! (`Barrel02fireLight`), Skyrim's `TorchFire`, Oblivion's `FlameNode2`,
//! particle emitters — which is why its spawn-side use was reverted.

use super::coord::zup_point_to_yup;
use super::transform::compose_transforms;
use super::units;
use super::walk::{as_ni_node, switch_active_children};
use crate::scene::NifScene;
use crate::types::NiTransform;

/// The authored node name, matched case-insensitively and exactly.
pub const ATTACH_LIGHT_NODE: &str = "AttachLight";

/// Bound on the node-chain walk; real light NIFs nest at most ~10 deep.
const MAX_DEPTH: u32 = 256;

/// Root-local position (renderer Y-up, scene length units applied) of the
/// NIF's `AttachLight` node, composed through every ancestor the same way
/// the flat mesh walk composes geometry — root transform included, switch /
/// LOD nodes following only their active child. `None` when the model
/// authors no such node; the caller then keeps the light at the placement.
pub fn find_attach_light_offset(scene: &NifScene) -> Option<[f32; 3]> {
    find_named_node_offset(scene, ATTACH_LIGHT_NODE)
}

/// The authored name of the node a launcher's projectiles leave from.
pub const PROJECTILE_NODE: &str = "ProjectileNode";

/// Root-local position of the NIF's `ProjectileNode` (the muzzle of a trap's dart
/// port: Skyrim's `TrapDartPortSingle01` authors it 16 units ahead of the model
/// origin), composed like [`find_attach_light_offset`]. `None` when the model
/// authors none.
pub fn find_projectile_node_offset(scene: &NifScene) -> Option<[f32; 3]> {
    find_named_node_offset(scene, PROJECTILE_NODE)
}

fn find_named_node_offset(scene: &NifScene, name: &str) -> Option<[f32; 3]> {
    let root = scene.root_index?;
    let mut visited = vec![false; scene.blocks.len()];
    let world = find_node(scene, root, &NiTransform::default(), 0, &mut visited, name)?;
    let mut offset = zup_point_to_yup(&world.translation);
    let scale = units::length_scale(scene);
    if scale != 1.0 {
        for c in &mut offset {
            *c *= scale;
        }
    }
    Some(offset)
}

fn find_node(
    scene: &NifScene,
    idx: usize,
    parent: &NiTransform,
    depth: u32,
    visited: &mut [bool],
    wanted: &str,
) -> Option<NiTransform> {
    if depth > MAX_DEPTH {
        return None;
    }
    let block = scene.blocks.get(idx)?;
    // Cycle guard: a malformed child list must terminate the walk.
    if std::mem::replace(visited.get_mut(idx)?, true) {
        return None;
    }
    let (node, children) = match switch_active_children(block.as_ref()) {
        Some((node, active)) => (node, active),
        None => {
            let node = as_ni_node(block.as_ref())?;
            let children = node.children.iter().filter_map(|c| c.index()).collect();
            (node, children)
        }
    };
    let world = compose_transforms(parent, &node.av.transform);
    if node
        .av
        .net
        .name
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
    {
        return Some(world);
    }
    children
        .into_iter()
        .find_map(|child| find_node(scene, child, &world, depth + 1, visited, wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::base::{NiAVObjectData, NiObjectNETData};
    use crate::blocks::node::NiNode;
    use crate::types::{BlockRef, NiMatrix3, NiPoint3};

    fn node(name: &str, translation: [f32; 3], rotation: NiMatrix3, children: &[u32]) -> NiNode {
        NiNode {
            av: NiAVObjectData {
                net: NiObjectNETData {
                    name: Some(std::sync::Arc::from(name)),
                    extra_data_refs: Vec::new(),
                    controller_ref: BlockRef::NULL,
                },
                flags: 0,
                transform: NiTransform {
                    rotation,
                    translation: NiPoint3 {
                        x: translation[0],
                        y: translation[1],
                        z: translation[2],
                    },
                    scale: 1.0,
                },
                properties: Vec::new(),
                collision_ref: BlockRef::NULL,
            },
            children: children.iter().map(|&i| BlockRef(i)).collect(),
            effects: Vec::new(),
        }
    }

    fn scene(nodes: Vec<NiNode>) -> NifScene {
        NifScene {
            blocks: nodes
                .into_iter()
                .map(|n| Box::new(n) as Box<dyn crate::blocks::NiObject>)
                .collect(),
            root_index: Some(0),
            ..NifScene::default()
        }
    }

    /// `barrel02firelight.nif`'s shape: the root is named with "fire" in
    /// it (the old substring rule's false positive) and `AttachLight` is a
    /// direct child 102 units up. Z-up (-0.4, 0.8, 102.1) → Y-up.
    #[test]
    fn exact_attach_light_wins_over_a_fire_named_root() {
        let s = scene(vec![
            node("Barrel02fireLight", [0.0; 3], NiMatrix3::default(), &[1, 2]),
            node("AddOnNode14", [-0.4, 0.8, 72.5], NiMatrix3::default(), &[]),
            node("AttachLight", [-0.4, 0.8, 102.1], NiMatrix3::default(), &[]),
        ]);
        let offset = find_attach_light_offset(&s).expect("AttachLight is authored");
        assert!((offset[0] - -0.4).abs() < 1e-4);
        assert!(
            (offset[1] - 102.1).abs() < 1e-4,
            "Z-up height becomes Y-up height"
        );
        assert!((offset[2] - -0.8).abs() < 1e-4);
    }

    /// Fire / flame names are not the light anchor: a NIF with only
    /// `TorchFire` / `FlameNode2` authors no light position.
    #[test]
    fn flame_and_fire_named_nodes_are_not_matched() {
        let s = scene(vec![
            node("Torch", [0.0; 3], NiMatrix3::default(), &[1, 2]),
            node("TorchFire", [0.0, 0.0, 30.0], NiMatrix3::default(), &[]),
            node("FlameNode2", [0.0, 0.0, 40.0], NiMatrix3::default(), &[]),
        ]);
        assert_eq!(find_attach_light_offset(&s), None);
    }

    /// The Oblivion hanging-lamp / Skyrim torch shape: `AttachLight` nested
    /// under a rotated, translated parent, with a translated root. The
    /// offset is the composed chain, not the node's local translation.
    #[test]
    fn nested_attach_light_composes_the_whole_chain() {
        // 90° about Z: (x, y, z) → (-y, x, z).
        let rot_z90 = NiMatrix3 {
            rows: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let s = scene(vec![
            node("Root", [0.0, 0.0, 5.0], NiMatrix3::default(), &[1]),
            node("Chain", [10.0, 0.0, 0.0], rot_z90, &[2]),
            node("attachlight", [3.0, 0.0, 0.0], NiMatrix3::default(), &[]),
        ]);
        let offset = find_attach_light_offset(&s).expect("case-insensitive match");
        // Z-up world: root (0,0,5) + chain (10,0,0) + rot·(3,0,0) = (10,3,5).
        // Y-up: (x, z, -y) = (10, 5, -3).
        let want = [10.0, 5.0, -3.0];
        for (got, want) in offset.iter().zip(want) {
            assert!((got - want).abs() < 1e-4, "{offset:?} != {want:?}");
        }
    }

    #[test]
    fn a_child_cycle_terminates() {
        let s = scene(vec![
            node("Root", [0.0; 3], NiMatrix3::default(), &[1]),
            node("Loop", [0.0; 3], NiMatrix3::default(), &[0]),
        ]);
        assert_eq!(find_attach_light_offset(&s), None);
    }
}
