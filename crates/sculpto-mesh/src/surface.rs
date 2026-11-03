//! Transport render vertices through a prepared control mesh. @req DECL0013

use std::fmt;

type Point = [f64; 3];

/// Invalid geometry cannot produce a usable render binding. @req DECL0013
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceError {
    EmptyGarment,
    EmptyControl,
    NonFinite {
        mesh: &'static str,
        vertex: usize,
    },
    InvalidIndex {
        triangle: usize,
        vertex: u32,
        vertices: usize,
    },
    CollapsedTriangle {
        triangle: usize,
    },
    TopologyMismatch,
    MissingTriangle {
        vertex: usize,
    },
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGarment => write!(f, "render surface has no vertices"),
            Self::EmptyControl => write!(f, "control surface has no triangles"),
            Self::NonFinite { mesh, vertex } => write!(f, "{mesh} vertex {vertex} is nonfinite"),
            Self::InvalidIndex {
                triangle,
                vertex,
                vertices,
            } => write!(
                f,
                "control triangle {triangle} references vertex {vertex}, but only {vertices} vertices exist"
            ),
            Self::CollapsedTriangle { triangle } => {
                write!(f, "control triangle {triangle} is collapsed")
            }
            Self::TopologyMismatch => write!(
                f,
                "control vertex count or ordered triangles differ from the binding"
            ),
            Self::MissingTriangle { vertex } => {
                write!(f, "render vertex {vertex} has no nearest control triangle")
            }
        }
    }
}

impl std::error::Error for SurfaceError {}

#[derive(Debug, Clone, PartialEq)]
struct BoundPoint {
    triangle: usize,
    coordinates: Point,
}

/// A reusable local-frame binding; applying never resnaps to the body. @req DECL0013
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceBinding {
    points: Vec<BoundPoint>,
    triangles: Vec<[u32; 3]>,
    vertices: usize,
}

impl SurfaceBinding {
    /// Preserve authored offsets in triangle frames, including outside edges. @req DECL0013
    pub fn bind(
        render: &[[f32; 3]],
        control: &[[f32; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<Self, SurfaceError> {
        if render.is_empty() {
            return Err(SurfaceError::EmptyGarment);
        }
        finite(render, "render")?;
        let frames = frames(control, triangles)?;
        let nearest = crate::check::nearest_triangle_indices(render, control, triangles);
        let points = render
            .iter()
            .zip(nearest)
            .enumerate()
            .map(|(vertex, (position, triangle))| {
                let triangle = triangle.ok_or(SurfaceError::MissingTriangle { vertex })?;
                Ok(BoundPoint {
                    triangle,
                    coordinates: frames[triangle].coordinates(position.map(f64::from)),
                })
            })
            .collect::<Result<Vec<_>, SurfaceError>>()?;
        Ok(Self {
            points,
            triangles: triangles.to_vec(),
            vertices: control.len(),
        })
    }

    /// Rotate thickness with the control while retaining render order. @req DECL0013
    pub fn apply(
        &self,
        control: &[[f32; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<Vec<[f32; 3]>, SurfaceError> {
        if control.len() != self.vertices || triangles != self.triangles {
            return Err(SurfaceError::TopologyMismatch);
        }
        let frames = frames(control, triangles)?;
        let output: Vec<_> = self
            .points
            .iter()
            .map(|point| {
                frames[point.triangle]
                    .position(point.coordinates)
                    .map(|value| value as f32)
            })
            .collect();
        finite(&output, "transported render")?;
        Ok(output)
    }

    /// Signed normal offsets inform the caller's collision guard. @req DECL0013
    pub fn normal_offset_range(&self) -> [f64; 2] {
        self.points
            .iter()
            .fold([f64::INFINITY, f64::NEG_INFINITY], |range, point| {
                [
                    range[0].min(point.coordinates[2]),
                    range[1].max(point.coordinates[2]),
                ]
            })
    }
}

struct Frame {
    origin: Point,
    first: Point,
    second: Point,
    normal: Point,
    area: f64,
}

impl Frame {
    fn new(corners: [Point; 3], triangle: usize) -> Result<Self, SurfaceError> {
        let first = subtract(corners[1], corners[0]);
        let second = subtract(corners[2], corners[0]);
        let normal = cross(first, second);
        let area = dot(normal, normal).sqrt();
        if area == 0.0 {
            return Err(SurfaceError::CollapsedTriangle { triangle });
        }
        Ok(Self {
            origin: corners[0],
            first,
            second,
            normal: normal.map(|value| value / area),
            area,
        })
    }

    fn coordinates(&self, point: Point) -> Point {
        let delta = subtract(point, self.origin);
        [
            dot(cross(delta, self.second), self.normal) / self.area,
            dot(cross(self.first, delta), self.normal) / self.area,
            dot(delta, self.normal),
        ]
    }

    fn position(&self, coordinates: Point) -> Point {
        std::array::from_fn(|axis| {
            self.origin[axis]
                + coordinates[0] * self.first[axis]
                + coordinates[1] * self.second[axis]
                + coordinates[2] * self.normal[axis]
        })
    }
}

fn finite(positions: &[[f32; 3]], mesh: &'static str) -> Result<(), SurfaceError> {
    match positions
        .iter()
        .position(|point| point.iter().any(|value| !value.is_finite()))
    {
        Some(vertex) => Err(SurfaceError::NonFinite { mesh, vertex }),
        None => Ok(()),
    }
}

fn frames(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Vec<Frame>, SurfaceError> {
    if triangles.is_empty() {
        return Err(SurfaceError::EmptyControl);
    }
    finite(positions, "control")?;
    triangles
        .iter()
        .enumerate()
        .map(|(triangle, indices)| {
            match indices
                .iter()
                .find(|&&vertex| vertex as usize >= positions.len())
            {
                Some(&vertex) => Err(SurfaceError::InvalidIndex {
                    triangle,
                    vertex,
                    vertices: positions.len(),
                }),
                None => Frame::new(
                    indices.map(|vertex| positions[vertex as usize].map(f64::from)),
                    triangle,
                ),
            }
        })
        .collect()
}

fn subtract(a: Point, b: Point) -> Point {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn dot(a: Point, b: Point) -> f64 {
    a.iter().zip(b).map(|(left, right)| left * right).sum()
}

fn cross(a: Point, b: Point) -> Point {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
