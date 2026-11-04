//! `sculpto garment control-*` end to end on an explicit synthetic shell.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// A double-walled tube in the display frame the CLI reads.
struct Shell {
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    start: [f32; 3],
    end: [f32; 3],
}

fn shell(offset: [f32; 3], scale: f32) -> Shell {
    let sides = 6;
    let stations = [-0.5f32, -0.2, 0.2, 0.5];
    let mut positions = Vec::new();
    let ring = |station: f32, radius: f32, positions: &mut Vec<[f32; 3]>| {
        (0..sides)
            .map(|side| {
                let angle = std::f32::consts::TAU * side as f32 / sides as f32;
                positions.push([
                    offset[0] + scale * radius * angle.cos(),
                    offset[1] + scale * radius * angle.sin(),
                    offset[2] + scale * station,
                ]);
                (positions.len() - 1) as u32
            })
            .collect::<Vec<_>>()
    };
    let outer = stations
        .iter()
        .map(|station| ring(*station, 0.2, &mut positions))
        .collect::<Vec<_>>();
    let inner = stations
        .iter()
        .map(|station| ring(*station, 0.1, &mut positions))
        .collect::<Vec<_>>();
    let mut triangles = Vec::new();
    for station in 0..stations.len() - 1 {
        for side in 0..sides {
            let next = (side + 1) % sides;
            triangles.push([
                outer[station][side],
                outer[station][next],
                outer[station + 1][next],
            ]);
            triangles.push([
                outer[station][side],
                outer[station + 1][next],
                outer[station + 1][side],
            ]);
            triangles.push([
                inner[station][side],
                inner[station + 1][next],
                inner[station][next],
            ]);
            triangles.push([
                inner[station][side],
                inner[station + 1][side],
                inner[station + 1][next],
            ]);
        }
    }
    for side in 0..sides {
        let next = (side + 1) % sides;
        let last = stations.len() - 1;
        triangles.push([outer[0][side], inner[0][side], inner[0][next]]);
        triangles.push([outer[0][side], inner[0][next], outer[0][next]]);
        triangles.push([outer[last][side], inner[last][next], inner[last][side]]);
        triangles.push([outer[last][side], outer[last][next], inner[last][next]]);
    }
    Shell {
        positions,
        triangles,
        start: [offset[0], offset[1], offset[2] + scale * stations[0]],
        end: [
            offset[0],
            offset[1],
            offset[2] + scale * stations[stations.len() - 1],
        ],
    }
}

/// Source `(x, y, z)` is written as display `(x, z, -y)`, the CLI's OBJ frame.
fn write_obj(path: &Path, positions: &[[f32; 3]], triangles: &[[u32; 3]]) {
    let mut text = String::from("# synthetic thick shell\n");
    for [x, y, z] in positions {
        text.push_str(&format!("v {x:.6} {z:.6} {:.6}\n", -y));
    }
    for [a, b, c] in triangles {
        text.push_str(&format!("f {} {} {}\n", a + 1, b + 1, c + 1));
    }
    fs::write(path, text).unwrap();
}

fn profile_json(shell: &Shell, scale: f32) -> String {
    let [sx, sy, sz] = shell.start;
    let [ex, ey, ez] = shell.end;
    let below = sz - 0.25 * scale;
    let above = ez + 0.25 * scale;
    format!(
        r#"{{
  "frame": "source-local-z-up",
  "guide_segments": [
    [[{sx:.6}, {sy:.6}, {below:.6}], [{sx:.6}, {sy:.6}, {sz:.6}]],
    [[{sx:.6}, {sy:.6}, {sz:.6}], [{ex:.6}, {ey:.6}, {ez:.6}]],
    [[{ex:.6}, {ey:.6}, {ez:.6}], [{ex:.6}, {ey:.6}, {above:.6}]]
  ],
  "apertures": [
    {{"role": "low", "anchor": [{sx:.6}, {sy:.6}, {sz:.6}], "radius": {radius:.6}}},
    {{"role": "high", "anchor": [{ex:.6}, {ey:.6}, {ez:.6}], "radius": {radius:.6}}}
  ],
  "source_repair": {{
    "max_incidental_loops": 2,
    "collapse_below_edges": 0,
    "max_fill_edges": 32,
    "max_tip_removal_share": 0.05
  }},
  "reduced_repair": {{
    "max_incidental_loops": 2,
    "collapse_below_edges": 6,
    "max_fill_edges": 32,
    "max_tip_removal_share": 0.05
  }},
  "reduction": {{
    "target_triangles": 40,
    "minimum_triangles": 12,
    "maximum_triangles": 60
  }},
  "coverage": {{"tolerance": {tolerance:.6}, "minimum": 0.5}},
  "contact": {{"mode": "reject", "clearance": 0.000001, "max_displacement": 0.001, "max_pairs": 1}}
}}"#,
        radius = 0.06 * scale,
        tolerance = 0.01 * scale,
    )
}

fn directory() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-control-cli-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(arguments)
        .output()
        .unwrap()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn exterior_reconstructs_a_thick_shell_and_check_accepts_it() {
    let directory = directory();
    let source = directory.join("source.obj");
    let profile = directory.join("profile.json");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    let report = directory.join("report.json");

    let shell = shell([0.0, 0.0, 0.0], 1.0);
    write_obj(&source, &shell.positions, &shell.triangles);
    fs::write(&profile, profile_json(&shell, 1.0)).unwrap();

    let exterior = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(
        exterior.status.success(),
        "{}",
        String::from_utf8_lossy(&exterior.stderr)
    );
    let plan = read_json(&plan);
    assert_eq!(plan["stage"], "exterior");
    assert_eq!(plan["apertures"]["matched"].as_array().unwrap().len(), 2);
    assert_eq!(plan["incidental"].as_array().unwrap().len(), 0);
    assert_eq!(
        plan["composition"]["interior_removed"].as_u64().unwrap(),
        60,
        "the inner wall and both rims are not cloth"
    );
    assert_eq!(
        plan["composition"]["exterior_triangles"].as_u64().unwrap(),
        36
    );
    assert_eq!(plan["vertices"].as_u64().unwrap(), 24);
    assert_eq!(plan["reduction"]["ratio"].as_f64().unwrap(), 1.0);

    let check = run(&[
        "garment",
        "control-check",
        "--control",
        control.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(
        check.status.success(),
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    let report = read_json(&report);
    assert!(report["qualification"]["passed"].as_bool().unwrap());
    assert_eq!(report["measured"]["euler"].as_i64().unwrap(), 0);
    assert_eq!(report["measured"]["self_contacts"].as_u64().unwrap(), 0);
    assert_eq!(report["measured"]["loop_vertex_space"], "exact-position");
    assert_eq!(report["measured"]["vertex_sources"].as_array().unwrap().len(), 24);
    assert_eq!(
        report["qualification"]["apertures"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn another_name_translation_and_scale_change_nothing() {
    let first = directory();
    let second = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let moved = shell;
    let scaled = 2.5;
    let offset = [4.0, -2.0, 1.0];

    let plans = [
        (&first, "source.obj", "profile.json", 1.0, [0.0, 0.0, 0.0]),
        (
            &second,
            "cell-b.obj",
            "unknown-garment.json",
            scaled,
            offset,
        ),
    ]
    .into_iter()
    .map(|(directory, mesh_name, profile_name, scale, offset)| {
        let source = directory.join(mesh_name);
        let profile = directory.join(profile_name);
        let control = directory.join("out.obj");
        let plan = directory.join("plan.json");
        let fixture = shell_for(&moved, offset, scale);
        write_obj(&source, &fixture.positions, &fixture.triangles);
        fs::write(&profile, profile_json(&fixture, scale)).unwrap();
        let run = run(&[
            "garment",
            "control-exterior",
            "--source",
            source.to_str().unwrap(),
            "--profile",
            profile.to_str().unwrap(),
            "--out",
            control.to_str().unwrap(),
            "--plan",
            plan.to_str().unwrap(),
        ]);
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        read_json(&plan)
    })
    .collect::<Vec<_>>();

    assert_eq!(plans[0]["composition"], plans[1]["composition"]);
    assert_eq!(plans[0]["vertices"], plans[1]["vertices"]);
    assert_eq!(plans[0]["triangles"], plans[1]["triangles"]);
    let roles = |plan: &Value| {
        plan["apertures"]["matched"]
            .as_array()
            .unwrap()
            .iter()
            .map(|matched| matched["role"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(roles(&plans[0]), roles(&plans[1]));
    fs::remove_dir_all(&first).unwrap();
    fs::remove_dir_all(&second).unwrap();
}

fn shell_for(shell: &Shell, offset: [f32; 3], scale: f32) -> Shell {
    Shell {
        positions: shell
            .positions
            .iter()
            .map(|point| {
                [
                    offset[0] + scale * point[0],
                    offset[1] + scale * point[1],
                    offset[2] + scale * point[2],
                ]
            })
            .collect(),
        triangles: shell.triangles.clone(),
        start: [
            offset[0] + scale * shell.start[0],
            offset[1] + scale * shell.start[1],
            offset[2] + scale * shell.start[2],
        ],
        end: [
            offset[0] + scale * shell.end[0],
            offset[1] + scale * shell.end[1],
            offset[2] + scale * shell.end[2],
        ],
    }
}

#[test]
fn a_wrong_aperture_anchor_fails_with_the_declared_radius() {
    let directory = directory();
    let source = directory.join("source.obj");
    let profile = directory.join("profile.json");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    write_obj(&source, &shell.positions, &shell.triangles);
    let genuine = profile_json(&shell, 1.0);
    let wrong = genuine.replace(
        &format!(
            "[{:.6}, {:.6}, {:.6}]",
            shell.end[0], shell.end[1], shell.end[2]
        ),
        "[0.0, 0.0, 0.9]",
    );
    assert_ne!(wrong, genuine, "the fixture must really move its anchor");
    fs::write(&profile, wrong).unwrap();
    let run = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("no boundary loop lies within"),
        "unexpected diagnostic: {stderr}"
    );
    assert!(
        !control.exists(),
        "a rejected construction writes no surface"
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn repair_plans_one_closure_for_a_punched_triangle() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let source = directory.join("source.obj");
    let profile = directory.join("profile.json");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    write_obj(&source, &shell.positions, &shell.triangles);
    fs::write(&profile, profile_json(&shell, 1.0)).unwrap();

    let exterior = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(exterior.status.success());

    let mut text = fs::read_to_string(&control).unwrap();
    // A triangle between two interior stations: its three edges stay one loop.
    let hole = text
        .lines()
        .filter(|line| line.starts_with("f "))
        .nth(13)
        .unwrap()
        .to_string();
    text = text.replace(&format!("{hole}\n"), "");
    let holed = directory.join("holed.obj");
    fs::write(&holed, text).unwrap();

    let repair_plan = directory.join("repair.json");
    let repaired = directory.join("repaired.obj");
    let repair = run(&[
        "garment",
        "control-repair",
        "--control",
        holed.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        repaired.to_str().unwrap(),
        "--plan",
        repair_plan.to_str().unwrap(),
    ]);
    assert!(
        repair.status.success(),
        "{}",
        String::from_utf8_lossy(&repair.stderr)
    );
    let report = read_json(&repair_plan);
    let actions = report["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["kind"], "collapse");
    assert_eq!(actions[0]["edges"], 3);
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_control_far_from_its_source_fails_the_coverage_gate() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let source = directory.join("source.obj");
    let profile = directory.join("profile.json");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    let report = directory.join("report.json");
    write_obj(&source, &shell.positions, &shell.triangles);
    fs::write(&profile, profile_json(&shell, 1.0)).unwrap();

    let exterior = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(exterior.status.success());

    // Widen the control: its openings keep their centres, its surface does not
    // follow the source any more.
    let moved = directory.join("moved.obj");
    let text = fs::read_to_string(&control)
        .unwrap()
        .lines()
        .map(|line| match line.strip_prefix("v ") {
            Some(rest) => {
                let parts = rest
                    .split_whitespace()
                    .map(|value| value.parse::<f32>().unwrap())
                    .collect::<Vec<_>>();
                format!(
                    "v {:.6} {:.6} {:.6}",
                    parts[0] * 1.3,
                    parts[1],
                    parts[2] * 1.3
                )
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&moved, text + "\n").unwrap();

    let check = run(&[
        "garment",
        "control-check",
        "--control",
        moved.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(!check.status.success());
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(
        stderr.contains("coverage"),
        "unexpected diagnostic: {stderr}"
    );
    assert!(report.exists(), "diagnostics are retained on failure");
    let report = read_json(&report);
    assert!(
        report["qualification"]["passed"].as_bool().unwrap(),
        "topology is fine"
    );
    assert!(
        report["deviations"]["source_exterior_to_control"]["coverage"]
            .as_f64()
            .unwrap()
            < 0.5
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn final_triangle_bounds_are_enforced_by_the_cli() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let source = directory.join("source.obj");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    let report = directory.join("report.json");
    write_obj(&source, &shell.positions, &shell.triangles);
    let genuine = profile_json(&shell, 1.0);
    let accepted = directory.join("accepted.json");
    fs::write(&accepted, &genuine).unwrap();
    let exterior = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        accepted.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(
        exterior.status.success(),
        "{}",
        String::from_utf8_lossy(&exterior.stderr)
    );
    let triangles = read_json(&plan)["triangles"].as_u64().unwrap() as usize;

    // Bounds above the produced surface: accepted, not merely reported.
    let check = run(&[
        "garment",
        "control-check",
        "--control",
        control.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--profile",
        accepted.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(
        check.status.success(),
        "{}{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(read_json(&report)["verdict"]["passed"].as_bool().unwrap());

    // A declared maximum below the surface must fail the same command.
    let narrow = directory.join("narrow.json");
    let maximum = triangles - 1;
    let minimum = maximum.saturating_sub(10);
    fs::write(
        &narrow,
        genuine
            .replace(
                "\"target_triangles\": 40",
                &format!("\"target_triangles\": {maximum}"),
            )
            .replace(
                "\"minimum_triangles\": 12",
                &format!("\"minimum_triangles\": {minimum}"),
            )
            .replace(
                "\"maximum_triangles\": 60",
                &format!("\"maximum_triangles\": {maximum}"),
            ),
    )
    .unwrap();
    let rejected = run(&[
        "garment",
        "control-check",
        "--control",
        control.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--profile",
        narrow.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(
        !rejected.status.success(),
        "a bound violation must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&rejected.stderr);
    assert!(stderr.contains("above the declared maximum"), "{stderr}");
    assert!(!read_json(&report)["verdict"]["passed"].as_bool().unwrap());
    assert!(
        read_json(&report)["qualification"]["error"]
            .as_str()
            .unwrap()
            .contains("maximum")
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn source_stage_limits_refuse_before_any_native_operation() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    // Punch one interior wall triangle: the source stage then carries one incidental loop.
    let mut positions = shell.positions.clone();
    let mut triangles = shell.triangles.clone();
    let interior = 2 * 12;
    triangles.remove(interior);
    let source = directory.join("source.obj");
    write_obj(&source, &positions, &triangles);
    positions.clear();

    let permissive = directory.join("permissive.json");
    fs::write(&permissive, profile_json(&shell, 1.0)).unwrap();
    let refused = directory.join("refused.json");
    fs::write(
        &refused,
        profile_json(&shell, 1.0)
            .replace("\"max_incidental_loops\": 2", "\"max_incidental_loops\": 0"),
    )
    .unwrap();
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");

    let permissive_run = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        permissive.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(
        permissive_run.status.success(),
        "{}",
        String::from_utf8_lossy(&permissive_run.stderr)
    );
    let actions = read_json(&plan);
    assert_eq!(actions["incidental"].as_array().unwrap().len(), 1);
    assert_eq!(actions["actions"].as_array().unwrap().len(), 1);
    assert_eq!(actions["actions"][0]["kind"], "fill");

    let strict_control = directory.join("strict.obj");
    let strict_plan = directory.join("strict-plan.json");
    let refused_run = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        refused.to_str().unwrap(),
        "--out",
        strict_control.to_str().unwrap(),
        "--plan",
        strict_plan.to_str().unwrap(),
    ]);
    assert!(!refused_run.status.success());
    let stderr = String::from_utf8_lossy(&refused_run.stderr);
    assert!(stderr.contains("non-aperture boundary loops"), "{stderr}");
    assert!(
        !strict_control.exists(),
        "a refused source stage writes no surface"
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_reversed_face_is_rejected_by_the_cli() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let source = directory.join("source.obj");
    let profile = directory.join("profile.json");
    let control = directory.join("control.obj");
    let plan = directory.join("plan.json");
    let report = directory.join("report.json");
    write_obj(&source, &shell.positions, &shell.triangles);
    fs::write(&profile, profile_json(&shell, 1.0)).unwrap();
    let exterior = run(&[
        "garment",
        "control-exterior",
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--out",
        control.to_str().unwrap(),
        "--plan",
        plan.to_str().unwrap(),
    ]);
    assert!(exterior.status.success());

    let text = fs::read_to_string(&control).unwrap();
    let mut flipped = None;
    let mut lines = Vec::new();
    for line in text.lines() {
        match line.strip_prefix("f ") {
            Some(rest) if flipped.is_none() => {
                let parts = rest.split_whitespace().collect::<Vec<_>>();
                lines.push(format!("f {} {} {}", parts[0], parts[2], parts[1]));
                flipped = Some(());
            }
            _ => lines.push(line.to_string()),
        }
    }
    assert!(flipped.is_some(), "the control has faces");
    let reversed = directory.join("reversed.obj");
    fs::write(&reversed, lines.join("\n") + "\n").unwrap();

    let check = run(&[
        "garment",
        "control-check",
        "--control",
        reversed.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(
        !check.status.success(),
        "inconsistent winding must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("winding is inconsistent"), "{stderr}");
    let report = read_json(&report);
    assert!(!report["verdict"]["passed"].as_bool().unwrap());
    assert!(
        report["measured"]["orientation_conflicts"]
            .as_u64()
            .unwrap()
            > 0
    );
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn declared_and_plane_frames_report_the_same_planes() {
    let directory = directory();
    let shell = shell([0.0, 0.0, 0.0], 1.0);
    let base = profile_json(&shell, 1.0);
    let source_cut = r#""source_cuts": [
    {"label": "top", "plane_point": [0.0, 0.0, 0.25], "normal": [0.0, 0.0, 1.0], "retain": "negative"}
  ],
  "#;
    // Display `(x, y, z)` is source `(x, -z, y)`: this is the same plane as the source
    // frame one, written in the other declared frame.
    let display_cut = r#""source_cuts": [
    {"label": "top", "plane_point": [0.0, 0.25, 0.0], "normal": [0.0, 1.0, 0.0], "retain": "negative"}
  ],
  "#;
    let source_profile = directory.join("source-frame.json");
    let display_profile = directory.join("display-frame.json");
    let source_report = directory.join("source-frame-report.json");
    let display_report = directory.join("display-frame-report.json");
    fs::write(
        &source_profile,
        base.replace(
            "\"source_repair\"",
            &format!("{source_cut}\"source_repair\""),
        ),
    )
    .unwrap();
    fs::write(
        &display_profile,
        base.replace(
            "\"source_repair\"",
            &format!("{display_cut}\"source_repair\""),
        )
        .replace(
            "\"frame\": \"source-local-z-up\"",
            "\"frame\": \"sculpto-display-y-up\"",
        ),
    )
    .unwrap();

    for (profile, report) in [
        (&source_profile, &source_report),
        (&display_profile, &display_report),
    ] {
        let output = run(&[
            "garment",
            "control-cuts",
            "--profile",
            profile.to_str().unwrap(),
            "--report",
            report.to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let source = read_json(&source_report);
    let display = read_json(&display_report);
    // The planes are always emitted in the mesh frame, whatever was declared.
    assert_eq!(source["plane_frame"], "source-local-z-up");
    assert_eq!(display["plane_frame"], "source-local-z-up");
    assert_eq!(source["declared_frame"], "source-local-z-up");
    assert_eq!(display["declared_frame"], "sculpto-display-y-up");
    assert_eq!(
        source["cuts"], display["cuts"],
        "the same plane declared in two frames must emit identical mesh-frame planes"
    );
}
