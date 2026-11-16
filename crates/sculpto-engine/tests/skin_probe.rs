//! Real-fixture probe for `actor skin` (0164): author the 65-bone skin on the
//! seated Emily and Mike actors and confirm the ownership the product reports —
//! one owner mid-limb, a blend at the joint, no side leak, no leaf weight.
//!
//! Skipped when the private host fixtures are absent. Run with `--nocapture` to
//! read the census. @req DECL0008

use std::path::PathBuf;

use sculpto_engine::skin::{SkinParams, run};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

/// The authored report of one seated actor.
fn authored(name: &str) -> Option<serde_json::Value> {
    let actor = home().join(format!("Models/sculpto-rebind-20261116/{name}.glb"));
    if !actor.exists() {
        eprintln!("skip: seated fixture {name}.glb absent");
        return None;
    }
    Some(
        run(&SkinParams {
            expect_input_sha: None,
            actor,
            out: None,
            report: None,
        })
        .expect("the seated actor authors a skin"),
    )
}

/// The footprint row of one palette bone.
fn footprint<'a>(report: &'a serde_json::Value, bone: &str) -> &'a serde_json::Value {
    report["footprints"]
        .as_array()
        .expect("footprints")
        .iter()
        .find(|row| row["bone"].as_str() == Some(bone))
        .unwrap_or_else(|| panic!("no footprint for {bone}"))
}

#[test]
fn the_authored_skin_owns_one_region_per_bone_on_both_bodies() {
    for (name, bones) in [("emilyM", "3BA"), ("mikeM", "HIMBO - Body")] {
        let Some(report) = authored(name) else {
            return;
        };
        assert_eq!(
            report["palette"]["bones"], 65,
            "{bones}: the seated palette is the 65-bone Mixamo rig"
        );
        assert_eq!(
            report["palette"]["deform_bones"], 52,
            "{bones}: 13 leaf tips carry no flesh"
        );
        assert_eq!(
            report["palette"]["leaves_excluded"]
                .as_array()
                .unwrap()
                .len(),
            13
        );
        assert_eq!(report["validation"]["empty_rows"], 0);
        assert_eq!(report["validation"]["widest_row"], 4);
        assert!(report["validation"]["max_sum_error"].as_f64().unwrap() < 1e-5);
        assert_eq!(report["gates"]["side_isolation"]["left_violations"], 0);
        assert_eq!(report["gates"]["side_isolation"]["right_violations"], 0);
        assert_eq!(report["gates"]["leaf_weight_mass"].as_f64().unwrap(), 0.0);
        assert_eq!(report["gates"]["discarded_over_0_05"], 0);
        assert!(report["rest"]["max_vertex"].as_f64().unwrap() < 1e-4);

        // The upper arm is one owner, not a four-way split: 0158's measured
        // biceps problem is what the authored skin exists to answer.
        for side in ["Left", "Right"] {
            let arm = footprint(&report, &format!("mixamorig:{side}Arm"));
            let dominance = arm["dominance"].as_f64().unwrap();
            eprintln!(
                "{name}: {side}Arm dominance {dominance:.3} over {} vertices",
                arm["dominated"]
            );
            assert!(dominance > 0.75, "{name}: {side}Arm dominance {dominance}");
        }

        // A torso bone does not own a limb: the folded Mike skin gave Spine2 5.9 %
        // of the biceps zone, and the authored skin must not repeat it.
        let spine2 = footprint(&report, "mixamorig:Spine2");
        let arm = footprint(&report, "mixamorig:LeftArm");
        eprintln!(
            "{name}: Spine2 mass {:.3} Arm mass {:.3}",
            spine2["mass_share"], arm["mass_share"]
        );
        assert!(
            spine2["mass_share"].as_f64().unwrap() < 0.40,
            "{name}: Spine2 takes {:.3} of the body",
            spine2["mass_share"]
        );
    }
}

#[test]
fn every_bone_the_authoring_admits_carries_weight() {
    let Some(report) = authored("emilyM") else {
        return;
    };
    let unweighted = report["unweighted_deform_bones"].as_array().unwrap();
    eprintln!("unweighted deform bones: {unweighted:?}");
    assert!(
        unweighted.is_empty(),
        "a deform bone with no flesh: {unweighted:?}"
    );
}

/// The native T-rest bodies carry the Skyrim palette, where a zero-length `CME`
/// link sits in front of every `NPC` bone. The authored skin must give a ghost
/// twin no weight at all — two live twins would split a limb between them.
#[test]
fn a_ghost_twin_carries_no_weight_on_the_t_rest_bodies() {
    for name in ["emily-tpose", "mike-tpose"] {
        let actor = home().join(format!("Models/sculpto-tpose-20261116/{name}.glb"));
        if !actor.exists() {
            eprintln!("skip: t-rest fixture {name}.glb absent");
            continue;
        }
        let report = run(&SkinParams {
            expect_input_sha: None,
            actor,
            out: None,
            report: None,
        })
        .expect("the t-rest body authors a skin");
        let footprints = report["footprints"].as_array().unwrap();
        let ghost: f64 = footprints
            .iter()
            .filter(|row| row["bone"].as_str().unwrap_or_default().starts_with("CME"))
            .map(|row| row["mass_share"].as_f64().unwrap())
            .sum();
        eprintln!(
            "{name}: {} deform bones, {} coincident bones dropped, ghost mass {ghost:.6}",
            report["palette"]["deform_bones"],
            report["palette"]["coincident_bones_dropped"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default()
        );
        assert_eq!(ghost, 0.0, "{name}: a ghost twin carries weight");
        assert_eq!(report["validation"]["empty_rows"], 0);
        assert_eq!(report["validation"]["widest_row"], 4);
        assert!(report["validation"]["max_sum_error"].as_f64().unwrap() < 1e-5);
        assert_eq!(report["gates"]["side_isolation"]["left_violations"], 0);
        assert_eq!(report["gates"]["leaf_weight_mass"].as_f64().unwrap(), 0.0);
    }
}
