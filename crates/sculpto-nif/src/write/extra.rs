//! `NiStringExtraData` blocks: the HDT-SMP physics-file link.
//!
//! BodySlide links a model to its physics XML with one `NiStringExtraData` that
//! the plugin scans for by name; the marker below is the reference's spelling,
//! not a guess: `OutfitProject.cpp:250-252` documents the link, and
//! `physics/Controller.cpp:154` compares against the canonical constant
//! `HDT Skinned Mesh Physics Object` (`physics/Controller.cpp:52`)
//! case-insensitively (`hdtRefUtils.h:120-128`). BodySlide only ever
//! keeps one, attached to the model root node (`OutfitProject.cpp:7023-7062`
//! deletes the inherited links before assigning the chosen one).
//!
//! On Skyrim the body is two string-table indices — the block name and the
//! string value — because `NiString` is a table index from `20.1.0.1` on
//! (`vendor/byroredux/nif/src/blocks/extra_data.rs:130-132`,
//! `vendor/byroredux/nif/src/stream.rs:589-597`).

use sculpto_core::mesh::{MeshAsset, PhysicsRef};

use super::header::NifWriteError;
use super::strings::StringTable;

/// Block type name for the link.
pub const BLOCK_TYPE: &str = "NiStringExtraData";

/// Extra-data name HDT-SMP scans for, compared case-insensitively.
pub const HDT_PHYSICS_NAME: &str = "HDT Skinned Mesh Physics Object";

/// The one extra-data block a model may carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicsLink {
    /// Name the plugin scans for.
    pub name: &'static str,
    /// Physics XML path, written verbatim.
    pub path: String,
}

/// Resolve the model's physics link.
///
/// A path becomes exactly one `NiStringExtraData`. An inline document has no
/// file the game could open, so it is refused instead of written as a link that
/// resolves to nothing.
pub fn physics_link(asset: &MeshAsset) -> Result<Option<PhysicsLink>, NifWriteError> {
    match &asset.physics {
        None => Ok(None),
        Some(PhysicsRef::Path(path)) => Ok(Some(PhysicsLink {
            name: HDT_PHYSICS_NAME,
            path: path.clone(),
        })),
        Some(PhysicsRef::Inline(_)) => Err(NifWriteError::InlinePhysicsUnsupported),
    }
}

/// A `NiStringExtraData` body: the name index, then the string-value index.
pub fn body(link: &PhysicsLink, strings: &mut StringTable) -> Vec<u8> {
    let name = strings.intern(link.name) as i32;
    let value = strings.intern(&link.path) as i32;
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&name.to_le_bytes());
    bytes.extend_from_slice(&value.to_le_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write::le_shape::write_le_shape;
    use crate::write::se_shape::write_se_shape;
    use byroredux_nif::blocks::extra_data::NiExtraData;
    use byroredux_nif::blocks::tri_shape::{BsTriShape, NiTriShape};
    use byroredux_nif::scene::NifScene;
    use sculpto_core::mesh::{PartitionId, PhysicsRef, Skin};

    fn garment() -> MeshAsset {
        MeshAsset {
            name: "Robe".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.5, 0.0, 0.0], [0.0, 2.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            triangle_partitions: vec![PartitionId(0)],
            ..MeshAsset::default()
        }
    }

    fn skinned() -> MeshAsset {
        let mut asset = garment();
        let mut bind = vec![[[0.0f32; 4]; 4]; 2];
        bind[0][0][0] = 1.0;
        bind[0][1][1] = 1.0;
        bind[0][2][2] = 1.0;
        bind[0][3][3] = 1.0;
        bind[1] = bind[0];
        asset.skin = Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: bind,
            indices: vec![[0, 1, 0, 0]; 3],
            weights: vec![[0.75, 0.25, 0.0, 0.0]; 3],
        });
        asset
    }

    /// The `NiStringExtraData` a block references, as `(name, value)`.
    fn linked_extra<'a>(
        scene: &'a NifScene,
        refs: &[byroredux_nif::types::BlockRef],
    ) -> Vec<(&'a str, &'a str)> {
        refs.iter()
            .filter_map(|link| link.index())
            .map(|index| {
                let extra = scene
                    .get_as::<NiExtraData>(index)
                    .unwrap_or_else(|| panic!("block {index} is not extra data"));
                assert_eq!(extra.type_name, BLOCK_TYPE);
                (
                    extra.name.as_deref().expect("named extra data"),
                    extra.string_value.as_deref().expect("string value"),
                )
            })
            .collect()
    }

    fn extra_data_blocks(scene: &NifScene) -> usize {
        (0..scene.len())
            .filter(|index| scene.get_as::<NiExtraData>(*index).is_some())
            .count()
    }

    #[test]
    fn se_garment_carries_one_named_physics_link() {
        let mut asset = garment();
        asset.physics = Some(PhysicsRef::Path("robe-physics.xml".into()));
        let bytes = write_se_shape(&asset).expect("SE writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shape = scene.get_as::<BsTriShape>(0).expect("shape block");
        assert_eq!(
            linked_extra(&scene, &shape.av.net.extra_data_refs),
            vec![(HDT_PHYSICS_NAME, "robe-physics.xml")]
        );
        // The shape is the file root without a skin, so the link lives here.
        assert_eq!(scene.root_index, Some(0), "unskinned root is the shape");
        assert_eq!(extra_data_blocks(&scene), 1);
        // The adapter still reads the shape.
        assert_eq!(crate::read(&bytes).expect("our reader").shapes.len(), 1);
    }

    #[test]
    fn le_garment_carries_one_named_physics_link() {
        let mut asset = garment();
        asset.physics = Some(PhysicsRef::Path("robe-physics.xml".into()));
        let bytes = write_le_shape(&asset).expect("LE writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shape = scene.get_as::<NiTriShape>(0).expect("shape block");
        assert_eq!(
            linked_extra(&scene, &shape.av.net.extra_data_refs),
            vec![(HDT_PHYSICS_NAME, "robe-physics.xml")]
        );
        assert_eq!(extra_data_blocks(&scene), 1);
    }

    /// The reference reads the link from the model root node
    /// (`OutfitProject.cpp:250-252`), and a skinned file has one, so the link
    /// moves there instead of staying on the child shape.
    #[test]
    fn se_skinned_garment_links_its_root_node() {
        let mut asset = skinned();
        asset.physics = Some(PhysicsRef::Path("robe-physics.xml".into()));
        let bytes = write_se_shape(&asset).expect("SE writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shape = scene.get_as::<BsTriShape>(0).expect("shape block");
        assert!(shape.av.net.extra_data_refs.is_empty(), "shape is a child");
        let root = scene
            .root()
            .expect("root node")
            .as_object_net()
            .expect("NiNode");
        assert_eq!(
            linked_extra(&scene, root.extra_data_refs()),
            vec![(HDT_PHYSICS_NAME, "robe-physics.xml")]
        );
        assert_eq!(extra_data_blocks(&scene), 1);
    }

    #[test]
    fn le_skinned_garment_links_its_root_node() {
        let mut asset = skinned();
        asset.physics = Some(PhysicsRef::Path("robe-physics.xml".into()));
        let bytes = write_le_shape(&asset).expect("LE writes");
        let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
        let shape = scene.get_as::<NiTriShape>(0).expect("shape block");
        assert!(shape.av.net.extra_data_refs.is_empty(), "shape is a child");
        let root = scene
            .root()
            .expect("root node")
            .as_object_net()
            .expect("NiNode");
        assert_eq!(
            linked_extra(&scene, root.extra_data_refs()),
            vec![(HDT_PHYSICS_NAME, "robe-physics.xml")]
        );
        assert_eq!(extra_data_blocks(&scene), 1);
    }

    #[test]
    fn a_model_without_physics_writes_no_link() {
        let asset = garment();
        for bytes in [
            write_se_shape(&asset).expect("SE writes"),
            write_le_shape(&asset).expect("LE writes"),
        ] {
            let scene = byroredux_nif::parse_nif(&bytes).expect("parses");
            assert_eq!(extra_data_blocks(&scene), 0);
            assert!(
                scene
                    .get(0)
                    .expect("shape")
                    .as_object_net()
                    .expect("shape has NiObjectNET")
                    .extra_data_refs()
                    .is_empty(),
                "no refs without a sidecar"
            );
        }
    }

    #[test]
    fn inline_physics_has_no_file_link_and_is_refused() {
        let mut asset = garment();
        asset.physics = Some(PhysicsRef::Inline("{\"constraints\":[]}".into()));
        assert_eq!(
            write_se_shape(&asset),
            Err(NifWriteError::InlinePhysicsUnsupported)
        );
        assert_eq!(
            write_le_shape(&asset),
            Err(NifWriteError::InlinePhysicsUnsupported)
        );
    }
}
