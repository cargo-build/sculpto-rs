//! Ordered block lists and the one NIF container tail both Skyrim writers use.
//!
//! A file's block type table, block size array and string table describe the
//! file, not one target's blocks: Skyrim SE and LE differ only in the BS stream
//! version (100 and 83). One encoder therefore writes both, so a single-shape
//! writer and a bundle writer cannot disagree with their own headers about the
//! block count, the type table order or the string table.
//!
//! [`BoneUnion`] is the other bundle-level rule both targets share: each bone
//! name gets exactly one `NiNode`, in first-appearance order, and every skinned
//! shape's palette points at those same blocks.

use super::header::{NifTarget, write_header};
use super::prepared::PreparedShape;
use super::strings::StringTable;

/// One ordered block body with its type name.
pub(super) struct Block {
    /// Block type table name, e.g. `NiTriShape`.
    pub(super) type_name: &'static str,
    /// Serialized block body.
    pub(super) body: Vec<u8>,
}

/// Encode the container around already-ordered blocks.
///
/// Layout: header, block count, BS stream header, block type table, one size per
/// block, string table, then every body in block order.
pub(super) fn write_file(target: NifTarget, blocks: &[Block], strings: &StringTable) -> Vec<u8> {
    let mut type_names: Vec<&'static str> = Vec::new();
    let mut type_indices: Vec<u16> = Vec::with_capacity(blocks.len());
    for block in blocks {
        let type_index = match type_names.iter().position(|name| *name == block.type_name) {
            Some(index) => index,
            None => {
                type_names.push(block.type_name);
                type_names.len() - 1
            }
        };
        type_indices.push(type_index as u16);
    }

    let mut file = write_header(target);
    // Block count precedes BSStreamHeader in the parsed layout.
    push_u32(&mut file, blocks.len() as u32);
    // BSStreamHeader: target bsver, empty author/process/export strings.
    push_u32(&mut file, target.bs_version());
    file.push(0); // author
    file.push(0); // process script
    file.push(0); // export script
    // Block type table.
    push_u16(&mut file, type_names.len() as u16);
    for name in &type_names {
        push_u32(&mut file, name.len() as u32);
        file.extend_from_slice(name.as_bytes());
    }
    for index in &type_indices {
        push_u16(&mut file, *index);
    }
    for block in blocks {
        push_u32(&mut file, block.body.len() as u32);
    }
    // String table: index 0 is empty, then the shape, bone and physics names.
    let max_length = strings.iter().map(str::len).max().unwrap_or(0);
    push_u32(&mut file, strings.len() as u32);
    push_u32(&mut file, max_length as u32);
    for value in strings.iter() {
        push_u32(&mut file, value.len() as u32);
        file.extend_from_slice(value.as_bytes());
    }
    push_u32(&mut file, 0); // num_groups
    for block in blocks {
        file.extend_from_slice(&block.body);
    }
    file
}

/// One bone `NiNode` in a bundle's shared union.
pub(super) struct BoneNode {
    /// Bone name as authored.
    name: String,
    /// String-table index of that name.
    pub(super) name_index: i32,
    /// Block index of the bone node.
    pub(super) block: i32,
}

/// The one `NiNode` per bone name a bundle shares across its skins.
pub(super) struct BoneUnion {
    nodes: Vec<BoneNode>,
}

impl BoneUnion {
    /// Assign one block per distinct bone name, in first-appearance order.
    ///
    /// Shapes are visited in bundle order and each palette in authored order, so
    /// the union starts with the first shape's bones and appends only names no
    /// earlier shape used.
    pub(super) fn plan(
        shapes: &[PreparedShape],
        next: &mut i32,
        strings: &mut StringTable,
    ) -> Self {
        let mut nodes: Vec<BoneNode> = Vec::new();
        for plan in shapes.iter().filter_map(|shape| shape.skin.as_ref()) {
            for name in &plan.names {
                if nodes.iter().any(|bone| &bone.name == name) {
                    continue;
                }
                let block = *next;
                *next += 1;
                nodes.push(BoneNode {
                    name: name.clone(),
                    name_index: strings.intern(name) as i32,
                    block,
                });
            }
        }
        Self { nodes }
    }

    /// The block of one union bone by name.
    pub(super) fn block(&self, name: &str) -> i32 {
        self.nodes
            .iter()
            .find(|bone| bone.name == name)
            .map_or(-1, |bone| bone.block)
    }

    /// The union's bone nodes in write order.
    pub(super) fn bones(&self) -> &[BoneNode] {
        &self.nodes
    }
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
