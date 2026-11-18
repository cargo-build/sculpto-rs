//! Clip a named material partition once on a rest surface, keeping a flexible strip between rigid pieces.
use super::{Candidate, Surface, seams::SeamWeights};
use crate::surface_util::coincident_groups;
use glam::DVec3;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// A location on one original material triangle. A cuff approaches one rigid owner at its cut rim.
#[derive(Clone)]
pub struct Site {
    pub face: [u32; 3],
    pub bary: [f64; 3],
    pub shares: Vec<(Option<usize>, f64)>,
    pub band_distance: f32,
}
impl Site {
    pub fn point(&self, points: &[[f64; 3]]) -> DVec3 {
        self.face
            .into_iter()
            .zip(self.bary)
            .map(|(i, w)| w * DVec3::from_array(points[i as usize]))
            .sum()
    }
}
/// Indexed outer material and newly cut, ordered loops. Caps are inside the joint sleeve.
#[derive(Default)]
pub struct Piece {
    pub sites: Vec<Site>,
    pub faces: Vec<[u32; 3]>,
    pub transitions: Vec<[u32; 3]>,
    pub rims: Vec<Vec<u32>>,
}
pub struct Partition {
    pub skin: Piece,
    pub pieces: Vec<Piece>,
}

#[derive(Default)]
struct Build {
    part: Piece,
    index: BTreeMap<[(u32, i64); 3], u32>,
    cut: Vec<(u32, u32)>,
}
struct FaceField<'a> {
    regions: &'a [(usize, DVec3)],
    tissue: DVec3,
    neighbours: [(Option<usize>, Option<usize>); 3],
}
impl FaceField<'_> {
    fn shares(&self, b: DVec3) -> (Vec<(Option<usize>, f64)>, f32) {
        let (owner, distance) = self
            .regions
            .iter()
            .map(|(r, f)| (Some(*r), 1. + b.dot(*f)))
            .fold((None, 1. + b.dot(self.tissue)), |a, c| {
                if c.1 > a.1 { c } else { a }
            });
        let t = distance.clamp(0., 1.);
        if t >= 1. - 1e-9 {
            return (vec![(owner, 1.)], 1.);
        }
        let gain = t * t * (3. - 2. * t);
        let mut weights = BTreeMap::new();
        for ((left, right), v) in self.neighbours.iter().zip(b.to_array()) {
            for which in [*left, *right] {
                *weights.entry(which).or_insert(0.) += 0.5 * v * (1. - gain);
            }
        }
        *weights.entry(owner).or_insert(0.) += gain;
        (
            weights.into_iter().filter(|(_, w)| *w > 0.).collect(),
            t as f32,
        )
    }
}
impl Build {
    fn site(&mut self, s: Site) -> Result<u32, String> {
        let mut key = [(u32::MAX, 0); 3];
        for (i, (&v, w)) in s.face.iter().zip(s.bary).enumerate() {
            let units = (w * 1e9).round() as i64;
            if units > 0 {
                key[i] = (v, units);
            }
        }
        key.sort_unstable();
        if let Some(&i) = self.index.get(&key) {
            return Ok(i);
        }
        let i = u32::try_from(self.part.sites.len()).map_err(|_| "too many cut vertices")?;
        self.index.insert(key, i);
        self.part.sites.push(s);
        Ok(i)
    }
    fn polygon(
        &mut self,
        face: [u32; 3],
        poly: &[DVec3],
        field: &FaceField<'_>,
        rigid: Option<usize>,
        marked: bool,
    ) -> Result<(), String> {
        if poly.len() < 3 {
            return Ok(());
        }
        let mut ids = Vec::with_capacity(poly.len());
        for &b in poly {
            let (shares, band_distance) = field.shares(b);
            let i = self.site(Site {
                face,
                bary: b.to_array(),
                shares,
                band_distance,
            })?;
            ids.push(i);
        }
        if let Some(r) = rigid {
            let field = field
                .regions
                .iter()
                .find(|f| f.0 == r)
                .ok_or("missing cut field")?
                .1;
            for (i, j) in (0..ids.len()).map(|i| (i, (i + 1) % ids.len())) {
                if ids[i] != ids[j]
                    && poly[i].dot(field).abs() < 1e-8
                    && poly[j].dot(field).abs() < 1e-8
                {
                    self.cut.push((ids[j], ids[i]));
                }
            }
        }
        for i in 1..ids.len() - 1 {
            let tri = [ids[0], ids[i], ids[i + 1]];
            if tri[0] != tri[1] && tri[0] != tri[2] && tri[1] != tri[2] {
                if marked {
                    self.part.transitions.push(tri);
                } else {
                    self.part.faces.push(tri);
                }
            }
        }
        Ok(())
    }
    fn finish(mut self, p: &[[f64; 3]]) -> Result<Piece, String> {
        let rim: Vec<_> = self
            .cut
            .iter()
            .flat_map(|&(a, b)| [a, b])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let xyz: Vec<_> = rim
            .iter()
            .map(|&i| self.part.sites[i as usize].point(p).as_vec3().to_array())
            .collect();
        let mut alias: BTreeMap<u32, u32> = rim.iter().map(|&i| (i, i)).collect();
        for group in coincident_groups(&xyz, 1e-6) {
            for &i in &group {
                alias.insert(rim[i], rim[group[0]]);
            }
        }
        let mut ring: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        let mut directed = BTreeSet::new();
        for (a, b) in self.cut {
            let (a, b) = (alias[&a], alias[&b]);
            if a == b {
                continue;
            }
            ring.entry(a).or_default().insert(b);
            ring.entry(b).or_default().insert(a);
            directed.insert((a, b));
        }
        if ring.values().any(|v| v.len() != 2) {
            return Err("the material cut does not form simple closed rings".into());
        }
        let mut todo: BTreeSet<_> = ring.keys().copied().collect();
        while let Some(&start) = todo.first() {
            let mut ids = vec![start];
            todo.remove(&start);
            let mut prev = start;
            let mut cur = *ring[&start]
                .iter()
                .find(|&&q| directed.contains(&(start, q)))
                .or_else(|| ring[&start].first())
                .ok_or("empty rim")?;
            while cur != start {
                if !todo.remove(&cur) {
                    return Err("revisited material rim".into());
                }
                ids.push(cur);
                let next = *ring[&cur]
                    .iter()
                    .find(|&&n| n != prev)
                    .ok_or("incomplete rim")?;
                prev = cur;
                cur = next;
            }
            if ids.len() < 3 {
                return Err("degenerate material rim".into());
            }
            self.part.rims.push(ids);
        }
        Ok(self.part)
    }
}
fn clip(poly: &[DVec3], field: DVec3) -> Vec<DVec3> {
    if poly.is_empty() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut prev = poly[poly.len() - 1];
    let mut dp = prev.dot(field);
    for &v in poly {
        let dv = v.dot(field);
        if (dp >= 0.) != (dv >= 0.) {
            out.push(prev.lerp(v, dp / (dp - dv)));
        }
        if dv >= 0. {
            out.push(v);
        }
        prev = v;
        dp = dv;
    }
    out
}

/// Use surface distance to the anatomical interfaces to allocate a finite-width connecting skin. @req DECL0013
pub fn cut(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    mut labels: Vec<Option<usize>>,
    widths: &[f64],
    count: usize,
) -> Result<Partition, String> {
    let n = positions.len();
    if n == 0
        || labels.len() != n
        || widths.len() != n
        || widths.iter().any(|v| !v.is_finite() || *v <= 0.)
        || labels.iter().flatten().any(|&i| i >= count)
        || triangles.iter().flatten().any(|&v| v as usize >= n)
    {
        return Err("invalid material segmentation inputs".into());
    }
    let p: Vec<_> = positions.iter().map(|&v| DVec3::from_array(v)).collect();
    let mut used = vec![false; n];
    for &i in triangles.iter().flatten() {
        used[i as usize] = true;
    }
    let seams = SeamWeights::find(positions, triangles, &used);
    for (a, b) in seams.links() {
        labels[b as usize] = labels[a as usize];
    }
    let surface = Surface::build(&p, triangles, seams.links());
    let mut across = labels.clone();
    let mut d = vec![4_f64; n];
    let mut heap = BinaryHeap::new();
    for a in 0..n {
        for k in surface.neighbours(a) {
            let b = surface.neighbours[k] as usize;
            if labels[a] != labels[b] {
                let len = surface.lengths[k] / (widths[a] + widths[b]);
                if len < d[a] {
                    d[a] = len;
                    across[a] = labels[b];
                    heap.push(Candidate {
                        distance: len,
                        vertex: a as u32,
                    });
                }
            }
        }
    }
    while let Some(Candidate { distance, vertex }) = heap.pop() {
        let a = vertex as usize;
        if distance > d[a] {
            continue;
        }
        for k in surface.neighbours(a) {
            let b = surface.neighbours[k] as usize;
            let v = distance + 2. * surface.lengths[k] / (widths[a] + widths[b]);
            if labels[a] == labels[b] && v < d[b] {
                d[b] = v;
                across[b] = across[a];
                heap.push(Candidate {
                    distance: v,
                    vertex: b as u32,
                });
            }
        }
    }
    let mut builders: Vec<Build> = (0..count).map(|_| Build::default()).collect();
    let mut skin = Build::default();
    let original = [DVec3::X, DVec3::Y, DVec3::Z];
    for &face in triangles {
        let owners = face.map(|v| labels[v as usize]);
        let mut unique = Vec::new();
        for r in owners.into_iter().flatten() {
            if !unique.contains(&r) {
                unique.push(r);
            }
        }
        let fields: Vec<_> = unique
            .iter()
            .map(|&r| {
                (
                    r,
                    DVec3::from_array(face.map(|v| {
                        let v = v as usize;
                        let s = if labels[v] == Some(r) { d[v] } else { -d[v] };
                        s - 1.
                    })),
                )
            })
            .collect();
        let tissue = DVec3::from_array(face.map(|v| {
            let i = v as usize;
            let x = if labels[i].is_none() { d[i] } else { -d[i] };
            x - 1.
        }));
        let field = FaceField {
            regions: &fields,
            tissue,
            neighbours: face.map(|v| (labels[v as usize], across[v as usize])),
        };
        let mut leftover = original.to_vec();
        for &(r, f) in &fields {
            builders[r].polygon(face, &clip(&original, f), &field, Some(r), false)?;
            leftover = clip(&leftover, -f);
        }
        skin.polygon(face, &clip(&leftover, tissue), &field, None, false)?;
        let band = clip(&leftover, -tissue);
        let mut centre = band.clone();
        // Split at zero signed distance so the stripe coordinate stays linear on each flank.
        for f in fields.iter().map(|p| p.1).chain([tissue]) {
            let side = f + DVec3::ONE;
            skin.polygon(face, &clip(&band, side), &field, None, true)?;
            centre = clip(&centre, -side);
        }
        skin.polygon(face, &centre, &field, None, true)?;
    }
    let pieces = builders
        .into_iter()
        .enumerate()
        .map(|(i, b)| b.finish(positions).map_err(|e| format!("part {i}: {e}")))
        .collect::<Result<_, _>>()?;
    Ok(Partition {
        skin: skin.part,
        pieces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipping_preserves_barycentric_area_without_overlapping_tissue() {
        let tri = [DVec3::X, DVec3::Y, DVec3::Z];
        let field = DVec3::new(2., 2., -3.);
        let area = |p: &[DVec3]| {
            (1..p.len().saturating_sub(1))
                .map(|i| (p[i] - p[0]).cross(p[i + 1] - p[0]).length() / 2.)
                .sum::<f64>()
        };
        assert!((area(&clip(&tri, field)) + area(&clip(&tri, -field)) - area(&tri)).abs() < 1e-12);
    }
}
