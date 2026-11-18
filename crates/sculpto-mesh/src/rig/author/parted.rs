//! Partition the entire A surface at shared seams and attach zero-width membranes to both rims.
use super::seams::SeamWeights;
use crate::surface_util::coincident_groups;
use glam::DVec3;
use std::collections::{BTreeMap, BTreeSet};

/// A material point in one original triangle. Each side of a seam has its own attachment.
#[derive(Clone)]
pub struct Site {
    pub face: [u32; 3],
    pub bary: [f64; 3],
    pub owner: Option<usize>,
    pub band_side: u8,
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

#[derive(Default)]
pub struct Piece {
    pub sites: Vec<Site>,
    pub faces: Vec<[u32; 3]>,
    pub rims: Vec<Vec<u32>>,
}
pub struct Partition {
    pub skin: Piece,
    pub membrane: Piece,
    pub pieces: Vec<Piece>,
}

#[derive(Eq, PartialEq, Ord, PartialOrd)]
struct MaterialPoint {
    vertices: [u32; 3],
    owner: Option<usize>,
    side: u8,
}
#[derive(Default)]
struct Build {
    part: Piece,
    index: BTreeMap<MaterialPoint, u32>,
    cut: Vec<(u32, u32)>,
}
impl Build {
    fn site(
        &mut self,
        face: [u32; 3],
        b: DVec3,
        owner: Option<usize>,
        side: u8,
    ) -> Result<u32, String> {
        // Categorical barycentric cells meet exactly at corners, edge midpoints and the centroid.
        // Reconstruct those points identically on either side, including a triple junction.
        let support = b.to_array().map(|w| w > 1e-12);
        let count = support.iter().filter(|&&v| v).count() as f64;
        let bary = support.map(|yes| if yes { 1. / count } else { 0. });
        let mut vertices = std::array::from_fn(|i| if support[i] { face[i] } else { u32::MAX });
        vertices.sort_unstable();
        let key = MaterialPoint {
            vertices,
            owner,
            side,
        };
        if let Some(&i) = self.index.get(&key) {
            return Ok(i);
        }
        let i = u32::try_from(self.part.sites.len()).map_err(|_| "too many cut vertices")?;
        self.index.insert(key, i);
        self.part.sites.push(Site {
            face,
            bary,
            owner,
            band_side: side,
        });
        Ok(i)
    }

    fn polygon(
        &mut self,
        face: [u32; 3],
        poly: &[DVec3],
        owner: Option<usize>,
    ) -> Result<Vec<u32>, String> {
        let ids = poly
            .iter()
            .map(|&p| self.site(face, p, owner, 0))
            .collect::<Result<Vec<_>, _>>()?;
        for side in ids[1..].windows(2) {
            self.part.faces.push([ids[0], side[0], side[1]]);
        }
        Ok(ids)
    }

    /// Follow each exterior half-edge with a reversed attachment edge. The paired points coincide in A. @req DECL0013
    fn bridge(
        &mut self,
        face: [u32; 3],
        edge: [DVec3; 2],
        owners: [Option<usize>; 2],
    ) -> Result<(), String> {
        let [p, q] = edge;
        let a = self.site(face, p, owners[0], 0)?;
        let b = self.site(face, q, owners[0], 0)?;
        let c = self.site(face, p, owners[1], 1)?;
        let d = self.site(face, q, owners[1], 1)?;
        self.part.faces.extend([[b, a, c], [b, c, d]]);
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

/// Allocate every source triangle to its adjacent materials; the membrane owns no A-pose area. @req DECL0013
pub fn cut(
    positions: &[[f64; 3]],
    triangles: &[[u32; 3]],
    mut labels: Vec<Option<usize>>,
    count: usize,
) -> Result<Partition, String> {
    if positions.is_empty()
        || labels.len() != positions.len()
        || labels.iter().flatten().any(|&i| i >= count)
        || triangles
            .iter()
            .flatten()
            .any(|&v| v as usize >= positions.len())
    {
        return Err("invalid material segmentation inputs".into());
    }
    let mut used = vec![false; positions.len()];
    for &v in triangles.iter().flatten() {
        used[v as usize] = true;
    }
    let seams = SeamWeights::find(positions, triangles, &used);
    for (a, b) in seams.links() {
        labels[b as usize] = labels[a as usize];
    }
    let mut rigid: Vec<_> = (0..count).map(|_| Build::default()).collect();
    let mut skin = Build::default();
    let mut membrane = Build::default();
    for &face in triangles {
        divide_face(
            face,
            face.map(|v| labels[v as usize]),
            &mut rigid,
            &mut skin,
            &mut membrane,
        )?;
    }
    let pieces = rigid
        .into_iter()
        .enumerate()
        .map(|(i, b)| b.finish(positions).map_err(|e| format!("part {i}: {e}")))
        .collect::<Result<_, _>>()?;
    Ok(Partition {
        skin: skin.part,
        membrane: membrane.part,
        pieces,
    })
}

fn divide_face(
    face: [u32; 3],
    owners: [Option<usize>; 3],
    rigid: &mut [Build],
    skin: &mut Build,
    membrane: &mut Build,
) -> Result<(), String> {
    let corners = [DVec3::X, DVec3::Y, DVec3::Z];
    let mut regions = BTreeMap::<Option<usize>, DVec3>::new();
    for (owner, point) in owners.into_iter().zip(corners) {
        *regions.entry(owner).or_default() += point;
    }
    for (&owner, score) in &regions {
        let mut area = corners.to_vec();
        for (&other, field) in &regions {
            if other != owner {
                area = clip(&area, *score - *field);
            }
        }
        area.dedup_by(|x, y| x.abs_diff_eq(*y, 1e-12));
        if area.len() > 1 && area[0].abs_diff_eq(*area.last().expect("nonempty polygon"), 1e-12) {
            area.pop();
        }
        if area.len() < 3 {
            return Err("material region has no exterior area".into());
        }
        let builder = match owner {
            Some(i) => &mut rigid[i],
            None => &mut *skin,
        };
        let ids = builder.polygon(face, &area, owner)?;
        for i in 0..area.len() {
            let j = (i + 1) % area.len();
            let (a, b) = (area[i], area[j]);
            let neighbour = regions.iter().find(|(other, field)| {
                **other != owner
                    && a.dot(*score - **field).abs() < 1e-9
                    && b.dot(*score - **field).abs() < 1e-9
            });
            if let Some((&other, _)) = neighbour {
                if owner.is_some() {
                    builder.cut.push((ids[j], ids[i]));
                }
                if owner < other {
                    membrane.bridge(face, [a, b], [owner, other])?;
                }
            }
        }
    }
    // A junction of three separating materials also opens a triangle between the three ribbon ends.
    if regions.len() == 3 {
        let b = DVec3::splat(1. / 3.);
        let [a, c, d] = owners;
        let tri = [
            membrane.site(face, b, a, 0)?,
            membrane.site(face, b, c, 0)?,
            membrane.site(face, b, d, 0)?,
        ];
        membrane.part.faces.push(tri);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projected_area(piece: &Piece, p: &[[f64; 3]], separation: f64) -> f64 {
        piece
            .faces
            .iter()
            .map(|f| {
                let [a, b, c] = f.map(|v| {
                    let s = &piece.sites[v as usize];
                    let xyz = s.point(p);
                    if s.owner.is_some() {
                        xyz + separation * DVec3::X
                    } else {
                        xyz
                    }
                });
                0.5 * (b - a).cross(c - a).z
            })
            .sum()
    }
    #[test]
    fn closed_edges_reserve_no_surface_and_unfold_only_on_the_opening_side() {
        let p = [[-1., -1., 0.], [1., 0., 0.], [-1., 1., 0.]];
        let (mut skin, mut rigid, mut film) =
            (Build::default(), [Build::default()], Build::default());
        divide_face(
            [0, 1, 2],
            [None, Some(0), None],
            &mut rigid,
            &mut skin,
            &mut film,
        )
        .unwrap();
        assert_eq!(
            projected_area(&skin.part, &p, 0.) + projected_area(&rigid[0].part, &p, 0.),
            2.
        );
        assert_eq!(film.part.faces.len(), 2);
        for gap in [0., 0.25, 0., -0.25, 0.] {
            assert_eq!(projected_area(&film.part, &p, gap), gap);
        }
    }
    #[test]
    fn three_materials_share_the_central_point_until_separated() {
        let p = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]];
        let (mut skin, mut film) = (Build::default(), Build::default());
        let mut rigid = [Build::default(), Build::default()];
        divide_face(
            [0, 1, 2],
            [None, Some(0), Some(1)],
            &mut rigid,
            &mut skin,
            &mut film,
        )
        .unwrap();
        assert_eq!(film.part.faces.len(), 7);
        assert_eq!(projected_area(&film.part, &p, 0.), 0.);
        let total = projected_area(&skin.part, &p, 0.)
            + rigid
                .iter()
                .map(|r| projected_area(&r.part, &p, 0.))
                .sum::<f64>();
        assert!((total - 0.5).abs() < 1e-12);
    }
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
