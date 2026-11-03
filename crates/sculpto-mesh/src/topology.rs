//! Position-only topology for diagnostics; render indices and corner attributes stay untouched.

use std::collections::HashMap;

/// Collapse exactly coincident referenced positions for geometric analysis. @req DECL0013
pub(crate) fn position_topology(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut unique = Vec::new();
    let mut indices = HashMap::new();
    let remapped = triangles
        .iter()
        .map(|triangle| {
            triangle.map(|index| {
                let point = positions[index as usize];
                let key = point.map(|value| match value == 0.0 {
                    true => 0,
                    false => value.to_bits(),
                });
                *indices.entry(key).or_insert_with(|| {
                    let next = unique.len() as u32;
                    unique.push(point);
                    next
                })
            })
        })
        .collect();
    (unique, remapped)
}
