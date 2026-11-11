//! Region selection reaches the registered fit operation. @req DECL0013

use std::path::PathBuf;

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::{dispatch, obj};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::{Value, json};

struct Fixture {
    root: PathBuf,
    garment: MeshAsset,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("sculpto-regions-{tag}-{}", std::process::id()));
        std::fs::create_dir(&root).expect("unique region fixture");
        let body = MeshAsset {
            name: "Body".into(),
            positions: vec![[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [10.0, 10.0, 0.0], [-10.0, 10.0, 0.0]],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            triangle_partitions: vec![PartitionId(0); 2],
            ..MeshAsset::default()
        };
        let garment = MeshAsset {
            name: "Garment".into(),
            positions: vec![[-4.0, -0.5, -0.2], [-2.0, -0.5, -0.2], [-3.0, 0.5, -0.2],
                [2.0, -0.5, -0.4], [4.0, -0.5, 0.4], [3.0, 0.5, 0.4]],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            triangle_partitions: vec![PartitionId(0); 2],
            ..MeshAsset::default()
        };
        std::fs::write(root.join("body.nif"), write_se_shape(&body).expect("body NIF")).expect("body file");
        std::fs::write(root.join("garment.nif"), write_se_shape(&garment).expect("garment NIF")).expect("garment file");
        obj::write_obj(&root.join("body.obj"), &body.positions, &body.triangles).expect("source body OBJ");
        std::fs::write(root.join("manifest.json"), serde_json::to_vec(&json!({
            "outfit":"Mixed garment", "nif":"garment.nif",
            "garment_shapes":[{"name":"Garment", "blender_cloth":[{"name":"loose", "triangles":[1]}]}],
        })).expect("manifest JSON")).expect("manifest file");
        Self { root, garment }
    }

    fn request(&self) -> Value {
        json!({"nif":self.root.join("garment.nif"), "shape":"Garment", "body_nif":self.root.join("body.nif"),
            "body_shape":"Body", "body":self.root.join("body.obj"), "manifest":self.root.join("manifest.json"),
            "out":self.root.join("fitted.obj")})
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove region fixture");
    }
}

#[test]
fn registered_fit_preserves_cloth_and_measures_both_regions() {
    let fixture = Fixture::new("mixed");
    let result = dispatch("outfit.fit", &fixture.request()).expect("manifest-aware fit");
    let output = obj::read_obj_positions(&fixture.root.join("fitted.obj")).expect("read fit output");
    assert_eq!(&output[3..], &fixture.garment.positions[3..], "cloth geometry must retain incoming positions");
    assert!(output[..3].iter().all(|point| point[2] > 0.0), "ordinary region must leave the body");
    assert_eq!(result["regions"]["ordinary_triangles"], 1);
    assert_eq!(result["regions"]["cloth_triangles"], 1);
    assert_eq!(result["regions"]["ordinary"]["inside_after"], 0);
    assert_eq!(result["regions"]["ordinary"]["intersection_pairs"], 0);
    assert!(result["regions"]["cloth_intersection_pairs"].as_u64().expect("cloth contact count") > 0);
    assert_eq!(result["intersection_pairs"], result["regions"]["intersection_pairs"]);
    assert!(result.get("inside_after").is_none(), "region inside counts need their explicit scope");
}

#[test]
fn mismatched_manifest_source_leaves_no_output() {
    let fixture = Fixture::new("wrong-source");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(fixture.root.join("manifest.json")).expect("manifest")).expect("JSON");
    manifest["nif"] = json!("body.nif");
    std::fs::write(fixture.root.join("manifest.json"), serde_json::to_vec(&manifest).expect("JSON")).expect("write manifest");
    let error = dispatch("outfit.fit", &fixture.request()).expect_err("source mismatch must fail");
    assert_eq!(error.code(), "outfit_source_mismatch");
    assert!(!fixture.root.join("fitted.obj").exists());
}

#[test]
fn configured_nightshade_regions_preserve_cloth_through_the_command() {
    let (Some(mod_root), Some(rig_root)) = (
        std::env::var_os("SCULPTO_MOD_FIXTURE"),
        std::env::var_os("SCULPTO_RIG_TEMPLATE_FIXTURES"),
    ) else {
        eprintln!("SKIP configured_nightshade_regions_preserve_cloth_through_the_command: configure both fixture roots");
        return;
    };
    let outfit_path = PathBuf::from(mod_root).join("apachii-divine-elegance/unpacked-bsa/meshes/divineelegance/female/robes/nightshade/robef_1.nif");
    let body_path = PathBuf::from(rig_root).join("bijin.nif");
    let outfit = sculpto_nif::read(&std::fs::read(&outfit_path).expect("configured outfit must exist")).expect("Nightshade NIF");
    let body_document = sculpto_nif::read(&std::fs::read(&body_path).expect("configured body must exist")).expect("Bijin NIF");
    let robe_index = outfit.shapes.iter().position(|shape| shape.name == "robe").expect("robe shape");
    let belt_index = outfit.shapes.iter().position(|shape| shape.name == "belt").expect("belt shape");
    let body_index = body_document.shapes.iter().position(|shape| shape.name == "BaseShape").expect("Bijin body shape");
    let robe = &outfit.shapes[robe_index];
    assert_eq!((robe.positions.len(), robe.triangles.len()), (3808, 6410));
    let robe_placement = outfit.shape_placement(robe_index).expect("robe placement");
    let source: Vec<_> = robe.positions.iter().map(|&point| robe_placement.position_in_source(point)).collect();
    let belt_placement = outfit.shape_placement(belt_index).expect("belt placement");
    let (bottom, top) = outfit.shapes[belt_index].positions.iter()
        .map(|&point| belt_placement.position_in_source(point)[2])
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), z| (min.min(z), max.max(z)));
    // This fixture's proposed cloth boundary follows the authored belt band.
    let waist = (bottom + top) * 0.5;
    let cloth: Vec<_> = robe.triangles.iter().enumerate()
        .filter(|(_, face)| face.iter().all(|&vertex| source[vertex as usize][2] <= waist))
        .map(|(index, _)| index).collect();
    assert_eq!(cloth.len(), 3052, "pinned candidate region");
    let fixture = Fixture::new("nightshade");
    std::fs::copy(&outfit_path, fixture.root.join("garment.nif")).expect("isolated source outfit");
    std::fs::copy(&body_path, fixture.root.join("body.nif")).expect("isolated source body");
    let body = &body_document.shapes[body_index];
    obj::write_obj(&fixture.root.join("body.obj"), &body.positions, &body.triangles).expect("body in stored frame");
    std::fs::write(fixture.root.join("manifest.json"), serde_json::to_vec(&json!({
        "outfit":"Nightshade mixed-region candidate", "nif":"garment.nif",
        "garment_shapes":[{"name":"robe", "blender_cloth":[{"name":"hanging_panels", "triangles":cloth}]}],
    })).expect("manifest JSON")).expect("manifest file");
    let mut input = fixture.request();
    input["shape"] = json!("robe");
    input["body_shape"] = json!("BaseShape");
    let report = dispatch("outfit.fit", &input).expect("mixed-region command");
    let output = obj::read_obj_positions(&fixture.root.join("fitted.obj")).expect("fitted OBJ");
    assert_eq!(output.len(), source.len());
    let output_text = std::fs::read_to_string(fixture.root.join("fitted.obj")).expect("OBJ text");
    assert_eq!(output_text.lines().filter(|line| line.starts_with("f ")).count(), 6410);
    let body_placement = body_document.shape_placement(body_index).expect("body placement");
    let vertices: std::collections::BTreeSet<_> = cloth.iter().flat_map(|&face| robe.triangles[face]).collect();
    let maximum = vertices.iter().map(|&vertex| {
        let after = body_placement.position_in_source(output[vertex as usize]);
        after.into_iter().zip(source[vertex as usize]).map(|(after, before)| f64::from(after - before).powi(2)).sum::<f64>().sqrt()
    }).fold(0.0_f64, f64::max);
    assert!(maximum <= 1e-4, "cloth changed across output frame conversion: {maximum}");
    assert_eq!(report["regions"]["ordinary_triangles"], 3358);
    assert_eq!(report["regions"]["cloth_triangles"], 3052);
    eprintln!("Nightshade region transport only: cloth_faces=3052 cloth_vertices={} max_cloth_frame_error={maximum:.9} ordinary={} cloth_contacts={}",
        vertices.len(), report["regions"]["ordinary"], report["regions"]["cloth_intersection_pairs"]);
}
