//! Outfit completeness gate domain tests. The nightshade garment names come
//! from the MOD-01 census, so the fixture manifest and these tests share one
//! source of truth.

use sculpto_mesh::outfit::{
    CheckShape, CompletenessDefect, CompletenessFailure, OutfitManifest, ShapeSlot,
    completeness_defects,
};

fn manifest(body_shapes: &[&str], garments: &[(&str, bool)]) -> OutfitManifest {
    OutfitManifest {
        outfit: "nightshade".into(),
        nif: "nightshade.nif".into(),
        body_shapes: body_shapes.iter().map(|name| (*name).into()).collect(),
        garment_shapes: garments
            .iter()
            .map(|&(name, requires_material)| ShapeSlot {
                name: name.into(),
                requires_material,
                blender_cloth: Vec::new(),
            })
            .collect(),
    }
}

fn complete_source() -> Vec<CheckShape<'static>> {
    // Census counts: robe 6410t, sleeves 886t, belt 427t, pants 632t.
    [
        ("robe", 6410),
        ("sleeves", 886),
        ("belt", 427),
        ("pants", 632),
    ]
    .iter()
    .map(|&(name, triangles)| CheckShape {
        name,
        triangles,
        has_diffuse: true,
    })
    .collect()
}

fn nightshade() -> OutfitManifest {
    manifest(
        &[],
        &[
            ("robe", true),
            ("sleeves", true),
            ("belt", true),
            ("pants", true),
        ],
    )
}

#[test]
fn complete_manifest_yields_no_defects() {
    assert_eq!(completeness_defects(&complete_source(), &nightshade()), []);
}

#[test]
fn missing_shape_is_absent_naming_only_it() {
    let shapes: Vec<_> = complete_source()
        .into_iter()
        .filter(|shape| shape.name != "belt")
        .collect();
    assert_eq!(
        completeness_defects(&shapes, &nightshade()),
        [CompletenessFailure {
            shape: "belt".into(),
            defect: CompletenessDefect::Absent,
        }],
    );
}

#[test]
fn empty_geometry_is_a_distinct_defect() {
    let mut shapes = complete_source();
    shapes[2] = CheckShape {
        name: "belt",
        triangles: 0,
        has_diffuse: true,
    };
    assert_eq!(
        completeness_defects(&shapes, &nightshade()),
        [CompletenessFailure {
            shape: "belt".into(),
            defect: CompletenessDefect::EmptyGeometry,
        }],
    );
}

#[test]
fn material_rule_is_per_slot_not_global() {
    let mut shapes = complete_source();
    for shape in &mut shapes {
        shape.has_diffuse = false;
    }
    // No slot requires a material, so an untextured bundle stays complete.
    let unrequiring = manifest(
        &[],
        &[
            ("robe", false),
            ("sleeves", false),
            ("belt", false),
            ("pants", false),
        ],
    );
    assert_eq!(completeness_defects(&shapes, &unrequiring), []);

    // The same bundle under the requiring manifest fails every slot.
    let defects = completeness_defects(&shapes, &nightshade());
    assert_eq!(defects.len(), 4);
    assert!(
        defects
            .iter()
            .all(|failure| failure.defect == CompletenessDefect::MissingMaterial)
    );

    // One authored diffuse clears only its own slot.
    shapes[0].has_diffuse = true;
    let defects = completeness_defects(&shapes, &nightshade());
    assert_eq!(defects.len(), 3);
    assert!(defects.iter().all(|failure| failure.shape != "robe"));
}

#[test]
fn body_and_unlisted_shapes_are_not_checked() {
    let mut shapes = complete_source();
    shapes.push(CheckShape {
        name: "BaseShape",
        triangles: 0,
        has_diffuse: false,
    });
    shapes.push(CheckShape {
        name: "unlisted",
        triangles: 0,
        has_diffuse: false,
    });
    // The manifest lists BaseShape only as a body shape, and also repeats it
    // as a garment slot: both the exclusion and the unlisted empty shape stay
    // outside the gate.
    let manifest = manifest(&["BaseShape"], &[("BaseShape", true)]);
    assert_eq!(completeness_defects(&shapes, &manifest), []);
}

#[test]
fn failures_keep_manifest_order_and_name_every_gap() {
    let shapes: Vec<_> = complete_source()
        .into_iter()
        .filter(|shape| shape.name != "belt")
        .map(|mut shape| {
            if shape.name == "sleeves" {
                shape.has_diffuse = false;
            }
            shape
        })
        .collect();
    let failures = completeness_defects(&shapes, &nightshade());
    assert_eq!(
        failures,
        [
            CompletenessFailure {
                shape: "sleeves".into(),
                defect: CompletenessDefect::MissingMaterial,
            },
            CompletenessFailure {
                shape: "belt".into(),
                defect: CompletenessDefect::Absent,
            },
        ],
    );
    let message = failures
        .iter()
        .map(|failure| failure.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    assert_eq!(
        message,
        "sleeves has no diffuse material; belt is not in the bundle",
    );
}
