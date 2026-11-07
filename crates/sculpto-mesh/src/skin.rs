//! Bone-weight transfer for fitted garments.
//!
//! Outfit Studio's `OutfitProject::CopyBoneWeights` builds a proximity cache
//! between the outfit and the reference body, resamples the body's per-bone
//! weight scalars through that cache and normalizes the result. This module
//! does the same thing directly: for every garment vertex it takes the closest
//! point on the body surface, interpolates the three corner weights
//! barycentrically, keeps the four strongest bones and normalizes them.
//!
//! The garment then follows the body in animation: identical bone ids and
//! weights as the skin underneath, so linear blend skinning moves both.

use crate::check::Bvh;

/// One vertex's skin influences: bone slots plus normalized weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Influence {
    pub bones: [u16; 4],
    pub weights: [f32; 4],
}

/// Transfer the body's skin weights onto garment vertices.
///
/// `body_positions` / `body_triangles` describe the body surface, `body_bones`
/// and `body_weights` are its per-vertex influences (parallel to positions).
/// Garment vertices are expected in the same frame as the body.
pub fn transfer_weights(
    body_positions: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
    body_bones: &[[u16; 4]],
    body_weights: &[[f32; 4]],
    garment_positions: &[[f32; 3]],
) -> Option<Vec<Influence>> {
    if body_positions.is_empty()
        || body_triangles.is_empty()
        || body_positions.len() != body_bones.len()
        || body_positions.len() != body_weights.len()
    {
        return None;
    }
    let bvh = Bvh::build_for_triangles(body_positions, body_triangles);
    let mut result = Vec::with_capacity(garment_positions.len());
    for point in garment_positions {
        let (triangle, _) = bvh.nearest(*point)?;
        let corners = body_triangles[triangle];
        let face = [
            body_positions[corners[0] as usize],
            body_positions[corners[1] as usize],
            body_positions[corners[2] as usize],
        ];
        let closest = closest_on_triangle(*point, face);
        let (u, v, w) = barycentric(closest, face);
        let mut accumulated: Vec<(u16, f32)> = Vec::with_capacity(12);
        for (corner, share) in [(corners[0], u), (corners[1], v), (corners[2], w)] {
            if share <= 0.0 {
                continue;
            }
            let index = corner as usize;
            for slot in 0..4 {
                let weight = body_weights[index][slot];
                if weight <= 0.0 {
                    continue;
                }
                let bone = body_bones[index][slot];
                match accumulated.iter_mut().find(|(id, _)| *id == bone) {
                    Some((_, total)) => *total += share * weight,
                    None => accumulated.push((bone, share * weight)),
                }
            }
        }
        if accumulated.is_empty() {
            // Degenerate corner data: fall back to the nearest body vertex.
            let mut nearest = 0usize;
            let mut best = f32::INFINITY;
            for (index, position) in body_positions.iter().enumerate() {
                let d = [
                    position[0] - point[0],
                    position[1] - point[1],
                    position[2] - point[2],
                ];
                let distance = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                if distance < best {
                    best = distance;
                    nearest = index;
                }
            }
            result.push(Influence {
                bones: body_bones[nearest],
                weights: body_weights[nearest],
            });
            continue;
        }
        accumulated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        accumulated.truncate(4);
        let total: f32 = accumulated.iter().map(|(_, weight)| *weight).sum();
        let mut bones = [0u16; 4];
        let mut weights = [0.0f32; 4];
        for (slot, (bone, weight)) in accumulated.iter().enumerate() {
            bones[slot] = *bone;
            weights[slot] = if total > 0.0 { weight / total } else { 0.0 };
        }
        result.push(Influence { bones, weights });
    }
    Some(result)
}

fn closest_on_triangle(point: [f32; 3], face: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = face;
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(point, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(point, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let cp = sub(point, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let t = d1 / (d1 - d3);
        return add(a, scale(ab, t));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let t = d2 / (d2 - d6);
        return add(a, scale(ac, t));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return add(b, scale(sub(c, b), t));
    }
    let denominator = 1.0 / (va + vb + vc);
    let v = vb * denominator;
    let w = vc * denominator;
    add(a, add(scale(ab, v), scale(ac, w)))
}

fn barycentric(point: [f32; 3], face: [[f32; 3]; 3]) -> (f32, f32, f32) {
    let [a, b, c] = face;
    let v0 = sub(b, a);
    let v1 = sub(c, a);
    let v2 = sub(point, a);
    let d00 = dot(v0, v0);
    let d01 = dot(v0, v1);
    let d11 = dot(v1, v1);
    let d20 = dot(v2, v0);
    let d21 = dot(v2, v1);
    let denominator = d00 * d11 - d01 * d01;
    if denominator.abs() < 1e-12 {
        return (1.0, 0.0, 0.0);
    }
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    (1.0 - v - w, v, w)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
