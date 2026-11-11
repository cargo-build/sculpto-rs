//! Returned ordinary geometry must pass independently of the fit report. @req DECL0013

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::{dispatch, obj};
use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::outfit::ClothRegion;
use sculpto_mesh::outfit::rigging::fit_rigged_regions;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::{FitOptions, Mesh, fit_to_body};
use sculpto_nif::write::{header::NifTarget, write_shape};
use sculpto_nif::{NifDocument, ShapePlacement};
use serde_json::{Value, json};

#[path = "support/garment_pose.rs"]
mod garment_pose;

struct Case {
    shape: &'static str,
    counts: (usize, usize),
    silhouette_baseline: f64,
}

struct Reference<'a> {
    path: &'a Path,
    name: &'static str,
    counts: (usize, usize),
    frame: &'a OrthographicFrame,
    feet: Option<&'a NifDocument>,
    template: &'a Path,
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "sculpto-fit-acceptance-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("unique fit acceptance directory");
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove owned acceptance files");
    }
}

fn document(path: &Path) -> NifDocument {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|error| panic!("configured fixture {}: {error}", path.display()));
    sculpto_nif::read(&bytes)
        .unwrap_or_else(|error| panic!("configured fixture {}: {error}", path.display()))
}

fn placed(document: &NifDocument, shape: usize) -> Vec<[f32; 3]> {
    let placement = document.shape_placement(shape).expect("source placement");
    document.shapes[shape]
        .positions
        .iter()
        .map(|&point| placement.position_in_source(point))
        .collect()
}

struct CollisionInput {
    nif: PathBuf,
    mesh: MeshAsset,
    placement: ShapePlacement,
    parts: Vec<Value>,
}

/// Supply every solid component through the existing named-reference contract. @req DECL0013
fn collision_input(
    fixture: &Fixture,
    body: &NifDocument,
    body_index: usize,
    feet: Option<&NifDocument>,
) -> CollisionInput {
    let mut selected = vec![(body, body_index)];
    if let Some(feet) = feet {
        selected.extend(
            feet.shapes
                .iter()
                .enumerate()
                .map(|(index, _)| (feet, index)),
        );
    }
    let mut mesh = MeshAsset {
        name: "CollisionReference".into(),
        ..MeshAsset::default()
    };
    let parts = selected
        .into_iter()
        .map(|(document, index)| {
            let shape = &document.shapes[index];
            let vertex_start = mesh.positions.len();
            let triangle_start = mesh.triangles.len();
            let offset = u32::try_from(vertex_start).expect("fixture index width");
            mesh.positions.extend(placed(document, index));
            mesh.triangles.extend(
                shape
                    .triangles
                    .iter()
                    .map(|face| face.map(|vertex| vertex + offset)),
            );
            json!({"shape":shape.name,"vertex_start":vertex_start,"vertices":shape.positions.len(),
                "triangle_start":triangle_start,"triangles":shape.triangles.len(),
                "source_global_skin":document.sources[index].global_skin})
        })
        .collect();
    mesh.triangle_partitions = vec![PartitionId(0); mesh.triangles.len()];
    let nif = fixture.root.join("collision-reference.nif");
    std::fs::write(
        &nif,
        write_shape(&mesh, NifTarget::Se).expect("solid reference NIF"),
    )
    .expect("collision reference file");
    let decoded = document(&nif);
    assert_eq!(decoded.shapes.len(), 1);
    assert_eq!(decoded.shapes[0].triangles, mesh.triangles);
    assert_eq!(
        placed(&decoded, 0),
        mesh.positions,
        "lossless source-world assembly"
    );
    let placement = decoded
        .shape_placement(0)
        .expect("collision reference frame");
    CollisionInput {
        nif,
        mesh,
        placement,
        parts,
    }
}

fn normal(points: &[[f32; 3]], face: [u32; 3]) -> [f64; 3] {
    let [a, b, c] = face.map(|vertex| points[vertex as usize].map(f64::from));
    let ab: [_; 3] = std::array::from_fn(|axis| b[axis] - a[axis]);
    let ac: [_; 3] = std::array::from_fn(|axis| c[axis] - a[axis]);
    [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ]
}

fn flipped(source: &[[f32; 3]], output: &[[f32; 3]], faces: &[[u32; 3]]) -> usize {
    faces
        .iter()
        .filter(|&&face| {
            normal(source, face)
                .iter()
                .zip(normal(output, face))
                .map(|(a, b)| a * b)
                .sum::<f64>()
                < 0.0
        })
        .count()
}

fn inside(garment: Mesh<'_>, body: Mesh<'_>) -> usize {
    let locks = VertexMask::from_values(vec![1.0; garment.0.len()]).expect("measurement locks");
    let (unchanged, diagnostic) = fit_to_body(garment, body, Some(&locks), &FitOptions::default())
        .expect("locked inside measurement");
    assert_eq!(
        unchanged, garment.0,
        "measurement must preserve returned positions"
    );
    diagnostic.inside_after
}

fn ordinary_case(
    fixture: &Fixture,
    garment_path: &Path,
    reference: &Reference<'_>,
    case: &Case,
) -> (Value, Vec<String>) {
    let garment = document(garment_path);
    let body = document(reference.path);
    let garment_index = garment
        .shapes
        .iter()
        .position(|shape| shape.name == case.shape)
        .expect("pinned garment shape");
    let body_index = body
        .shapes
        .iter()
        .position(|shape| shape.name == reference.name)
        .expect("pinned body shape");
    let shape = &garment.shapes[garment_index];
    let body_shape = &body.shapes[body_index];
    assert_eq!(
        (body_shape.positions.len(), body_shape.triangles.len()),
        reference.counts,
        "accepted body identity"
    );
    assert_eq!((shape.positions.len(), shape.triangles.len()), case.counts);
    let source = placed(&garment, garment_index);
    let body_positions = placed(&body, body_index);
    let collision = collision_input(fixture, &body, body_index, reference.feet);
    let body_obj = fixture.root.join("body.obj");
    obj::write_obj(
        &body_obj,
        &collision.mesh.positions,
        &collision.mesh.triangles,
    )
    .expect("complete target in collision-reference frame");
    let manifest = fixture.root.join("manifest.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec(&json!({
            "outfit":"FIT-08 ordinary acceptance",
            "nif":garment_path,
            "garment_shapes":[{"name":case.shape,"blender_cloth":[]}],
        }))
        .expect("manifest JSON"),
    )
    .expect("source-bound ordinary manifest");
    let output_path = fixture.root.join(format!("{}.obj", case.shape));
    let start = Instant::now();
    let report = dispatch(
        "outfit.fit",
        &json!({
            "nif":garment_path,"shape":case.shape,"body_nif":collision.nif,
            "body_shape":collision.mesh.name,"body":body_obj,"manifest":manifest,
            "out":output_path,
        }),
    )
    .expect("registered manifest-aware fit");
    let elapsed = start.elapsed();
    let stored = obj::read_obj_positions(&output_path).expect("actual returned OBJ");
    let output: Vec<_> = stored
        .into_iter()
        .map(|point| collision.placement.position_in_source(point))
        .collect();
    assert_eq!(output.len(), source.len(), "no vertex trimming");
    let text = std::fs::read_to_string(&output_path).expect("returned topology");
    let faces: Vec<_> = text.lines().filter(|line| line.starts_with("f ")).collect();
    let expected: Vec<_> = shape
        .triangles
        .iter()
        .map(|[a, b, c]| format!("f {} {} {}", a + 1, b + 1, c + 1))
        .collect();
    assert_eq!(faces, expected, "source faces and order must survive");
    let body_contacts = intersection_pairs(
        &output,
        &shape.triangles,
        &body_positions,
        &body_shape.triangles,
    );
    assert_eq!(report["regions"]["ordinary_triangles"], case.counts.1);
    assert_eq!(report["regions"]["cloth_triangles"], 0);
    let body_inside = inside(
        (&output, &shape.triangles),
        (&body_positions, &body_shape.triangles),
    );
    let feet_contacts = reference.feet.map_or(0, |feet| {
        feet.shapes
            .iter()
            .enumerate()
            .map(|(index, feet_shape)| {
                intersection_pairs(
                    &output,
                    &shape.triangles,
                    &placed(feet, index),
                    &feet_shape.triangles,
                )
            })
            .sum::<usize>()
    });
    let full_inside = inside(
        (&output, &shape.triangles),
        (&collision.mesh.positions, &collision.mesh.triangles),
    );
    assert_eq!(
        report["regions"]["ordinary"]["intersection_pairs"],
        body_contacts + feet_contacts,
        "report must describe all solid components on the written ordinary surface"
    );
    assert_eq!(report["regions"]["ordinary"]["inside_after"], full_inside);
    let original_self: HashSet<_> = self_intersection_witnesses(&source, &shape.triangles)
        .into_iter()
        .collect();
    let contacts = self_intersection_witnesses(&output, &shape.triangles);
    let new_self = contacts
        .iter()
        .filter(|witness| !original_self.contains(witness))
        .count();
    let flips = flipped(&source, &output, &shape.triangles);
    let overlap = compare(
        (&source, &shape.triangles),
        (&output, &shape.triangles),
        reference.frame,
    )
    .expect("fixed silhouette frame without cropping");
    let pose = garment_pose::transferred_garment(
        &garment_pose::Case {
            label: case.shape,
            body: reference.path.to_path_buf(),
            body_shape: reference.name,
            body_counts: reference.counts,
            template: reference.template.to_path_buf(),
            garment: garment_path.to_path_buf(),
            garment_shape: case.shape,
            garment_counts: case.counts,
            copy_radius: match case.shape {
                "Armor001" => 11.0,
                _ => sculpto_mesh::garment::DEFAULT_TRANSFER_RADIUS,
            },
            default_radius_refusal: None,
        },
        garment_pose::Geometry::Fitted(&output),
    );
    let measured = json!({
        "shape":case.shape,"vertices":output.len(),"triangles":faces.len(),
        "body_contacts":body_contacts,"feet_contacts":feet_contacts,
        "body_inside":body_inside,"full_reference_inside":full_inside,
        "source_self":original_self.len(),"returned_self":contacts.len(),
        "new_self":new_self,"flips":flips,"silhouette_iou":overlap.iou,
        "silhouette_baseline":case.silhouette_baseline,
        "fit_seconds":elapsed.as_secs_f64(),"report":report,
        "collision_reference":{"vertices":collision.mesh.positions.len(),
            "triangles":collision.mesh.triangles.len(),"parts":collision.parts},
        "pose":{"influences":pose.widest,"sum_error":pose.sum_error,
            "moving":pose.moving,"detached":pose.detached,
            "rest_error":pose.max_rest,"displacement_error":pose.max_error,
            "weight_controls":pose.controls.map(|c|json!({"locked_bone":c.locked_bone,
                "locked_vertices":c.locked_vertices,"max_locked_delta":c.max_locked_delta,
                "excluded_body_vertex":c.excluded_body_vertex,"preserved_garment_vertex":c.preserved_garment_vertex}))},
    });
    let mut failures = Vec::new();
    if body_contacts + feet_contacts != 0 || body_inside != 0 || full_inside != 0 {
        failures.push(format!(
            "{}: body={body_contacts}, feet={feet_contacts}, inside={body_inside}/{full_inside}",
            case.shape,
        ));
    }
    if new_self != 0 || flips != 0 {
        failures.push(format!(
            "{}: new_self={new_self}, flips={flips}",
            case.shape
        ));
    }
    if overlap.iou < case.silhouette_baseline {
        failures.push(format!(
            "{}: silhouette {} < {}",
            case.shape, overlap.iou, case.silhouette_baseline
        ));
    }
    if elapsed >= Duration::from_secs(60) {
        failures.push(format!("{}: fit took {elapsed:?}", case.shape));
    }
    (measured, failures)
}

#[test]
fn registered_fit_uses_the_complete_placed_collision_reference() {
    let fixture = Fixture::new();
    let square = |name: &str, height: f32| MeshAsset {
        name: name.into(),
        positions: vec![
            [-2.0, -2.0, height],
            [2.0, -2.0, height],
            [2.0, 2.0, height],
            [-2.0, 2.0, height],
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0); 2],
        ..MeshAsset::default()
    };
    let save = |asset: &MeshAsset| {
        let path = fixture.root.join(format!("{}.nif", asset.name));
        std::fs::write(
            &path,
            write_shape(asset, NifTarget::Se).expect("portable NIF"),
        )
        .expect("portable source file");
        path
    };
    let body = document(&save(&square("Body", 100.0)));
    let mut feet = document(&save(&square("Foot", 20.0)));
    feet.sources[0].local.translation = [0.0, 0.0, -20.0];
    let garment = MeshAsset {
        name: "Garment".into(),
        positions: vec![[-0.5, -0.5, -1.0], [0.5, -0.5, 1.0], [0.0, 0.5, 1.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        ..MeshAsset::default()
    };
    let source = save(&garment);
    let manifest = fixture.root.join("manifest.json");
    std::fs::write(&manifest, serde_json::to_vec(&json!({
        "outfit":"Complete collision reference control","nif":source,
        "garment_shapes":[{"name":"Garment","blender_cloth":[{"name":"preserved","triangles":[0]}]}],
    })).expect("manifest JSON")).expect("manifest file");
    let output = fixture.root.join("fitted.obj");
    let run = |collision: &CollisionInput| {
        let target = fixture.root.join("body.obj");
        obj::write_obj(
            &target,
            &collision.mesh.positions,
            &collision.mesh.triangles,
        )
        .expect("target OBJ");
        dispatch(
            "outfit.fit",
            &json!({
                "nif":source,"shape":"Garment","body_nif":collision.nif,
                "body_shape":collision.mesh.name,"body":target,"manifest":manifest,"out":output,
            }),
        )
        .expect("registered collision reference")
    };
    let primary = collision_input(&fixture, &body, 0, None);
    let before = run(&primary);
    assert_eq!(before["regions"]["cloth_intersection_pairs"], 0);
    let complete = collision_input(&fixture, &body, 0, Some(&feet));
    assert_eq!(
        (complete.mesh.positions.len(), complete.mesh.triangles.len()),
        (8, 4)
    );
    assert_eq!(
        &complete.mesh.positions[4..],
        &square("Expected", 0.0).positions
    );
    assert_eq!(complete.parts[1]["vertex_start"], 4);
    let after = run(&complete);
    assert!(
        after["regions"]["cloth_intersection_pairs"]
            .as_u64()
            .expect("contact count")
            > 0
    );
    assert_eq!(
        obj::read_obj_positions(&output).expect("preserved cloth output"),
        garment.positions
    );
}

#[test]
fn cloth_preservation_does_not_waive_an_ordinary_contact() {
    let body = [
        [-10.0, -10.0, 0.0],
        [10.0, -10.0, 0.0],
        [10.0, 10.0, 0.0],
        [-10.0, 10.0, 0.0],
    ];
    let body_faces = [[0, 1, 2], [0, 2, 3]];
    let garment = [
        [-1.0, -1.0, -0.25],
        [1.0, -1.0, -0.25],
        [0.0, -2.0, -0.25],
        [0.0, 1.0, 0.25],
    ];
    let faces = [[0, 1, 2], [0, 1, 3]];
    let cloth = [ClothRegion {
        name: "loose panel".into(),
        triangles: Some(vec![0]),
    }];
    let locks = VertexMask::from_values(vec![1.0; garment.len()]).expect("all locked");
    let (output, report) = fit_rigged_regions(
        (&garment, &faces),
        (&body, &body_faces),
        &cloth,
        Some(&locks),
        &FitOptions::default(),
    )
    .expect("locked mixed selection remains measurable");
    assert_eq!(
        output, garment,
        "cloth, shared seam and explicit locks are exact"
    );
    assert_eq!((report.ordinary_triangles, report.cloth_triangles), (1, 1));
    let ordinary = report.ordinary.expect("ordinary report");
    let measured = intersection_pairs(&output, &faces[1..], &body, &body_faces);
    assert!(
        measured > 0,
        "negative control must retain an ordinary contact"
    );
    assert_eq!(ordinary.intersection_pairs, measured);
    assert!(
        ordinary.inside_after > 0,
        "locked inside groups remain unqualified"
    );
}

#[test]
fn configured_ordinary_outputs_meet_contact_and_construction_bounds() {
    let names = [
        "SCULPTO_FIT_FIXTURE",
        "SCULPTO_MOD_FIXTURE",
        "SCULPTO_RIG_TEMPLATE_FIXTURES",
    ];
    let missing: Vec<_> = names
        .iter()
        .filter(|name| std::env::var_os(name).is_none())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "SKIP configured_ordinary_outputs_meet_contact_and_construction_bounds: missing {missing:?}"
        );
        return;
    }
    let root = |name: &str| PathBuf::from(std::env::var_os(name).expect("checked fixture root"));
    let fit = root(names[0]);
    let mods = root(names[1]);
    let rigs = root(names[2]);
    let fixture = Fixture::new();
    let sentinel_frame =
        OrthographicFrame::new([[-31.0, 31.0], [7.0, 118.0]], RasterSettings::default())
            .expect("frozen Sentinel frame");
    let apachii_frame =
        OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
            .expect("frozen Apachii frame");
    let feet = document(&mods.join("bijin-warmaidens/Bijin Warmaidens v3.1.3/03 CBBE/meshes/actors/character/Bijin Warmaidens/femalefeet_1.nif"));
    let male_feet = document(
        &mods.join("himbo-v5-core/CalienteTools/BodySlide/ShapeData/HIMBO Res/HIMBO Feet.nif"),
    );
    assert_eq!(male_feet.shapes.len(), 1, "pinned raw HIMBO feet");
    assert_eq!(
        (
            male_feet.shapes[0].positions.len(),
            male_feet.shapes[0].triangles.len()
        ),
        (1284, 2244),
        "feet reference must match the raw HIMBO family"
    );
    let sentinel = [
        Case {
            shape: "Armor001",
            counts: (2737, 4040),
            silhouette_baseline: 0.9372047546854745,
        },
        Case {
            shape: "Armor001_1",
            counts: (702, 1200),
            silhouette_baseline: 0.9040660175646305,
        },
    ];
    let apachii = [
        Case {
            shape: "pants",
            counts: (392, 632),
            silhouette_baseline: 0.8883417222146647,
        },
        Case {
            shape: "belt",
            counts: (403, 427),
            silhouette_baseline: 0.6361788617886179,
        },
        Case {
            shape: "sleeves",
            counts: (556, 886),
            silhouette_baseline: 0.8338973076357217,
        },
    ];
    let sentinel_body = fit.join("body.nif");
    let male_template = rigs.join("skeleton_male_sse.nif");
    let bijin_body = rigs.join("bijin.nif");
    let female_template = rigs.join("skeleton_female_sse.nif");
    let sentinel_reference = Reference {
        path: &sentinel_body,
        name: "HIMBO - Body",
        counts: (9346, 18074),
        frame: &sentinel_frame,
        feet: Some(&male_feet),
        template: &male_template,
    };
    let bijin_reference = Reference {
        path: &bijin_body,
        name: "BaseShape",
        counts: (3471, 6516),
        frame: &apachii_frame,
        feet: Some(&feet),
        template: &female_template,
    };
    let nightshade = mods.join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif");
    let results: Vec<_> = sentinel
        .iter()
        .map(|case| ordinary_case(&fixture, &fit.join("tunic.nif"), &sentinel_reference, case))
        .chain(
            apachii
                .iter()
                .map(|case| ordinary_case(&fixture, &nightshade, &bijin_reference, case)),
        )
        .collect();
    results
        .iter()
        .for_each(|(measurements, _)| eprintln!("FIT08_GEOMETRY {measurements}"));
    let failures: Vec<_> = results
        .into_iter()
        .flat_map(|(_, failures)| failures)
        .collect();
    assert!(
        failures.is_empty(),
        "ordinary fit acceptance failed:\n{}",
        failures.join("\n")
    );
}
