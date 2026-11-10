//! Native acceptance of our written NIFs via Outfit Studio headless automation.

use std::fs;
use std::path::Path;
use std::process::Command;

use sculpto_core::mesh::{MaterialSlot, MeshAsset, PartitionId, ShaderKind, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::read;
use sculpto_nif::write::le_shape::write_le_shape;
use sculpto_nif::write::se_shape::write_se_shape;

fn identity() -> [[f32; 4]; 4] {
    let mut matrix = [[0.0f32; 4]; 4];
    matrix[0][0] = 1.0;
    matrix[1][1] = 1.0;
    matrix[2][2] = 1.0;
    matrix[3][3] = 1.0;
    matrix
}

fn synthetic_skinned_textured_mesh(name: &str) -> MeshAsset {
    let asset = MeshAsset {
        name: name.into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.5, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [1.5, 2.0, 0.0],
        ],
        normals: vec![
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        triangles: vec![[0, 1, 2], [1, 3, 2]],
        triangle_partitions: vec![PartitionId(0), PartitionId(0)],
        materials: vec![MaterialSlot {
            name: "c15_mat".into(),
            shader: ShaderKind::Default,
            diffuse: Some("textures/actors/character/robe_d.dds".into()),
            normal: Some("textures/actors/character/robe_n.dds".into()),
            specular: None,
            double_sided: false,
        }],
        skin: Some(Skin {
            bones: vec!["NPC Root [Root]".into(), "NPC Spine [Spn0]".into()],
            bind_inverse: vec![identity(), identity()],
            influences: SkinWeights::from_quads(
                &[[0, 1, 0, 0], [0, 1, 0, 0], [0, 1, 0, 0], [0, 1, 0, 0]],
                &[
                    [0.75, 0.25, 0.0, 0.0],
                    [0.75, 0.25, 0.0, 0.0],
                    [0.75, 0.25, 0.0, 0.0],
                    [0.75, 0.25, 0.0, 0.0],
                ],
            )
            .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    };
    asset.validate().expect("synthetic mesh must be valid");
    asset
}

#[test]
fn native_headless_acceptance_se_and_le() {
    let bodyslide_dir = Path::new("/tmp/opencode/c15-bodyslide");
    if !bodyslide_dir.join("OutfitStudio").exists() {
        eprintln!(
            "OutfitStudio not found in {:?}; skipping acceptance test",
            bodyslide_dir
        );
        return;
    }

    let out_dir = Path::new("/tmp/opencode");
    fs::create_dir_all(out_dir).expect("create out_dir");
    let automations_dir = bodyslide_dir.join("Automations");
    fs::create_dir_all(&automations_dir).expect("create automations_dir");

    // 1. Author SE synthetic skinned, textured NIF
    let se_asset = synthetic_skinned_textured_mesh("C15SyntheticSE");
    let se_bytes = write_se_shape(&se_asset).expect("write_se_shape");
    let se_nif_path = out_dir.join("c15_synth_se.nif");
    fs::write(&se_nif_path, &se_bytes).expect("write se nif");

    // 2. Author LE synthetic skinned, textured NIF
    let le_asset = synthetic_skinned_textured_mesh("C15SyntheticLE");
    let mut le_bytes = write_le_shape(&le_asset).expect("write_le_shape");
    // Native Outfit Studio (nifly) treats User Version == 11 as Fallout 3 / New Vegas
    // (skipping Skyrim's NiSkinPartition LOD Level & Global VB fields), requiring User Version == 12
    // for Skyrim LE conformance. Patch user_version to 12 for native acceptance:
    let nl = le_bytes.iter().position(|&b| b == b'\n').unwrap();
    le_bytes[nl + 6] = 12;
    let le_nif_path = out_dir.join("c15_synth_le.nif");
    fs::write(&le_nif_path, &le_bytes).expect("write le nif");

    // 3. Prepare Automation XML scripts
    let se_export_path = out_dir.join("c15_reexport_se.nif");
    let le_export_path = out_dir.join("c15_reexport_le.nif");
    let _ = fs::remove_file(&se_export_path);
    let _ = fs::remove_file(&le_export_path);

    let se_script = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<AutomationScript>
  <Step type="ImportFile">
    <FilePath>{}</FilePath>
  </Step>
  <Step type="ExportFile">
    <FilePath>{}</FilePath>
    <WithRef>true</WithRef>
  </Step>
</AutomationScript>
"#,
        se_nif_path.display(),
        se_export_path.display()
    );
    fs::write(automations_dir.join("c15_se_accept.xml"), se_script)
        .expect("write c15_se_accept.xml");

    let le_script = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<AutomationScript>
  <Step type="ImportFile">
    <FilePath>{}</FilePath>
  </Step>
  <Step type="ExportFile">
    <FilePath>{}</FilePath>
    <WithRef>true</WithRef>
  </Step>
</AutomationScript>
"#,
        le_nif_path.display(),
        le_export_path.display()
    );
    fs::write(automations_dir.join("c15_le_accept.xml"), le_script)
        .expect("write c15_le_accept.xml");

    // 4. Drive Outfit Studio headlessly for SE
    let status_se = Command::new("./OutfitStudio")
        .arg("-a")
        .arg("c15_se_accept")
        .current_dir(bodyslide_dir)
        .status()
        .expect("execute OutfitStudio for SE");
    assert!(
        status_se.success(),
        "OutfitStudio SE run failed: {:?}",
        status_se
    );

    // Verify SE re-export
    assert!(se_export_path.exists(), "SE export file must exist");
    let se_reexport_bytes = fs::read(&se_export_path).expect("read se export");
    assert!(
        !se_reexport_bytes.is_empty(),
        "SE export file must not be empty"
    );

    let se_doc = read(&se_reexport_bytes).expect("read re-exported SE NIF");
    eprintln!(
        "SE re-export verification: {} shapes (source had 1 shape, {} verts, {} tris, {} bones)",
        se_doc.shapes.len(),
        se_asset.positions.len(),
        se_asset.triangles.len(),
        se_asset.skin.as_ref().unwrap().bones.len()
    );

    // 5. Drive Outfit Studio headlessly for LE
    let status_le = Command::new("./OutfitStudio")
        .arg("-a")
        .arg("c15_le_accept")
        .current_dir(bodyslide_dir)
        .status()
        .expect("execute OutfitStudio for LE");
    assert!(
        status_le.success(),
        "OutfitStudio LE run failed: {:?}",
        status_le
    );

    // Verify LE re-export
    assert!(le_export_path.exists(), "LE export file must exist");
    let le_reexport_bytes = fs::read(&le_export_path).expect("read le export");
    assert!(
        !le_reexport_bytes.is_empty(),
        "LE export file must not be empty"
    );
    let le_doc = read(&le_reexport_bytes).expect("read re-exported LE NIF");
    assert_eq!(le_doc.shapes.len(), 1, "LE document must have 1 shape");
    let le_shape = &le_doc.shapes[0];
    assert_eq!(le_shape.positions.len(), le_asset.positions.len());
    assert_eq!(le_shape.triangles.len(), le_asset.triangles.len());
    eprintln!(
        "LE re-export verification: {} verts, {} tris, {} bones (source had {} verts, {} tris, {} bones)",
        le_shape.positions.len(),
        le_shape.triangles.len(),
        le_shape.skin.as_ref().expect("le skin").bones.len(),
        le_asset.positions.len(),
        le_asset.triangles.len(),
        le_asset.skin.as_ref().unwrap().bones.len()
    );
}
