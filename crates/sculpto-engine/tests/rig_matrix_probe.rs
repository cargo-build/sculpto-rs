//! Real-fixture probe for the rig-matrix experiment (0155): resolve the native
//! NPC skeleton onto Mixamo's 65-bone palette through the correspondence layer,
//! and name every native track that finds no Mixamo target.
//!
//! Skipped when the private host fixtures are absent. Run with `--nocapture` to
//! read the mapping table. @req DECL0013

use std::collections::BTreeSet;
use std::path::PathBuf;

use sculpto_engine::adapt::actor_rig;
use sculpto_engine::retarget::Correspondence;
use sculpto_mesh::actor::Actor;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

#[test]
fn native_skeleton_resolves_onto_mixamo_palette() {
    let native = home().join("Models/sculpto-rig-matrix/native-rig.glb");
    let ybot = home().join("Models/sculpto-jokes-20261115/ybot_walking.glb");
    if !native.exists() || !ybot.exists() {
        eprintln!("skip: rig-matrix fixtures absent");
        return;
    }
    let source = Actor::from_glb(&std::fs::read(&native).unwrap()).unwrap();
    let target = Actor::from_glb(&std::fs::read(&ybot).unwrap()).unwrap();
    let (rig, palette) = actor_rig(&target).unwrap();
    eprintln!("target palette: {} bones", palette.len());

    let corr = Correspondence::build(&source, &rig, &palette).unwrap();
    let mapped: Vec<_> = corr.pairs.iter().filter(|p| p.source.is_some()).collect();
    eprintln!("mapped palette bones: {}", mapped.len());
    for p in &mapped {
        eprintln!(
            "  {:24} <- {:24} [{:?}] rest_angle {:.1}",
            p.palette,
            p.source_name.as_deref().unwrap_or("?"),
            p.method,
            p.rest_angle_degrees
        );
    }
    eprintln!("unmatched source bones: {}", corr.unmatched.len());
    for u in &corr.unmatched {
        eprintln!("  {:28} role={:?} — {}", u.name, u.role, u.reason);
    }

    // Every source node a palette bone claims (direct or folded).
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    for p in &corr.pairs {
        if let Some(name) = &p.source_name {
            claimed.insert(name.clone());
        }
    }
    // The native track inventory decoded by `actor havok` (99 tracks).
    let havok = home().join("Models/sculpto-rig-matrix/native-havok.json");
    if havok.exists() {
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&havok).unwrap()).unwrap();
        let tracks = report["track_bindings"].as_array().unwrap();
        let mut hit = 0;
        let mut miss: Vec<String> = Vec::new();
        for t in tracks {
            let bone = t["bone"].as_str().unwrap_or("");
            if claimed.contains(bone) {
                hit += 1;
            } else {
                miss.push(bone.to_owned());
            }
        }
        eprintln!(
            "native tracks finding a Mixamo target: {hit}/{}",
            tracks.len()
        );
        eprintln!("native tracks with NO Mixamo target ({}):", miss.len());
        for m in &miss {
            eprintln!("  {m}");
        }
        assert!(hit >= 40, "expected the anatomical core to map, got {hit}");
    }
    assert!(mapped.len() >= 50, "expected >=50 mapped bones");
}
