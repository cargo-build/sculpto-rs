//! `export.actor` on the two default bodies: parts present and coloured, one
//! palette, the rest reproduced, and the clips reaching the geometry. @req DECL0008

use std::path::{Path, PathBuf};

use sculpto_engine::export_actor::{ActorLayer, ActorSource, ExportActorParams, run};
use sculpto_mesh::actor::{Actor, Pose};
use serde_json::Value;

/// The host fixture this gate reads; unset means the gate prints its SKIP line.
const FIXTURE: &str = "SCULPTO_ACTOR_FIXTURES";

fn fixture() -> Option<Value> {
    let path = std::env::var("SCULPTO_ACTOR_FIXTURES").ok()?;
    let text = std::fs::read_to_string(&path).expect("the declared fixture JSON");
    Some(serde_json::from_str(&text).expect("the fixture JSON parses"))
}

fn path(value: &Value, key: &str) -> PathBuf {
    PathBuf::from(
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("the fixture needs `{key}`")),
    )
}

/// Display-frame OBJ positions: the writer's glTF frame at the declared scale.
fn obj_positions(path: &Path) -> Vec<[f32; 3]> {
    let text = std::fs::read_to_string(path).expect("the declared OBJ");
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            if parts.next() != Some("v") {
                return None;
            }
            Some([
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
                parts.next()?.parse().ok()?,
            ])
        })
        .collect()
}

fn params(case: &Value, out_dir: &Path) -> ExportActorParams {
    ExportActorParams {
        nif: path(case, "nif"),
        body_shape: case["body_shape"].as_str().expect("body_shape").to_string(),
        sources: case
            .get("sources")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| ActorSource {
                        nif: entry.get("nif").and_then(Value::as_str).map(PathBuf::from),
                        shape: entry["shape"].as_str().expect("source shape").to_string(),
                        placement: entry.get("placement").filter(|value| !value.is_null()).map(
                            |value| {
                                serde_json::from_value(value.clone())
                                    .expect("the source placement matches the declaration")
                            },
                        ),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        body: path(case, "body"),
        layers: case["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .map(|layer| ActorLayer {
                order: layer["order"].as_u64().expect("order") as u32,
                obj: path(layer, "obj"),
            })
            .collect(),
        skeleton: Some(path(case, "skeleton")),
        clips: Some(path(case, "clips")),
        clip_names: case["clip_names"]
            .as_array()
            .expect("clip_names")
            .iter()
            .map(|name| name.as_str().expect("clip name").to_string())
            .collect(),
        units_per_metre: case.get("units_per_metre").and_then(Value::as_f64),
        rest_translation_tolerance: None,
        rest_scale_tolerance: None,
        rest_frame_tolerance_degrees: None,
        reach_radius: None,
        layer_reach_radius: None,
        smooth_iterations: None,
        source_reach_tolerance: None,
        extremity_bones: case["extremity_bones"]
            .as_array()
            .expect("extremity_bones")
            .iter()
            .map(|name| name.as_str().expect("bone").to_string())
            .collect(),
        out: out_dir.join(format!("{}.glb", case["name"].as_str().expect("name"))),
        report: Some(out_dir.join(format!("{}.json", case["name"].as_str().expect("name")))),
    }
}

#[test]
fn the_default_bodies_export_as_complete_coloured_actors() {
    let Some(fixture) = fixture() else {
        eprintln!("SKIP default actor export: {FIXTURE} is not set");
        return;
    };
    let out_dir = PathBuf::from(
        fixture
            .get("out_dir")
            .and_then(Value::as_str)
            .expect("the fixture needs `out_dir`"),
    );
    std::fs::create_dir_all(&out_dir).expect("the fixture output directory");
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(cases.len() >= 2, "both default bodies are declared");

    for case in cases {
        let label = case["name"].as_str().expect("name").to_string();
        let params = params(case, &out_dir);
        let report = run(&params).unwrap_or_else(|error| panic!("{label}: {error:?}"));

        // One actor: every declared part is a primitive with its own material.
        let parts = report["parts"].as_array().expect("parts");
        let expected_parts = case["expect"]["parts"]
            .as_u64()
            .expect("expected part count");
        assert_eq!(parts.len() as u64, expected_parts, "{label}: part count");
        assert_eq!(report["skin"]["skins"], 1, "{label}: one skin");
        assert_eq!(report["skin"]["parts"], expected_parts as usize);
        let mut total = 0;
        for part in parts {
            let vertices = part["vertices"].as_u64().expect("vertices") as usize;
            assert!(vertices > 0, "{label}: a part has no vertices");
            assert!(
                part["triangles"].as_u64().expect("triangles") > 0,
                "{label}: a part has no triangles"
            );
            total += vertices;
        }
        assert_eq!(
            report["palette"]["influences"]["vertices"], total,
            "{label}: every vertex is skinned"
        );
        assert!(
            report["palette"]["influences"]["widest"]
                .as_u64()
                .expect("widest")
                <= 4,
            "{label}: at most four influences"
        );

        // The review palette colours each part, and no part is the white default.
        let declared: Vec<&str> = case["expect"]["materials"]
            .as_array()
            .expect("expected materials")
            .iter()
            .map(|value| value.as_str().expect("material"))
            .collect();
        let written: Vec<&str> = parts
            .iter()
            .map(|part| part["material"].as_str().expect("material"))
            .collect();
        assert_eq!(written, declared, "{label}: part materials");
        for material in &written {
            assert_ne!(*material, "#FFFFFF", "{label}: a part is untextured white");
        }

        // Every garment layer is welded before the bind: its authored panels
        // joined, its seam boundary closed, and the audit record present.
        for part in parts.iter().filter(|part| part["role"] == "layer") {
            let seam = &part["seam_weld"];
            assert!(
                !seam.is_null(),
                "{label}: layer {} reports no seam weld",
                part["name"]
            );
            let merged = seam["merged_pairs"].as_u64().expect("merged pairs");
            assert!(
                merged > 0,
                "{label}: {} merged no panel pairs",
                part["name"]
            );
            assert_eq!(
                seam["dropped_faces"].as_u64().expect("dropped faces"),
                0,
                "{label}: {} lost a face to the weld",
                part["name"]
            );
            assert!(
                seam["components_after"].as_u64().expect("components after")
                    < seam["components_before"]
                        .as_u64()
                        .expect("components before"),
                "{label}: {} panels did not join",
                part["name"]
            );
        }

        // The extremities are bound, with the counts the fallback measured.
        let fallback: Vec<String> = report["palette"]["fallback_bones"]
            .as_array()
            .expect("fallback bones")
            .iter()
            .map(|value| value.as_str().expect("bone").to_string())
            .collect();
        for bone in case["expect"]["fallback"].as_array().expect("fallback") {
            let bone = bone.as_str().expect("bone");
            assert!(
                fallback.iter().any(|name| name == bone),
                "{label}: {bone} is a fallback candidate, have {fallback:?}"
            );
        }
        for bone in &params.extremity_bones {
            let bound: u64 = parts
                .iter()
                .flat_map(|part| {
                    part["fallback_bones"]
                        .as_array()
                        .expect("part fallback")
                        .iter()
                })
                .filter(|row| row["bone"].as_str() == Some(bone.as_str()))
                .map(|row| row["vertices"].as_u64().expect("vertices"))
                .sum();
            match bone.contains("Neck") {
                // The neck seam is the smoothed boundary, not an extremity.
                true => {}
                false => assert!(bound > 0, "{label}: nothing bound to {bone}"),
            }
        }

        // The rest proof holds at the written units.
        let rest = report["rest"]["max_vertex"].as_f64().expect("max_vertex");
        assert!(
            rest <= 1e-4,
            "{label}: rest reconstruction {rest:.3e} above the 1e-4 tolerance"
        );

        // Re-open the file as an Actor: the rest sample reproduces the input OBJ
        // positions, which are already in the written glTF frame at scale.
        let bytes = std::fs::read(&params.out).expect("the written GLB");
        let actor = Actor::from_glb(&bytes).unwrap_or_else(|error| panic!("{label}: {error:?}"));
        let names = actor.clip_names();
        assert_eq!(
            names.len(),
            params.clip_names.len(),
            "{label}: every declared clip is written"
        );
        let mesh_name = params.body_shape.clone();
        let sampled = actor
            .sample(&[&mesh_name], Pose::Rest)
            .unwrap_or_else(|error| panic!("{label}: {error:?}"));
        assert_eq!(sampled.parts.len(), expected_parts as usize);
        let source = obj_positions(&params.body);
        let body_part = &sampled.parts[0];
        // Every primitive samples the shared arena, so the body's own vertices
        // are its part's leading range.
        assert!(
            body_part.vertices.len() >= source.len(),
            "{label}: the body part samples {} vertices for {} body vertices",
            body_part.vertices.len(),
            source.len()
        );
        let scale = report["units"]["scale"].as_f64().expect("scale");
        let mut worst = 0.0_f64;
        for (index, expected) in source.iter().enumerate() {
            let actual = sampled.vertices[body_part.vertices.start + index];
            for axis in 0..3 {
                let gap = (f64::from(expected[axis]) * scale - actual[axis]).abs();
                worst = worst.max(gap);
            }
        }
        assert!(
            worst <= 1e-5,
            "{label}: the rest sample moved a body vertex by {worst:.3e}"
        );

        // A clip reaches the geometry: the feet swing in the moving clip.
        for clip in &params.clip_names {
            let posed = actor
                .sample(
                    &[&mesh_name],
                    Pose::Clip {
                        name: clip,
                        time: 0.5,
                    },
                )
                .unwrap_or_else(|error| panic!("{label}: {error:?}"));
            let mut moved = 0;
            let mut largest = 0.0_f64;
            for (expected, actual) in sampled.vertices.iter().zip(&posed.vertices) {
                let gap = ((expected[0] - actual[0]).powi(2)
                    + (expected[1] - actual[1]).powi(2)
                    + (expected[2] - actual[2]).powi(2))
                .sqrt();
                if gap > 1e-4 {
                    moved += 1;
                }
                largest = largest.max(gap);
            }
            assert!(
                largest > 1e-3,
                "{label}: clip {clip} moved no vertex (largest {largest:.3e})"
            );
            eprintln!(
                "{label}: clip {clip} moved {moved}/{} vertices, largest {largest:.6} m",
                sampled.vertices.len()
            );
        }
        eprintln!(
            "{label}: {} parts, {} vertices, {} palette bones, rest {rest:.3e}, sha256 {}",
            parts.len(),
            total,
            report["palette"]["bones"],
            report["sha256"].as_str().expect("sha256")
        );
    }
}
