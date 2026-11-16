//! Real-fixture probe for `actor rebind` (0160): seat Emily's 3BA mesh on
//! Mixamo's 65-bone skeleton and confirm the named fold census the rig-matrix
//! experiment measured (0155): ~19.17 % folds, breast ~13.77 %, twist ~4.00 %.
//!
//! Skipped when the private host fixtures are absent. Run with `--nocapture` to
//! read the census. @req DECL0013

use std::path::PathBuf;

use sculpto_engine::rebind::{RebindParams, run};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

#[test]
fn emily_seats_on_mixamo_with_the_named_fold_census() {
    let actor = home().join("Models/sculpto-pelvis-reparent-20261116/emily-walk.glb");
    let skeleton = home().join("Models/sculpto-jokes-20261115/ybot_walking.glb");
    if !actor.exists() || !skeleton.exists() {
        eprintln!("skip: rebind fixtures absent");
        return;
    }
    let report = run(&RebindParams {
        actor,
        skeleton,
        out: None,
        report: None,
    })
    .expect("Emily rebinds onto Mixamo");

    assert_eq!(report["palette"]["target_bones"], 65);
    assert_eq!(report["palette"]["source_bones"], 126);
    assert_eq!(report["palette"]["mapped_source_bones"], 52);
    assert_eq!(report["validation"]["empty_rows"], 0);
    assert!(report["validation"]["max_sum_error"].as_f64().unwrap() < 1e-5);
    assert!(report["validation"]["rest"]["max_vertex"].as_f64().unwrap() < 1e-4);

    let folded = report["weight_census"]["folded_share"].as_f64().unwrap();
    let mapped = report["weight_census"]["mapped_share"].as_f64().unwrap();
    eprintln!("mapped {mapped:.4}  folded {folded:.4}");
    assert!((folded - 0.1917).abs() < 0.01, "folded share {folded}");
    assert!((mapped - 0.8083).abs() < 0.01, "mapped share {mapped}");

    let groups = report["weight_census"]["groups"].as_array().unwrap();
    let share = |name: &str| {
        groups
            .iter()
            .find(|group| group["group"] == name)
            .and_then(|group| group["share"].as_f64())
            .unwrap_or(0.0)
    };
    for group in groups {
        eprintln!(
            "  folded {:8} {:.4} -> {}",
            group["group"].as_str().unwrap(),
            group["share"].as_f64().unwrap(),
            group["targets"]
        );
    }
    let breast = share("breast");
    let twist = share("twist");
    eprintln!(
        "breast {breast:.4}  twist {twist:.4}  helpers {:.4}",
        share("helpers")
    );
    assert!((breast - 0.1377).abs() < 0.01, "breast share {breast}");
    assert!((twist - 0.04).abs() < 0.01, "twist share {twist}");

    // The heaviest target bones carry the body's mass.
    let mut per_bone: Vec<(f64, String)> = report["per_bone_mass"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["mass"].as_f64().unwrap(),
                row["bone"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    per_bone.sort_by(|left, right| right.0.total_cmp(&left.0));
    eprintln!("top target bones by mass:");
    for (mass, bone) in per_bone.iter().take(8) {
        eprintln!("  {bone:24} {mass:.1}");
    }
    assert!(per_bone[0].0 > 0.0);
}
