//! Reconstruct axial material before the T-bake can pull it with the old arm weights.
use super::{ArmIsolation, SkinAsset, fail, map_attribute};
use crate::{CommandError, export_glb::actor_error};
use glam::{DMat4, DVec3};
use sculpto_gltf::pack::PackWriter;
use sculpto_mesh::actor::SurfaceAttribute;

/// The A material follows its remaining owners across the bind change. @req DECL0013
fn axial_frame(
    row: &[(u16, f32)],
    relative: &[DMat4],
    iso: &ArmIsolation,
) -> Result<DMat4, CommandError> {
    let mut blend = DMat4::ZERO;
    let mut total = 0.0;
    for &(slot, w) in row {
        let slot = usize::from(if iso.slots[usize::from(slot)] {
            iso.spine
        } else {
            slot
        });
        let weight = f64::from(w);
        blend += relative[slot] * weight;
        total += weight;
    }
    if total <= 0.0 || !total.is_finite() {
        return Err(fail("empty axial bind row"));
    }
    let blend = blend * (1. / total);
    if !blend.inverse().is_finite() {
        return Err(fail("singular axial bind change"));
    }
    Ok(blend)
}

fn shading(
    a: &SkinAsset,
    t: &SkinAsset,
    part: usize,
    frames: &[Option<DMat4>],
) -> Result<Vec<SurfaceAttribute>, CommandError> {
    let mut attrs = t
        .actor
        .surface_attributes(t.mesh_node, part)
        .map_err(actor_error)?;
    let source = a
        .actor
        .surface_attributes(a.mesh_node, part)
        .map_err(actor_error)?;
    for attr in attrs
        .iter_mut()
        .filter(|v| matches!(v.semantic.as_str(), "NORMAL" | "TANGENT"))
    {
        let old = source
            .iter()
            .find(|v| v.semantic == attr.semantic && v.width == attr.width)
            .ok_or_else(|| {
                fail(format!(
                    "A reference lacks {} for part {part}",
                    attr.semantic
                ))
            })?;
        if attr.values.len() != frames.len() || old.values.len() != frames.len() {
            return Err(fail("bind shading pool differs from reference"));
        }
        for ((v, p), frame) in attr.values.iter_mut().zip(&old.values).zip(frames) {
            if let Some(m) = frame {
                *v = map_attribute(p.map(f64::from), &attr.semantic, *m).map(|n| n as f32);
            }
        }
    }
    Ok(attrs)
}

pub(super) fn restore(
    a: &SkinAsset,
    t: &mut SkinAsset,
    iso: &ArmIsolation,
    identical: bool,
    writer: &mut PackWriter,
) -> Result<Vec<SurfaceAttribute>, CommandError> {
    if identical {
        return t
            .actor
            .surface_attributes(t.mesh_node, 0)
            .map_err(actor_error);
    }
    let relative: Vec<_> = (0..a.slots.len())
        .map(|j| t.world(j) * a.world(j).inverse())
        .collect();
    let to_mesh = t.frame.inverse();
    let mut frames = vec![None; a.pool.vertices.len()];
    let mut encoded = t
        .actor
        .bind_positions(t.mesh_node, 0)
        .map_err(actor_error)?;
    if encoded.len() != frames.len() {
        return Err(fail("original bind positions differ from sampled pool"));
    }
    for (i, row) in a.pool.rows.iter().enumerate() {
        if !iso.redirected[i]
            && !row
                .iter()
                .any(|(slot, weight)| *weight > 0. && iso.slots[usize::from(*slot)])
        {
            continue;
        }
        let map = axial_frame(row, &relative, iso)?;
        let world = map.transform_point3(DVec3::from_array(a.pool.vertices[i]));
        t.pool.vertices[i] = world.to_array();
        encoded[i] = to_mesh.transform_point3(world).as_vec3().to_array();
        frames[i] = Some(to_mesh * map * a.frame);
    }
    let mut pos = None;
    let mut offset = t.pool.parts[0].triangles;
    for part in 1..t.parts {
        let end = offset + t.pool.parts[part].triangles;
        let affected = t.pool.triangles[offset..end]
            .iter()
            .flatten()
            .any(|&id| frames[id as usize].is_some());
        offset = end;
        if !affected {
            continue;
        }
        let acc = match pos {
            Some(i) => i,
            None => {
                let i = writer.morph_vectors(&encoded).map_err(fail)?;
                pos = Some(i);
                i
            }
        };
        writer
            .set_attribute(t.mesh, part, "POSITION", acc)
            .map_err(fail)?;
        for attr in shading(a, t, part, &frames)?
            .into_iter()
            .filter(|v| matches!(v.semantic.as_str(), "NORMAL" | "TANGENT"))
        {
            writer
                .set_bind_directions(t.mesh, part, &attr.semantic, &attr.values)
                .map_err(fail)?;
        }
    }
    shading(a, t, 0, &frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chest_basis_follows_spine_even_when_a_previous_bind_was_raised() {
        let axial = DMat4::from_rotation_y(0.25);
        let arm = DMat4::from_rotation_z(1.1);
        let iso = ArmIsolation {
            slots: vec![false, true],
            redirected: vec![],
            spine: 0,
        };
        let got = axial_frame(&[(0, 0.25), (1, 0.75)], &[axial, arm], &iso).unwrap();
        assert!(got.abs_diff_eq(axial, 1e-12));
    }
}
