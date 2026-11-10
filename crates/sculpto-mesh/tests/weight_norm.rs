//! Acceptance for the native weight normalizer and the strict influence pack.
//!
//! Every case is synthetic and hand-computed from `BoneWeightAutoNormalizer`
//! (`WeightNorm.cpp:64-181`) and `ProjectOperations::NormalizeCopiedWeights`
//! (`ProjectOperations.cpp:627-647`): the reference arithmetic uses
//! `WEIGHT_EPSILON`, and the pack is our explicit stricter step.

use std::collections::BTreeSet;

use sculpto_mesh::garment::Influences;
use sculpto_mesh::weight_norm::{
    MAX_INFLUENCES, SUM_TOLERANCE, WEIGHT_EPSILON, WeightError, WeightNormalizer, pack_influences,
};

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_string()).collect()
}

fn locked(list: &[&str]) -> BTreeSet<String> {
    names(list).into_iter().collect()
}

fn influences(pairs: &[(&str, f32)]) -> Influences {
    pairs
        .iter()
        .map(|(bone, weight)| ((*bone).to_string(), *weight))
        .collect()
}

fn close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{label}: {actual} is not {expected}"
    );
}

fn mass(influences: &Influences) -> f32 {
    influences.values().sum()
}

#[test]
fn locked_three_tenths_and_modified_nine_tenths_scale_to_seven() {
    let normalizer =
        WeightNormalizer::new(&names(&["Chest"]), &locked(&["Spine"]), 1, false).expect("classes");
    let mut vertex = influences(&[("Spine", 0.3), ("Chest", 0.9)]);
    let report = normalizer.adjust(&mut vertex).expect("adjusted");
    close(vertex["Chest"], 0.7, "modified Chest");
    close(vertex["Spine"], 0.3, "locked Spine");
    close(report.modified_mass, 0.7, "modified mass");
    close(report.locked_mass, 0.3, "locked mass");
    close(mass(&vertex), 1.0, "sum");
}

#[test]
fn the_normalizable_class_absorbs_the_remainder() {
    let normalizer = WeightNormalizer::new(&names(&["Chest", "Hips"]), &BTreeSet::new(), 1, false)
        .expect("classes");
    let mut vertex = influences(&[("Chest", 0.2), ("Hips", 0.3)]);
    let report = normalizer.adjust(&mut vertex).expect("adjusted");
    // Normalizable mass 0.3 is scaled by (1 - 0.2) / 0.3.
    close(vertex["Chest"], 0.2, "modified Chest");
    close(vertex["Hips"], 0.8, "normalizable Hips");
    close(report.normalizable_mass, 0.8, "normalizable mass");
    close(report.spread_mass, 0.0, "no spread");
    close(mass(&vertex), 1.0, "sum");
}

#[test]
fn spread_fills_an_underweight_sum_only_when_asked() {
    let bones = names(&["Chest", "Hips"]);
    let spreading = WeightNormalizer::new(&bones, &BTreeSet::new(), 1, true).expect("classes");
    let mut vertex = influences(&[("Chest", 0.2)]);
    let report = spreading.adjust(&mut vertex).expect("adjusted");
    // The remainder 0.8 is divided over the single normalizable bone, which the
    // reference creates at zero first (WeightNorm.cpp:167-180).
    close(vertex["Hips"], 0.8, "spread Hips");
    close(report.spread_mass, 0.8, "spread mass");
    close(mass(&vertex), 1.0, "sum");

    let plain = WeightNormalizer::new(&bones, &BTreeSet::new(), 1, false).expect("classes");
    let mut underweight = influences(&[("Chest", 0.2)]);
    let plain_report = plain.adjust(&mut underweight).expect("adjusted");
    assert!(!underweight.contains_key("Hips"), "no bone is invented");
    close(underweight["Chest"], 0.2, "underweight Chest");
    close(plain_report.spread_mass, 0.0, "no spread");
}

#[test]
fn epsilon_edges_follow_the_native_thresholds() {
    // A locked total below the threshold is treated as zero (WeightNorm.cpp:93-98).
    let no_modified = WeightNormalizer::new(&[], &locked(&["Spine"]), 0, false).expect("classes");
    let tiny_lock = WEIGHT_EPSILON / 2.0;
    let mut locked_only = influences(&[("Spine", tiny_lock)]);
    let report = no_modified.adjust(&mut locked_only).expect("adjusted");
    close(report.locked_mass, 0.0, "rounded locked mass");
    assert_eq!(
        locked_only["Spine"].to_bits(),
        tiny_lock.to_bits(),
        "a locked weight is never rewritten"
    );

    // A modified weight below the threshold is zeroed, one near the ceiling is
    // raised to one (WeightNorm.cpp:110-117).
    let normalizer =
        WeightNormalizer::new(&names(&["Chest"]), &BTreeSet::new(), 1, false).expect("classes");
    let mut tiny = influences(&[("Chest", tiny_lock)]);
    normalizer.adjust(&mut tiny).expect("adjusted");
    assert_eq!(tiny["Chest"], 0.0);
    let mut ceiling = influences(&[("Chest", 0.9995)]);
    normalizer.adjust(&mut ceiling).expect("adjusted");
    assert_eq!(ceiling["Chest"], 1.0);
}

#[test]
fn invalid_classes_and_weights_are_named_errors() {
    let empty: BTreeSet<String> = BTreeSet::new();
    assert_eq!(
        WeightNormalizer::new(&names(&[""]), &empty, 0, false).expect_err("empty name"),
        WeightError::EmptyBoneName
    );
    assert!(matches!(
        WeightNormalizer::new(&names(&["Chest", "Chest"]), &empty, 0, false),
        Err(WeightError::DuplicateBone { .. })
    ));
    assert!(matches!(
        WeightNormalizer::new(&names(&["Chest"]), &locked(&["Chest"]), 1, false),
        Err(WeightError::AmbiguousClass { .. })
    ));
    assert!(matches!(
        WeightNormalizer::new(&names(&["Chest"]), &empty, 2, false),
        Err(WeightError::ModifiedOutOfRange {
            modified: 2,
            bones: 1
        })
    ));

    let normalizer = WeightNormalizer::new(&names(&["Chest"]), &empty, 1, false).expect("classes");
    let mut unclassified = influences(&[("Tail", 1.0)]);
    assert!(matches!(
        normalizer.adjust(&mut unclassified),
        Err(WeightError::UnclassifiedBone { .. })
    ));
    let mut non_finite = influences(&[("Chest", f32::NAN)]);
    assert!(matches!(
        normalizer.adjust(&mut non_finite),
        Err(WeightError::NonFiniteWeight { .. })
    ));
    let mut negative = influences(&[("Chest", -0.5)]);
    assert!(matches!(
        normalizer.adjust(&mut negative),
        Err(WeightError::NegativeWeight { .. })
    ));

    let over_locked = WeightNormalizer::new(&[], &locked(&["Spine"]), 0, false).expect("classes");
    let mut impossible = influences(&[("Spine", 1.2)]);
    assert!(matches!(
        over_locked.adjust(&mut impossible),
        Err(WeightError::ImpossibleLockedMass { .. })
    ));
}

#[test]
fn locked_bones_are_preserved_bit_exact() {
    let third = 1.0f32 / 3.0;
    let lock = locked(&["Spine"]);
    let normalizer = WeightNormalizer::new(&names(&["Chest"]), &lock, 1, false).expect("classes");
    let mut vertex = influences(&[("Spine", third), ("Chest", 0.2)]);
    normalizer.adjust(&mut vertex).expect("adjusted");
    assert_eq!(
        vertex["Spine"].to_bits(),
        third.to_bits(),
        "adjust keeps bits"
    );

    let packed = pack_influences(&vertex, &lock).expect("packed");
    assert_eq!(
        packed["Spine"].to_bits(),
        third.to_bits(),
        "pack keeps bits"
    );
    close(mass(&packed), 1.0, "packed sum");
}

#[test]
fn sums_stay_within_1e_4() {
    let lock = locked(&["Spine"]);
    let normalizer =
        WeightNormalizer::new(&names(&["Chest", "Hips"]), &lock, 1, false).expect("classes");
    let cases = [
        influences(&[("Spine", 0.0005), ("Chest", 0.4), ("Hips", 0.4)]),
        influences(&[("Spine", 0.3), ("Chest", 0.9)]),
        influences(&[("Chest", 0.1), ("Hips", 0.1)]),
        influences(&[("Chest", 2.0)]),
    ];
    for case in cases {
        let mut vertex = case.clone();
        normalizer.adjust(&mut vertex).expect("adjusted");
        let packed = pack_influences(&vertex, &lock).expect("packed");
        let sum = mass(&packed);
        assert!(
            (sum - 1.0).abs() <= SUM_TOLERANCE,
            "{case:?} packed to {packed:?} with sum {sum}"
        );
        assert!(
            packed.len() <= MAX_INFLUENCES,
            "{packed:?} exceeds the slots"
        );
    }
}

#[test]
fn six_positives_reduce_to_four() {
    let vertex = influences(&[
        ("A", 0.1),
        ("B", 0.2),
        ("C", 0.3),
        ("D", 0.1),
        ("E", 0.1),
        ("F", 0.2),
    ]);
    let packed = pack_influences(&vertex, &BTreeSet::new()).expect("packed");
    assert_eq!(packed.len(), MAX_INFLUENCES);
    // Largest first; the 0.1 tie resolves by ascending name, so A wins.
    assert_eq!(
        packed.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["A", "B", "C", "F"]
    );
    // The four kept weights sum to 0.8 and are scaled to the exact remainder.
    close(packed["C"], 0.375, "scaled C");
    close(mass(&packed), 1.0, "packed sum");
}

#[test]
fn a_tiny_locked_mass_is_retained_with_the_exact_remainder() {
    let tiny = WEIGHT_EPSILON / 2.0;
    let lock = locked(&["Spine"]);
    let vertex = influences(&[("Spine", tiny), ("Chest", 0.4)]);
    let packed = pack_influences(&vertex, &lock).expect("packed");
    // The pack uses the exact locked mass, not the normalizer's rounded zero.
    assert_eq!(packed["Spine"].to_bits(), tiny.to_bits());
    close(packed["Chest"], 1.0 - tiny, "unlocked remainder");
    close(mass(&packed), 1.0, "packed sum");
}

#[test]
fn packing_refuses_impossible_constraints() {
    let five_locks = influences(&[
        ("A", 0.1),
        ("B", 0.1),
        ("C", 0.1),
        ("D", 0.1),
        ("E", 0.1),
        ("Chest", 0.5),
    ]);
    let lock = locked(&["A", "B", "C", "D", "E"]);
    assert!(matches!(
        pack_influences(&five_locks, &lock),
        Err(WeightError::TooManyLocked {
            locked: 5,
            slots: 4
        })
    ));

    let over_mass = influences(&[("A", 0.7), ("B", 0.4)]);
    assert!(matches!(
        pack_influences(&over_mass, &locked(&["A", "B"])),
        Err(WeightError::LockedMassOverOne { .. })
    ));

    // Four locks fill every slot, so the positive remainder has nowhere to go.
    let full = influences(&[
        ("A", 0.25),
        ("B", 0.25),
        ("C", 0.2),
        ("D", 0.2),
        ("Chest", 0.1),
    ]);
    assert!(matches!(
        pack_influences(&full, &locked(&["A", "B", "C", "D"])),
        Err(WeightError::MissingUnlockedMass { .. })
    ));

    // A locked-only vertex with a remainder and no copied weight is refused.
    let locked_only = influences(&[("A", 0.6)]);
    assert!(matches!(
        pack_influences(&locked_only, &locked(&["A"])),
        Err(WeightError::MissingUnlockedMass { .. })
    ));
    // An empty vertex has a full remainder and nothing to carry it.
    let empty = Influences::new();
    assert!(matches!(
        pack_influences(&empty, &BTreeSet::new()),
        Err(WeightError::MissingUnlockedMass { remainder }) if remainder == 1.0
    ));
}
