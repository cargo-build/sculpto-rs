//! The body as a solid partitioned into one cell per deform bone.
//!
//! A deform node does not own a patch *on* the skin; it owns a **cell of the
//! solid** `Ω` bounded by `∂Ω`. The visible boundary between two bones is only
//! where two cells meet the surface; the ownership itself lives in the volume,
//! which is why flesh at depth — a garment layer, the fat under the chest wall —
//! belongs to the same bone as the skin above it.
//!
//! A surface-geodesic partition cannot express that. In an A-pose the upper arm
//! rests against the ribcage, and a surface path that skims the contact patch
//! carries the arm's influence onto the chest wall. A cell cannot leak across a
//! *contact*: the two surfaces touch, so the film between them is surface voxels,
//! not flesh, and the deep flesh of the arm and of the ribcage meet only through
//! the shoulder.
//!
//! The cells are cut from the field [`crate::medial`] already builds — voxel
//! occupancy by surface rasterisation plus exterior flood, and the inscribed
//! radius by EDT — so this module adds no second distance structure. Seeding is
//! the nearest bone inside its own inscribed section; growth is a watershed from
//! the deepest cells outwards, so the partition settles where the flesh is a
//! narrow neck. That neck is the crease, measured rather than drawn. @req DECL0008

use std::collections::BTreeMap;

use glam::DVec3;

use crate::medial::{Grid, MedialField};

/// A bone the solid is cut into cells for: its name and its head-to-child segments.
#[derive(Debug, Clone, PartialEq)]
pub struct CellBone {
    /// Bone name, for the report.
    pub name: String,
    /// Head-to-child segments in the rest pose, metres.
    pub segments: Vec<(DVec3, DVec3)>,
}

/// No cell reached this voxel (the growth is expected to leave none inside).
pub const UNLABELLED: u32 = u32::MAX;

/// Seeds reach this fraction of a bone's own inscribed section around its axis.
/// A tube, not a cone: the chest wall beside the armpit is farther from the
/// humerus than the arm's own section, so the arm seeds its own flesh and the
/// watershed decides the armpit, where the flesh is a narrow neck.
const SEED_SECTIONS: f64 = 1.0;
/// A bone's section is capped here, metres: a clavicle seated in the shoulder's
/// fat would otherwise seed the whole deltoid.
const SECTION_MAX_M: f64 = 0.030;
/// A bone's section never reads below this, metres.
const SECTION_FLOOR_M: f64 = 0.004;
/// A cell's blend band is this fraction of its own flesh radius.
const BAND_FLESH: f64 = 0.6;
/// Narrowest band a cell may blend over, metres: one voxel-scale crease.
const BAND_MIN_M: f64 = 0.003;

/// The solid partitioned into deform cells.
#[derive(Debug, Clone)]
pub struct FleshCells {
    /// The voxel grid the cells live on.
    pub grid: Grid,
    /// Bone index per voxel, [`UNLABELLED`] outside the flesh and where no cell grew.
    pub label: Vec<u32>,
    /// Voxels per bone, in the order the caller declared the bones.
    pub volumes: Vec<usize>,
    /// Blend band per bone, metres: proportional to the flesh it owns, capped.
    pub bands: Vec<f64>,
    /// The widest band the partition may hand out, metres.
    pub band_max: f64,
    /// The deepest inscribed radius on the interface between two cells, metres: the
    /// neck each boundary crosses. Keyed by bone pair, ascending.
    pub interfaces: BTreeMap<(u32, u32), f64>,
}

/// What the cells permit at one point: the owner and the neighbours' shares.
#[derive(Debug, Clone, PartialEq)]
pub struct CellBlend {
    /// The cell whose closure contains the point.
    pub owner: u32,
    /// Owner first, then neighbours by descending share; the shares sum to 1.
    pub shares: Vec<(u32, f64)>,
}

/// Cut the solid into one cell per bone.
///
/// The seeds are the flesh each bone is nearest to, inside its own inscribed
/// section; the growth is ordered by inscribed radius (deepest first), so where
/// two cells meet the surface chooses the narrow neck between them. The result
/// is a partition of the whole solid: every inside voxel carries a bone.
pub fn flesh_cells(field: &MedialField, bones: &[CellBone], band_max: f64) -> FleshCells {
    let grid = &field.grid;
    let count = grid.len();
    let section = sections(field, bones);
    let mut label = vec![UNLABELLED; count];
    let mut seeded = vec![f32::MAX; count];
    seed(grid, field, bones, &section, &mut label, &mut seeded);
    grow(grid, field, &mut label);
    let mut volumes = vec![0_usize; bones.len()];
    let mut deepest = vec![0.0_f64; bones.len()];
    let mut interfaces: BTreeMap<(u32, u32), f64> = BTreeMap::new();
    for index in 0..count {
        let slot = label[index];
        if slot == UNLABELLED {
            continue;
        }
        volumes[slot as usize] += 1;
        deepest[slot as usize] = deepest[slot as usize].max(f64::from(field.radius[index]));
        let (x, y, z) = unflatten(grid, index);
        for (nx, ny, nz) in neighbours(x, y, z, grid.dims) {
            let other = label[grid.idx(nx, ny, nz)];
            if other == UNLABELLED || other == slot {
                continue;
            }
            let pair = match slot < other {
                true => (slot, other),
                false => (other, slot),
            };
            // The neck's depth: how deep the two cells are still contiguous, i.e.
            // how thick the flesh bridging them is. A wide bridge (hip socket) is
            // deep; the armpit contact is a film.
            let radius =
                f64::from(field.radius[index]).max(f64::from(field.radius[grid.idx(nx, ny, nz)]));
            interfaces
                .entry(pair)
                .and_modify(|known| *known = known.max(radius))
                .or_insert(radius);
        }
    }
    let bands = deepest
        .iter()
        .map(|radius| (radius * BAND_FLESH).clamp(BAND_MIN_M, band_max))
        .collect();
    FleshCells {
        grid: grid.clone(),
        label,
        volumes,
        bands,
        band_max,
        interfaces,
    }
}

impl FleshCells {
    /// The cell closest to a world point, searching a few voxels around it.
    ///
    /// A surface vertex sits on the boundary of the solid, not inside it, so the
    /// lookup is the nearest labelled voxel within `rings` rings, never a floor.
    pub fn nearest_cell(&self, point: DVec3, rings: isize) -> Option<u32> {
        self.nearest_with(point, rings).0
    }

    /// The books a cell boundary opens at a world point: who owns it, and how
    /// much of the blend each neighbour may take.
    ///
    /// The owner always keeps at least half, and reaches the whole row once the
    /// point is `band` inside its own cell; two cells meeting at the interface
    /// share it 50/50. That is the hard cut of the operator's model expressed as a
    /// budget: the cell decides who dominates, the band keeps the surface
    /// continuous, and no bone outside the band carries anything.
    pub fn blend_at(&self, point: DVec3, band: f64) -> Option<CellBlend> {
        let [nx, ny, nz] = self.grid.dims;
        let base = self.base_cell(point);
        let rings = ((band / self.grid.h).ceil() as isize).max(1);
        let (owner, _) = self.nearest_with(point, rings + 1);
        let owner = owner?;
        // The interface is where a *different* cell begins. Its distance is what
        // decides dominance: a vertex a whole band inside its own cell owns the
        // row outright, and one sitting on the interface shares it 50/50.
        let mut interface = f64::INFINITY;
        let mut neighbours: Vec<(u32, f64)> = Vec::new();
        for dx in -rings - 1..=rings + 1 {
            for dy in -rings - 1..=rings + 1 {
                for dz in -rings - 1..=rings + 1 {
                    let (x, y, z) = (base[0] + dx, base[1] + dy, base[2] + dz);
                    if x < 0 || y < 0 || z < 0 {
                        continue;
                    }
                    let (x, y, z) = (x as usize, y as usize, z as usize);
                    if x >= nx || y >= ny || z >= nz {
                        continue;
                    }
                    let slot = self.label[self.grid.idx(x, y, z)];
                    if slot == UNLABELLED || slot == owner {
                        continue;
                    }
                    let distance = self.grid.world(x, y, z).distance(point);
                    interface = interface.min(distance);
                    if distance > band {
                        continue;
                    }
                    let share = 1.0 - distance / band;
                    match neighbours.iter_mut().find(|(known, _)| *known == slot) {
                        Some((_, best)) => *best = best.max(share),
                        None => neighbours.push((slot, share)),
                    }
                }
            }
        }
        neighbours.sort_by(|left, right| right.1.total_cmp(&left.1));
        let dominance = match interface.is_finite() {
            true => 0.5 + 0.5 * (interface / band).min(1.0),
            false => 1.0,
        };
        let total: f64 = neighbours.iter().map(|(_, share)| *share).sum();
        let mut shares = Vec::with_capacity(neighbours.len() + 1);
        shares.push((owner, dominance));
        if total > 0.0 {
            let rest = 1.0 - dominance;
            for (slot, share) in neighbours {
                shares.push((slot, rest * share / total));
            }
        }
        Some(CellBlend { owner, shares })
    }

    /// The band a cell earns, metres.
    pub fn band_at(&self, cell: u32) -> f64 {
        self.bands
            .get(cell as usize)
            .copied()
            .unwrap_or(self.band_max)
    }

    /// Grid coordinates nearest a world point.
    fn base_cell(&self, point: DVec3) -> [isize; 3] {
        [
            ((point.x - self.grid.origin.x) / self.grid.h).round() as isize,
            ((point.y - self.grid.origin.y) / self.grid.h).round() as isize,
            ((point.z - self.grid.origin.z) / self.grid.h).round() as isize,
        ]
    }

    /// The nearest labelled voxel and its distance, within `rings` rings.
    fn nearest_with(&self, point: DVec3, rings: isize) -> (Option<u32>, f64) {
        let [nx, ny, nz] = self.grid.dims;
        let base = self.base_cell(point);
        let mut best: Option<(f64, u32)> = None;
        for dx in -rings..=rings {
            for dy in -rings..=rings {
                for dz in -rings..=rings {
                    let (x, y, z) = (base[0] + dx, base[1] + dy, base[2] + dz);
                    if x < 0 || y < 0 || z < 0 {
                        continue;
                    }
                    let (x, y, z) = (x as usize, y as usize, z as usize);
                    if x >= nx || y >= ny || z >= nz {
                        continue;
                    }
                    let slot = self.label[self.grid.idx(x, y, z)];
                    if slot == UNLABELLED {
                        continue;
                    }
                    let distance = self.grid.world(x, y, z).distance(point);
                    if best.is_none_or(|(known, _)| distance < known) {
                        best = Some((distance, slot));
                    }
                }
            }
        }
        match best {
            Some((distance, slot)) => (Some(slot), distance),
            None => (None, f64::INFINITY),
        }
    }
}

/// Each bone's own inscribed section: the median flesh radius along its segments.
///
/// The median, not the deepest sample: a bone that passes through the pelvis would
/// otherwise measure the pelvis and seed half the trunk as its own. The cap keeps
/// a short bone seated in fat (a clavicle inside the shoulder) from claiming the
/// deltoid, while the watershed still grows its cell into whatever is left.
fn sections(field: &MedialField, bones: &[CellBone]) -> Vec<f64> {
    bones
        .iter()
        .map(|bone| {
            let mut samples = Vec::new();
            for (head, tail) in &bone.segments {
                let axis = *tail - *head;
                let length = axis.length();
                let steps = ((length / field.grid.h).ceil() as usize).max(1);
                for step in 0..=steps {
                    let point = *head + axis * (step as f64 / steps as f64);
                    samples.push(inscribed_at(field, point));
                }
            }
            samples.sort_by(f64::total_cmp);
            let median = samples
                .get(samples.len() / 2)
                .copied()
                .unwrap_or(SECTION_FLOOR_M);
            median.clamp(SECTION_FLOOR_M, SECTION_MAX_M)
        })
        .collect()
}

/// Inscribed radius near a point, searching outwards when the point is outside
/// the flesh (a clavicle seat sits 12 mm past the skin on this body).
fn inscribed_at(field: &MedialField, point: DVec3) -> f64 {
    let direct = field.radius_at(point);
    if direct > 0.0 {
        return direct;
    }
    let [nx, ny, nz] = field.grid.dims;
    let base = [
        ((point.x - field.grid.origin.x) / field.grid.h).round() as isize,
        ((point.y - field.grid.origin.y) / field.grid.h).round() as isize,
        ((point.z - field.grid.origin.z) / field.grid.h).round() as isize,
    ];
    let mut best = 0.0_f64;
    for ring in 1..=6_isize {
        for dx in -ring..=ring {
            for dy in -ring..=ring {
                for dz in -ring..=ring {
                    if dx.abs().max(dy.abs()).max(dz.abs()) != ring {
                        continue;
                    }
                    let (x, y, z) = (base[0] + dx, base[1] + dy, base[2] + dz);
                    if x < 0 || y < 0 || z < 0 {
                        continue;
                    }
                    let (x, y, z) = (x as usize, y as usize, z as usize);
                    if x >= nx || y >= ny || z >= nz {
                        continue;
                    }
                    best = best.max(f64::from(field.radius[field.grid.idx(x, y, z)]));
                }
            }
        }
        if best > 0.0 {
            return best;
        }
    }
    best
}

/// Seed every flesh voxel within a bone's own section with that bone, keeping the
/// nearest bone where two sections overlap.
fn seed(
    grid: &Grid,
    field: &MedialField,
    bones: &[CellBone],
    section: &[f64],
    label: &mut [u32],
    seeded: &mut [f32],
) {
    let [nx, ny, nz] = grid.dims;
    for (slot, bone) in bones.iter().enumerate() {
        let reach = SEED_SECTIONS * section[slot];
        let mut low = DVec3::splat(f64::INFINITY);
        let mut high = DVec3::splat(f64::NEG_INFINITY);
        for (head, tail) in &bone.segments {
            low = low.min(*head).min(*tail);
            high = high.max(*head).max(*tail);
        }
        let lo = ((low - DVec3::splat(reach) - grid.origin) / grid.h).floor();
        let hi = ((high + DVec3::splat(reach) - grid.origin) / grid.h).ceil();
        let lo = [
            lo.x.max(0.0) as usize,
            lo.y.max(0.0) as usize,
            lo.z.max(0.0) as usize,
        ];
        let hi = [
            (hi.x.max(0.0) as usize).min(nx - 1),
            (hi.y.max(0.0) as usize).min(ny - 1),
            (hi.z.max(0.0) as usize).min(nz - 1),
        ];
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let index = grid.idx(x, y, z);
                    if !field.inside[index] {
                        continue;
                    }
                    let point = grid.world(x, y, z);
                    let mut best = f64::INFINITY;
                    for (head, tail) in &bone.segments {
                        best = best.min(segment_distance(point, *head, *tail));
                    }
                    if best > reach || best >= f64::from(seeded[index]) {
                        continue;
                    }
                    seeded[index] = best as f32;
                    label[index] = slot as u32;
                }
            }
        }
    }
}

/// Grow the labelled flesh into the rest of the solid, deepest cell first.
///
/// A watershed ordered by inscribed radius puts the boundary at the narrow neck
/// between two cells — the crease — instead of halfway along a straight line
/// through fat. The depth scale is the field's own EDT, so no second distance
/// structure is introduced. Seeds are never relabelled: a seed is a cell's origin,
/// and the flood that reaches it first is not its owner.
fn grow(grid: &Grid, field: &MedialField, label: &mut [u32]) {
    const BINS: usize = 512;
    let top = field
        .radius
        .iter()
        .cloned()
        .fold(0.0_f32, f32::max)
        .max(1e-6);
    let bucket = |radius: f32| -> usize {
        (((radius / top) * (BINS - 1) as f32).round() as usize).min(BINS - 1)
    };
    let mut queues: Vec<Vec<u32>> = vec![Vec::new(); BINS];
    let mut settled: Vec<bool> = vec![false; grid.len()];
    for (index, &slot) in label.iter().enumerate() {
        if slot != UNLABELLED {
            settled[index] = true;
            queues[bucket(field.radius[index])].push(index as u32);
        }
    }
    for bin in (0..BINS).rev() {
        while let Some(index) = queues[bin].pop() {
            let slot = label[index as usize];
            let (x, y, z) = unflatten(grid, index as usize);
            for (nx, ny, nz) in neighbours(x, y, z, grid.dims) {
                let next = grid.idx(nx, ny, nz);
                if settled[next] || !field.inside[next] {
                    continue;
                }
                label[next] = slot;
                settled[next] = true;
                // A neighbour is normally shallower; when it is deeper its bucket
                // has already passed, so it joins this one rather than wait.
                queues[bucket(field.radius[next]).min(bin)].push(next as u32);
            }
        }
    }
}

/// Six-connectivity neighbours inside the grid.
fn neighbours(x: usize, y: usize, z: usize, dims: [usize; 3]) -> [(usize, usize, usize); 6] {
    [
        (x.saturating_sub(1), y, z),
        (x, y.saturating_sub(1), z),
        (x, y, z.saturating_sub(1)),
        ((x + 1).min(dims[0] - 1), y, z),
        (x, (y + 1).min(dims[1] - 1), z),
        (x, y, (z + 1).min(dims[2] - 1)),
    ]
}

/// Flat index back to (x, y, z), matching [`Grid::idx`].
fn unflatten(grid: &Grid, index: usize) -> (usize, usize, usize) {
    let x = index % grid.dims[0];
    let rest = index / grid.dims[0];
    let y = rest % grid.dims[1];
    (x, y, rest / grid.dims[1])
}

/// Distance from a point to a segment.
fn segment_distance(point: DVec3, head: DVec3, tail: DVec3) -> f64 {
    let axis = tail - head;
    let length_squared = axis.length_squared();
    if length_squared <= f64::EPSILON {
        return point.distance(head);
    }
    let t = ((point - head).dot(axis) / length_squared).clamp(0.0, 1.0);
    point.distance(head + axis * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed tube along +X: rings of `sides` vertices, capped at both ends.
    fn tube(
        length: f64,
        radius: f64,
        rings: usize,
        sides: usize,
    ) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let mut points = Vec::new();
        for ring in 0..rings {
            let centre = DVec3::X * (length * ring as f64 / (rings - 1) as f64);
            for side in 0..sides {
                let angle = std::f64::consts::TAU * side as f64 / sides as f64;
                points.push(
                    (centre
                        + DVec3::Y * (radius * angle.cos())
                        + DVec3::Z * (radius * angle.sin()))
                    .to_array(),
                );
            }
        }
        let mut triangles = Vec::new();
        let index = |ring: usize, side: usize| (ring * sides + side % sides) as u32;
        for ring in 0..rings - 1 {
            for side in 0..sides {
                let (a, b) = (index(ring, side), index(ring, side + 1));
                let (c, d) = (index(ring + 1, side), index(ring + 1, side + 1));
                triangles.push([a, c, b]);
                triangles.push([b, c, d]);
            }
        }
        let back = points.len() as u32;
        points.push((-DVec3::X * 0.001).to_array());
        points.push((DVec3::X * (length + 0.001)).to_array());
        for side in 0..sides {
            let (a, b) = (index(0, side), index(0, side + 1));
            triangles.push([b, a, back]);
            let (a, b) = (index(rings - 1, side), index(rings - 1, side + 1));
            triangles.push([a, b, back + 1]);
        }
        (points, triangles)
    }

    #[test]
    fn the_solid_is_partitioned_by_its_bones() {
        let (points, triangles) = tube(0.30, 0.05, 40, 24);
        let field = MedialField::from_mesh(&points, &triangles, 0.006);
        let bones = vec![
            CellBone {
                name: "Upper".into(),
                segments: vec![(DVec3::ZERO, DVec3::new(0.15, 0.0, 0.0))],
            },
            CellBone {
                name: "Lower".into(),
                segments: vec![(DVec3::new(0.15, 0.0, 0.0), DVec3::new(0.30, 0.0, 0.0))],
            },
        ];
        let cells = flesh_cells(&field, &bones, 0.020);
        assert_eq!(cells.volumes.len(), 2);
        assert!(
            cells.volumes.iter().all(|&volume| volume > 0),
            "every bone owns flesh: {:?}",
            cells.volumes
        );
        let unlabelled = (0..field.grid.len())
            .filter(|&index| field.inside[index] && cells.label[index] == UNLABELLED)
            .count();
        assert_eq!(unlabelled, 0, "the partition leaves no flesh unowned");
        assert_eq!(cells.nearest_cell(DVec3::new(0.05, 0.0, 0.0), 3), Some(0));
        assert_eq!(cells.nearest_cell(DVec3::new(0.26, 0.0, 0.0), 3), Some(1));
        let interface = cells
            .interfaces
            .get(&(0, 1))
            .copied()
            .expect("the two cells share a boundary");
        assert!(interface > 0.0, "the cut is a real surface: {interface} m");
    }

    #[test]
    fn a_vertex_inside_one_cell_gets_one_owner() {
        let (points, triangles) = tube(0.30, 0.05, 40, 24);
        let field = MedialField::from_mesh(&points, &triangles, 0.006);
        let bones = vec![CellBone {
            name: "Only".into(),
            segments: vec![(DVec3::ZERO, DVec3::new(0.30, 0.0, 0.0))],
        }];
        let cells = flesh_cells(&field, &bones, 0.020);
        let blend = cells
            .blend_at(DVec3::new(0.15, 0.05, 0.0), 0.010)
            .expect("the surface sits next to the one cell");
        assert_eq!(blend.owner, 0);
        assert_eq!(blend.shares, vec![(0, 1.0)], "one cell, one owner");
    }
}
