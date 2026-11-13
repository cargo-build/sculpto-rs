//! Store contract tests: identity, versions, links and concurrent writers.

use std::sync::{Arc, Barrier};
use std::thread;

use boethos::{Kind, Put, Ref, Store};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Character {
    name: String,
    stage: String,
}

struct Characters;

impl Kind for Characters {
    const NAME: &'static str = "character";
    const VERSION: u32 = 1;
    type Record = Character;

    fn validate(record: &Self::Record) -> Result<(), String> {
        if record.name.trim().is_empty() {
            return Err("name must not be empty".to_string());
        }
        Ok(())
    }
}

struct CharactersV2;

impl Kind for CharactersV2 {
    const NAME: &'static str = "character";
    const VERSION: u32 = 2;
    type Record = Character;

    fn validate(_record: &Self::Record) -> Result<(), String> {
        Ok(())
    }
}

fn temp_path(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sculpto-store-{}-{label}-{nanos}.redb",
        std::process::id()
    ))
}

fn character(name: &str) -> Character {
    Character {
        name: name.to_string(),
        stage: "raw".to_string(),
    }
}

#[test]
fn insert_get_delete_round_trip() {
    let path = temp_path("round-trip");
    let store = Store::open(&path).expect("open");
    assert_eq!(
        store
            .insert::<Characters>("hero", &character("Hero"))
            .expect("insert"),
        Put::Inserted
    );
    assert_eq!(
        store.get::<Characters>("hero").expect("get"),
        Some(character("Hero"))
    );
    assert!(store.delete::<Characters>("hero").expect("delete"));
    assert_eq!(store.get::<Characters>("hero").expect("get"), None);
    let _ = std::fs::remove_file(path);
}

#[test]
fn insert_refuses_duplicate_and_upsert_replaces() {
    let path = temp_path("duplicate");
    let store = Store::open(&path).expect("open");
    store
        .insert::<Characters>("hero", &character("First"))
        .expect("insert");
    assert_eq!(
        store
            .insert::<Characters>("hero", &character("Second"))
            .expect("insert"),
        Put::Exists
    );
    assert_eq!(
        store
            .upsert::<Characters>("hero", &character("Second"))
            .expect("upsert"),
        Put::Replaced
    );
    assert_eq!(
        store.get::<Characters>("hero").expect("get"),
        Some(character("Second"))
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn version_mismatch_is_classified() {
    let path = temp_path("version");
    let store = Store::open(&path).expect("open");
    store
        .insert::<Characters>("hero", &character("Hero"))
        .expect("insert");
    let error = store
        .get::<CharactersV2>("hero")
        .expect_err("version mismatch");
    assert!(error.to_string().contains("version 1, expected 2"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn consumer_validation_refuses_write() {
    let path = temp_path("validation");
    let store = Store::open(&path).expect("open");
    let error = store
        .insert::<Characters>("hero", &character("  "))
        .expect_err("validation");
    assert!(error.to_string().contains("name must not be empty"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn links_are_unique_and_removable() {
    let path = temp_path("links");
    let store = Store::open(&path).expect("open");
    let from = Ref {
        kind: "character",
        id: "hero",
    };
    let to = Ref {
        kind: "artifact",
        id: "raw-mesh",
    };
    assert!(store.link(from, "produced", to).expect("link"));
    assert!(!store.link(from, "produced", to).expect("link"));
    let links = store.links_from(from).expect("links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].relation, "produced");
    assert_eq!(links[0].to_kind, "artifact");
    assert_eq!(links[0].to_id, "raw-mesh");
    assert!(store.unlink(from, "produced", to).expect("unlink"));
    assert!(store.links_from(from).expect("links").is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn iteration_is_ordered_and_export_is_one_line_per_record() {
    let path = temp_path("export");
    let store = Store::open(&path).expect("open");
    for id in ["c", "a", "b"] {
        store
            .insert::<Characters>(id, &character(id))
            .expect("insert");
    }
    let ids: Vec<String> = store
        .iter_kind::<Characters>()
        .expect("iter")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, ["a", "b", "c"]);

    let export = path.with_extension("jsonl");
    assert_eq!(store.export_jsonl(&export).expect("export"), 3);
    let text = std::fs::read_to_string(&export).expect("read");
    assert_eq!(text.lines().count(), 3);
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("json");
        assert_eq!(value["kind"], "character");
        assert_eq!(value["version"], 1);
    }
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(export);
}

#[test]
fn concurrent_inserts_admit_exactly_one_writer() {
    let path = temp_path("concurrent");
    let store = Arc::new(Store::open(&path).expect("open"));
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            store
                .insert::<Characters>("hero", &character("Hero"))
                .expect("insert")
        }));
    }
    let outcomes: Vec<Put> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert_eq!(
        outcomes.iter().filter(|put| **put == Put::Inserted).count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|put| **put == Put::Exists).count(),
        7
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn empty_identity_and_reserved_separator_are_refused() {
    let path = temp_path("keys");
    let store = Store::open(&path).expect("open");
    assert!(store.insert::<Characters>("", &character("Hero")).is_err());
    assert!(
        store
            .insert::<Characters>("bad\u{1f}id", &character("Hero"))
            .is_err()
    );
    let _ = std::fs::remove_file(path);
}
