//! BODY-01 acceptance, portable half: `body.assemble` places and concatenates
//! declared parts with a stable vertex order, seals the declared joints through
//! the shared joint-seal path, and refuses a manifest the assembly cannot honour
//! by a named code. The real Bijin numbers live in the env-gated sibling
//! `body_assemble_real.rs`.

use std::path::{Path, PathBuf};

use sculpto_engine::{ErrorKind, dispatch};
use serde_json::{Value, json};

const SIDES: usize = 8;

/// Writes OBJ text in the display frame (`[x, z, -y]`) from a Z-up design, the
/// convention `read_obj_surface` converts back to the source frame.
#[derive(Default)]
struct Builder {
    text: String,
    vertices: usize,
}

impl Builder {
    fn object(&mut self, name: &str) -> &mut Self {
        self.text.push_str(&format!("o {name}\n"));
        self
    }

    /// Append one octagon ring; returns its 1-based vertex ids.
    fn ring(&mut self, radius: f64, z: f64) -> Vec<usize> {
        let mut ids = Vec::with_capacity(SIDES);
        for k in 0..SIDES {
            let angle = k as f64 * std::f64::consts::FRAC_PI_4;
            let (x, y) = (radius * angle.cos(), radius * angle.sin());
            self.vertices += 1;
            self.text.push_str(&format!("v {x:.6} {z:.6} {:.6}\n", -y));
            ids.push(self.vertices);
        }
        ids
    }

    fn tube(&mut self, bottom: &[usize], top: &[usize]) -> &mut Self {
        for k in 0..SIDES {
            let next = (k + 1) % SIDES;
            let (b0, b1, t0, t1) = (bottom[k], bottom[next], top[k], top[next]);
            self.text
                .push_str(&format!("f {b0} {b1} {t1}\nf {b0} {t1} {t0}\n"));
        }
        self
    }

    fn cap(&mut self, ring: &[usize]) -> &mut Self {
        for k in 1..SIDES - 1 {
            self.text
                .push_str(&format!("f {} {} {}\n", ring[0], ring[k], ring[k + 1]));
        }
        self
    }

    fn into_text(self) -> String {
        self.text
    }
}

/// The torso: an open tube from the ankle ring (z 4) to the neck ring (z 11).
fn torso_text() -> String {
    let mut builder = Builder::default();
    builder.object("BaseShape");
    let bottom = builder.ring(2.0, 4.0);
    let top = builder.ring(2.0, 11.0);
    builder.tube(&bottom, &top);
    builder.into_text()
}

/// The head: a collar ring 0.8 above the neck ring, a second ring and a cap.
fn head_text(separation: f64) -> String {
    let mut builder = Builder::default();
    let collar = builder.ring(2.0, 11.0 + separation);
    let crown = builder.ring(1.4, 13.6 + separation);
    builder.tube(&collar, &crown);
    builder.cap(&crown);
    builder.into_text()
}

/// Two closed feet, each large enough for the classifier to name.
fn feet_text() -> String {
    let mut builder = Builder::default();
    for radius in [1.0, 0.8] {
        let bottom = builder.ring(radius, 0.0);
        let top = builder.ring(radius, 1.5);
        builder.tube(&bottom, &top);
        builder.cap(&top);
    }
    builder.into_text()
}

/// Both hands in one file: a tube per wrist, each cuff a 1.9-radius ring
/// centred on the 2.0 torso ring it welds to (0.1 away: near enough for the
/// classifier's coincident-onto-torso rule, never exactly coincident, like
/// a seat-transformed cuff). The far end is capped, so each hand owns one
/// closed boundary cycle.
fn hands_text() -> String {
    let mut builder = Builder::default();
    builder.object("HandBelow");
    let cuff = builder.ring(1.9, 4.0);
    let outer = builder.ring(1.9, 0.0);
    builder.tube(&cuff, &outer);
    builder.cap(&outer);
    builder.object("HandAbove");
    let cuff = builder.ring(1.9, 11.0);
    let outer = builder.ring(1.9, 15.0);
    builder.tube(&cuff, &outer);
    builder.cap(&outer);
    builder.into_text()
}

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sculpto-body-assemble-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("fixture written");
    path.display().to_string()
}

fn assemble(
    manifest: Value,
    out: &Path,
    report: &Path,
) -> Result<Value, sculpto_engine::CommandError> {
    let manifest_path = out.with_extension("manifest.json");
    std::fs::write(&manifest_path, manifest.to_string()).expect("manifest written");
    dispatch(
        "body.assemble",
        &json!({
            "manifest": manifest_path.display().to_string(),
            "out": out.display().to_string(),
            "report": report.display().to_string(),
        }),
    )
}

fn part(path: &str, role: &str) -> Value {
    json!({ "path": path, "role": role })
}

fn vertices_of(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .expect("output OBJ")
        .lines()
        .filter(|line| line.starts_with("v "))
        .count()
}

/// The assembled report names the hand shells by role: the pre-seat
/// classifier calls the coincident cuffs `leg`/`leg 2` (BODY-01 00B0), and
/// the report rows must show the declared role's labels instead, while the
/// seal keeps the classifier names as its addressing keys, carries its own
/// `handedness` verdict untouched, and welds the same numbers.
#[test]
fn the_report_names_hand_shells_by_role_not_by_the_classifier() {
    let dir = work_dir("labels");
    let body = write(&dir, "body.obj", &torso_text());
    let hands = write(&dir, "hands.obj", &hands_text());
    let out = dir.join("hands-assembled.obj");
    let report = dir.join("hands-assembled.json");
    let result = assemble(
        json!({
            "version": 1,
            "parts": [part(&body, "body"), part(&hands, "hands")],
            "joints": [{ "part": "hands", "socket": "body", "seat": true }],
            "seal": { "smooth": false },
        }),
        &out,
        &report,
    )
    .expect("the hands assemble, seat and weld");

    // The joint row: role name at the top, role-derived labels per shell.
    let joint = &result["joints"][0];
    assert_eq!(joint["part"], "hands");
    assert_eq!(joint["socket"], "body");
    let shells = joint["shells"].as_array().expect("hand shells");
    assert_eq!(
        shells
            .iter()
            .map(|shell| shell["part"].as_str().expect("part label"))
            .collect::<Vec<_>>(),
        vec!["hand 1", "hand 2"],
        "the shells carry role labels, not the classifier's `leg` names"
    );
    assert_eq!(shells[0]["socket"], "torso");

    // The seal rows: same labels, the classifier socket name, the seal's own
    // handedness verdict, and the 8↔8 ring weld unchanged.
    let rows = result["seal"]["joints"].as_array().expect("seal rows");
    assert_eq!(rows.len(), 2);
    for (index, row) in rows.iter().enumerate() {
        let label = format!("hand {}", index + 1);
        assert_eq!(row["part"], label);
        assert_eq!(row["joint"], format!("{label}:torso"));
        assert_eq!(row["socket"], "torso");
        let handedness = row["handedness"].as_str().expect("handedness row");
        // The synthetic cuffs sit on their own socket cycles, so the authored
        // pin decides the pairing before the cost search; the marker must name
        // that evidence and survive the role relabel.
        assert_eq!(
            handedness, "decisive: measured coincidence pins the authored pairing",
            "a coincident cuff is pinned by measurement, not by cost"
        );
        // The authored-coincident cuff keeps its placement and welds onto the
        // socket ring: no ribbon is built, the two 8-vertex loops become one.
        assert_eq!(row["correspondence"], "authored");
        assert_eq!(row["bridge_triangles"], 0, "a weld is not a bridge");
        assert_eq!(row["boundary_ring_before"], 8);
        assert_eq!(row["boundary_ring_after"], 0);
    }

    // The welds are the only added geometry: two 8-vertex rings, 16
    // triangles each, no vertex gained or lost.
    assert_eq!(result["assembled"]["vertices"], 48);
    assert_eq!(result["assembled"]["triangles"], 60);
    assert_eq!(result["output"]["vertices"], 48);
    assert_eq!(
        result["output"]["triangles"], 60,
        "the weld adds no ribbon triangles to the assembled 60"
    );
    assert_eq!(vertices_of(&out), 48);
}

#[test]
fn assemble_places_concatenates_and_seals_the_neck() {
    let dir = work_dir("seal");
    let body = write(&dir, "body.obj", &torso_text());
    let head = write(&dir, "head.obj", &head_text(0.8));
    let feet = write(&dir, "feet.obj", &feet_text());
    let out = dir.join("assembled.obj");
    let report = dir.join("assembled.json");
    let result = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                part(&feet, "feet"),
                part(&head, "head"),
            ],
            "joints": [{ "part": "head", "socket": "body" }],
            "seal": { "smooth": false },
        }),
        &out,
        &report,
    )
    .expect("the synthetic body assembles and seals");

    let parts = result["parts"].as_array().expect("part rows");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0]["role"], "body");
    assert_eq!(parts[0]["vertex_offset"], 0);
    assert_eq!(parts[1]["role"], "feet");
    assert_eq!(parts[1]["vertex_offset"], parts[0]["vertices"]);
    assert_eq!(parts[2]["role"], "head");
    let head_offset =
        parts[0]["vertices"].as_u64().unwrap() + parts[1]["vertices"].as_u64().unwrap();
    assert_eq!(parts[2]["vertex_offset"].as_u64(), Some(head_offset));
    assert_eq!(result["assembled"]["vertices"], 16 + 32 + 16);
    assert!(
        result["parts"][0]["sha256"]
            .as_str()
            .expect("digest")
            .starts_with("sha256:"),
        "each part carries its file hash"
    );

    // The declared joint resolves to the seal's own shell names, not the roles.
    let joint = &result["joints"][0];
    assert_eq!(joint["part"], "head");
    assert_eq!(joint["seat"], false);
    assert_eq!(joint["shells"][0]["socket"], "torso");

    let sealed = &result["seal"]["joints"][0];
    assert_eq!(sealed["bridge_triangles"], 16);
    assert_eq!(sealed["boundary_ring_after"], 0);
    assert_eq!(sealed["boundary_arc_after"], 0);
    assert_eq!(sealed["non_manifold_after"], 0);
    let assembled_triangles = result["assembled"]["triangles"]
        .as_u64()
        .expect("triangles");
    assert_eq!(
        result["output"]["triangles"],
        assembled_triangles + 16,
        "the bridge adds exactly the 16 closing triangles"
    );

    // The file, the report and the envelope agree.
    assert_eq!(
        vertices_of(&out),
        result["output"]["vertices"].as_u64().unwrap() as usize
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report file"))
            .expect("report JSON");
    assert_eq!(written["seal"]["joints"][0]["bridge_triangles"], 16);
    assert_eq!(written["output"], result["output"]);
}

#[test]
fn an_object_selection_reads_one_block_and_compacts_it() {
    let dir = work_dir("object");
    let mut text = torso_text();
    // A second block the manifest does not select: the body keeps 16 vertices.
    text.push_str("o extra\n");
    for k in 0..4 {
        text.push_str(&format!("v {k}.0 1.0 0.0\n"));
    }
    text.push_str("f 17 18 19\nf 17 19 20\n");
    let body = write(&dir, "two-blocks.obj", &text);
    let out = dir.join("body-only.obj");
    let report = dir.join("body-only.json");
    let result = assemble(
        json!({
            "version": 1,
            "parts": [{ "path": body, "role": "body", "object": "BaseShape" }],
            "joints": [],
        }),
        &out,
        &report,
    )
    .expect("the block assembles");
    assert_eq!(result["assembled"]["vertices"], 16);
    assert_eq!(result["parts"][0]["object"], "BaseShape");
    assert_eq!(vertices_of(&out), 16);

    let error = assemble(
        json!({
            "version": 1,
            "parts": [{ "path": body, "role": "body", "object": "Absent" }],
        }),
        &out,
        &report,
    )
    .expect_err("an unknown block is refused");
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.code(), "assemble_object_missing");
    assert!(error.message().contains("BaseShape"), "{}", error.message());
}

#[test]
fn an_unplaced_part_cannot_be_sealed_silently() {
    let dir = work_dir("unplaced");
    let body = write(&dir, "body.obj", &torso_text());
    let head = write(&dir, "head.obj", &head_text(0.8));
    let out = dir.join("unplaced.obj");
    let report = dir.join("unplaced.json");
    let error = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                { "path": head, "role": "head", "translate": [0.0, 0.0, 50.0] },
            ],
            "joints": [{ "part": "head", "socket": "body" }],
            "seal": { "smooth": false },
        }),
        &out,
        &report,
    )
    .expect_err("a part off the socket band is refused");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "assemble_part_unplaced");
    assert!(error.message().contains("drop"), "{}", error.message());
}

#[test]
fn a_declared_mirror_rewinds_the_part_and_is_reported() {
    let dir = work_dir("mirror");
    let body = write(&dir, "body.obj", &torso_text());
    let head = write(&dir, "head.obj", &head_text(0.8));
    let plain = dir.join("plain.obj");
    assemble(
        json!({
            "version": 1,
            "parts": [part(&body, "body"), part(&head, "head")],
        }),
        &plain,
        &dir.join("plain.json"),
    )
    .expect("the unmirrored assembly");
    let mirrored = dir.join("mirrored.obj");
    let result = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                { "path": head, "role": "head", "scale": [-1.0, 1.0, 1.0] },
            ],
        }),
        &mirrored,
        &dir.join("mirrored.json"),
    )
    .expect("a declared mirror assembles");
    assert_eq!(result["parts"][0]["placement"]["mirrored"], false);
    assert_eq!(result["parts"][1]["placement"]["mirrored"], true);
    assert_eq!(
        result["parts"][1]["placement"]["scale"],
        json!([-1.0, 1.0, 1.0])
    );

    // The same head faces, rewound: corner 2 and 3 swap, no id changes.
    let faces = |path: &Path| -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("output OBJ")
            .lines()
            .filter(|line| line.starts_with("f "))
            .map(str::to_string)
            .collect()
    };
    let plain_faces = faces(&plain);
    let mirrored_faces = faces(&mirrored);
    assert_eq!(plain_faces.len(), mirrored_faces.len());
    let swap = |line: &str| -> String {
        let ids: Vec<&str> = line.split_whitespace().skip(1).collect();
        format!("f {} {} {}", ids[0], ids[2], ids[1])
    };
    let head_faces = result["parts"][1]["triangles"]
        .as_u64()
        .expect("head faces") as usize;
    let split = plain_faces.len() - head_faces;
    assert_eq!(
        &plain_faces[..split],
        &mirrored_faces[..split],
        "the body is untouched"
    );
    for (plain, mirrored) in plain_faces[split..].iter().zip(&mirrored_faces[split..]) {
        assert_eq!(swap(plain), *mirrored, "a mirrored face is rewound");
    }
}

#[test]
fn the_seat_is_signed_and_drop_is_downward_only() {
    let dir = work_dir("seat");
    let body = write(&dir, "body.obj", &torso_text());
    let head = write(&dir, "head.obj", &head_text(0.8));
    let out = dir.join("seat.obj");
    let report = dir.join("seat.json");

    let up = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                { "path": head, "role": "head", "translate": [0.0, 0.0, 3.0] },
            ],
        }),
        &out,
        &report,
    )
    .expect("an upward seat is expressible");
    assert_eq!(
        up["parts"][1]["placement"]["translation"],
        json!([0.0, 0.0, 3.0])
    );

    let down = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                { "path": head, "role": "head", "drop": 2.0 },
            ],
        }),
        &out,
        &report,
    )
    .expect("a downward drop is expressible");
    assert_eq!(
        down["parts"][1]["placement"]["translation"],
        json!([0.0, 0.0, -2.0])
    );

    for (manifest, code) in [
        (
            json!({
                "version": 1,
                "parts": [
                    part(&body, "body"),
                    { "path": head, "role": "head", "drop": -1.0 },
                ],
            }),
            "assemble_part_placement",
        ),
        (
            json!({
                "version": 1,
                "parts": [
                    part(&body, "body"),
                    { "path": head, "role": "head", "scale": [1.0, 0.0, 1.0] },
                ],
            }),
            "assemble_part_scale",
        ),
    ] {
        let error = assemble(manifest, &out, &report).expect_err("refused");
        assert_eq!(error.code(), code, "{}", error.message());
    }
}

#[test]
fn a_non_obj_part_is_refused_before_it_is_read() {
    let dir = work_dir("format");
    let body = write(&dir, "body.obj", &torso_text());
    let error = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                part("/no/such/feet.nif", "feet"),
            ],
        }),
        &dir.join("format.obj"),
        &dir.join("format.json"),
    )
    .expect_err("a NIF part is refused");
    assert_eq!(error.code(), "assemble_part_format");
    assert!(
        error.message().contains("export-obj"),
        "{}",
        error.message()
    );
}

#[test]
fn manifest_defects_are_named_before_any_part_is_read() {
    let dir = work_dir("refusals");
    let body = write(&dir, "body.obj", &torso_text());
    let out = dir.join("refused.obj");
    let report = dir.join("refused.json");
    let cases: Vec<(Value, &str)> = vec![
        (
            json!({ "version": 2, "parts": [part(&body, "body")] }),
            "assemble_version",
        ),
        (json!({ "version": 1, "parts": [] }), "assemble_parts_empty"),
        (
            json!({ "version": 1, "parts": [part(&body, "head"), part(&body, "body")] }),
            "assemble_body_not_first",
        ),
        (
            json!({ "version": 1, "parts": [part(&body, "head")] }),
            "assemble_body_role",
        ),
        (
            json!({
                "version": 1,
                "parts": [part(&body, "body"), part(&body, "head"), part(&body, "head")],
            }),
            "assemble_role_duplicate",
        ),
        (
            json!({
                "version": 1,
                "parts": [part(&body, "body"), part(&body, "feet")],
                "joints": [{ "part": "feet", "socket": "body" }],
            }),
            "assemble_feet_joint",
        ),
        (
            json!({
                "version": 1,
                "parts": [part(&body, "body"), part(&body, "head")],
                "joints": [{ "part": "head", "socket": "body", "seat": true }],
            }),
            "assemble_seat_role",
        ),
        (
            json!({
                "version": 1,
                "parts": [part(&body, "body")],
                "joints": [{ "part": "head", "socket": "body" }],
            }),
            "assemble_joint_role",
        ),
        (
            json!({
                "version": 1,
                "parts": [part(&body, "body")],
                "joints": [{ "part": "body", "socket": "body" }],
            }),
            "assemble_joint_same_role",
        ),
        (
            json!({ "version": 1, "parts": [part(&body, "body")], "extra": 1 }),
            "assemble_manifest",
        ),
    ];
    for (manifest, code) in cases {
        let error = assemble(manifest, &out, &report).expect_err("refused");
        assert_eq!(error.code(), code, "{}", error.message());
    }
}

#[test]
fn a_part_file_that_does_not_exist_is_named_by_role() {
    let dir = work_dir("io");
    let body = write(&dir, "body.obj", &torso_text());
    let out = dir.join("io.obj");
    let report = dir.join("io.json");
    let error = assemble(
        json!({
            "version": 1,
            "parts": [
                part(&body, "body"),
                part(dir.join("absent.obj").to_str().expect("utf8"), "feet"),
            ],
        }),
        &out,
        &report,
    )
    .expect_err("a missing part file is refused");
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "assemble_part_io");
    assert!(error.message().contains("feet"), "{}", error.message());
}
