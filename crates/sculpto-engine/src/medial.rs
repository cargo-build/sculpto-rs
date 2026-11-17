//! The medial axis of the flesh: centres of maximal inscribed spheres.
//!
//! The seat takes its joint positions from the body's own centreline, not from
//! bone-weight centroids or a table of anatomical offsets. The mesh is voxelised,
//! a signed distance field is computed (Felzenszwalb–Huttenlocher separable EDT),
//! and the medial axis is the ridge of that field — the voxels whose inscribed
//! radius is a local maximum. A limb's ridge is a clean centreline that follows a
//! flexed elbow as easily as a straight one; a mass (torso, skull) has a ridge
//! sheet, sampled here by the deepest inscribed centre per cross-section.
//!
//! Mixamo supplies the graph (65 joints, hierarchy, names); this module supplies
//! the positions and the local inscribed radius at each. No native joint seat
//! enters the placement. @req DECL0008

use std::collections::BTreeMap;

use glam::DVec3;
use serde_json::{Value, json};

/// A regular voxel grid in world Y-up metres.
pub struct Grid {
    /// Cell counts along x, y, z.
    pub dims: [usize; 3],
    /// World position of the centre of cell (0, 0, 0).
    pub origin: DVec3,
    /// Cell edge length, metres.
    pub h: f64,
}

impl Grid {
    /// A grid covering `min..max` with a two-cell margin, at edge length `h`.
    pub fn from_bounds(min: DVec3, max: DVec3, h: f64) -> Self {
        let pad = 2.0 * h;
        let lo = min - DVec3::splat(pad);
        let hi = max + DVec3::splat(pad);
        let dims = [
            ((hi.x - lo.x) / h).ceil() as usize + 1,
            ((hi.y - lo.y) / h).ceil() as usize + 1,
            ((hi.z - lo.z) / h).ceil() as usize + 1,
        ];
        Self {
            dims,
            origin: lo,
            h,
        }
    }

    /// Flat index of cell (x, y, z); x fastest, then y, then z.
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.dims[0] * (y + self.dims[1] * z)
    }

    /// World centre of cell (x, y, z).
    #[inline]
    pub fn world(&self, x: usize, y: usize, z: usize) -> DVec3 {
        self.origin + DVec3::new(x as f64, y as f64, z as f64) * self.h
    }

    /// Total cell count.
    #[inline]
    pub fn len(&self) -> usize {
        self.dims[0] * self.dims[1] * self.dims[2]
    }

    /// Whether the grid has no cells.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Distance from a point to a triangle (Ericson, Real-Time Collision Detection).
fn point_tri_distance(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (ap - ab * v).length();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (ap - ac * w).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (bp - (c - b) * w).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (ap - ab * v - ac * w).length()
}

/// Voxelise a closed triangle mesh into an inside mask.
///
/// A cell is surface when its centre lies within half a voxel diagonal of a
/// triangle (a conservative superset of the cells the triangle passes through).
/// The exterior is flooded from the grid boundary through non-surface cells
/// (6-connectivity); everything the flood cannot reach is flesh. Unlike vertical
/// ray parity this cannot leave a hole where a ray grazes a vertex or edge.
pub fn occupancy(
    grid: &Grid,
    vertices: &[[f64; 3]],
    triangles: &[[u32; 3]],
    seal_voxels: usize,
) -> Vec<bool> {
    let [nx, ny, nz] = grid.dims;
    let mut surface = vec![false; grid.len()];
    let reach = grid.h * 0.866;
    let cell = |v: DVec3, axis: usize| -> isize {
        ((v[axis] - grid.origin[axis]) / grid.h).floor() as isize
    };
    for tri in triangles {
        let a = DVec3::from_array(vertices[tri[0] as usize]);
        let b = DVec3::from_array(vertices[tri[1] as usize]);
        let c = DVec3::from_array(vertices[tri[2] as usize]);
        let lo = a.min(b).min(c);
        let hi = a.max(b).max(c);
        let x0 = (cell(lo, 0) - 1).max(0) as usize;
        let x1 = (cell(hi, 0) + 2).min(nx as isize - 1) as usize;
        let y0 = (cell(lo, 1) - 1).max(0) as usize;
        let y1 = (cell(hi, 1) + 2).min(ny as isize - 1) as usize;
        let z0 = (cell(lo, 2) - 1).max(0) as usize;
        let z1 = (cell(hi, 2) + 2).min(nz as isize - 1) as usize;
        for iz in z0..=z1 {
            for iy in y0..=y1 {
                for ix in x0..=x1 {
                    let i = grid.idx(ix, iy, iz);
                    if surface[i] {
                        continue;
                    }
                    let centre = grid.origin
                        + DVec3::new(ix as f64 + 0.5, iy as f64 + 0.5, iz as f64 + 0.5) * grid.h;
                    if point_tri_distance(centre, a, b, c) <= reach {
                        surface[i] = true;
                    }
                }
            }
        }
    }
    // Close seam gaps: the body's part junctions (neck, wrists, ankles) are unwelded
    // and leave sub-voxel holes the exterior flood would leak through. Dilate the
    // surface shell by `seal_voxels` (every cell within that distance of a surface
    // cell becomes surface), sealing gaps up to ~2*seal_voxels wide.
    if seal_voxels > 0 {
        let dist_surf = edt_to_source(grid, &surface);
        let seal_h = (seal_voxels as f64 * grid.h) as f32;
        for i in 0..surface.len() {
            if dist_surf[i] <= seal_h {
                surface[i] = true;
            }
        }
    }
    // Flood the exterior from the grid boundary through non-surface cells
    // (6-connectivity). Any cell the flood cannot reach is enclosed flesh.
    let mut exterior = vec![false; surface.len()];
    let mut stack: Vec<usize> = Vec::new();
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                let boundary =
                    ix == 0 || iy == 0 || iz == 0 || ix == nx - 1 || iy == ny - 1 || iz == nz - 1;
                if !boundary {
                    continue;
                }
                let i = grid.idx(ix, iy, iz);
                if !surface[i] && !exterior[i] {
                    exterior[i] = true;
                    stack.push(i);
                }
            }
        }
    }
    let nxy = nx * ny;
    while let Some(i) = stack.pop() {
        let x = i % nx;
        let y = (i / nx) % ny;
        let z = i / nxy;
        let mut try_push = |nx_i: usize, ny_i: usize, nz_i: usize, stack: &mut Vec<usize>| {
            if nx_i >= nx || ny_i >= ny || nz_i >= nz {
                return;
            }
            let n = grid.idx(nx_i, ny_i, nz_i);
            if !surface[n] && !exterior[n] {
                exterior[n] = true;
                stack.push(n);
            }
        };
        if x > 0 {
            try_push(x - 1, y, z, &mut stack);
        }
        if x + 1 < nx {
            try_push(x + 1, y, z, &mut stack);
        }
        if y > 0 {
            try_push(x, y - 1, z, &mut stack);
        }
        if y + 1 < ny {
            try_push(x, y + 1, z, &mut stack);
        }
        if z > 0 {
            try_push(x, y, z - 1, &mut stack);
        }
        if z + 1 < nz {
            try_push(x, y, z + 1, &mut stack);
        }
    }
    // Flesh is everything the exterior flood did not reach (surface shell included).
    exterior.iter().map(|&e| !e).collect()
}

/// One-dimensional squared distance transform (Felzenszwalb–Huttenlocher).
///
/// `f` holds squared distances (0 at a source cell, a large sentinel elsewhere);
/// the result is the lower envelope of parabolas, i.e. the squared distance to
/// the nearest source cell along this line.
fn dt1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let mut d = vec![0.0_f32; n];
    if n == 0 {
        return d;
    }
    let mut v = vec![0_usize; n];
    let mut z = vec![0.0_f32; n + 1];
    let mut k = 0_usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let fq = f[q] + (q * q) as f32;
        let mut s;
        loop {
            let vk = v[k];
            s = (fq - (f[vk] + (vk * vk) as f32)) / (2.0 * (q as f32 - vk as f32));
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let dq = q as f32 - v[k] as f32;
        *out = dq * dq + f[v[k]];
    }
    d
}

/// Distance (metres) from every cell to the nearest source cell (`source == true`).
/// Three separable squared-distance passes (x, y, z); source cells carry 0.
pub fn edt_to_source(grid: &Grid, source: &[bool]) -> Vec<f32> {
    let [nx, ny, nz] = grid.dims;
    let big = (nx * nx + ny * ny + nz * nz) as f32 * 4.0 + 1.0;
    let mut f: Vec<f32> = source.iter().map(|&b| if b { 0.0 } else { big }).collect();
    let mut line = vec![0.0_f32; nx.max(ny).max(nz)];
    // Pass along x.
    for iz in 0..nz {
        for iy in 0..ny {
            let base = grid.idx(0, iy, iz);
            line[..nx].copy_from_slice(&f[base..base + nx]);
            let d = dt1d(&line[..nx]);
            f[base..base + nx].copy_from_slice(&d[..nx]);
        }
    }
    // Pass along y.
    for iz in 0..nz {
        for ix in 0..nx {
            for iy in 0..ny {
                line[iy] = f[grid.idx(ix, iy, iz)];
            }
            let d = dt1d(&line[..ny]);
            for iy in 0..ny {
                f[grid.idx(ix, iy, iz)] = d[iy];
            }
        }
    }
    // Pass along z.
    for iy in 0..ny {
        for ix in 0..nx {
            for iz in 0..nz {
                line[iz] = f[grid.idx(ix, iy, iz)];
            }
            let d = dt1d(&line[..nz]);
            for iz in 0..nz {
                f[grid.idx(ix, iy, iz)] = d[iz];
            }
        }
    }
    let h2 = (grid.h * grid.h) as f32;
    f.iter().map(|&v| (v * h2).sqrt()).collect()
}

/// Inscribed radius of the flesh: distance from each inside cell to the nearest
/// outside cell. Outside cells carry 0.
pub fn distance_field(grid: &Grid, inside: &[bool]) -> Vec<f32> {
    let outside: Vec<bool> = inside.iter().map(|&b| !b).collect();
    edt_to_source(grid, &outside)
}

/// The medial field: grid, inside mask, inscribed radius per cell, and the ridge
/// (medial core) cells whose radius is a local maximum over the 26 neighbourhood.
pub struct MedialField {
    /// The voxel grid.
    pub grid: Grid,
    /// Inside mask.
    pub inside: Vec<bool>,
    /// Inscribed radius (metres) per cell; 0 outside.
    pub radius: Vec<f32>,
    /// Ridge cells: inside, radius > 0, and >= every 26-neighbour's radius.
    pub core: Vec<usize>,
    /// Curve skeleton: the inside thinned to a 1-cell-wide medial centreline tree.
    pub skeleton: Vec<bool>,
}

impl MedialField {
    /// Build the field from a closed mesh at voxel edge length `h` (no seam closing).
    pub fn from_mesh(vertices: &[[f64; 3]], triangles: &[[u32; 3]], h: f64) -> Self {
        Self::from_mesh_sealed(vertices, triangles, h, 0)
    }

    /// Build the field from a mesh whose part seams may be unwelded, closing gaps up
    /// to ~`2*seal_voxels` wide so the exterior flood cannot leak into the flesh.
    pub fn from_mesh_sealed(
        vertices: &[[f64; 3]],
        triangles: &[[u32; 3]],
        h: f64,
        seal_voxels: usize,
    ) -> Self {
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        for v in vertices {
            let p = DVec3::from_array(*v);
            min = min.min(p);
            max = max.max(p);
        }
        let grid = Grid::from_bounds(min, max, h);
        let inside = occupancy(&grid, vertices, triangles, seal_voxels);
        let radius = distance_field(&grid, &inside);
        let core = ridge(&grid, &inside, &radius);
        let skeleton = curve_skeleton(&grid, &inside, &radius);
        Self {
            grid,
            inside,
            radius,
            core,
            skeleton,
        }
    }

    /// Inscribed radius (metres) at a world point, sampled at the nearest cell.
    pub fn radius_at(&self, point: DVec3) -> f64 {
        let [nx, ny, nz] = self.grid.dims;
        let lx = ((point.x - self.grid.origin.x) / self.grid.h).round() as isize;
        let ly = ((point.y - self.grid.origin.y) / self.grid.h).round() as isize;
        let lz = ((point.z - self.grid.origin.z) / self.grid.h).round() as isize;
        if lx < 0 || ly < 0 || lz < 0 {
            return 0.0;
        }
        let (x, y, z) = (lx as usize, ly as usize, lz as usize);
        if x >= nx || y >= ny || z >= nz {
            return 0.0;
        }
        f64::from(self.radius[self.grid.idx(x, y, z)])
    }

    /// Whether a world point is inside the flesh (nearest cell inside).
    pub fn is_inside(&self, point: DVec3) -> bool {
        let [nx, ny, nz] = self.grid.dims;
        let lx = ((point.x - self.grid.origin.x) / self.grid.h).round() as isize;
        let ly = ((point.y - self.grid.origin.y) / self.grid.h).round() as isize;
        let lz = ((point.z - self.grid.origin.z) / self.grid.h).round() as isize;
        if lx < 0 || ly < 0 || lz < 0 {
            return false;
        }
        let (x, y, z) = (lx as usize, ly as usize, lz as usize);
        if x >= nx || y >= ny || z >= nz {
            return false;
        }
        self.inside[self.grid.idx(x, y, z)]
    }

    /// The connected flesh cross-sections at grid height `iy` (4-connectivity in
    /// the XZ plane). Each component reports its inscribed centre (the deepest
    /// cell — the medial-axis point of that cross-section), its inscribed radius,
    /// and its mean x (for left/central/right classification).
    pub fn row_components(&self, iy: usize) -> Vec<CrossSection> {
        let [nx, _ny, nz] = self.grid.dims;
        let mut label = vec![0_u32; nx * nz];
        let mut out = Vec::new();
        let mut next = 0_u32;
        for iz in 0..nz {
            for ix in 0..nx {
                let li = ix + nx * iz;
                if label[li] != 0 || !self.inside[self.grid.idx(ix, iy, iz)] {
                    continue;
                }
                next += 1;
                let mut stack = vec![(ix, iz)];
                label[li] = next;
                let mut best_r = 0.0_f32;
                let mut best = (ix, iz);
                let mut sum_x = 0.0_f64;
                let mut count = 0_u32;
                while let Some((x, z)) = stack.pop() {
                    let r = self.radius[self.grid.idx(x, iy, z)];
                    if r > best_r {
                        best_r = r;
                        best = (x, z);
                    }
                    sum_x += self.grid.world(x, iy, z).x;
                    count += 1;
                    for (dx, dz) in [(-1_i32, 0_i32), (1, 0), (0, -1), (0, 1)] {
                        let xx = x as i32 + dx;
                        let zz = z as i32 + dz;
                        if xx < 0 || zz < 0 {
                            continue;
                        }
                        let (xx, zz) = (xx as usize, zz as usize);
                        if xx >= nx || zz >= nz {
                            continue;
                        }
                        let nl = xx + nx * zz;
                        if label[nl] == 0 && self.inside[self.grid.idx(xx, iy, zz)] {
                            label[nl] = next;
                            stack.push((xx, zz));
                        }
                    }
                }
                let centre = self.grid.world(best.0, iy, best.1);
                out.push(CrossSection {
                    centre,
                    radius: f64::from(best_r),
                    mean_x: sum_x / f64::from(count),
                    cells: count,
                });
            }
        }
        out
    }
}

/// One connected flesh cross-section at a height: its medial inscribed centre.
pub struct CrossSection {
    /// The deepest inscribed centre (medial-axis point) of the cross-section.
    pub centre: DVec3,
    /// Inscribed radius at that centre, metres.
    pub radius: f64,
    /// Mean x of the component's cells, metres (left/central/right classification).
    pub mean_x: f64,
    /// Cell count of the component.
    pub cells: u32,
}

/// A joint's placement template: its chain, arc-length fraction along that chain
/// (0..1), and the chain's proximal/distal endpoint names.
pub struct JointTemplate {
    /// Chain name (`spine`, `arm_left`, `finger_left_index`, …).
    pub chain: String,
    /// Arc-length fraction along the chain, 0 at proximal, 1 at distal.
    pub fraction: f64,
    /// Chain proximal endpoint name.
    pub proximal: String,
    /// Chain distal endpoint name.
    pub distal: String,
}

/// Per-joint chain fractions from a skeleton's own rest positions (the authority).
///
/// Shared anchors (Hand, ToeBase) keep their primary limb chain (arm/leg); the
/// finger/toe chains place only their own digits. This is Mixamo's anatomy expressed
/// as fractions, independent of any native seat.
pub fn chain_fractions(palette: &[String], rest: &[DVec3]) -> BTreeMap<usize, JointTemplate> {
    let mut out: BTreeMap<usize, JointTemplate> = BTreeMap::new();
    for (name, names) in mixamo_chains() {
        let joints: Vec<usize> = names
            .iter()
            .filter_map(|n| palette.iter().position(|p| p == n))
            .collect();
        if joints.len() < 2 {
            continue;
        }
        let mut arc = vec![0.0_f64; joints.len()];
        for k in 1..joints.len() {
            arc[k] = arc[k - 1] + rest[joints[k]].distance(rest[joints[k - 1]]);
        }
        let total = arc.last().copied().unwrap_or(0.0);
        for (k, &j) in joints.iter().enumerate() {
            let fraction = if total > 1e-9 { arc[k] / total } else { 0.0 };
            out.entry(j).or_insert(JointTemplate {
                chain: name.clone(),
                fraction,
                proximal: names[0].clone(),
                distal: names.last().unwrap().clone(),
            });
        }
    }
    out
}

/// The nearest medial-skeleton cell to a world point (the snap onto the flesh
/// centreline). Returns the cell centre and its inscribed radius, or None when the
/// skeleton has no cell within `max_dist` (a thin region the grid did not resolve).
pub fn snap_to_medial(field: &MedialField, point: DVec3, max_dist: f64) -> Option<(DVec3, f64)> {
    let [nx, ny, _nz] = field.grid.dims;
    let nxy = nx * ny;
    let mut best = (max_dist, None::<DVec3>, 0.0_f64);
    for (i, &sk) in field.skeleton.iter().enumerate() {
        if !sk {
            continue;
        }
        let x = i % nx;
        let y = (i / nx) % ny;
        let z = i / nxy;
        let p = field.grid.world(x, y, z);
        let d = point.distance(p);
        if d < best.0 {
            best = (d, Some(p), f64::from(field.radius[i]));
        }
    }
    best.1.map(|p| (p, best.2))
}

/// Signed surface distance of a point, millimetres: the true point-triangle
/// distance to the nearest face, signed by ray parity voted over the six axis
/// directions. A single +X ray degenerates where it runs along a finger row or
/// grazes a seam (measured: LeftHandPinky1 counts 10 crossings on +X but 1 on
/// every other axis), so inside is the majority vote; a 3-3 tie falls back to
/// +X and is visible in the vote count. This is the containment authority the
/// acceptance gate should use; the voxel field's nearest-cell probe flips on
/// surface-coincident joints at 3 mm resolution. A point on the surface reads
/// ±hundredths of a millimetre — parity is undecidable there, hence the gate's
/// 1 mm tolerance.
pub fn surface_probe(
    vertices: &[[f64; 3]],
    triangles: &[[u32; 3]],
    point: DVec3,
) -> (f64, bool, u32) {
    let mut best = f64::INFINITY;
    let mut crossings = [0_u32; 6];
    let dirs = [
        DVec3::X,
        -DVec3::X,
        DVec3::Y,
        -DVec3::Y,
        DVec3::Z,
        -DVec3::Z,
    ];
    for tri in triangles {
        let a = DVec3::from_array(vertices[tri[0] as usize]);
        let b = DVec3::from_array(vertices[tri[1] as usize]);
        let c = DVec3::from_array(vertices[tri[2] as usize]);
        best = best.min(point_tri_distance(point, a, b, c));
        for (k, dir) in dirs.iter().enumerate() {
            if ray_triangle(point, *dir, a, b, c).is_some() {
                crossings[k] += 1;
            }
        }
    }
    let votes: u32 = crossings.iter().map(|&c| u32::from(c % 2 == 1)).sum();
    let inside = if votes == 3 {
        crossings[0] % 2 == 1
    } else {
        votes > 3
    };
    (
        best * 1000.0 * if inside { -1.0 } else { 1.0 },
        inside,
        votes,
    )
}

fn ray_triangle(origin: DVec3, direction: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<f64> {
    let edge1 = b - a;
    let edge2 = c - a;
    let h = direction.cross(edge2);
    let determinant = edge1.dot(h);
    if determinant.abs() < 1e-12 {
        return None;
    }
    let from_a = origin - a;
    let u = from_a.dot(h) / determinant;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = from_a.cross(edge1);
    let v = q.dot(direction) / determinant;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge2.dot(q) / determinant;
    (distance > 1e-9).then_some(distance)
}

/// Nearest forward surface hit in metres; winding and direction magnitude do not matter.
/// A missing hit stays unresolved rather than inventing a closed surface. @req DECL0013
pub(crate) fn first_surface_hit(
    vertices: &[[f64; 3]],
    triangles: &[[u32; 3]],
    origin: DVec3,
    direction: DVec3,
) -> Option<f64> {
    let direction = direction.try_normalize()?;
    triangles
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = triangle.map(|i| DVec3::from_array(vertices[i as usize]));
            ray_triangle(origin, direction, a, b, c)
        })
        .min_by(f64::total_cmp)
}

/// The closest point on the flesh surface to `point` (nearest triangle projection,
/// Ericson). Used to seat the zero-weight leaf-tip markers (fingertip / toe tip / crown)
/// on the skin: they define the chain's terminal extent and carry no weight, so placing
/// them on the surface keeps them within the gate tolerance instead of past it.
pub fn closest_surface_point(vertices: &[[f64; 3]], triangles: &[[u32; 3]], point: DVec3) -> DVec3 {
    let mut best = f64::INFINITY;
    let mut out = point;
    for tri in triangles {
        let a = DVec3::from_array(vertices[tri[0] as usize]);
        let b = DVec3::from_array(vertices[tri[1] as usize]);
        let c = DVec3::from_array(vertices[tri[2] as usize]);
        let (d, p) = closest_point_on_tri(point, a, b, c);
        if d < best {
            best = d;
            out = p;
        }
    }
    out
}

/// Closest point on a triangle to `p` and its distance (Ericson, Real-Time Collision
/// Detection): the barycentric region test, clamped to the nearest edge/vertex.
fn closest_point_on_tri(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (f64, DVec3) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (ap.length(), a);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (bp.length(), b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        let q = a + ab * v;
        return ((p - q).length(), q);
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (cp.length(), c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        let q = a + ac * w;
        return ((p - q).length(), q);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        let q = b + (c - b) * w;
        return ((p - q).length(), q);
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    let q = a + ab * v + ac * w;
    ((p - q).length(), q)
}

/// Half 1 — the anatomy template of a skeleton's own body, as data.
///
/// One row per joint: its chain, arc-length fraction (0 at the root end, 1 at the
/// terminal), the medial inscribed radius at its position, and its offset from the
/// chain axis. Fractions are the skeleton's authority; radii/offsets are measured
/// on the flesh medial axis. Branch points name which chains meet at which joint.
pub fn anatomy_template(field: &MedialField, palette: &[String], rest: &[DVec3]) -> Value {
    let fracs = chain_fractions(palette, rest);
    let chains = mixamo_chains();
    let axis_of = |chain: &str| -> Option<(DVec3, DVec3)> {
        let (_, names) = chains.iter().find(|(n, _)| n == chain)?;
        let a = palette.iter().position(|p| p == &names[0])?;
        let b = palette.iter().position(|p| p == names.last().unwrap())?;
        Some((rest[a], rest[b]))
    };
    let mut rows = Vec::new();
    for (j, name) in palette.iter().enumerate() {
        let Some(t) = fracs.get(&j) else { continue };
        let radius_mm = field.radius_at(rest[j]) * 1000.0;
        let offset_mm = axis_of(&t.chain)
            .map(|(a, b)| {
                let ab = b - a;
                let len = ab.length();
                if len < 1e-9 {
                    0.0
                } else {
                    let d = ab / len;
                    let tt = (rest[j] - a).dot(d).clamp(0.0, len);
                    (rest[j] - (a + d * tt)).length() * 1000.0
                }
            })
            .unwrap_or(0.0);
        rows.push(json!({
            "name": name,
            "chain": t.chain,
            "fraction": t.fraction,
            "medial_radius_mm": radius_mm,
            "axis_offset_mm": offset_mm,
            "position_mm": [rest[j].x * 1000.0, rest[j].y * 1000.0, rest[j].z * 1000.0],
        }));
    }
    let branch_points = json!([
        {"joint": "mixamorig:Hips", "chains": ["spine", "leg_left", "leg_right"]},
        {"joint": "mixamorig:LeftShoulder", "chains": ["spine", "arm_left"]},
        {"joint": "mixamorig:RightShoulder", "chains": ["spine", "arm_right"]},
        {"joint": "mixamorig:LeftHand", "chains": ["arm_left", "finger_left_thumb", "finger_left_index", "finger_left_middle", "finger_left_ring", "finger_left_pinky"]},
        {"joint": "mixamorig:RightHand", "chains": ["arm_right", "finger_right_thumb", "finger_right_index", "finger_right_middle", "finger_right_ring", "finger_right_pinky"]},
        {"joint": "mixamorig:LeftToeBase", "chains": ["leg_left", "toe_left"]},
        {"joint": "mixamorig:RightToeBase", "chains": ["leg_right", "toe_right"]},
    ]);
    json!({
        "method": "medial axis of the skeleton's own flesh; fraction = chain arc-length (0 root .. 1 terminal)",
        "joints": rows,
        "branch_points": branch_points,
    })
}

/// The body's own flesh centrelines and branch landmarks, measured on the medial
/// axis alone — no native joint, name or seat enters. @req DECL0008
///
/// The torso medial axis is a sheet, so the spine is the deepest inscribed centre
/// of the central cross-section component per height (a clean midline even where
/// the thinned skeleton stays sheet-like). Each limb is a lateral cross-section
/// component tracked by centre continuity from its seed (shoulder down to the hand,
/// foot up to the hip), so an A-pose arm is followed as easily as a straight one.
/// Branch landmarks (pelvis centre, neck, skull centre, wrist, ankle, toe base) are
/// read from the radius profile and the skeleton's branch junctions on those paths.
pub struct FleshCentrelines {
    /// Central torso midline, pelvis floor -> crown, with inscribed radius.
    pub spine: Vec<(DVec3, f64)>,
    /// Left arm, shoulder -> hand tip (full lateral track).
    pub arm_left: Vec<(DVec3, f64)>,
    /// Right arm, shoulder -> hand tip.
    pub arm_right: Vec<(DVec3, f64)>,
    /// Left leg, hip -> foot tip (full lateral track).
    pub leg_left: Vec<(DVec3, f64)>,
    /// Right leg, hip -> foot tip.
    pub leg_right: Vec<(DVec3, f64)>,
    /// Body midline x (symmetry plane), metres.
    pub cx: f64,
    /// Pelvis centre: deepest inscribed centre of the lower torso lobe (hip anchor).
    pub hip_anchor: Option<(DVec3, f64)>,
    /// Neck branch: the radius minimum between the chest and head lobes.
    pub neck_branch: Option<(DVec3, f64)>,
    /// Skull centre: deepest inscribed centre of the head lobe.
    pub head_centre: Option<(DVec3, f64)>,
    /// Crown: the topmost midline point.
    pub crown: Option<(DVec3, f64)>,
    /// Left wrist branch (arm -> hand/fingers), with inscribed radius.
    pub wrist_left: Option<(DVec3, f64)>,
    /// Right wrist branch.
    pub wrist_right: Option<(DVec3, f64)>,
    /// Left ankle (the leg -> foot bend), with inscribed radius.
    pub ankle_left: Option<(DVec3, f64)>,
    /// Right ankle.
    pub ankle_right: Option<(DVec3, f64)>,
    /// Left toe base (ball of the foot, where the toes branch).
    pub toe_base_left: Option<(DVec3, f64)>,
    /// Right toe base.
    pub toe_base_right: Option<(DVec3, f64)>,
    /// Left shoulder branch (where the arm leaves the torso), with inscribed radius.
    pub shoulder_left: Option<(DVec3, f64)>,
    /// Right shoulder branch.
    pub shoulder_right: Option<(DVec3, f64)>,
    /// Left hip branch (where the leg leaves the pelvis), at the femoral head.
    pub hip_left: Option<(DVec3, f64)>,
    /// Right hip branch, at the femoral head.
    pub hip_right: Option<(DVec3, f64)>,
    /// Left femoral-head measurement: the crotch anchor and its inscribed radius, before the
    /// pair is mirrored.
    pub femoral_head_left: Option<(DVec3, f64)>,
    /// Right femoral-head measurement, same shape.
    pub femoral_head_right: Option<(DVec3, f64)>,
}

impl FleshCentrelines {
    /// Arc length of a centreline polyline.
    pub fn arc(pts: &[(DVec3, f64)]) -> f64 {
        pts.windows(2).map(|w| w[0].0.distance(w[1].0)).sum()
    }

    /// The point at fraction `f` of arc length along a centreline, interpolated.
    pub fn point_at(pts: &[(DVec3, f64)], f: f64) -> (DVec3, f64) {
        if pts.is_empty() {
            return (DVec3::ZERO, 0.0);
        }
        if pts.len() == 1 {
            return pts[0];
        }
        let total = Self::arc(pts);
        let target = f.clamp(0.0, 1.0) * total;
        let mut acc = 0.0;
        for k in 1..pts.len() {
            let seg = pts[k].0.distance(pts[k - 1].0);
            if acc + seg >= target {
                let u = if seg > 1e-9 {
                    (target - acc) / seg
                } else {
                    0.0
                };
                let p = pts[k - 1].0.lerp(pts[k].0, u);
                let r = pts[k - 1].1 + (pts[k].1 - pts[k - 1].1) * u;
                return (p, r);
            }
            acc += seg;
        }
        *pts.last().unwrap()
    }

    /// The sub-path of `pts` between the points nearest `a` and `b` (inclusive),
    /// ordered proximal -> distal. Cuts a placement axis at branch landmarks.
    pub fn sub_path(pts: &[(DVec3, f64)], a: DVec3, b: DVec3) -> Vec<(DVec3, f64)> {
        if pts.is_empty() {
            return Vec::new();
        }
        let nearest = |p: DVec3| {
            pts.iter()
                .enumerate()
                .min_by(|x, y| x.1.0.distance(p).total_cmp(&y.1.0.distance(p)))
                .map(|(i, _)| i)
                .unwrap_or(0)
        };
        let (ia, ib) = (nearest(a), nearest(b));
        let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
        pts[lo..=hi].to_vec()
    }

    /// Spine placement axis: pelvis centre -> skull centre along the midline.
    pub fn spine_axis(&self) -> Vec<(DVec3, f64)> {
        match (self.hip_anchor, self.head_centre) {
            (Some((a, _)), Some((b, _))) => Self::sub_path(&self.spine, a, b),
            _ => self.spine.clone(),
        }
    }

    /// Spine length pelvis centre -> skull centre, metres.
    pub fn spine_length(&self) -> f64 {
        Self::arc(&self.spine_axis())
    }

    /// A limb placement axis: the proximal branch point, then the tracked centreline cut
    /// at the distal branch (wrist / toe base). The branch sits in the torso mass off the
    /// track, so it is prepended; the track supplies the through-elbow/knee path.
    fn limb_axis(
        track: &[(DVec3, f64)],
        branch: Option<(DVec3, f64)>,
        distal: Option<(DVec3, f64)>,
    ) -> Vec<(DVec3, f64)> {
        let prox = track.first().map(|(p, _)| *p);
        let cut = match (prox, distal) {
            (Some(a), Some((d, _))) => Self::sub_path(track, a, d),
            _ => track.to_vec(),
        };
        match branch {
            Some((b, br)) => {
                if cut.first().is_some_and(|(p, _)| p.distance(b) < 1e-6) {
                    cut
                } else {
                    std::iter::once((b, br)).chain(cut).collect()
                }
            }
            None => cut,
        }
    }

    /// Left arm placement axis: shoulder branch -> wrist through the elbow.
    pub fn arm_axis_left(&self) -> Vec<(DVec3, f64)> {
        Self::limb_axis(&self.arm_left, self.shoulder_left, self.wrist_left)
    }

    /// Right arm placement axis: shoulder branch -> wrist through the elbow.
    pub fn arm_axis_right(&self) -> Vec<(DVec3, f64)> {
        Self::limb_axis(&self.arm_right, self.shoulder_right, self.wrist_right)
    }

    /// Left leg placement axis: hip branch -> toe base through the knee and ankle.
    pub fn leg_axis_left(&self) -> Vec<(DVec3, f64)> {
        Self::limb_axis(&self.leg_left, self.hip_left, self.toe_base_left)
    }

    /// Right leg placement axis: hip branch -> toe base through the knee and ankle.
    pub fn leg_axis_right(&self) -> Vec<(DVec3, f64)> {
        Self::limb_axis(&self.leg_right, self.hip_right, self.toe_base_right)
    }
}

/// The symmetry plane x: the mean x of the deep inscribed centres (the torso core).
fn symmetry_plane(field: &MedialField) -> f64 {
    let [nx, ny, _nz] = field.grid.dims;
    let nxy = nx * ny;
    let mut sum = 0.0;
    let mut n = 0;
    for (i, &sk) in field.skeleton.iter().enumerate() {
        if !sk || field.radius[i] <= 0.05 {
            continue;
        }
        let x = i % nx;
        let y = (i / nx) % ny;
        let z = i / nxy;
        sum += field.grid.world(x, y, z).x;
        n += 1;
    }
    if n > 0 { sum / f64::from(n) } else { 0.0 }
}

/// The central torso midline: the deepest inscribed cell within a narrow band on the
/// symmetry plane, tracked by centre continuity from the pelvis up to the crown. The
/// continuity keeps the path in one inscribed pocket so the torso sheet cannot zig-zag
/// it anterior/posterior; the band keeps it off the lateral hip sockets and breasts.
fn central_midline(field: &MedialField, cx: f64) -> Vec<(DVec3, f64)> {
    let [nx, ny, nz] = field.grid.dims;
    let band = 0.03;
    let cand: Vec<Option<(DVec3, f64)>> = (0..ny)
        .map(|iy| {
            let mut best: Option<(f64, DVec3)> = None;
            for iz in 0..nz {
                for ix in 0..nx {
                    let p = field.grid.world(ix, iy, iz);
                    if (p.x - cx).abs() > band {
                        continue;
                    }
                    let r = f64::from(field.radius[field.grid.idx(ix, iy, iz)]);
                    if r > 0.02 && best.is_none_or(|(br, _)| r > br) {
                        best = Some((r, p));
                    }
                }
            }
            best.map(|(r, p)| (p, r))
        })
        .collect();
    let Some(seed) = (0..ny).find(|&iy| cand[iy].is_some()) else {
        return Vec::new();
    };
    let mut track = vec![cand[seed].unwrap()];
    let mut iy = seed as i32 + 1;
    while (iy as usize) < ny {
        let last = track.last().unwrap().0;
        let mut next: Option<((DVec3, f64), i32)> = None;
        for ahead in 0..6 {
            let j = iy + ahead;
            if j as usize >= ny {
                break;
            }
            if let Some(c) = cand[j as usize] {
                let d = c.0.distance(last);
                if d < 0.06 && next.is_none_or(|(p, _)| d < p.0.distance(last)) {
                    next = Some((c, j));
                }
            }
        }
        match next {
            Some((c, j)) => {
                track.push(c);
                iy = j + 1;
            }
            None => break,
        }
    }
    track
}

/// Track a lateral limb cross-section component by centre continuity. The arm seeds at
/// the shoulder (topmost lateral component) and walks down to the hand; the leg seeds at
/// the foot (bottommost) and walks up to the hip — the foot is unambiguous, whereas the
/// A-pose hand hangs below the hip and would contaminate a height-bounded leg seed. Each
/// step picks the nearest centre on the same side within a look-ahead, so the track
/// follows the rest pose however far lateral the limb travels; `bridge` is the maximum
/// centre jump (larger for the leg's foot -> ankle bend).
fn track_limb(
    field: &MedialField,
    cx: f64,
    side: f64,
    upper: bool,
    bridge: f64,
) -> Vec<(DVec3, f64)> {
    let ny = field.grid.dims[1];
    let lateral: Vec<Option<(DVec3, f64)>> = (0..ny)
        .map(|iy| {
            field
                .row_components(iy)
                .into_iter()
                .filter(|c| (c.mean_x - cx) * side > 0.05 && c.radius > 0.01)
                .max_by(|a, b| a.radius.total_cmp(&b.radius))
                .map(|c| (c.centre, c.radius))
        })
        .collect();
    let seed = if upper {
        (0..ny).rev().find(|&iy| lateral[iy].is_some())
    } else {
        (0..ny).find(|&iy| lateral[iy].is_some())
    };
    let Some(seed) = seed else { return Vec::new() };
    let mut track = vec![lateral[seed].unwrap()];
    let step: i32 = if upper { -1 } else { 1 };
    let mut iy = seed as i32 + step;
    while iy >= 0 && (iy as usize) < ny {
        let last = track.last().unwrap().0;
        let mut next: Option<((DVec3, f64), i32)> = None;
        for ahead in 0..8 {
            let j = iy + step * ahead;
            if j < 0 || j as usize >= ny {
                break;
            }
            if let Some(c) = lateral[j as usize] {
                let d = c.0.distance(last);
                if d < bridge && next.is_none_or(|(p, _)| d < p.0.distance(last)) {
                    next = Some((c, j));
                }
            }
        }
        match next {
            Some((c, j)) => {
                track.push(c);
                iy = j + step;
            }
            None => break,
        }
    }
    if !upper {
        track.reverse(); // hip -> foot, proximal first
    }
    track
}

/// The branch junction where a limb leaves the torso: the skeleton junction nearest the
/// limb track's proximal end among those inside the torso band (|x - cx| < 0.13). The
/// shoulder and hip branches sit in the torso mass, where the cross-section does not
/// separate them, so the thinned skeleton's junction marks them instead.
fn proximal_branch(
    junctions: &[(DVec3, f64, usize)],
    track: &[(DVec3, f64)],
    cx: f64,
) -> Option<(DVec3, f64)> {
    let prox = track.first()?.0;
    junctions
        .iter()
        .filter(|(p, _, _)| (p.x - cx).abs() < 0.13 && p.distance(prox) < 0.20)
        .min_by(|a, b| a.0.distance(prox).total_cmp(&b.0.distance(prox)))
        .map(|(p, r, _)| (*p, *r))
}

/// Skeleton branch junctions: cells with >= 3 skeleton neighbours in the
/// 26-neighbourhood, with inscribed radius. The limbs are clean curves, so a junction
/// near a limb's distal end is its branch point (wrist, toe base).
fn skeleton_junctions(field: &MedialField) -> Vec<(DVec3, f64, usize)> {
    let [nx, ny, nz] = field.grid.dims;
    let nxy = nx * ny;
    let skel: Vec<usize> = (0..field.skeleton.len())
        .filter(|&i| field.skeleton[i])
        .collect();
    let set: std::collections::BTreeSet<usize> = skel.iter().copied().collect();
    let mut out = Vec::new();
    for &i in &skel {
        let x = (i % nx) as i32;
        let y = ((i / nx) % ny) as i32;
        let z = (i / nxy) as i32;
        let mut deg = 0;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                    if xx < 0 || yy < 0 || zz < 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (xx as usize, yy as usize, zz as usize);
                    if xx >= nx || yy >= ny || zz >= nz {
                        continue;
                    }
                    if set.contains(&field.grid.idx(xx, yy, zz)) {
                        deg += 1;
                    }
                }
            }
        }
        if deg >= 3 {
            out.push((
                field.grid.world(x as usize, y as usize, z as usize),
                f64::from(field.radius[i]),
                deg,
            ));
        }
    }
    out
}

/// The branch junction nearest the distal end of a limb track (wrist / toe base):
/// the highest-degree junction within `reach` of the track's last few points.
fn distal_junction(
    junctions: &[(DVec3, f64, usize)],
    track: &[(DVec3, f64)],
    reach: f64,
) -> Option<(DVec3, f64)> {
    let tail: Vec<DVec3> = track.iter().rev().take(12).map(|(p, _)| *p).collect();
    if tail.is_empty() {
        return None;
    }
    junctions
        .iter()
        .filter(|(p, _, _)| tail.iter().any(|t| t.distance(*p) < reach))
        .max_by(|a, b| a.2.cmp(&b.2).then(a.1.total_cmp(&b.1)))
        .map(|(p, r, _)| (*p, *r))
}

/// The ankle: the sharpest bend on the leg track (the leg tube turns into the foot),
/// measured as the maximum direction change over a window in the distal half.
fn ankle_bend(track: &[(DVec3, f64)]) -> Option<(DVec3, f64)> {
    if track.len() < 8 {
        return None;
    }
    let start = track.len() / 2;
    let w = 4;
    let mut best: Option<(f64, usize)> = None;
    for k in start..track.len().saturating_sub(w) {
        if k < w {
            continue;
        }
        let a = (track[k].0 - track[k - w].0).normalize_or_zero();
        let b = (track[k + w].0 - track[k].0).normalize_or_zero();
        let turn = 1.0 - a.dot(b).clamp(-1.0, 1.0);
        if best.is_none_or(|(t, _)| turn > t) {
            best = Some((turn, k));
        }
    }
    best.map(|(_, k)| track[k])
}

/// Index of the radius maximum on a polyline within `[lo, hi)` (the lobe centre).
fn lobe_max(pts: &[(DVec3, f64)], lo: usize, hi: usize) -> Option<usize> {
    (lo..hi.min(pts.len())).max_by(|&a, &b| pts[a].1.total_cmp(&pts[b].1))
}

/// Extract the flesh centrelines and branch landmarks from the medial field alone.
///
/// No native joint is read: the symmetry plane, the central midline, the limb tracks
/// and every branch landmark come from the inscribed-radius field and the thinned
/// skeleton's junctions. The result follows the body's rest pose.
pub fn flesh_centrelines(field: &MedialField) -> FleshCentrelines {
    let cx = symmetry_plane(field);
    let spine = central_midline(field, cx);
    let arm_left = track_limb(field, cx, -1.0, true, 0.07);
    let arm_right = track_limb(field, cx, 1.0, true, 0.07);
    let leg_left = track_limb(field, cx, -1.0, false, 0.12);
    let leg_right = track_limb(field, cx, 1.0, false, 0.12);
    let junctions = skeleton_junctions(field);

    // Spine landmarks from the radius profile of the clean midline. The pelvis is the
    // first significant bulge (> 60 mm) from the bottom — the crotch/perineum is thin and
    // must not be mistaken for it. The waist is the first minimum AFTER the pelvis bulge;
    // the pelvis centre is the deepest point below the waist. The neck is the deep minimum
    // between the chest and head lobes; the skull centre is the deepest point above it.
    let n = spine.len();
    let is_min = |k: usize| {
        k > 0 && k + 1 < n && spine[k].1 <= spine[k - 1].1 && spine[k].1 <= spine[k + 1].1
    };
    let is_max = |k: usize| {
        k > 0 && k + 1 < n && spine[k].1 >= spine[k - 1].1 && spine[k].1 >= spine[k + 1].1
    };
    let pelvis_peak = (1..n.saturating_sub(1)).find(|&k| spine[k].1 > 0.06 && is_max(k));
    let waist = pelvis_peak.and_then(|p| (p..n.saturating_sub(1)).find(|&k| is_min(k)));
    let hip_anchor = waist
        .and_then(|w| lobe_max(&spine, 0, w))
        .or(pelvis_peak)
        .or_else(|| lobe_max(&spine, 0, n / 3))
        .map(|k| spine[k]);
    let chest = waist.and_then(|w| lobe_max(&spine, w, n)).unwrap_or(n / 2);
    let neck = (chest..n.saturating_sub(1))
        .filter(|&k| is_min(k))
        .min_by(|&a, &b| spine[a].1.total_cmp(&spine[b].1));
    let neck_branch = neck.map(|k| spine[k]);
    let head_centre = neck
        .and_then(|nk| lobe_max(&spine, nk, n))
        .map(|k| spine[k]);
    let crown = spine.last().copied();

    let wrist_left = distal_junction(&junctions, &arm_left, 0.06);
    let wrist_right = distal_junction(&junctions, &arm_right, 0.06);
    let ankle_left = ankle_bend(&leg_left);
    let ankle_right = ankle_bend(&leg_right);
    let toe_base_left = distal_junction(&junctions, &leg_left, 0.06);
    let toe_base_right = distal_junction(&junctions, &leg_right, 0.06);
    let shoulder_left = proximal_branch(&junctions, &arm_left, cx);
    let shoulder_right = proximal_branch(&junctions, &arm_right, cx);
    let heads = femoral_heads(&leg_left, &leg_right, cx, hip_anchor);

    FleshCentrelines {
        spine,
        arm_left,
        arm_right,
        leg_left,
        leg_right,
        cx,
        hip_anchor,
        neck_branch,
        head_centre,
        crown,
        wrist_left,
        wrist_right,
        ankle_left,
        ankle_right,
        toe_base_left,
        toe_base_right,
        shoulder_left,
        shoulder_right,
        hip_left: heads.left,
        hip_right: heads.right,
        femoral_head_left: heads.left_measured,
        femoral_head_right: heads.right_measured,
    }
}

/// The femoral heads: the crotch anchor carried up one thigh radius, mirrored onto the
/// body's midline.
///
/// A standing body's hip joints are level and symmetric. The pelvis's medial skeleton is a
/// *sheet*, so its junctions sit in the upper thigh and pick a point 116 mm below the crotch
/// (measured on Emily: `y` 814.2 / 790.2 against a 1846 mm body) — a thigh junction, not a
/// hip — and the leg track itself stops there too, so neither supplies the level. The level
/// is the crotch anchor carried up one thigh radius; the lateral offset and the depth come
/// from the thigh's proximal centre. The pair is mirrored about the midline: a
/// one-sided error would otherwise be inherited by the hip branch. No native joint is read.
fn femoral_heads(
    left: &[(DVec3, f64)],
    right: &[(DVec3, f64)],
    cx: f64,
    hip_anchor: Option<(DVec3, f64)>,
) -> FemoralHeads {
    // The crotch anchor for one leg: the track is hip -> foot, so its first point is the
    // proximal end and its inscribed radius is the thigh's there.
    let raw = |track: &[(DVec3, f64)]| -> Option<(DVec3, f64)> {
        let (anchor, radius) = *track.first()?;
        if radius <= 0.0 {
            return None;
        }
        Some((anchor, radius))
    };
    let left_raw = raw(left);
    let right_raw = raw(right);
    if left_raw.is_none() && right_raw.is_none() {
        return FemoralHeads::default();
    }
    // The level: the crotch carried up one thigh radius — the hip ball sits at the top of the
    // thigh's proximal inscribed sphere. The leg track stops 116 mm below the crotch on this
    // body, so the level keeps that known residual and the caller reports it.
    let level = match (left_raw, right_raw) {
        (Some((l, rl)), Some((r, rr))) => (l.y + rl + (r.y + rr)) * 0.5,
        (Some((l, rl)), None) => l.y + rl,
        (None, Some((r, rr))) => r.y + rr,
        (None, None) => hip_anchor.map(|(p, _)| p.y).unwrap_or(0.0),
    };
    // One lateral offset, one depth: the mean of the two crotch anchors about the midline.
    let lateral = match (left_raw, right_raw) {
        (Some((l, _)), Some((r, _))) => ((cx - l.x).abs() + (r.x - cx).abs()) * 0.5,
        (Some((l, _)), None) => (cx - l.x).abs(),
        (None, Some((r, _))) => (r.x - cx).abs(),
        (None, None) => 0.0,
    };
    let depth = match (left_raw, right_raw) {
        (Some((l, _)), Some((r, _))) => (l.z + r.z) * 0.5,
        (Some((l, _)), None) => l.z,
        (None, Some((r, _))) => r.z,
        (None, None) => hip_anchor.map(|(p, _)| p.z).unwrap_or(0.0),
    };
    let radius_of = |m: Option<(DVec3, f64)>| m.map(|(_, radius)| radius).unwrap_or(0.0);
    let left_socket = DVec3::new(cx - lateral, level, depth);
    let right_socket = DVec3::new(cx + lateral, level, depth);
    FemoralHeads {
        left: left_raw.map(|_| (left_socket, radius_of(left_raw))),
        right: right_raw.map(|_| (right_socket, radius_of(right_raw))),
        left_measured: left_raw,
        right_measured: right_raw,
    }
}

/// Both femoral heads: the socket the seat uses per side, and the crotch measurement it was
/// derived from (for the report).
#[derive(Default)]
struct FemoralHeads {
    /// Left socket and its crotch inscribed radius.
    left: Option<(DVec3, f64)>,
    /// Right socket and its crotch inscribed radius.
    right: Option<(DVec3, f64)>,
    /// Left crotch anchor and its inscribed radius.
    left_measured: Option<(DVec3, f64)>,
    /// Right crotch anchor and its inscribed radius.
    right_measured: Option<(DVec3, f64)>,
}

/// Flesh digit centrelines: the medial-skeleton graph paths from an anchor junction
/// (wrist / toe base) to the `count` farthest reachable fingertip endpoints on a side.
/// Each path is a centreline inside the flesh by construction (skeleton cells are inside),
/// so digit joints placed along it stay in the hand/foot rather than following Mixamo's
/// T-pose digit direction into the air beside an A-pose hand. Paths are ordered by the
/// endpoint angle around the anchor so the digits fan in anatomical order. No native input.
pub fn digit_paths(
    field: &MedialField,
    anchor: DVec3,
    side: f64,
    cx: f64,
    count: usize,
) -> Vec<Vec<(DVec3, f64)>> {
    let [nx, ny, nz] = field.grid.dims;
    let nxy = nx * ny;
    let skel: Vec<usize> = (0..field.skeleton.len())
        .filter(|&i| field.skeleton[i])
        .collect();
    if skel.is_empty() {
        return Vec::new();
    }
    let slot: BTreeMap<usize, usize> = skel.iter().enumerate().map(|(s, &c)| (c, s)).collect();
    let world = |c: usize| {
        let x = c % nx;
        let y = (c / nx) % ny;
        let z = c / nxy;
        field.grid.world(x, y, z)
    };
    // Anchor cell: the skeleton cell nearest the anchor junction.
    let anchor_slot = skel
        .iter()
        .enumerate()
        .min_by(|a, b| {
            world(*a.1)
                .distance(anchor)
                .total_cmp(&world(*b.1).distance(anchor))
        })
        .map(|(s, _)| s)
        .unwrap_or(0);
    // BFS over the 26-adjacent skeleton graph, recording parents for path reconstruction.
    let mut parent: Vec<Option<usize>> = vec![None; skel.len()];
    let mut seen = vec![false; skel.len()];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    seen[anchor_slot] = true;
    queue.push_back(anchor_slot);
    while let Some(u) = queue.pop_front() {
        let c = skel[u];
        let x = (c % nx) as i32;
        let y = ((c / nx) % ny) as i32;
        let z = (c / nxy) as i32;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                    if xx < 0 || yy < 0 || zz < 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (xx as usize, yy as usize, zz as usize);
                    if xx >= nx || yy >= ny || zz >= nz {
                        continue;
                    }
                    if let Some(&v) = slot.get(&field.grid.idx(xx, yy, zz))
                        && !seen[v]
                    {
                        seen[v] = true;
                        parent[v] = Some(u);
                        queue.push_back(v);
                    }
                }
            }
        }
    }
    // Fingertip endpoints on this side, far lateral and within reach of the anchor.
    let mut ends: Vec<(f64, usize)> = skeleton_endpoints(field)
        .into_iter()
        .filter(|(p, _)| {
            (p.x - cx) * side > 0.25 && p.distance(anchor) > 0.04 && p.distance(anchor) < 0.26
        })
        .filter_map(|(p, _)| {
            let c = field.grid.idx(
                ((p.x - field.grid.origin.x) / field.grid.h).round() as usize,
                ((p.y - field.grid.origin.y) / field.grid.h).round() as usize,
                ((p.z - field.grid.origin.z) / field.grid.h).round() as usize,
            );
            slot.get(&c).map(|&s| (p.distance(anchor), s))
        })
        .filter(|(_, s)| seen[*s])
        .collect();
    ends.sort_by(|a, b| b.0.total_cmp(&a.0));
    ends.truncate(count);
    // Order by endpoint angle around the anchor (anatomical fan order).
    ends.sort_by(|a, b| {
        let pa = world(skel[a.1]);
        let pb = world(skel[b.1]);
        let aa = (pa.z - anchor.z).atan2((pa.x - anchor.x) * side);
        let bb = (pb.z - anchor.z).atan2((pb.x - anchor.x) * side);
        aa.total_cmp(&bb)
    });
    // Reconstruct each anchor -> endpoint path as a centreline polyline.
    let mut out = Vec::new();
    for (_, end_slot) in ends {
        let mut path = Vec::new();
        let mut cur = Some(end_slot);
        while let Some(s) = cur {
            let c = skel[s];
            path.push((world(c), f64::from(field.radius[c])));
            if s == anchor_slot {
                break;
            }
            cur = parent[s];
        }
        path.reverse();
        if path.len() >= 2 {
            out.push(path);
        }
    }
    out
}

/// Place points on a centreline at target **chord** fractions (the fraction of the
/// cumulative straight-line distance between consecutive placed points, which is how
/// Mixamo's own chain fractions and the acceptance gate are measured). The points stay
/// on the medial axis — only their arc parameters are adjusted so the seated chord
/// fractions match the targets. A gently curved centreline converges in a few passes;
/// this reconciles arc-length placement with the chord-fraction gate without bending
/// the skeleton off the flesh.
pub fn place_at_chord_fractions(pts: &[(DVec3, f64)], fractions: &[f64]) -> Vec<(DVec3, f64)> {
    let m = fractions.len();
    if m == 0 || pts.is_empty() {
        return Vec::new();
    }
    if m == 1 {
        return vec![FleshCentrelines::point_at(pts, fractions[0])];
    }
    let mut t: Vec<f64> = fractions.to_vec();
    let mut out: Vec<(DVec3, f64)> = Vec::new();
    for _ in 0..12 {
        out = t
            .iter()
            .map(|&f| FleshCentrelines::point_at(pts, f))
            .collect();
        let mut cum = vec![0.0_f64; m];
        for k in 1..m {
            cum[k] = cum[k - 1] + out[k].0.distance(out[k - 1].0);
        }
        let total = cum[m - 1];
        if total < 1e-9 {
            break;
        }
        let mut max_err = 0.0_f64;
        for k in 1..m - 1 {
            let cf = cum[k] / total;
            let err = fractions[k] - cf;
            max_err = max_err.max(err.abs());
            t[k] = (t[k] + err).clamp(0.0, 1.0);
        }
        // Keep the arc parameters monotonic so the points stay ordered along the axis.
        for k in 1..m {
            if t[k] < t[k - 1] {
                t[k] = t[k - 1];
            }
        }
        if max_err < 1e-4 {
            break;
        }
    }
    out
}

/// Skeleton endpoints: cells with <= 1 skeleton neighbour in the 26-neighbourhood, with
/// inscribed radius. The fingertips and toe tips are endpoints of the thinned centreline;
/// they seed the flesh-derived digit axes (no native input).
pub fn skeleton_endpoints(field: &MedialField) -> Vec<(DVec3, f64)> {
    let [nx, ny, nz] = field.grid.dims;
    let nxy = nx * ny;
    let skel: Vec<usize> = (0..field.skeleton.len())
        .filter(|&i| field.skeleton[i])
        .collect();
    let set: std::collections::BTreeSet<usize> = skel.iter().copied().collect();
    let mut out = Vec::new();
    for &i in &skel {
        let x = (i % nx) as i32;
        let y = ((i / nx) % ny) as i32;
        let z = (i / nxy) as i32;
        let mut deg = 0;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                    if xx < 0 || yy < 0 || zz < 0 {
                        continue;
                    }
                    let (xx, yy, zz) = (xx as usize, yy as usize, zz as usize);
                    if xx >= nx || yy >= ny || zz >= nz {
                        continue;
                    }
                    if set.contains(&field.grid.idx(xx, yy, zz)) {
                        deg += 1;
                    }
                }
            }
        }
        if deg <= 1 {
            out.push((
                field.grid.world(x as usize, y as usize, z as usize),
                f64::from(field.radius[i]),
            ));
        }
    }
    out
}

/// One anatomical chain's flesh centreline, extracted from the medial skeleton.
pub struct Chain {
    /// Chain name (`spine`, `arm_left`, `leg_right`, `finger_left_middle`, …).
    pub name: String,
    /// Palette indices of the chain's joints, proximal first.
    pub joints: Vec<usize>,
    /// Centreline points proximal->distal with the inscribed radius at each.
    pub centreline: Vec<(DVec3, f64)>,
    /// Cumulative arc length (metres) at each centreline point.
    pub arc: Vec<f64>,
}

impl Chain {
    /// Total centreline arc length, metres.
    pub fn length(&self) -> f64 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// The centreline point at fraction `f` (0..1) of arc length, interpolated.
    pub fn at_fraction(&self, f: f64) -> (DVec3, f64) {
        if self.centreline.is_empty() {
            return (DVec3::ZERO, 0.0);
        }
        let target = f.clamp(0.0, 1.0) * self.length();
        for k in 1..self.arc.len() {
            if self.arc[k] >= target {
                let span = self.arc[k] - self.arc[k - 1];
                let u = if span > 1e-9 {
                    (target - self.arc[k - 1]) / span
                } else {
                    0.0
                };
                let p = self.centreline[k - 1].0.lerp(self.centreline[k].0, u);
                let r = self.centreline[k - 1].1
                    + (self.centreline[k].1 - self.centreline[k - 1].1) * u;
                return (p, r);
            }
        }
        *self.centreline.last().unwrap()
    }

    /// The arc fraction of the centreline point nearest a world position.
    pub fn fraction_of(&self, p: DVec3) -> (f64, f64) {
        let mut best = (f64::INFINITY, 0.0_f64);
        for (k, (c, _)) in self.centreline.iter().enumerate() {
            let d = p.distance(*c);
            if d < best.0 {
                best = (d, self.arc[k] / self.length().max(1e-9));
            }
        }
        best
    }
}

/// The standard Mixamo chain definitions: (name, joint names proximal->distal).
pub fn mixamo_chains() -> Vec<(String, Vec<String>)> {
    let mut chains: Vec<(String, Vec<String>)> = vec![
        (
            "spine".to_string(),
            ["Hips", "Spine", "Spine1", "Spine2", "Neck", "Head"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
        (
            "head_top".to_string(),
            ["Head", "HeadTop_End"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
        (
            "arm_left".to_string(),
            ["LeftShoulder", "LeftArm", "LeftForeArm", "LeftHand"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
        (
            "arm_right".to_string(),
            ["RightShoulder", "RightArm", "RightForeArm", "RightHand"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
        (
            "leg_left".to_string(),
            ["LeftUpLeg", "LeftLeg", "LeftFoot", "LeftToeBase"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
        (
            "leg_right".to_string(),
            ["RightUpLeg", "RightLeg", "RightFoot", "RightToeBase"]
                .iter()
                .map(|n| format!("mixamorig:{n}"))
                .collect(),
        ),
    ];
    for side in ["Left", "Right"] {
        for digit in ["Thumb", "Index", "Middle", "Ring", "Pinky"] {
            let mut names = vec![format!("mixamorig:{side}Hand")];
            for seg in 1..=4 {
                names.push(format!("mixamorig:{side}Hand{digit}{seg}"));
            }
            chains.push((
                format!("finger_{}_{}", side.to_lowercase(), digit.to_lowercase()),
                names,
            ));
        }
        chains.push((
            if side == "Left" {
                "toe_left".to_string()
            } else {
                "toe_right".to_string()
            },
            vec![
                format!("mixamorig:{side}ToeBase"),
                format!("mixamorig:{side}Toe_End"),
            ],
        ));
    }
    chains
}

/// Extract each chain's flesh centreline from the medial skeleton.
///
/// Every skeleton cell is assigned to the nearest chain axis (the inherited
/// proximal->distal segment), so the torso's medial sheet falls to the spine and
/// each limb's medial curve falls to its arm/leg chain regardless of pose. Cells
/// are then binned along the axis and the deepest inscribed centre per bin kept,
/// giving a 1-cell-wide centreline that follows a flexed limb as easily as a
/// straight one. `rest` are the inherited joint world positions (the axis guide).
pub fn extract_chains(field: &MedialField, palette: &[String], rest: &[DVec3]) -> Vec<Chain> {
    let defs = mixamo_chains();
    let index_of = |name: &str| palette.iter().position(|p| p == name);
    // Resolve each chain's joints and axis endpoints.
    struct Def {
        name: String,
        joints: Vec<usize>,
        a: DVec3,
        b: DVec3,
    }
    let mut defs_resolved: Vec<Def> = Vec::new();
    for (name, names) in &defs {
        let joints: Vec<usize> = names.iter().filter_map(|n| index_of(n)).collect();
        if joints.len() < 2 {
            continue;
        }
        let a = rest[joints[0]];
        let b = rest[*joints.last().unwrap()];
        if a.distance(b) < 1e-6 {
            continue;
        }
        defs_resolved.push(Def {
            name: name.to_string(),
            joints,
            a,
            b,
        });
    }
    // Assign each skeleton cell to the nearest chain axis LINE within that chain's
    // flesh tube. Distance to the infinite line (not the clamped segment) keeps a
    // leg cell below the pelvis out of the spine: it is lateral to the spine line,
    // so it falls to its own leg line. The tube radius bounds each chain's flesh.
    let tube_of = |name: &str| -> f64 {
        if name == "spine" {
            0.13
        } else if name.starts_with("arm") {
            0.07
        } else if name.starts_with("leg") {
            0.09
        } else if name.starts_with("finger") {
            0.035
        } else {
            0.04
        }
    };
    let mut assigned: Vec<Vec<(f64, DVec3, f64)>> = vec![Vec::new(); defs_resolved.len()];
    for (i, &sk) in field.skeleton.iter().enumerate() {
        if !sk {
            continue;
        }
        let x = i % field.grid.dims[0];
        let y = (i / field.grid.dims[0]) % field.grid.dims[1];
        let z = i / (field.grid.dims[0] * field.grid.dims[1]);
        let p = field.grid.world(x, y, z);
        let r = f64::from(field.radius[i]);
        let mut best = (f64::INFINITY, usize::MAX, 0.0_f64);
        for (ci, def) in defs_resolved.iter().enumerate() {
            let ab = def.b - def.a;
            let len = ab.length();
            let d = ab / len;
            let t_line = (p - def.a).dot(d);
            if t_line < -0.05 * len || t_line > 1.05 * len {
                continue;
            }
            let perp = (p - (def.a + d * t_line)).length();
            if perp > tube_of(&def.name) {
                continue;
            }
            if perp < best.0 {
                best = (perp, ci, t_line.clamp(0.0, len));
            }
        }
        if best.1 != usize::MAX {
            assigned[best.1].push((best.2, p, r));
        }
    }
    // Bin each chain's cells along its axis; keep the deepest centre per bin.
    let mut chains = Vec::new();
    for (ci, def) in defs_resolved.iter().enumerate() {
        let mut cells = std::mem::take(&mut assigned[ci]);
        if cells.is_empty() {
            chains.push(Chain {
                name: def.name.clone(),
                joints: def.joints.clone(),
                centreline: Vec::new(),
                arc: Vec::new(),
            });
            continue;
        }
        cells.sort_by(|a, b| a.0.total_cmp(&b.0));
        let span = def.a.distance(def.b);
        let bins = (span / field.grid.h).ceil().max(1.0) as usize;
        let mut centreline: Vec<(DVec3, f64)> = Vec::new();
        for b in 0..=bins {
            let lo = span * b as f64 / bins as f64;
            let hi = span * (b + 1) as f64 / bins as f64;
            let mut best: Option<(f64, DVec3)> = None;
            for (axial, p, r) in &cells {
                if *axial >= lo && *axial < hi && best.is_none_or(|(br, _)| *r > br) {
                    best = Some((*r, *p));
                }
            }
            if let Some((r, p)) = best {
                centreline.push((p, r));
            }
        }
        let mut arc = vec![0.0_f64; centreline.len()];
        for k in 1..centreline.len() {
            arc[k] = arc[k - 1] + centreline[k].0.distance(centreline[k - 1].0);
        }
        chains.push(Chain {
            name: def.name.clone(),
            joints: def.joints.clone(),
            centreline,
            arc,
        });
    }
    chains
}
/// Ridge cells: inside cells whose inscribed radius is >= all 26 neighbours.
fn ridge(grid: &Grid, inside: &[bool], radius: &[f32]) -> Vec<usize> {
    let [nx, ny, nz] = grid.dims;
    let mut core = Vec::new();
    for iz in 0..nz {
        for iy in 0..ny {
            for ix in 0..nx {
                let i = grid.idx(ix, iy, iz);
                if !inside[i] || radius[i] <= 0.0 {
                    continue;
                }
                let r = radius[i];
                let mut is_max = true;
                for dz in -1_i32..=1 {
                    for dy in -1_i32..=1 {
                        for dx in -1_i32..=1 {
                            if dx == 0 && dy == 0 && dz == 0 {
                                continue;
                            }
                            let x = ix as i32 + dx;
                            let y = iy as i32 + dy;
                            let z = iz as i32 + dz;
                            if x < 0 || y < 0 || z < 0 {
                                continue;
                            }
                            let (x, y, z) = (x as usize, y as usize, z as usize);
                            if x >= nx || y >= ny || z >= nz {
                                continue;
                            }
                            if radius[grid.idx(x, y, z)] > r {
                                is_max = false;
                            }
                        }
                    }
                }
                if is_max {
                    core.push(i);
                }
            }
        }
    }
    core
}

/// Skeleton neighbours of cell `i` in its 26-neighbourhood: their count and the
/// number of 26-connected components they form (excluding `i` itself).
fn neighbour_components(grid: &Grid, skel: &[bool], i: usize) -> (usize, usize) {
    let [nx, ny, nz] = grid.dims;
    let x = (i % nx) as i32;
    let y = ((i / nx) % ny) as i32;
    let z = (i / (nx * ny)) as i32;
    let mut nbrs: Vec<(i32, i32, i32)> = Vec::new();
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                if xx < 0 || yy < 0 || zz < 0 {
                    continue;
                }
                let (ux, uy, uz) = (xx as usize, yy as usize, zz as usize);
                if ux >= nx || uy >= ny || uz >= nz {
                    continue;
                }
                if skel[grid.idx(ux, uy, uz)] {
                    nbrs.push((xx, yy, zz));
                }
            }
        }
    }
    let count = nbrs.len();
    if count <= 1 {
        return (count, count);
    }
    let mut seen = vec![false; count];
    let mut comps = 0;
    for s in 0..count {
        if seen[s] {
            continue;
        }
        comps += 1;
        let mut stack = vec![s];
        seen[s] = true;
        while let Some(a) = stack.pop() {
            let (ax, ay, az) = nbrs[a];
            for b in 0..count {
                if seen[b] {
                    continue;
                }
                let (bx, by, bz) = nbrs[b];
                if (ax - bx).abs() <= 1 && (ay - by).abs() <= 1 && (az - bz).abs() <= 1 {
                    seen[b] = true;
                    stack.push(b);
                }
            }
        }
    }
    (count, comps)
}

/// Whether cell `i` has a 6-neighbour outside the skeleton (a peelable surface cell).
fn is_boundary6(grid: &Grid, skel: &[bool], i: usize) -> bool {
    let [nx, ny, nz] = grid.dims;
    let x = i % nx;
    let y = (i / nx) % ny;
    let z = i / (nx * ny);
    let outside = |xx: usize, yy: usize, zz: usize| -> bool {
        xx >= nx || yy >= ny || zz >= nz || !skel[grid.idx(xx, yy, zz)]
    };
    x == 0
        || y == 0
        || z == 0
        || x == nx - 1
        || y == ny - 1
        || z == nz - 1
        || outside(x - 1, y, z)
        || outside(x + 1, y, z)
        || outside(x, y - 1, z)
        || outside(x, y + 1, z)
        || outside(x, y, z - 1)
        || outside(x, y, z + 1)
}

/// Distance-ordered homotopic thinning to a 1-cell-wide medial centreline tree.
///
/// Boundary cells are peeled in ascending inscribed radius; a cell is removed only
/// when it has >= 2 skeleton neighbours forming a single 26-component (a side cell
/// of a thick region), so endpoints and branch junctions survive. Processing in
/// distance order cascades inward in one pass and keeps the deepest medial cells
/// longest, so the resulting curve follows the medial axis. The torso's medial
/// sheet collapses to its central curve; a limb stays a clean centreline.
pub fn curve_skeleton(grid: &Grid, inside: &[bool], radius: &[f32]) -> Vec<bool> {
    let mut skel = inside.to_vec();
    let mut order: Vec<usize> = (0..skel.len()).filter(|&i| inside[i]).collect();
    order.sort_by(|&a, &b| radius[a].total_cmp(&radius[b]));
    for &i in &order {
        if !skel[i] || !is_boundary6(grid, &skel, i) {
            continue;
        }
        let (count, comps) = neighbour_components(grid, &skel, i);
        if count >= 2 && comps == 1 {
            skel[i] = false;
        }
    }
    skel
}

#[cfg(test)]
mod tests {
    use super::*;
    use sculpto_mesh::actor::Actor;

    /// A closed UV sphere of radius `r` centred at the origin.
    fn sphere(r: f64, rings: usize, sectors: usize) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let mut verts = Vec::new();
        for i in 0..=rings {
            let phi = std::f64::consts::PI * i as f64 / rings as f64;
            for j in 0..=sectors {
                let theta = 2.0 * std::f64::consts::PI * j as f64 / sectors as f64;
                verts.push([
                    r * phi.sin() * theta.cos(),
                    r * phi.cos(),
                    r * phi.sin() * theta.sin(),
                ]);
            }
        }
        let mut tris = Vec::new();
        let row = (sectors + 1) as u32;
        for i in 0..rings as u32 {
            for j in 0..sectors as u32 {
                let a = i * row + j;
                let b = a + row;
                tris.push([a, b, a + 1]);
                tris.push([a + 1, b, b + 1]);
            }
        }
        (verts, tris)
    }

    /// A closed cylinder of radius `r`, height `h`, axis along Y, centred at origin.
    fn cylinder(r: f64, h: f64, sectors: usize) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let mut verts = Vec::new();
        for &y in &[-h / 2.0, h / 2.0] {
            for j in 0..sectors {
                let theta = 2.0 * std::f64::consts::PI * j as f64 / sectors as f64;
                verts.push([r * theta.cos(), y, r * theta.sin()]);
            }
        }
        let bottom_centre = verts.len() as u32;
        verts.push([0.0, -h / 2.0, 0.0]);
        let top_centre = verts.len() as u32;
        verts.push([0.0, h / 2.0, 0.0]);
        let s = sectors as u32;
        let mut tris = Vec::new();
        for j in 0..s {
            let jn = (j + 1) % s;
            tris.push([j, jn, s + jn]);
            tris.push([j, s + jn, s + j]);
            tris.push([bottom_centre, jn, j]);
            tris.push([top_centre, s + j, s + jn]);
        }
        (verts, tris)
    }

    #[test]
    fn sphere_inscribed_radius_matches() {
        let r = 0.10;
        let (v, t) = sphere(r, 48, 48);
        let field = MedialField::from_mesh(&v, &t, 0.004);
        let max_r = field.radius.iter().copied().fold(0.0_f32, f32::max);
        // The deepest inscribed sphere approaches the true radius within a voxel.
        assert!(
            (f64::from(max_r) - r).abs() < 0.006,
            "max inscribed radius {max_r} vs true {r}"
        );
        // The centre is inside and its radius is near the maximum.
        assert!(field.is_inside(DVec3::ZERO));
        assert!(field.radius_at(DVec3::ZERO) > r - 0.006);
        // The ridge of a sphere collapses to a small core near the centre.
        assert!(!field.core.is_empty());
        for &i in &field.core {
            let z = i / (field.grid.dims[0] * field.grid.dims[1]);
            let y = (i / field.grid.dims[0]) % field.grid.dims[1];
            let x = i % field.grid.dims[0];
            let p = field.grid.world(x, y, z);
            assert!(p.length() < 0.02, "sphere core stray at {p:?}");
        }
    }

    #[test]
    fn cylinder_centreline_radius_matches() {
        let r = 0.03;
        let h = 0.30;
        let (v, t) = cylinder(r, h, 64);
        let field = MedialField::from_mesh(&v, &t, 0.003);
        // Along the axis the inscribed radius is the cylinder radius.
        for y in [-0.10, -0.05, 0.0, 0.05, 0.10] {
            let rad = field.radius_at(DVec3::new(0.0, y, 0.0));
            assert!(
                (rad - r).abs() < 0.004,
                "axis radius at y={y}: {rad} vs {r}"
            );
        }
        // Off-axis points have a smaller inscribed radius.
        assert!(field.radius_at(DVec3::new(0.02, 0.0, 0.0)) < r);
        assert!(!field.is_inside(DVec3::new(0.05, 0.0, 0.0)));
    }

    #[test]
    fn surface_probe_signs_and_distances_on_a_closed_sphere() {
        let r = 0.10;
        let (v, t) = sphere(r, 48, 48);
        // Centre: inside by parity, one radius from the surface.
        let (mm, inside, votes) = surface_probe(&v, &t, DVec3::ZERO);
        assert!(inside, "sphere centre must be inside");
        // The probe's contract is a majority vote, not unanimity: the ±Y rays pass
        // through the sphere's pole vertices, where parity is degenerate, so up to two
        // axes may disagree. A convex centre still wins the vote (>= 4 of 6).
        assert!(
            votes >= 4,
            "a convex body wins the axis vote, got {votes}/6"
        );
        assert!((mm + r * 1000.0).abs() < 1.0, "centre signed {mm} vs -100");
        // Outside on +X: positive, distance to the surface.
        let (mm, inside, _) = surface_probe(&v, &t, DVec3::new(0.2, 0.0, 0.0));
        assert!(!inside, "exterior point must be outside");
        assert!(
            (mm - r * 1000.0).abs() < 1.0,
            "exterior signed {mm} vs +100"
        );
        // On the surface: distance ~0; parity is undecidable there (the gate's
        // 1 mm tolerance exists for exactly this coincidence).
        let (mm, _, _) = surface_probe(&v, &t, DVec3::new(r, 0.0, 0.0));
        assert!(mm.abs() < 1.0, "surface point signed {mm}");
    }

    #[test]
    fn first_surface_hit_is_nearest_forward_distance_independent_of_winding() {
        let vertices = [
            [-1.0, 2.0, -1.0],
            [1.0, 2.0, -1.0],
            [0.0, 2.0, 1.0],
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [0.0, 1.0, 1.0],
            [-1.0, -0.5, -1.0],
            [1.0, -0.5, -1.0],
            [0.0, -0.5, 1.0],
        ];
        let triangles = [[0, 1, 2], [5, 4, 3], [6, 7, 8]];
        let hit = first_surface_hit(&vertices, &triangles, DVec3::ZERO, DVec3::Y * 3.0);
        assert_eq!(hit, Some(1.0));
        assert_eq!(
            first_surface_hit(&vertices, &triangles, DVec3::ZERO, -DVec3::Y),
            Some(0.5)
        );
        assert_eq!(
            first_surface_hit(&vertices, &triangles, DVec3::ZERO, DVec3::X),
            None
        );
        assert_eq!(
            first_surface_hit(&vertices, &triangles, DVec3::ZERO, DVec3::ZERO),
            None
        );
    }

    /// Measure the field on a real body fixture (set SCULPTO_MEDIAL_FIXTURE to a
    /// skinned GLB). Prints the grid, the inscribed-radius range, the ridge size,
    /// and the deepest inscribed centre per height — the torso/head centreline.
    #[test]
    fn fixture_medial_structure() {
        let Ok(path) = std::env::var("SCULPTO_MEDIAL_FIXTURE") else {
            eprintln!("SCULPTO_MEDIAL_FIXTURE not set; skipping");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let actor = Actor::from_glb(&bytes).unwrap();
        let (rig, palette) = crate::adapt::actor_rig(&actor).unwrap();
        // Combine every skinned mesh node at rest into one world-space flesh pool
        // (Y-Bot carries several; the M body carries one).
        let names: Vec<String> = actor
            .document()
            .nodes()
            .filter(|n| n.mesh().is_some() && n.skin().is_some())
            .filter_map(|n| n.name().map(str::to_owned))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let prefs: Vec<&str> = palette.iter().map(String::as_str).collect();
        let sampled = actor
            .rig_sample(&refs, sculpto_mesh::actor::Pose::Rest, &prefs)
            .unwrap();
        let vertices = sampled.sample.vertices.clone();
        let triangles: Vec<[u32; 3]> = sampled
            .sample
            .triangles
            .iter()
            .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        let h = 0.003;
        let seal: usize = std::env::var("SCULPTO_MEDIAL_SEAL")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        let field = MedialField::from_mesh_sealed(&vertices, &triangles, h, seal);
        let [nx, ny, nz] = field.grid.dims;
        let inside_count = field.inside.iter().filter(|&&b| b).count();
        let max_r = field.radius.iter().copied().fold(0.0_f32, f32::max);
        eprintln!(
            "FIXTURE dims={:?} cells={} inside={inside_count} max_radius={max_r:.4}m core={}",
            field.grid.dims,
            field.grid.len(),
            field.core.len()
        );
        // Deepest inscribed centre per height row (the torso/head medial centreline).
        eprintln!("FIXTURE per-height deepest centre (y_mm, x_mm, z_mm, radius_mm):");
        for iy in (0..ny).step_by(12) {
            let mut best = (0.0_f32, 0_usize, 0_usize);
            for iz in 0..nz {
                for ix in 0..nx {
                    let r = field.radius[field.grid.idx(ix, iy, iz)];
                    if r > best.0 {
                        best = (r, ix, iz);
                    }
                }
            }
            if best.0 <= 0.0 {
                continue;
            }
            let p = field.grid.world(best.1, iy, best.2);
            eprintln!(
                "  y={:7.1} x={:7.1} z={:7.1} r={:6.1}",
                p.y * 1000.0,
                p.x * 1000.0,
                p.z * 1000.0,
                f64::from(best.0) * 1000.0
            );
        }
        let _ = rig;
        // Skeleton graph structure: classify thinned centreline cells by degree.
        let [nx2, ny2, _nz2] = field.grid.dims;
        let nxy2 = nx2 * ny2;
        let skel: Vec<usize> = (0..field.skeleton.len())
            .filter(|&i| field.skeleton[i])
            .collect();
        let skel_set: std::collections::BTreeSet<usize> = skel.iter().copied().collect();
        eprintln!("FIXTURE skeleton cells={}", skel.len());
        let degree = |i: usize| -> usize {
            let x = (i % nx2) as i32;
            let y = ((i / nx2) % ny2) as i32;
            let z = (i / nxy2) as i32;
            let mut d = 0;
            for dz in -1..=1 {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        let (xx, yy, zz) = (x + dx, y + dy, z + dz);
                        if xx < 0 || yy < 0 || zz < 0 {
                            continue;
                        }
                        let (xx, yy, zz) = (xx as usize, yy as usize, zz as usize);
                        if xx >= nx2 || yy >= ny2 || zz >= field.grid.dims[2] {
                            continue;
                        }
                        if skel_set.contains(&field.grid.idx(xx, yy, zz)) {
                            d += 1;
                        }
                    }
                }
            }
            d
        };
        let mut endpoints = Vec::new();
        let mut junctions = Vec::new();
        for &i in &skel {
            let d = degree(i);
            let x = i % nx2;
            let y = (i / nx2) % ny2;
            let z = i / nxy2;
            let p = field.grid.world(x, y, z);
            let r = f64::from(field.radius[i]) * 1000.0;
            if d <= 1 {
                endpoints.push((p, r));
            } else if d >= 3 {
                junctions.push((p, r, d));
            }
        }
        eprintln!(
            "FIXTURE skel endpoints={} junctions={}",
            endpoints.len(),
            junctions.len()
        );
        eprintln!("FIXTURE skel junctions (x,y,z mm, radius mm, degree):");
        let mut j = junctions.clone();
        j.sort_by(|a, b| b.0.y.total_cmp(&a.0.y));
        for (p, r, d) in j.iter().take(40) {
            eprintln!(
                "  J x={:7.1} y={:7.1} z={:7.1} r={:6.1} deg={d}",
                p.x * 1000.0,
                p.y * 1000.0,
                p.z * 1000.0,
                r
            );
        }
        eprintln!("FIXTURE skel endpoints (x,y,z mm, radius mm):");
        let mut e = endpoints.clone();
        e.sort_by(|a, b| b.0.y.total_cmp(&a.0.y));
        for (p, r) in e.iter().take(40) {
            eprintln!(
                "  E x={:7.1} y={:7.1} z={:7.1} r={:6.1}",
                p.x * 1000.0,
                p.y * 1000.0,
                p.z * 1000.0,
                r
            );
        }
        // Chain centrelines + each joint's fraction/offset/radius (the template terms).
        use sculpto_core::rig::JointId;
        let rest: Vec<DVec3> = (0..palette.len())
            .map(|i| {
                let m = rig.rest_world(JointId::new(i as u32)).unwrap();
                DVec3::new(m[3][0], m[3][1], m[3][2])
            })
            .collect();
        let chains = extract_chains(&field, &palette, &rest);
        for c in &chains {
            eprintln!(
                "CHAIN {} joints={} pts={} length_mm={:.1}",
                c.name,
                c.joints.len(),
                c.centreline.len(),
                c.length() * 1000.0
            );
            for &j in &c.joints {
                let (offset, frac) = c.fraction_of(rest[j]);
                let r = field.radius_at(rest[j]) * 1000.0;
                eprintln!(
                    "   {:28} frac={:.3} offset_mm={:6.1} radius_mm={:6.1}",
                    palette[j],
                    frac,
                    offset * 1000.0,
                    r
                );
            }
        }
        // Flesh centrelines + branch landmarks (M2 placement basis, no native input).
        let fc = flesh_centrelines(&field);
        let len = |p: &[(DVec3, f64)]| -> f64 { FleshCentrelines::arc(p) * 1000.0 };
        let lm = |o: Option<(DVec3, f64)>, name: &str| match o {
            Some((p, r)) => eprintln!(
                "  LM {:14} = ({:7.1},{:7.1},{:7.1}) r={:6.1}",
                name,
                p.x * 1000.0,
                p.y * 1000.0,
                p.z * 1000.0,
                r * 1000.0
            ),
            None => eprintln!("  LM {name:14} = None"),
        };
        eprintln!(
            "FLESH cx_mm={:.1} midline_mm={:.1}(pts{}) armL_mm={:.1}(pts{}) armR_mm={:.1} legL_mm={:.1}(pts{}) legR_mm={:.1}",
            fc.cx * 1000.0,
            len(&fc.spine),
            fc.spine.len(),
            len(&fc.arm_left),
            fc.arm_left.len(),
            len(&fc.arm_right),
            len(&fc.leg_left),
            fc.leg_left.len(),
            len(&fc.leg_right)
        );
        lm(fc.hip_anchor, "hip_anchor");
        lm(fc.neck_branch, "neck_branch");
        lm(fc.head_centre, "head_centre");
        lm(fc.crown, "crown");
        lm(fc.wrist_left, "wrist_left");
        lm(fc.wrist_right, "wrist_right");
        lm(fc.ankle_left, "ankle_left");
        lm(fc.ankle_right, "ankle_right");
        lm(fc.toe_base_left, "toe_base_left");
        lm(fc.toe_base_right, "toe_base_right");
        lm(fc.shoulder_left, "shoulder_left");
        lm(fc.shoulder_right, "shoulder_right");
        lm(fc.hip_left, "hip_left");
        lm(fc.hip_right, "hip_right");
        // Placement axes (branch-to-branch) and their end radii.
        let axis = |pts: &[(DVec3, f64)], name: &str| {
            let (a, b) = (pts.first(), pts.last());
            if let (Some(a), Some(b)) = (a, b) {
                eprintln!(
                    "  AXIS {:10} extent_mm={:7.1} r_prox={:6.1} r_dist={:6.1} prox=({:.1},{:.1},{:.1}) dist=({:.1},{:.1},{:.1})",
                    name,
                    len(pts),
                    a.1 * 1000.0,
                    b.1 * 1000.0,
                    a.0.x * 1000.0,
                    a.0.y * 1000.0,
                    a.0.z * 1000.0,
                    b.0.x * 1000.0,
                    b.0.y * 1000.0,
                    b.0.z * 1000.0
                );
            }
        };
        axis(&fc.spine_axis(), "spine");
        axis(&fc.arm_axis_left(), "armL");
        axis(&fc.arm_axis_right(), "armR");
        axis(&fc.leg_axis_left(), "legL");
        axis(&fc.leg_axis_right(), "legR");
    }

    /// Publish Half 1 (the Mixamo-body anatomy template) as JSON. Set
    /// SCULPTO_MEDIAL_FIXTURE to ybot_walking.glb and SCULPTO_TEMPLATE_OUT to the
    /// output path. Prints the spine and left-arm rows for verification.
    #[test]
    fn template_publish() {
        let Ok(path) = std::env::var("SCULPTO_MEDIAL_FIXTURE") else {
            return;
        };
        let Ok(out) = std::env::var("SCULPTO_TEMPLATE_OUT") else {
            eprintln!("SCULPTO_TEMPLATE_OUT not set; skipping");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        let actor = Actor::from_glb(&bytes).unwrap();
        let (rig, palette) = crate::adapt::actor_rig(&actor).unwrap();
        let names: Vec<String> = actor
            .document()
            .nodes()
            .filter(|n| n.mesh().is_some() && n.skin().is_some())
            .filter_map(|n| n.name().map(str::to_owned))
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let prefs: Vec<&str> = palette.iter().map(String::as_str).collect();
        let sampled = actor
            .rig_sample(&refs, sculpto_mesh::actor::Pose::Rest, &prefs)
            .unwrap();
        let vertices = sampled.sample.vertices.clone();
        let triangles: Vec<[u32; 3]> = sampled
            .sample
            .triangles
            .iter()
            .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();
        let field = MedialField::from_mesh_sealed(&vertices, &triangles, 0.003, 2);
        use sculpto_core::rig::JointId;
        let rest: Vec<DVec3> = (0..palette.len())
            .map(|i| {
                let m = rig.rest_world(JointId::new(i as u32)).unwrap();
                DVec3::new(m[3][0], m[3][1], m[3][2])
            })
            .collect();
        let template = anatomy_template(&field, &palette, &rest);
        std::fs::write(&out, serde_json::to_string_pretty(&template).unwrap()).unwrap();
        eprintln!("TEMPLATE written to {out}");
        for row in template["joints"].as_array().unwrap() {
            let chain = row["chain"].as_str().unwrap();
            if chain == "spine" || chain == "arm_left" || chain == "leg_left" {
                eprintln!(
                    "ROW {:26} chain={:9} frac={:.3} r_mm={:6.1} offset_mm={:6.1}",
                    row["name"].as_str().unwrap(),
                    chain,
                    row["fraction"].as_f64().unwrap(),
                    row["medial_radius_mm"].as_f64().unwrap(),
                    row["axis_offset_mm"].as_f64().unwrap()
                );
            }
        }
    }
}
