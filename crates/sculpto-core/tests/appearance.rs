//! Appearance catalogue reader tests (CAT-01 step 1).
//!
//! Two layers:
//! - Synthetic boethos records exercise compatibility, race exclusion and
//!   name refusals everywhere, with no host state.
//! - `SCULPTO_APPEARANCE_DB` opts into the real catalogue database and pins
//!   stable counts; without it the suite skips explicitly.

use std::path::PathBuf;

use boethos_catalogue::{Asset, Category, Sex, Status};
use sculpto_core::appearance::{
    AppearanceCatalogue, AppearanceError, FaceApplication, FaceRace, HeadRig,
    load_appearance_catalogue,
};

fn asset(id: &str, category: Category, name: &str, entry: Option<&str>, sex: Option<Sex>) -> Asset {
    let path = match entry {
        Some(entry) => format!("/fixtures/pack.7z::{entry}"),
        None => format!("/fixtures/{name}"),
    };
    Asset {
        id: id.to_string(),
        category,
        name: name.to_string(),
        path,
        source: "test".to_string(),
        archive: entry.map(|_| "/fixtures/pack.7z".to_string()),
        entry: entry.map(str::to_string),
        format: "nif".to_string(),
        shape: None,
        vertices: None,
        triangles: None,
        clip: None,
        sha256: None,
        source_sha256: None,
        bytes: None,
        sex,
        family: sex.map(|sex| match sex {
            Sex::Female => "bijin".to_string(),
            _ => "pride".to_string(),
        }),
        tags: Vec::new(),
        status: Status::Raw,
    }
}

fn bijin_face(id: &str, npc: &str) -> Asset {
    let entry = format!(
        "Pack/01 Characters/{npc}/meshes/actors/character/FaceGenData/FaceGeom/Skyrim.esm/00013D73.NIF"
    );
    asset(
        id,
        Category::Face,
        "Skyrim.esm/00013D73.NIF",
        Some(&entry),
        Some(Sex::Female),
    )
}

#[test]
fn compatible_faces_follow_body_family_sex() {
    let catalogue = AppearanceCatalogue::from_assets(&[
        bijin_face("a".repeat(64).as_str(), "Ingjard"),
        asset(
            "b".repeat(64).as_str(),
            Category::Face,
            "Skyrim.esm/00013255.nif",
            None,
            Some(Sex::Male),
        ),
    ]);
    let female = catalogue
        .compatible_faces("cbbe")
        .expect("cbbe is supported");
    assert_eq!(female.len(), 1, "only the female face pairs with cbbe");
    assert_eq!(female[0].npc.as_deref(), Some("Ingjard"));
    let male = catalogue
        .compatible_faces("himbo")
        .expect("himbo is supported");
    assert_eq!(male.len(), 1);
    assert_eq!(male[0].name, "skyrim.esm/00013255.nif");
}

#[test]
fn unknown_body_family_is_a_named_refusal() {
    let catalogue = AppearanceCatalogue::default();
    let error = catalogue
        .compatible_faces("xs")
        .expect_err("xs is not a supported body family");
    match &error {
        AppearanceError::UnknownBodyFamily {
            requested,
            supported,
        } => {
            assert_eq!(requested, "xs");
            assert!(
                supported.contains(&"himbo"),
                "refusal names the supported families"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn forbidden_race_evidence_excludes_a_face() {
    let catalogue = AppearanceCatalogue::from_assets(&[
        asset(
            "c".repeat(64).as_str(),
            Category::FacePreset,
            "Athis_HPH_darkElf.jslot",
            None,
            Some(Sex::Male),
        ),
        asset(
            "d".repeat(64).as_str(),
            Category::FacePreset,
            "Farkas_HPH_nord.jslot",
            None,
            Some(Sex::Male),
        ),
    ]);
    let dark = catalogue
        .face("Athis_HPH_darkElf")
        .expect("preset is in the catalogue");
    assert_eq!(dark.race, FaceRace::Forbidden("darkelf".to_string()));
    assert_eq!(dark.application, FaceApplication::RaceMenuSlot);
    assert!(!dark.application.headless());
    let compatible = catalogue
        .compatible_faces("himbo")
        .expect("himbo is supported");
    let names: Vec<&str> = compatible.iter().map(|face| face.name.as_str()).collect();
    assert_eq!(names, vec!["Farkas_HPH_nord"], "forbidden race drops out");
    assert_eq!(
        catalogue
            .face("Farkas_HPH_nord")
            .expect("nord preset kept")
            .race,
        FaceRace::Human("nord".to_string())
    );
}

#[test]
fn face_lookup_refuses_unknown_and_ambiguous_names() {
    let catalogue = AppearanceCatalogue::from_assets(&[
        bijin_face("e".repeat(64).as_str(), "Ingjard"),
        asset(
            "f".repeat(64).as_str(),
            Category::Face,
            "Skyrim.esm/00013D73.NIF",
            None,
            Some(Sex::Female),
        ),
    ]);
    let error = catalogue
        .face("skyrim.esm/99999999.nif")
        .expect_err("no record carries that name");
    assert!(
        matches!(error, AppearanceError::UnknownFace { .. }),
        "{error}"
    );
    let error = catalogue
        .face("skyrim.esm/00013d73.nif")
        .expect_err("the name matches two packs");
    match &error {
        AppearanceError::AmbiguousFace { matches, .. } => assert_eq!(matches.len(), 2),
        other => panic!("unexpected error: {other}"),
    }
    // Lowercased FaceGeom names are the canonical keys.
    let by_id = catalogue
        .face(&"e".repeat(64))
        .expect("exact boethos id resolves the ambiguity");
    assert_eq!(by_id.npc.as_deref(), Some("Ingjard"));
}

#[test]
fn hairstyles_pair_by_sex_and_keep_unknown_sex() {
    let catalogue = AppearanceCatalogue::from_assets(&[
        asset(
            "1".repeat(64).as_str(),
            Category::Hair,
            "ks_hair_01.nif",
            None,
            Some(Sex::Female),
        ),
        asset(
            "2".repeat(64).as_str(),
            Category::Hair,
            "apachii_hair.nif",
            None,
            None,
        ),
        asset(
            "3".repeat(64).as_str(),
            Category::Hair,
            "male_only.nif",
            None,
            Some(Sex::Male),
        ),
    ]);
    let female = catalogue
        .compatible_hairstyles("3ba")
        .expect("3ba is supported");
    let names: Vec<&str> = female.iter().map(|style| style.name.as_str()).collect();
    assert_eq!(names, vec!["apachii_hair.nif", "ks_hair_01.nif"]);
}

#[test]
fn unreadable_database_is_a_named_refusal() {
    let bogus = PathBuf::from("/tmp/opencode/sculpto-appearance-not-a-store.redb");
    std::fs::write(&bogus, b"not a redb store").expect("writable scratch file");
    let error =
        load_appearance_catalogue(&bogus).expect_err("an arbitrary file is not a boethos database");
    assert!(matches!(error, AppearanceError::Store { .. }), "{error}");
    let _ = std::fs::remove_file(&bogus);
}

fn database() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_APPEARANCE_DB").map(PathBuf::from)
}

#[test]
fn real_catalogue_pins_stable_counts_and_refusals() {
    let Some(db) = database() else {
        eprintln!("skipping: set SCULPTO_APPEARANCE_DB to a boethos assets.redb");
        return;
    };
    let catalogue = load_appearance_catalogue(&db).expect("catalogue database loads");

    // Pinned 2026-11-14 against from-skyrim-community + skyrim-presets roots.
    let baked: Vec<_> = catalogue
        .faces
        .iter()
        .filter(|face| face.application == FaceApplication::BakedFaceGeom)
        .collect();
    let racemenu: Vec<_> = catalogue
        .faces
        .iter()
        .filter(|face| face.application == FaceApplication::RaceMenuSlot)
        .collect();
    assert_eq!(baked.len(), 466, "FaceGeom NIF face count");
    assert_eq!(racemenu.len(), 52, "RaceMenu .jslot preset count");

    let count = |faces: &[&sculpto_core::appearance::HeadEntry], sex: Sex| {
        faces.iter().filter(|face| face.sex == sex).count()
    };
    assert_eq!(count(&baked, Sex::Female), 51, "Bijin female faces");
    assert_eq!(count(&baked, Sex::Male), 368, "Pride male faces");
    assert_eq!(
        count(&baked, Sex::Unknown),
        47,
        "unsexed AIO-YBMT/Daring faces"
    );
    assert_eq!(count(&racemenu, Sex::Male), 37, "Pride male presets");
    assert_eq!(
        count(&racemenu, Sex::Unknown),
        15,
        "AIO/R246 unsexed presets"
    );
    assert!(
        baked.iter().all(|face| face.application.headless()),
        "every baked FaceGeom is headless-applicable"
    );

    let female = catalogue
        .compatible_faces("cbbe")
        .expect("cbbe is supported");
    assert_eq!(
        female.len(),
        51,
        "all Bijin faces pair with the female body"
    );
    let male = catalogue
        .compatible_faces("himbo")
        .expect("himbo is supported");
    // 368 baked FaceGeom + 24 race-eligible male presets; 13 of the 37 male
    // presets carry forbidden elf/orc tokens and drop out.
    assert_eq!(
        male.len(),
        392,
        "himbo-compatible faces: baked plus eligible presets"
    );
    assert_eq!(
        male.iter()
            .filter(|face| face.application.headless())
            .count(),
        368,
        "the headless subset is exactly the baked Pride faces"
    );
    assert!(
        male.iter()
            .all(|face| face.sex == Sex::Male && face.race.eligible()),
        "compatible male faces keep sex and race gates"
    );

    let forbidden: Vec<_> = catalogue
        .faces
        .iter()
        .filter(|face| matches!(face.race, FaceRace::Forbidden(_)))
        .collect();
    assert_eq!(
        forbidden.len(),
        13,
        "forbidden elf/orc preset evidence stays visible as exclusions"
    );
    let athis = catalogue
        .face("Athis_HPH_darkElf")
        .expect("forbidden preset remains addressable by name");
    assert!(!male.iter().any(|face| face.id == athis.id));

    let error = catalogue
        .face("skyrim.esm/000deadbeef.nif")
        .expect_err("no such record");
    assert!(
        matches!(error, AppearanceError::UnknownFace { .. }),
        "{error}"
    );
    let error = catalogue
        .face("dawnguard.esm/0001541b.nif")
        .expect_err("shipped by Warmaidens and AIO-YBMT");
    match &error {
        AppearanceError::AmbiguousFace { matches, .. } => assert_eq!(matches.len(), 2),
        other => panic!("unexpected error: {other}"),
    }

    assert!(
        !catalogue.hairstyles.is_empty(),
        "hair members are indexed for the supplied packs"
    );
    assert!(
        catalogue
            .hairstyles
            .iter()
            .all(|hair| hair.rig == HeadRig::SkyrimHumanHead && hair.name.ends_with(".nif")),
        "every hair record is a static human-head NIF"
    );
}
