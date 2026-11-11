//! Native exit status does not establish original-frame garment acceptance. @req DECL0013

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use glam::DVec3;
use sculpto_mesh::check::{intersection_pair_list, nearest_distances, self_intersection_witnesses};
use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::silhouette::{OrthographicFrame, RasterSettings, compare};
use sculpto_mesh::wrap::{FitOptions, fit_to_body};
use serde_json::json;

#[test]
fn configured_original_standoffs_contract_native_initializer() {
    let Some(root) = std::env::var_os("SCULPTO_NATIVE_COMPOSITION_FIXTURE") else {
        eprintln!("SKIP native composition: SCULPTO_NATIVE_COMPOSITION_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(root.join(name)).expect("configured composition artifact"),
        )
        .expect("composition JSON")
    };
    let original = read("original.json");
    let native = read("candidate.json");
    let source: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("source positions");
    let current: Vec<[f32; 3]> =
        serde_json::from_value(native["positions"].clone()).expect("native positions");
    let faces: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("original faces");
    let body: Vec<[f32; 3]> = serde_json::from_value(original["reference"]["positions"].clone())
        .expect("actual body and feet");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("actual reference faces");
    let maximum = |points: &[[f32; 3]]| {
        nearest_distances(points, &body, &body_faces)
            .into_iter()
            .fold(0.0_f32, f32::max)
    };
    let original_reach = maximum(&source);
    let native_reach = maximum(&current);
    let radius = 10.0_f32.max(original_reach.max(native_reach) + 0.05);
    let original_signed: Vec<_> =
        sculpto_mesh::clip_fix::signed_distances_to_body((&body, &body_faces), &source, radius)
            .expect("signed source audit")
            .into_iter()
            .map(|value| value.expect("measured radius reaches every source point"))
            .collect();
    let options = FitOptions {
        gap_mode: sculpto_mesh::wrap::GapMode::FromShape,
        min_gap: 0.05,
        relief_smoothing: 10,
        flip_rings: 0,
        max_distance: radius,
        ..FitOptions::default()
    };
    let started = std::time::Instant::now();
    let (fitted, fit) = sculpto_mesh::wrap::fit_to_body_from(
        (&current, &faces),
        (&body, &body_faces),
        (&body, &body_faces),
        Some(&source),
        None,
        &options,
    )
    .expect("one declared FromShape composition");
    let fit_seconds = started.elapsed().as_secs_f64();
    let started = std::time::Instant::now();
    let (repaired, repair) = sculpto_mesh::wrap::repair::repair_after_fit(
        (&fitted, &faces),
        (&body, &body_faces),
        None,
        &options,
    )
    .expect("existing guarded post-fit repair");
    let repair_seconds = started.elapsed().as_secs_f64();
    let output = root.join("composition01");
    std::fs::create_dir(&output).expect("fresh composition output");
    for name in ["original.json", "admission.json", "native-physical.obj"] {
        std::fs::copy(root.join(name), output.join(name))
            .expect("copy immutable qualification inputs");
    }
    std::fs::write(
        output.join("raw-from-shape.json"),
        serde_json::to_vec(&json!({"positions":fitted})).expect("raw JSON"),
    )
    .expect("raw candidate");
    std::fs::write(
        output.join("candidate.json"),
        serde_json::to_vec(&json!({"positions":repaired})).expect("candidate JSON"),
    )
    .expect("repaired candidate");
    let receipt = json!({"recipe":"ONE FromShape call, native current positions as rest, original source as gap_source, actual full body+feet as both gap_from and target; then repair_after_fit",
        "regularization_rest":"native last-saved70, explicitly authorized by coordinator",
        "final_quality_authority":"unchanged original source topology/orientation/strain/silhouette",
        "options":{"gap_mode":"FromShape","min_gap":0.05,"relief_smoothing":10,"flip_rings":0,"max_distance":radius,"otherwise":"FitOptions::default"},
        "source_maximum_nearest_distance":original_reach,"native_maximum_nearest_distance":native_reach,
        "radius_rule":"max(default10,max(source,native) distance + min_gap.05)",
        "source_signed_audit_owner":"clip_fix::signed_distances_to_body; FromShape internally resolves its oriented-body signed_standoffs, clamps to min_gap, then adds configured native-rest relief",
        "source_signed_audit":original_signed,"source_negative_standoffs":original_signed.iter().filter(|&&gap|gap<0.0).count(),
        "source_minimum_standoff":original_signed.iter().copied().fold(f32::INFINITY,f32::min),
        "source_maximum_standoff":original_signed.iter().copied().fold(f32::NEG_INFINITY,f32::max),
        "fit_seconds":fit_seconds,"repair_seconds":repair_seconds,"fit_report":format!("{fit:?}"),"repair_report":repair,
        "qualification":"not claimed; independent original-source measurement follows"});
    std::fs::write(
        output.join("composition-receipt.json"),
        serde_json::to_vec_pretty(&receipt).expect("receipt JSON"),
    )
    .expect("receipt");
    eprintln!(
        "NATIVE_COMPOSITION_EXECUTED source_reach={original_reach} native_reach={native_reach} radius={radius} fit_seconds={fit_seconds} repair_seconds={repair_seconds} fit={fit:?} repair={repair:?}; independent qualification required"
    );
}

#[test]
fn configured_native_avatar_matches_recorded_target_surface() {
    let Some(root) = std::env::var_os("SCULPTO_NATIVE_AVATAR_FIXTURE") else {
        eprintln!("SKIP native avatar: SCULPTO_NATIVE_AVATAR_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |name: &str| -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let data: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(name)).expect("configured avatar artifact"),
        )
        .expect("avatar JSON");
        (
            serde_json::from_value(data["positions"].clone()).expect("positions"),
            serde_json::from_value(data["triangles"].clone()).expect("triangles"),
        )
    };
    let (avatar, avatar_faces) = read("avatar.json");
    let (target, target_faces) = read("target.json");
    let forward = nearest_distances(&avatar, &target, &target_faces)
        .into_iter()
        .fold(0.0_f32, f32::max);
    let reverse = nearest_distances(&target, &avatar, &avatar_faces)
        .into_iter()
        .fold(0.0_f32, f32::max);
    let measured = json!({"paired_avatar_vertices":avatar.len(),"paired_avatar_faces":avatar_faces.len(),
        "recorded_target_vertices":target.len(),"recorded_target_faces":target_faces.len(),
        "paired_to_target_max_vertex_surface_distance":forward,"target_to_paired_max_vertex_surface_distance":reverse,
        "scope":"bidirectional vertex-to-surface agreement under the recorded triangle subdivision; full original body+feet contacts are measured separately"});
    std::fs::write(
        root.join("avatar-qualification.json"),
        serde_json::to_vec_pretty(&measured).expect("avatar report"),
    )
    .expect("write avatar report");
    eprintln!("NATIVE_AVATAR {measured}");
    assert!(
        forward < 1e-4 && reverse < 1e-4,
        "decoded paired avatar differs from target"
    );
}

#[test]
fn configured_native_candidate_passes_original_geometry_gates() {
    let Some(root) = std::env::var_os("SCULPTO_NATIVE_CANDIDATE_FIXTURE") else {
        eprintln!("SKIP native candidate: SCULPTO_NATIVE_CANDIDATE_FIXTURE unset");
        return;
    };
    let root = Path::new(&root);
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root.join(name)).expect("configured artifact"))
            .expect("artifact JSON")
    };
    let original = read("original.json");
    let candidate = read("candidate.json");
    let admission = read("admission.json");
    let source: Vec<[f32; 3]> =
        serde_json::from_value(original["positions"].clone()).expect("source positions");
    let output: Vec<[f32; 3]> =
        serde_json::from_value(candidate["positions"].clone()).expect("candidate positions");
    let faces: Vec<[u32; 3]> =
        serde_json::from_value(original["triangles"].clone()).expect("source faces");
    let admitted_faces: Vec<[u32; 3]> =
        serde_json::from_value(admission["render_faces"].clone()).expect("admitted faces");
    let mapping: Vec<usize> =
        serde_json::from_value(admission["render_to_physical"].clone()).expect("render mapping");
    let body: Vec<[f32; 3]> =
        serde_json::from_value(original["reference"]["positions"].clone()).expect("full reference");
    let body_faces: Vec<[u32; 3]> =
        serde_json::from_value(original["reference"]["triangles"].clone())
            .expect("reference faces");
    assert_eq!((source.len(), output.len(), faces.len()), (392, 392, 632));
    assert_eq!(
        faces, admitted_faces,
        "original ordered faces survive native admission"
    );
    assert_eq!((body.len(), body_faces.len()), (5825, 10488));
    let mut representatives = vec![None; 326];
    mapping
        .iter()
        .enumerate()
        .for_each(|(index, &group)| match representatives[group] {
            None => representatives[group] = Some(output[index]),
            Some(point) => assert_eq!(
                point, output[index],
                "physical seam copies remain identical"
            ),
        });
    assert!(representatives.iter().all(Option::is_some));
    let native = std::fs::read_to_string(root.join("native-physical.obj"))
        .expect("physical native artifact");
    let native_faces: Vec<[usize; 3]> = native
        .lines()
        .filter(|line| line.starts_with("f "))
        .map(|line| {
            let indices: Vec<usize> = line
                .split_whitespace()
                .skip(1)
                .map(|item| {
                    item.split('/')
                        .next()
                        .expect("OBJ index")
                        .parse::<usize>()
                        .expect("OBJ integer")
                        - 1
                })
                .collect();
            indices.try_into().expect("native triangles")
        })
        .collect();
    assert_eq!(
        native_faces,
        faces
            .iter()
            .map(|face| face.map(|v| mapping[v as usize]))
            .collect::<Vec<_>>(),
        "native output ordered physical faces"
    );
    let pairs = intersection_pair_list(&output, &faces, &body, &body_faces);
    let original_self: HashSet<_> = self_intersection_witnesses(&source, &faces)
        .into_iter()
        .collect();
    let self_contacts = self_intersection_witnesses(&output, &faces);
    let new_self = self_contacts
        .iter()
        .filter(|w| !original_self.contains(w))
        .count();
    let vector = |p: [f32; 3]| DVec3::from(p.map(f64::from));
    let normal = |p: &[[f32; 3]], face: [u32; 3]| {
        let [a, b, c] = face.map(|v| vector(p[v as usize]));
        (b - a).cross(c - a)
    };
    let flipped: Vec<_> = faces
        .iter()
        .enumerate()
        .filter_map(|(index, &face)| {
            (normal(&source, face).dot(normal(&output, face)) < 0.0).then_some(index)
        })
        .collect();
    let distances = nearest_distances(&output, &body, &body_faces);
    let max_distance = distances.iter().copied().fold(0.0_f32, f32::max);
    let radius = 10.0_f32.max(max_distance + 0.05);
    let locks = VertexMask::from_values(vec![1.0; output.len()]).expect("measurement locks");
    let options = FitOptions {
        max_distance: radius,
        ..FitOptions::default()
    };
    let (unchanged, diagnostic) = fit_to_body(
        (&output, &faces),
        (&body, &body_faces),
        Some(&locks),
        &options,
    )
    .expect("locked inside query");
    assert_eq!(unchanged, output);
    let edges: BTreeSet<_> = faces
        .iter()
        .flat_map(|&[a, b, c]| {
            [
                (a.min(b), a.max(b)),
                (b.min(c), b.max(c)),
                (c.min(a), c.max(a)),
            ]
        })
        .collect();
    let strain = edges
        .iter()
        .map(|&(a, b)| {
            let length = |p: &[[f32; 3]]| vector(p[a as usize]).distance(vector(p[b as usize]));
            let old = length(&source);
            let new = length(&output);
            ((new / old).max(old / new) - 1.0, [a, b], old, new)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .expect("source edges");
    let bounds = [
        output
            .iter()
            .map(|&p| vector(p))
            .fold(DVec3::splat(f64::INFINITY), DVec3::min)
            .to_array(),
        output
            .iter()
            .map(|&p| vector(p))
            .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max)
            .to_array(),
    ];
    let frame = OrthographicFrame::new([[-43.0, 43.0], [-8.0, 126.0]], RasterSettings::default())
        .expect("original fixed frame");
    let silhouette = compare((&source, &faces), (&output, &faces), &frame);
    let measured = json!({"vertices":output.len(),"faces":faces.len(),"ordered_topology":true,
        "body_contacts":pairs.iter().filter(|(_,body)|*body<6516).count(),"feet_contacts":pairs.iter().filter(|(_,body)|*body>=6516).count(),
        "contacts":pairs.len(),"contact_pairs":pairs,"inside_groups":diagnostic.inside_after,
        "source_self":original_self.len(),"returned_self":self_contacts.len(),"new_self":new_self,
        "self_witnesses":self_contacts.iter().map(|w|json!({"left":w.left,"right":w.right,"kind":format!("{:?}",w.kind)})).collect::<Vec<_>>(),
        "flipped_faces":flipped,"flips":flipped.len(),"bounds":bounds,"maximum_nearest_distance":max_distance,"inside_query_radius":radius,
        "max_source_symmetric_strain":strain.0,"strain_edge":strain.1,"source_edge_length":strain.2,"returned_edge_length":strain.3,
        "source_edge_points":strain.1.map(|v|source[v as usize]),"returned_edge_points":strain.1.map(|v|output[v as usize]),
        "silhouette":match &silhouette {Ok(overlap)=>json!({"iou":overlap.iou}),Err(error)=>json!({"error":error.to_string()})},
        "silhouette_baseline":0.8883417222146647});
    std::fs::write(
        root.join("qualification.json"),
        serde_json::to_vec_pretty(&measured).expect("measurement JSON"),
    )
    .expect("measurement artifact");
    eprintln!(
        "NATIVE_CANDIDATE contacts={} inside={} self={} flips={} strain={} bounds={bounds:?} silhouette={silhouette:?}",
        pairs.len(),
        diagnostic.inside_after,
        new_self,
        flipped.len(),
        strain.0
    );
    assert_eq!(
        (
            pairs.len(),
            diagnostic.inside_after,
            new_self,
            flipped.len()
        ),
        (0, 0, 0, 0)
    );
    assert!(silhouette.expect("fixed-frame qualification").iou >= 0.888_341_722_214_664_7);
}
