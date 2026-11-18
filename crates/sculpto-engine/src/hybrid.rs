//! Preserve an A-reference material cuff between rigid limb sections and transplant registered soft helpers.
mod pelvis;
mod sections;

use crate::{
    CommandError,
    actor_io::{Pool, read_pool},
    export_glb::{actor_error, hex, refused},
};
use glam::DMat4;
use sculpto_gltf::pack::PackWriter;
use sculpto_mesh::actor::{Actor, Pose};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn fail(c: impl ToString) -> CommandError {
    refused("actor_hybrid", c.to_string())
}

struct SkinAsset {
    actor: Actor,
    mesh_node: usize,
    mesh: usize,
    parts: usize,
    skin: usize,
    slots: Vec<usize>,
    names: Vec<String>,
    worlds: Vec<DMat4>,
    frame: DMat4,
    pool: Pool,
}
impl SkinAsset {
    fn read(bytes: &[u8]) -> Result<Self, CommandError> {
        let actor = Actor::from_glb(bytes).map_err(actor_error)?;
        let (node, name) = actor.skinned_mesh_node().map_err(actor_error)?;
        let slots = actor.skin_joints().map_err(actor_error)?;
        if slots.len() + 10 > u16::MAX as usize {
            return Err(fail("palette exceeds four-slot u16 encoding"));
        }
        let names = slots
            .iter()
            .map(|&i| {
                actor
                    .node_name(i)
                    .map(str::to_string)
                    .ok_or_else(|| fail("unnamed joint"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let pool = read_pool(&actor, node, &name, &names)?;
        let parts = pool.parts.len();
        let frame = actor.shared_skin_frame(node).map_err(actor_error)?;
        let worlds = actor.skeleton_worlds(Pose::Rest).map_err(actor_error)?;
        let selected = actor
            .document()
            .nodes()
            .nth(node)
            .ok_or_else(|| fail("missing skin node"))?;
        let mesh = selected
            .mesh()
            .ok_or_else(|| fail("missing selected mesh"))?
            .index();
        let skin = selected
            .skin()
            .ok_or_else(|| fail("missing selected skin"))?
            .index();
        Ok(Self {
            actor,
            mesh_node: node,
            mesh,
            parts,
            skin,
            slots,
            names,
            worlds,
            frame,
            pool,
        })
    }
    fn slot(&self, name: &str) -> Result<usize, CommandError> {
        let mut found = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.as_str() == name);
        match (found.next(), found.next()) {
            (Some((i, _)), None) => Ok(i),
            _ => Err(fail(format!("expected one joint named {name}"))),
        }
    }
    fn mix_slot(&self, name: &str) -> Result<usize, CommandError> {
        let mut found = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.rsplit(':').next() == Some(name));
        match (found.next(), found.next()) {
            (Some((i, _)), None) => Ok(i),
            _ => Err(fail(format!("expected one Mixamo {name}"))),
        }
    }
    fn world(&self, slot: usize) -> DMat4 {
        self.worlds[self.slots[slot]]
    }
    fn matching_topology(&self, other: &Self) -> Result<(), CommandError> {
        if self.parts != other.parts
            || self.pool.vertices.len() != other.pool.vertices.len()
            || self.pool.triangles != other.pool.triangles
            || self.pool.partitions != other.pool.partitions
        {
            return Err(fail(
                "body/layer vertex count, triangle IDs or partition order differ from the A reference",
            ));
        }
        Ok(())
    }
}
fn matrix32(m: DMat4) -> Result<[f32; 16], CommandError> {
    let a = m.to_cols_array().map(|v| v as f32);
    if !m.inverse().is_finite() || a.iter().any(|v| !v.is_finite()) {
        return Err(fail("invalid output frame"));
    }
    Ok(a)
}

/// The remaining body and underlayers follow the trunk. Only explicit joint bridges may regain arm mass. @req DECL0013
fn isolate_trunk(target: &mut SkinAsset) -> Result<(Vec<bool>, usize), CommandError> {
    let parents = target.actor.node_parents();
    let roots = [
        target.slots[target.mix_slot("LeftShoulder")?],
        target.slots[target.mix_slot("RightShoulder")?],
    ];
    let spine = target.mix_slot("Spine2")? as u16;
    let arm: Vec<_> = target
        .slots
        .iter()
        .map(|&joint| {
            std::iter::successors(Some(joint), |&n| parents[n]).any(|n| roots.contains(&n))
        })
        .collect();
    let changed = target
        .pool
        .rows
        .iter_mut()
        .map(|row| usize::from(redirect_arm_mass(row, &arm, spine)))
        .sum();
    Ok((arm, changed))
}

/// Donor helpers follow the original palette and retain their share. @req DECL0013
fn redirect_arm_mass(row: &mut Vec<(u16, f32)>, arm: &[bool], spine: u16) -> bool {
    let in_arm = |slot: u16| arm.get(usize::from(slot)) == Some(&true);
    let mass: f64 = row
        .iter()
        .filter(|p| in_arm(p.0))
        .map(|p| f64::from(p.1))
        .sum();
    if mass == 0.0 {
        return false;
    }
    row.retain(|p| !in_arm(p.0));
    if let Some((_, current)) = row.iter_mut().find(|p| p.0 == spine) {
        *current = (f64::from(*current) + mass) as f32;
    } else {
        row.push((spine, mass as f32));
    }
    true
}

/// Cut the A-pose limbs, palms and digits; preserve the tissue around each joint. @req DECL0013
pub fn prepare(
    bytes: &[u8],
    a_reference: &[u8],
    native: Option<&[u8]>,
    mark_transitions: bool,
) -> Result<(Vec<u8>, Value), CommandError> {
    let reference = SkinAsset::read(a_reference)?;
    let mut target = SkinAsset::read(bytes)?;
    reference.matching_topology(&target)?;
    if reference.names != target.names {
        return Err(fail("target palette differs from its A reference"));
    }
    for name in target.actor.document().nodes().filter_map(|n| n.name()) {
        if name.starts_with("Rigid ") || name.starts_with("Joint core ") {
            return Err(fail("input is already articulated"));
        }
    }
    for (a, t) in reference.slots.iter().zip(&target.slots) {
        let pa = reference.actor.node_parents()[*a].and_then(|i| reference.actor.node_name(i));
        let pt = target.actor.node_parents()[*t].and_then(|i| target.actor.node_name(i));
        if pa != pt {
            return Err(fail("target joint parents differ from the A reference"));
        }
    }
    let mut writer = PackWriter::from_glb(bytes).map_err(fail)?;
    let donor = native.map(SkinAsset::read).transpose()?;
    let graft = donor
        .as_ref()
        .map(|d| pelvis::transplant(&mut target, &reference, d, &mut writer))
        .transpose()?;
    let (arm, isolated) = isolate_trunk(&mut target)?;
    writer
        .set_shared_skin(target.mesh, target.parts, &target.pool.rows)
        .map_err(fail)?;
    let section = sections::apply(&reference, &target, &mut writer, mark_transitions, &arm)?;
    let encoded = writer.finish().map_err(fail)?;
    let written = Actor::from_glb(&encoded).map_err(actor_error)?;
    let final_joints = written.skin_joints().map_err(actor_error)?.len();
    let expected = target.slots.len() + if donor.is_some() { 10 } else { 0 };
    if final_joints != expected {
        return Err(fail("unexpected emitted palette width"));
    }
    let report = json!({"command":"actor hybrid","input_sha256":hex(&Sha256::digest(bytes)),
        "reference_sha256":hex(&Sha256::digest(a_reference)),"donor_sha256":native.map(|b|hex(&Sha256::digest(b))),
        "output_sha256":hex(&Sha256::digest(&encoded)),"original_joints":target.slots.len(),"output_joints":final_joints,
        "sections":section,"pelvis":graft,"original_pool_rows_with_arm_mass_redirected":isolated});
    Ok((encoded, report))
}

#[cfg(test)]
mod tests {
    use super::redirect_arm_mass;

    #[test]
    fn arm_motion_cannot_pull_trunk_or_underlayers_after_redirect() {
        let arm = [false, false, true, true];
        let mut trunk = vec![(2, 0.125), (1, 0.25), (3, 0.125), (4, 0.5)];
        assert!(redirect_arm_mass(&mut trunk, &arm, 1));
        assert_eq!(trunk, [(1, 0.5), (4, 0.5)]);
        assert!(!redirect_arm_mass(&mut trunk, &arm, 1));
        let mut bra = vec![(0, 0.25), (2, 0.75)];
        assert!(redirect_arm_mass(&mut bra, &arm, 1));
        assert_eq!(bra, [(0, 0.25), (1, 0.75)]);
    }
}
