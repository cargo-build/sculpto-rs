//! Native acceptance of the accepted Bijin CBBE female body via Outfit Studio headless automation.
//!
//! Private-host tier: the body NIF is reached through `SCULPTO_BIJIN_NIF`;
//! without it, or without a headless Outfit Studio copy under `/tmp/opencode/`,
//! the test skips so CI / `just gate` remains portable.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sculpto_nif::read;

#[test]
fn bijin_body_native_headless_reimport_matches() {
    let Ok(bijin_path_str) = std::env::var("SCULPTO_BIJIN_NIF") else {
        eprintln!("SCULPTO_BIJIN_NIF not set; skipping Bijin native acceptance test");
        return;
    };
    let bijin_path = PathBuf::from(&bijin_path_str);
    if !bijin_path.exists() {
        eprintln!(
            "Bijin NIF at {:?} does not exist; skipping test",
            bijin_path
        );
        return;
    }

    // 1. Read source Bijin body NIF with sculpto_nif::read
    let source_bytes = fs::read(&bijin_path).expect("read source Bijin NIF bytes");
    let source_doc = read(&source_bytes).expect("parse source Bijin NIF with sculpto_nif::read");
    assert_eq!(
        source_doc.shapes.len(),
        1,
        "source NIF must contain 1 shape"
    );
    assert_eq!(
        source_doc.bsver, 83,
        "source Bijin body is Skyrim LE format"
    );

    let source_shape = &source_doc.shapes[0];
    assert_eq!(source_shape.name, "BaseShape");
    assert_eq!(source_shape.positions.len(), 3471);
    assert_eq!(source_shape.triangles.len(), 6516);
    assert_eq!(source_shape.bones.len(), 22);

    // 2. Locate or initialize BodySlide home copy under /tmp/opencode/
    let bs_copy = PathBuf::from("/tmp/opencode/bijin-bodyslide");
    if !bs_copy.join("OutfitStudio").exists() {
        let c15_copy = PathBuf::from("/tmp/opencode/c15-bodyslide");
        if c15_copy.join("OutfitStudio").exists() {
            let _ = Command::new("cp")
                .arg("-a")
                .arg("--reflink=auto")
                .arg(&c15_copy)
                .arg(&bs_copy)
                .status();
        } else {
            let host_bs = PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join("Models/bodyslide-project-20261025-01a17320/BodySlide");
            if host_bs.join("OutfitStudio").exists() {
                let _ = Command::new("cp")
                    .arg("-a")
                    .arg("--reflink=auto")
                    .arg(&host_bs)
                    .arg(&bs_copy)
                    .status();
            } else {
                eprintln!("OutfitStudio executable not found under /tmp/opencode/; skipping");
                return;
            }
        }
    }

    let out_dir = PathBuf::from("/tmp/opencode/bijin-proof");
    fs::create_dir_all(&out_dir).expect("create proof out_dir");
    let automations_dir = bs_copy.join("Automations");
    fs::create_dir_all(&automations_dir).expect("create Automations dir");

    let reexport_nif_path = out_dir.join("bijin_reexport.nif");
    let reexport_obj_path = out_dir.join("bijin_reexport.obj");
    let _ = fs::remove_file(&reexport_nif_path);
    let _ = fs::remove_file(&reexport_obj_path);

    // 3. Write automation script XML
    let script_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<AutomationScript>
  <Step type="ImportFile">
    <FilePath>{}</FilePath>
  </Step>
  <Step type="ExportFile">
    <FilePath>{}</FilePath>
  </Step>
  <Step type="ExportFile">
    <FilePath>{}</FilePath>
  </Step>
</AutomationScript>
"#,
        bijin_path.display(),
        reexport_nif_path.display(),
        reexport_obj_path.display()
    );
    let script_name = "bijin_reimport_test";
    fs::write(
        automations_dir.join(format!("{script_name}.xml")),
        script_xml,
    )
    .expect("write automation XML");

    // 4. Drive OutfitStudio headlessly
    let status = Command::new("./OutfitStudio")
        .arg("-a")
        .arg(script_name)
        .current_dir(&bs_copy)
        .status()
        .expect("execute OutfitStudio");
    assert!(
        status.success(),
        "OutfitStudio process failed: {:?}",
        status
    );

    // 5. Read re-exported NIF with sculpto_nif::read
    assert!(
        reexport_nif_path.exists(),
        "re-exported NIF must exist at {:?}",
        reexport_nif_path
    );
    let reexport_bytes = fs::read(&reexport_nif_path).expect("read re-exported NIF bytes");
    let reexport_doc = read(&reexport_bytes).expect("parse re-exported NIF with sculpto_nif::read");

    assert_eq!(
        reexport_doc.shapes.len(),
        1,
        "re-exported NIF must have 1 shape"
    );
    assert_eq!(reexport_doc.bsver, 100, "Outfit Studio converted LE to SE");

    let reexport_shape = &reexport_doc.shapes[0];
    assert_eq!(reexport_shape.name, "BaseShape");
    assert_eq!(
        reexport_shape.positions.len(),
        source_shape.positions.len(),
        "vertex counts must match exactly"
    );
    assert_eq!(
        reexport_shape.triangles.len(),
        source_shape.triangles.len(),
        "triangle counts must match exactly"
    );
    assert_eq!(
        reexport_shape.bones.len(),
        source_shape.bones.len(),
        "bone counts must match exactly"
    );
    assert_eq!(
        reexport_shape.bones, source_shape.bones,
        "bone list must match"
    );

    // Max position coordinate difference across all 3471 vertices
    let mut max_pos_diff = 0.0f32;
    for (src_pos, re_pos) in source_shape.positions.iter().zip(&reexport_shape.positions) {
        for c in 0..3 {
            let diff = (src_pos[c] - re_pos[c]).abs();
            if diff > max_pos_diff {
                max_pos_diff = diff;
            }
        }
    }
    assert!(
        max_pos_diff < 1e-4,
        "vertex positions must match within tolerance, max diff: {}",
        max_pos_diff
    );

    // 6. Check native OBJ counts
    assert!(reexport_obj_path.exists(), "OBJ file must exist");
    let obj_text = fs::read_to_string(&reexport_obj_path).expect("read re-exported OBJ");
    let obj_verts = obj_text.lines().filter(|l| l.starts_with("v ")).count();
    let obj_tris = obj_text.lines().filter(|l| l.starts_with("f ")).count();
    assert_eq!(obj_verts, 3471, "OBJ vertex count must be 3471");
    assert_eq!(obj_tris, 6516, "OBJ triangle count must be 6516");
}
