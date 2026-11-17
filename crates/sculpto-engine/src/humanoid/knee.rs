//! An oblique landmark section refines a girth-derived knee seed. @req DECL0013

use glam::{DMat3, DVec2, DVec3};
use serde_json::json;
use sculpto_mesh::actor::horizontal_sections;

use super::{JointEstimate, Surface, loop_distance};
use super::profile::{Border, Feature, Turn, border, transition};
use crate::CommandError;
use crate::export_glb::{actor_error, refused};

pub(super) const SEARCH_HEIGHT_FRACTION: f64 = 0.04;

fn landmark_pair(front: &[DVec3], back: &[DVec3], prior: f64, search: f64, window: f64)
    -> Result<(Feature, Feature), CommandError>
{
    let anterior = transition(front, prior, search, window, Turn::Positive)
        .ok_or_else(|| refused("humanoid_knee", "anterior distal landmark is unresolved"))?;
    let posterior = transition(back, prior, search, window, Turn::Negative)
        .ok_or_else(|| refused("humanoid_knee", "posterior proximal hollow is unresolved"))?;
    Ok((anterior, posterior))
}

/// The lateral direction completes the profile chord into a section plane; the closest
/// point keeps the seed's lateral position without prescribing a chord fraction. @req DECL0013
fn project(seed: DVec3, front: DVec3, back: DVec3) -> Result<(DVec3, DMat3, f64), CommandError> {
    let along = DVec3::new(0.0, back.y-front.y, back.z-front.z).try_normalize()
        .ok_or_else(|| refused("humanoid_knee", "landmarks do not define a sagittal chord"))?;
    let normal = along.cross(DVec3::X);
    let plane = front.dot(normal);
    let pivot = seed - normal * (seed.dot(normal) - plane);
    Ok((pivot, DMat3::from_cols(DVec3::X, normal, along), plane))
}

pub(super) fn place(body: &Surface, seed: DVec3, prior: f64) -> Result<JointEstimate, CommandError> {
    let search = body.height * SEARCH_HEIGHT_FRACTION;
    let sign = seed.x.signum();
    let rows: Vec<_> = body.sections.iter().filter(|s| (s.plane-prior).abs() < search*2.5)
        .filter_map(|s| s.loops.iter().filter(|r| r.closed && sign*r.centroid[0]>0.0
            && sign*r.centroid[0]<body.width*0.22)
            .max_by(|a,b|a.area.abs().total_cmp(&b.area.abs())).map(|r|(s.plane,r))).collect();
    let (_, near) = rows.iter().min_by(|(a,_),(b,_)|(a-prior).abs().total_cmp(&(b-prior).abs()))
        .ok_or_else(|| refused("humanoid_knee", "leg sections are absent"))?;
    let window = (near.area.abs()/std::f64::consts::PI).sqrt()*0.7;
    let profile = |side| rows.iter().filter_map(|(y,r)|border(r,side,*y)).collect::<Vec<_>>();
    let front = profile(Border::Anterior);
    let back = profile(Border::Posterior);
    let (anterior, posterior) = landmark_pair(&front,&back,prior,search,window)?;
    let (pivot,basis,plane) = project(seed,anterior.position,posterior.position)?;
    let displaced = [0.85,1.15].map(|factor| {
        let (a,b)=landmark_pair(&front,&back,prior,search,window*factor)?;
        project(seed,a.position,b.position).map(|(p,_,_)|p)
    });
    let sensitivity = displaced.into_iter().collect::<Result<Vec<_>,_>>()?.iter()
        .map(|p|pivot.distance(*p)).fold(0.0,f64::max);
    let turned: Vec<[f64;3]> = body.vertices.iter()
        .map(|p|(basis.transpose()*DVec3::from_array(*p)).to_array()).collect();
    let section = horizontal_sections(&turned,&body.triangles,&[plane]).map_err(actor_error)?;
    let local = basis.transpose()*pivot;
    let point = DVec2::new(local.x,local.z);
    let ring = section[0].loops.iter().filter(|r|r.closed && r.area.abs()>body.height.powi(2)*0.0002)
        .filter(|r|loop_distance(r,point)<1e-8)
        .max_by(|a,b|a.area.abs().total_cmp(&b.area.abs()))
        .ok_or_else(|| refused("humanoid_knee", "projected pivot is outside a resolved knee section"))?;
    let centroid = basis*DVec3::new(ring.centroid[0],plane,ring.centroid[1]);
    let chord = posterior.position-anterior.position;
    let fraction = (pivot-anterior.position).dot(chord)/chord.length_squared();
    Ok(JointEstimate {pivot,report:json!({
        "method":"shortest projection of the girth-derived seed onto the paired-landmark section",
        "anterior_distal_landmark_m":anterior.position.to_array(),
        "posterior_proximal_landmark_m":posterior.position.to_array(),
        "anterior_turn_degrees":anterior.turn.to_degrees(),"posterior_turn_degrees":posterior.turn.to_degrees(),
        "section_normal":basis.y_axis.to_array(),"section_plane_m":plane,
        "section_centroid_m":centroid.to_array(),"section_area_m2":ring.area.abs(),
        "seed_m":seed.to_array(),"seed_plane_offset_m":seed.dot(basis.y_axis)-plane,
        "measured_fraction_from_anterior":fraction,"fraction_is_input":false,
        "profile_half_window_m":window,"selection_sensitivity_m":sensitivity,
        "uncertainty":"The landmark plane and nearest projection define a geometric candidate, not a directly observed anatomical hinge. The section centroid is diagnostic only; articulation requires validation."
    })})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_plane_projection_keeps_lateral_position_without_a_fraction_prior() {
        let seed=DVec3::new(0.2,0.3,0.5);
        let (pivot,basis,plane)=project(seed,DVec3::ZERO,DVec3::new(0.1,1.0,1.0)).unwrap();
        assert!(pivot.distance(DVec3::new(0.2,0.4,0.4))<1e-12);
        assert!((pivot.dot(basis.y_axis)-plane).abs()<1e-12);
        assert!((pivot-seed).cross(basis.y_axis).length()<1e-12);
        assert!(project(seed,DVec3::ZERO,DVec3::X).is_err());
    }
}
