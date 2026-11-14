use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId};
use sculpto_engine::{ErrorKind, dispatch};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::{Value, json};

struct Fixture {
    directory: PathBuf,
    nif: PathBuf,
}

impl Fixture {
    fn new(label: &str, shape_name: &str) -> Self {
        Self::textured(label, shape_name, None)
    }

    /// A fixture whose shape carries one material slot with an optional
    /// diffuse path, so the material leg of `outfit.check` runs through the
    /// real writer and reader.
    fn textured(label: &str, shape_name: &str, diffuse: Option<&str>) -> Self {
        let directory =
            std::env::temp_dir().join(format!("sculpto-outfit-{}-{label}", std::process::id()));
        std::fs::create_dir(&directory).expect("test directory");
        let nif = directory.join("outfit.nif");
        let shape = MeshAsset {
            name: shape_name.into(),
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.5, 0.5, 1.0],
            ],
            triangles: vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]],
            triangle_partitions: vec![PartitionId(0); 4],
            materials: vec![MaterialSlot {
                name: shape_name.into(),
                diffuse: diffuse.map(str::to_string),
                ..MaterialSlot::default()
            }],
            ..MeshAsset::default()
        };
        std::fs::write(&nif, write_se_shape(&shape).expect("fixture writes")).expect("fixture");
        Self { directory, nif }
    }

    fn manifest(&self, shapes: Value) -> PathBuf {
        let path = self.directory.join("outfit.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "outfit": "mixed",
                "nif": "outfit.nif",
                "garment_shapes": shapes,
            }))
            .expect("manifest JSON"),
        )
        .expect("manifest fixture");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn request(nif: &Path, manifest: Option<&Path>) -> Value {
    json!({ "nif": nif, "manifest": manifest })
}

#[test]
fn a_mixed_shape_reports_blender_cloth_and_the_ordinary_rigged_complement() {
    let fixture = Fixture::new("mixed", "robe");
    let manifest = fixture.manifest(json!([{
        "name": "robe",
        "requires_material": true,
        "blender_cloth": [{ "name": "skirt", "triangles": [3, 1] }],
    }]));
    let result = dispatch("outfit.inspect", &request(&fixture.nif, Some(&manifest)))
        .expect("the manifest binds");

    assert_eq!(result["outfit"], "mixed");
    assert_eq!(result["shapes"][0]["shape_index"], 0);
    assert_eq!(result["shapes"][0]["role"], "garment");
    assert_eq!(result["shapes"][0]["requires_material"], true);
    assert_eq!(
        result["shapes"][0]["cloth"],
        json!([
            { "path": "blender_cloth", "source": "manifest", "region": "skirt", "triangle_indices": [1, 3] },
            { "path": "ordinary_rigged", "source": "ordinary_rig", "triangle_indices": [0, 2] },
        ]),
    );
}

#[test]
fn whole_piece_selection_and_the_usual_default_stay_distinct() {
    let fixture = Fixture::new("whole", "robe");
    let plain = dispatch("outfit.inspect", &request(&fixture.nif, None)).expect("census");
    assert_eq!(
        plain["shapes"][0]["cloth"],
        json!([{ "path": "ordinary_rigged", "source": "ordinary_rig" }]),
    );
    let manifest = fixture.manifest(json!([{
        "name": "robe",
        "blender_cloth": [{ "name": "whole robe" }],
    }]));
    let selected = dispatch("outfit.inspect", &request(&fixture.nif, Some(&manifest)))
        .expect("the whole garment can be selected");
    assert_eq!(
        selected["shapes"][0]["cloth"],
        json!([{ "path": "blender_cloth", "source": "manifest", "region": "whole robe" }]),
    );
}

#[test]
fn wrong_sources_unknown_shapes_and_overlapping_regions_are_refused() {
    let fixture = Fixture::new("invalid", "robe");
    let different = Fixture::new("other", "robe");
    let valid = fixture.manifest(json!([{ "name": "robe" }]));
    let error = dispatch("outfit.inspect", &request(&different.nif, Some(&valid)))
        .expect_err("a manifest is specific to one source");
    assert_eq!(error.code(), "outfit_source_mismatch");

    let unknown = fixture.manifest(json!([{ "name": "beltX" }]));
    let error = dispatch("outfit.inspect", &request(&fixture.nif, Some(&unknown)))
        .expect_err("an unknown shape cannot be selected");
    assert_eq!(error.code(), "outfit_shape_unknown");
    assert!(error.message().contains("beltX"));

    let overlap = fixture.manifest(json!([{
        "name": "robe",
        "blender_cloth": [
            { "name": "hem", "triangles": [0, 1] },
            { "name": "panel", "triangles": [1, 3] },
        ],
    }]));
    let error = dispatch("outfit.inspect", &request(&fixture.nif, Some(&overlap)))
        .expect_err("each source face has one route");
    assert_eq!(error.code(), "outfit_cloth_invalid");
    assert!(error.message().contains("triangle 1"));

    let outside = fixture.manifest(json!([{
        "name": "robe",
        "blender_cloth": [{ "name": "hem", "triangles": [4] }],
    }]));
    let error = dispatch("outfit.inspect", &request(&fixture.nif, Some(&outside)))
        .expect_err("an ordinal at the triangle count is outside the source");
    assert_eq!(error.code(), "outfit_cloth_invalid");
    assert!(error.message().contains("triangle 4 outside"));
}

#[test]
fn invalid_manifest_json_and_blank_names_share_the_parameter_error() {
    let fixture = Fixture::new("manifest", "robe");
    let path = fixture.manifest(json!([]));
    for data in ["not-json", r#"{"outfit":" ","nif":"outfit.nif"}"#] {
        std::fs::write(&path, data).expect("invalid manifest fixture");
        let error = dispatch("outfit.inspect", &request(&fixture.nif, Some(&path)))
            .expect_err("the manifest is invalid");
        assert_eq!(error.code(), "outfit_manifest_invalid");
        assert_eq!(error.kind(), ErrorKind::InvalidParams);
    }
}

#[test]
fn an_authored_garment_role_can_override_a_body_name() {
    let fixture = Fixture::new("role", "BaseShape");
    let inferred = dispatch("outfit.inspect", &request(&fixture.nif, None)).expect("census");
    assert_eq!(inferred["shapes"][0]["role"], "body");
    assert_eq!(inferred["shapes"][0]["cloth"], json!([]));

    let manifest = fixture.manifest(json!([{ "name": "BaseShape" }]));
    let overridden = dispatch("outfit.inspect", &request(&fixture.nif, Some(&manifest)))
        .expect("manifest role overrides the name");
    assert_eq!(overridden["shapes"][0]["role"], "garment");
    assert_eq!(
        overridden["shapes"][0]["cloth"][0]["path"],
        "ordinary_rigged"
    );
}

#[test]
fn outfit_check_refuses_a_missing_required_shape_as_incomplete() {
    let fixture = Fixture::new("check-missing", "robe");
    let manifest = fixture.manifest(json!([
        { "name": "robe" },
        { "name": "belt" },
    ]));
    let error = dispatch("outfit.check", &request(&fixture.nif, Some(&manifest)))
        .expect_err("a belt-less bundle is incomplete, not unknown");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "outfit_incomplete");
    assert!(error.message().contains("belt"), "{}", error.message());
    assert!(!error.message().contains("robe"), "{}", error.message());
}

#[test]
fn outfit_check_passes_a_complete_bundle() {
    let fixture = Fixture::new("check-complete", "robe");
    let manifest = fixture.manifest(json!([{ "name": "robe" }]));
    let result = dispatch("outfit.check", &request(&fixture.nif, Some(&manifest)))
        .expect("every manifest requirement is met");

    assert_eq!(result["outfit"], "mixed");
    assert_eq!(result["verdict"], "complete");
    let rows = result["shapes"].as_array().expect("one row per slot");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "robe");
    assert_eq!(rows[0]["shape_index"], 0);
    assert_eq!(rows[0]["vertices"], 5);
    assert_eq!(rows[0]["triangles"], 4);
    assert_eq!(rows[0]["requires_material"], false);
    assert_eq!(rows[0]["has_diffuse"], false);
}

#[test]
fn outfit_check_requires_the_declared_diffuse() {
    let textured = Fixture::textured("check-diffuse", "robe", Some("textures/robe_d.dds"));
    let manifest = textured.manifest(json!([{ "name": "robe", "requires_material": true }]));
    let result = dispatch("outfit.check", &request(&textured.nif, Some(&manifest)))
        .expect("the authored diffuse satisfies the slot");
    assert_eq!(result["verdict"], "complete");
    assert_eq!(result["shapes"][0]["has_diffuse"], true);

    let bare = Fixture::textured("check-bare", "robe", None);
    let manifest = bare.manifest(json!([{ "name": "robe", "requires_material": true }]));
    let error = dispatch("outfit.check", &request(&bare.nif, Some(&manifest)))
        .expect_err("a declared diffuse is missing");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "outfit_incomplete");
    assert!(
        error.message().contains("robe has no diffuse material"),
        "{}",
        error.message()
    );
}

#[test]
fn real_outfit_census() {
    let Some(root) = std::env::var_os("SCULPTO_MOD_FIXTURE") else {
        eprintln!("SCULPTO_MOD_FIXTURE not set; skipping Apachii and Sentinel outfit census");
        return;
    };
    let root = PathBuf::from(root);
    for (file, expected) in [
        (
            "apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif",
            &[
                ("pants", 392, 632),
                ("belt", 403, 427),
                ("sleeves", 556, 886),
                ("robe", 3808, 6410),
            ][..],
        ),
        (
            "sentinel-himbo-refit/CalienteTools/BodySlide/ShapeData/HIMBO Sentinel - Body - Common Padded Tunic/Common Padded Tunic - Body.nif",
            &[("Armor001", 2737, 4040), ("Armor001_1", 702, 1200)][..],
        ),
    ] {
        let result = dispatch("outfit.inspect", &request(&root.join(file), None))
            .expect("configured outfit census");
        let rows = result["shapes"].as_array().expect("shape rows");
        assert_eq!(rows.len(), expected.len());
        for (row, &(name, vertices, triangles)) in rows.iter().zip(expected) {
            assert_eq!(row["name"], name);
            assert_eq!(row["vertices"], vertices);
            assert_eq!(row["triangles"], triangles);
            assert_eq!(row["role"], "garment");
            assert_eq!(row["cloth"][0]["path"], "ordinary_rigged");
            eprintln!("{name}: {vertices} vertices, {triangles} triangles");
        }
    }
}

/// A fixture whose shape carries a material with an authored texture-set
/// table, written through the real NIF writer and read back through the real
/// reader.
fn texture_table(label: &str, table: &[&str]) -> Fixture {
    let directory = std::env::temp_dir().join(format!(
        "sculpto-outfit-textures-{label}-{}",
        std::process::id()
    ));
    std::fs::create_dir(&directory).expect("test directory");
    let nif = directory.join("outfit.nif");
    let shape = MeshAsset {
        name: "robe".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.5, 0.5, 1.0],
        ],
        triangles: vec![[0, 1, 4], [1, 2, 4], [2, 3, 4], [3, 0, 4]],
        triangle_partitions: vec![PartitionId(0); 4],
        materials: vec![MaterialSlot {
            name: "robe".into(),
            texture_set: table.iter().map(|path| path.to_string()).collect(),
            ..MaterialSlot::default()
        }],
        ..MeshAsset::default()
    };
    std::fs::write(&nif, write_se_shape(&shape).expect("fixture writes")).expect("fixture");
    Fixture { directory, nif }
}

#[test]
fn authored_textures_resolve_loose_and_report_the_missing() {
    let fixture = texture_table(
        "mixed",
        &[
            "textures\\divineelegance\\robe_d.dds",
            "textures\\missing.dds",
        ],
    );
    let root = fixture.directory.clone();
    let loose = root.join("textures/divineelegance/robe_d.dds");
    std::fs::create_dir_all(loose.parent().expect("parent")).expect("loose dir");
    std::fs::write(&loose, b"DDS").expect("loose file");

    let request = json!({
        "nif": fixture.nif,
        "texture_roots": [root],
    });
    let result = dispatch("outfit.inspect", &request).expect("texture resolution runs");
    assert_eq!(result["textures"]["resolved"], 1);
    assert_eq!(
        result["textures"]["unresolved"],
        json!(["textures\\missing.dds"])
    );
    let row = &result["textures"]["shapes"][0];
    assert_eq!(row[0]["slot"], 0);
    assert_eq!(row[0]["path"], "textures\\divineelegance\\robe_d.dds");
    assert_eq!(row[0]["source"]["source"], "loose");
    assert!(
        row[0]["source"]["path"]
            .as_str()
            .expect("loose path")
            .replace('\\', "/")
            .contains("textures/divineelegance/robe_d.dds")
    );
    assert_eq!(row[1]["slot"], 1);
    assert_eq!(row[1]["source"], serde_json::Value::Null);
}

#[test]
fn without_roots_or_archives_no_reference_invents_resolution() {
    let fixture = texture_table("orphan", &["textures\\divineelegance\\robe_d.dds"]);
    let result = dispatch("outfit.inspect", &request(&fixture.nif, None))
        .expect("the report runs without sources");
    assert_eq!(result["textures"]["resolved"], 0);
    assert_eq!(
        result["textures"]["unresolved"],
        json!(["textures\\divineelegance\\robe_d.dds"])
    );
}

#[test]
fn an_archive_that_does_not_index_is_a_named_refusal() {
    let fixture = texture_table("bad-bsa", &[]);
    // Header-sized non-BSA bytes: a shorter file would refuse as truncated
    // before the magic is even read.
    let mut fake = b"BTDX".to_vec();
    fake.resize(64, 0);
    std::fs::write(fixture.directory.join("not.bsa"), fake).expect("fake archive");
    let request = json!({
        "nif": fixture.nif,
        "archives": [fixture.directory.join("not.bsa")],
    });
    let error = dispatch("outfit.inspect", &request).expect_err("a non-BSA archive refuses");
    assert_eq!(error.code(), "bsa_magic");
}

/// Real gate (tier 2): the Nightshade female outfit resolves every authored
/// texture from the configured Apachii textures BSA.
#[test]
fn real_outfit_textures_resolve_from_the_configured_archive() {
    let (Some(root), Some(archive)) = (
        std::env::var_os("SCULPTO_MOD_FIXTURE"),
        std::env::var_os("SCULPTO_MOD_BSA"),
    ) else {
        eprintln!(
            "SCULPTO_MOD_FIXTURE or SCULPTO_MOD_BSA not set; skipping real texture resolution"
        );
        return;
    };
    let nif = PathBuf::from(root)
        .join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif");
    let archive = PathBuf::from(archive).display().to_string();
    let request = json!({ "nif": nif, "archives": [archive] });
    let result = dispatch("outfit.inspect", &request).expect("real textures resolve");
    assert_eq!(
        result["textures"]["unresolved"],
        json!([]),
        "every Nightshade texture resolves from the archive"
    );
    for row in result["textures"]["shapes"].as_array().expect("shape rows") {
        for reference in row.as_array().expect("texture rows") {
            eprintln!(
                "slot {} `{}` -> {}",
                reference["slot"],
                reference["path"].as_str().unwrap(),
                serde_json::to_string(&reference["source"]).unwrap_or_else(|_| "?".into())
            );
        }
    }
    let resolved = result["textures"]["resolved"].as_u64().expect("counted");
    // Four shapes: pants, belt and sleeves carry the vampire pair, the robe
    // the robe pair — eight references, four unique paths (pinned by the
    // first configured run, MOD-03 D-5).
    assert_eq!(
        resolved, 8,
        "every Nightshade reference resolves from the archive"
    );
    eprintln!("resolved references: {resolved}");
}
