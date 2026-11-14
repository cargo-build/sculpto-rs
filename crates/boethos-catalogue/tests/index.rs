//! Indexer contract: classification, content identity and idempotent upsert.

use std::fs;
use std::path::{Path, PathBuf};

use boethos::Store;
use boethos_catalogue::{Assets, Category, IndexOptions, index_root};

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "boethos-catalogue-{}-{label}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp dir");
    path
}

fn fixture(root: &Path) {
    for (path, bytes) in [
        ("SliderPresets/Test.xml", b"<SliderPresets/>".as_slice()),
        (
            "meshes/actors/character/FaceGeom/skyrim.esm/0001.NIF",
            b"NIF".as_slice(),
        ),
        ("textures/skin.dds", b"DDS ".as_slice()),
        ("mixamo/animations/walk.fbx", b"FBX".as_slice()),
        ("body/body.nif", b"NIF-BODY".as_slice()),
        ("clothes/armor.nif", b"NIF-CLOTHES".as_slice()),
        ("hair/hair.nif", b"NIF-HAIR".as_slice()),
        ("readme.txt", b"notes".as_slice()),
    ] {
        let target = root.join(path);
        fs::create_dir_all(target.parent().expect("parent")).expect("dirs");
        fs::write(target, bytes).expect("write");
    }
}

#[test]
fn classifies_and_indexes_files() {
    let root = temp_dir("classify");
    fixture(&root);
    let db = root.with_extension("redb");
    let store = Store::open(&db).expect("store");
    let report = index_root(
        &store,
        &root,
        "fixture",
        &IndexOptions {
            archive_entries: false,
            prune: false,
        },
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.files, 8);
    assert_eq!(report.inserted, 8);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let categories: Vec<Category> = rows.iter().map(|(_, asset)| asset.category).collect();
    assert!(categories.contains(&Category::Preset));
    assert!(categories.contains(&Category::Face));
    assert!(categories.contains(&Category::Texture));
    assert!(categories.contains(&Category::Animation));
    assert!(categories.contains(&Category::Other));
    assert!(categories.contains(&Category::Body));
    assert!(categories.contains(&Category::Outfit));
    assert!(categories.contains(&Category::Hair));

    let again = index_root(
        &store,
        &root,
        "fixture",
        &IndexOptions {
            archive_entries: false,
            prune: false,
        },
    );
    assert_eq!(again.inserted, 0);
    assert_eq!(again.updated, 0);
    assert_eq!(again.unchanged, 8);
    let _ = fs::remove_file(&db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn equal_bytes_at_distinct_paths_stay_distinct() {
    let first = temp_dir("identity-a");
    let second = temp_dir("identity-b");
    for root in [&first, &second] {
        fs::write(root.join("same.dds"), b"same bytes").expect("write");
    }
    let store = Store::open(first.with_extension("redb")).expect("store");
    let options = IndexOptions {
        archive_entries: false,
        prune: false,
    };
    index_root(&store, &first, "a", &options);
    index_root(&store, &second, "b", &options);
    let rows = store.iter_kind::<Assets>().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].1.id, rows[1].1.id);
    assert_eq!(rows[0].1.sha256, rows[1].1.sha256);
    let _ = fs::remove_dir_all(first);
    let _ = fs::remove_dir_all(second);
}

#[test]
fn content_change_updates_and_prune_removes_gone_files() {
    let root = temp_dir("prune");
    fs::create_dir_all(root.join("presets")).expect("dirs");
    fs::write(root.join("presets/keep.xml"), b"one").expect("write");
    fs::write(root.join("presets/drop.xml"), b"two").expect("write");
    let db = root.with_extension("redb");
    let store = Store::open(&db).expect("store");
    let options = IndexOptions {
        archive_entries: false,
        prune: true,
    };
    let first = index_root(&store, &root, "fixture", &options);
    assert_eq!(first.inserted, 2);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let keep = rows
        .iter()
        .find(|(_, asset)| asset.name == "keep.xml")
        .expect("keep")
        .1
        .id
        .clone();
    fs::write(root.join("presets/keep.xml"), b"changed").expect("write");
    fs::remove_file(root.join("presets/drop.xml")).expect("remove");
    let second = index_root(&store, &root, "fixture", &options);
    assert_eq!(second.inserted, 0);
    assert_eq!(second.updated, 1);
    assert_eq!(second.pruned, 1);
    let rows = store.iter_kind::<Assets>().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.id, keep);
    assert_eq!(rows[0].1.bytes, Some(7));
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn family_tokens_do_not_match_substrings() {
    let root = temp_dir("family");
    fs::create_dir_all(root.join("PAN_NPCs")).expect("dirs");
    fs::write(root.join("PAN_NPCs/face.dds"), b"pan").expect("write");
    fs::create_dir_all(root.join("pants")).expect("dirs");
    fs::write(root.join("pants/leg.dds"), b"pants").expect("write");
    let store = Store::open(root.with_extension("redb")).expect("store");
    index_root(
        &store,
        &root,
        "fixture",
        &IndexOptions {
            archive_entries: false,
            prune: false,
        },
    );
    let rows = store.iter_kind::<Assets>().expect("rows");
    let family = |name: &str| {
        rows.iter()
            .find(|(_, asset)| asset.name == name)
            .and_then(|(_, asset)| asset.family.clone())
    };
    assert_eq!(family("face.dds").as_deref(), Some("pan"));
    assert_eq!(family("leg.dds"), None);
    let _ = fs::remove_file(root.with_extension("redb"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn indexes_archive_members_end_to_end() {
    let root = temp_dir("archive");
    fs::create_dir_all(root.join("SliderPresets")).expect("dirs");
    fs::write(root.join("SliderPresets/Test.xml"), b"<SliderPresets/>").expect("write");
    fs::create_dir_all(root.join("FaceGeom/skyrim.esm")).expect("dirs");
    fs::write(root.join("FaceGeom/skyrim.esm/0001.NIF"), b"NIF").expect("write");
    fs::create_dir_all(root.join("meshes/actors/character/hair")).expect("dirs");
    fs::write(
        root.join("meshes/actors/character/hair/hair01.nif"),
        b"HAIR",
    )
    .expect("write");
    fs::create_dir_all(root.join("meshes/clothes")).expect("dirs");
    fs::write(root.join("meshes/clothes/shirt.nif"), b"SHIRT").expect("write");
    let status = std::process::Command::new("7z")
        .current_dir(&root)
        .args([
            "a",
            "-y",
            "pack.7z",
            "SliderPresets/Test.xml",
            "FaceGeom/skyrim.esm/0001.NIF",
            "meshes/actors/character/hair/hair01.nif",
            "meshes/clothes/shirt.nif",
        ])
        .status()
        .expect("7z");
    assert!(status.success(), "7z could not create the fixture archive");
    fs::remove_dir_all(root.join("SliderPresets")).expect("remove");
    fs::remove_dir_all(root.join("FaceGeom")).expect("remove");
    fs::remove_dir_all(root.join("meshes")).expect("remove");
    let store = Store::open(root.with_extension("redb")).expect("store");
    let report = index_root(
        &store,
        &root,
        "from-skyrim-community",
        &IndexOptions {
            archive_entries: true,
            prune: false,
        },
    );
    assert_eq!(report.archives, 1);
    assert_eq!(report.members, 4, "{:?}", report.errors);
    let rows = store.iter_kind::<Assets>().expect("rows");
    assert!(
        rows.iter()
            .any(|(_, asset)| asset.category == Category::Face)
    );
    assert!(
        rows.iter()
            .any(|(_, asset)| asset.category == Category::Preset)
    );
    assert!(
        rows.iter()
            .any(|(_, asset)| asset.category == Category::Hair)
    );
    let shirt = rows
        .iter()
        .find(|(_, asset)| asset.name.ends_with("shirt.nif"))
        .map(|(_, asset)| asset)
        .expect("garment member");
    assert_eq!(shirt.category, Category::Outfit);
    assert!(
        shirt.tags.contains(&"wear:top".to_string()),
        "{:?}",
        shirt.tags
    );
    let _ = fs::remove_file(root.with_extension("redb"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn classifies_extracted_underwear_as_tagged_outfit() {
    let root = temp_dir("garment-tags");
    fs::write(root.join("female-cbbe-bra.obj"), b"bra").expect("write");
    fs::write(root.join("female-3ba-panty.obj"), b"panty").expect("write");
    fs::write(root.join("femalebody_0.nif"), b"body").expect("write");
    let db = root.with_extension("redb");
    let store = Store::open(&db).expect("store");
    let report = index_root(
        &store,
        &root,
        "underwear-extract",
        &IndexOptions {
            archive_entries: false,
            prune: false,
        },
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let row = |name: &str| {
        rows.iter()
            .find(|(_, asset)| asset.name == name)
            .map(|(_, asset)| asset)
            .expect(name)
    };
    let bra = row("female-cbbe-bra.obj");
    assert_eq!(bra.category, Category::Outfit);
    assert_eq!(bra.sex, Some(boethos_catalogue::Sex::Female));
    assert_eq!(bra.family.as_deref(), Some("cbbe"));
    for tag in [
        "wear:underwear",
        "wear:top",
        "garment:bra",
        "fit:tight",
        "layer:inner",
    ] {
        assert!(bra.tags.contains(&tag.to_string()), "{tag}: {:?}", bra.tags);
    }
    let panty = row("female-3ba-panty.obj");
    assert_eq!(panty.category, Category::Outfit);
    assert_eq!(panty.family.as_deref(), Some("3ba"));
    for tag in [
        "wear:underwear",
        "wear:bottom",
        "garment:panty",
        "fit:tight",
        "layer:inner",
    ] {
        assert!(
            panty.tags.contains(&tag.to_string()),
            "{tag}: {:?}",
            panty.tags
        );
    }
    assert_eq!(row("femalebody_0.nif").category, Category::Body);
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn garment_named_fbx_is_tagged_outfit() {
    let root = temp_dir("garment-fbx");
    fs::write(root.join("female-cbbe-armor.fbx"), b"FBX").expect("write");
    let db = root.with_extension("redb");
    let store = Store::open(&db).expect("store");
    let report = index_root(
        &store,
        &root,
        "tripo3d",
        &IndexOptions {
            archive_entries: false,
            prune: false,
        },
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let asset = rows.first().expect("garment row").1.clone();
    assert_eq!(asset.category, Category::Outfit);
    assert_eq!(asset.sex, Some(boethos_catalogue::Sex::Female));
    assert_eq!(asset.family.as_deref(), Some("cbbe"));
    for tag in ["wear:armor", "sex:female", "body:cbbe", "origin:tripo3d"] {
        assert!(
            asset.tags.contains(&tag.to_string()),
            "{tag}: {:?}",
            asset.tags
        );
    }
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn body_part_asset_keeps_shape_counts_and_origin() {
    let asset = boethos_catalogue::body_part_asset(&boethos_catalogue::BodyPartInput {
        name: "Feet".to_string(),
        shape: "Feet".to_string(),
        sex: Some(boethos_catalogue::Sex::Female),
        family: Some("cbbe-3ba".to_string()),
        source: "from-skyrim-community".to_string(),
        path: None,
        archive: Some("/assets/pack.7z".to_string()),
        entry: Some("meshes/.../femalefeet_0.nif".to_string()),
        sha256: Some("b6658f".to_string()),
        bytes: Some(123),
        tags: Vec::new(),
        vertices: 8_876,
        triangles: 16_288,
    })
    .expect("body part");
    assert_eq!(asset.category, Category::BodyPart);
    assert_eq!(asset.shape.as_deref(), Some("Feet"));
    assert_eq!(asset.vertices, Some(8_876));
    assert_eq!(asset.triangles, Some(16_288));
    assert_eq!(asset.family.as_deref(), Some("cbbe-3ba"));
    for tag in ["body-part", "part:feet", "origin:skyrimmods"] {
        assert!(
            asset.tags.contains(&tag.to_string()),
            "{tag}: {:?}",
            asset.tags
        );
    }
    assert_eq!(asset.path, "/assets/pack.7z::meshes/.../femalefeet_0.nif");
}

#[test]
fn body_part_bra_is_underwear_not_body() {
    let asset = boethos_catalogue::body_part_asset(&boethos_catalogue::BodyPartInput {
        name: "CBBE bra".to_string(),
        shape: "Bra".to_string(),
        sex: Some(boethos_catalogue::Sex::Female),
        family: Some("cbbe".to_string()),
        source: "from-skyrim-community".to_string(),
        path: None,
        archive: Some("/assets/pack.7z".to_string()),
        entry: Some("meshes/femalebody_0.nif".to_string()),
        sha256: None,
        bytes: None,
        tags: vec!["wear:underwear".to_string()],
        vertices: 500,
        triangles: 926,
    })
    .expect("bra");
    assert!(
        asset.tags.contains(&"part:underwear".to_string()),
        "{:?}",
        asset.tags
    );
    assert!(
        !asset.tags.contains(&"part:body".to_string()),
        "{:?}",
        asset.tags
    );
    let json = serde_json::to_value(&asset).expect("json");
    assert_eq!(json["category"], "body-part");
}

#[test]
fn body_part_asset_refuses_ambiguous_location() {
    let base = boethos_catalogue::BodyPartInput {
        name: "Feet".to_string(),
        shape: "Feet".to_string(),
        sex: None,
        family: None,
        source: "test".to_string(),
        path: Some("/assets/body.nif".to_string()),
        archive: Some("/assets/pack.7z".to_string()),
        entry: Some("meshes/feet.nif".to_string()),
        sha256: None,
        bytes: None,
        tags: Vec::new(),
        vertices: 1,
        triangles: 1,
    };
    assert!(boethos_catalogue::body_part_asset(&base).is_err());
    let mut path_with_entry = base.clone();
    path_with_entry.archive = None;
    assert!(boethos_catalogue::body_part_asset(&path_with_entry).is_err());
    let mut archive_without_entry = base.clone();
    archive_without_entry.path = None;
    archive_without_entry.entry = None;
    assert!(boethos_catalogue::body_part_asset(&archive_without_entry).is_err());
}

#[test]
fn raw_part_asset_keeps_source_digest_and_part_tags() {
    let asset = boethos_catalogue::raw_part_asset(&boethos_catalogue::RawPartInput {
        name: "female-mannequin-figure-01".to_string(),
        part: "figure-01".to_string(),
        category: Some("mesh".to_string()),
        sex: Some(boethos_catalogue::Sex::Female),
        family: None,
        source: "tripo3d-parts".to_string(),
        path: "/parts/female-mannequin-figure-01.glb".to_string(),
        source_path: Some("/assets/female mannequin 3d model.glb".to_string()),
        source_sha256: Some("597dfcf0".to_string()),
        sha256: Some("part-sha".to_string()),
        bytes: Some(42),
        tags: vec!["figure:01".to_string()],
        vertices: 197_966,
        triangles: 383_838,
    })
    .expect("raw part");
    assert_eq!(asset.category, Category::Mesh);
    assert_eq!(asset.shape.as_deref(), Some("figure-01"));
    assert_eq!(asset.source_sha256.as_deref(), Some("597dfcf0"));
    for tag in ["raw:part", "figure:01", "origin:tripo3d"] {
        assert!(
            asset.tags.contains(&tag.to_string()),
            "{tag}: {:?}",
            asset.tags
        );
    }
    assert_eq!(asset.vertices, Some(197_966));
}
