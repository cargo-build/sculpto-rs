//! Eight-pair native parity matrix generalising `parity_native.rs`.
//!
//! Evaluates the two-body parity matrix:
//! - 2 sets: `HIMBO Body - Vanilla Nevernude`, `CBBE Body`
//! - 2 presets: `HIMBO Mike`, one CBBE preset on host (e.g. `CBBE Curvy` or `- Zeroed Sliders -`)
//! - 2 endpoints: `SliderSize::Small` (`_0`), `SliderSize::Big` (`_1`)
//!
//! For each pair:
//! - resolve the plan from `SCULPTO_BODYSLIDE_HOME`;
//! - apply it to the source NIF and compare against the native `_0`/`_1` output
//!   when present, else print `SKIPPED: <pair> native output absent` and return;
//! - assertions when native output exists:
//!   - vertex count exact
//!   - per-vertex max deviation <= 1e-6
//!   - weights <= 1e-4
//!   - triangle set exact
//! - wrong-endpoint negative control: the same comparison against the other
//!   endpoint must fail.
//!
//! Native files are read from host paths under or alongside the environment home;
//! no private asset payloads are committed.

use std::path::{Path, PathBuf};

use sculpto_bake::BakePlan;
use sculpto_core::{Document, Preset, Project, SliderSet, SliderSize};

/// Calculate 3D Euclidean distance between two positions.
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Body family mapping to project and preset files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFamily {
    Himbo,
    Cbbe,
}

impl BodyFamily {
    /// Return the `.osp` slider set document path under BodySlide home.
    pub fn osp_path(self, home: &Path) -> PathBuf {
        match self {
            Self::Himbo => home.join("SliderSets/HIMBO.osp"),
            Self::Cbbe => home.join("SliderSets/CBBE.osp"),
        }
    }

    /// Return the `.xml` slider presets document path under BodySlide home.
    pub fn preset_path(self, home: &Path) -> PathBuf {
        match self {
            Self::Himbo => home.join("SliderPresets/HIMBO.xml"),
            Self::Cbbe => home.join("SliderPresets/CBBE.xml"),
        }
    }
}

/// Specifications for one matrix cell.
#[derive(Debug, Clone)]
pub struct MatrixPair {
    pub set_family: BodyFamily,
    pub set_name: &'static str,
    pub preset_family: BodyFamily,
    pub preset_name: String,
    pub size: SliderSize,
}

impl MatrixPair {
    /// Human-readable label for reporting.
    pub fn label(&self) -> String {
        let size_str = match self.size {
            SliderSize::Small => "Small (0)",
            SliderSize::Big => "Big (1)",
        };
        format!("{} | {} | {}", self.set_name, self.preset_name, size_str)
    }

    /// Suffix corresponding to the endpoint index (`_0` or `_1`).
    pub fn endpoint_suffix(&self) -> &'static str {
        match self.size {
            SliderSize::Small => "_0.nif",
            SliderSize::Big => "_1.nif",
        }
    }

    /// Opposite endpoint suffix used for negative controls.
    pub fn opposite_suffix(&self) -> &'static str {
        match self.size {
            SliderSize::Small => "_1.nif",
            SliderSize::Big => "_0.nif",
        }
    }
}

/// Measured outcome for one parity evaluation.
#[derive(Debug, Clone)]
pub struct ParityMeasurement {
    pub label: String,
    pub vertices: usize,
    pub max_deviation: f32,
    pub p99_deviation: f32,
    pub max_weight_diff: f32,
    pub triangles_exact: bool,
    pub negative_control_max_deviation: f32,
}

/// Discover the host CBBE preset to evaluate.
///
/// Priority:
/// 1. `SCULPTO_CBBE_PRESET` environment variable
/// 2. `- Zeroed Sliders -` when verified native batch build is found in `/tmp/opencode/bs-cbbe`
/// 3. Standard host presets (`CBBE Curvy`, `CBBE Slim`, `CBBE Vanilla`)
pub fn detect_cbbe_preset(home: &Path) -> String {
    if let Some(var) = std::env::var_os("SCULPTO_CBBE_PRESET") {
        return var.to_string_lossy().into_owned();
    }
    // When the verified opencode batch build is present, default to `- Zeroed Sliders -`
    if Path::new("/tmp/opencode/bs-cbbe/meshes/actors/character/character assets/femalebody_1.nif")
        .is_file()
    {
        return "- Zeroed Sliders -".to_string();
    }
    let presets_file = BodyFamily::Cbbe.preset_path(home);
    if let Ok(Document::Presets(presets)) = Document::parse_file(&presets_file) {
        for candidate in &[
            "CBBE Curvy",
            "CBBE Slim",
            "CBBE Vanilla",
            "- Zeroed Sliders -",
        ] {
            if presets.iter().any(|p| p.name == *candidate) {
                return (*candidate).to_string();
            }
        }
        if let Some(first) = presets.first() {
            return first.name.clone();
        }
    }
    "CBBE Curvy".to_string()
}

/// Search for a native NIF matching the requested set, preset, and suffix.
fn find_native_nif(
    home: &Path,
    set: &SliderSet,
    preset_name: &str,
    suffix: &str,
) -> Option<PathBuf> {
    let output_file_stem = &set.output_file.name;
    let expected_file_name = format!("{output_file_stem}{suffix}");
    let rel_subpath = set.output_path.replace('\\', "/");
    let full_rel_path = Path::new(&rel_subpath).join(&expected_file_name);

    // 1. Explicit environment variable overrides
    let direct_env = match suffix {
        "_1.nif" => std::env::var_os("SCULPTO_NATIVE_NIF"),
        "_0.nif" => std::env::var_os("SCULPTO_NATIVE_NIF_SMALL"),
        _ => None,
    };
    if let Some(path) = direct_env {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(dir) = std::env::var_os("SCULPTO_NATIVE_DIR") {
        let p_rel = Path::new(&dir).join(&full_rel_path);
        if p_rel.is_file() {
            return Some(p_rel);
        }
        let p_direct = Path::new(&dir).join(&expected_file_name);
        if p_direct.is_file() {
            return Some(p_direct);
        }
    }

    // Preset directory slugs
    let preset_slugs: Vec<String> = match preset_name {
        "HIMBO Mike" => vec!["mike".to_string(), "himbo-mike".to_string()],
        "HIMBO Chonky" => vec!["himbo-chonky".to_string(), "chonky".to_string()],
        "- Zeroed Sliders -" => vec!["zeroed".to_string(), "cbbe-zeroed".to_string()],
        "CBBE Curvy" => vec!["curvy".to_string(), "cbbe-curvy".to_string()],
        "CBBE Slim" => vec!["slim".to_string(), "cbbe-slim".to_string()],
        other => vec![other.to_lowercase().replace(' ', "-"), other.to_string()],
    };

    let mut candidate_dirs = Vec::new();
    // Directly under home output
    candidate_dirs.push(home.join("output"));
    // Under parent of home (e.g. Models/bodyslide-project-.../output)
    if let Some(parent) = home.parent() {
        candidate_dirs.push(parent.join("output"));
        // Known male target output path
        candidate_dirs.push(parent.join("clothing-male-targets-20261101/outputs"));
        if let Some(grandparent) = parent.parent() {
            candidate_dirs.push(grandparent.join("clothing-male-targets-20261101/outputs"));
        }
    }

    for dir in &candidate_dirs {
        for slug in &preset_slugs {
            let p = dir.join(slug).join(&full_rel_path);
            if p.is_file() {
                return Some(p);
            }
            let p_flat = dir.join(slug).join(&expected_file_name);
            if p_flat.is_file() {
                return Some(p_flat);
            }
        }
    }

    // Direct match under home
    let direct = home.join(&full_rel_path);
    if direct.is_file() {
        return Some(direct);
    }

    // Host fixture: /tmp/opencode/bs-cbbe strictly for CBBE zeroed build
    if output_file_stem == "femalebody" && preset_name == "- Zeroed Sliders -" {
        let p = Path::new("/tmp/opencode/bs-cbbe").join(&full_rel_path);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

/// Load and parse a Project from its family path.
fn load_project(home: &Path, family: BodyFamily) -> Project {
    let path = family.osp_path(home);
    match Document::parse_file(&path) {
        Ok(Document::Project(project)) => project,
        other => panic!("expected a project at {:?}: {other:?}", path),
    }
}

/// Load and parse Presets from its family path.
fn load_presets(home: &Path, family: BodyFamily) -> Vec<Preset> {
    let path = family.preset_path(home);
    match Document::parse_file(&path) {
        Ok(Document::Presets(presets)) => presets,
        other => panic!("expected presets at {:?}: {other:?}", path),
    }
}

/// Run parity comparison on one cell of the matrix.
///
/// Returns `Some(ParityMeasurement)` when native output exists and all assertions pass.
/// Returns `None` when native output is absent, printing a skip notice.
pub fn evaluate_pair(home: &Path, pair: &MatrixPair) -> Option<ParityMeasurement> {
    let label = pair.label();
    let project = load_project(home, pair.set_family);
    let presets = load_presets(home, pair.preset_family);

    let Some(preset) = presets.iter().find(|p| p.name == pair.preset_name) else {
        println!(
            "SKIPPED: {label} (preset not found in {:?})",
            pair.preset_family.preset_path(home)
        );
        return None;
    };

    let set = match project.sets.iter().find(|s| s.name == pair.set_name) {
        Some(set) => set,
        None => {
            println!(
                "SKIPPED: {label} (set not found in {:?})",
                pair.set_family.osp_path(home)
            );
            return None;
        }
    };

    // Locate matching native output
    let Some(native_path) = find_native_nif(home, set, &pair.preset_name, pair.endpoint_suffix())
    else {
        println!("SKIPPED: {label} native output absent");
        return None;
    };

    // Locate opposite native output for negative control
    let opposite_native_path =
        find_native_nif(home, set, &pair.preset_name, pair.opposite_suffix());

    // Resolve bake plan
    let plan = match BakePlan::resolve(&project, preset, pair.set_name, pair.size, home) {
        Ok(plan) => plan,
        Err(err) => panic!("{label}: plan failed to resolve: {err:?}"),
    };
    assert!(
        plan.unresolved.is_empty(),
        "{label}: unresolved morph records: {:?}",
        plan.unresolved
    );

    // Read source NIF
    let folder = set.data_folder.replace('\\', "/");
    let source_file = set.source_file.replace('\\', "/");
    let source_path = home.join("ShapeData").join(folder).join(source_file);
    let source_bytes = std::fs::read(&source_path)
        .unwrap_or_else(|e| panic!("{label}: failed to read source NIF {:?}: {e}", source_path));
    let source_doc = sculpto_nif::read(&source_bytes).unwrap_or_else(|e| {
        panic!(
            "{label}: failed to parse source NIF {:?}: {e:?}",
            source_path
        )
    });

    // Select primary body shape
    let source_shape = source_doc
        .shapes
        .iter()
        .max_by_key(|shape| shape.positions.len())
        .unwrap_or_else(|| panic!("{label}: source NIF contains no shapes"));

    // Find the slider target name associated with this shape
    let target_name = set
        .shapes
        .iter()
        .find(|s| s.name == source_shape.name)
        .map(|s| s.target.as_str())
        .unwrap_or(source_shape.name.as_str());

    // Apply plan offsets
    let ours = plan
        .apply(target_name, &source_shape.positions)
        .unwrap_or_else(|e| panic!("{label}: failed to apply offsets: {e:?}"));

    // Read native NIF
    let native_bytes = std::fs::read(&native_path)
        .unwrap_or_else(|e| panic!("{label}: failed to read native NIF {:?}: {e}", native_path));
    let native_doc = sculpto_nif::read(&native_bytes).unwrap_or_else(|e| {
        panic!(
            "{label}: failed to parse native NIF {:?}: {e:?}",
            native_path
        )
    });
    let native_shape = native_doc
        .shapes
        .iter()
        .find(|s| s.name == source_shape.name)
        .unwrap_or_else(|| {
            native_doc
                .shapes
                .iter()
                .max_by_key(|shape| shape.positions.len())
                .unwrap_or_else(|| panic!("{label}: native NIF contains no shapes"))
        });

    // Assertion 1: vertex count exact
    assert_eq!(
        ours.len(),
        native_shape.positions.len(),
        "{label}: vertex counts differ (ours {} vs native {})",
        ours.len(),
        native_shape.positions.len()
    );

    // Compute vertex deviations
    let mut distances: Vec<f32> = ours
        .iter()
        .zip(&native_shape.positions)
        .map(|(a, b)| distance(*a, *b))
        .collect();
    distances.sort_by(|a, b| a.partial_cmp(b).expect("finite distances"));
    let max_deviation = *distances.last().unwrap_or(&0.0);
    let p99_deviation = distances[(distances.len() as f64 * 0.99) as usize];

    // Assertion 2: per-vertex max deviation <= 1e-6
    assert!(
        max_deviation <= 1e-6,
        "{label}: per-vertex max deviation {:.9} exceeds tolerance 1e-6",
        max_deviation
    );

    // Compute skin weight differences
    let mut max_weight_diff = 0.0f32;
    for (sw, nw) in source_shape.weights.iter().zip(&native_shape.weights) {
        for (a, b) in sw.iter().zip(nw.iter()) {
            let diff = (a - b).abs();
            if diff > max_weight_diff {
                max_weight_diff = diff;
            }
        }
    }

    // Assertion 3: weights <= 1e-4
    assert!(
        max_weight_diff <= 1e-4,
        "{label}: max weight difference {:.9} exceeds tolerance 1e-4",
        max_weight_diff
    );

    // Assertion 4: triangle set exact
    let triangles_exact = source_shape.triangles == native_shape.triangles;
    assert!(
        triangles_exact,
        "{label}: triangle set differs between source and native"
    );

    // Negative control: comparison against the other endpoint must fail
    let negative_control_max_dev = if let Some(opp_path) = opposite_native_path {
        let opp_bytes = std::fs::read(&opp_path).unwrap_or_else(|e| {
            panic!(
                "{label}: failed to read opposite native NIF {:?}: {e}",
                opp_path
            )
        });
        let opp_doc = sculpto_nif::read(&opp_bytes).unwrap_or_else(|e| {
            panic!(
                "{label}: failed to parse opposite native NIF {:?}: {e:?}",
                opp_path
            )
        });
        let opp_shape = opp_doc
            .shapes
            .iter()
            .find(|s| s.name == source_shape.name)
            .unwrap_or_else(|| {
                opp_doc
                    .shapes
                    .iter()
                    .max_by_key(|shape| shape.positions.len())
                    .unwrap_or_else(|| panic!("{label}: opposite NIF has no shapes"))
            });
        let opp_dev = ours
            .iter()
            .zip(&opp_shape.positions)
            .map(|(a, b)| distance(*a, *b))
            .fold(0.0f32, f32::max);
        assert!(
            opp_dev > 1e-6,
            "{label}: wrong-endpoint negative control failed: deviation {:.9} is <= 1e-6",
            opp_dev
        );
        opp_dev
    } else {
        println!("  (negative control skipped: opposite endpoint file absent)");
        0.0
    };

    println!(
        "PASS: {} ({} vertices, max dev {:.9} u, p99 {:.9} u, max weight diff {:.9}, triangles exact {}, neg control dev {:.6} u)",
        label,
        ours.len(),
        max_deviation,
        p99_deviation,
        max_weight_diff,
        triangles_exact,
        negative_control_max_dev
    );

    Some(ParityMeasurement {
        label,
        vertices: ours.len(),
        max_deviation,
        p99_deviation,
        max_weight_diff,
        triangles_exact,
        negative_control_max_deviation: negative_control_max_dev,
    })
}

/// Construct the eight matrix combinations (2 sets × 2 presets × 2 endpoints).
pub fn build_matrix(home: &Path) -> Vec<MatrixPair> {
    let cbbe_preset = detect_cbbe_preset(home);

    let sets = [
        (BodyFamily::Himbo, "HIMBO Body - Vanilla Nevernude"),
        (BodyFamily::Cbbe, "CBBE Body"),
    ];
    let presets = [
        (BodyFamily::Himbo, "HIMBO Mike".to_string()),
        (BodyFamily::Cbbe, cbbe_preset),
    ];
    let endpoints = [SliderSize::Small, SliderSize::Big];

    let mut matrix = Vec::with_capacity(8);
    for &(set_family, set_name) in &sets {
        for (preset_family, preset_name) in &presets {
            for &size in &endpoints {
                matrix.push(MatrixPair {
                    set_family,
                    set_name,
                    preset_family: *preset_family,
                    preset_name: preset_name.clone(),
                    size,
                });
            }
        }
    }
    matrix
}

#[test]
fn eight_pair_matrix_sweep() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from) else {
        println!("SKIPPED: SCULPTO_BODYSLIDE_HOME is not set");
        return;
    };

    let matrix = build_matrix(&home);
    assert_eq!(matrix.len(), 8, "matrix must contain exactly 8 pairs");

    println!("=== Eight-Pair Native Parity Matrix ===");
    let mut passed = Vec::new();
    let mut skipped = Vec::new();

    for pair in &matrix {
        match evaluate_pair(&home, pair) {
            Some(measurement) => passed.push(measurement),
            None => skipped.push(pair.label()),
        }
    }

    println!("=== Matrix Summary ===");
    println!("Total cells: {}", matrix.len());
    println!("Evaluated (PASS): {}", passed.len());
    println!("Skipped (ABSENT): {}", skipped.len());
    for m in &passed {
        println!(
            "  [PASS] {} — max dev: {:.9} u, neg control: {:.6} u",
            m.label, m.max_deviation, m.negative_control_max_deviation
        );
    }
    for s in &skipped {
        println!("  [SKIPPED] {}", s);
    }

    // When native outputs are present on host, at least the HIMBO Mike default body must pass
    if !passed.is_empty() {
        assert!(
            passed.iter().any(|m| m.label.contains("HIMBO Mike")),
            "HIMBO Mike must pass when fixtures exist"
        );
    }
}

#[test]
fn himbo_mike_small_endpoint() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from) else {
        return;
    };
    let pair = MatrixPair {
        set_family: BodyFamily::Himbo,
        set_name: "HIMBO Body - Vanilla Nevernude",
        preset_family: BodyFamily::Himbo,
        preset_name: "HIMBO Mike".to_string(),
        size: SliderSize::Small,
    };
    if let Some(m) = evaluate_pair(&home, &pair) {
        assert!(m.max_deviation <= 1e-6);
        assert!(m.max_weight_diff <= 1e-4);
        assert!(m.triangles_exact);
        assert!(m.negative_control_max_deviation > 0.1);
    }
}

#[test]
fn himbo_mike_big_endpoint() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from) else {
        return;
    };
    let pair = MatrixPair {
        set_family: BodyFamily::Himbo,
        set_name: "HIMBO Body - Vanilla Nevernude",
        preset_family: BodyFamily::Himbo,
        preset_name: "HIMBO Mike".to_string(),
        size: SliderSize::Big,
    };
    if let Some(m) = evaluate_pair(&home, &pair) {
        assert!(m.max_deviation <= 1e-6);
        assert!(m.max_weight_diff <= 1e-4);
        assert!(m.triangles_exact);
        assert!(m.negative_control_max_deviation > 0.1);
    }
}

#[test]
fn cbbe_small_endpoint() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from) else {
        return;
    };
    let cbbe_preset = detect_cbbe_preset(&home);
    let pair = MatrixPair {
        set_family: BodyFamily::Cbbe,
        set_name: "CBBE Body",
        preset_family: BodyFamily::Cbbe,
        preset_name: cbbe_preset,
        size: SliderSize::Small,
    };
    if let Some(m) = evaluate_pair(&home, &pair) {
        assert!(m.max_deviation <= 1e-6);
        assert!(m.max_weight_diff <= 1e-4);
        assert!(m.triangles_exact);
        assert!(m.negative_control_max_deviation > 0.1);
    }
}

#[test]
fn cbbe_big_endpoint() {
    let Some(home) = std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from) else {
        return;
    };
    let cbbe_preset = detect_cbbe_preset(&home);
    let pair = MatrixPair {
        set_family: BodyFamily::Cbbe,
        set_name: "CBBE Body",
        preset_family: BodyFamily::Cbbe,
        preset_name: cbbe_preset,
        size: SliderSize::Big,
    };
    if let Some(m) = evaluate_pair(&home, &pair) {
        assert!(m.max_deviation <= 1e-6);
        assert!(m.max_weight_diff <= 1e-4);
        assert!(m.triangles_exact);
        assert!(m.negative_control_max_deviation > 0.1);
    }
}
