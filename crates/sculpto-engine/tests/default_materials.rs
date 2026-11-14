//! The material-complete default body (MOD-03): the configured Bijin female
//! wears the supplied Bijin Skin CBBE, and nothing else about the body moves.
//!
//! Configured-fixture coverage needs the five `SCULPTO_*` fixture roots the
//! config references; unset roots skip that test with a notice. The refusal
//! controls are synthetic and run unconditionally.

use std::path::{Path, PathBuf};

use sculpto_core::mesh::{MeshAsset, TextureSlot};
use sculpto_engine::ErrorKind;
use sculpto_engine::ops::{
    BodyDescriptor, SkinTextureSet, apply_default_skin, load_bodies_config, load_shape,
};

/// FNV-1a over positions, UVs, triangles and skin influences: the geometry and
/// rig facts a skin application must not disturb.
fn fingerprint(mesh: &MeshAsset) -> u64 {
    let mut bytes = Vec::new();
    for position in &mesh.positions {
        for value in position {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    for uv in &mesh.uvs {
        for value in uv {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
    }
    for triangle in &mesh.triangles {
        for value in triangle {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    if let Some(skin) = &mesh.skin {
        bytes.extend_from_slice(skin.bones.join("\0").as_bytes());
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn write_config(label: &str, body_json: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sculpto-materials-{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bodies.json");
    std::fs::write(&path, body_json).expect("config fixture writes");
    path
}

fn synthetic_skin() -> (PathBuf, SkinTextureSet) {
    let dir = std::env::temp_dir().join(format!("sculpto-materials-skin-{}", std::process::id()));
    let root = dir.join("2K - Default/textures/actors/character/female");
    std::fs::create_dir_all(&root).expect("skin dir");
    std::fs::write(root.join("femalebody_1.dds"), b"DDS fake diffuse").expect("diffuse");
    std::fs::write(root.join("femalebody_1_msn.dds"), b"DDS fake normal").expect("normal");
    std::fs::write(root.join("femalebody_1_s.dds"), b"DDS fake specular").expect("specular");
    (
        dir.clone(),
        SkinTextureSet {
            root: dir,
            diffuse: "2K - Default/textures/actors/character/female/femalebody_1.dds".into(),
            normal: "2K - Default/textures/actors/character/female/femalebody_1_msn.dds".into(),
            specular: "2K - Default/textures/actors/character/female/femalebody_1_s.dds".into(),
        },
    )
}

/// A minimal body carrying one authored material slot with a slot table.
fn textured_body() -> MeshAsset {
    MeshAsset {
        name: "BaseShape".into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2]],
        materials: vec![sculpto_core::mesh::MaterialSlot {
            name: "robe".into(),
            texture_set: vec![
                "textures\\robe\\old.dds".into(),
                "textures\\robe\\old_n.dds".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                "textures\\robe\\old_s.dds".into(),
            ],
            ..Default::default()
        }],
        ..MeshAsset::default()
    }
}

/// The configured Bijin female default wears the supplied skin and keeps its
/// geometry, UVs and rig byte-for-byte; the male default stays skinless.
#[test]
fn configured_bijin_female_default_carries_supplied_skin() {
    let names = [
        "SCULPTO_KITS_HOME",
        "SCULPTO_HEAD_HOME",
        "SCULPTO_BODYSLIDE_HOME",
        "SCULPTO_RIG_TEMPLATE_FIXTURES",
        "SCULPTO_MOD_FIXTURE",
    ];
    let missing: Vec<&str> = names
        .iter()
        .filter(|name| std::env::var_os(name).is_none())
        .copied()
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "SKIP configured_bijin_female_default_carries_supplied_skin: missing {missing:?}"
        );
        return;
    }
    let config_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/bodies.json");
    let config = load_bodies_config(&config_path).expect("config loads with fixture roots set");
    assert_eq!(config.bodies.len(), 2, "both default bodies are configured");
    let bijin = config
        .bodies
        .iter()
        .find(|body| body.name == "Female Bijin")
        .expect("the Bijin female default is configured");
    assert_eq!(bijin.family, "cbbe");
    let skin = bijin.skin.as_ref().expect("the Bijin default wears a skin");

    let mut body = load_shape(&bijin.body).expect("the Bijin body loads");
    assert_eq!(body.name, "BaseShape");
    assert_eq!(
        body.positions.len(),
        3471,
        "accepted Bijin body vertex count"
    );
    assert_eq!(
        body.triangles.len(),
        6516,
        "accepted Bijin body triangle count"
    );
    let before = fingerprint(&body);
    let material_before = body.materials.clone();

    apply_default_skin(&mut body, skin).expect("the supplied skin applies");
    let after = fingerprint(&body);
    assert_eq!(
        before, after,
        "a skin application moves no geometry, UV or rig fact"
    );
    assert_ne!(
        body.materials, material_before,
        "the skin must change the material"
    );
    for slot in [
        TextureSlot::Diffuse,
        TextureSlot::Normal,
        TextureSlot::Specular,
    ] {
        let host = skin.host_path(slot);
        assert!(
            host.is_file(),
            "skin {} map resolves loose: {}",
            slot.as_str(),
            host.display()
        );
        let authored = skin.authored_path(slot);
        assert!(
            authored.starts_with("textures\\actors\\character\\female\\"),
            "authored path {authored} stays data-relative"
        );
        assert_eq!(
            body.materials[0].texture(slot),
            Some(authored.as_str()),
            "the {slot:?} map is the supplied one"
        );
    }
    println!(
        "Female Bijin: diffuse `{}`",
        skin.authored_path(TextureSlot::Diffuse)
    );

    let male = config
        .bodies
        .iter()
        .find(|body| body.name == "Male HIMBO")
        .expect("the male default stays configured");
    assert!(
        male.skin.is_none(),
        "the male default carries no skin (male appearance unaffected)"
    );
    let _ = BodyDescriptor::clone(male);
}

/// A configured skin whose file is absent is a named refusal, never a silent
/// untextured fallback.
#[test]
fn missing_skin_asset_is_a_named_refusal() {
    let (root, mut skin) = synthetic_skin();
    skin.specular = "2K - Default/textures/actors/character/female/femalebody_1_gone.dds".into();
    let mut body = textured_body();
    let error = apply_default_skin(&mut body, &skin).expect_err("the specular map is absent");
    assert_eq!(error.kind(), ErrorKind::NotFound);
    assert_eq!(error.code(), "skin_missing");
    assert!(
        error.message().contains("femalebody_1_gone.dds"),
        "the refusal names the missing path: {}",
        error.message()
    );
    let _ = root;
}

/// A skin map outside a `textures/` data root cannot produce a game-relative
/// authored path and is refused at config load.
#[test]
fn skin_without_a_data_root_is_refused() {
    let path = write_config(
        "no-data-root",
        r#"{"bodies":[{"name":"b","family":"f","body":"body.obj","head":"h.obj","skin":{"root":"/tmp","diffuse":"options/plain.dds","normal":"options/n.dds","specular":"options/s.dds"}}]}"#,
    );
    let error = load_bodies_config(&path).expect_err("no textures/ data root");
    assert_eq!(error.code(), "invalid_value");
    assert!(
        error.message().contains("textures/"),
        "the refusal names the rule: {}",
        error.message()
    );
}

/// An unset fixture variable inside a skin root is the config's named
/// refusal, exactly like a body or head path.
#[test]
fn config_with_unset_skin_root_is_a_named_refusal() {
    let path = write_config(
        "unset-skin-root",
        r#"{"bodies":[{"name":"b","family":"f","body":"body.obj","head":"h.obj","skin":{"root":"${MOD03_DEFINITELY_UNSET}","diffuse":"o/textures/a.dds","normal":"o/textures/b.dds","specular":"o/textures/c.dds"}}]}"#,
    );
    let error = load_bodies_config(&path).expect_err("unset variable must refuse");
    assert_eq!(error.code(), "config_unresolved");
    assert!(
        error.message().contains("MOD03_DEFINITELY_UNSET"),
        "the refusal names the variable: {}",
        error.message()
    );
}

/// A material-less body gains exactly one skin-named slot; a body with an
/// authored table keeps its slot vocabulary and has slots 0, 1 and 7
/// overridden.
#[test]
fn skin_application_targets_the_material_only() {
    let (_root, skin) = synthetic_skin();
    let mut bare = textured_body();
    bare.materials.clear();
    apply_default_skin(&mut bare, &skin).expect("a bare body gains a slot");
    assert_eq!(bare.materials.len(), 1);
    assert_eq!(bare.materials[0].name, "BaseShape");
    assert_eq!(
        bare.materials[0].diffuse.as_deref(),
        Some("textures\\actors\\character\\female\\femalebody_1.dds")
    );

    let mut textured = textured_body();
    apply_default_skin(&mut textured, &skin).expect("an authored table is overridden in place");
    let table = &textured.materials[0].texture_set;
    assert_eq!(table.len(), 8, "the authored table keeps its own count");
    assert_eq!(
        table[0],
        "textures\\actors\\character\\female\\femalebody_1.dds"
    );
    assert_eq!(
        table[1],
        "textures\\actors\\character\\female\\femalebody_1_msn.dds"
    );
    assert_eq!(table[2], "", "an unassigned slot keeps its position");
    assert_eq!(
        table[7],
        "textures\\actors\\character\\female\\femalebody_1_s.dds"
    );
}
