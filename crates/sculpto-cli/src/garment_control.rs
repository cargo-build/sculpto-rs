//! `sculpto garment control-*`: source-guided control reconstruction. @req DECL0013
//!
//! The CLI owns file IO and recipe validation; every geometric decision comes from
//! [`sculpto_mesh::control`]. Native mesh operators (fill, collapse, decimation)
//! stay in the host adapter, which applies the plan this module prints and reports
//! the result back to `control-check`.

use std::path::{Path, PathBuf};

use sculpto_mesh::control::contact::separate;
use sculpto_mesh::control::measure::{ControlDiagnostics, Deviation, Deviations};
use sculpto_mesh::control::profile::ControlProfile;
use sculpto_mesh::control::{
    ApertureResolution, BoundaryLoop, ControlError, ExteriorOutcome, Qualification, RemovalReason,
    RepairAction, RepairOutcome, exterior_control, repair_control, source_surface,
};
use serde_json::{Value, json};

use crate::{parse_flags, read_obj, required, to_source, write_obj};

/// Reconstruct the outward control surface and print the native-operator plan.
pub fn exterior_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let source_path = PathBuf::from(required(&flags, "source")?);
    let profile_path = PathBuf::from(required(&flags, "profile")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let plan_path = PathBuf::from(required(&flags, "plan")?);
    let profile = profile(&profile_path)?;
    let (source_display, triangles) = read_obj(&source_path)?;
    let source = to_source(&source_display);
    let outcome = exterior_control(&source, &triangles, &profile).map_err(|error| {
        format!(
            "{}: {error}; no reconstruction decision was accepted",
            source_path.display()
        )
    })?;
    write_obj(&out, &outcome.positions, &outcome.triangles)
        .map_err(|error| format!("{}: {error}", out.display()))?;
    let plan = exterior_plan(&source_path, &profile_path, &outcome);
    write_json(&plan_path, &plan)?;
    print_exterior(&out, &outcome, &plan_path);
    Ok(())
}

/// Repair plan for a decimated control: corner flares and incidental loops.
pub fn repair_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let control_path = PathBuf::from(required(&flags, "control")?);
    let profile_path = PathBuf::from(required(&flags, "profile")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let plan_path = PathBuf::from(required(&flags, "plan")?);
    let profile = profile(&profile_path)?;
    let (control_display, triangles) = read_obj(&control_path)?;
    let control = to_source(&control_display);
    let outcome = repair_control(&control, &triangles, &profile).map_err(|error| {
        format!(
            "{}: {error}; the decimated control was not repaired",
            control_path.display()
        )
    })?;
    write_obj(&out, &outcome.positions, &outcome.triangles)
        .map_err(|error| format!("{}: {error}", out.display()))?;
    let plan = repair_plan(&control_path, &profile_path, &outcome);
    write_json(&plan_path, &plan)?;
    print_repair(&out, &outcome, &plan_path);
    Ok(())
}

/// Apply the profile's single bounded local separation, if one is declared.
pub fn separate_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let control_path = PathBuf::from(required(&flags, "control")?);
    let profile_path = PathBuf::from(required(&flags, "profile")?);
    let out = PathBuf::from(required(&flags, "out")?);
    let report_path = PathBuf::from(required(&flags, "report")?);
    let profile = profile(&profile_path)?;
    let (control_display, triangles) = read_obj(&control_path)?;
    let control = to_source(&control_display);
    let mut report = serde_json::Map::new();
    report.insert(
        "policy".to_string(),
        json!({
            "mode": profile.contact.mode.name(),
            "clearance": profile.contact.clearance,
            "max_displacement": profile.contact.max_displacement,
            "max_pairs": profile.contact.max_pairs,
        }),
    );
    report.insert(
        "control".to_string(),
        json!(control_path.display().to_string()),
    );
    match separate(&control, &triangles, &profile.contact) {
        Ok(None) => {
            write_obj(&out, &control, &triangles)
                .map_err(|error| format!("{}: {error}", out.display()))?;
            report.insert("applied".to_string(), Value::Bool(false));
            report.insert("pairs".to_string(), Value::from(0));
            write_json(&report_path, &Value::Object(report))?;
            println!("{}: no contact, nothing to separate", out.display());
            Ok(())
        }
        Ok(Some(outcome)) => {
            write_obj(&out, &outcome.positions, &triangles)
                .map_err(|error| format!("{}: {error}", out.display()))?;
            let separation = &outcome.separation;
            report.insert("applied".to_string(), Value::Bool(true));
            report.insert("pairs".to_string(), Value::from(separation.pairs));
            report.insert(
                "separation".to_string(),
                json!({
                    "witness": [separation.witness.left, separation.witness.right],
                    "moving_face": separation.moving_face,
                    "fixed_face": separation.fixed_face,
                    "shared_anchors": separation.shared_anchors,
                    "axis": separation.axis,
                    "normal": separation.normal,
                    "overlap": separation.overlap,
                    "clearance": separation.clearance,
                    "translation": separation.translation,
                    "max_displacement": separation.max_displacement,
                    "min_incident_area_ratio": separation.min_incident_area_ratio,
                    "moved": separation.moved.iter().map(|movement| json!({
                        "vertex": movement.vertex,
                        "share": movement.share,
                        "from": movement.from,
                        "to": movement.to,
                    })).collect::<Vec<_>>(),
                }),
            );
            write_json(&report_path, &Value::Object(report))?;
            println!(
                "{}: separated witness {:?} along {} by {:.6} (max vertex move {:.6}, {} vertices)",
                out.display(),
                [separation.witness.left, separation.witness.right],
                separation.axis,
                separation.translation,
                separation.max_displacement,
                separation.moved.len()
            );
            println!("report: {}", report_path.display());
            Ok(())
        }
        Err(error) => {
            report.insert("applied".to_string(), Value::Bool(false));
            report.insert("error".to_string(), json!(error.to_string()));
            write_json(&report_path, &Value::Object(report))?;
            Err(format!("{}: {error}", control_path.display()))
        }
    }
}

/// Validate the profile's declared source cuts and print the host plan.
///
/// The host must obtain this plan before it mutates any source derivative: the
/// shared core validates labels, frame, finite plane values and the retained side.
pub fn cuts_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let profile_path = PathBuf::from(required(&flags, "profile")?);
    let report_path = PathBuf::from(required(&flags, "report")?);
    let profile = profile(&profile_path)?;
    let cuts = profile
        .source_cuts
        .iter()
        .map(|cut| {
            json!({
                "label": cut.label,
                "plane_point": cut.point,
                "normal": cut.normal,
                "retain": cut.retain.name(),
            })
        })
        .collect::<Vec<_>>();
    let plan = json!({
        // The emitted planes are already converted; the declared profile frame is kept
        // beside them so a reader never mistakes one for the other.
        "plane_frame": "source-local-z-up",
        "declared_frame": profile.frame.name(),
        "cuts": cuts,
        "note": "plane_point/normal are in plane_frame; declared_frame is the profile frame they were written in. The host applies the plane_frame values to a private derivative and leaves openings unfilled",
    });
    write_json(&report_path, &plan)?;
    println!(
        "{}: {} declared source cut(s) ({})",
        report_path.display(),
        profile.source_cuts.len(),
        profile
            .source_cuts
            .iter()
            .map(|cut| format!("{}:{}", cut.label, cut.retain.name()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// Measure a finished control, resolve its apertures and gate it.
pub fn check_inner(args: &[String]) -> Result<(), String> {
    let flags = parse_flags(args)?;
    let control_path = PathBuf::from(required(&flags, "control")?);
    let source_path = PathBuf::from(required(&flags, "source")?);
    let profile_path = PathBuf::from(required(&flags, "profile")?);
    let report_path = PathBuf::from(required(&flags, "report")?);
    let profile = profile(&profile_path)?;
    let (control_display, control_triangles) = read_obj(&control_path)?;
    let control = to_source(&control_display);
    let (source_display, source_triangles) = read_obj(&source_path)?;
    let source = to_source(&source_display);

    let measured = ControlDiagnostics::measure(&control, &control_triangles);
    let diagnostics = measured.as_ref().ok();
    let qualification = diagnostics.map(|diagnostics| diagnostics.qualify(&profile));
    let exterior = source_surface(&source, &source_triangles, &profile.guide, profile.surface);
    let deviations = exterior.as_ref().ok().map(|(surface, kept)| {
        sculpto_mesh::control::measure::deviations(
            surface,
            kept,
            &control,
            &control_triangles,
            &profile.coverage,
        )
    });
    let coverage = deviations.as_ref().map(|deviations| match deviations {
        Ok(deviations) => coverage_check(deviations, &profile),
        Err(error) => Err(format!("deviation measurement failed: {error}")),
    });
    let verdict = match (qualification.as_ref(), coverage.as_ref()) {
        (Some(Ok(_)), Some(Ok(()))) => Ok(()),
        (Some(Ok(_)), Some(Err(reason))) => Err(reason.clone()),
        (Some(Err(error)), _) => Err(format!("control rejected: {error}")),
        (None, _) => Err("the control surface cannot be measured".to_string()),
        (_, None) => Err("no deviation measurement was produced".to_string()),
    };
    let report = check_report(
        &control_path,
        &source_path,
        &profile_path,
        CheckEvidence {
            diagnostics,
            qualification: qualification.as_ref(),
            deviations: deviations.as_ref(),
            source_error: exterior.as_ref().err(),
            verdict: &verdict,
        },
        &profile,
    );
    write_json(&report_path, &report)?;
    print_check(
        diagnostics,
        qualification.as_ref(),
        deviations
            .as_ref()
            .and_then(|deviations| deviations.as_ref().ok()),
        &report_path,
    );
    let Some(diagnostics) = diagnostics else {
        let Err(error) = &measured else {
            return Err("the control surface was measured but not retained".to_string());
        };
        return Err(format!("the control surface cannot be measured: {error}"));
    };
    let qualification = match qualification {
        Some(Ok(qualification)) => qualification,
        Some(Err(error)) => return Err(format!("control rejected: {error}")),
        None => return Err("the control surface was not qualified".to_string()),
    };
    verdict.as_ref().map_err(|reason| reason.clone())?;
    let roles = qualification
        .apertures
        .iter()
        .map(|matched| matched.role.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "control accepted: {} apertures ({roles}), {} triangles within the declared bounds",
        qualification.apertures.len(),
        diagnostics.triangles
    );
    Ok(())
}

/// The declared coverage conjunction, in one place for the report verdict.
fn coverage_check(deviations: &Deviations, profile: &ControlProfile) -> Result<(), String> {
    for (name, deviation) in [
        ("control to source", deviations.control_to_source),
        (
            "source exterior to control",
            deviations.source_exterior_to_control,
        ),
    ] {
        if deviation.coverage < profile.coverage.minimum {
            return Err(format!(
                "{name} coverage {:.1}% is below the declared {:.1}% within {:.4}",
                f64::from(deviation.coverage) * 100.0,
                f64::from(profile.coverage.minimum) * 100.0,
                profile.coverage.tolerance
            ));
        }
    }
    Ok(())
}

fn profile(path: &Path) -> Result<ControlProfile, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    ControlProfile::from_json(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn loop_json(boundary: &BoundaryLoop) -> Value {
    json!({
        "edges": boundary.edges,
        "closed": boundary.closed,
        "center": boundary.center,
        "perimeter": boundary.perimeter,
        "vertices": boundary.vertices,
    })
}

fn loops_json(loops: &[BoundaryLoop]) -> Value {
    Value::Array(loops.iter().map(loop_json).collect())
}

fn apertures_json(resolution: &ApertureResolution) -> Value {
    json!({
        "matched": resolution
            .matched
            .iter()
            .map(|matched| json!({
                "role": matched.role,
                "loop": matched.loop_index,
                "distance": matched.distance,
            }))
            .collect::<Vec<_>>(),
        "incidental": resolution.incidental,
    })
}

fn incidental_json(loops: &[BoundaryLoop], resolution: &ApertureResolution) -> Value {
    Value::Array(
        resolution
            .incidental
            .iter()
            .map(|index| {
                let boundary = &loops[*index];
                json!({
                    "loop": index,
                    "edges": boundary.edges,
                    "closed": boundary.closed,
                    "vertices": boundary.vertices,
                })
            })
            .collect(),
    )
}

fn exterior_plan(source: &Path, profile: &Path, outcome: &ExteriorOutcome) -> Value {
    json!({
        "stage": "exterior",
        "source": source.display().to_string(),
        "profile": profile.display().to_string(),
        "vertices": outcome.positions.len(),
        "triangles": outcome.triangles.len(),
        "composition": {
            "source_triangles": outcome.composition.source_triangles,
            "source_vertices": outcome.composition.source_vertices,
            "coincident_vertices": outcome.composition.coincident_vertices,
            "exterior_triangles": outcome.composition.exterior_triangles,
            "interior_removed": outcome.composition.interior_removed,
            "welded_degenerate_removed": outcome.composition.welded_degenerate_removed,
            "island_components_removed": outcome.composition.island_components_removed,
            "island_triangles_removed": outcome.composition.island_triangles_removed,
            "tip_triangles_removed": outcome.composition.tip_triangles_removed,
        },
        "vertex_sources": outcome.vertex_sources,
        "selected_faces": outcome.selected_faces,
        "removed": removals_json(outcome),
        "loops": loops_json(&outcome.loops),
        "apertures": apertures_json(&outcome.apertures),
        "incidental": incidental_json(&outcome.loops, &outcome.apertures),
        "actions": actions_json(outcome),
        "reduction": {
            "current_triangles": outcome.reduction.current_triangles,
            "target_triangles": outcome.reduction.target_triangles,
            "ratio": outcome.reduction.ratio,
            "removed": outcome.reduction.removed,
        },
    })
}

/// Removed source triangles grouped by the stage that removed them.
fn removals_json(outcome: &ExteriorOutcome) -> Value {
    let mut grouped: std::collections::BTreeMap<&'static str, Vec<u32>> =
        std::collections::BTreeMap::new();
    for removal in &outcome.removals {
        grouped
            .entry(removal.reason.name())
            .or_default()
            .push(removal.triangle);
    }
    let mut object = serde_json::Map::new();
    for reason in [
        RemovalReason::Interior,
        RemovalReason::Island,
        RemovalReason::CornerFlare,
        RemovalReason::WeldedDegenerate,
    ] {
        object.insert(
            reason.name().to_string(),
            Value::Array(
                grouped
                    .remove(reason.name())
                    .unwrap_or_default()
                    .into_iter()
                    .map(Value::from)
                    .collect(),
            ),
        );
    }
    Value::Object(object)
}

/// Native closures the source stage requires, already limit-checked in the core.
fn actions_json(outcome: &ExteriorOutcome) -> Value {
    actions_value(&outcome.actions, &outcome.loops)
}

fn actions_value(actions: &[RepairAction], loops: &[BoundaryLoop]) -> Value {
    Value::Array(
        actions
            .iter()
            .map(|action| match action {
                RepairAction::Collapse {
                    loop_index,
                    edges,
                    edge,
                    length,
                } => json!({
                    "kind": "collapse",
                    "loop": loop_index,
                    "edges": edges,
                    "edge": edge,
                    "length": length,
                    "vertices": loops[*loop_index].vertices,
                }),
                RepairAction::Fill {
                    loop_index,
                    edges,
                    closure,
                } => json!({
                    "kind": "fill",
                    "loop": loop_index,
                    "edges": edges,
                    "closure": closure.name(),
                    "vertices": loops[*loop_index].vertices,
                }),
            })
            .collect(),
    )
}

fn repair_plan(control: &Path, profile: &Path, outcome: &RepairOutcome) -> Value {
    json!({
        "stage": "repair",
        "control": control.display().to_string(),
        "profile": profile.display().to_string(),
        "vertices": outcome.positions.len(),
        "triangles": outcome.triangles.len(),
        "tip_removed": outcome.tip_removed,
        "tip_vertices": outcome.tip_vertices,
        "tip_triangles": outcome.tip_triangles,
        "loops": loops_json(&outcome.loops),
        "apertures": apertures_json(&outcome.apertures),
        "actions": actions_value(&outcome.actions, &outcome.loops),
    })
}

fn deviation_json(deviation: &Deviation) -> Value {
    json!({
        "points": deviation.points,
        "max": deviation.max,
        "mean": deviation.mean,
        "p95": deviation.p95,
        "covered": deviation.covered,
        "coverage": deviation.coverage,
    })
}

/// Everything `control-check` measured, including what failed to measure.
struct CheckEvidence<'a> {
    diagnostics: Option<&'a ControlDiagnostics>,
    qualification: Option<&'a Result<Qualification, ControlError>>,
    deviations: Option<&'a Result<Deviations, ControlError>>,
    source_error: Option<&'a ControlError>,
    verdict: &'a Result<(), String>,
}

fn check_report(
    control: &Path,
    source: &Path,
    profile_path: &Path,
    evidence: CheckEvidence<'_>,
    profile: &ControlProfile,
) -> Value {
    let measurement = evidence.diagnostics.map(|diagnostics| {
        json!({
            "vertices": diagnostics.vertices,
            "triangles": diagnostics.triangles,
            "edges": diagnostics.edges,
            "euler": diagnostics.euler,
            "components": diagnostics.components,
            "non_manifold_vertices": diagnostics.non_manifold_vertices,
            "non_manifold_edges": diagnostics.non_manifold_edges,
            "orientation_conflicts": diagnostics.orientation_conflicts,
            "coincident_vertices": diagnostics.coincident_vertices,
            "loop_vertex_space": "exact-position",
            "vertex_sources": diagnostics.vertex_sources,
            "self_contacts": diagnostics.self_contacts,
            "bounds": diagnostics.bounds,
            "extent": diagnostics.extent(),
            "loops": loops_json(&diagnostics.loops),
        })
    });
    let qualification = evidence.qualification.map(|qualified| match qualified {
        Ok(qualification) => json!({
            "passed": true,
            "apertures": qualification.apertures.iter().map(|matched| json!({
                "role": matched.role,
                "loop": matched.loop_index,
                "distance": matched.distance,
            })).collect::<Vec<_>>(),
            "incidental": qualification.incidental,
        }),
        Err(error) => json!({"passed": false, "error": error.to_string()}),
    });
    let deviations = evidence
        .deviations
        .map(|deviations| match deviations {
            Ok(deviations) => json!({
                "control_to_source": deviation_json(&deviations.control_to_source),
                "source_exterior_to_control": deviation_json(
                    &deviations.source_exterior_to_control
                ),
            }),
            Err(error) => json!({"error": error.to_string()}),
        })
        .or_else(|| {
            evidence
                .source_error
                .map(|error| json!({"source_error": error.to_string()}))
        });
    json!({
        "stage": "check",
        "control": control.display().to_string(),
        "source": source.display().to_string(),
        "profile": profile_path.display().to_string(),
        "measured": measurement.unwrap_or(Value::Null),
        "qualification": qualification.unwrap_or(Value::Null),
        "deviations": deviations.unwrap_or(Value::Null),
        "coverage": {
            "tolerance": profile.coverage.tolerance,
            "minimum": profile.coverage.minimum,
        },
        "verdict": match evidence.verdict {
            Ok(()) => json!({"passed": true}),
            Err(reason) => json!({"passed": false, "reason": reason}),
        },
    })
}

fn print_exterior(out: &Path, outcome: &ExteriorOutcome, plan: &Path) {
    let roles = outcome
        .apertures
        .matched
        .iter()
        .map(|matched| {
            format!(
                "{} ({} edges)",
                matched.role, outcome.loops[matched.loop_index].edges
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "{}: {} vertices, {} triangles from {} source triangles",
        out.display(),
        outcome.positions.len(),
        outcome.triangles.len(),
        outcome.composition.source_triangles
    );
    println!(
        "kept {} exterior of {} (interior {}, welded degenerate {}, islands {}, tips {})",
        outcome.composition.exterior_triangles,
        outcome.composition.source_triangles,
        outcome.composition.interior_removed,
        outcome.composition.welded_degenerate_removed,
        outcome.composition.island_triangles_removed,
        outcome.composition.tip_triangles_removed
    );
    println!("apertures: {roles}");
    println!(
        "reduction: {} -> {} triangles at ratio {:.5}",
        outcome.reduction.current_triangles,
        outcome.reduction.target_triangles,
        outcome.reduction.ratio
    );
    println!("plan: {}", plan.display());
}

fn print_repair(out: &Path, outcome: &RepairOutcome, plan: &Path) {
    println!(
        "{}: {} vertices, {} triangles, {} corner-flare triangles removed",
        out.display(),
        outcome.positions.len(),
        outcome.triangles.len(),
        outcome.tip_removed
    );
    println!(
        "apertures: {}, incidental loops {}",
        outcome.apertures.matched.len(),
        outcome.apertures.incidental.len()
    );
    for action in &outcome.actions {
        match action {
            RepairAction::Collapse {
                loop_index,
                edges,
                edge,
                length,
            } => println!(
                "collapse loop {loop_index}: {edges} edges, shortest {:?} at {length:.6}",
                edge
            ),
            RepairAction::Fill {
                loop_index,
                edges,
                closure,
            } => {
                println!(
                    "fill loop {loop_index}: {edges} edges, {} patch",
                    closure.name()
                );
            }
        }
    }
    println!("plan: {}", plan.display());
}

fn print_check(
    measured: Option<&ControlDiagnostics>,
    qualified: Option<&Result<Qualification, ControlError>>,
    deviations: Option<&Deviations>,
    report: &Path,
) {
    if let Some(diagnostics) = measured {
        println!(
            "topology: {} vertices, {} triangles, {} edges, euler {}, {} component(s), \
             non-manifold vertices {}, edges {}, winding conflicts {}, coincident {}, self contacts {}",
            diagnostics.vertices,
            diagnostics.triangles,
            diagnostics.edges,
            diagnostics.euler,
            diagnostics.components,
            diagnostics.non_manifold_vertices,
            diagnostics.non_manifold_edges,
            diagnostics.orientation_conflicts,
            diagnostics.coincident_vertices,
            diagnostics.self_contacts
        );
        let loops = diagnostics
            .loops
            .iter()
            .map(|boundary| boundary.edges.to_string())
            .collect::<Vec<_>>()
            .join("/");
        println!(
            "openings: {} loops ({loops} edges)",
            diagnostics.loops.len()
        );
    }
    match qualified {
        Some(Ok(qualification)) => println!(
            "apertures: {} matched, {} incidental",
            qualification.apertures.len(),
            qualification.incidental
        ),
        Some(Err(error)) => println!("qualification: rejected — {error}"),
        None => println!("qualification: not measurable"),
    }
    if let Some(deviations) = deviations {
        println!(
            "deviation: control to source mean {:.6} p95 {:.6} max {:.6}; \
             source exterior to control mean {:.6} p95 {:.6} max {:.6}",
            deviations.control_to_source.mean,
            deviations.control_to_source.p95,
            deviations.control_to_source.max,
            deviations.source_exterior_to_control.mean,
            deviations.source_exterior_to_control.p95,
            deviations.source_exterior_to_control.max
        );
    }
    println!("report: {}", report.display());
}
