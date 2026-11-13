//! A declared complete collision surface reaches the registered fit. @req DECL0013

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_core::shape_data::{MorphBlock, MorphContainer, SUPPORTED_VERSION};
use sculpto_engine::{ErrorKind, dispatch, obj};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::{Value, json};

/// One connected part of a combined collision surface.
type Part = (Vec<[f32; 3]>, Vec<[u32; 3]>);

/// The primary authored body: a floor patch the garment must leave.
const BODY: [[f32; 3]; 4] = [
    [-10.0, -10.0, 0.0],
    [10.0, -10.0, 0.0],
    [10.0, 10.0, 0.0],
    [-10.0, 10.0, 0.0],
];
const BODY_FACES: [[u32; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

/// A separate part (a stand-in hand) standing above the body, clear of the
/// ordinary garment and crossing the retained cloth panel.
fn separate_part(shift: [f32; 3]) -> Part {
    let [dx, dy, dz] = shift;
    (
        [
            [0.2, 0.0, 0.3],
            [0.7, 0.0, 0.3],
            [0.7, 0.0, 0.7],
            [0.2, 0.0, 0.7],
        ]
        .map(|[x, y, z]| [x + dx, y + dy, z + dz])
        .to_vec(),
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

fn body_part() -> Part {
    (BODY.to_vec(), BODY_FACES.to_vec())
}

fn max_move(before: &[[f32; 3]], after: &[[f32; 3]]) -> f64 {
    before
        .iter()
        .zip(after)
        .map(|(a, b)| {
            a.iter()
                .zip(b)
                .map(|(a, b)| f64::from(a - b).powi(2))
                .sum::<f64>()
                .sqrt()
        })
        .fold(0.0, f64::max)
}

/// Combined surfaces keep every part in one index space, as an assembled
/// character export does.
fn write_parts(path: &Path, parts: &[Part]) {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for (part_positions, part_triangles) in parts {
        let base = u32::try_from(positions.len()).expect("fixture index space");
        positions.extend_from_slice(part_positions);
        triangles.extend(
            part_triangles
                .iter()
                .map(|face| face.map(|index| index + base)),
        );
    }
    obj::write_obj(path, &positions, &triangles).expect("collision OBJ");
}

struct Fixture {
    root: PathBuf,
    garment: MeshAsset,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("sculpto-collision-{tag}-{}", std::process::id()));
        std::fs::create_dir(&root).expect("unique collision fixture");
        let body = MeshAsset {
            name: "Body".into(),
            positions: BODY.to_vec(),
            triangles: BODY_FACES.to_vec(),
            triangle_partitions: vec![PartitionId(0); 2],
            ..MeshAsset::default()
        };
        let garment = MeshAsset {
            name: "Garment".into(),
            positions: vec![
                [-4.0, -0.5, -0.2],
                [-2.0, -0.5, -0.2],
                [-3.0, 0.5, -0.2],
                [0.0, -1.0, 0.5],
                [1.0, -1.0, 0.5],
                [0.0, 1.0, 0.5],
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            triangle_partitions: vec![PartitionId(0); 2],
            ..MeshAsset::default()
        };
        std::fs::write(
            root.join("body.nif"),
            write_se_shape(&body).expect("body NIF"),
        )
        .expect("body file");
        std::fs::write(
            root.join("garment.nif"),
            write_se_shape(&garment).expect("garment NIF"),
        )
        .expect("garment file");
        obj::write_obj(&root.join("body.obj"), &body.positions, &body.triangles)
            .expect("source body OBJ");
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&json!({
                "outfit":"Collision fixture", "nif":"garment.nif",
                "garment_shapes":[{"name":"Garment", "blender_cloth":[{"name":"loose", "triangles":[1]}]}],
            }))
            .expect("manifest JSON"),
        )
        .expect("manifest file");
        Self { root, garment }
    }

    /// The region request without a collision scope: ordinary faces are fitted
    /// and measured against the primary body shape only.
    fn request(&self) -> Value {
        json!({"nif":self.root.join("garment.nif"), "shape":"Garment", "body_nif":self.root.join("body.nif"),
            "body_shape":"Body", "body":self.root.join("body.obj"), "manifest":self.root.join("manifest.json"),
            "out":self.root.join("fitted.obj")})
    }

    fn collision(&self, name: &str, parts: &[Part]) -> PathBuf {
        let path = self.root.join(name);
        write_parts(&path, parts);
        path
    }

    fn out(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove collision fixture");
    }
}

#[test]
fn slider_preview_measures_against_the_declared_surface() {
    let fixture = Fixture::new("preview");
    std::fs::write(
        fixture.root.join("conform.osd"),
        MorphContainer {
            version: SUPPORTED_VERSION,
            blocks: vec![MorphBlock {
                name: "GarmentLift".into(),
                offsets: Vec::new(),
            }],
        }
        .to_osd_bytes()
        .expect("conform data"),
    )
    .expect("conform file");
    let preview = |collision: Option<&Path>, out: &str| {
        let mut request = fixture.request();
        let fields = request.as_object_mut().expect("object");
        fields.remove("manifest");
        request["conform"] = json!(fixture.root.join("conform.osd"));
        request["slider"] = json!("Lift");
        request["percent"] = json!(100.0);
        request["out"] = json!(fixture.out(out));
        if let Some(collision) = collision {
            request["collision"] = json!(collision);
            request["collision_frame"] = json!("source-scene");
        }
        dispatch("outfit.fit", &request).expect("slider preview fit")
    };
    let single = preview(None, "single.obj");
    assert_eq!(single["intersection_pairs"], 0);
    assert!(single.get("collision").is_none());
    let single_out =
        obj::read_obj_positions(&fixture.out("single.obj")).expect("body-only preview output");
    let reported_move = single["repair"]["max_displacement"]
        .as_f64()
        .expect("returned-geometry displacement");
    assert!((reported_move - max_move(&fixture.garment.positions, &single_out)).abs() < 1e-5);
    assert_eq!(
        single["repair"]["intersection_pairs"],
        single["intersection_pairs"]
    );
    assert!(single["repair"]["max_strain"].as_f64().is_some());
    assert_eq!(single["repair"]["collapsed_edges"], 0);
    assert!(
        single_out[0][2] > 0.0,
        "the authored body lifts the ordinary faces in the single-shape path"
    );

    // A declared scope replaces the body as the obstacle set: a part that is
    // nowhere near the garment leaves it at its authored positions.
    let hands_only = fixture.collision("hand.obj", &[separate_part([5.0, 3.0, 0.0])]);
    let scoped = preview(Some(&hands_only), "scoped.obj");
    assert_eq!(scoped["collision"]["frame"], "source-scene");
    assert_eq!(scoped["collision"]["scope"], "caller-declared");
    assert_eq!(scoped["collision"]["triangles"], 2);
    assert_eq!(scoped["intersection_pairs"], 0);
    let scoped_out =
        obj::read_obj_positions(&fixture.out("scoped.obj")).expect("scoped preview output");
    assert_eq!(
        scoped_out, fixture.garment.positions,
        "no body plane was declared, so the preview cannot fit the garment to one"
    );
}

#[test]
fn positions_only_body_obj_keeps_relative_face_syntax() {
    let fixture = Fixture::new("relative-faces");
    let relative = fixture.out("body-relative.obj");
    std::fs::write(
        &relative,
        "v -10 -10 0\nv 10 -10 0\nv 10 10 0\nv -10 10 0\nf -3 -2 -1\n",
    )
    .expect("positions-only body OBJ");
    assert_eq!(
        obj::read_obj_positions(&relative)
            .expect("the positions-only reader ignores face records")
            .len(),
        4
    );
    let mesh = obj::read_obj_mesh(&relative).expect_err("a mesh needs absolute face indices");
    assert!(
        mesh.to_string()
            .contains("face corner `-3` is not a vertex index"),
        "{mesh}"
    );

    // The registered slider preview consumes the body OBJ through the
    // positions-only contract, so relative face syntax must stay acceptable.
    std::fs::write(
        fixture.root.join("conform.osd"),
        MorphContainer {
            version: SUPPORTED_VERSION,
            blocks: vec![MorphBlock {
                name: "GarmentLift".into(),
                offsets: Vec::new(),
            }],
        }
        .to_osd_bytes()
        .expect("conform data"),
    )
    .expect("conform file");
    let mut request = fixture.request();
    request.as_object_mut().expect("object").remove("manifest");
    request["conform"] = json!(fixture.root.join("conform.osd"));
    request["slider"] = json!("Lift");
    request["percent"] = json!(100.0);
    request["body"] = json!(relative);
    let result = dispatch("outfit.fit", &request).expect("relative-face body still fits");
    assert_eq!(result["intersection_pairs"], 0);
}

#[test]
fn unreadable_obj_files_stay_io_errors() {
    let fixture = Fixture::new("missing-file");
    let missing = fixture.out("absent.obj");
    let error = obj::read_obj_positions(&missing).expect_err("missing positions-only OBJ");
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "io_error");
    assert!(error.to_string().contains("absent.obj"), "{error}");

    let mut request = fixture.request();
    request["collision"] = json!(missing);
    request["collision_frame"] = json!("source-scene");
    let error = dispatch("outfit.fit", &request).expect_err("missing collision OBJ");
    assert_eq!(error.kind(), ErrorKind::Io);
    assert_eq!(error.code(), "io_error");
    assert!(!fixture.root.join("fitted.obj").exists());
}

#[test]
fn separate_part_contact_is_not_reported_clear() {
    let fixture = Fixture::new("separate-part");
    let body_only = dispatch("outfit.fit", &fixture.request()).expect("body-only fit");
    assert_eq!(body_only["intersection_pairs"], 0);
    assert_eq!(body_only["regions"]["cloth_intersection_pairs"], 0);
    assert!(
        body_only.get("collision").is_none(),
        "no declared scope means no scope report"
    );

    let collision = fixture.collision("body-and-hand.obj", &[body_part(), separate_part([0.0; 3])]);
    let mut request = fixture.request();
    request["collision"] = json!(collision);
    request["collision_frame"] = json!("source-scene");
    let scoped = dispatch("outfit.fit", &request).expect("scoped fit");
    assert!(
        scoped["regions"]["cloth_intersection_pairs"]
            .as_u64()
            .expect("cloth contacts")
            > 0,
        "a separate part crossing the retained panel is a contact: {scoped}"
    );
    assert!(
        scoped["intersection_pairs"].as_u64().expect("contacts") > 0,
        "the complete garment cannot be reported clear: {scoped}"
    );
    assert_eq!(scoped["collision"]["frame"], "source-scene");
    assert_eq!(scoped["collision"]["scope"], "caller-declared");
    assert_eq!(scoped["collision"]["vertices"], 8);
    assert_eq!(scoped["collision"]["triangles"], 4);

    // The ordinary region is still fitted off the body, and the retained cloth
    // panel keeps its incoming positions: the scope only adds obstacles.
    let output = obj::read_obj_positions(&fixture.root.join("fitted.obj")).expect("fit output");
    let reported_move = scoped["regions"]["max_displacement"]
        .as_f64()
        .expect("whole-garment displacement");
    assert!((reported_move - max_move(&fixture.garment.positions, &output)).abs() < 1e-5);
    assert!(scoped["regions"]["max_strain"].as_f64().is_some());
    assert_eq!(scoped["regions"]["collapsed_edges"], 0);
    assert!(
        output[..3].iter().all(|point| point[2] > 0.0),
        "ordinary faces must leave the body and the separate part"
    );
    assert_eq!(&output[3..], &fixture.garment.positions[3..]);
}

#[test]
fn a_body_only_scope_reproduces_the_single_shape_path() {
    let fixture = Fixture::new("body-only");
    let single = dispatch("outfit.fit", &fixture.request()).expect("single-shape fit");
    let collision = fixture.collision("body.obj", &[body_part()]);
    let mut request = fixture.request();
    request["collision"] = json!(collision);
    request["collision_frame"] = json!("source-scene");
    request["out"] = json!(fixture.out("scoped.obj"));
    let scoped = dispatch("outfit.fit", &request).expect("body-only scope fit");
    assert_eq!(scoped["intersection_pairs"], single["intersection_pairs"]);
    assert_eq!(
        scoped["regions"]["ordinary"]["intersection_pairs"],
        single["regions"]["ordinary"]["intersection_pairs"]
    );
    assert_eq!(
        scoped["regions"]["cloth_intersection_pairs"],
        single["regions"]["cloth_intersection_pairs"]
    );
    assert_eq!(scoped["collision"]["triangles"], 2);
    // Adding the surface that already was the reference changes no verdict;
    // it only makes the measured scope explicit.
    assert_eq!(scoped["intersection_pairs"], 0);
}

#[test]
fn declared_coordinates_are_used_literally() {
    let fixture = Fixture::new("placement");
    let crossing = fixture.collision("crossing.obj", &[body_part(), separate_part([0.0; 3])]);
    let elsewhere = fixture.collision(
        "elsewhere.obj",
        &[body_part(), separate_part([5.0, 3.0, 0.0])],
    );
    let contact = |collision: &Path, out: &str| {
        let mut request = fixture.request();
        request["collision"] = json!(collision);
        request["collision_frame"] = json!("source-scene");
        request["out"] = json!(fixture.out(out));
        dispatch("outfit.fit", &request).expect("placed scope fit")
    };
    let crossed = contact(&crossing, "crossed.obj");
    let moved = contact(&elsewhere, "moved.obj");
    assert!(
        crossed["regions"]["cloth_intersection_pairs"]
            .as_u64()
            .expect("cloth contacts")
            > 0
    );
    assert_eq!(
        moved["regions"]["cloth_intersection_pairs"], 0,
        "a part moved away in the declared frame adds no contact: {moved}"
    );

    // The declared frame is the shared OBJ display frame of the NIF source
    // scene, so the crossing blade is written as `(x, z, -y)`. A reader that
    // skipped that conversion would put it in the body plane and report none.
    let display = fixture.out("display.obj");
    std::fs::write(
        &display,
        "\
v -10 -10 10\nv 10 -10 10\nv 10 10 10\nv -10 10 10\n\
v 0.2 0.3 -0\nv 0.7 0.3 -0\nv 0.7 0.7 -0\nv 0.2 0.7 -0\n\
f 1 2 3\nf 1 3 4\nf 5 6 7\nf 5 7 8\n",
    )
    .expect("display-frame collision OBJ");
    let axis = contact(&display, "axis.obj");
    assert!(
        axis["regions"]["cloth_intersection_pairs"]
            .as_u64()
            .expect("cloth contacts")
            > 0,
        "the declared axis conversion places the part across the panel: {axis}"
    );
}

#[test]
fn an_explicit_collision_needs_a_guarded_fit() {
    let fixture = Fixture::new("unguarded");
    let collision = fixture.collision("body-and-hand.obj", &[body_part(), separate_part([0.0; 3])]);
    let mut request = fixture.request();
    request["collision"] = json!(collision);
    request["collision_frame"] = json!("source-scene");
    let fields = request.as_object_mut().expect("object");
    fields.remove("manifest");
    fields.remove("body_nif");
    let error = dispatch("outfit.fit", &request).expect_err("unsupported combination");
    assert_eq!(error.code(), "outfit_fit_collision_scope");
    assert!(!fixture.root.join("fitted.obj").exists());
}

#[test]
fn unsupported_or_undeclared_frames_are_refused_before_output() {
    let fixture = Fixture::new("frame");
    let collision = fixture.collision("body-and-hand.obj", &[body_part(), separate_part([0.0; 3])]);
    let refused = |frame: Option<&str>, with_surface: bool| {
        let mut request = fixture.request();
        if with_surface {
            request["collision"] = json!(collision);
        }
        if let Some(frame) = frame {
            request["collision_frame"] = json!(frame);
        }
        dispatch("outfit.fit", &request).expect_err("frame refusal")
    };
    for error in [
        refused(Some("mesh"), true),
        refused(None, true),
        refused(Some("source-scene"), false),
    ] {
        assert_eq!(error.code(), "outfit_fit_collision_frame");
        assert!(!fixture.root.join("fitted.obj").exists());
    }
}

#[test]
fn malformed_collision_input_is_refused_before_output() {
    let fixture = Fixture::new("malformed");
    let refuse = |name: &str, text: &str| {
        let path = fixture.out(name);
        std::fs::write(&path, text).expect("collision fixture");
        let mut request = fixture.request();
        request["collision"] = json!(path);
        request["collision_frame"] = json!("source-scene");
        dispatch("outfit.fit", &request).expect_err("malformed collision refusal")
    };
    let cases = [
        ("empty.obj", ""),
        ("positions.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\n"),
        ("index.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 99\n"),
    ];
    for (name, text) in cases {
        let error = refuse(name, text);
        assert!(
            ["invalid_value", "outfit_fit_collision_surface"].contains(&error.code()),
            "{name}: {error}"
        );
        assert!(!fixture.root.join("fitted.obj").exists());
    }
    let nan = refuse("nan.obj", "v NaN 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
    assert_eq!(nan.code(), "outfit_fit_collision_surface");
    assert!(!fixture.root.join("fitted.obj").exists());

    let degenerate = refuse("degenerate.obj", "v 0 0 0\nv 1 0 0\nv 2 0 0\nf 1 2 3\n");
    assert_eq!(degenerate.code(), "outfit_fit_collision_surface");
    assert!(
        degenerate.to_string().contains("triangle 0"),
        "{degenerate}"
    );
    assert!(!fixture.root.join("fitted.obj").exists());
}
