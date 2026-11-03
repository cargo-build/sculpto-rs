//! Position-only topology for diagnostics; render indices and corner attributes stay untouched.

use std::collections::HashMap;

/// Exactly coincident referenced positions collapsed for geometric analysis.
/// `vertex_sources[k]` is the lowest source vertex index at that position, so a
/// decision that keeps or removes a triangle keeps its lineage. @req DECL0013
pub(crate) struct Identical {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub vertex_sources: Vec<u32>,
}

/// The bit-exact key of a position; `-0.0` and `0.0` are the same place.
fn key(point: [f32; 3]) -> [u32; 3] {
    point.map(|value| match value == 0.0 {
        true => 0,
        false => value.to_bits(),
    })
}

/// Collapse exactly coincident referenced positions for geometric analysis.
/// @req DECL0013
pub(crate) fn position_topology(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Identical {
    let mut unique = Vec::new();
    let mut keys = Vec::new();
    let mut indices = HashMap::new();
    let remapped = triangles
        .iter()
        .map(|triangle| {
            triangle.map(|index| {
                let point = positions[index as usize];
                let identity = key(point);
                *indices.entry(identity).or_insert_with(|| {
                    let next = unique.len() as u32;
                    unique.push(point);
                    keys.push(identity);
                    next
                })
            })
        })
        .collect();
    let mut lowest: HashMap<[u32; 3], u32> = HashMap::new();
    for (index, point) in positions.iter().enumerate() {
        let source = index as u32;
        lowest
            .entry(key(*point))
            .and_modify(|current| *current = (*current).min(source))
            .or_insert(source);
    }
    let vertex_sources = keys
        .iter()
        .map(|identity| lowest.get(identity).copied().unwrap_or_default())
        .collect();
    Identical {
        positions: unique,
        triangles: remapped,
        vertex_sources,
    }
}
