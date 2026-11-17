//! Pair rest-coincident body rims for skinning while retaining the render topology.
//! @req DECL0013

use std::collections::{BTreeMap, BTreeSet};

use crate::surface_util::{OpenEdges, coincident_groups, edge_pairs};

const COINCIDENT_METRES: f32 = 1e-6;

/// Distinct mesh endpoints representing one point along paired body edges.
pub(super) struct SeamWeights(Vec<Vec<usize>>);

impl SeamWeights {
    pub(super) fn find(positions: &[[f64; 3]], triangles: &[[u32; 3]], flesh: &[bool]) -> Self {
        let boundary = OpenEdges::build(triangles, positions.len());
        let vertices: Vec<usize> = flesh
            .iter()
            .enumerate()
            .filter_map(|(v, &body)| (body && boundary.vertex_is_open(v as u32)).then_some(v))
            .collect();
        let points = vertices
            .iter()
            .map(|&v| positions[v].map(|axis| axis as f32))
            .collect::<Vec<_>>();
        let mut position = (0..positions.len()).collect::<Vec<_>>();
        for group in coincident_groups(&points, COINCIDENT_METRES) {
            let first = vertices[group[0]];
            group
                .into_iter()
                .for_each(|member| position[vertices[member]] = first);
        }

        let rims = paired_rims(triangles, flesh, &position, &boundary);
        let mut copies = BTreeMap::<usize, BTreeSet<usize>>::new();
        for ((start, end), edges) in rims {
            if let [(a, b, first), (c, d, second)] = edges.as_slice()
                && first != second
            {
                copies.entry(start).or_default().extend([*a, *c]);
                copies.entry(end).or_default().extend([*b, *d]);
            }
        }
        Self(
            copies
                .into_values()
                .filter(|members| members.len() > 1)
                .map(|members| members.into_iter().collect())
                .collect(),
        )
    }

    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    /// The surface walk can cross render seams by their rest separation.
    pub(super) fn links(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.0.iter().flat_map(|group| {
            group[1..]
                .iter()
                .map(|&other| (group[0] as u32, other as u32))
        })
    }

    /// Preserve symmetry isolation even if an alias straddles the edge of the side band.
    pub(super) fn share(&self, weights: &mut [f64], vertices: usize, side: &mut [f64]) {
        for group in &self.0 {
            let progress = group
                .iter()
                .map(|&v| side[v])
                .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                .expect("body seam has members");
            group.iter().for_each(|&v| side[v] = progress);
            for field in weights.chunks_exact_mut(vertices) {
                let mean = group.iter().map(|&v| field[v]).sum::<f64>() / group.len() as f64;
                group.iter().for_each(|&v| field[v] = mean);
            }
        }
    }
}

type Rim = (usize, usize, bool);

/// Compatible rims have two boundary edges with opposite winding at one rest location.
fn paired_rims(
    triangles: &[[u32; 3]],
    flesh: &[bool],
    position: &[usize],
    boundary: &OpenEdges,
) -> BTreeMap<(usize, usize), Vec<Rim>> {
    let mut rims = BTreeMap::<(usize, usize), Vec<Rim>>::new();
    for face in triangles
        .iter()
        .map(|t| t.map(|i| i as usize))
        .filter(|t| t.iter().all(|&v| flesh[v]))
    {
        for (a, b) in edge_pairs(&face) {
            if !boundary.used_once(a as u32, b as u32) || position[a] == position[b] {
                continue;
            }
            match position[a] < position[b] {
                true => rims
                    .entry((position[a], position[b]))
                    .or_default()
                    .push((a, b, true)),
                false => rims
                    .entry((position[b], position[a]))
                    .or_default()
                    .push((b, a, false)),
            }
        }
    }
    rims
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_render_vertices_follow_one_deformation_across_a_body_rim() {
        let points = [
            [0., 0., 0.],
            [1., 0., 0.],
            [0., 1., 0.],
            [1., 1e-7, 0.],
            [0., 1e-7, 0.],
            [0., -1., 0.],
            [1., 0., 0.],
            [0., 0., 0.],
            [0., 0., 1.],
        ];
        let triangles = [[0, 1, 2], [3, 4, 5], [6, 7, 8]];
        let mut flesh = [true; 9];
        flesh[6..].fill(false);
        let seams = SeamWeights::find(&points, &triangles, &flesh);
        let mut weights = vec![
            0.6, 0.6, 1., 0.2, 0.2, 0.1, 1., 1., 1., 0.4, 0.4, 0., 0.8, 0.8, 0.9, 0., 0., 0.,
        ];
        seams.share(&mut weights, points.len(), &mut [0.; 9]);
        let (unchanged, moving) = weights.split_at(points.len());
        // In a +1 m joint translation the moving weight is the displacement.
        assert_eq!(moving[0], moving[4]);
        assert_eq!(moving[1], moving[3]);
        assert_eq!(moving[6], 0.);
        assert_eq!(moving[8], 0.);
        assert!((unchanged[0] + moving[0] - 1.0).abs() < 1e-12);
    }
}
