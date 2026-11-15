//! BODY-01 acceptance: the durable Emily fixture assembles to the recorded
//! accepted numbers.
//!
//! Env-gated on `SCULPTO_ASSEMBLE_EMILY_FIXTURES` pointing at the private
//! fixture directory `~/Models/sculpto-emily-assembly-20261115/`, which holds
//! `manifest.json`, `parts/{emily-body,feet,head,hands}.obj` and `SHA256SUMS`:
//!
//! - `parts/emily-body.obj` — the gallery 3BA "Natural & Realistic Bodies -
//!   Emily" bake, 18,436 v/35,904 f;
//! - `parts/feet.obj` / `parts/hands.obj` — the shared 3BA shells used by the
//!   accepted assembly (8,876 v / 6,374 v);
//! - `parts/head.obj` — the accepted authoring head, rotated −90° about source
//!   X and translated `[0, -2.5868, 3.5498]` by the manifest.
//!
//! The manifest declares the two ankle joints through explicit shell pairs
//! (`sole R:leg R`, `sole L:leg L`). Pre-seal, the classifier gives `leg R` to
//! a hand cuff, so the body's own leg resolves as the occurrence-counter twin
//! `leg R 2`; the manifest keeps the anatomical name and the report prints the
//! resolved socket. The asserted rows are the accepted Emily measurements
//! (BODY-01, 2026-11-15): output 34,682 v/65,808 t, neck 26↔29 → 54 bridge
//! triangles with both rims at 0, both wrists welded `authored` at identity
//! seat (bridge 0 — the seated-bridge twist fix), and both ankles 20↔20 → 40
//! bridge triangles each with rings, arcs and non-manifold edges all at 0.

use std::path::{Path, PathBuf};

use sculpto_engine::dispatch;
use serde_json::{Value, json};

fn fixtures() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_ASSEMBLE_EMILY_FIXTURES")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sculpto-body-assemble-emily-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn count(path: &Path, prefix: &str) -> usize {
    std::fs::read_to_string(path)
        .expect("output OBJ")
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

#[test]
fn emily_assembly_reproduces_the_accepted_numbers() {
    let Some(root) = fixtures() else {
        eprintln!(
            "SKIP emily_assembly_reproduces_the_accepted_numbers: SCULPTO_ASSEMBLE_EMILY_FIXTURES is unset"
        );
        return;
    };
    let dir = work_dir();
    let out = dir.join("emily-assembled.obj");
    let report_path = dir.join("emily-assembled.json");
    let result = dispatch(
        "body.assemble",
        &json!({
            "manifest": root.join("manifest.json").display().to_string(),
            "out": out.display().to_string(),
            "report": report_path.display().to_string(),
        }),
    )
    .expect("the Emily fixture assembles");

    // Four declared parts, torso first, with the recorded vertex counts and
    // offsets — no part dropped, every part digested.
    let parts = result["parts"].as_array().expect("part rows");
    assert_eq!(parts.len(), 4);
    let roles: Vec<&str> = parts
        .iter()
        .map(|part| part["role"].as_str().expect("part role"))
        .collect();
    assert_eq!(roles, vec!["body", "feet", "head", "hands"]);
    let counts: Vec<u64> = parts
        .iter()
        .map(|part| part["vertices"].as_u64().expect("vertex count"))
        .collect();
    assert_eq!(counts, vec![18_436, 8_876, 997, 6_374]);
    let offsets: Vec<u64> = parts
        .iter()
        .map(|part| part["vertex_offset"].as_u64().expect("vertex offset"))
        .collect();
    assert_eq!(offsets, vec![0, 18_436, 27_312, 28_309]);
    for part in parts {
        let digest = part["sha256"].as_str().expect("part digest");
        assert!(!digest.is_empty(), "part {} carries a digest", part["role"]);
    }
    assert_eq!(result["assembled"]["vertices"], 34_683);
    assert_eq!(result["assembled"]["triangles"], 65_730);

    // One neck row, two wrist rows, two ankle rows: the hands joint expands to
    // one seal request per hand shell and the feet joint to one per declared
    // ankle shell pair.
    let rows = result["seal"]["joints"].as_array().expect("seal rows");
    assert_eq!(rows.len(), 5);

    let neck = &rows[0];
    assert_eq!(neck["part"], "head");
    assert_eq!(neck["bridge_triangles"], 54);
    assert_eq!(neck["boundary_ring_before"], 26);
    assert_eq!(neck["boundary_ring_after"], 0);
    assert_eq!(neck["boundary_arc_before"], 28);
    assert_eq!(neck["boundary_arc_after"], 0);
    assert_eq!(neck["non_manifold_after"], 0);

    // Both wrists: the authored-coincident cuffs weld (bridge 0) at the
    // identity seat — the seated-bridge twist fix keeps the hands placed as
    // authored instead of flipping them.
    for wrist in &rows[1..3] {
        assert_eq!(wrist["correspondence"], "authored");
        assert_eq!(wrist["bridge_triangles"], 0);
        assert_eq!(wrist["boundary_ring_before"], 20);
        assert_eq!(wrist["boundary_ring_after"], 0);
        assert_eq!(wrist["boundary_arc_after"], 0);
        assert_eq!(wrist["non_manifold_after"], 0);
        assert_eq!(wrist["seat"]["head_drop"], 0.0);
        assert_eq!(wrist["seat"]["scale"], 1.0);
        let rms = wrist["seat"]["rms"].as_f64().expect("seat rms");
        assert!(rms <= 0.25, "seat rms {rms}");
    }

    // Both ankles: the declared shell pairs bridge 20↔20 rims with 40
    // triangles each and leave no boundary or non-manifold edges. The report
    // resolves the anatomical `leg R`/`leg L` to the pre-seal occurrence
    // twins `leg R 2`/`leg L 2` (a hand cuff holds the plain name until the
    // wrist weld frees it).
    assert_eq!(rows[3]["part"], "sole R");
    assert_eq!(rows[3]["socket"], "leg R 2");
    assert_eq!(rows[4]["part"], "sole L");
    assert_eq!(rows[4]["socket"], "leg L 2");
    for ankle in &rows[3..] {
        assert_eq!(ankle["correspondence"], "monotone_dp");
        assert_eq!(ankle["bridge_triangles"], 40);
        assert_eq!(ankle["boundary_ring_before"], 20);
        assert_eq!(ankle["boundary_ring_after"], 0);
        assert_eq!(ankle["boundary_arc_before"], 20);
        assert_eq!(ankle["boundary_arc_after"], 0);
        assert_eq!(ankle["non_manifold_after"], 0);
    }

    // The bridge triangles are real: the neck adds 54 and each ankle 40, and
    // the head repair adds its patches; the recipe digests are reported.
    assert_eq!(result["output"]["vertices"], 34_686);
    assert_eq!(result["output"]["triangles"], 65_882);
    assert_eq!(vertices_of(&out), 34_686);
    assert_eq!(count(&out, "f "), 65_882);
    // The delivered head carries no puncture: the pre-seal fill closed its own
    // small loops and the post-seal repair closed the skull slit.
    let holes = &result["head_holes"];
    assert!(!holes["filled"].as_array().expect("filled").is_empty());
    let repair = &result["head_seal_repair"];
    assert!(
        repair["filled"].as_u64().expect("repair count") >= 2,
        "the skull slit breaks into at least two punctures"
    );
    assert_eq!(repair["left_open"].as_array().expect("left open").len(), 0);
    let input_digest = result["seal"]["input_digest"]
        .as_str()
        .expect("input digest");
    let output_digest = result["seal"]["output_digest"]
        .as_str()
        .expect("output digest");
    assert_eq!(input_digest.len(), 16, "input digest {input_digest}");
    assert_eq!(output_digest.len(), 16, "output digest {output_digest}");

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("report file"))
            .expect("report JSON");
    assert_eq!(written["output"], result["output"]);
    assert_eq!(written["seal"]["joints"][0]["bridge_triangles"], 54);
}

fn vertices_of(path: &Path) -> usize {
    count(path, "v ")
}
