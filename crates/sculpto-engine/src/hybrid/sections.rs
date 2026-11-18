//! Cut the material surface in the reference A pose before articulating or covering any joint.
mod anatomy;
mod geometry;
use super::{SkinAsset, fail, matrix32};
use crate::CommandError;
use glam::{DMat4, DVec3};
use sculpto_gltf::pack::PackWriter;
use sculpto_mesh::actor::SurfaceAttribute;
use sculpto_mesh::rig::author::parted;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn apply(
    a: &SkinAsset,
    t: &SkinAsset,
    writer: &mut PackWriter,
    mark: bool,
    arm: &[bool],
    target_attrs: Vec<SurfaceAttribute>,
) -> Result<Value, CommandError> {
    let body = &a.pool.triangles[..a.pool.parts[0].triangles];
    let layout = anatomy::Layout::new(a, body)?;
    let template =
        parted::cut(&a.pool.vertices, body, layout.owners(a), layout.parts.len()).map_err(fail)?;
    let attributes = |s: &SkinAsset| {
        s.actor
            .surface_attributes(s.mesh_node, 0)
            .map_err(crate::export_glb::actor_error)
    };
    let source = geometry::Sources {
        a,
        target: t,
        parts: &layout.parts,
        source_attrs: attributes(a)?,
        target_attrs,
    };
    let skin = source.materialize(&template.skin, None)?;
    let membrane = source.materialize(&template.membrane, None)?;
    if skin.outer.iter().flatten().any(|&v| {
        skin.rows[v as usize]
            .iter()
            .any(|&(j, w)| w > 0. && arm.get(usize::from(j)) == Some(&true))
    }) {
        return Err(fail("unmarked trunk has a residual shoulder/arm influence"));
    }
    let material = t
        .actor
        .document()
        .meshes()
        .nth(t.mesh)
        .and_then(|m| m.primitives().next())
        .and_then(|p| p.material().index());
    writer
        .flexible_surface(t.mesh, 0, &skin, &membrane, mark)
        .map_err(fail)?;
    let interior = writer.core_material().map_err(fail)?;
    let sphere = writer.joint_core_mesh(interior).map_err(fail)?;
    let mut report = Vec::new();
    let mut cores = Vec::new();
    let mut core = |writer: &mut PackWriter, slot: usize, depth: f64| -> Result<(), CommandError> {
        let role = t.names[slot]
            .rsplit(':')
            .next()
            .ok_or_else(|| fail("unnamed articulation"))?;
        let radius = depth * 0.5;
        let w = t.world(slot);
        let local = w.inverse()
            * DMat4::from_translation(w.w_axis.truncate())
            * DMat4::from_scale(DVec3::splat(radius));
        writer
            .joint_core(
                sphere,
                t.slots[slot],
                &format!("Joint core {role}"),
                matrix32(local)?,
            )
            .map_err(fail)?;
        cores.push(json!({"joint":role,"radius_m":radius,"skin_depth_A_m":depth}));
        Ok(())
    };
    for (i, (part, cut)) in layout.parts.iter().zip(&template.pieces).enumerate() {
        if cut.faces.is_empty() || cut.rims.is_empty() {
            return Err(fail(format!(
                "{} has no closed cut ends or no rigid exterior",
                part.name
            )));
        }
        let geom = source.materialize(cut, Some(i))?;
        let name = format!("Rigid {}", part.name);
        writer
            .cut_piece(&geom, material, interior, t.slots[part.slot], &name)
            .map_err(fail)?;
        let mut hash = Sha256::new();
        for p in &geom.positions {
            for v in p {
                hash.update(v.to_le_bytes());
            }
        }
        for f in geom.outer.iter().chain(&geom.caps) {
            for i in f {
                hash.update(i.to_le_bytes());
            }
        }
        let axis = (part.tip - part.root).normalize();
        let mut axial = [f64::INFINITY, f64::NEG_INFINITY];
        for s in &cut.sites {
            let z = (s.point(&a.pool.vertices) - part.root).dot(axis);
            axial = [axial[0].min(z), axial[1].max(z)];
        }
        report.push(json!({"joint":part.name,"rigid_node":name,"compact_vertices":geom.positions.len(),"outer_faces":geom.outer.len(),
            "cap_faces":geom.caps.len(),"closed_loops":cut.rims.len(),"length_m":part.root.distance(part.tip),"extent_along_axis_m":axial,
            "rigid_geometry_sha256":crate::export_glb::hex(&hash.finalize())}));
        core(writer, part.slot, part.depth)?;
    }
    for &(slot, _, depth) in &layout.ankles {
        core(writer, slot, depth)?;
    }
    Ok(
        json!({"source_A_faces":body.len(),"remaining_skin_faces":skin.outer.len(),"remaining_skin_vertices":skin.positions.len(),
        "transition_faces":membrane.outer.len(),"membrane_rest_area_A_m2":template.membrane.faces.iter().map(|f| {
            let [p,q,r]=f.map(|v|template.membrane.sites[v as usize].point(&a.pool.vertices));
            0.5*(q-p).cross(r-p).length()
        }).sum::<f64>(),"marked_transitions":mark,"segments":report,"cores":cores}),
    )
}
