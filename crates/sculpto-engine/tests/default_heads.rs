//! SHP-02 acceptance: default replaceable heads per configured body.
//!
//! Two layers:
//! - Synthetic controls run everywhere: a compatible head replacement preserves
//!   the body exactly (array equality plus a fingerprint hash), and missing /
//!   incompatible heads are named refusals.
//! - A configured-fixture test loads `config/bodies.json` and exercises the real
//!   default heads when the fixture roots are present
//!   (`SCULPTO_KITS_HOME`, `SCULPTO_HEAD_HOME`, `SCULPTO_BODYSLIDE_HOME`);
//!   otherwise it skips, like the other fixture-gated engine tests.
//!
//! The real bodies are OBJ geometry (no skin), so rig/bone order and material
//! names are reported as not verifiable headlessly; the synthetic skinned pair
//! exercises the rig-match and rig-mismatch paths the OBJ fixtures cannot.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::role::ShapeRole;
use sculpto_core::skin::SkinWeights;
use sculpto_engine::ErrorKind;
use sculpto_engine::ops::{
    AssignedShape, BodyDescriptor, CheckStatus, RoleBundle, head_compatibility, load_bodies_config,
    load_shape, resolve_default_head,
};

/// The shared skeleton every synthetic skinned part repeats, in bind order.
const SKELETON: [&str; 3] = ["root", "spine", "head"];

fn skin(bones: &[&str], rows: usize) -> Skin {
    Skin {
        bones: bones.iter().map(|bone| (*bone).to_string()).collect(),
        bind_inverse: Vec::new(),
        influences: SkinWeights::from_quads(
            &vec![[0u16, 0, 0, 0]; rows],
            &vec![[1.0f32, 0.0, 0.0, 0.0]; rows],
        )
        .expect("one influence per vertex"),
    }
}

/// A small skinned quad used for the body, hands and feet.
fn part(name: &str, bones: &[&str], partition: u16) -> MeshAsset {
    MeshAsset {
        name: name.into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(partition); 2],
        skin: Some(skin(bones, 4)),
        ..MeshAsset::default()
    }
}

/// An open tube with a neck seam: the bottom ring (z = 0) is a boundary loop,
/// so the head has a seam to attach. `height` lets a replacement differ from
/// the default geometrically.
fn tube_head(name: &str, bones: &[&str], height: f32) -> MeshAsset {
    let n = 8usize;
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    for ring in 0..2 {
        for i in 0..n {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / n as f32;
            positions.push([angle.cos(), angle.sin(), ring as f32 * height]);
            uvs.push([i as f32 / n as f32, ring as f32]);
        }
    }
    let mut triangles = Vec::new();
    for i in 0..n {
        let b0 = i as u32;
        let b1 = ((i + 1) % n) as u32;
        let t0 = (i + n) as u32;
        let t1 = ((i + 1) % n + n) as u32;
        triangles.push([b0, b1, t1]);
        triangles.push([b0, t1, t0]);
    }
    MeshAsset {
        name: name.into(),
        uvs,
        triangle_partitions: vec![PartitionId(30); triangles.len()],
        triangles,
        skin: Some(skin(bones, positions.len())),
        positions,
        ..MeshAsset::default()
    }
}

/// A watertight tetrahedron: no boundary, hence no neck seam.
fn closed_head(name: &str, bones: &[&str]) -> MeshAsset {
    MeshAsset {
        name: name.into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.5, 0.5]],
        triangles: vec![[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]],
        triangle_partitions: vec![PartitionId(30); 4],
        skin: Some(skin(bones, 4)),
        ..MeshAsset::default()
    }
}

/// FNV-1a over the body's positions, triangles and skin: a stable fingerprint
/// for the before/after preservation proof.
fn fingerprint(mesh: &MeshAsset) -> u64 {
    let mut bytes = Vec::new();
    for position in &mesh.positions {
        for value in position {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    for triangle in &mesh.triangles {
        for index in triangle {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
    }
    if let Some(skin) = &mesh.skin {
        for bone in &skin.bones {
            bytes.extend_from_slice(bone.as_bytes());
            bytes.push(0);
        }
        for row in 0..skin.influences.rows() {
            if let Some(row) = skin.influences.row(row) {
                for joint in row.joints {
                    bytes.extend_from_slice(&joint.to_le_bytes());
                }
                for weight in row.weights {
                    bytes.extend_from_slice(&weight.to_bits().to_le_bytes());
                }
            }
        }
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn shape(bundle: &RoleBundle, role: ShapeRole) -> &MeshAsset {
    &bundle
        .shapes
        .iter()
        .find(|shape| shape.role == role)
        .expect("role present")
        .mesh
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "pass",
        CheckStatus::Fail => "FAIL",
        CheckStatus::NotVerifiable => "not-verifiable",
    }
}

#[test]
fn synthetic_replacement_preserves_body_geometry_and_rig() {
    let body = part("body", &SKELETON, 32);
    let hands = part("hands", &SKELETON, 33);
    let feet = part("feet", &SKELETON, 37);
    let default_head = tube_head("head_default", &SKELETON, 10.0);
    let bundle = RoleBundle::new(vec![
        AssignedShape {
            role: ShapeRole::Body,
            mesh: body.clone(),
        },
        AssignedShape {
            role: ShapeRole::Hands,
            mesh: hands.clone(),
        },
        AssignedShape {
            role: ShapeRole::Feet,
            mesh: feet.clone(),
        },
        AssignedShape {
            role: ShapeRole::Head,
            mesh: default_head.clone(),
        },
    ])
    .expect("unique roles, one skeleton");
    bundle
        .validate_required_parts()
        .expect("complete character");

    // A geometrically different, compatible replacement head.
    let replacement = tube_head("head_replacement", &SKELETON, 12.0);
    let report = head_compatibility(&body, &replacement);
    assert!(
        report.is_compatible(),
        "synthetic compatible head must pass"
    );

    let before = fingerprint(&body);
    let after = bundle
        .replace_head(replacement, &report)
        .expect("replacement");

    assert_eq!(shape(&after, ShapeRole::Body).positions, body.positions);
    assert_eq!(shape(&after, ShapeRole::Body).triangles, body.triangles);
    assert_eq!(shape(&after, ShapeRole::Body).skin, body.skin);
    assert_eq!(fingerprint(shape(&after, ShapeRole::Body)), before);
    assert_eq!(shape(&after, ShapeRole::Hands), &hands);
    assert_eq!(shape(&after, ShapeRole::Feet), &feet);

    let head = after.head().expect("head slot");
    assert_eq!(head.name, "head_replacement");
    assert_ne!(
        head.positions, default_head.positions,
        "the head actually swapped"
    );
    assert_eq!(
        head.triangle_partitions,
        vec![ShapeRole::Head.default_partition(); head.triangles.len()],
        "the replacement is stamped with the head partition"
    );
    println!(
        "synthetic body fingerprint before={before:016x} after={:016x} (preserved)",
        fingerprint(shape(&after, ShapeRole::Body))
    );
}

#[test]
fn missing_configured_head_is_a_named_refusal() {
    let descriptor = BodyDescriptor {
        name: "test body".into(),
        family: "test".into(),
        body: PathBuf::from("/nonexistent/shp02/body.obj"),
        head: PathBuf::from("/nonexistent/shp02/head.obj"),
    };
    let error = resolve_default_head(&descriptor).expect_err("missing head must refuse");
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.code(), "head_missing");
    assert!(
        error.message().contains("/nonexistent/shp02/head.obj"),
        "the refusal names the missing path: {}",
        error.message()
    );
}

#[test]
fn incompatible_head_by_rig_is_a_named_refusal() {
    let body = part("body", &SKELETON, 32);
    let default_head = tube_head("head_default", &SKELETON, 10.0);
    let bundle = RoleBundle::new(vec![
        AssignedShape {
            role: ShapeRole::Body,
            mesh: body.clone(),
        },
        AssignedShape {
            role: ShapeRole::Head,
            mesh: default_head,
        },
    ])
    .expect("body + head slot");

    // Same geometry, but a bone palette that differs from the body's.
    let bad = tube_head("head_bad_rig", &["root", "spine", "tail"], 10.0);
    let report = head_compatibility(&body, &bad);
    assert!(!report.is_compatible());
    assert!(
        report
            .failures()
            .iter()
            .any(|check| check.name == "rig_bone_order"),
        "the rig check must fail"
    );

    let error = bundle
        .replace_head(bad, &report)
        .expect_err("incompatible head");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "head_incompatible");
    assert!(
        error.message().contains("rig_bone_order"),
        "the refusal names the failed check: {}",
        error.message()
    );
}

#[test]
fn watertight_head_without_neck_seam_is_a_named_refusal() {
    let body = part("body", &SKELETON, 32);
    let closed = closed_head("head_closed", &SKELETON);
    let report = head_compatibility(&body, &closed);
    assert!(!report.is_compatible());
    assert!(
        report
            .failures()
            .iter()
            .any(|check| check.name == "neck_seam"),
        "a closed head has no neck seam"
    );
}

#[test]
fn required_parts_gate_names_the_missing_head() {
    let bundle = RoleBundle::new(vec![
        AssignedShape {
            role: ShapeRole::Body,
            mesh: part("body", &SKELETON, 32),
        },
        AssignedShape {
            role: ShapeRole::Hands,
            mesh: part("hands", &SKELETON, 33),
        },
        AssignedShape {
            role: ShapeRole::Feet,
            mesh: part("feet", &SKELETON, 37),
        },
    ])
    .expect("body + hands + feet");
    let error = bundle
        .validate_required_parts()
        .expect_err("head is missing");
    assert_eq!(error.code(), "missing_required_roles");
    assert!(error.message().contains("head"), "{}", error.message());
}

#[test]
fn configured_default_heads_resolve_and_replace_preserving_body() {
    let names = [
        "SCULPTO_KITS_HOME",
        "SCULPTO_HEAD_HOME",
        "SCULPTO_BODYSLIDE_HOME",
    ];
    let missing: Vec<&str> = names
        .iter()
        .filter(|name| std::env::var_os(name).is_none())
        .copied()
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "SKIP configured_default_heads_resolve_and_replace_preserving_body: missing {missing:?}"
        );
        return;
    }
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/bodies.json");
    let config = load_bodies_config(&config_path).expect("config loads with fixture roots set");
    assert_eq!(config.bodies.len(), 2, "both default bodies are configured");

    for descriptor in &config.bodies {
        let body = load_shape(&descriptor.body).expect("configured body loads");
        let head = resolve_default_head(descriptor).expect("configured default head resolves");
        let report = head_compatibility(&body, &head);

        println!(
            "== body `{}` (family `{}`)",
            descriptor.name, descriptor.family
        );
        println!(
            "   body {} v / {} tris, head `{}` {} v / {} tris",
            body.positions.len(),
            body.triangles.len(),
            head.name,
            head.positions.len(),
            head.triangles.len()
        );
        for check in &report.checks {
            println!(
                "   [{}] {}: {}",
                status_label(check.status),
                check.name,
                check.detail
            );
        }
        assert!(
            report.is_compatible(),
            "configured default head for `{}` must be compatible",
            descriptor.name
        );

        // Replace through the explicit head slot with a re-resolved copy of the
        // configured head (one head is configured per body); the body must come
        // out byte-identical. A geometric cross-head swap is proven by the
        // synthetic control above.
        let bundle = RoleBundle::new(vec![
            AssignedShape {
                role: ShapeRole::Body,
                mesh: body.clone(),
            },
            AssignedShape {
                role: ShapeRole::Head,
                mesh: head.clone(),
            },
        ])
        .expect("body + head slot");
        let before = fingerprint(&body);
        let replacement = resolve_default_head(descriptor).expect("head re-resolves");
        let replacement_report = head_compatibility(&body, &replacement);
        let after = bundle
            .replace_head(replacement, &replacement_report)
            .expect("replacement preserves the bundle");

        let after_body = shape(&after, ShapeRole::Body);
        assert_eq!(after_body.positions, body.positions);
        assert_eq!(after_body.triangles, body.triangles);
        assert_eq!(after_body.skin, body.skin);
        let after_hash = fingerprint(after_body);
        assert_eq!(after_hash, before, "body fingerprint must be unchanged");
        println!("   body fingerprint before={before:016x} after={after_hash:016x} (preserved)");
    }
}

/// The config loader refuses an unset fixture root by name rather than guessing.
#[test]
fn config_with_unset_root_is_a_named_refusal() {
    // A tiny inline config referencing a variable that is not defined.
    let dir = std::env::temp_dir().join(format!("shp02-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bodies.json");
    std::fs::write(
        &path,
        r#"{"bodies":[{"name":"b","family":"f","body":"${SHP02_DEFINITELY_UNSET}/body.obj","head":"h.obj"}]}"#,
    )
    .expect("write config");
    let error = load_bodies_config(&path).expect_err("unset variable must refuse");
    assert_eq!(error.code(), "config_unresolved");
    assert!(
        error.message().contains("SHP02_DEFINITELY_UNSET"),
        "{}",
        error.message()
    );
    std::fs::remove_file(&path).ok();
    std::fs::remove_dir(&dir).ok();
}

/// A duplicate role is refused; a character has exactly one head slot.
#[test]
fn duplicate_head_role_is_refused() {
    let error = RoleBundle::new(vec![
        AssignedShape {
            role: ShapeRole::Body,
            mesh: part("body", &SKELETON, 32),
        },
        AssignedShape {
            role: ShapeRole::Head,
            mesh: tube_head("head_a", &SKELETON, 10.0),
        },
        AssignedShape {
            role: ShapeRole::Head,
            mesh: tube_head("head_b", &SKELETON, 10.0),
        },
    ])
    .expect_err("two heads");
    assert_eq!(error.code(), "duplicate_role");
}
