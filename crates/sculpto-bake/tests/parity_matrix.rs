//! Same-family native parity matrix for the HIMBO and CBBE body families.
//!
//! Exactly six cells run on a provisioned host, and every pairing is
//! same-family — a HIMBO slider set with HIMBO presets, the CBBE set with a
//! CBBE preset. Cross-family pairings (a male set with a CBBE preset) are not
//! buildable BodySlide combinations and are never constructed:
//!
//! - `HIMBO Body - Vanilla Nevernude` × {`HIMBO Mike`, `HIMBO Chonky`}
//!   × {small `_0`, big `_1`} — four cells;
//! - `CBBE Body` × `SCULPTO_CBBE_PRESET` × {small, big} — two cells.
//!
//! The female family has one preset on purpose: `SCULPTO_CBBE_PRESET` must
//! name the preset the native fixture was built with, so the suite cannot
//! guess a preset identity it cannot verify. This host has no second female
//! native build (no `CBBE Curvy` output), so no second female cell is
//! fabricated.
//!
//! Every cell serialises the bake through our SE writer (`write_se_shape`)
//! and compares the re-read artifact against the native output, so each
//! comparison is bake-vs-native, never source-vs-native:
//! - vertex counts asserted (non-zero) before any zip;
//! - positions <= 1e-6;
//! - triangles and bone lists exact;
//! - UVs <= 1e-3 and weights <= 1e-3 — both bounded by our writer's `f16`
//!   storage, the same tolerance class as the C14 round-trip suite;
//! - wrong-endpoint negative control required per cell, deviation > 0.1 u —
//!   one threshold for both families and every entry point;
//! - a non-identity guard: at least one evaluated cell must carry a non-empty
//!   plan and move vertices by more than 1e-6 against its source.
//!
//! `SCULPTO_BODYSLIDE_HOME` unset: each entry point prints one explicit
//! `SKIPPED` line and stops. Set: a missing required native fixture (either
//! endpoint) is a failure, never a skip.
//!
//! Native files are read from host paths under or alongside the environment
//! home (plus `SCULPTO_NATIVE_DIR`/`SCULPTO_NATIVE_NIF*` overrides); no
//! private asset payloads are committed.

use std::path::{Path, PathBuf};

use sculpto_bake::BakePlan;
use sculpto_core::mesh::MeshAsset;
use sculpto_core::{Document, Preset, Project, SliderSet, SliderSize};
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::{NifDocument, NifShape};

/// Position tolerance, one value for both body families.
const POSITION_TOLERANCE_U: f32 = 1e-6;
/// UV tolerance: our SE writer stores UVs as `f16` (C14 round-trip suite).
const UV_TOLERANCE: f32 = 1e-3;
/// Weight tolerance: our SE writer stores weights as `f16` (C14 round-trip).
const WEIGHT_TOLERANCE: f32 = 1e-3;
/// Wrong-endpoint control floor, one threshold for both families and every
/// entry point.
const NEGATIVE_CONTROL_MIN_DEVIATION_U: f32 = 0.1;
/// Non-identity guard floor: somewhere the bake must move vertices further
/// than this.
const NON_IDENTITY_MIN_DEVIATION_U: f32 = 1e-6;
/// Cell count this suite actually runs: 4 HIMBO (two presets × two endpoints)
/// + 2 CBBE (one env-named preset × two endpoints).
const EXPECTED_CELLS: usize = 6;

/// Calculate 3D Euclidean distance between two positions.
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Maximum elementwise absolute difference of two `f32` slots.
fn max_abs_diff(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max)
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

/// Specifications for one same-family matrix cell.
#[derive(Debug, Clone)]
pub struct MatrixPair {
    pub family: BodyFamily,
    pub set_name: &'static str,
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
        format!(
            "{:?} | {} | {} | {}",
            self.family, self.set_name, self.preset_name, size_str
        )
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

/// Measured outcome for one parity evaluation. Every field is measured; no
/// sentinel values.
#[derive(Debug, Clone)]
pub struct ParityMeasurement {
    pub family: BodyFamily,
    pub label: String,
    pub native_path: PathBuf,
    pub plan_blocks: usize,
    pub vertices: usize,
    pub max_deviation: f32,
    pub p99_deviation: f32,
    pub max_weight_diff: f32,
    pub uv_max_diff: f32,
    pub triangles_exact: bool,
    pub bones_equal: bool,
    pub bake_moved_source: f32,
    pub negative_control_deviation: f32,
}

/// The female preset must be named; the suite never infers a native build's
/// preset identity.
fn cbbe_preset_name() -> String {
    std::env::var("SCULPTO_CBBE_PRESET").unwrap_or_else(|_| {
        panic!(
            "SCULPTO_CBBE_PRESET must name the preset the native CBBE fixture \
             was built with; the suite does not infer it"
        )
    })
}

/// Shared fixture gate: without `SCULPTO_BODYSLIDE_HOME` each entry point
/// reports one explicit skip; with it, missing fixtures are failures.
fn home_or_skip() -> Option<PathBuf> {
    match std::env::var_os("SCULPTO_BODYSLIDE_HOME") {
        Some(home) => Some(PathBuf::from(home)),
        None => {
            println!("SKIPPED: SCULPTO_BODYSLIDE_HOME is not set");
            None
        }
    }
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

/// Select the shape in a parsed document that corresponds to the source body
/// shape: same name when the writer kept it, else the largest shape.
fn select_shape<'a>(
    document: &'a NifDocument,
    source_name: &str,
    label: &str,
    role: &str,
) -> &'a NifShape {
    document
        .shapes
        .iter()
        .find(|s| s.name == source_name)
        .unwrap_or_else(|| {
            document
                .shapes
                .iter()
                .max_by_key(|shape| shape.positions.len())
                .unwrap_or_else(|| panic!("{label}: {role} NIF contains no shapes"))
        })
}

/// Read and parse a NIF from `path`, panicking with the cell label on failure.
fn read_nif(path: &Path, label: &str, role: &str) -> NifDocument {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("{label}: failed to read {role} NIF {path:?}: {e}"));
    sculpto_nif::read(&bytes)
        .unwrap_or_else(|e| panic!("{label}: failed to parse {role} NIF {path:?}: {e:?}"))
}

/// Run parity comparison on one cell of the matrix.
///
/// Every required fixture (native output for the cell and for its opposite
/// endpoint) must exist: with `SCULPTO_BODYSLIDE_HOME` set a missing fixture
/// is a failure, not a skip.
pub fn evaluate_pair(home: &Path, pair: &MatrixPair) -> ParityMeasurement {
    let label = pair.label();
    let project = load_project(home, pair.family);
    let presets = load_presets(home, pair.family);

    let preset = presets
        .iter()
        .find(|p| p.name == pair.preset_name)
        .unwrap_or_else(|| {
            panic!(
                "{label}: preset {:?} not found in {:?}",
                pair.preset_name,
                pair.family.preset_path(home)
            )
        });

    let set = project
        .sets
        .iter()
        .find(|s| s.name == pair.set_name)
        .unwrap_or_else(|| {
            panic!(
                "{label}: set {:?} not found in {:?}",
                pair.set_name,
                pair.family.osp_path(home)
            )
        });

    // Both endpoints are required: the cell comparison and its negative control.
    let native_path = find_native_nif(home, set, &pair.preset_name, pair.endpoint_suffix())
        .unwrap_or_else(|| {
            panic!(
                "{label}: required native fixture not found — with \
                 SCULPTO_BODYSLIDE_HOME set a missing fixture is a failure, not a skip"
            )
        });
    let opposite_native_path =
        find_native_nif(home, set, &pair.preset_name, pair.opposite_suffix()).unwrap_or_else(
            || {
                panic!(
                    "{label}: opposite-endpoint native fixture not found; \
                     the wrong-endpoint negative control is required"
                )
            },
        );

    // Resolve bake plan
    let plan = BakePlan::resolve(&project, preset, pair.set_name, pair.size, home)
        .unwrap_or_else(|err| panic!("{label}: plan failed to resolve: {err:?}"));
    assert!(
        plan.unresolved.is_empty(),
        "{label}: unresolved morph records: {:?}",
        plan.unresolved
    );
    let plan_blocks = plan.block_count();

    // Read source NIF and select the primary body shape
    let folder = set.data_folder.replace('\\', "/");
    let source_file = set.source_file.replace('\\', "/");
    let source_path = home.join("ShapeData").join(folder).join(source_file);
    let source_doc = read_nif(&source_path, &label, "source");
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

    // Non-identity evidence for this cell: how far the bake moved the source.
    let bake_moved_source = ours
        .iter()
        .zip(&source_shape.positions)
        .map(|(a, b)| distance(*a, *b))
        .fold(0.0f32, f32::max);

    // Serialise the bake through our own writer so every comparison below is
    // bake-vs-native: the artifact carries the baked positions and the
    // arrays our pipeline passes through.
    let bake_asset = MeshAsset {
        positions: ours,
        ..MeshAsset::from(source_shape)
    };
    let baked_bytes = write_se_shape(&bake_asset)
        .unwrap_or_else(|e| panic!("{label}: our SE writer refused the baked body: {e:?}"));
    let baked_doc = sculpto_nif::read(&baked_bytes)
        .unwrap_or_else(|e| panic!("{label}: our written SE NIF failed to parse: {e:?}"));
    let baked = baked_doc
        .shapes
        .first()
        .unwrap_or_else(|| panic!("{label}: our written SE NIF contains no shapes"));

    // Read native NIF and select the matching shape
    let native_doc = read_nif(&native_path, &label, "native");
    let native_shape = select_shape(&native_doc, &source_shape.name, &label, "native");

    // Assertion: vertex counts (non-zero) before any zip
    assert_eq!(
        baked.positions.len(),
        native_shape.positions.len(),
        "{label}: vertex counts differ (bake {} vs native {})",
        baked.positions.len(),
        native_shape.positions.len()
    );
    assert!(
        !baked.positions.is_empty(),
        "{label}: empty vertex arrays cannot prove parity"
    );

    // Positions: per-vertex deviation bake vs native
    let mut distances: Vec<f32> = baked
        .positions
        .iter()
        .zip(&native_shape.positions)
        .map(|(a, b)| distance(*a, *b))
        .collect();
    distances.sort_by(|a, b| a.partial_cmp(b).expect("finite distances"));
    let max_deviation = distances[distances.len() - 1];
    let p99_deviation = distances[(distances.len() as f64 * 0.99) as usize];

    // Assertion: per-vertex max deviation <= 1e-6
    assert!(
        max_deviation <= POSITION_TOLERANCE_U,
        "{label}: per-vertex max deviation {max_deviation:.9} exceeds tolerance \
         {POSITION_TOLERANCE_U}"
    );

    // Assertion: UV counts (non-zero) before the zip; UVs within the f16 bound
    assert_eq!(
        baked.uvs.len(),
        native_shape.uvs.len(),
        "{label}: UV counts differ (bake {} vs native {})",
        baked.uvs.len(),
        native_shape.uvs.len()
    );
    assert!(!baked.uvs.is_empty(), "{label}: native body carries no UVs");
    let uv_max_diff = baked
        .uvs
        .iter()
        .zip(&native_shape.uvs)
        .flat_map(|(a, b)| a.iter().zip(b.iter()))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        uv_max_diff <= UV_TOLERANCE,
        "{label}: max UV difference {uv_max_diff:.9} exceeds tolerance {UV_TOLERANCE}"
    );

    // Assertion: weight counts (non-zero) before the zip; weights within f16
    assert_eq!(
        baked.weights.len(),
        native_shape.weights.len(),
        "{label}: weighted vertex counts differ (bake {} vs native {})",
        baked.weights.len(),
        native_shape.weights.len()
    );
    assert!(
        !baked.weights.is_empty(),
        "{label}: native body carries no skin weights"
    );
    let max_weight_diff = baked
        .weights
        .iter()
        .zip(&native_shape.weights)
        .map(|(a, b)| max_abs_diff(a, b))
        .fold(0.0f32, f32::max);
    assert!(
        max_weight_diff <= WEIGHT_TOLERANCE,
        "{label}: max weight difference {max_weight_diff:.9} exceeds tolerance \
         {WEIGHT_TOLERANCE}"
    );

    // Assertion: triangle set exact between bake and native
    assert!(
        !baked.triangles.is_empty(),
        "{label}: native body carries no triangles"
    );
    let triangles_exact = baked.triangles == native_shape.triangles;
    assert!(
        triangles_exact,
        "{label}: triangle set differs between our bake and the native output"
    );

    // Assertion: bone list exact between bake and native
    let bones_equal = baked.bones == native_shape.bones;
    assert!(
        bones_equal,
        "{label}: bone lists differ between our bake and the native output"
    );

    // Negative control: the same baked artifact against the other endpoint
    // must deviate well beyond parity; counts asserted before the zip.
    let opp_doc = read_nif(&opposite_native_path, &label, "opposite native");
    let opp_shape = select_shape(&opp_doc, &source_shape.name, &label, "opposite native");
    assert_eq!(
        baked.positions.len(),
        opp_shape.positions.len(),
        "{label}: opposite-endpoint vertex counts differ (bake {} vs native {})",
        baked.positions.len(),
        opp_shape.positions.len()
    );
    let negative_control_deviation = baked
        .positions
        .iter()
        .zip(&opp_shape.positions)
        .map(|(a, b)| distance(*a, *b))
        .fold(0.0f32, f32::max);
    assert!(
        negative_control_deviation > NEGATIVE_CONTROL_MIN_DEVIATION_U,
        "{label}: wrong-endpoint negative control failed: deviation \
         {negative_control_deviation:.9} is <= {NEGATIVE_CONTROL_MIN_DEVIATION_U}"
    );

    // Per-cell provenance and numbers for the acceptance ledger.
    println!(
        "PASS: {} | native {} | {} vertices | bake-vs-native: max dev {max_deviation:.9} u, \
         p99 {p99_deviation:.9} u, weight max {max_weight_diff:.9}, triangles exact \
         {triangles_exact}, uv max {uv_max_diff:.9}, bones equal {bones_equal}",
        label,
        native_path.display(),
        baked.positions.len(),
    );
    println!(
        "  bake moved source by {bake_moved_source:.9} u | plan blocks {plan_blocks} | \
         neg control (vs {}) dev {negative_control_deviation:.6} u > \
         {NEGATIVE_CONTROL_MIN_DEVIATION_U}",
        opposite_native_path.display(),
    );

    ParityMeasurement {
        family: pair.family,
        label,
        native_path,
        plan_blocks,
        vertices: baked.positions.len(),
        max_deviation,
        p99_deviation,
        max_weight_diff,
        uv_max_diff,
        triangles_exact,
        bones_equal,
        bake_moved_source,
        negative_control_deviation,
    }
}

/// Construct the six same-family cells: two HIMBO presets and the env-named
/// CBBE preset, each at both endpoints. Cross-family pairings are never built.
pub fn build_matrix() -> Vec<MatrixPair> {
    let cbbe_preset = cbbe_preset_name();

    let mut matrix = Vec::with_capacity(EXPECTED_CELLS);
    for &size in &[SliderSize::Small, SliderSize::Big] {
        for preset_name in ["HIMBO Mike", "HIMBO Chonky"] {
            matrix.push(MatrixPair {
                family: BodyFamily::Himbo,
                set_name: "HIMBO Body - Vanilla Nevernude",
                preset_name: preset_name.to_string(),
                size,
            });
        }
        matrix.push(MatrixPair {
            family: BodyFamily::Cbbe,
            set_name: "CBBE Body",
            preset_name: cbbe_preset.clone(),
            size,
        });
    }
    matrix
}

#[test]
fn six_cell_same_family_matrix_sweep() {
    let Some(home) = home_or_skip() else {
        return;
    };

    let matrix = build_matrix();
    assert_eq!(
        matrix.len(),
        EXPECTED_CELLS,
        "matrix must contain exactly {EXPECTED_CELLS} same-family cells \
         (4 HIMBO + 2 CBBE)"
    );

    println!("=== Same-Family Native Parity Matrix ===");
    println!("Home: {}", home.display());
    let passed: Vec<ParityMeasurement> = matrix
        .iter()
        .map(|pair| evaluate_pair(&home, pair))
        .collect();

    // Every cell must evaluate when the fixture home is set; per-family
    // coverage is asserted explicitly.
    assert_eq!(
        passed.len(),
        matrix.len(),
        "every cell must evaluate when SCULPTO_BODYSLIDE_HOME is set"
    );
    for family in [BodyFamily::Himbo, BodyFamily::Cbbe] {
        let expected = matrix.iter().filter(|p| p.family == family).count();
        let evaluated = passed.iter().filter(|m| m.family == family).count();
        assert_eq!(
            evaluated, expected,
            "{family:?} family must cover all {expected} of its cells"
        );
    }

    // Non-identity guard: somewhere the bake must have moved vertices.
    assert!(
        passed
            .iter()
            .any(|m| m.plan_blocks > 0 && m.bake_moved_source > NON_IDENTITY_MIN_DEVIATION_U),
        "no cell proved the bake moved vertices (max|ours - source| > \
         {NON_IDENTITY_MIN_DEVIATION_U} with a non-empty plan); the suite would \
         pass on an identity transform"
    );

    println!("=== Matrix Summary ===");
    println!(
        "Total cells: {} / Evaluated (PASS): {} / Skipped: 0",
        matrix.len(),
        passed.len()
    );
    for m in &passed {
        println!(
            "  [PASS] {} — {} vertices, max dev {:.9} u, p99 {:.9} u, weight max {:.9}, \
             triangles exact {}, uv max {:.9}, bones equal {}, neg control {:.6} u, \
             bake moved source {:.9} u, plan blocks {}, native {}",
            m.label,
            m.vertices,
            m.max_deviation,
            m.p99_deviation,
            m.max_weight_diff,
            m.triangles_exact,
            m.uv_max_diff,
            m.bones_equal,
            m.negative_control_deviation,
            m.bake_moved_source,
            m.plan_blocks,
            m.native_path.display()
        );
    }
}

#[test]
fn himbo_mike_small_endpoint() {
    let Some(home) = home_or_skip() else {
        return;
    };
    let pair = MatrixPair {
        family: BodyFamily::Himbo,
        set_name: "HIMBO Body - Vanilla Nevernude",
        preset_name: "HIMBO Mike".to_string(),
        size: SliderSize::Small,
    };
    let m = evaluate_pair(&home, &pair);
    assert!(m.max_deviation <= POSITION_TOLERANCE_U);
    assert!(m.max_weight_diff <= WEIGHT_TOLERANCE);
    assert!(m.triangles_exact);
    assert!(m.negative_control_deviation > NEGATIVE_CONTROL_MIN_DEVIATION_U);
}

#[test]
fn himbo_mike_big_endpoint() {
    let Some(home) = home_or_skip() else {
        return;
    };
    let pair = MatrixPair {
        family: BodyFamily::Himbo,
        set_name: "HIMBO Body - Vanilla Nevernude",
        preset_name: "HIMBO Mike".to_string(),
        size: SliderSize::Big,
    };
    let m = evaluate_pair(&home, &pair);
    assert!(m.max_deviation <= POSITION_TOLERANCE_U);
    assert!(m.max_weight_diff <= WEIGHT_TOLERANCE);
    assert!(m.triangles_exact);
    assert!(m.negative_control_deviation > NEGATIVE_CONTROL_MIN_DEVIATION_U);
}

#[test]
fn cbbe_small_endpoint() {
    let Some(home) = home_or_skip() else {
        return;
    };
    let pair = MatrixPair {
        family: BodyFamily::Cbbe,
        set_name: "CBBE Body",
        preset_name: cbbe_preset_name(),
        size: SliderSize::Small,
    };
    let m = evaluate_pair(&home, &pair);
    assert!(m.max_deviation <= POSITION_TOLERANCE_U);
    assert!(m.max_weight_diff <= WEIGHT_TOLERANCE);
    assert!(m.triangles_exact);
    assert!(m.negative_control_deviation > NEGATIVE_CONTROL_MIN_DEVIATION_U);
}

#[test]
fn cbbe_big_endpoint() {
    let Some(home) = home_or_skip() else {
        return;
    };
    let pair = MatrixPair {
        family: BodyFamily::Cbbe,
        set_name: "CBBE Body",
        preset_name: cbbe_preset_name(),
        size: SliderSize::Big,
    };
    let m = evaluate_pair(&home, &pair);
    assert!(m.max_deviation <= POSITION_TOLERANCE_U);
    assert!(m.max_weight_diff <= WEIGHT_TOLERANCE);
    assert!(m.triangles_exact);
    assert!(m.negative_control_deviation > NEGATIVE_CONTROL_MIN_DEVIATION_U);
}
