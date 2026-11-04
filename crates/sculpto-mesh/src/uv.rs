//! Positive-area qualification of ordered triangle-corner bake atlases. @req DECL0013

use std::error::Error;
use std::fmt;

use robust::Coord;

/// Complete atlas diagnostics retain the caller's triangle and corner identities. @req DECL0013
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Qualification {
    /// Collinear or coincident triangles, in input order.
    pub degenerate_triangles: Vec<usize>,
    /// Sorted unique face pairs whose interiors overlap in positive area.
    pub overlap_pairs: Vec<[usize; 2]>,
    /// Triangle/corner indices outside the closed unit square.
    pub out_of_bounds_corners: Vec<[usize; 2]>,
}

impl Qualification {
    /// Baking requires all three geometric and atlas-domain checks to pass. @req DECL0013
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.degenerate_triangles.is_empty()
            && self.overlap_pairs.is_empty()
            && self.out_of_bounds_corners.is_empty()
    }
}

/// A nonfinite coordinate cannot participate in robust orientation predicates. @req DECL0013
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct NonFiniteCoordinate {
    /// Input triangle index.
    pub triangle: usize,
    /// Corner index within that triangle.
    pub corner: usize,
    /// Coordinate index: zero for U, one for V.
    pub axis: usize,
}

impl fmt::Display for NonFiniteCoordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "nonfinite UV coordinate at triangle {}, corner {}, axis {}",
            self.triangle, self.corner, self.axis
        )
    }
}

impl Error for NonFiniteCoordinate {}

struct Triangle {
    id: usize,
    corners: [Coord<f64>; 3],
    min: [f32; 2],
    max: [f32; 2],
}

/// Qualify every UV triangle; boundary-only contact is legal regardless of topology. @req DECL0013
///
/// # Errors
/// Returns the first nonfinite coordinate, indexed by triangle, corner and axis.
pub fn qualify(triangles: &[[[f32; 2]; 3]]) -> Result<Qualification, NonFiniteCoordinate> {
    let mut report = Qualification::default();
    let mut finite = Vec::with_capacity(triangles.len());
    triangles
        .iter()
        .enumerate()
        .try_for_each(|(id, triangle)| {
            triangle
                .iter()
                .enumerate()
                .try_for_each(|(corner, point)| {
                    point.iter().enumerate().try_for_each(|(axis, value)| {
                        match value.is_finite() {
                            true => Ok(()),
                            false => Err(NonFiniteCoordinate {
                                triangle: id,
                                corner,
                                axis,
                            }),
                        }
                    })?;
                    match point.iter().all(|value| (0.0..=1.0).contains(value)) {
                        true => {}
                        false => report.out_of_bounds_corners.push([id, corner]),
                    }
                    Ok(())
                })?;
            let mut corners = triangle.map(|[x, y]| Coord {
                x: f64::from(x),
                y: f64::from(y),
            });
            let orientation = robust::orient2d(corners[0], corners[1], corners[2]);
            match orientation {
                0.0 => report.degenerate_triangles.push(id),
                _ => {
                    if orientation < 0.0 {
                        corners.swap(1, 2);
                    }
                    finite.push(Triangle {
                        id,
                        corners,
                        min: [0, 1].map(|axis| {
                            triangle
                                .iter()
                                .map(|point| point[axis])
                                .fold(f32::INFINITY, f32::min)
                        }),
                        max: [0, 1].map(|axis| {
                            triangle
                                .iter()
                                .map(|point| point[axis])
                                .fold(f32::NEG_INFINITY, f32::max)
                        }),
                    });
                }
            }
            Ok(())
        })?;
    finite.sort_by(|left, right| {
        left.min[0]
            .total_cmp(&right.min[0])
            .then(left.id.cmp(&right.id))
    });
    let mut active: Vec<&Triangle> = Vec::new();
    finite.iter().for_each(|current| {
        active.retain(|other| other.max[0] > current.min[0]);
        active
            .iter()
            .filter(|other| other.max[1] > current.min[1] && current.max[1] > other.min[1])
            .filter(|other| interiors_overlap(&current.corners, &other.corners))
            .for_each(|other| {
                report
                    .overlap_pairs
                    .push([other.id.min(current.id), other.id.max(current.id)])
            });
        active.push(current);
    });
    report.overlap_pairs.sort_unstable();
    Ok(report)
}

/// Nonpositive half-plane support separates interiors while allowing shared boundaries.
fn separates(left: &[Coord<f64>; 3], right: &[Coord<f64>; 3]) -> bool {
    (0..3).any(|edge| {
        right
            .iter()
            .all(|point| robust::orient2d(left[edge], left[(edge + 1) % 3], *point) <= 0.0)
    })
}

fn interiors_overlap(left: &[Coord<f64>; 3], right: &[Coord<f64>; 3]) -> bool {
    !separates(left, right) && !separates(right, left)
}
