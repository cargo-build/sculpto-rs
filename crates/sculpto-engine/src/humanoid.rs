//! Geometry-conditioned humanoid pivots; the template supplies graph and roll. @req DECL0013

use std::collections::BTreeMap;

use glam::{DMat3, DMat4, DQuat, DVec2, DVec3};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_mesh::actor::{Actor, Pose, Section, SectionLoop, horizontal_sections};
use serde_json::{Value, json};

use crate::CommandError;
use crate::export_glb::{actor_error, refused};

mod ankle;
mod elbow;
mod knee;
mod profile;
mod toes;

const STEPS: usize = 1000;

struct JointEstimate {
    pivot: DVec3,
    report: Value,
}

/// The existing generic rebind still accepts smaller, non-humanoid palettes.
pub(crate) fn supports(palette: &[String]) -> bool {
    palette.len() == 65
        && palette.iter().all(|name| name.starts_with("mixamorig:"))
        && [
            "Hips",
            "Head",
            "LeftHand",
            "RightHand",
            "LeftFoot",
            "RightFoot",
        ]
        .iter()
        .all(|name| {
            palette
                .iter()
                .any(|value| value == &format!("mixamorig:{name}"))
        })
}

fn toe_marker(name: &str) -> bool {
    matches!(name, "mixamorig:LeftToe_End" | "mixamorig:RightToe_End")
}

/// The outside-flesh exception is valid only for auxiliary endpoints with zero influence.
/// Validate the encoded skin, not an expectation based on node names. @req DECL0013
pub(crate) fn validate_marker_weights(
    palette: &[String],
    mass: &[f64],
) -> Result<(), CommandError> {
    if palette.len() != mass.len() {
        return Err(refused(
            "humanoid_toe_marker",
            "palette and weight census lengths differ",
        ));
    }
    if let Some((name, weight)) = palette
        .iter()
        .zip(mass)
        .find(|(n, w)| toe_marker(n) && **w > 0.0)
    {
        return Err(refused(
            "humanoid_toe_marker",
            format!("auxiliary endpoint {name} carries weight mass {weight}"),
        ));
    }
    Ok(())
}

fn foot_axes(seed: DVec3, forefoot: DVec3) -> Result<(DVec3, DVec3), CommandError> {
    let forward = DVec3::new(forefoot.x - seed.x, 0.0, forefoot.z - seed.z)
        .try_normalize()
        .ok_or_else(|| refused("humanoid_ankle", "foot direction is unresolved"))?;
    Ok((forward, DVec3::Y.cross(forward) * seed.x.signum()))
}

struct Surface {
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    basis: DMat3,
    origin: DVec3,
    height: f64,
    width: f64,
    sections: Vec<Section>,
}

impl Surface {
    fn new(vertices: &[[f64; 3]], triangles: &[[u32; 3]]) -> Result<Self, CommandError> {
        let used: Vec<DVec3> = triangles
            .iter()
            .flatten()
            .map(|index| {
                vertices
                    .get(*index as usize)
                    .copied()
                    .map(DVec3::from_array)
            })
            .collect::<Option<_>>()
            .ok_or_else(|| refused("humanoid_surface", "triangle index outside the surface"))?;
        if used.is_empty() || used.iter().any(|p| !p.is_finite()) {
            return Err(refused("humanoid_surface", "empty or nonfinite surface"));
        }
        let lo = used
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |a, p| a.min(*p));
        let hi = used
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
        let height = hi.y - lo.y;
        if height <= 1e-6 {
            return Err(refused("humanoid_surface", "surface has no Y-up stature"));
        }
        // Area-weighted horizontal covariance measures the frame, never a joint pivot.
        let (mass, mean, xx, xz, zz) = triangles.iter().fold(
            (0.0, DVec3::ZERO, 0.0, 0.0, 0.0),
            |(mass, mean, xx, xz, zz), t| {
                let [a, b, c] = t.map(|i| DVec3::from_array(vertices[i as usize]));
                let area = (b - a).cross(c - a).length() * 0.5;
                let p = (a + b + c) / 3.0;
                (
                    mass + area,
                    mean + p * area,
                    xx + p.x * p.x * area,
                    xz + p.x * p.z * area,
                    zz + p.z * p.z * area,
                )
            },
        );
        if mass <= 0.0 {
            return Err(refused("humanoid_surface", "surface has no triangle area"));
        }
        let mean = mean / mass;
        let angle = 0.5
            * (2.0 * (xz / mass - mean.x * mean.z))
                .atan2(xx / mass - mean.x * mean.x - zz / mass + mean.z * mean.z);
        let mut lateral = DVec3::new(angle.cos(), 0.0, angle.sin());
        let mut anterior = DVec3::Y.cross(lateral);
        let projected_mid = |low: f64, high: f64| -> Option<f64> {
            let (min, max) = used
                .iter()
                .filter(|p| p.y >= lo.y + low * height && p.y <= lo.y + high * height)
                .map(|p| p.dot(anterior))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            (min.is_finite() && max.is_finite()).then_some((min + max) * 0.5)
        };
        let feet =
            projected_mid(0.0, 0.07).ok_or_else(|| refused("humanoid_frame", "feet absent"))?;
        let shins =
            projected_mid(0.10, 0.18).ok_or_else(|| refused("humanoid_frame", "shins absent"))?;
        if (feet - shins).abs() < height * 0.005 {
            return Err(refused(
                "humanoid_frame",
                "foot projection does not resolve anterior direction",
            ));
        }
        if feet < shins {
            lateral = -lateral;
            anterior = -anterior;
        }
        let basis = DMat3::from_cols(lateral, DVec3::Y, anterior);
        let projected: Vec<DVec3> = used.iter().map(|p| basis.transpose() * *p).collect();
        let min = projected
            .iter()
            .fold(DVec3::splat(f64::INFINITY), |a, p| a.min(*p));
        let max = projected
            .iter()
            .fold(DVec3::splat(f64::NEG_INFINITY), |a, p| a.max(*p));
        let origin = basis * DVec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
        let normalized: Vec<[f64; 3]> = vertices
            .iter()
            .map(|p| (basis.transpose() * (DVec3::from_array(*p) - origin)).to_array())
            .collect();
        let heights: Vec<f64> = (1..STEPS)
            .map(|i| height * i as f64 / STEPS as f64)
            .collect();
        let sections =
            horizontal_sections(&normalized, triangles, &heights).map_err(actor_error)?;
        Ok(Self {
            vertices: normalized,
            triangles: triangles.to_vec(),
            basis,
            origin,
            height,
            width: max.x - min.x,
            sections,
        })
    }

    fn local(&self, p: DVec3) -> DVec3 {
        self.basis.transpose() * (p - self.origin)
    }
    fn world(&self, p: DVec3) -> DVec3 {
        self.origin + self.basis * p
    }

    fn cut(&self, y: f64) -> Result<Section, CommandError> {
        horizontal_sections(&self.vertices, &self.triangles, &[y])
            .map_err(actor_error)?
            .into_iter()
            .next()
            .ok_or_else(|| refused("humanoid_section", "section result absent"))
    }

    fn central(&self, y: f64) -> Result<SectionLoop, CommandError> {
        self.cut(y)?
            .loops
            .into_iter()
            .filter(|r| {
                r.closed
                    && r.area.abs() > self.height.powi(2) * 0.00025
                    && r.centroid[0].abs() < self.height * 0.03
            })
            .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
            .ok_or_else(|| {
                refused(
                    "humanoid_section",
                    format!("no resolved central section at {y:.6} m"),
                )
            })
    }

    fn neck(&self) -> Result<(f64, &SectionLoop), CommandError> {
        self.sections
            .iter()
            .filter(|s| s.plane > self.height * 0.83 && s.plane < self.height * 0.91)
            .flat_map(|s| s.loops.iter().map(move |r| (s.plane, r)))
            .filter(|(_, r)| {
                r.closed
                    && r.centroid[0].abs() < self.height * 0.03
                    && r.area.abs() > self.height.powi(2) * 0.00025
            })
            .min_by(|(_, a), (_, b)| a.area.abs().total_cmp(&b.area.abs()))
            .ok_or_else(|| refused("humanoid_neck", "no resolved neck section"))
    }

    fn crotch(&self) -> Result<f64, CommandError> {
        self.triangles
            .iter()
            .flat_map(|t| {
                let p = t.map(|i| DVec3::from_array(self.vertices[i as usize]));
                [(0, 1), (1, 2), (2, 0)]
                    .into_iter()
                    .filter(move |&(a, b)| (p[a].x > 0.0) != (p[b].x > 0.0))
                    .map(move |(a, b)| p[a].lerp(p[b], p[a].x / (p[a].x - p[b].x)))
            })
            .filter(|p| p.y > self.height * 0.35 && p.y < self.height * 0.65)
            .map(|p| p.y)
            .min_by(f64::total_cmp)
            .ok_or_else(|| {
                refused(
                    "humanoid_pelvis",
                    "surface does not resolve a sagittal crotch",
                )
            })
    }

    fn thighs(&self, crotch: f64) -> Result<(DVec3, f64), CommandError> {
        let y = crotch - self.height * 0.015;
        let loops: Vec<SectionLoop> = self
            .cut(y)?
            .loops
            .into_iter()
            .filter(|r| {
                r.closed
                    && r.area.abs() > self.height.powi(2) * 0.003
                    && r.centroid[0].abs() < self.height * 0.2
            })
            .collect();
        if loops.len() != 2 {
            return Err(refused(
                "humanoid_pelvis",
                format!("expected two proximal thigh loops, found {}", loops.len()),
            ));
        }
        let centre = (centre(&loops[0], y) + centre(&loops[1], y)) * 0.5;
        let radius = loops
            .iter()
            .map(|r| (r.area.abs() / std::f64::consts::PI).sqrt())
            .sum::<f64>()
            * 0.5;
        Ok((centre, radius))
    }
}

fn centre(r: &SectionLoop, y: f64) -> DVec3 {
    DVec3::new(r.centroid[0], y, r.centroid[1])
}

fn point(nodes: &BTreeMap<String, DVec3>, name: &str) -> Result<DVec3, CommandError> {
    nodes
        .get(name)
        .copied()
        .ok_or_else(|| refused("humanoid_palette", format!("missing {name}")))
}

fn at_fraction(path: &[DVec3], fraction: f64) -> Result<DVec3, CommandError> {
    let total = path.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>();
    if total <= 1e-9 || path.len() < 2 {
        return Err(refused("humanoid_chain", "a measured chain collapsed"));
    }
    let mut remaining = total * fraction.clamp(0.0, 1.0);
    for pair in path.windows(2) {
        let length = pair[0].distance(pair[1]);
        if length > 1e-12 && remaining <= length {
            return Ok(pair[0].lerp(pair[1], remaining / length));
        }
        remaining -= length;
    }
    path.last()
        .copied()
        .ok_or_else(|| refused("humanoid_chain", "path has no distal point"))
}

fn chain_fractions(
    nodes: &BTreeMap<String, DVec3>,
    names: &[String],
) -> Result<Vec<f64>, CommandError> {
    let points: Vec<DVec3> = names
        .iter()
        .map(|n| point(nodes, n))
        .collect::<Result<_, _>>()?;
    let lengths: Vec<f64> = points.windows(2).map(|p| p[0].distance(p[1])).collect();
    let total = lengths.iter().sum::<f64>();
    if total <= 1e-9 || lengths.iter().any(|length| *length <= 1e-9) {
        return Err(refused(
            "humanoid_template",
            "template has a collapsed segment",
        ));
    }
    Ok(std::iter::once(0.0)
        .chain(lengths.iter().scan(0.0, |sum, length| {
            *sum += length;
            Some(*sum / total)
        }))
        .collect())
}

fn loop_distance(r: &SectionLoop, p: DVec2) -> f64 {
    let mut crossings = 0;
    let mut distance = f64::INFINITY;
    for (a, b) in r
        .ring
        .iter()
        .zip(r.ring.iter().cycle().skip(1))
        .take(r.ring.len())
    {
        let a = DVec2::from_array(*a);
        let b = DVec2::from_array(*b);
        let edge = b - a;
        if edge.length_squared() > 1e-20 {
            let t = ((p - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
            distance = distance.min(p.distance(a + edge * t));
        }
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            crossings += 1;
        }
    }
    match crossings % 2 {
        1 => 0.0,
        _ => distance,
    }
}

fn depth_chord(r: &SectionLoop, x: f64) -> Result<f64, CommandError> {
    let mut hits: Vec<f64> = r
        .ring
        .iter()
        .zip(r.ring.iter().cycle().skip(1))
        .take(r.ring.len())
        .filter(|(a, b)| (a[0] > x) != (b[0] > x))
        .map(|(a, b)| a[1] + (x - a[0]) * (b[1] - a[1]) / (b[0] - a[0]))
        .collect();
    hits.sort_by(f64::total_cmp);
    match hits.as_slice() {
        [front, back] => Ok((front + back) * 0.5),
        _ => Err(refused(
            "humanoid_shoulder",
            format!("depth chord crosses {} faces", hits.len()),
        )),
    }
}

fn torso_alignment(
    source: &BTreeMap<String, DVec3>,
    hips: DVec3,
    head: DVec3,
) -> Result<(DQuat, f64), CommandError> {
    let original = point(source, "Head")? - point(source, "Hips")?;
    let target = head - hips;
    if original.length() <= 1e-9 || target.length() <= 1e-9 {
        return Err(refused("humanoid_torso", "torso axis collapsed"));
    }
    Ok((
        DQuat::from_rotation_arc(original.normalize(), target.normalize()),
        target.length() / original.length(),
    ))
}

/// Project the measured arm prior into the template girdle plane in the fitted torso frame.
/// Final clavicle-root proportions are resolved after this endpoint. @req DECL0013
fn project_arm_prior(
    source: &BTreeMap<String, DVec3>,
    placed: &BTreeMap<String, DVec3>,
    side: &str,
    surface_prior: DVec3,
) -> Result<(DVec3, Value), CommandError> {
    let (rotation, _) = torso_alignment(source, point(placed, "Hips")?, point(placed, "Head")?)?;
    let branch = rotation * (point(source, &format!("{side}Shoulder"))? - point(source, "Spine2")?);
    let clavicle = rotation
        * (point(source, &format!("{side}Arm"))? - point(source, &format!("{side}Shoulder"))?);
    let origin = point(placed, "Spine2")?;
    let target = surface_prior - origin;
    let aa = branch.length_squared();
    let bb = clavicle.length_squared();
    let ab = branch.dot(clavicle);
    let determinant = aa * bb - ab * ab;
    if determinant <= aa * bb * 1e-8 {
        return Err(refused(
            "humanoid_shoulder",
            "template girdle segments are collinear",
        ));
    }
    let a = (branch.dot(target) * bb - clavicle.dot(target) * ab) / determinant;
    let b = (clavicle.dot(target) * aa - branch.dot(target) * ab) / determinant;
    if a <= 0.0 || b <= 0.0 || !a.is_finite() || !b.is_finite() {
        return Err(refused(
            "humanoid_shoulder",
            "surface prior reverses a girdle segment",
        ));
    }
    let root = origin + branch * a;
    let ball = root + clavicle * b;
    Ok((
        ball,
        json!({"method":"arm prior projected into the template girdle plane",
        "branch_basis_scale":a,"clavicle_basis_scale":b,"surface_prior_m":surface_prior.to_array(),
        "surface_prior_residual_m":ball.distance(surface_prior)}),
    ))
}

/// Preserve the template's two-segment proportion and angle between fixed body endpoints.
/// This experimental prior is qualified on Emily first. @req DECL0013
fn proportional_clavicle(
    source: &BTreeMap<String, DVec3>,
    side: &str,
    spine: DVec3,
    arm: DVec3,
) -> Result<(DVec3, Value), CommandError> {
    let origin = point(source, "Spine2")?;
    let middle = point(source, &format!("{side}Shoulder"))?;
    let end = point(source, &format!("{side}Arm"))?;
    let original = end - origin;
    let target = arm - spine;
    let first = middle.distance(origin);
    let second = end.distance(middle);
    if original.length() < 1e-8 || target.length() < 1e-8 || first < 1e-8 || second < 1e-8 {
        return Err(refused("humanoid_shoulder", "girdle chain collapsed"));
    }
    let rotation = DQuat::from_rotation_arc(original.normalize(), target.normalize());
    let scale = target.length() / original.length();
    let clavicle = spine + rotation * (middle - origin) * scale;
    Ok((
        clavicle,
        json!({"method":"two-segment template similarity with fixed Spine2 and Arm",
        "template_lengths_m":[first,second],"placed_lengths_m":[clavicle.distance(spine),arm.distance(clavicle)],
        "template_ratio":first/second,"placed_ratio":clavicle.distance(spine)/arm.distance(clavicle),
        "uniform_scale":scale,"endpoint_alignment_degrees":original.angle_between(target).to_degrees(),
        "uncertainty":"Operator-selected experimental girdle proportion; other-body generalization is unverified."}),
    ))
}

fn axis_at_height(points: &[DVec3], height: f64) -> Result<(DVec3, f64), CommandError> {
    if points.len() < 6 {
        return Err(refused(
            "humanoid_shoulder",
            "too few isolated upper-arm sections",
        ));
    }
    let mean = points.iter().copied().sum::<DVec3>() / points.len() as f64;
    let yy = points.iter().map(|p| (p.y - mean.y).powi(2)).sum::<f64>();
    if yy <= 1e-10 {
        return Err(refused(
            "humanoid_shoulder",
            "upper-arm section span collapsed",
        ));
    }
    let slope = points
        .iter()
        .map(|p| (*p - mean) * (p.y - mean.y))
        .sum::<DVec3>()
        / yy;
    let rms = (points
        .iter()
        .map(|p| p.distance_squared(mean + slope * (p.y - mean.y)))
        .sum::<f64>()
        / points.len() as f64)
        .sqrt();
    Ok((mean + slope * (height - mean.y), rms))
}

/// The upper-arm shaft supplies lateral position and depth; stature alone cannot measure
/// shoulder breadth. The girdle height remains an explicit template prior. @req DECL0013
fn upper_arm_axis(
    body: &Surface,
    rows: &[(usize, &SectionLoop)],
    elbow: DVec3,
    height: f64,
) -> Result<(DVec3, Value), CommandError> {
    let points = |fraction| {
        rows.iter()
            .filter_map(|(i, r)| {
                let y = body.sections[*i].plane;
                (y > elbow.y + fraction * (height - elbow.y)
                    && y < height
                    && body.sections[*i].loops.iter().filter(|r| r.closed).count() >= 3)
                    .then(|| centre(r, y))
            })
            .collect::<Vec<_>>()
    };
    let samples = points(0.25);
    let (prior, rms) = axis_at_height(&samples, height)?;
    let (wide, _) = axis_at_height(&points(0.20), height)?;
    let (narrow, _) = axis_at_height(&points(0.30), height)?;
    let first = samples
        .first()
        .ok_or_else(|| refused("humanoid_shoulder", "upper-arm axis absent"))?;
    let last = samples
        .last()
        .ok_or_else(|| refused("humanoid_shoulder", "upper-arm axis absent"))?;
    Ok((
        prior,
        json!({"method":"isolated upper-arm section axis at template girdle height",
        "section_count":samples.len(),"measured_y_range_m":[first.y,last.y],
        "section_axis_rms_m":rms,"extrapolation_m":height-last.y,
        "selection_sensitivity_m":prior.distance(wide).max(prior.distance(narrow)),
        "uncertainty":"The surface determines a shaft axis; shoulder height remains a template prior, and an anatomical joint centre is not directly observed."}),
    ))
}

/// Preserve the template head axis while its own skull surface supplies the length.
/// A crown-section centroid would discard the authored inclination. @req DECL0013
fn head_tip(
    body: &Surface,
    reference: &Surface,
    source: &BTreeMap<String, DVec3>,
    head: DVec3,
) -> Result<(DVec3, Value), CommandError> {
    let source_head = point(source, "Head")?;
    let source_axis = point(source, "HeadTop_End")? - source_head;
    let direction = source_axis
        .try_normalize()
        .ok_or_else(|| refused("humanoid_head", "template head axis collapsed"))?;
    let extent = |surface: &Surface, origin| {
        crate::medial::first_surface_hit(&surface.vertices, &surface.triangles, origin, direction)
            .ok_or_else(|| {
                refused(
                    "humanoid_head",
                    "head-axis surface intersection is unresolved",
                )
            })
    };
    let source_extent = extent(reference, source_head)?;
    let body_extent = extent(body, head)?;
    let fraction = source_axis.length() / source_extent;
    if !(0.0..=1.0).contains(&fraction) {
        return Err(refused(
            "humanoid_head",
            "template head endpoint extends outside its surface",
        ));
    }
    let tip = head + direction * (body_extent * fraction);
    Ok((
        tip,
        json!({"method":"template head-axis direction and endpoint-to-surface fraction",
        "direction_body_frame":direction.to_array(),"template_surface_extent_m":source_extent,
        "body_surface_extent_m":body_extent,"endpoint_surface_fraction":fraction,
        "head_pivot_unchanged":true,
        "uncertainty":"The template defines the head frame; the observed surface limits its endpoint, not the internal anatomy."}),
    ))
}

fn torso(
    body: &Surface,
    reference: &Surface,
    source: &BTreeMap<String, DVec3>,
    placed: &mut BTreeMap<String, DVec3>,
) -> Result<Value, CommandError> {
    let crotch = body.crotch()?;
    let source_crotch = reference.crotch()?;
    let (thigh, radius) = body.thighs(crotch)?;
    let (source_thigh, source_radius) = reference.thighs(source_crotch)?;
    let scale = radius / source_radius;
    let sockets = (point(source, "LeftUpLeg")? + point(source, "RightUpLeg")?) * 0.5;
    let socket = DVec3::new(
        0.0,
        crotch + (sockets.y - source_crotch) * scale,
        thigh.z + (sockets.z - source_thigh.z) * scale,
    );
    let hips = socket + (point(source, "Hips")? - sockets) * scale;
    let half_width =
        (point(source, "LeftUpLeg")?.x - point(source, "RightUpLeg")?.x).abs() * 0.5 * scale;
    placed.insert("LeftUpLeg".into(), socket - DVec3::X * half_width);
    placed.insert("RightUpLeg".into(), socket + DVec3::X * half_width);
    let (neck_y, _) = body.neck()?;
    let (source_neck_y, _) = reference.neck()?;
    let head_scale = (body.height - neck_y) / (reference.height - source_neck_y);
    let source_head = point(source, "Head")?;
    let head_y = neck_y + (source_head.y - source_neck_y) * head_scale;
    let head = DVec3::new(
        0.0,
        head_y,
        body.central(head_y)?.centroid[1]
            + (source_head.z - reference.central(source_head.y)?.centroid[1]) * head_scale,
    );
    let (rotation, spine_scale) = torso_alignment(source, hips, head)?;
    for name in ["Hips", "Spine", "Spine1", "Spine2", "Neck", "Head"] {
        placed.insert(
            name.into(),
            hips + rotation * (point(source, name)? - point(source, "Hips")?) * spine_scale,
        );
    }
    let (tip, head_report) = head_tip(body, reference, source, head)?;
    placed.insert("HeadTop_End".into(), tip);
    Ok(json!({"crotch_m":crotch,"template_crotch_m":source_crotch,
        "thigh_equivalent_radius_m":radius,"template_thigh_radius_m":source_radius,
        "neck_section_m":neck_y,"template_neck_section_m":source_neck_y,"head_tip":head_report,
        "uncertainty":"Hip and skull-base positions are surface-conditioned template priors; internal anatomy is unobserved."}))
}

struct Track<'a> {
    section: &'a SectionLoop,
    path: Vec<DVec3>,
    split: usize,
}

fn arm(
    body: &Surface,
    reference: &Surface,
    source: &BTreeMap<String, DVec3>,
    side: &str,
    sign: f64,
    placed: &mut BTreeMap<String, DVec3>,
) -> Result<Value, CommandError> {
    let scale = body.height / reference.height;
    let source_arm = point(source, &format!("{side}Arm"))?;
    let mut surface_prior = source_arm * scale;
    surface_prior.z = depth_chord(&body.central(surface_prior.y)?, surface_prior.x)?;
    let (seed_shoulder, _) = project_arm_prior(source, placed, side, surface_prior)?;
    let lateral = |r: &&SectionLoop| {
        r.closed
            && sign * r.centroid[0] > body.width * 0.18
            && r.area.abs() > body.height.powi(2) * 0.0000003
    };
    let rows: Vec<(usize, &SectionLoop)> = body
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.plane > body.height * 0.40 && s.plane < body.height * 0.75)
        .filter_map(|(i, s)| {
            s.loops
                .iter()
                .filter(lateral)
                .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
                .map(|r| (i, r))
        })
        .collect();
    let &(palm_i, _) = rows
        .iter()
        .filter(|(i, _)| {
            let y = body.sections[*i].plane;
            y > body.height * 0.48 && y < body.height * 0.55
        })
        .max_by(|(_, a), (_, b)| a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| refused("humanoid_palm", format!("{side}: palm lobe absent")))?;
    let &(wrist_i, wrist_loop) = rows
        .iter()
        .filter(|(i, _)| {
            let y = body.sections[*i].plane;
            y > body.sections[palm_i].plane + body.height * 0.01 && y < body.height * 0.63
        })
        .min_by(|(_, a), (_, b)| a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| refused("humanoid_wrist", format!("{side}: wrist minimum absent")))?;
    let wrist = centre(wrist_loop, body.sections[wrist_i].plane);
    let axis: Vec<DVec3> = std::iter::once(seed_shoulder)
        .chain(
            rows.iter()
                .rev()
                .filter(|(i, _)| {
                    *i > wrist_i && body.sections[*i].loops.iter().filter(|r| r.closed).count() >= 3
                })
                .map(|(i, r)| centre(r, body.sections[*i].plane)),
        )
        .chain(std::iter::once(wrist))
        .collect();
    let fractions = chain_fractions(
        source,
        &[
            format!("{side}Arm"),
            format!("{side}ForeArm"),
            format!("{side}Hand"),
        ],
    )?;
    let prior = at_fraction(&axis, fractions[1])?;
    let estimate = elbow::place(body, &rows, prior.y, seed_shoulder.y - wrist.y)?;
    let elbow = estimate.pivot;
    let (axis_prior, axis_report) = upper_arm_axis(body, &rows, elbow, seed_shoulder.y)?;
    let (shoulder, arm_prior_report) = project_arm_prior(source, placed, side, axis_prior)?;
    let (clavicle, girdle_report) =
        proportional_clavicle(source, side, point(placed, "Spine2")?, shoulder)?;
    placed.insert(format!("{side}Arm"), shoulder);
    placed.insert(format!("{side}Shoulder"), clavicle);
    placed.insert(format!("{side}ForeArm"), elbow);
    placed.insert(format!("{side}Hand"), wrist);
    let mut previous = vec![Track {
        section: wrist_loop,
        path: vec![wrist],
        split: 0,
    }];
    let mut terminals = Vec::new();
    for s in body.sections[..wrist_i]
        .iter()
        .rev()
        .take_while(|s| s.plane > body.height * 0.4)
    {
        let mut counts = vec![0; previous.len()];
        let children: Vec<(&SectionLoop, usize)> = s
            .loops
            .iter()
            .filter(lateral)
            .filter_map(|r| {
                let p = DVec2::from_array(r.centroid);
                let parent = previous
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        (
                            i,
                            loop_distance(t.section, p)
                                + p.distance(DVec2::from_array(t.section.centroid)) * 0.001,
                        )
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match parent {
                    Some((i, d)) if d < body.height * 0.006 => {
                        counts[i] += 1;
                        Some((r, i))
                    }
                    _ => None,
                }
            })
            .collect();
        previous
            .iter()
            .zip(&counts)
            .filter(|(_, count)| **count == 0)
            .for_each(|(t, _)| terminals.push((t.path.clone(), t.split)));
        previous = children
            .into_iter()
            .map(|(r, parent)| {
                let t = &previous[parent];
                let split = match counts[parent] > 1 {
                    true => t.path.len(),
                    false => t.split,
                };
                let mut path = t.path.clone();
                path.push(centre(r, s.plane));
                Track {
                    section: r,
                    path,
                    split,
                }
            })
            .collect();
    }
    terminals.extend(previous.into_iter().map(|t| (t.path, t.split)));
    let mut paths: Vec<Vec<DVec3>> = terminals
        .into_iter()
        .filter(|(p, _)| {
            p.last()
                .is_some_and(|tip| wrist.y - tip.y > body.height * 0.04)
        })
        .map(|(path, split)| {
            std::iter::once(wrist)
                .chain(path.into_iter().skip(split))
                .collect()
        })
        .collect();
    if paths.len() != 5 {
        let tips: Vec<[f64; 3]> = paths
            .iter()
            .filter_map(|p| p.last().map(|v| v.to_array()))
            .collect();
        let sections: Vec<Value> = body.sections.iter().filter(|s| s.plane > wrist.y-body.height*0.12 && s.plane < wrist.y)
            .step_by(10).map(|s|json!({"y":s.plane,"loops":s.loops.iter().filter(lateral).count(),"open":s.open_chains})).collect();
        return Err(refused(
            "humanoid_digits",
            format!(
                "{side}: expected five distal branches, found {}; wrist={:?}, palm_y={}, tips={tips:?}, sections={sections:?}",
                paths.len(),
                wrist.to_array(),
                body.sections[palm_i].plane
            ),
        ));
    }
    paths.sort_by(|a, b| (sign * a[a.len() - 1].x).total_cmp(&(sign * b[b.len() - 1].x)));
    let thumb = paths.remove(0);
    paths.sort_by(|a, b| b[b.len() - 1].z.total_cmp(&a[a.len() - 1].z));
    let paths: Vec<Vec<DVec3>> = std::iter::once(thumb).chain(paths).collect();
    let mut tips = Vec::new();
    for (digit, path) in ["Thumb", "Index", "Middle", "Ring", "Pinky"]
        .iter()
        .zip(&paths)
    {
        let names: Vec<String> = std::iter::once(format!("{side}Hand"))
            .chain((1..=4).map(|i| format!("{side}Hand{digit}{i}")))
            .collect();
        let fractions = chain_fractions(source, &names)?;
        for (name, fraction) in names.iter().zip(fractions).skip(1) {
            placed.insert(name.clone(), at_fraction(path, fraction)?);
        }
        tips.push(json!({"digit":digit,"tip_m":path[path.len()-1].to_array()}));
    }
    Ok(
        json!({"side":side,"shoulder_m":shoulder.to_array(),"clavicle_m":clavicle.to_array(),
        "girdle":girdle_report,"arm_prior_projection":arm_prior_report,"upper_arm_axis":axis_report,
        "wrist_m":wrist.to_array(),"wrist_section_area_m2":wrist_loop.area.abs(),
        "elbow_prior_m":prior.to_array(),"elbow_m":elbow.to_array(),
        "elbow_surface_fit":estimate.report,"digits":tips}),
    )
}

fn foot_section(body: &Surface, z: f64, sign: f64) -> Result<DVec3, CommandError> {
    let turned: Vec<[f64; 3]> = body.vertices.iter().map(|p| [p[0], p[2], -p[1]]).collect();
    let cut = horizontal_sections(&turned, &body.triangles, &[z]).map_err(actor_error)?;
    let r = cut[0]
        .loops
        .iter()
        .filter(|r| {
            r.closed
                && sign * r.centroid[0] > 0.0
                && -r.centroid[1] < body.height * 0.10
                && -r.centroid[1] >= 0.0
        })
        .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| {
            refused(
                "humanoid_foot",
                format!("no foot section at anterior {z:.6} m"),
            )
        })?;
    Ok(DVec3::new(r.centroid[0], -r.centroid[1], z))
}

fn leg(
    body: &Surface,
    reference: &Surface,
    source: &BTreeMap<String, DVec3>,
    side: &str,
    sign: f64,
    placed: &mut BTreeMap<String, DVec3>,
) -> Result<Value, CommandError> {
    let scale = body.height / reference.height;
    let prior = point(source, &format!("{side}Leg"))?.y * scale;
    let (knee_y, knee_loop) = body
        .sections
        .iter()
        .filter(|s| (s.plane - prior).abs() < body.height * knee::SEARCH_HEIGHT_FRACTION)
        .flat_map(|s| s.loops.iter().map(move |r| (s.plane, r)))
        .filter(|(_, r)| {
            r.closed && sign * r.centroid[0] > 0.0 && sign * r.centroid[0] < body.width * 0.22
        })
        .min_by(|(_, a), (_, b)| a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| refused("humanoid_knee", format!("{side}: knee section absent")))?;
    let estimate = knee::place(body, centre(knee_loop, knee_y), prior)?;
    let knee = estimate.pivot;
    placed.insert(format!("{side}Leg"), knee);
    let ankle_y = point(source, &format!("{side}Foot"))?.y * scale;
    let ankle_loop = body
        .cut(ankle_y)?
        .loops
        .into_iter()
        .filter(|r| r.closed && sign * r.centroid[0] > 0.0)
        .max_by(|a, b| a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| refused("humanoid_ankle", format!("{side}: ankle section absent")))?;
    let ankle_seed = centre(&ankle_loop, ankle_y);
    let extent = |s: &Surface| {
        s.triangles
            .iter()
            .flatten()
            .map(|i| s.vertices[*i as usize])
            .filter(|p| p[1] < s.height * 0.10 && sign * p[0] > 0.0)
            .map(|p| p[2])
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            })
    };
    let (min, max) = extent(body);
    let (rmin, rmax) = extent(reference);
    if !min.is_finite() || !rmin.is_finite() || max <= min || rmax <= rmin {
        return Err(refused("humanoid_foot", "foot extent unresolved"));
    }
    let fraction = (point(source, &format!("{side}ToeBase"))?.z - rmin) / (rmax - rmin);
    let toe = foot_section(body, min + (max - min) * fraction, sign)?;
    let ankle_estimate = ankle::place(body, ankle_seed, toe)?;
    let ankle = ankle_estimate.pivot;
    let tip_estimate = toes::place(body, ankle_seed, toe)?;
    let tip = tip_estimate.pivot;
    placed.insert(format!("{side}Foot"), ankle);
    placed.insert(format!("{side}ToeBase"), toe);
    placed.insert(format!("{side}Toe_End"), tip);
    Ok(
        json!({"side":side,"knee_prior_m":prior,"knee_m":knee.to_array(),"knee_surface_fit":estimate.report,
        "ankle_m":ankle.to_array(),"ankle_surface_fit":ankle_estimate.report,"toe_m":toe.to_array(),
        "toe_end_fit":tip_estimate.report}),
    )
}

fn template_surface(actor: &Actor) -> Result<Surface, CommandError> {
    let names = actor.mesh_parts();
    let sample = actor.sample(&names, Pose::Rest).map_err(actor_error)?;
    let part = sample
        .parts
        .iter()
        .max_by_key(|p| p.triangles.len())
        .ok_or_else(|| refused("humanoid_template", "template has no surface"))?;
    let triangles: Vec<[u32; 3]> = sample.triangles[part.triangles.clone()]
        .iter()
        .map(|t| t.map(|i| (i - part.vertices.start) as u32))
        .collect();
    Surface::new(&sample.vertices[part.vertices.clone()], &triangles)
}

fn palm_frame(nodes: &BTreeMap<String, DVec3>, side: &str) -> Result<DMat3, CommandError> {
    let along =
        point(nodes, &format!("{side}HandMiddle1"))? - point(nodes, &format!("{side}Hand"))?;
    let across =
        point(nodes, &format!("{side}HandIndex1"))? - point(nodes, &format!("{side}HandPinky1"))?;
    if along.length() < 1e-8 {
        return Err(refused("humanoid_palm", "palm axis collapsed"));
    }
    let along = along.normalize();
    let across = across - along * across.dot(along);
    if across.length() < 1e-8 {
        return Err(refused("humanoid_palm", "palm width does not resolve roll"));
    }
    let across = across.normalize();
    Ok(DMat3::from_cols(across, along, across.cross(along)))
}

/// Place a supported upright A-rest humanoid without the source actor's rig or weights.
pub(crate) fn place(
    vertices: &[[f64; 3]],
    triangles: &[[u32; 3]],
    template: &Actor,
    rig: &Rig,
    palette: &[String],
) -> Result<(Rig, Value), CommandError> {
    let body = Surface::new(vertices, triangles)?;
    let reference = template_surface(template)?;
    let authored: Vec<DMat4> = (0..palette.len())
        .map(|i| {
            rig.rest_world(JointId::new(i as u32))
                .map(|m| DMat4::from_cols_array_2d(&m))
                .map_err(|e| refused("humanoid_template", e.to_string()))
        })
        .collect::<Result<_, _>>()?;
    let source: BTreeMap<String, DVec3> = palette
        .iter()
        .zip(&authored)
        .map(|(n, w)| {
            (
                n.trim_start_matches("mixamorig:").to_owned(),
                reference.local(w.w_axis.truncate()),
            )
        })
        .collect();
    let mut placed = BTreeMap::new();
    let torso_report = torso(&body, &reference, &source, &mut placed)?;
    let mut arm_reports = Vec::new();
    let mut leg_reports = Vec::new();
    for (side, sign) in [("Left", -1.0), ("Right", 1.0)] {
        arm_reports.push(arm(&body, &reference, &source, side, sign, &mut placed)?);
        leg_reports.push(leg(&body, &reference, &source, side, sign, &mut placed)?);
    }
    let positions: Vec<DVec3> = palette
        .iter()
        .map(|n| point(&placed, n.trim_start_matches("mixamorig:")))
        .collect::<Result<_, _>>()?;
    let parents: Vec<Option<usize>> = (0..palette.len())
        .map(|i| {
            rig.joint(JointId::new(i as u32))
                .and_then(|j| j.parent)
                .map(|j| j.index())
        })
        .collect();
    let mut worlds = Vec::new();
    let mut rows = Vec::new();
    for (i, name) in palette.iter().enumerate() {
        let short = name.trim_start_matches("mixamorig:");
        let child = match short {
            "Hips" => palette.iter().position(|n| n == "mixamorig:Spine"),
            "Spine" => palette.iter().position(|n| n == "mixamorig:Spine1"),
            "Spine1" => palette.iter().position(|n| n == "mixamorig:Spine2"),
            "Spine2" => palette.iter().position(|n| n == "mixamorig:Neck"),
            "Neck" => palette.iter().position(|n| n == "mixamorig:Head"),
            "Head" => palette.iter().position(|n| n == "mixamorig:HeadTop_End"),
            "LeftHand" => palette
                .iter()
                .position(|n| n == "mixamorig:LeftHandMiddle1"),
            "RightHand" => palette
                .iter()
                .position(|n| n == "mixamorig:RightHandMiddle1"),
            _ => parents.iter().position(|p| *p == Some(i)),
        };
        let delta = match child {
            Some(c) => {
                let from = reference.local(authored[c].w_axis.truncate()) - point(&source, short)?;
                let to = positions[c] - positions[i];
                if from.length() < 1e-8 || to.length() < 1e-8 {
                    return Err(refused(
                        "humanoid_chain",
                        format!("collapsed segment at {name}"),
                    ));
                }
                DQuat::from_rotation_arc(from.normalize(), to.normalize())
            }
            None => DQuat::IDENTITY,
        };
        let correction = match short {
            "LeftHand" => palm_frame(&placed, "Left")? * palm_frame(&source, "Left")?.transpose(),
            "RightHand" => {
                palm_frame(&placed, "Right")? * palm_frame(&source, "Right")?.transpose()
            }
            _ => DMat3::from_quat(delta),
        };
        let rotation = body.basis
            * correction
            * reference.basis.transpose()
            * DMat3::from_quat(authored[i].to_scale_rotation_translation().1);
        let world =
            DMat4::from_rotation_translation(DQuat::from_mat3(&rotation), body.world(positions[i]));
        let (distance, inside, votes) =
            crate::medial::surface_probe(vertices, triangles, world.w_axis.truncate());
        if toe_marker(name) && parents.contains(&Some(i)) {
            return Err(refused(
                "humanoid_toe_marker",
                format!("auxiliary endpoint {name} has a child"),
            ));
        }
        rows.push(json!({"bone":name,"seated_position_mm":(world.w_axis.truncate()*1000.0).to_array(),
            "flesh_distance_mm":distance,"inside":inside,"inside_votes_of_six":votes,
            "containment_exception":(toe_marker(name) && !inside).then_some("zero_weight_auxiliary_toe_endpoint"),
            "landmark_source":"surface_sections_template_prior"}));
        worlds.push(world);
    }
    let joints = palette
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let parent = parents[i].map_or(DMat4::IDENTITY, |p| worlds[p]);
            let (_, rotation, translation) =
                (parent.inverse() * worlds[i]).to_scale_rotation_translation();
            Joint {
                name: Some(name.clone()),
                parent: parents[i].map(|p| JointId::new(p as u32)),
                rest: LocalTransform {
                    translation: translation.as_vec3().to_array(),
                    rotation: rotation.as_quat().to_array(),
                    scale: 1.0,
                },
            }
        })
        .collect();
    let output =
        Rig::new(joints, RigFrame::YUp).map_err(|e| refused("humanoid_rig", e.to_string()))?;
    let unresolved: Vec<f64> = body
        .sections
        .iter()
        .filter(|s| s.unresolved.is_some())
        .map(|s| s.plane)
        .collect();
    let report = json!({"method":"surface-conditioned humanoid placement; body proportions, Mixamo graph and orientation conventions",
        "bones":palette.len(),"per_bone":rows,"torso":torso_report,"arms":arm_reports,"legs":leg_reports,
        "body_height_m":body.height,"template_height_m":reference.height,"sampling_step_m":body.height/STEPS as f64,
        "unresolved_section_heights_m":unresolved,"whole_surface_closure_certified":false,
        "native_placement_inputs":0,"mesh_vertices_moved":0,
        "supported_pose":"upright Y-up A rest, separated legs, five separated hand digits, and distinguishable medial toe branches",
        "uncertainty":"Surface sections and template priors estimate pivots; anatomical joint centres and motion quality require separate validation."});
    Ok((output, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clavicle_similarity_preserves_ratio_and_angle_at_new_endpoints() {
        let source = BTreeMap::from([
            ("Spine2".into(), DVec3::ZERO),
            ("LeftShoulder".into(), DVec3::new(1.0, 1.0, 0.0)),
            ("LeftArm".into(), DVec3::new(3.0, 1.0, 0.0)),
        ]);
        let spine = DVec3::new(4.0, 2.0, 1.0);
        let arm = DVec3::new(4.0, 8.0, 3.0);
        let (joint, _) = proportional_clavicle(&source, "Left", spine, arm).unwrap();
        assert!((joint.distance(spine) / arm.distance(joint) - 0.5_f64.sqrt()).abs() < 1e-12);
        assert!(((joint - spine).angle_between(arm - joint).to_degrees() - 45.0).abs() < 1e-10);
        let scale = arm.distance(spine) / 10.0_f64.sqrt();
        assert!((joint.distance(spine) - 2.0_f64.sqrt() * scale).abs() < 1e-12);
        assert!(proportional_clavicle(&source, "Left", spine, spine).is_err());
    }

    #[test]
    fn an_auxiliary_toe_endpoint_must_have_zero_encoded_weight() {
        let palette = vec!["mixamorig:ToeBase".into(), "mixamorig:LeftToe_End".into()];
        assert!(validate_marker_weights(&palette, &[1.0, 0.0]).is_ok());
        assert!(validate_marker_weights(&palette, &[1.0, 0.001]).is_err());
        assert!(validate_marker_weights(&palette, &[1.0]).is_err());
    }
}
