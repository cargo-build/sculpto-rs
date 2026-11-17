//! Zero-weight toe endpoints between the two medial terminal branches. @req DECL0013

use glam::{DMat3, DVec2, DVec3};
use serde_json::json;
use sculpto_mesh::actor::{Section, SectionLoop, horizontal_sections};

use super::{JointEstimate, STEPS, Surface, foot_axes, loop_distance};
use crate::CommandError;
use crate::export_glb::{actor_error, refused};

struct Split<'a> {
    index: usize,
    medial: &'a SectionLoop,
    next: &'a SectionLoop,
}

fn children<'a>(parent: &SectionLoop, section: &'a Section, reach: f64) -> Vec<&'a SectionLoop> {
    section.loops.iter().filter(|r|
        loop_distance(parent,DVec2::from_array(r.centroid))<reach).collect()
}

/// Following the medial trunk through its last lateral split identifies the first two
/// toes even when the second is longer. Vertical tip islands are not another toe. @req DECL0013
fn medial_split(sections:&[Section],step:f64)->Option<Split<'_>>{
    let mut parent=sections.first()?.loops.iter()
        .max_by(|a,b|a.area.abs().total_cmp(&b.area.abs()))?;
    let mut split=None;
    for (index,section) in sections.iter().enumerate().skip(1){
        let mut next=children(parent,section,step*3.0);
        next.sort_by(|a,b|a.centroid[0].total_cmp(&b.centroid[0]));
        if next.is_empty(){break;}
        let lateral_split=next.get(1).is_some_and(|b|{
            let a=next[0];
            let width=(b.centroid[0]-a.centroid[0]).abs();
            width>(b.centroid[1]-a.centroid[1]).abs() && width>step*2.0
        });
        parent=match lateral_split{
            true=>{
                split=Some(Split{index,medial:next[0],next:next[1]});
                next[0]
            },
            false=>next.into_iter().max_by(|a,b|a.area.abs().total_cmp(&b.area.abs()))?,
        };
    }
    split
}

fn terminal(sections:&[Section],index:usize,initial:&SectionLoop,step:f64)->DVec3{
    let mut parent=initial;
    let mut point=DVec3::new(parent.centroid[0],sections[index].plane,parent.centroid[1]);
    for section in sections.iter().skip(index+1){
        let next=children(parent,section,step*3.0);
        let Some(child)=next.into_iter().max_by(|a,b|a.area.abs().total_cmp(&b.area.abs())) else {break};
        parent=child;
        point=DVec3::new(parent.centroid[0],section.plane,parent.centroid[1]);
    }
    point
}

pub(super) fn place(body:&Surface,seed:DVec3,forefoot:DVec3)->Result<JointEstimate,CommandError>{
    let (forward,outward)=foot_axes(seed,forefoot)?;
    let basis=DMat3::from_cols(outward,forward,DVec3::Y);
    let turned:Vec<[f64;3]>=body.vertices.iter().map(|p|
        (basis.transpose()*(DVec3::from_array(*p)-seed)).to_array()).collect();
    let sign=seed.x.signum();
    let end=body.triangles.iter().flatten().filter_map(|i|{
        let p=body.vertices[*i as usize];
        (sign*p[0]>0.0 && p[1]<body.height*0.10).then_some(turned[*i as usize][1])
    }).max_by(f64::total_cmp).ok_or_else(||refused("humanoid_toes","foot extent is unresolved"))?;
    let start=(forefoot-seed).dot(forward);
    let step=body.height/STEPS as f64;
    if end-start<step*4.0{
        return Err(refused("humanoid_toes","forefoot leaves no resolvable terminal span"));
    }
    let planes:Vec<f64>=(0..=((end-start)/step).floor() as usize)
        .map(|i|start+step*i as f64).collect();
    let mut sections=horizontal_sections(&turned,&body.triangles,&planes).map_err(actor_error)?;
    sections.iter_mut().for_each(|s|s.loops.retain(|r|r.closed
        && r.area.abs()>body.height.powi(2)*0.0000003
        && r.centroid[0].abs()<body.height*0.06
        && r.centroid[1]+seed.y>0.0 && r.centroid[1]+seed.y<body.height*0.10));
    let split=medial_split(&sections,step)
        .ok_or_else(||refused("humanoid_toes","two medial toe branches are unresolved"))?;
    let medial=seed+basis*terminal(&sections,split.index,split.medial,step);
    let next=seed+basis*terminal(&sections,split.index,split.next,step);
    let pivot=(medial+next)*0.5;
    Ok(JointEstimate{pivot,report:json!({
        "method":"midpoint of the terminal sections of the two medial toe branches",
        "medial_tip_m":medial.to_array(),"next_tip_m":next.to_array(),
        "sweep_step_m":step,"last_medial_split_m":sections[split.index].plane,
        "toe_base_unchanged":true,"requires_zero_weight":true,
        "containment_exception":"operator-approved auxiliary Toe_End may lie in the inter-toe gap",
        "uncertainty":"Terminal contour centres estimate the gap direction; this zero-weight endpoint is not an anatomical articulation pivot."
    })})
}

#[cfg(test)]
mod tests{
    use super::*;

    fn ring(low:f64,high:f64)->SectionLoop{
        SectionLoop{area:(high-low)*2.0,centroid:[(low+high)*0.5,0.0],
            ring:vec![[low,-1.0],[high,-1.0],[high,1.0],[low,1.0]],segments:4,closed:true}
    }

    fn section(plane:f64,loops:Vec<SectionLoop>)->Section{
        Section{part:"test foot".into(),level:"forward".into(),plane,
            segments:loops.len()*4,pinched:0,loops,open_chains:0,selected:vec![],unresolved:None}
    }

    #[test]
    fn each_medial_toe_reaches_its_own_terminal_section(){
        let sections=vec![section(0.0,vec![ring(-3.0,3.0)]),
            section(0.1,vec![ring(-3.0,-1.0),ring(1.0,3.0)]),
            section(0.2,vec![ring(-3.0,-1.0),ring(1.0,3.0)]),
            section(0.3,vec![ring(1.0,3.0)])];
        let split=medial_split(&sections,0.1).unwrap();
        assert_eq!(split.index,1);
        assert_eq!(terminal(&sections,split.index,split.medial,0.1),DVec3::new(-2.0,0.2,0.0));
        assert_eq!(terminal(&sections,split.index,split.next,0.1),DVec3::new(2.0,0.3,0.0));
        let unsplit=vec![section(0.0,vec![ring(-3.0,3.0)]),section(0.1,vec![ring(-2.0,2.0)])];
        assert!(medial_split(&unsplit,0.1).is_none());
    }
}
