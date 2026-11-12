//! Returned ordinary geometry must pass independently of the fit report. @req DECL0013

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::{dispatch, obj};
use sculpto_mesh::check::{intersection_pairs, self_intersection_witnesses};
use sculpto_mesh::control::measure::ControlDiagnostics;
use sculpto_mesh::control::{ControlProfile, SourceSurface, compact, source_selection};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::outfit::rigging::fit_rigged_regions;
use sculpto_mesh::outfit::{ClothRegion, OutfitManifest};
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
    preparation: Option<&'static str>,
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

/// Bit-exact position identity; `-0.0` and `0.0` are one place, as in intake.
fn position_key(point: [f32; 3]) -> [u32; 3] {
    point.map(|value| match value == 0.0 {
        true => 0,
        false => value.to_bits(),
    })
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
    let Some(profile_path) = case.preparation else {
        return fitted_case(fixture, garment_path, reference, case);
    };
    let original = document(garment_path);
    let index = original
        .shapes
        .iter()
        .position(|s| s.name == case.shape)
        .expect("original shape");
    let shape = &original.shapes[index];
    assert_eq!((shape.positions.len(), shape.triangles.len()), case.counts);
    let source = placed(&original, index);
    let profile = ControlProfile::from_json(
        &std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(profile_path))
            .expect("declared source profile"),
    )
    .expect("valid source preparation");
    assert!(
        profile.source_cuts.is_empty(),
        "this source-face route does not execute native cuts"
    );
    let selection =
        source_selection(&source, &shape.triangles, &profile).expect("qualified source selection");
    let selected: Vec<_> = selection
        .selected_faces
        .iter()
        .map(|&id| shape.triangles[id as usize])
        .collect();
    let (positions, triangles, origins) = compact(&source, &selected);
    let physical = SourceSurface::identify(&positions, &triangles).expect("physical seam identity");
    let diagnostics = ControlDiagnostics::measure(physical.positions(), physical.triangles())
        .expect("prepared physical topology");
    diagnostics
        .qualify(&profile)
        .expect("all intended openings, genus and no incidental defects");
    let before = compare(
        (&source, &shape.triangles),
        (&positions, &triangles),
        reference.frame,
    )
    .expect("original/prepared fixed frame");
    assert_eq!(
        before.iou, 1.0,
        "preparation retains the original silhouette"
    );
    triangles
        .iter()
        .zip(&selection.selected_faces)
        .for_each(|(face, &id)| {
            assert_eq!(
                face.map(|v| origins[v as usize]),
                shape.triangles[id as usize],
                "complete source corner lineage"
            )
        });
    let source_asset = MeshAsset::from(shape);
    let prepared = MeshAsset {
        name: shape.name.clone(),
        positions,
        triangles,
        uvs: origins.iter().map(|&v| shape.uvs[v as usize]).collect(),
        triangle_partitions: selection
            .selected_faces
            .iter()
            .map(|&id| source_asset.triangle_partitions[id as usize])
            .collect(),
        materials: source_asset.materials,
        ..MeshAsset::default()
    };
    let prepared_path = fixture.root.join(format!("{}-prepared.nif", case.shape));
    std::fs::write(
        &prepared_path,
        write_shape(&prepared, NifTarget::Se).expect("prepared source NIF"),
    )
    .expect("derived source file");
    let readback = document(&prepared_path);
    assert_eq!(
        placed(&readback, 0),
        prepared.positions,
        "source scene placement preserved"
    );
    assert_eq!(readback.shapes[0].triangles, prepared.triangles);
    assert_eq!(
        readback.shapes[0].uvs, prepared.uvs,
        "UV seams and corners survive the handoff"
    );
    let prepared_case = Case {
        shape: case.shape,
        counts: (prepared.positions.len(), prepared.triangles.len()),
        silhouette_baseline: case.silhouette_baseline,
        preparation: None,
    };
    let (mut measured, mut failures) =
        fitted_case(fixture, &prepared_path, reference, &prepared_case);
    // The acceptance collision assembly is source-world identity; fitted_case checks read-back.
    let output = obj::read_obj_positions(&fixture.root.join(format!("{}.obj", case.shape)))
        .expect("written prepared fit");
    let final_overlap = compare(
        (&source, &shape.triangles),
        (&output, &prepared.triangles),
        reference.frame,
    )
    .expect("complete original silhouette frame");
    let physical =
        SourceSurface::identify(&output, &prepared.triangles).expect("fitted physical identity");
    ControlDiagnostics::measure(physical.positions(), physical.triangles())
        .expect("fitted physical topology")
        .qualify(&profile)
        .expect("fitted openings and construction");
    assert!(
        self_intersection_witnesses(&source, &shape.triangles).is_empty(),
        "this preparation fixture has no original self contacts"
    );
    if final_overlap.iou < case.silhouette_baseline {
        failures.push(format!(
            "{}: original silhouette {} < {}",
            case.shape, final_overlap.iou, case.silhouette_baseline
        ));
    }
    let kept: HashSet<_> = selection.selected_faces.iter().copied().collect();
    let area = |face: [u32; 3]| {
        normal(&source, face)
            .iter()
            .map(|n| n * n)
            .sum::<f64>()
            .sqrt()
            * 0.5
    };
    let original_area = shape.triangles.iter().map(|&f| area(f)).sum::<f64>();
    let omitted_area = shape
        .triangles
        .iter()
        .enumerate()
        .filter(|(id, _)| !kept.contains(&(*id as u32)))
        .map(|(_, &f)| area(f))
        .sum::<f64>();
    measured["preparation"] = json!({"profile":profile_path,"original_vertices":source.len(),"original_triangles":shape.triangles.len(),"source_vertices":origins,"source_faces":selection.selected_faces,
        "recovered_source_faces":selection.recovery.disks.iter().flat_map(|d|d.faces.iter().copied()).collect::<Vec<_>>(),"original_area":original_area,"omitted_original_area":omitted_area,"omitted_original_area_share":omitted_area/original_area,"prepared_source_iou":before.iou,"original_fitted_iou":final_overlap.iou,"exact_uv_corners":true});
    (measured, failures)
}

fn fitted_case(
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
            preparation: None,
        },
        Case {
            shape: "Armor001_1",
            counts: (702, 1200),
            silhouette_baseline: 0.9040660175646305,
            preparation: None,
        },
    ];
    let apachii = [
        Case {
            shape: "pants",
            counts: (392, 632),
            silhouette_baseline: 0.8883417222146647,
            preparation: Some("tests/fixtures/fit08/nightshade-pants-profile.json"),
        },
        Case {
            shape: "belt",
            counts: (403, 427),
            silhouette_baseline: 0.6361788617886179,
            preparation: None,
        },
        Case {
            shape: "sleeves",
            counts: (556, 886),
            silhouette_baseline: 0.8338973076357217,
            preparation: None,
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

#[test]
fn configured_mixed_robe_preserves_cloth_and_qualifies_ordinary_regions() {
    let names = ["SCULPTO_MOD_FIXTURE", "SCULPTO_RIG_TEMPLATE_FIXTURES"];
    let missing: Vec<_> = names
        .iter()
        .filter(|name| std::env::var_os(name).is_none())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "SKIP configured_mixed_robe_preserves_cloth_and_qualifies_ordinary_regions: missing {missing:?}"
        );
        return;
    }
    let mods = PathBuf::from(std::env::var_os(names[0]).expect("checked fixture root"));
    let rigs = PathBuf::from(std::env::var_os(names[1]).expect("checked fixture root"));
    let fixture = Fixture::new();
    let outfit_path = mods.join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif");
    let outfit = document(&outfit_path);
    for (name, counts) in [
        ("pants", (392, 632)),
        ("belt", (403, 427)),
        ("sleeves", (556, 886)),
        ("robe", (3808, 6410)),
    ] {
        let shape = outfit
            .shapes
            .iter()
            .find(|shape| shape.name == name)
            .expect("pinned Nightshade shape");
        assert_eq!(
            (shape.positions.len(), shape.triangles.len()),
            counts,
            "pinned source identity for {name}"
        );
    }
    let robe_index = outfit
        .shapes
        .iter()
        .position(|shape| shape.name == "robe")
        .expect("robe shape");
    let robe = &outfit.shapes[robe_index];
    let source = placed(&outfit, robe_index);

    // The frozen pre-fit selection is the authority; the belt band only cross-checks it.
    let descriptor_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fit08/nightshade-robe-regions.json");
    let descriptor_bytes = std::fs::read(&descriptor_path).expect("frozen region descriptor");
    let declared: OutfitManifest =
        serde_json::from_slice(&descriptor_bytes).expect("valid frozen manifest");
    let robe_slot = declared
        .garment_shapes
        .iter()
        .find(|slot| slot.name == "robe")
        .expect("declared robe slot");
    assert!(
        declared
            .garment_shapes
            .iter()
            .filter(|slot| slot.name != "robe")
            .all(|slot| slot.blender_cloth.is_empty()),
        "only the robe carries a declared cloth region"
    );
    let region = robe_slot
        .blender_cloth
        .first()
        .expect("declared cloth region");
    assert_eq!(region.name, "hanging_panels");
    let cloth_faces = region.triangles.as_ref().expect("explicit source faces");
    assert_eq!(cloth_faces.len(), 3052, "pinned cloth face count");
    let cloth_vertices: BTreeSet<_> = cloth_faces
        .iter()
        .flat_map(|&face| robe.triangles[face])
        .collect();
    assert_eq!(cloth_vertices.len(), 1870, "pinned cloth vertex count");
    let belt_index = outfit
        .shapes
        .iter()
        .position(|shape| shape.name == "belt")
        .expect("belt shape");
    let (bottom, top) = placed(&outfit, belt_index)
        .iter()
        .map(|point| point[2])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), z| {
            (min.min(z), max.max(z))
        });
    let waist = (bottom + top) * 0.5;
    let band: Vec<_> = robe
        .triangles
        .iter()
        .enumerate()
        .filter(|(_, face)| {
            face.iter()
                .all(|&vertex| source[vertex as usize][2] <= waist)
        })
        .map(|(index, _)| index)
        .collect();
    let mut declared_sorted = cloth_faces.clone();
    declared_sorted.sort_unstable();
    assert_eq!(
        band, declared_sorted,
        "frozen panels must equal the authored belt-band selection on the pinned source"
    );

    let body = document(&rigs.join("bijin.nif"));
    let body_index = body
        .shapes
        .iter()
        .position(|shape| shape.name == "BaseShape")
        .expect("pinned Bijin body shape");
    assert_eq!(
        (
            body.shapes[body_index].positions.len(),
            body.shapes[body_index].triangles.len()
        ),
        (3471, 6516),
        "accepted Bijin body identity"
    );
    let feet = document(&mods.join("bijin-warmaidens/Bijin Warmaidens v3.1.3/03 CBBE/meshes/actors/character/Bijin Warmaidens/femalefeet_1.nif"));
    let feet_counts = feet
        .shapes
        .iter()
        .fold((0, 0), |(vertices, triangles), shape| {
            (
                vertices + shape.positions.len(),
                triangles + shape.triangles.len(),
            )
        });
    assert_eq!(feet_counts, (2354, 3972), "pinned Bijin feet identity");
    let collision = collision_input(&fixture, &body, body_index, Some(&feet));
    assert_eq!(
        (
            collision.mesh.positions.len(),
            collision.mesh.triangles.len()
        ),
        (5825, 10488),
        "complete placed body+feet reference"
    );
    let body_positions = placed(&body, body_index);
    let body_obj = fixture.root.join("body.obj");
    obj::write_obj(
        &body_obj,
        &collision.mesh.positions,
        &collision.mesh.triangles,
    )
    .expect("complete target in collision-reference frame");
    let mut runtime: Value =
        serde_json::from_slice(&descriptor_bytes).expect("frozen manifest JSON");
    runtime["nif"] = json!(outfit_path);
    let manifest = fixture.root.join("manifest.json");
    std::fs::write(
        &manifest,
        serde_json::to_vec(&runtime).expect("manifest JSON"),
    )
    .expect("runtime manifest");
    let output_path = fixture.root.join("robe.obj");
    let start = Instant::now();
    let report = dispatch(
        "outfit.fit",
        &json!({
            "nif":outfit_path,"shape":"robe","body_nif":collision.nif,
            "body_shape":collision.mesh.name,"body":body_obj,"manifest":manifest,
            "out":output_path,
        }),
    )
    .expect("registered mixed-robe fit");
    let elapsed = start.elapsed();
    let stored = obj::read_obj_positions(&output_path).expect("actual returned OBJ");
    let output: Vec<_> = stored
        .into_iter()
        .map(|point| collision.placement.position_in_source(point))
        .collect();
    assert_eq!(output.len(), source.len(), "no vertex trimming");
    let text = std::fs::read_to_string(&output_path).expect("returned topology");
    let faces: Vec<_> = text.lines().filter(|line| line.starts_with("f ")).collect();
    let expected: Vec<_> = robe
        .triangles
        .iter()
        .map(|[a, b, c]| format!("f {} {} {}", a + 1, b + 1, c + 1))
        .collect();
    assert_eq!(faces, expected, "source faces and order must survive");

    let cloth_set: HashSet<_> = cloth_faces.iter().copied().collect();
    let cloth_keys: HashSet<_> = cloth_vertices
        .iter()
        .map(|&vertex| position_key(source[vertex as usize]))
        .collect();
    let mut preserved = 0_usize;
    let mut aliases = 0_usize;
    let mut max_cloth_error = 0.0_f64;
    for (vertex, &before) in source.iter().enumerate() {
        if !cloth_keys.contains(&position_key(before)) {
            continue;
        }
        preserved += 1;
        if !cloth_vertices.contains(&(vertex as u32)) {
            aliases += 1;
        }
        let error = output[vertex]
            .into_iter()
            .zip(before)
            .map(|(after, before)| f64::from(after - before).powi(2))
            .sum::<f64>()
            .sqrt();
        max_cloth_error = max_cloth_error.max(error);
    }

    let ordinary_faces: Vec<_> = robe
        .triangles
        .iter()
        .enumerate()
        .filter(|(index, _)| !cloth_set.contains(index))
        .map(|(_, &face)| face)
        .collect();
    assert_eq!(ordinary_faces.len(), 3358);
    let cloth_face_list: Vec<_> = cloth_faces
        .iter()
        .map(|&face| robe.triangles[face])
        .collect();
    let body_contacts = intersection_pairs(
        &output,
        &ordinary_faces,
        &body_positions,
        &body.shapes[body_index].triangles,
    );
    let feet_contacts = feet
        .shapes
        .iter()
        .enumerate()
        .map(|(index, feet_shape)| {
            intersection_pairs(
                &output,
                &ordinary_faces,
                &placed(&feet, index),
                &feet_shape.triangles,
            )
        })
        .sum::<usize>();
    // Inside counts weld every supplied position, so measure each route on its
    // own compacted returned surface; the full array would count cloth vertices.
    let (ordinary_positions, ordinary_triangles, _) = compact(&output, &ordinary_faces);
    let body_inside = inside(
        (&ordinary_positions, &ordinary_triangles),
        (&body_positions, &body.shapes[body_index].triangles),
    );
    let full_inside = inside(
        (&ordinary_positions, &ordinary_triangles),
        (&collision.mesh.positions, &collision.mesh.triangles),
    );
    let cloth_contacts = intersection_pairs(
        &output,
        &cloth_face_list,
        &collision.mesh.positions,
        &collision.mesh.triangles,
    );
    let (cloth_positions, cloth_triangles, _) = compact(&output, &cloth_face_list);
    let cloth_inside = inside(
        (&cloth_positions, &cloth_triangles),
        (&collision.mesh.positions, &collision.mesh.triangles),
    );
    let ordinary_vertices: BTreeSet<_> = ordinary_faces.iter().flatten().copied().collect();
    let max_ordinary_displacement = ordinary_vertices
        .iter()
        .map(|&vertex| {
            let vertex = vertex as usize;
            output[vertex]
                .into_iter()
                .zip(source[vertex])
                .map(|(after, before)| f64::from(after - before).powi(2))
                .sum::<f64>()
                .sqrt()
        })
        .fold(0.0_f64, f64::max);
    assert_eq!(
        report["regions"]["ordinary_triangles"],
        ordinary_faces.len()
    );
    assert_eq!(report["regions"]["cloth_triangles"], cloth_faces.len());
    assert_eq!(
        report["regions"]["ordinary"]["intersection_pairs"],
        body_contacts + feet_contacts,
        "report must describe the returned ordinary surface"
    );
    assert_eq!(report["regions"]["ordinary"]["inside_after"], full_inside);
    assert_eq!(
        report["regions"]["cloth_intersection_pairs"],
        cloth_contacts
    );
    assert_eq!(
        report["regions"]["intersection_pairs"],
        body_contacts + feet_contacts + cloth_contacts
    );
    assert_eq!(
        report["intersection_pairs"],
        report["regions"]["intersection_pairs"]
    );
    assert!(
        report.get("inside_after").is_none(),
        "region inside counts need their explicit scope"
    );

    let original_self: HashSet<_> = self_intersection_witnesses(&source, &robe.triangles)
        .into_iter()
        .collect();
    let returned_self = self_intersection_witnesses(&output, &robe.triangles);
    let new_self = returned_self
        .iter()
        .filter(|witness| !original_self.contains(witness))
        .count();
    let flips = flipped(&source, &output, &robe.triangles);
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("frozen Apachii frame");
    let overlap = compare(
        (&source, &robe.triangles),
        (&output, &robe.triangles),
        &frame,
    )
    .expect("fixed silhouette frame without cropping");

    let mut failures = Vec::new();
    if body_contacts + feet_contacts != 0 || body_inside != 0 || full_inside != 0 {
        failures.push(format!(
            "robe ordinary: body={body_contacts}, feet={feet_contacts}, inside={body_inside}/{full_inside}"
        ));
    }
    if max_cloth_error > 1e-4 {
        failures.push(format!("robe cloth: max frame error {max_cloth_error}"));
    }
    if new_self != 0 || flips != 0 {
        failures.push(format!(
            "robe construction: new_self={new_self}, flips={flips}"
        ));
    }
    if elapsed >= Duration::from_secs(60) {
        failures.push(format!("robe fit took {elapsed:?}"));
    }
    let measured = json!({
        "shape":"robe","vertices":output.len(),"triangles":faces.len(),
        "declared_source":declared.nif,"cloth_faces":cloth_faces.len(),
        "cloth_vertices":cloth_vertices.len(),"preserved_positions":preserved,
        "shared_boundary_aliases":aliases,"max_cloth_frame_error":max_cloth_error,
        "ordinary_faces":ordinary_faces.len(),"ordinary_body_contacts":body_contacts,
        "ordinary_feet_contacts":feet_contacts,"ordinary_body_inside":body_inside,
        "ordinary_full_inside":full_inside,
        "max_ordinary_displacement":max_ordinary_displacement,
        "cloth_rest_contacts":cloth_contacts,
        "cloth_rest_inside":cloth_inside,
        "cloth_rest_scope":"rest diagnostic for the Blender handoff; not ordinary clearance and not simulated clearance",
        "source_self":original_self.len(),"returned_self":returned_self.len(),
        "new_self":new_self,"flips":flips,"silhouette_iou":overlap.iou,
        "fit_seconds":elapsed.as_secs_f64(),
        "collision_reference":{"vertices":collision.mesh.positions.len(),
            "triangles":collision.mesh.triangles.len(),"parts":collision.parts},
        "pose_profile_handoff":{"applied":false,
            "reason":"the shared pose helper copies weights over the whole shape and has no region-scoped entry; a copied full rig must not drive declared cloth vertices"},
        "report":report,
    });
    eprintln!("FIT08_ROBE_GEOMETRY {measured}");
    assert!(
        failures.is_empty(),
        "mixed robe acceptance failed:\n{}",
        failures.join("\n")
    );
}
