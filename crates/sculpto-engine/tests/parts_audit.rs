//! BLD-11 acceptance: `body.audit_parts` is registered, read-only, accepts the
//! correct synthetic body, and refuses a deliberately moved part by name with
//! the measured value.

use std::path::PathBuf;

use sculpto_engine::{ErrorKind, catalogue, dispatch};
use serde_json::json;

const SIDES: usize = 8;

/// One octagon ring, OBJ text order.
fn ring(radius: f64, z: f64) -> Vec<[f64; 3]> {
    (0..SIDES)
        .map(|k| {
            let angle = k as f64 * std::f64::consts::FRAC_PI_4;
            [radius * angle.cos(), radius * angle.sin(), z]
        })
        .collect()
}

/// Write the synthetic four-shell body: torso (rings z 4 and 11), leg (top ring
/// coincident with the torso bottom), foot (ring over capped bottom), head
/// (loop over capped top), with the whole head optionally dropped by
/// `head_drop` and the foot by `foot_drop`. The body is designed Z-up and
/// written in the OBJ display frame (`[x, z, -y]`), the convention
/// `read_obj_surface` converts back to source.
fn body_obj(head_drop: f64, foot_drop: f64) -> String {
    let rings = [
        ring(2.0, 4.0),
        ring(2.0, 11.0),
        ring(2.0, 4.0),
        ring(0.9, 1.0),
        ring(1.0, 1.5 + foot_drop),
        ring(0.8, 0.0 + foot_drop),
        ring(2.0, 11.8 + head_drop),
        ring(1.4, 13.6 + head_drop),
    ];
    let mut text = String::new();
    for ring in &rings {
        for &[x, y, z] in ring.iter() {
            let (display_y, display_z) = (z, -y);
            text.push_str(&format!("v {x:.6} {display_y:.6} {display_z:.6}\n"));
        }
    }
    for (bottom, top) in [(0usize, 8usize), (16, 24), (32, 40), (48, 56)] {
        for k in 0..SIDES {
            let next = (k + 1) % SIDES;
            let b0 = bottom + k + 1;
            let b1 = bottom + next + 1;
            let t0 = top + k + 1;
            let t1 = top + next + 1;
            text.push_str(&format!("f {b0} {b1} {t1}\n"));
            text.push_str(&format!("f {b0} {t1} {t0}\n"));
        }
    }
    // Caps: foot bottom (base 40), head top (base 56), 1-based.
    for base in [40usize, 56usize] {
        for k in 1..SIDES - 1 {
            text.push_str(&format!(
                "f {} {} {}\n",
                base + 1,
                base + k + 1,
                base + k + 2
            ));
        }
    }
    text
}

fn write_body(tag: &str, head_drop: f64, foot_drop: f64) -> PathBuf {
    let path = std::env::temp_dir().join(format!("sculpto-bld11-{tag}-{}.obj", std::process::id()));
    std::fs::write(&path, body_obj(head_drop, foot_drop)).expect("fixture written");
    path
}

/// The head shell alone (16 vertices), its prism and cap faces, with the neck
/// ring yawed by `yaw_degrees` about the design vertical. Written in the OBJ
/// display frame like the body fixture.
fn head_override_obj(yaw_degrees: f64) -> String {
    let head_rings = [ring(2.0, 11.8), ring(1.4, 13.6)];
    let (sin, cos) = yaw_degrees.to_radians().sin_cos();
    let mut text = String::new();
    for (index, [x, y, z]) in head_rings.iter().flatten().enumerate() {
        let (x, y) = match index < SIDES {
            true => (x * cos - y * sin, x * sin + y * cos),
            false => (*x, *y),
        };
        let (display_y, display_z) = (*z, -y);
        text.push_str(&format!("v {x:.6} {display_y:.6} {display_z:.6}\n"));
    }
    for k in 0..SIDES {
        let next = (k + 1) % SIDES;
        let b0 = k + 1;
        let b1 = next + 1;
        let t0 = SIDES + k + 1;
        let t1 = SIDES + next + 1;
        text.push_str(&format!("f {b0} {b1} {t1}\n"));
        text.push_str(&format!("f {b0} {t1} {t0}\n"));
    }
    for k in 1..SIDES - 1 {
        text.push_str(&format!(
            "f {} {} {}\n",
            SIDES + 1,
            SIDES + k + 1,
            SIDES + k + 2
        ));
    }
    text
}

#[test]
fn registry_lists_the_read_only_audit() {
    let spec = catalogue()
        .iter()
        .find(|spec| spec.id.as_str() == "body.audit_parts")
        .expect("body.audit_parts registered");
    assert!(!spec.mutates, "the audit writes nothing");
    assert!(spec.implemented());
    assert_eq!(spec.input_schema()["additionalProperties"], false);
    assert!(spec.summary.to_lowercase().contains("read-only"));
}

#[test]
fn correct_body_accepts_and_reports_parts() {
    let path = write_body("accept", 0.0, 0.0);
    let out = dispatch(
        "body.audit_parts",
        &json!({ "body": path.display().to_string() }),
    )
    .unwrap();
    assert_eq!(out["verdict"], "accept");
    let parts = out["parts"].as_array().expect("part rows");
    assert_eq!(parts.len(), 3, "leg, foot, head");
    assert_eq!(parts[2]["name"], "head");
    let head_slot = parts[2]["seams"][0]["slot_min"].as_f64().unwrap();
    assert!((head_slot - 0.8).abs() < 1e-3, "{head_slot}");
    assert_eq!(out["open_sockets"].as_array().unwrap().len(), 0);
}

#[test]
fn a_dropped_head_is_refused_by_name_with_the_measured_slot() {
    let path = write_body("drop", -0.5, 0.0);
    let error = dispatch(
        "body.audit_parts",
        &json!({ "body": path.display().to_string() }),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "part_head_slot");
    assert!(error.message().contains("head"), "{}", error.message());
    assert!(
        error.message().contains("0.30"),
        "measured value named: {}",
        error.message()
    );
}

#[test]
fn an_override_part_file_is_judged_against_the_shipped_baseline() {
    // The shipped body carries the correct head; the override yaw-rotates the
    // neck ring 15 degrees, which breaks the coverage ceiling only.
    let shipped = write_body("baseline", 0.0, 0.0);
    let override_path =
        std::env::temp_dir().join(format!("sculpto-bld11-override-{}.obj", std::process::id()));
    std::fs::write(&override_path, head_override_obj(15.0)).expect("override written");
    let error = dispatch(
        "body.audit_parts",
        &json!({
            "body": shipped.display().to_string(),
            "parts": [override_path.display().to_string()],
        }),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "part_head_coverage");
    assert!(error.message().contains("head"), "{}", error.message());
    assert!(
        error.message().contains("authored 0.8"),
        "{}",
        error.message()
    );
}

#[test]
fn a_refused_audit_keeps_every_refusal_in_the_message() {
    let path = write_body("twodef", -0.5, -0.3);
    let error = dispatch(
        "body.audit_parts",
        &json!({ "body": path.display().to_string() }),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert!(error.message().contains("head"), "{}", error.message());
    assert!(error.message().contains("foot"), "{}", error.message());
    assert!(
        !error.message().contains("+1 more"),
        "every refusal is spelled out: {}",
        error.message()
    );
}

#[test]
fn strict_false_returns_the_full_report_with_the_refusals() {
    let path = write_body("report", -0.5, 0.0);
    let out = dispatch(
        "body.audit_parts",
        &json!({
            "body": path.display().to_string(),
            "strict": false,
        }),
    )
    .expect("the report mode carries the outcome");
    assert_eq!(out["verdict"], "refuse");
    let refusals = out["refusals"].as_array().expect("typed refusals");
    assert!(!refusals.is_empty());
    assert_eq!(refusals[0]["code"], "part_head_slot");
    let parts = out["parts"]
        .as_array()
        .expect("per-part rows stay reachable");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2]["verdict"], "part_head_slot");
}
