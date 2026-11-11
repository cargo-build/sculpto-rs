//! Preserve the neutral rest proof in a complete glTF hierarchy. @req DECL0008

use glam::DMat4;
use sculpto_core::mesh::MeshAsset;
use sculpto_core::rig::RigFrame;
use sculpto_core::rig::rest::{BoundRestSkin, RestBinding};
use serde_json::{Value, json};

use crate::{Clip, GltfError, encode_glb};

/// Export a checked source shape and full rig through the shared GLB encoder.
/// Clips retain palette-indexed targets and rig-local values. @req DECL0008
///
/// # Errors
/// Refuses ambiguous or missing shape identity, mismatched source geometry/skin,
/// and every geometry, influence or clip error of [`crate::write_glb_with`].
pub fn write_rigged_glb(
    asset: &MeshAsset,
    binding: &RestBinding<'_>,
    shape: usize,
    clips: &[Clip],
) -> Result<Vec<u8>, GltfError> {
    let layout = Layout::new(asset, binding, shape)?;
    encode_glb(asset, clips, Some(&layout))
}

pub(crate) struct Layout<'a> {
    binding: &'a RestBinding<'a>,
    skin: &'a BoundRestSkin<'a>,
}

impl<'a> Layout<'a> {
    fn new(
        asset: &MeshAsset,
        binding: &'a RestBinding<'a>,
        shape: usize,
    ) -> Result<Self, GltfError> {
        let mismatch = |field| GltfError::RigBinding { shape, field };
        let mut matches = binding
            .skins()
            .iter()
            .filter(|skin| skin.source().shape == shape);
        let skin = matches.next().ok_or_else(|| mismatch("shape identity"))?;
        if matches.next().is_some() {
            return Err(mismatch("ambiguous shape identity"));
        }
        if asset.positions != skin.source().positions {
            return Err(mismatch("positions"));
        }
        if asset.skin.as_ref() != Some(skin.source().skin) {
            return Err(mismatch("skin"));
        }
        Ok(Self { binding, skin })
    }

    pub(crate) fn bone_node(&self, bone: usize) -> usize {
        self.skin.palette()[bone].index() + 1
    }

    pub(crate) fn joint_nodes(&self) -> Vec<usize> {
        self.skin
            .palette()
            .iter()
            .map(|joint| joint.index() + 1)
            .collect()
    }

    pub(crate) fn root(&self) -> usize {
        self.binding.rig().joints().len() + 1
    }

    pub(crate) fn scene(&self) -> usize {
        self.root() + 1
    }

    pub(crate) fn nodes(&self, mut mesh: Value) -> Vec<Value> {
        let rig = self.binding.rig();
        let global = self
            .skin
            .source()
            .global_to_skin
            .map(|column| column.map(f64::from));
        let placement = DMat4::from_cols_array_2d(&global).inverse();
        mesh["matrix"] = json!(placement.to_cols_array());
        let mut nodes = vec![mesh];
        nodes.extend(rig.joints().iter().map(|joint| {
            let mut node = json!({
                "translation": joint.rest.translation,
                "rotation": joint.rest.rotation,
                "scale": ([joint.rest.scale; 3]),
            });
            if let Some(name) = &joint.name {
                node["name"] = json!(name);
            }
            node
        }));
        let mut children = vec![Vec::new(); rig.joints().len()];
        rig.joints().iter().enumerate().for_each(|(index, joint)| {
            if let Some(parent) = joint.parent {
                children[parent.index()].push(index + 1);
            }
        });
        children
            .into_iter()
            .enumerate()
            .filter(|(_, children)| !children.is_empty())
            .for_each(|(index, children)| nodes[index + 1]["children"] = json!(children));
        let basis: Vec<_> = self.binding.basis().into_iter().flatten().collect();
        nodes.push(json!({"matrix":basis,"children":rig.roots().iter().map(|root|root.index()+1).collect::<Vec<_>>()}));
        let frame = match rig.frame() {
            RigFrame::SourceZUp => [
                1.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            RigFrame::YUp => DMat4::IDENTITY.to_cols_array(),
        };
        nodes.push(json!({"matrix":frame,"children":[0,self.root()]}));
        nodes
    }
}
