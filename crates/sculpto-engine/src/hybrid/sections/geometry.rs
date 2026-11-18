//! Materialize a compact surface from A-pose barycentric cut sites and close each rigid end.
use super::{SkinAsset, anatomy::Part, fail};
use crate::CommandError;
use glam::{DMat4, DVec3};
use sculpto_gltf::pack::CutGeometry;
use sculpto_mesh::{
    actor::SurfaceAttribute,
    rig::author::parted::{Piece, Site},
};
use std::collections::BTreeMap;

pub(super) struct Sources<'a> {
    pub a: &'a SkinAsset,
    pub target: &'a SkinAsset,
    pub parts: &'a [Part],
    pub source_attrs: Vec<SurfaceAttribute>,
    pub target_attrs: Vec<SurfaceAttribute>,
}
fn interpolate(attr: &SurfaceAttribute, site: &Site) -> [f64; 4] {
    let mut out = [0.; 4];
    for (i, f) in site.face.into_iter().zip(site.bary) {
        for (v, x) in out.iter_mut().zip(attr.values[i as usize]) {
            *v += f * f64::from(x);
        }
    }
    out
}
fn appearance(attr: &SurfaceAttribute, site: &Site, map: DMat4) -> [f64; 4] {
    super::super::map_attribute(interpolate(attr, site), &attr.semantic, map)
}

fn row(s: &Site, target: &SkinAsset, parts: &[Part]) -> Vec<(u16, f32)> {
    if let Some(i) = s.owner {
        return vec![(parts[i].slot as u16, 1.)];
    }
    let mut result = BTreeMap::<u16, f64>::new();
    for (v, b) in s.face.into_iter().zip(s.bary) {
        for &(j, w) in &target.pool.rows[v as usize] {
            *result.entry(j).or_default() += b * f64::from(w);
        }
    }
    let mut packed: Vec<_> = result.into_iter().filter(|p| p.1 > 0.).collect();
    packed.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    packed.truncate(4);
    let sum: f64 = packed.iter().map(|p| p.1).sum();
    packed
        .into_iter()
        .map(|(i, w)| (i, (w / sum) as f32))
        .collect()
}

impl Sources<'_> {
    pub fn materialize(
        &self,
        piece: &Piece,
        rigid: Option<usize>,
    ) -> Result<CutGeometry, CommandError> {
        if self.source_attrs.len() != self.target_attrs.len()
            || self
                .source_attrs
                .iter()
                .zip(&self.target_attrs)
                .any(|(a, b)| a.semantic != b.semantic || a.width != b.width)
        {
            return Err(fail("target cut attributes differ from the A reference"));
        }
        let mut data = CutGeometry {
            outer: piece.faces.clone(),
            attributes: self
                .source_attrs
                .iter()
                .map(|a| (a.semantic.clone(), a.width, Vec::new()))
                .collect(),
            ..CutGeometry::default()
        };
        let frames: Vec<_> = self
            .parts
            .iter()
            .map(|p| self.a.world(p.slot).inverse())
            .collect();
        let to_mesh = self.target.frame.inverse();
        let anchors: Vec<_> = self
            .parts
            .iter()
            .zip(&frames)
            .map(|(p, &inv)| to_mesh * self.target.world(p.slot) * inv)
            .collect();
        for site in &piece.sites {
            let material = site.point(&self.a.pool.vertices);
            let accepted = to_mesh.transform_point3(site.point(&self.target.pool.vertices));
            let position = match rigid {
                Some(i) => frames[i].transform_point3(material),
                None => site
                    .owner
                    .map_or(accepted, |i| anchors[i].transform_point3(material)),
            };
            data.positions.push(position.as_vec3().to_array());
            if rigid.is_none() {
                data.band_uv
                    .extend([0.125 + 2.5 * f32::from(site.band_side), 0.5]);
                data.rows.push(row(site, self.target, self.parts));
            }
            for ((dst, base), current) in data
                .attributes
                .iter_mut()
                .zip(&self.source_attrs)
                .zip(&self.target_attrs)
            {
                let mut value = match rigid {
                    Some(i) => appearance(base, site, frames[i] * self.a.frame),
                    None => match site.owner {
                        Some(i) => appearance(base, site, anchors[i] * self.a.frame),
                        None => interpolate(current, site),
                    },
                };
                if base.semantic == "NORMAL" || base.semantic == "TANGENT" {
                    let n = DVec3::new(value[0], value[1], value[2]).normalize_or_zero();
                    value[..3].copy_from_slice(&n.to_array());
                    if base.semantic == "TANGENT" {
                        value[3] = if value[3] < 0. { -1. } else { 1. };
                    }
                }
                dst.2.extend(value[..base.width].iter().map(|&v| v as f32));
            }
        }
        for ring in &piece.rims {
            seal(&mut data, ring)?;
        }
        Ok(data)
    }
}

fn seal(data: &mut CutGeometry, ring: &[u32]) -> Result<(), CommandError> {
    let vertices: Vec<_> = ring
        .iter()
        .map(|&i| DVec3::from_array(data.positions[i as usize].map(f64::from)))
        .collect();
    let mid: DVec3 = vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    let normal = vertices
        .iter()
        .zip(vertices.iter().cycle().skip(1))
        .take(vertices.len())
        .map(|(a, b)| (*a - mid).cross(*b - mid))
        .sum::<DVec3>()
        .try_normalize()
        .ok_or_else(|| fail("cut end has no area"))?;
    let start = data.positions.len() as u32;
    // Duplicate the rim only for shading. Surface and cap coordinates still coincide bit-for-bit.
    for &v in ring {
        data.positions.push(data.positions[v as usize]);
    }
    let centre = data.positions.len() as u32;
    data.positions.push(mid.as_vec3().to_array());
    for (name, width, values) in &mut data.attributes {
        let mut mean = vec![0_f32; *width];
        for &v in ring {
            let from = v as usize * *width;
            let mut x = values[from..from + *width].to_vec();
            if name == "NORMAL" {
                x.copy_from_slice(&normal.as_vec3().to_array());
            }
            if name == "TANGENT" {
                x = normal
                    .any_orthonormal_vector()
                    .as_vec3()
                    .extend(1.)
                    .to_array()
                    .to_vec();
            }
            for (m, s) in mean.iter_mut().zip(&x) {
                *m += *s / ring.len() as f32;
            }
            values.extend(x);
        }
        if name == "NORMAL" {
            mean.copy_from_slice(&normal.as_vec3().to_array());
        }
        if name == "TANGENT" {
            mean = normal
                .any_orthonormal_vector()
                .as_vec3()
                .extend(1.)
                .to_array()
                .to_vec();
        }
        values.extend(mean);
    }
    for i in 0..ring.len() as u32 {
        data.caps
            .push([start + i, start + (i + 1) % ring.len() as u32, centre]);
    }
    Ok(())
}
