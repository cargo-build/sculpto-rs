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
        },
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.files, 5);
    assert_eq!(report.inserted, 5);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let categories: Vec<Category> = rows.iter().map(|(_, asset)| asset.category).collect();
    assert!(categories.contains(&Category::Preset));
    assert!(categories.contains(&Category::Face));
    assert!(categories.contains(&Category::Texture));
    assert!(categories.contains(&Category::Animation));
    assert!(categories.contains(&Category::Other));

    let again = index_root(
        &store,
        &root,
        "fixture",
        &IndexOptions {
            archive_entries: false,
        },
    );
    assert_eq!(again.inserted, 0);
    assert_eq!(again.replaced, 5);
    let _ = fs::remove_file(&db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn content_identity_is_stable_across_paths() {
    let first = temp_dir("identity-a");
    let second = temp_dir("identity-b");
    for root in [&first, &second] {
        fs::write(root.join("same.dds"), b"same bytes").expect("write");
    }
    let store = Store::open(first.with_extension("redb")).expect("store");
    let options = IndexOptions {
        archive_entries: false,
    };
    index_root(&store, &first, "a", &options);
    let rows = store.iter_kind::<Assets>().expect("rows");
    let before = rows[0].1.id.clone();
    index_root(&store, &second, "b", &options);
    let rows = store.iter_kind::<Assets>().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1.id, before);
    let _ = fs::remove_dir_all(first);
    let _ = fs::remove_dir_all(second);
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
