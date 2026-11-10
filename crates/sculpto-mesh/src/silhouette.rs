//! Fixed-frame projected overlap; contact and 3D shape remain separate measures.
//! @req DECL0013

use std::collections::TryReserveError;
use std::fmt;

use crate::surface_util::first_non_finite;
use crate::wrap::Mesh;

/// Coordinates retained by an orthographic projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionPlane {
    Xy,
    Xz,
    Yz,
}

impl ProjectionPlane {
    fn project(self, point: [f32; 3]) -> [f64; 2] {
        let [x, y, z] = point.map(f64::from);
        match self {
            Self::Xy => [x, y],
            Self::Xz => [x, z],
            Self::Yz => [y, z],
        }
    }
}

/// Raster settings shared by both silhouettes, independently of mesh bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RasterSettings {
    pub resolution: u16,
    pub plane: ProjectionPlane,
}

impl Default for RasterSettings {
    fn default() -> Self {
        Self {
            resolution: 512,
            plane: ProjectionPlane::Xz,
        }
    }
}

/// Validated camera bounds; one frame must be reused for both compared meshes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrthographicFrame {
    bounds: [[f64; 2]; 2],
    settings: RasterSettings,
}

impl OrthographicFrame {
    /// Each bounds row is `[min, max]` on one retained axis. @req DECL0013
    pub fn new(bounds: [[f64; 2]; 2], settings: RasterSettings) -> Result<Self, SilhouetteError> {
        if settings.resolution == 0 {
            return Err(SilhouetteError::ZeroResolution);
        }
        if !bounds.iter().all(|[min, max]| {
            min.is_finite() && max.is_finite() && max > min && (max - min).is_finite()
        }) {
            return Err(SilhouetteError::InvalidBounds);
        }
        Ok(Self { bounds, settings })
    }

    pub fn bounds(&self) -> [[f64; 2]; 2] {
        self.bounds
    }

    pub fn settings(&self) -> RasterSettings {
        self.settings
    }
}

/// Input identity retained in validation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshRole {
    Reference,
    Candidate,
}

/// Measurements, with no admission threshold or implicit shape normalization.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SilhouetteOverlap {
    pub reference_pixels: u64,
    pub candidate_pixels: u64,
    pub intersection_pixels: u64,
    pub union_pixels: u64,
    pub iou: f64,
}

/// Why a projection cannot supply a valid comparison.
#[derive(Debug)]
pub enum SilhouetteError {
    ZeroResolution,
    InvalidBounds,
    EmptyMesh {
        role: MeshRole,
    },
    NonFiniteVertex {
        role: MeshRole,
        vertex: usize,
    },
    FaceIndex {
        role: MeshRole,
        triangle: usize,
        vertex: u32,
    },
    OutsideFrame {
        role: MeshRole,
        vertex: usize,
    },
    EmptyReference,
    Allocation {
        words: usize,
        source: TryReserveError,
    },
}

impl fmt::Display for SilhouetteError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroResolution => write!(out, "silhouette resolution must be positive"),
            Self::InvalidBounds => {
                write!(out, "silhouette bounds must have finite positive extents")
            }
            Self::EmptyMesh { role } => write!(out, "{role:?} mesh has no vertices or faces"),
            Self::NonFiniteVertex { role, vertex } => {
                write!(out, "{role:?} vertex {vertex} is not finite")
            }
            Self::FaceIndex {
                role,
                triangle,
                vertex,
            } => write!(
                out,
                "{role:?} triangle {triangle} references missing vertex {vertex}"
            ),
            Self::OutsideFrame { role, vertex } => write!(
                out,
                "{role:?} vertex {vertex} would be cropped by the silhouette frame"
            ),
            Self::EmptyReference => write!(out, "reference silhouette covers no pixel centers"),
            Self::Allocation { words, source } => {
                write!(out, "cannot allocate {words} raster words: {source}")
            }
        }
    }
}

impl std::error::Error for SilhouetteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Allocation { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Compare triangle-union silhouettes without moving their shared frame.
/// A candidate with no covered pixels scores zero. @req DECL0013
pub fn compare(
    reference: Mesh<'_>,
    candidate: Mesh<'_>,
    frame: &OrthographicFrame,
) -> Result<SilhouetteOverlap, SilhouetteError> {
    let reference = rasterize(reference, frame, MeshRole::Reference)?;
    let candidate = rasterize(candidate, frame, MeshRole::Candidate)?;
    let reference_pixels = pixels(&reference);
    if reference_pixels == 0 {
        return Err(SilhouetteError::EmptyReference);
    }
    let intersection_pixels = reference
        .iter()
        .zip(&candidate)
        .map(|(a, b)| u64::from((a & b).count_ones()))
        .sum();
    let union_pixels = reference
        .iter()
        .zip(&candidate)
        .map(|(a, b)| u64::from((a | b).count_ones()))
        .sum();
    Ok(SilhouetteOverlap {
        reference_pixels,
        candidate_pixels: pixels(&candidate),
        intersection_pixels,
        union_pixels,
        iou: intersection_pixels as f64 / union_pixels as f64,
    })
}

fn pixels(words: &[u64]) -> u64 {
    words.iter().map(|word| u64::from(word.count_ones())).sum()
}

fn projected(
    mesh: Mesh<'_>,
    frame: &OrthographicFrame,
    role: MeshRole,
) -> Result<Vec<[f64; 2]>, SilhouetteError> {
    if mesh.0.is_empty() || mesh.1.is_empty() {
        return Err(SilhouetteError::EmptyMesh { role });
    }
    if let Some(vertex) = first_non_finite(mesh.0) {
        return Err(SilhouetteError::NonFiniteVertex { role, vertex });
    }
    mesh.1.iter().enumerate().try_for_each(|(triangle, face)| {
        face.iter()
            .try_for_each(|&vertex| match (vertex as usize) < mesh.0.len() {
                true => Ok(()),
                false => Err(SilhouetteError::FaceIndex {
                    role,
                    triangle,
                    vertex,
                }),
            })
    })?;
    mesh.0
        .iter()
        .enumerate()
        .map(|(vertex, &point)| {
            let point = frame.settings.plane.project(point);
            if point
                .iter()
                .zip(frame.bounds)
                .any(|(&value, [min, max])| value < min || value > max)
            {
                return Err(SilhouetteError::OutsideFrame { role, vertex });
            }
            Ok(std::array::from_fn(|axis| {
                let [min, max] = frame.bounds[axis];
                (point[axis] - min) / (max - min) * f64::from(frame.settings.resolution)
            }))
        })
        .collect()
}

fn rasterize(
    mesh: Mesh<'_>,
    frame: &OrthographicFrame,
    role: MeshRole,
) -> Result<Vec<u64>, SilhouetteError> {
    let positions = projected(mesh, frame, role)?;
    let resolution = usize::from(frame.settings.resolution);
    let words = (resolution * resolution).div_ceil(64);
    let mut raster = Vec::new();
    raster
        .try_reserve_exact(words)
        .map_err(|source| SilhouetteError::Allocation { words, source })?;
    raster.resize(words, 0);
    mesh.1.iter().for_each(|face| {
        draw_triangle(
            face.map(|vertex| positions[vertex as usize]),
            resolution,
            &mut raster,
        );
    });
    Ok(raster)
}

fn edge(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (point[1] - a[1]) - (b[1] - a[1]) * (point[0] - a[0])
}

fn draw_triangle(corners: [[f64; 2]; 3], resolution: usize, raster: &mut [u64]) {
    let area = edge(corners[0], corners[1], corners[2]);
    if area == 0.0 {
        return;
    }
    let sign = area.signum();
    let ranges: [_; 2] = std::array::from_fn(|axis| {
        let min = corners
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min);
        let max = corners
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max);
        let first = min.floor() as usize;
        let last = (max.ceil() as usize).min(resolution - 1);
        first..=last
    });
    for row in ranges[1].clone() {
        for column in ranges[0].clone() {
            let point = [column as f64 + 0.5, row as f64 + 0.5];
            if [(0, 1), (1, 2), (2, 0)]
                .into_iter()
                .all(|(a, b)| edge(corners[a], corners[b], point) * sign >= 0.0)
            {
                let index = row * resolution + column;
                raster[index / 64] |= 1 << (index % 64);
            }
        }
    }
}
