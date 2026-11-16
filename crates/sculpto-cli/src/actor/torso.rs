//! `actor torso-fit`: place the spine chain on the flesh the sections measure.
//!
//! The verb is a rig step, not a render: it cuts the body at each torso joint's
//! own height, reads the area-weighted centre of the flesh standing there, fits
//! the one axis the mesh decides (lateral, from the body's mirror symmetry) and
//! reports the other (anterior) as unresolved with the measurement that makes it
//! undecidable. Rest geometry is untouched by construction, which the command
//! proves by sampling both files and comparing every vertex.

use std::collections::HashMap;
use std::process::ExitCode;

use sculpto_mesh::actor::{
    Actor, AnatomyLevel, BodyFrame, Decision, LimbProbe, PartSelector, Pose, Sample, Section,
    TorsoJoint, TorsoOutcome, torso_fit, write_frames,
};
use serde_json::{Value, json};

const USAGE: &str = "usage: sculpto actor torso-fit --input actor.glb --body NAME[#i] \
    [--layer NAME[#i] ...] [--out fitted.glb] [--report report.json]";

struct Arguments<'arg> {
    input: &'arg str,
    body: &'arg str,
    layers: Vec<&'arg str>,
    output: Option<&'arg str>,
    report: Option<&'arg str>,
}

pub(super) fn run(args: &[String]) -> ExitCode {
    let arguments = match arguments(args) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("actor torso-fit: {error}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match execute(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("actor torso-fit: {error}");
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
            "--input" | "--body" | "--out" | "--report" => {
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
    let output = flags.get("--out").copied();
    let report = flags.get("--report").copied();
    if output == Some(input)
        || report == Some(input)
        || report.is_some_and(|path| Some(path) == output)
    {
        return Err("input, output and report paths must differ".to_owned());
    }
    Ok(Arguments {
        input,
        body,
        layers,
        output,
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
    let outcome = torso_fit(&actor, &body, &layers).map_err(|error| error.to_string())?;

    let before = actor
        .sample(&[body.node_name.as_str()], Pose::Rest)
        .map_err(|error| error.to_string())?;
    let mut written_bytes = None;
    let mut rest_shift = 0.0;
    if let Some(path) = arguments.output {
        let written = write_frames(&bytes, &outcome.frames).map_err(|error| error.to_string())?;
        std::fs::write(path, &written).map_err(|error| format!("{path}: {error}"))?;
        let fitted = Actor::from_glb(&written).map_err(|error| error.to_string())?;
        let after = fitted
            .sample(&[body.node_name.as_str()], Pose::Rest)
            .map_err(|error| error.to_string())?;
        rest_shift = rest_shift_of(&before, &after);
        written_bytes = Some(written.len());
    }
    let extent = extent_of(&before);
    let limit = 1e-5 * extent;

    let report = report(
        arguments,
        &outcome,
        &body,
        &layers,
        written_bytes,
        rest_shift,
        limit,
    );
    let text = serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?;
    if let Some(path) = arguments.report {
        std::fs::write(path, format!("{text}\n")).map_err(|error| format!("{path}: {error}"))?;
    }
    for joint in &outcome.joints {
        println!(
            "actor torso-fit: {:7} lateral {:+.2} mm ({}) anterior {:+.2} mm ({}), \
             posterior standoff {}",
            joint.level,
            joint.lateral_before * 1000.0,
            joint.lateral_decision.as_str(),
            joint.anterior_before * 1000.0,
            joint.anterior_decision.as_str(),
            joint
                .posterior_standoff
                .map(|value| format!("{:.1} mm", value * 1000.0))
                .unwrap_or_else(|| "n/a".to_owned()),
        );
    }
    println!(
        "actor torso-fit: {} joint(s) moved, max lateral shift {:.2} mm, \
         instrument bias {} / {} mm (lateral / anterior), rest reconstruction {:.3e} \
         (limit {:.3e})",
        outcome.moved_joints,
        outcome.max_pivot_shift * 1000.0,
        outcome.lateral_bias * 1000.0,
        outcome.anterior_bias * 1000.0,
        rest_shift,
        limit
    );
    Ok(())
}

/// Largest per-vertex distance between two same-length rest samples.
fn rest_shift_of(before: &Sample, after: &Sample) -> f64 {
    before
        .vertices
        .iter()
        .zip(&after.vertices)
        .map(|(left, right)| {
            let delta = [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
            (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt()
        })
        .fold(0.0_f64, f64::max)
}

/// Diagonal of the sampled body, the scale a residual is judged against.
fn extent_of(sample: &Sample) -> f64 {
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for point in &sample.vertices {
        for axis in 0..3 {
            low[axis] = low[axis].min(point[axis]);
            high[axis] = high[axis].max(point[axis]);
        }
    }
    let delta = [high[0] - low[0], high[1] - low[1], high[2] - low[2]];
    (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt()
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
        "centroid_m": centroid_json(section, frame),
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

/// The section's centre as a 3D point: its two in-plane coordinates rebuilt on
/// the actor's own axes.
fn centroid_json(section: &Section, frame: &BodyFrame) -> Option<[f64; 3]> {
    section.centroid().map(|centre| {
        [0, 1, 2].map(|axis| {
            frame.lateral[axis] * centre[0]
                + frame.anterior[axis] * centre[1]
                + frame.up[axis] * section.plane
        })
    })
}

fn joint_json(joint: &TorsoJoint) -> Value {
    json!({
        "level": joint.level,
        "name": joint.name,
        "node": joint.node,
        "owned": joint.owned,
        "record": {
            "anatomical_estimate": joint.anatomical_estimate,
            "rig_pivot_before": joint.pivot_before,
            "rig_pivot_after": joint.pivot_after,
            "uncertainty_m": joint.uncertainty,
        },
        "lateral": {
            "before_m": joint.lateral_before,
            "after_m": joint.lateral_after,
            "decision": joint.lateral_decision.as_str(),
        },
        "anterior": {
            "before_m": joint.anterior_before,
            "decision": joint.anterior_decision.as_str(),
            "fraction_from_anterior": joint.fraction_from_anterior,
            "posterior_standoff_m": joint.posterior_standoff,
            "nearest_boundary_m": joint.nearest_boundary,
        },
        "evidence": joint.evidence,
    })
}

fn probe_json(probe: &LimbProbe) -> Value {
    json!({
        "label": probe.label,
        "plane_m": probe.plane,
        "bone_m": probe.bone,
        "centroid_m": probe.centroid,
        "lateral_m": probe.lateral,
        "anterior_m": probe.anterior,
        "area_m2": probe.area,
        "asymmetry_m": probe.asymmetry,
    })
}

fn anatomy_json(level: &AnatomyLevel) -> Value {
    json!({
        "label": level.label,
        "plane_m": level.plane,
        "area_m2": level.area,
        "centroid_m": level.centroid,
    })
}

fn report(
    arguments: &Arguments<'_>,
    outcome: &TorsoOutcome,
    body: &PartSelector,
    layers: &[PartSelector],
    written_bytes: Option<usize>,
    rest_shift: f64,
    rest_limit: f64,
) -> Value {
    let agreed = outcome
        .joints
        .iter()
        .filter(|joint| joint.lateral_decision == Decision::Agreed)
        .count();
    let unresolved = outcome
        .joints
        .iter()
        .filter(|joint| joint.lateral_decision == Decision::Unresolved)
        .count();
    json!({
        "verb": "actor torso-fit",
        "input": arguments.input,
        "output": arguments.output,
        "bytes": written_bytes,
        "body": body.label(),
        "layers": layers.iter().map(PartSelector::label).collect::<Vec<_>>(),
        "coordinates": "actor units, the frame the input GLB carries",
        "frame": frame_json(&outcome.frame),
        "vertex_count": outcome.vertex_count,
        "instrument": {
            "note": "the same area centroid measured where the flesh is near a cylinder; the \
                     distance from the trusted bone is the instrument's own bias",
            "probes": outcome.instrument.iter().map(probe_json).collect::<Vec<_>>(),
            "lateral_bias_m": outcome.lateral_bias,
            "anterior_bias_m": outcome.anterior_bias,
            "skipped": outcome.instrument_skipped,
        },
        "anatomy": outcome.anatomy.iter().map(anatomy_json).collect::<Vec<_>>(),
        "refusal": {
            "axis": "anterior",
            "decision": "unresolved",
            "why": "the section's centre of area is where the soft tissue balances, not where \
                    the vertebral column stands: breasts, belly and glutes move it by tens of \
                    millimetres while no bone follows that mass. The limb probes measure the \
                    same instrument on flesh that is near a cylinder, where the bone's position \
                    is trusted, and the centroid sits the reported anterior_bias away from it. \
                    Moving the chain onto the area centre would place the vertebral pivots in \
                    the viscera, which is an invented offset.",
            "what_would_decide_it": "a landmark the mesh does not carry: an authored vertebral \
                                     reference on the same skeleton, a second body of the same \
                                     asset family measured at the same levels, or a deformation \
                                     test that separates the candidate pivots.",
            "measured_bias_m": outcome.anterior_bias,
            "levels_with_a_section": outcome
                .joints
                .iter()
                .filter(|joint| joint.anterior_before != 0.0)
                .count(),
        },
        "sections": outcome
            .sections
            .iter()
            .map(|section| section_json(section, &outcome.frame))
            .collect::<Vec<_>>(),
        "joints": outcome.joints.iter().map(joint_json).collect::<Vec<_>>(),
        "summary": {
            "joints": outcome.joints.len(),
            "moved_joints": outcome.moved_joints,
            "lateral_agreed": agreed,
            "lateral_unresolved": unresolved,
            "max_pivot_shift_m": outcome.max_pivot_shift,
            "skin_base_spread": outcome.skin_base_spread,
        },
        "gates": {
            "lateral_tolerance_m": sculpto_mesh::actor::LATERAL_TOLERANCE,
            "lateral_pass": outcome
                .joints
                .iter()
                .all(|joint| joint.lateral_after.abs() <= sculpto_mesh::actor::LATERAL_TOLERANCE
                    || joint.lateral_decision == Decision::Unresolved),
            "rest_reconstruction_limit_m": rest_limit,
            "rest_reconstruction_m": rest_shift,
            "rest_reconstruction_pass": rest_shift <= rest_limit,
        },
    })
}
