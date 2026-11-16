//! `actor shoulder-seat`: read the shoulder girdle's seat off the body surface.
//!
//! The verb is a measurement, not a fit. It answers one question — is the shoulder
//! joint inside the deltoid mass and is the clavicle root inside the upper chest —
//! with millimetres, and it refuses to move a pivot whose correct interior depth
//! the mesh cannot name. `0148` suspected this seat through a 2.598 × rest-distance
//! ratio across two different skeletons; a ratio cannot decide a seat, a surface
//! can decide only its sign, and the report says which is which.

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_mesh::actor::{
    Actor, BodyFrame, Decision, PartSelector, SeatJoint, SeatOutcome, Section, shoulder_seat,
};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor shoulder-seat --input actor.glb --body NAME[#i] \
    [--layer NAME[#i] ...] [--report report.json]";

struct Arguments<'arg> {
    input: &'arg str,
    body: &'arg str,
    layers: Vec<&'arg str>,
    report: Option<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor shoulder-seat: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor shoulder-seat: {error}");
            ExitCode::FAILURE
        }
    }
}

fn arguments(args: &[String]) -> Result<Arguments<'_>, String> {
    let mut flags: HashMap<&str, &str> = HashMap::new();
    let mut layers = Vec::new();
    let mut iter = args.iter();
    while let Some(key) = iter.next().map(String::as_str) {
        match key {
            "--layer" => layers.push(iter.next().ok_or("--layer needs a value")?.as_str()),
            "--input" | "--body" | "--report" => {
                let value = iter.next().ok_or_else(|| format!("{key} needs a value"))?;
                if flags.insert(key, value.as_str()).is_some() {
                    return Err(format!("duplicate {key}"));
                }
            }
            _ => return Err(format!("unexpected argument {key:?}")),
        }
    }
    let required = |key| {
        flags
            .get(key)
            .copied()
            .ok_or_else(|| format!("{key} is required"))
    };
    let input = required("--input")?;
    let body = required("--body")?;
    let report = flags.get("--report").copied();
    if report == Some(input) {
        return Err("input and report paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        body,
        layers,
        report,
    })
}

fn execute(arguments: &Arguments<'_>) -> Result<(), String> {
    let bytes =
        std::fs::read(arguments.input).map_err(|error| format!("{}: {error}", arguments.input))?;
    let actor = Actor::from_glb(&bytes).map_err(|error| error.to_string())?;
    let body = PartSelector::parse(arguments.body).map_err(|error| error.to_string())?;
    let layers = arguments
        .layers
        .iter()
        .map(|layer| PartSelector::parse(layer))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let outcome = shoulder_seat(&actor, &body, &layers).map_err(|error| error.to_string())?;

    for joint in &outcome.joints {
        println!(
            "actor shoulder-seat: {}{} {:8} {:+.2} mm signed ({:8}), nearest {:6.2} mm, \
             up wall {}, anterior wall {}",
            joint.side,
            if joint.owned { "" } else { " (context)" },
            joint.level,
            joint.signed * 1000.0,
            joint.decision.as_str(),
            joint.nearest * 1000.0,
            wall_text(joint, "up"),
            wall_text(joint, "anterior"),
        );
    }
    if let Some(base) = &outcome.neck_base {
        println!(
            "actor shoulder-seat: trapezius line {:.1} mm ({:.1} mm below the neck joint), \
             half-width {:.2} mm = {:.2} x the neck's",
            base.level * 1000.0,
            base.below_neck_joint * 1000.0,
            base.half_width * 1000.0,
            base.ratio,
        );
    }
    println!(
        "actor shoulder-seat: {} joint(s) read, {} outside the flesh, 0 moved — the surface \
         fixes the sign of a seat, not its depth",
        outcome.joints.len(),
        outcome.outside(),
    );

    let report = report(arguments, &outcome, &body, &layers);
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    Ok(())
}

fn length(vector: [f64; 3]) -> f64 {
    dot(vector, vector).sqrt()
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn wall_text(joint: &SeatJoint, direction: &str) -> String {
    joint
        .walls
        .iter()
        .find(|wall| wall.direction == direction)
        .and_then(|wall| wall.distance)
        .map_or_else(
            || "open".to_owned(),
            |value| format!("{:.2} mm", value * 1000.0),
        )
}

fn frame_json(frame: &BodyFrame) -> Value {
    json!({
        "up": frame.up,
        "lateral": frame.lateral,
        "anterior": frame.anterior,
        "anterior_source": frame.anterior_source,
    })
}

fn section_json(section: &Section, frame: &BodyFrame) -> Value {
    json!({
        "part": section.part,
        "level": section.level,
        "plane_m": section.plane,
        "segments": section.segments,
        "pinched": section.pinched,
        "loops": section.loops.len(),
        "open_chains": section.open_chains,
        "selected_loops": section.selected.len(),
        "area_m2": section.area(),
        "centroid_m": section.centroid().map(|centre| [0, 1, 2].map(|axis| {
            frame.lateral[axis] * centre[0]
                + frame.anterior[axis] * centre[1]
                + frame.up[axis] * section.plane
        })),
        "bounds": section.bounds().map(|(lateral_min, lateral_max, anterior_min, anterior_max, asymmetry)| json!({
            "lateral_min_m": lateral_min,
            "lateral_max_m": lateral_max,
            "anterior_min_m": anterior_min,
            "anterior_max_m": anterior_max,
            "asymmetry_m": asymmetry,
        })),
        "unresolved": section.unresolved,
    })
}

fn joint_json(joint: &SeatJoint, frame: &BodyFrame) -> Value {
    let walls: Value = joint
        .walls
        .iter()
        .map(|wall| {
            (
                wall.direction.to_owned(),
                wall.distance.map_or(Value::Null, |value| json!(value)),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into();
    json!({
        "side": joint.side.to_string(),
        "level": joint.level,
        "name": joint.name,
        "node": joint.node,
        "owned": joint.owned,
        "record": {
            "rig_pivot_m": joint.pivot,
            "height_m": joint.height,
            "parent": joint.parent,
            "parent_link_m": joint.parent_link.map(length),
            "parent_link_up_m": joint.parent_link.map(|link| dot(link, frame.up)),
            "nearest_surface_m": joint.nearest,
            "signed_m": joint.signed,
            "nearest_point_m": joint.closest,
            "inside": joint.signed <= 0.0,
            "walls_m": walls,
            "below_neck_base_m": joint.below_neck_base,
        },
        "section": section_json(&joint.section, frame),
        "decision": joint.decision.as_str(),
        "evidence": joint.evidence,
    })
}

fn report(
    arguments: &Arguments<'_>,
    outcome: &SeatOutcome,
    body: &PartSelector,
    layers: &[PartSelector],
) -> Value {
    let owned = outcome.joints.iter().filter(|joint| joint.owned).count();
    let agreed = outcome
        .joints
        .iter()
        .filter(|joint| joint.owned && joint.decision == Decision::Agreed)
        .count();
    let outside = outcome
        .joints
        .iter()
        .filter(|joint| joint.owned && joint.decision == Decision::Unresolved)
        .count();
    json!({
        "verb": "actor shoulder-seat",
        "input": arguments.input,
        "body": body.label(),
        "layers": layers.iter().map(PartSelector::label).collect::<Vec<_>>(),
        "coordinates": "actor units, the frame the input GLB carries",
        "statement": "measures the shoulder girdle's seat against the body surface; a pivot the \
                      flesh proves outside is reported with its excursion and never moved to an \
                      invented depth",
        "frame": frame_json(&outcome.frame),
        "vertex_count": outcome.vertex_count,
        "neck_base": outcome.neck_base.map(|base| json!({
            "level_m": base.level,
            "half_width_m": base.half_width,
            "neck_half_width_m": base.neck_half_width,
            "ratio": base.ratio,
            "below_neck_joint_m": base.below_neck_joint,
        })),
        "joints": outcome
            .joints
            .iter()
            .map(|joint| joint_json(joint, &outcome.frame))
            .collect::<Vec<_>>(),
        "layers": outcome
            .layers
            .iter()
            .map(|section| section_json(section, &outcome.frame))
            .collect::<Vec<_>>(),
        "refusal": {
            "axis": "seat depth",
            "decision": "unresolved",
            "why": "a skinned surface carries no clavicle, acromion or glenoid marker. The mesh \
                    decides inside from outside exactly, and decides nothing about how deep a \
                    pivot belongs; placing an outside pivot on the nearest surface point would \
                    be a boundary, not a joint centre, and any deeper target would be invented.",
            "what_would_decide_it": "a deformation probe that poses the shoulder and compares \
                                     edge stretch at the authored seat and at a candidate interior \
                                     seat, or an anatomical reference measured on the same \
                                     skeleton.",
            "minimum_correction_m": outcome
                .joints
                .iter()
                .filter(|joint| joint.signed > 0.0)
                .map(|joint| json!({
                    "side": joint.side.to_string(),
                    "level": joint.level,
                    "excursion_m": joint.signed,
                }))
                .collect::<Vec<_>>(),
        },
        "summary": {
            "joints": outcome.joints.len(),
            "owned": owned,
            "agreed": agreed,
            "outside": outside,
            "moved_joints": 0,
        },
    })
}
