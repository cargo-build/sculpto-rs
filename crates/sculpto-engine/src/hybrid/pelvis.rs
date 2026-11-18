//! Copy donor soft-tissue influence from its existing canonical parent budget.
use super::{SkinAsset, fail};
use crate::{CommandError, actor_io::f32_of};
use glam::DVec3;
use sculpto_gltf::pack::{HelperJoint, PackWriter};
use serde_json::{Value, json};

struct Transfer {
    source: usize,
    destination: u16,
    owner: u16,
}

/// Preserve each parent group's combined weight, including at material boundaries. @req DECL0013
fn split(
    row: &mut Vec<(u16, f32)>,
    donor: &[(u16, f32)],
    slots: &[Transfer],
    owners: &[u16],
) -> Result<(bool, f64), CommandError> {
    let mut changed = false;
    let mut largest = 0_f64;
    for &owner in owners {
        let mut desired = Vec::new();
        for s in slots.iter().filter(|s| s.owner == owner) {
            if let Some(&(_, w)) = donor.iter().find(|(i, _)| usize::from(*i) == s.source) {
                desired.push((s.destination, f64::from(w)));
            }
        }
        if desired.is_empty() {
            continue;
        }
        let requested: f64 = desired.iter().map(|p| p.1).sum();
        let available = row
            .iter()
            .find(|p| p.0 == owner)
            .map_or(0., |p| f64::from(p.1));
        largest = largest.max((requested - available).max(0.));
        if available <= 0. {
            continue;
        }
        let used = available.min(requested);
        let factor = used / requested;
        row.retain(|p| p.0 != owner);
        if available > used {
            row.push((owner, (available - used) as f32));
        }
        row.extend(desired.into_iter().map(|(i, w)| (i, (w * factor) as f32)));
        changed = true;
    }
    if row.len() > 4 {
        return Err(fail(
            "native transfer would discard an unrelated influence at the four-slot cap",
        ));
    }
    Ok((changed, largest))
}

pub(super) fn transplant(
    target: &mut SkinAsset,
    reference: &SkinAsset,
    donor: &SkinAsset,
    writer: &mut PackWriter,
) -> Result<Value, CommandError> {
    reference.matching_topology(donor)?;
    let registration = reference
        .pool
        .vertices
        .iter()
        .zip(&donor.pool.vertices)
        .map(|(&a, &b)| DVec3::from_array(a).distance(DVec3::from_array(b)))
        .fold(0., f64::max);
    if registration > 5e-5 {
        return Err(fail(format!(
            "donor A-bind registration differs by {registration} m"
        )));
    }
    let mut helpers = Vec::new();
    let mut transfers = Vec::new();
    let mut records = Vec::new();
    let mut binds: Vec<_> = target
        .actor
        .skin_inverse_binds(target.mesh_node)
        .map_err(crate::export_glb::actor_error)?
        .into_iter()
        .map(f32_of)
        .collect();
    let hips = target.mix_slot("Hips")?;
    let mut owners = vec![hips as u16];
    for (prefix, side) in [("L", "Left"), ("R", "Right")] {
        let upper = target.mix_slot(&format!("{side}UpLeg"))?;
        owners.push(upper as u16);
        for tag in ["Butt", "FrontThigh", "RearThigh"] {
            let pre = format!("NPC {prefix} Pre{tag}");
            let leaf = format!("NPC {prefix} {tag}");
            let ps = donor.slot(&pre)?;
            let ls = donor.slot(&leaf)?;
            if donor.actor.node_parents()[donor.slots[ls]] != Some(donor.slots[ps]) {
                return Err(fail(format!("unexpected donor hierarchy for {leaf}")));
            }
            let butt = tag == "Butt";
            if butt && !donor.world(ps).abs_diff_eq(donor.world(ls), 1e-6) {
                return Err(fail(format!(
                    "cannot merge distinct bind frames {pre} and {leaf}"
                )));
            }
            let owner = if butt { hips } else { upper };
            let mut parent = target.slots[owner];
            let mut parent_world = target.world(owner);
            let to_target = parent_world * reference.world(owner).inverse();
            for (s, name, weights) in [
                (ps, pre.as_str(), if butt { Some(ls) } else { None }),
                (ls, leaf.as_str(), Some(ls)),
            ]
            .into_iter()
            .take(if butt { 1 } else { 2 })
            {
                if target
                    .actor
                    .document()
                    .nodes()
                    .any(|node| node.name() == Some(name))
                {
                    return Err(fail(format!("target already has {name}")));
                }
                let world = to_target * donor.world(s);
                let local = parent_world.inverse() * world;
                let (scale, rot, tr) = local.to_scale_rotation_translation();
                let recomposed = glam::DMat4::from_scale_rotation_translation(scale, rot, tr);
                if !recomposed.is_finite() || !recomposed.abs_diff_eq(local, 2e-6) {
                    return Err(fail(format!(
                        "helper {name} cannot retain its bind frame as TRS"
                    )));
                }
                let next_node = target.actor.document().nodes().len() + helpers.len();
                let slot = target.slots.len() + helpers.len();
                helpers.push(HelperJoint {
                    name: name.to_owned(),
                    parent,
                    translation: tr.as_vec3().to_array(),
                    rotation: rot.as_quat().to_array(),
                    scale: scale.as_vec3().to_array(),
                });
                binds.push(f32_of(world.inverse() * target.frame));
                if let Some(source) = weights {
                    transfers.push(Transfer {
                        source,
                        destination: slot as u16,
                        owner: owner as u16,
                    });
                }
                records.push(json!({"joint":name,"parent":parent,"slot":slot,"donor_A_world_m":donor.world(s).w_axis.truncate().to_array(),"target_bind_m":world.w_axis.truncate().to_array()}));
                parent = next_node;
                parent_world = world;
            }
        }
    }
    if donor.pool.rows.iter().flatten().any(|(i, w)| {
        *w > 0.
            && donor.names[usize::from(*i)].contains(" Pre")
            && helpers
                .iter()
                .any(|h| h.name == donor.names[usize::from(*i)])
    }) {
        return Err(fail(
            "donor pre-joints have independent positive weights requiring an explicit mapping",
        ));
    }
    writer
        .extend_skin(target.skin, &helpers, &binds)
        .map_err(fail)?;
    let mut changed = 0;
    let mut capped = 0;
    let mut max_shortage = 0_f64;
    for (row, source) in target.pool.rows.iter_mut().zip(&donor.pool.rows) {
        let (touched, missing) = split(row, source, &transfers, &owners)?;
        changed += usize::from(touched);
        capped += usize::from(missing > 1e-7);
        max_shortage = max_shortage.max(missing);
    }
    Ok(
        json!({"registration_max_m":registration,"changed_rows":changed,"rows_limited_to_parent_mass":capped,"max_limited_mass":max_shortage,"helpers":records}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_budget_limits_a_donor_without_moving_other_owners() {
        let mut row = vec![(0, 0.125), (1, 0.875)];
        let src = [(3, 0.25), (4, 0.125)];
        let defs = [
            Transfer {
                source: 3,
                destination: 7,
                owner: 0,
            },
            Transfer {
                source: 4,
                destination: 8,
                owner: 0,
            },
        ];
        let (_, missing) = split(&mut row, &src, &defs, &[0]).unwrap();
        assert_eq!(missing, 0.25);
        assert_eq!(row[0], (1, 0.875));
        assert!((row[1].1 + row[2].1 - 0.125).abs() < 1e-7);
    }
}
