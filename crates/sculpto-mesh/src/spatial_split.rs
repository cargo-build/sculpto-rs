//! Split a generated mesh into figures separated by empty X columns.
//!
//! Tripo exports the whole scene as one mesh: several separated figures share
//! one index buffer and one material. Connected components are garment panels,
//! not figures, so the split uses the empty world-space X bands between
//! figures. The threshold is a declared fraction of the model extent; a
//! triangle that crosses a band is refused instead of silently reassigned.

use std::fmt;

/// A gap must exceed this fraction of the X extent to separate two figures.
const GAP_FRACTION: f32 = 0.01;

/// One spatial part, reindexed to its own vertex range.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitPart {
    /// Vertex positions in source units.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals, empty when the source carries none.
    pub normals: Vec<[f32; 3]>,
    /// Vertex UVs, empty when the source carries none.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices into this part's vertex range.
    pub triangles: Vec<[u32; 3]>,
    /// Minimum position per axis.
    pub min: [f32; 3],
    /// Maximum position per axis.
    pub max: [f32; 3],
}

/// Why a mesh cannot be split by X gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// The source has no vertices.
    NoVertices,
    /// A per-vertex attribute is neither empty nor one value per position.
    AttributeLengthMismatch { attribute: &'static str },
    /// A triangle references a vertex outside the position list.
    TriangleOutOfRange { triangle: usize },
    /// A position is not finite.
    NonFinite { index: usize },
    /// No empty X band exceeds the split threshold.
    NoGap,
    /// A triangle has vertices on both sides of a band.
    CrossingTriangle { triangle: usize },
}

impl fmt::Display for SplitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVertices => write!(formatter, "mesh has no vertices"),
            Self::AttributeLengthMismatch { attribute } => {
                write!(
                    formatter,
                    "{attribute} must be empty or one value per position"
                )
            }
            Self::TriangleOutOfRange { triangle } => {
                write!(formatter, "triangle {triangle} references a missing vertex")
            }
            Self::NonFinite { index } => {
                write!(formatter, "position {index} is not finite")
            }
            Self::NoGap => write!(formatter, "no empty X band separates the model"),
            Self::CrossingTriangle { triangle } => {
                write!(formatter, "triangle {triangle} crosses an X band")
            }
        }
    }
}

impl std::error::Error for SplitError {}

/// Split positions and triangles at empty X bands into ascending X order.
///
/// Normals and UVs are optional; when present they are reindexed with the
/// positions. Every triangle must lie entirely in one part.
pub fn split_x(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    triangles: &[[u32; 3]],
) -> Result<Vec<SplitPart>, SplitError> {
    if positions.is_empty() {
        return Err(SplitError::NoVertices);
    }
    if !normals.is_empty() && normals.len() != positions.len() {
        return Err(SplitError::AttributeLengthMismatch {
            attribute: "normals",
        });
    }
    if !uvs.is_empty() && uvs.len() != positions.len() {
        return Err(SplitError::AttributeLengthMismatch { attribute: "uvs" });
    }
    for (index, position) in positions.iter().enumerate() {
        if !position.iter().all(|value| value.is_finite()) {
            return Err(SplitError::NonFinite { index });
        }
    }
    for (triangle, [a, b, c]) in triangles.iter().enumerate() {
        let limit = positions.len() as u32;
        if *a >= limit || *b >= limit || *c >= limit {
            return Err(SplitError::TriangleOutOfRange { triangle });
        }
    }

    let mut xs: Vec<f32> = positions.iter().map(|position| position[0]).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    let extent = xs.last().expect("nonempty") - xs.first().expect("nonempty");
    if extent <= 0.0 {
        return Err(SplitError::NoGap);
    }
    let threshold = extent * GAP_FRACTION;
    let boundaries: Vec<f32> = xs
        .windows(2)
        .filter(|pair| pair[1] - pair[0] > threshold)
        .map(|pair| (pair[0] + pair[1]) * 0.5)
        .collect();
    if boundaries.is_empty() {
        return Err(SplitError::NoGap);
    }

    let group_of: Vec<usize> = positions
        .iter()
        .map(|position| {
            boundaries
                .iter()
                .filter(|bound| position[0] > **bound)
                .count()
        })
        .collect();
    for (triangle, [a, b, c]) in triangles.iter().enumerate() {
        let group = group_of[*a as usize];
        if group_of[*b as usize] != group || group_of[*c as usize] != group {
            return Err(SplitError::CrossingTriangle { triangle });
        }
    }

    let mut members: Vec<Vec<usize>> = vec![Vec::new(); boundaries.len() + 1];
    for (index, group) in group_of.iter().enumerate() {
        members[*group].push(index);
    }
    let mut parts = Vec::with_capacity(members.len());
    for indices in members {
        if indices.is_empty() {
            continue;
        }
        let mut reindex = vec![u32::MAX; positions.len()];
        let mut part = SplitPart {
            positions: Vec::with_capacity(indices.len()),
            normals: Vec::new(),
            uvs: Vec::new(),
            triangles: Vec::new(),
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        if !normals.is_empty() {
            part.normals.reserve(indices.len());
        }
        if !uvs.is_empty() {
            part.uvs.reserve(indices.len());
        }
        for (new, old) in indices.into_iter().enumerate() {
            reindex[old] = new as u32;
            let position = positions[old];
            part.positions.push(position);
            if !normals.is_empty() {
                part.normals.push(normals[old]);
            }
            if !uvs.is_empty() {
                part.uvs.push(uvs[old]);
            }
            for (axis, value) in position.iter().enumerate() {
                part.min[axis] = part.min[axis].min(*value);
                part.max[axis] = part.max[axis].max(*value);
            }
        }
        for triangle in triangles {
            let [a, b, c] = *triangle;
            let (a, b, c) = (
                reindex[a as usize],
                reindex[b as usize],
                reindex[c as usize],
            );
            if a != u32::MAX && b != u32::MAX && c != u32::MAX {
                part.triangles.push([a, b, c]);
            }
        }
        parts.push(part);
    }
    Ok(parts)
}
