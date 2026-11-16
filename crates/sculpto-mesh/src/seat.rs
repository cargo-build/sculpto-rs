//! Seat a garment band onto the body at a declared clearance.
//!
//! Authored underwear is shaped to clear a whole preset family, not to fit
//! one body: the waistband the asset authors as one-size cover can stand
//! far enough off the torso that its top edge reads as a step in the
//! silhouette. [`seat_band`] moves one height band of the garment so the
//! band's outermost surface per direction sits at a caller-declared
//! clearance from the body, while the vertices below the band, the
//! garment's vertical extent and its triangle list are left untouched —
//! a seat can never open a hole the garment covered, because no vertex
//! moves along the up axis.
//!
//! The band seats as one displacement field, not per vertex: for every
//! (height slice, angular bin) the correction comes from the silhouette
//! radius — the outermost garment vertex in that bin — so the authored
//! layering inside the band (a waistband plate over the trunk wall)
//! translates together and the shells keep their order. Vertices the
//! shared correction would push inside the body are floored to half the
//! clearance, which compresses the authored relief instead of burying it.

use crate::check::SurfaceIndex;

/// Which frame axis is up: OBJ-derived meshes arrive in the source scene
/// frame (`Z`), synthetic geometry is usually `Y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpAxis {
    /// `+y` is up; the horizontal plane is `x, z`.
    #[default]
    Y,
    /// `+z` is up; the horizontal plane is `x, y`.
    Z,
}

impl UpAxis {
    /// The index of the up axis.
    pub fn index(self) -> usize {
        match self {
            UpAxis::Y => 1,
            UpAxis::Z => 2,
        }
    }

    /// The indices of the horizontal plane, in the order the seat measures.
    pub fn plane(self) -> [usize; 2] {
        match self {
            UpAxis::Y => [0, 2],
            UpAxis::Z => [0, 1],
        }
    }
}

/// Parameters of one band seat. Distances are in the meshes' own units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeatOptions {
    /// Height band `[lo, hi]` along the up axis whose silhouette is seated.
    pub band: [f32; 2],
    /// Target standoff outside the body surface for the band's silhouette.
    pub clearance: f32,
    /// Height above `band[0]` over which the correction fades in; the band's
    /// lower edge keeps its authored shape.
    pub blend: f32,
    /// Height of one slice the displacement field is sampled at.
    pub slice: f32,
    /// Angular bins the silhouette is measured over.
    pub bins: usize,
    /// Which frame axis is up.
    pub up: UpAxis,
}

impl Default for SeatOptions {
    fn default() -> Self {
        Self {
            band: [0.0, 0.0],
            clearance: 0.0,
            blend: 0.0,
            slice: 0.7,
            bins: 64,
            up: UpAxis::Y,
        }
    }
}

impl SeatOptions {
    /// A malformed seat refuses instead of moving anything: an empty band, a
    /// negative clearance or blend, a non-positive slice or fewer than eight
    /// bins have no meaning. @req DECL0013
    pub fn validate(&self) -> Result<(), SeatError> {
        let [lo, hi] = self.band;
        if lo >= hi {
            return Err(SeatError::Band(self.band));
        }
        if !self.clearance.is_finite() || self.clearance < 0.0 {
            return Err(SeatError::Clearance(self.clearance));
        }
        if !self.blend.is_finite() || self.blend < 0.0 || lo + self.blend > hi {
            return Err(SeatError::Blend(self.blend));
        }
        if !self.slice.is_finite() || self.slice <= 0.0 || self.slice > hi - lo {
            return Err(SeatError::Slice(self.slice));
        }
        if self.bins < 8 {
            return Err(SeatError::Bins(self.bins));
        }
        Ok(())
    }
}

/// Why a seat refused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SeatError {
    /// The band is empty or reversed.
    Band([f32; 2]),
    /// The clearance is negative or not finite.
    Clearance(f32),
    /// The blend is negative, not finite or wider than the band.
    Blend(f32),
    /// The slice is not positive, not finite or wider than the band.
    Slice(f32),
    /// Fewer than eight bins cannot carry a silhouette.
    Bins(usize),
}

impl std::fmt::Display for SeatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            SeatError::Band([lo, hi]) => {
                write!(f, "the band [{lo}, {hi}] is empty or reversed")
            }
            SeatError::Clearance(value) => {
                write!(f, "the clearance {value} is negative or not finite")
            }
            SeatError::Blend(value) => {
                write!(
                    f,
                    "the blend {value} is negative, not finite or wider than the band"
                )
            }
            SeatError::Slice(value) => {
                write!(
                    f,
                    "the slice {value} is not positive, not finite or wider than the band"
                )
            }
            SeatError::Bins(count) => write!(f, "{count} bins cannot carry a silhouette (8 least)"),
        }
    }
}

/// One sampled slice of the seat, for the report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeatSlice {
    /// Slice centre on the up axis.
    pub height: f32,
    /// Bins whose silhouette and body radius both resolved.
    pub bins_seated: usize,
    /// The silhouette's worst standoff before the seat: the largest
    /// `silhouette - body` radius gap over the seated bins.
    pub worst_before: f32,
    /// The same gap after the seat.
    pub worst_after: f32,
}

/// What one seat did.
#[derive(Debug, Clone, PartialEq)]
pub struct SeatReport {
    /// Vertices of the garment inside the band.
    pub in_band: usize,
    /// Vertices whose position changed, blend-weighted or clamped.
    pub moved: usize,
    /// Largest displacement length applied.
    pub max_shift: f32,
    /// Vertices floored to half the clearance instead of following the
    /// shared correction, so they stay outside the body.
    pub clamped: usize,
    /// The sampled slices, low to high.
    pub slices: Vec<SeatSlice>,
}

/// Seat the garment band described by `options` onto the body.
///
/// Both meshes arrive in one frame, up axis `+y`; the body is the surface
/// the band must meet. The returned positions keep the input length and the
/// input triangles are used unmodified — a seat is a displacement, never a
/// topology change. @req DECL0013
pub fn seat_band(
    garment: &[[f32; 3]],
    body_positions: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
    options: &SeatOptions,
) -> Result<(Vec<[f32; 3]>, SeatReport), SeatError> {
    options.validate()?;
    let index = SurfaceIndex::build(body_positions, body_triangles);
    let [lo, _hi] = options.band;
    let slice = options.slice;
    let up = options.up.index();
    let [plane_a, plane_b] = options.up.plane();
    let point_at = |horizontal: [f32; 2], height: f32| -> [f32; 3] {
        let mut point = [0.0_f32; 3];
        point[plane_a] = horizontal[0];
        point[plane_b] = horizontal[1];
        point[up] = height;
        point
    };
    let mut height = lo + 0.5 * slice;
    let mut slice_centres = Vec::new();
    while height < options.band[1] {
        slice_centres.push(height);
        height += slice;
    }
    if slice_centres.is_empty() {
        return Ok((
            garment.to_vec(),
            SeatReport {
                in_band: 0,
                moved: 0,
                max_shift: 0.0,
                clamped: 0,
                slices: Vec::new(),
            },
        ));
    }

    // Body vertices per slice: the centre the silhouette measures from.
    let mut centres = vec![None; slice_centres.len()];
    let mut body_of_slice: Vec<Vec<[f32; 2]>> = vec![Vec::new(); slice_centres.len()];
    for &point in body_positions {
        if let Some(slot) = slice_slot(point[up], &slice_centres, slice) {
            body_of_slice[slot].push([point[plane_a], point[plane_b]]);
        }
    }
    for (slot, points) in body_of_slice.iter().enumerate() {
        if points.len() >= 3 {
            let mut centre = [0.0_f32; 2];
            for point in points {
                centre[0] += point[0];
                centre[1] += point[1];
            }
            centres[slot] = Some([
                centre[0] / points.len() as f32,
                centre[1] / points.len() as f32,
            ]);
        }
    }

    // The silhouette radius and the body radius per (slice, bin).
    let tau = std::f32::consts::TAU;
    let mut in_band = 0_usize;
    let mut silhouette = vec![vec![f32::NEG_INFINITY; options.bins]; slice_centres.len()];
    for &point in garment {
        if point[up] < options.band[0] || point[up] > options.band[1] {
            continue;
        }
        in_band += 1;
        let Some(slot) = slice_slot(point[up], &slice_centres, slice) else {
            continue;
        };
        let Some(centre) = centres[slot] else {
            continue;
        };
        let dx = point[plane_a] - centre[0];
        let dz = point[plane_b] - centre[1];
        let radius = (dx * dx + dz * dz).sqrt();
        if radius <= f32::EPSILON {
            continue;
        }
        let bin = bin_of(dx, dz, options.bins);
        silhouette[slot][bin] = silhouette[slot][bin].max(radius);
    }
    let mut body_radius = vec![vec![f32::NAN; options.bins]; slice_centres.len()];
    for (slot, centre) in centres.iter().enumerate() {
        let Some(centre) = centre else {
            continue;
        };
        let origin = point_at(*centre, slice_centres[slot]);
        for bin in 0..options.bins {
            // The body this bin seats against must lie at or inside the
            // garment's own silhouette: a band stands off a surface it
            // covers, never floats inside empty space outside the body. A
            // first hit beyond the silhouette is a different surface — the
            // arms beside the waist, the far side of an open band — and the
            // bin keeps its authored shape instead of seating onto it.
            let outer = silhouette[slot][bin];
            if !outer.is_finite() {
                continue;
            }
            let angle = bin as f32 * tau / options.bins as f32;
            let horizontal = [angle.cos(), angle.sin()];
            let direction = point_at(horizontal, 0.0);
            if let Some((_, distance)) =
                index.first_hit(origin, direction, 0.0, outer + options.clearance, &[])
            {
                body_radius[slot][bin] = distance;
            }
        }
    }

    // The shared correction per (slice, bin): where the band's silhouette or
    // the body radius did not resolve, the bin keeps its authored shape.
    let mut correction = vec![vec![0.0_f32; options.bins]; slice_centres.len()];
    let mut seated_bins = vec![0_usize; slice_centres.len()];
    for slot in 0..slice_centres.len() {
        for bin in 0..options.bins {
            let outer = silhouette[slot][bin];
            let floor = body_radius[slot][bin];
            if !outer.is_finite() || floor.is_nan() {
                continue;
            }
            correction[slot][bin] = floor + options.clearance - outer;
            seated_bins[slot] += 1;
        }
    }

    // Apply the field: every band vertex follows its bin's correction,
    // faded in over the blend and floored so nothing crosses the body.
    let mut seated_positions = garment.to_vec();
    let mut moved = 0;
    let mut clamped = 0;
    let mut max_shift = 0.0_f32;
    for point in seated_positions.iter_mut() {
        if point[up] < options.band[0] || point[up] > options.band[1] {
            continue;
        }
        let weight = blend_weight(point[up], options);
        if weight <= 0.0 {
            continue;
        }
        let Some((low, high, fraction)) = bracket(point[up], &slice_centres, slice) else {
            continue;
        };
        let centre = centres[low].or(centres[high]);
        let Some(centre) = centre else {
            continue;
        };
        let dx = point[plane_a] - centre[0];
        let dz = point[plane_b] - centre[1];
        let radius = (dx * dx + dz * dz).sqrt();
        if radius <= f32::EPSILON {
            continue;
        }
        let bin = bin_of(dx, dz, options.bins);
        let delta = correction[low][bin].mul_add(1.0 - fraction, correction[high][bin] * fraction);
        let shift = delta * weight;
        if shift == 0.0 {
            continue;
        }
        let body_r = if body_radius[low][bin].is_nan() {
            body_radius[high][bin]
        } else {
            body_radius[low][bin]
        };
        let floor = if body_r.is_nan() {
            f32::NEG_INFINITY
        } else {
            body_r + 0.5 * options.clearance
        };
        let new_radius = (radius + shift).max(floor);
        if new_radius == radius {
            continue;
        }
        if (radius + shift) < floor {
            clamped += 1;
        }
        max_shift = max_shift.max((new_radius - radius).abs());
        let scale = new_radius / radius;
        point[plane_a] = centre[0] + dx * scale;
        point[plane_b] = centre[1] + dz * scale;
        moved += 1;
    }

    // Measure the seat on the geometry it produced: the worst radial gap of
    // the seated silhouette against the body, per slice.
    let mut slice_rows = Vec::new();
    for slot in 0..slice_centres.len() {
        let Some(centre) = centres[slot] else {
            slice_rows.push(SeatSlice {
                height: slice_centres[slot],
                bins_seated: 0,
                worst_before: f32::NAN,
                worst_after: f32::NAN,
            });
            continue;
        };
        let mut after = vec![f32::NEG_INFINITY; options.bins];
        for &point in &seated_positions {
            if slice_slot(point[up], &slice_centres, slice) != Some(slot) {
                continue;
            }
            let dx = point[plane_a] - centre[0];
            let dz = point[plane_b] - centre[1];
            let radius = (dx * dx + dz * dz).sqrt();
            if radius <= f32::EPSILON {
                continue;
            }
            let bin = bin_of(dx, dz, options.bins);
            after[bin] = after[bin].max(radius);
        }
        let mut worst_before = f32::NEG_INFINITY;
        let mut worst_after = f32::NEG_INFINITY;
        for bin in 0..options.bins {
            let floor = body_radius[slot][bin];
            if floor.is_nan() {
                continue;
            }
            if silhouette[slot][bin].is_finite() {
                worst_before = worst_before.max(silhouette[slot][bin] - floor);
            }
            if after[bin].is_finite() {
                worst_after = worst_after.max(after[bin] - floor);
            }
        }
        slice_rows.push(SeatSlice {
            height: slice_centres[slot],
            bins_seated: seated_bins[slot],
            worst_before,
            worst_after,
        });
    }

    Ok((
        seated_positions,
        SeatReport {
            in_band,
            moved,
            max_shift,
            clamped,
            slices: slice_rows,
        },
    ))
}

/// The slice a height falls into, if any. The top half-slice above the last
/// centre belongs to it, so a vertex on the band's top edge still seats.
fn slice_slot(height: f32, centres: &[f32], slice: f32) -> Option<usize> {
    if centres.is_empty() {
        return None;
    }
    let last = centres.len() - 1;
    let slot = (height - centres[0]) / slice + 0.5;
    if !(0.0..=last as f32 + 1.0).contains(&slot) {
        return None;
    }
    Some((slot.floor() as usize).min(last))
}

/// The two bracketing slices and the interpolation fraction between them.
fn bracket(height: f32, centres: &[f32], slice: f32) -> Option<(usize, usize, f32)> {
    let slot = slice_slot(height, centres, slice)?;
    let low = slot.min(centres.len() - 1);
    let high = (low + 1).min(centres.len() - 1);
    let span = centres[high] - centres[low];
    let fraction = if span.abs() <= f32::EPSILON {
        0.0
    } else {
        ((height - centres[low]) / span).clamp(0.0, 1.0)
    };
    Some((low, high, fraction))
}

/// The angular bin of a horizontal offset, 0 at `+x` growing towards `+z`.
fn bin_of(dx: f32, dz: f32, bins: usize) -> usize {
    let angle = dz.atan2(dx);
    let tau = std::f32::consts::TAU;
    let bin = (angle / tau * bins as f32).round().rem_euclid(bins as f32);
    bin as usize
}

/// The correction's fade-in over the blend, 0 below the band to 1 above it.
fn blend_weight(height: f32, options: &SeatOptions) -> f32 {
    let t = if options.blend <= f32::EPSILON {
        1.0
    } else {
        ((height - options.band[0]) / options.blend).clamp(0.0, 1.0)
    };
    // Smoothstep keeps the transition's slope zero at both ends.
    t * t * (3.0 - 2.0 * t)
}
