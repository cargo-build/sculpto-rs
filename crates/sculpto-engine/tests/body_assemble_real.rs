//! BODY-01 acceptance, real half: the Bijin prepared body assembles to the
//! recorded canonical numbers.
//!
//! Env-gated on `SCULPTO_ASSEMBLE_FIXTURES` pointing at a directory holding the
//! three pinned private fixtures:
//!
//! - `bijin0-body-feet-yup.obj` — sha256 `af75623c0c127c44…`, 5,825 v/10,488 f,
//!   `o BaseShape` (3,471 v) + `o femalefeet_1` (2,354 v);
//! - `head-before-0.obj` — sha256 `0da86aea8ef7c2e3…`, 996 v/1,706 f, authored
//!   Z-up and turned −90° about source X;
//! - `hands-actual.obj` — sha256 `aba9b19b4a6f38a9…`, 1,780 v/2,912 f, both
//!   hands (890 v each), seated by the similarity solve.
//!
//! The asserted seal rows are the BLD-13 measurements: neck 26↔29 → 54 bridge
//! triangles with both ring counts at 0, each wrist 10↔10 → 20 bridge triangles
//! at seat scale 64.0–65.8 and RMS ≤ 0.25, and the whole output at 8,601 v.
//!
//! A second env gate, `SCULPTO_ASSEMBLE_MIKE_FIXTURES`, covers the male body: a
//! directory with `body.obj`, `feet.obj`, `hands.obj` exported from the pinned
//! Mike NIFs (`sculpto export-obj`, NIF units) and `head.obj` from the
//! authoring-target export. The head's authoring frame is mirrored and 71.4286×
//! smaller, so the manifest declares the signed scale; the neck then welds
//! 26↔27 with 51 triangles (BODY-01 0030 §4).

use std::path::{Path, PathBuf};

use sculpto_engine::dispatch;
use serde_json::{Value, json};

fn fixtures() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_ASSEMBLE_FIXTURES")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn work_dir() -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sculpto-body-assemble-real-{}", std::process::id()));
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
fn bijin_assembly_reproduces_the_canonical_numbers() {
    let Some(root) = fixtures() else {
        eprintln!(
            "SKIP bijin_assembly_reproduces_the_canonical_numbers: SCULPTO_ASSEMBLE_FIXTURES is unset"
        );
        return;
    };
    let body = root.join("bijin0-body-feet-yup.obj");
    let head = root.join("head-before-0.obj");
    let hands = root.join("hands-actual.obj");
    let dir = work_dir();
    let manifest = dir.join("bijin.json");
    std::fs::write(
        &manifest,
        json!({
            "version": 1,
            "parts": [
                { "path": body, "role": "body", "object": "BaseShape" },
                { "path": body, "role": "feet", "object": "femalefeet_1" },
                { "path": head, "role": "head", "rotate": { "axis": "x", "degrees": -90.0 } },
                { "path": hands, "role": "hands" },
            ],
            "joints": [
                { "part": "head", "socket": "body" },
                { "part": "hands", "socket": "body", "seat": true },
            ],
        })
        .to_string(),
    )
    .expect("manifest written");
    let out = dir.join("bijin-assembled.obj");
    let report_path = dir.join("bijin-assembled.json");
    let result = dispatch(
        "body.assemble",
        &json!({
            "manifest": manifest.display().to_string(),
            "out": out.display().to_string(),
            "report": report_path.display().to_string(),
        }),
    )
    .expect("the Bijin prepared body assembles");

    // Four declared parts, torso first, with the recorded vertex counts.
    let parts = result["parts"].as_array().expect("part rows");
    let counts: Vec<u64> = parts
        .iter()
        .map(|part| part["vertices"].as_u64().expect("vertex count"))
        .collect();
    assert_eq!(counts, vec![3_471, 2_354, 996, 1_780]);
    assert_eq!(parts[0]["vertex_offset"], 0);
    assert_eq!(parts[1]["vertex_offset"], 3_471);
    assert_eq!(parts[2]["vertex_offset"], 5_825);
    assert_eq!(parts[3]["vertex_offset"], 6_821);
    assert_eq!(result["assembled"]["vertices"], 8_601);

    // The neck and both wrists: one declared joint expands to three seal rows.
    let joints = result["joints"].as_array().expect("joint rows");
    assert_eq!(joints.len(), 2);
    assert_eq!(joints[0]["part"], "head");
    assert_eq!(
        joints[0]["shells"].as_array().expect("head shells").len(),
        1
    );
    assert_eq!(joints[1]["part"], "hands");
    assert_eq!(
        joints[1]["shells"].as_array().expect("hand shells").len(),
        2,
        "one hands part expands to one seal request per hand shell"
    );

    let rows = result["seal"]["joints"].as_array().expect("seal rows");
    assert_eq!(rows.len(), 3);
    let neck = &rows[0];
    assert_eq!(neck["part"], "head");
    assert_eq!(neck["socket"], "torso");
    assert_eq!(neck["bridge_triangles"], 54);
    assert_eq!(neck["boundary_ring_before"], 26);
    assert_eq!(neck["boundary_ring_after"], 0);
    assert_eq!(neck["boundary_arc_before"], 28);
    assert_eq!(neck["boundary_arc_after"], 0);
    assert_eq!(neck["non_manifold_after"], 0);
    for wrist in &rows[1..] {
        assert_eq!(wrist["bridge_triangles"], 20);
        assert_eq!(wrist["boundary_ring_before"], 10);
        assert_eq!(wrist["boundary_ring_after"], 0);
        assert_eq!(wrist["boundary_arc_after"], 0);
        assert_eq!(wrist["seat"]["head_drop"], 0.0);
        let scale = wrist["seat"]["scale"].as_f64().expect("seat scale");
        assert!((64.0..=65.8).contains(&scale), "seat scale {scale}");
        let rms = wrist["seat"]["rms"].as_f64().expect("seat rms");
        assert!(rms <= 0.25, "seat rms {rms}");
    }

    // The bridge is real: the mesh gains exactly the bridged triangles and
    // loses no vertex.
    assert_eq!(result["output"]["vertices"], 8_601);
    assert_eq!(result["output"]["triangles"], 15_200);
    assert_eq!(vertices_of(&out), 8_601);
    assert_eq!(count(&out, "f "), 15_200);

    // Same input, same digests: the manifest and the placement are the recipe.
    assert_eq!(result["seal"]["input_digest"], "c4970a1d28ad3c32");
    assert_eq!(result["seal"]["output_digest"], "640765dcca9f77ee");

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("report file"))
            .expect("report JSON");
    assert_eq!(written["output"], result["output"]);
    assert_eq!(written["seal"]["joints"][0]["bridge_triangles"], 54);
}

fn vertices_of(path: &Path) -> usize {
    count(path, "v ")
}

/// A non-zero baked seat is expressible, and today it collides with the seal's
/// collar extraction: `extract_arc` trims the part rim against
/// `authored_positions` (`joint_seal.rs:2723-2726`), the snapshot taken before
/// its own `--head-drop`, so a *placed* head is measured at its placed distance.
/// Measured on the Bijin fixture: drops 0 and 0.25 seal, 0.5 and above refuse
/// `seal_arc_too_short`. This pins the boundary for BLD-13 instead of hiding it.
#[test]
fn a_baked_seat_past_the_collar_band_refuses_by_the_seal_code() {
    let Some(root) = fixtures() else {
        eprintln!(
            "SKIP a_baked_seat_past_the_collar_band_refuses_by_the_seal_code: SCULPTO_ASSEMBLE_FIXTURES is unset"
        );
        return;
    };
    let dir = work_dir();
    let manifest = json!({
        "version": 1,
        "parts": [
            { "path": root.join("bijin0-body-feet-yup.obj"), "role": "body", "object": "BaseShape" },
            { "path": root.join("bijin0-body-feet-yup.obj"), "role": "feet", "object": "femalefeet_1" },
            { "path": root.join("head-before-0.obj"), "role": "head", "drop": 2.0,
              "rotate": { "axis": "x", "degrees": -90.0 } },
        ],
        "joints": [{ "part": "head", "socket": "body" }],
    });
    let path = dir.join("bijin-drop2.json");
    std::fs::write(&path, manifest.to_string()).expect("manifest written");
    let error = dispatch(
        "body.assemble",
        &json!({
            "manifest": path.display().to_string(),
            "out": dir.join("bijin-drop2.obj").display().to_string(),
        }),
    )
    .expect_err("the place-then-seal path refuses a seat the seal cannot trim");
    assert_eq!(error.code(), "seal_arc_too_short", "{}", error.message());
}

#[test]
fn mike_neck_seals_through_the_declared_mirror_scale() {
    let Some(root) = std::env::var_os("SCULPTO_ASSEMBLE_MIKE_FIXTURES")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    else {
        eprintln!(
            "SKIP mike_neck_seals_through_the_declared_mirror_scale: SCULPTO_ASSEMBLE_MIKE_FIXTURES is unset"
        );
        return;
    };
    let dir = work_dir();
    let manifest = dir.join("mike.json");
    std::fs::write(
        &manifest,
        json!({
            "version": 1,
            "parts": [
                { "path": root.join("body.obj"), "role": "body" },
                { "path": root.join("feet.obj"), "role": "feet" },
                // The authoring-target head frame is mirrored in x and z and
                // 71.4286x smaller; the mirror is declared, never inferred.
                { "path": root.join("head.obj"), "role": "head",
                  "scale": [-71.4286, -71.4286, 71.4286] },
                { "path": root.join("hands.obj"), "role": "hands" },
            ],
            // The feet pair per shell: the soles against the leg sockets.
            // The manifest keeps the anatomical leg names (`foot R/L`);
            // pre-seal the classifier hands a cuff the plain name first, so
            // the report may print the resolved `foot R 2` — the weld rows
            // are asserted by semantics, not by the occurrence counter.
            "joints": [
                { "part": "head", "socket": "body" },
                { "part": "hands", "socket": "body", "seat": true },
                { "part": "feet", "socket": "body",
                  "shells": [ { "part": "sole R", "socket": "foot R" },
                              { "part": "sole L", "socket": "foot L" } ] },
            ],
        })
        .to_string(),
    )
    .expect("manifest written");
    let out = dir.join("mike-assembled.obj");
    let result = dispatch(
        "body.assemble",
        &json!({
            "manifest": manifest.display().to_string(),
            "out": out.display().to_string(),
        }),
    )
    .expect("the Mike body assembles");

    assert_eq!(result["assembled"]["vertices"], 13_380);
    assert_eq!(
        result["parts"][2]["placement"]["scale"],
        json!([-71.4286, -71.4286, 71.4286])
    );
    assert_eq!(
        result["parts"][2]["placement"]["mirrored"], false,
        "two negative lanes are a rotation, not a mirror: the winding already faces outward"
    );
    let rows = result["seal"]["joints"].as_array().expect("seal rows");
    assert_eq!(rows.len(), 5, "neck + two wrists + two ankles");
    let neck = &rows[0];
    assert_eq!(neck["bridge_triangles"], 51);
    assert_eq!(neck["boundary_ring_before"], 26);
    assert_eq!(neck["boundary_ring_after"], 0);
    assert_eq!(neck["boundary_arc_before"], 25);
    assert_eq!(neck["boundary_arc_after"], 0);
    assert_eq!(neck["non_manifold_after"], 0);
    let dihedral = neck["smoothing"]["max_dihedral_after_deg"]
        .as_f64()
        .expect("dihedral");
    assert!(dihedral < 30.0, "seam dihedral {dihedral} deg");
    for wrist in &rows[1..3] {
        assert_eq!(wrist["correspondence"], "authored");
        assert_eq!(wrist["bridge_triangles"], 0);
        assert_eq!(wrist["boundary_ring_after"], 0);
        assert_eq!(wrist["boundary_arc_after"], 0);
    }
    // The HIMBO ankles are authored as one coincident ring (measured snaps
    // 4.8e-7..1.21e-3 u over ~1.93 u steps): they weld with no ribbon, no
    // smoothing move and no non-manifold edge — never the zero-span sliver
    // the bridge would build here.
    for ankle in &rows[3..5] {
        assert_eq!(ankle["correspondence"], "coincident_weld");
        assert_eq!(ankle["bridge_triangles"], 0);
        assert_eq!(ankle["boundary_ring_before"], 10);
        assert_eq!(ankle["boundary_ring_after"], 0);
        assert_eq!(ankle["boundary_arc_after"], 0);
        assert_eq!(ankle["non_manifold_after"], 0);
        assert!(ankle["weld_merged"].as_u64().expect("weld merged") >= 10);
        let shift = ankle["weld_max_shift"].as_f64().expect("weld shift");
        assert!(shift <= 0.0013, "weld snap {shift}");
        assert!(shift > 0.0, "a real snap is measured");
    }
    // The ankle welds add no bridge; the neck adds 51 and the head repair 59.
    assert_eq!(result["output"]["triangles"], 25_256 + 51 + 59);
}
