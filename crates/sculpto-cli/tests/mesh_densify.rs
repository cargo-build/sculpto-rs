//! `mesh densify` on the argv surface: same envelope as `run --cmd`, same
//! artifact, plus the real-data acceptance on the supplied 664-v panty and the
//! golden 3BA panty.
//!
//! The real tests are path-gated: they run when the documented boethos
//! fixtures exist (or `SCULPTO_DENSIFY_PANTY` / `SCULPTO_DENSIFY_3BA_PANTY`
//! point at copies) and skip silently otherwise, so the portable suite never
//! depends on a host.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

fn work_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sculpto-cli-densify-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run_ok(command: &mut Command) -> String {
    let output = command.output().expect("spawn");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is UTF-8")
}

/// The `result` member of the one envelope every surface prints.
fn dispatched(stdout: &str) -> Value {
    let value: Value = serde_json::from_str(stdout.trim()).expect("one JSON envelope on stdout");
    assert_eq!(value["ok"], true, "the command failed: {value}");
    value["result"].clone()
}

fn read_positions(path: &Path) -> Vec<[f64; 3]> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .filter_map(|line| line.strip_prefix("v "))
        .map(|rest| {
            let numbers: Vec<f64> = rest
                .split_whitespace()
                .map(|value| value.parse().expect("f64"))
                .collect();
            [numbers[0], numbers[1], numbers[2]]
        })
        .collect()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Median edge length over the face list (0-based corner indices).
fn median_edge(positions: &[[f64; 3]], faces: &[[usize; 3]]) -> f64 {
    let mut lengths: Vec<f64> = faces
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .map(|(a, b)| distance(positions[a], positions[b]))
        .collect();
    lengths.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    lengths[lengths.len() / 2]
}

fn read_faces(path: &Path) -> Vec<[usize; 3]> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .filter_map(|line| line.strip_prefix("f "))
        .map(|rest| {
            let corners: Vec<usize> = rest
                .split_whitespace()
                .map(|corner| {
                    corner
                        .split('/')
                        .next()
                        .expect("corner")
                        .parse()
                        .expect("index")
                })
                .collect();
            [corners[0] - 1, corners[1] - 1, corners[2] - 1]
        })
        .collect()
}

/// A kite fixture: four boundary corners around one interior vertex.
fn kite_obj() -> String {
    "v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0.5 0.5 0
f 1 2 5
f 2 3 5
f 3 4 5
f 4 1 5
"
    .to_string()
}

#[test]
fn argv_and_run_surfaces_agree() {
    let dir = work_dir("parity");
    let input = PathBuf::from(&dir).join("kite.obj");
    std::fs::write(&input, kite_obj()).expect("fixture");

    let argv_out = dir.join("argv.obj");
    let argv_stdout = run_ok(sculpto().args([
        "mesh",
        "densify",
        "--in",
        input.to_str().unwrap(),
        "--out",
        argv_out.to_str().unwrap(),
        "--cuts",
        "1",
        "--smooth-iterations",
        "2",
    ]));

    let run_out = dir.join("run.obj");
    let run_stdout = run_ok(
        sculpto().args([
            "run",
            "--cmd",
            "mesh.densify",
            "--input",
            &json!({
                "in": input.display().to_string(),
                "out": run_out.display().to_string(),
                "cuts": 1,
                "smooth_iterations": 2,
            })
            .to_string(),
        ]),
    );

    // Same artifact bytes and same numbers, different transport.
    assert_eq!(
        std::fs::read(&argv_out).expect("argv artifact"),
        std::fs::read(&run_out).expect("run artifact"),
        "densify artifacts differ between argv and command surfaces"
    );
    let argv = dispatched(&argv_stdout);
    let run = dispatched(&run_stdout);
    for field in [
        "vertices",
        "triangles",
        "boundary_max_drift",
        "diagnostic",
        "smoothing",
    ] {
        assert_eq!(
            argv[field], run[field],
            "field `{field}` differs between surfaces"
        );
    }
    // The kite densifies to its deterministic counts.
    assert_eq!(argv["source_vertices"], 5);
    assert_eq!(argv["vertices"], 13);
    assert_eq!(argv["triangles"], 16);
    assert_eq!(argv["diagnostic"], true);
}

#[test]
fn unknown_flags_are_refused() {
    let output = sculpto()
        .args([
            "mesh", "densify", "--in", "x.obj", "--out", "y.obj", "--cuts-x", "1",
        ])
        .output()
        .expect("spawn");
    assert_eq!(output.status.code(), Some(2), "unknown flag must exit 2");
}

/// The documented supplied-asset fixture, or a host override; `None` skips.
fn panty() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SCULPTO_DENSIFY_PANTY") {
        return Some(PathBuf::from(path));
    }
    let default = home().join(".local/state/boethos/extracted/underwear/female-cbbe-panty.obj");
    default.exists().then_some(default)
}

/// The golden 3BA panty, or a host override; `None` skips.
fn panty_3ba() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SCULPTO_DENSIFY_3BA_PANTY") {
        return Some(PathBuf::from(path));
    }
    let default = home().join(".local/state/boethos/extracted/underwear/female-3ba-panty.obj");
    default.exists().then_some(default)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).expect("$HOME")
}

/// The documented acceptance numbers for a 664 v / 1,056 f panty: counts,
/// boundary drift zero, per-part preservation, twin integrity and the densified
/// edge resolution that makes the 0.9 u junction and the 0.29 u hem scallop
/// visible.
fn assert_panty_acceptance(source: &Path, tag: &str) {
    let dir = work_dir(tag);
    let out = dir.join("panty.d1.obj");
    let stdout = run_ok(sculpto().args([
        "mesh",
        "densify",
        "--in",
        source.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--cuts",
        "1",
    ]));
    let report = dispatched(&stdout);
    assert_eq!(report["source_vertices"], 664, "{tag}: source vertices");
    assert_eq!(report["source_triangles"], 1056, "{tag}: source triangles");
    assert_eq!(report["vertices"], 2380, "{tag}: V' = V + E");
    assert_eq!(report["triangles"], 4224, "{tag}: F' = 4F");
    assert_eq!(report["diagnostic"], true);
    // Boundaries and parts are exactly preserved; nothing drifts.
    assert_eq!(report["boundary_max_drift"], 0.0);
    assert_eq!(
        report["boundary_edges"]["before"].as_u64().expect("count"),
        report["boundary_edges"]["after"].as_u64().expect("count") / 2
    );
    // The documented panty boundary inventory: 264 raw boundary edges, and
    // every boundary edge adds exactly one midpoint vertex to the boundary.
    assert_eq!(
        report["boundary_edges"]["before"], 264,
        "{tag}: boundary edges"
    );
    assert_eq!(
        report["boundary_vertices"]["before"]
            .as_u64()
            .expect("count")
            + report["boundary_edges"]["before"].as_u64().expect("count"),
        report["boundary_vertices"]["after"]
            .as_u64()
            .expect("count"),
        "{tag}: boundary vertices grow by one midpoint per boundary edge"
    );
    assert_eq!(
        report["components"]["indexed"]["before"],
        report["components"]["indexed"]["after"]
    );
    assert_eq!(
        report["components"]["exact_position"]["before"],
        report["components"]["exact_position"]["after"]
    );
    assert!(
        !report["openings"]["before"]
            .as_array()
            .expect("rows")
            .is_empty()
            && !report["openings"]["after"]
                .as_array()
                .expect("rows")
                .is_empty(),
        "{tag}: both boundary inventories are reported (the loop partition may \
         differ at pinch vertices; the totals above are the invariant)"
    );

    // Vertex identity on the source range: the authored junction twins (the
    // pair the dress solve tears 0.92 u apart) stay exactly coincident under
    // the neutral densify.
    let before = read_positions(source);
    let after = read_positions(&out);
    assert_eq!(after.len(), 2380);
    assert!(
        distance(before[225], before[257]) < 1e-6,
        "{tag}: authored pair 225/257 is not coincident in the input"
    );
    assert!(
        distance(after[225], after[257]) < 1e-6,
        "{tag}: the junction twins must stay coincident — a densify is not a solve"
    );

    // The resolution witness: subdivided edges halve, which is what makes the
    // authored 0.29 u hem scallop cross the visibility threshold.
    let source_faces = read_faces(source);
    let out_faces = read_faces(&out);
    let ratio = median_edge(&after, &out_faces) / median_edge(&before, &source_faces);
    assert!(
        (0.4..=0.6).contains(&ratio),
        "{tag}: median edge ratio {ratio} is not a halving"
    );

    // A smoothing request on the authored boundary loops is allowed and
    // reports the drift it must keep at zero.
    let smooth_out = dir.join("panty.smooth.obj");
    let stdout = run_ok(sculpto().args([
        "mesh",
        "densify",
        "--in",
        source.to_str().unwrap(),
        "--out",
        smooth_out.to_str().unwrap(),
        "--cuts",
        "1",
        "--smooth-iterations",
        "4",
    ]));
    let report = dispatched(&stdout);
    assert_eq!(report["smoothing"]["enabled"], true);
    assert_eq!(report["smoothing"]["boundary_max_displacement"], 0.0);
    assert!(
        report["smoothing"]["max_displacement"]
            .as_f64()
            .expect("number")
            > 0.0,
        "{tag}: smoothing must move interior vertices"
    );
    assert_eq!(report["boundary_max_drift"], 0.0);
}

#[test]
fn the_supplied_panty_densifies_to_the_documented_numbers() {
    let Some(panty) = panty() else {
        eprintln!("skip: no supplied panty fixture");
        return;
    };
    assert_panty_acceptance(&panty, "panty-cbbe");
}

#[test]
fn the_golden_3ba_panty_densifies_to_the_documented_numbers() {
    let Some(panty) = panty_3ba() else {
        eprintln!("skip: no golden 3BA panty fixture");
        return;
    };
    assert_panty_acceptance(&panty, "panty-3ba");
}

#[test]
fn the_supplied_panty_densifies_twice_to_the_documented_counts() {
    let Some(panty) = panty() else {
        eprintln!("skip: no supplied panty fixture");
        return;
    };
    let dir = work_dir("panty-cuts2");
    let out = dir.join("panty.d2.obj");
    let stdout = run_ok(sculpto().args([
        "mesh",
        "densify",
        "--in",
        panty.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--cuts",
        "2",
    ]));
    let report = dispatched(&stdout);
    // V2 = V + E + E1, E1 = 2E + 3F = 6,600: 664 + 1,716 + 6,600 = 8,980.
    assert_eq!(report["vertices"], 8980);
    assert_eq!(report["triangles"], 16896);
    assert_eq!(report["boundary_max_drift"], 0.0);
}
