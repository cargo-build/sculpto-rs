//! G06 kind gate and G08 both genders: a synthetic female skirt and a
//! synthetic male pair of trousers are written by `sculpto_gltf::write_glb`,
//! read by `read_raw`, classified against their synthetic bodies with the
//! fitted path's rules, and handed to the fit path. The kinds must differ and
//! both fits must preserve vertex counts. No private fixture is involved.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId};
use sculpto_gltf::write_glb;
use sculpto_mesh::classify::{ClassifyError, GarmentKind};
use sculpto_mesh::conform::Conform;
use sculpto_mesh::intake_gltf::{KindError, RawGarment, ScaleReport, ScaleRule, read_raw};

/// The classifier tests' centimetre mannequin shrunk to a 1.70 m body
/// (the geometry of `tests/classify.rs`): every coordinate becomes metres, so
/// intake reads the `ScaleRule::Metres` band while every classification
/// fraction and ratio is unchanged.
const UNIT: f32 = 0.017;
const SEGMENTS: usize = 16;

/// A prism tube offset in the section plane (`tests/classify.rs:12`).
fn tube(cx: f32, cy: f32, radius: f32, heights: &[f32]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = prism(radius, heights, SEGMENTS);
    for position in &mut positions {
        position[0] += cx;
        position[1] += cy;
    }
    triangles.clear();
    let levels = heights.len();
    for level in 1..levels {
        for index in 0..SEGMENTS {
            let current = (
                (level * SEGMENTS + index) as u32,
                (level * SEGMENTS + (index + 1) % SEGMENTS) as u32,
            );
            let below = (current.0 - SEGMENTS as u32, current.1 - SEGMENTS as u32);
            triangles.push([below.0, below.1, current.0]);
            triangles.push([below.1, current.1, current.0]);
        }
    }
    (positions, triangles)
}

fn prism(radius: f32, heights: &[f32], segments: usize) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    for height in heights {
        for index in 0..segments {
            let angle = std::f32::consts::TAU * index as f32 / segments as f32;
            positions.push([radius * angle.cos(), radius * angle.sin(), *height]);
        }
    }
    (positions, Vec::new())
}

/// Merge meshes, reindexing the second into the first (`tests/classify.rs:69`).
fn merge(
    (mut left_positions, mut left_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
    (right_positions, right_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = left_positions.len() as u32;
    left_positions.extend(right_positions);
    left_triangles.extend(
        right_triangles
            .into_iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );
    (left_positions, left_triangles)
}

/// Bridge the top rings of two tubes into one component: the jumpsuit crotch
/// (`tests/classify.rs:185`).
fn bridged(
    (mut left_positions, mut left_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
    (right_positions, right_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = left_positions.len() as u32;
    let right_top_ring = offset + (right_positions.len() - SEGMENTS) as u32;
    left_positions.extend(right_positions);
    left_triangles.extend(
        right_triangles
            .into_iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );
    for index in 0..SEGMENTS {
        let next = (index + 1) % SEGMENTS;
        let left_top = offset - SEGMENTS as u32 + index as u32;
        let left_next = offset - SEGMENTS as u32 + next as u32;
        let right_top = right_top_ring + index as u32;
        let right_next = right_top_ring + next as u32;
        left_triangles.push([left_top, left_next, right_top]);
        left_triangles.push([left_next, right_next, right_top]);
    }
    (left_positions, left_triangles)
}

fn heights(z0: f32, z1: f32, rings: usize) -> Vec<f32> {
    (0..rings)
        .map(|index| z0 + (z1 - z0) * index as f32 / (rings - 1) as f32)
        .collect()
}

/// Scale one centimetre-space mesh into the 1.70 m body frame.
fn shrink(
    (positions, triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        positions
            .iter()
            .map(|position| position.map(|value| value * UNIT))
            .collect(),
        triangles,
    )
}

/// Torso (z 45..85, r `torso`), head (z 86..100, r 6) and two legs (z 0..45,
/// r `leg` at x = ±`stride`): 100 units tall before [`UNIT`].
fn mannequin(torso: f32, stride: f32, leg: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let torso = tube(0.0, 0.0, torso, &heights(45.0, 85.0, 3));
    let head = tube(0.0, 0.0, 6.0, &heights(86.0, 100.0, 2));
    let legs = merge(
        tube(-stride, 0.0, leg, &[0.0, 22.0, 45.0]),
        tube(stride, 0.0, leg, &[0.0, 22.0, 45.0]),
    );
    shrink(merge(merge(torso, head), legs))
}

/// The lady mannequin: the classifier tests' proportions (torso r 10, legs
/// r 4 at x = ±6).
fn female() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    mannequin(10.0, 6.0, 4.0)
}

/// The gentleman mannequin: broader torso (r 13), longer-set thicker legs
/// (r 5 at x = ±7), same 1.70 m height.
fn male() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    mannequin(13.0, 7.0, 5.0)
}

/// A skirt-like tube around both legs, hip to mid-thigh: the female fixture.
fn skirt_like() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    shrink(tube(0.0, 0.0, 16.0, &heights(20.0, 62.0, 5)))
}

/// A trouser-like pair of leg columns, ankle to above the hip: the male
/// fixture.
fn trouser_like() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    shrink(merge(
        tube(-7.0, 0.0, 6.0, &heights(5.0, 50.0, 4)),
        tube(7.0, 0.0, 6.0, &heights(5.0, 50.0, 4)),
    ))
}

/// A jumpsuit silhouette: bridged leg columns reaching above the trouser
/// line, the classifier's undecided shape (`tests/classify.rs:210`).
fn jumpsuit_like() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    shrink(bridged(
        tube(-6.0, 0.0, 5.0, &heights(5.0, 60.0, 4)),
        tube(6.0, 0.0, 5.0, &heights(5.0, 60.0, 4)),
    ))
}

fn garment_asset(name: &str, (positions, triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>)) -> MeshAsset {
    MeshAsset {
        name: name.into(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    }
}

/// A hand-built intake read, for the refusal cases the reader cannot produce
/// (the pattern of `tests/intake_fit.rs:140`).
fn raw_garment(positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> RawGarment {
    RawGarment {
        name: "garment".into(),
        positions,
        normals: Vec::new(),
        uvs: Vec::new(),
        triangles,
        material: MaterialSlot {
            name: "default".into(),
            ..MaterialSlot::default()
        },
        scale: ScaleReport {
            rule: ScaleRule::Metres,
            source_extent: 1.0,
            factor: 1.0,
        },
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("sculpto-intake-kinds-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp home");
    root
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    std::fs::write(path, bytes).expect("write fixture");
    path.to_owned()
}

/// Read a written GLB back through the intake and classify it against its
/// body: the G06 gate between `read_raw` and the G05 hand-off.
fn read_and_classify(
    path: &Path,
    body: &[[f32; 3]],
    body_triangles: &[[u32; 3]],
) -> (RawGarment, GarmentKind) {
    let raw = read_raw(path).expect("intake reads the written GLB");
    assert_eq!(
        raw.scale.rule,
        ScaleRule::Metres,
        "the metre-scale fixture must not be rescaled"
    );
    let kind = raw
        .classify(body, body_triangles)
        .expect("the synthetic garment has a decided kind");
    (raw, kind)
}

#[test]
fn female_skirt_and_male_trousers_classify_differently_and_fit() {
    let root = temp_root("genders");
    let (female_body, female_triangles) = female();
    let (male_body, male_triangles) = male();

    let female_bytes =
        write_glb(&garment_asset("skirt", skirt_like())).expect("writes the skirt GLB");
    let female_path = write(&root.join("skirt.glb"), &female_bytes);
    let (skirt, skirt_kind) = read_and_classify(&female_path, &female_body, &female_triangles);

    let male_bytes =
        write_glb(&garment_asset("trousers", trouser_like())).expect("writes the trousers GLB");
    let male_path = write(&root.join("trousers.glb"), &male_bytes);
    let (trousers, trousers_kind) = read_and_classify(&male_path, &male_body, &male_triangles);

    println!(
        "female fixture: kind={}, vertices={}; male fixture: kind={}, vertices={}",
        skirt_kind.name(),
        skirt.positions.len(),
        trousers_kind.name(),
        trousers.positions.len(),
    );

    assert_eq!(skirt_kind, GarmentKind::Skirt);
    assert_eq!(trousers_kind, GarmentKind::Trousers);
    assert_ne!(
        skirt_kind, trousers_kind,
        "the two gender fixtures must classify into different kinds"
    );

    for (garment, kind, body) in [
        (skirt, skirt_kind, &female_body),
        (trousers, trousers_kind, &male_body),
    ] {
        let vertices = garment.positions.len();
        let fit = garment
            .into_fit_input()
            .expect("hands the garment to the fit path");
        assert_eq!(
            fit.positions.len(),
            vertices,
            "the hand-off keeps the intake vertex order"
        );
        let conform = Conform::bind(&fit.positions, body).expect("binds to its body");
        let fitted = conform.apply(body).expect("applies the unchanged body");
        assert_eq!(
            fitted.len(),
            fit.positions.len(),
            "{kind:?} fit must preserve the vertex count"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_undecided_kind_is_a_typed_refusal_listing_the_measurements() {
    let (body, body_triangles) = female();
    let (positions, triangles) = jumpsuit_like();
    let garment = raw_garment(positions, triangles);

    let error = garment
        .classify(&body, &body_triangles)
        .expect_err("the jumpsuit silhouette has no decided kind");
    let KindError::Undecided { features, evidence } = &error else {
        panic!("expected Undecided, got {error}");
    };
    assert!(
        (features.bottom - 0.05).abs() < 0.01,
        "bottom {} of body height",
        features.bottom
    );
    assert!(
        (features.top - 0.60).abs() < 0.01,
        "top {} of body height",
        features.top
    );
    assert!(
        (2..=4).contains(&features.leg_columns),
        "leg_columns {}",
        features.leg_columns
    );
    assert!(
        evidence
            .iter()
            .any(|line| line.contains("nearest: trousers, dress")),
        "the refusal names its alternatives: {evidence:?}"
    );

    let text = error.to_string();
    println!("refusal case: {text}");
    assert!(text.contains("undecided"), "{text}");
    for measurement in [
        "bottom=0.05",
        "top=0.60",
        "coverage=0.55",
        "leg_columns=2",
        "body_columns=",
        "radial_ratio=",
        "angular=",
        "components=",
        "vertices=128",
    ] {
        assert!(
            text.contains(measurement),
            "the refusal must list {measurement}: {text}"
        );
    }
}

#[test]
fn classification_failures_refuse_as_the_fitted_path_does() {
    let (body, body_triangles) = female();
    let (positions, triangles) = skirt_like();
    let garment = raw_garment(positions, triangles);

    assert_eq!(
        garment.classify(&[], &[]),
        Err(KindError::Classified(ClassifyError::EmptyBody)),
        "an empty body is the classifier's own refusal"
    );
    assert_eq!(
        garment.classify(&[[0.0; 3]; 4], &[[0, 1, 2]]),
        Err(KindError::Classified(ClassifyError::FlatBody)),
        "a flat body is the classifier's own refusal"
    );
    let stray = raw_garment(
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[0, 1, 9]],
    );
    assert_eq!(
        stray.classify(&body, &body_triangles),
        Err(KindError::Classified(ClassifyError::BadIndex {
            triangle: 0,
            vertex: 9,
        })),
        "a triangle outside the garment is the classifier's own refusal",
    );
}
