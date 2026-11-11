use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::{ErrorKind, dispatch};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::{Value, json};

struct Fixture {
    directory: PathBuf,
    nif: PathBuf,
}

impl Fixture {
    fn new(label: &str, shape_name: &str) -> Self {
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
    for data in [
        "{ broken json",
        r#"{"outfit":" ","nif":"outfit.nif"}"#,
    ] {
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
