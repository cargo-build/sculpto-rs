//! `export.actor` clip admission: role coverage and rest compatibility refuse
//! a clip that cannot drive the actor. @req DECL0008

use sculpto_engine::export_actor::{
    CLIP_ROLES, ExportActorParams, admit_clip, clip_coverage, clip_role, clip_value, run,
};
use sculpto_engine::export_glb::{ClipPlan, ClipSemantics, RootMotion};
use sculpto_gltf::Clip;

/// Emily's real 126-bone palette, as the export wrote it: the standard rig,
/// the 3BA physics bones, the finger chains and the scaffolding.
fn emily_palette() -> Vec<String> {
    [
        "Scene Root",
        "NPC L Clavicle [LClv]",
        "NPC L Thigh [LThg]",
        "NPC Pelvis [Pelv]",
        "NPC R Clavicle [RClv]",
        "NPC R Thigh [RThg]",
        "NPC Spine [Spn0]",
        "NPC Spine1 [Spn1]",
        "NPC Spine2 [Spn2]",
        "NPC L RearThigh",
        "NPC L Butt",
        "NPC R Butt",
        "L Breast01",
        "NPC R RearThigh",
        "NPC Belly",
        "L Breast03",
        "L Breast02",
        "R Breast03",
        "R Breast02",
        "R Breast01",
        "NPC L Pussy02",
        "NPC RT Anus2",
        "Clitoral1",
        "NPC LT Anus2",
        "NPC RB Anus2",
        "NPC R Pussy02",
        "VaginaB1",
        "NPC LB Anus2",
        "NPC R FrontThigh",
        "NPC L RearCalf [LrClf]",
        "NPC R RearCalf [RrClf]",
        "NPC L FrontThigh",
        "NPC L Calf [LClf]",
        "NPC L Forearm [LLar]",
        "NPC L ForearmTwist1 [LLt1]",
        "NPC L ForearmTwist2 [LLt2]",
        "NPC L UpperArm [LUar]",
        "NPC L UpperarmTwist1 [LUt1]",
        "NPC L UpperarmTwist2 [LUt2]",
        "NPC R Calf [RClf]",
        "NPC R Forearm [RLar]",
        "NPC R ForearmTwist1 [RLt1]",
        "NPC R ForearmTwist2 [RLt2]",
        "NPC R UpperArm [RUar]",
        "NPC R UpperarmTwist1 [RUt1]",
        "NPC R UpperarmTwist2 [RUt2]",
        "NPC L Foot [Lft ]",
        "NPC R Foot [Rft ]",
        "NPC L Hand [LHnd]",
        "NPC R Hand [RHnd]",
        "NPC",
        "NPC Root [Root]",
        "NPC COM [COM ]",
        "NPC L Toe0 [LToe]",
        "NPC L PreRearCalf",
        "NPC L PreFrontThigh",
        "NPC L PreRearThigh",
        "NPC R Toe0 [RToe]",
        "NPC R PreRearCalf",
        "NPC R PreFrontThigh",
        "NPC R PreRearThigh",
        "Genitals",
        "Pussy",
        "PussyA",
        "Clitoral",
        "VaginaB",
        "PussyXP",
        "NPC L Pussy01",
        "NPC R Pussy01",
        "Butt",
        "NPC R PreButt0",
        "NPC R PreButt",
        "NPC L PreButt0",
        "NPC L PreButt",
        "Anal",
        "NPC LT Anus1",
        "NPC RT Anus1",
        "NPC LB Anus1",
        "NPC RB Anus1",
        "NPC Neck [Neck]",
        "NPC Head [Head]",
        "NPC R Finger00 [RF00]",
        "NPC R Finger01 [RF01]",
        "NPC R Finger02 [RF02]",
        "NPC R Finger10 [RF10]",
        "NPC R Finger11 [RF11]",
        "NPC R Finger12 [RF12]",
        "NPC R Finger20 [RF20]",
        "NPC R Finger21 [RF21]",
        "NPC R Finger22 [RF22]",
        "NPC R Finger30 [RF30]",
        "NPC R Finger31 [RF31]",
        "NPC R Finger32 [RF32]",
        "NPC R Finger40 [RF40]",
        "NPC R Finger41 [RF41]",
        "NPC R Finger42 [RF42]",
        "NPC L Finger00 [LF00]",
        "NPC L Finger01 [LF01]",
        "NPC L Finger02 [LF02]",
        "NPC L Finger10 [LF10]",
        "NPC L Finger11 [LF11]",
        "NPC L Finger12 [LF12]",
        "NPC L Finger20 [LF20]",
        "NPC L Finger21 [LF21]",
        "NPC L Finger22 [LF22]",
        "NPC L Finger30 [LF30]",
        "NPC L Finger31 [LF31]",
        "NPC L Finger32 [LF32]",
        "NPC L Finger40 [LF40]",
        "NPC L Finger41 [LF41]",
        "NPC L Finger42 [LF42]",
        "Breast",
        "NPC L PreBreast",
        "L PreBreast00",
        "L Breast00",
        "L PreBreast01",
        "L PreBreast02",
        "L PreBreast03",
        "NPC R PreBreast",
        "R PreBreast00",
        "R Breast00",
        "R PreBreast01",
        "R PreBreast02",
        "R PreBreast03",
        "Belly",
        "HDT Belly",
    ]
    .iter()
    .map(|bone| (*bone).to_owned())
    .collect()
}

/// The 18 bones Emily's re-baked walk drives: legs, arms and spine, no toes,
/// neck, head or twist at all.
const EMILY_DRIVEN: [&str; 18] = [
    "NPC L Calf [LClf]",
    "NPC L Clavicle [LClv]",
    "NPC L Foot [Lft ]",
    "NPC L Forearm [LLar]",
    "NPC L Hand [LHnd]",
    "NPC L Thigh [LThg]",
    "NPC L UpperArm [LUar]",
    "NPC Pelvis [Pelv]",
    "NPC R Calf [RClf]",
    "NPC R Clavicle [RClv]",
    "NPC R Foot [Rft ]",
    "NPC R Forearm [RLar]",
    "NPC R Hand [RHnd]",
    "NPC R Thigh [RThg]",
    "NPC R UpperArm [RUar]",
    "NPC Spine [Spn0]",
    "NPC Spine1 [Spn1]",
    "NPC Spine2 [Spn2]",
];

/// A planned clip carrying only what the admission gate reads.
fn plan(name: &str, mapped: &[&str], rest_comparable: bool) -> ClipPlan {
    ClipPlan {
        clip: Clip {
            name: name.to_owned(),
            times: vec![0.0, 0.5, 1.0],
            channels: Vec::new(),
        },
        duration: 1.0,
        scale: 71.4286,
        mapped_bones: mapped.len(),
        mapped_bone_names: mapped.iter().map(|bone| (*bone).to_owned()).collect(),
        dropped_bones: Vec::new(),
        root_bones: 0,
        moving_channels: mapped.len(),
        max_motion_degrees: 108.9,
        max_motion_translation: 3.6,
        max_rest_rotation_degrees: match rest_comparable {
            true => 62.45,
            false => 179.88,
        },
        max_rest_translation_residual: match rest_comparable {
            true => 3.0e-8,
            false => 0.4811,
        },
        rest_pose_comparable: rest_comparable,
        rest_pose_outlier: match rest_comparable {
            true => "NPC R Hand [RHnd]".to_owned(),
            false => "CME R Finger11 [RF11]".to_owned(),
        },
        max_frame_delta_degrees: 115.3,
        max_swing_degrees: 115.3,
        frame_outlier: "NPC R Calf [RClf]".to_owned(),
        roll_normalization_degrees: 5.3,
        scale_min: 71.4,
        scale_max: 71.5,
        scale_shared: 1.0,
        scale_outliers: Vec::new(),
        semantics: ClipSemantics::Motion,
        axis_convention_degrees: 0.0,
        direction_error_median_degrees: 0.0,
        direction_error_max_degrees: 0.0,
        pose_error_median_degrees: 0.0,
        pose_error_max_degrees: 0.0,
        pose_error_outlier: String::new(),
        root_motion: RootMotion::Keep,
        root_motion_bone: None,
        source_root_bone: None,
        root_motion_travel: 0.0,
        root_travel: 0.0,
    }
}

fn required_all() -> Vec<String> {
    CLIP_ROLES.iter().map(|role| (*role).to_owned()).collect()
}

#[test]
fn clip_role_names_the_standard_rig_roles() {
    let cases: &[(&str, Option<&str>)] = &[
        ("NPC L Thigh [LThg]", Some("hips")),
        ("CME R Thigh [RThg]", Some("hips")),
        ("CME L Hip", Some("hips")),
        ("NPC L Calf [LClf]", Some("knees")),
        ("CME L Knee [LKne]", Some("knees")),
        ("NPC R Foot [Rft ]", Some("ankles")),
        ("NPC L Toe0 [LToe]", Some("toes")),
        ("NPC R UpperArm [RUar]", Some("shoulders")),
        ("NPC L Clavicle [LClv]", Some("shoulders")),
        ("CME R Shoulder", Some("shoulders")),
        ("NPC L Forearm [LLar]", Some("elbows")),
        ("NPC R Hand [RHnd]", Some("hands")),
        ("CME Neck [Neck]", Some("neck")),
        ("NPC Head [Head]", Some("head")),
        ("NPC L ForearmTwist1 [LLt1]", Some("twists")),
        ("NPC R UpperarmTwist2 [RUt2]", Some("twists")),
        // The torso chain: the hips carry it, and the mirror family roots its
        // upper body one level above the spine bones.
        ("NPC Spine [Spn0]", Some("spine")),
        ("NPC Spine1 [Spn1]", Some("spine")),
        ("CME Spine2 [Spn2]", Some("spine")),
        ("CME UBody [UBody]", Some("spine")),
        // Physics and scaffolding bones carry no role: a clip that drives only
        // these has not moved a limb.
        ("NPC L Butt", None),
        ("NPC L RearThigh", None),
        ("Scene Root", None),
        ("NPC COM [COM ]", None),
        ("NPC R Finger11 [RF11]", None),
    ];
    for (bone, role) in cases {
        assert_eq!(&clip_role(bone), role, "bone {bone:?}");
    }
}

#[test]
fn an_eighteen_of_126_clip_is_refused_when_roles_are_required() {
    let plan = plan("WalkForward", &EMILY_DRIVEN, true);
    let refused = admit_clip(&plan, &emily_palette(), &required_all(), false, false, 0.05)
        .expect_err("the stub clip drives no toe, neck or head at all");
    assert_eq!(refused.code(), "export_actor_clip_role_coverage");
    let message = format!("{refused}");
    for expected in [
        "\"WalkForward\"",
        "18 of 126 palette bones",
        "14.3 %",
        "toes, neck, head, twists",
        "covered: hips, spine, knees, ankles, shoulders, elbows, hands",
    ] {
        assert!(
            message.contains(expected),
            "the refusal names {expected:?}: {message}"
        );
    }
}

#[test]
fn a_rest_incompatible_clip_is_refused_by_default_and_admitted_with_the_flag() {
    let plan = plan("WalkForward", &EMILY_DRIVEN, false);
    let refused = admit_clip(&plan, &emily_palette(), &[], false, false, 0.05)
        .expect_err("a 0.48 m rest residual is not this rig at rest");
    assert_eq!(refused.code(), "export_actor_clip_rest_incompatible");
    let message = format!("{refused}");
    for expected in [
        "\"WalkForward\"",
        "179.88°",
        "0.4811",
        "0.0500",
        "CME R Finger11",
    ] {
        assert!(
            message.contains(expected),
            "the refusal names {expected:?}: {message}"
        );
    }
    let coverage = admit_clip(&plan, &emily_palette(), &[], true, false, 0.05)
        .expect("the override admits the measured clip");
    assert_eq!(coverage.mapped_bones, 18);
}

#[test]
fn a_partial_upper_body_clip_is_admitted_under_report_only() {
    let driven = [
        "NPC L Clavicle [LClv]",
        "NPC R Clavicle [RClv]",
        "NPC L UpperArm [LUar]",
        "NPC R UpperArm [RUar]",
        "NPC L Forearm [LLar]",
        "NPC R Forearm [RLar]",
        "NPC L Hand [LHnd]",
        "NPC R Hand [RHnd]",
        "NPC Neck [Neck]",
        "NPC Head [Head]",
        "NPC Spine [Spn0]",
        "NPC Spine1 [Spn1]",
        "NPC Spine2 [Spn2]",
    ];
    let plan = plan("Idle", &driven, true);
    let coverage =
        admit_clip(&plan, &emily_palette(), &[], false, false, 0.05).expect("no role is required");
    assert_eq!(
        coverage.roles_covered,
        vec!["spine", "shoulders", "elbows", "hands", "neck", "head"]
    );
    assert_eq!(
        coverage.roles_missing,
        vec!["hips", "knees", "ankles", "toes", "twists"]
    );
    // The report row shows the gap instead of blocking it.
    let row = clip_value(0, &plan, &coverage);
    assert_eq!(row["palette_bones"], 126);
    assert_eq!(
        row["untouched_bones"].as_array().map(Vec::len),
        Some(126 - 13)
    );
}

#[test]
fn a_clip_covering_every_role_passes_the_full_requirement() {
    let driven = [
        "NPC L Thigh [LThg]",
        "NPC R Thigh [RThg]",
        "NPC L Calf [LClf]",
        "CME R Knee [RKne]",
        "NPC L Foot [Lft ]",
        "CME R Foot [Rft ]",
        "NPC L Toe0 [LToe]",
        "NPC R Toe0 [RToe]",
        "NPC L UpperArm [LUar]",
        "CME R Clavicle [RClv]",
        "NPC L Forearm [LLar]",
        "NPC R Forearm [RLar]",
        "NPC L Hand [LHnd]",
        "CME R Hand [RHnd]",
        "NPC Neck [Neck]",
        "CME Neck [Neck]",
        "NPC Head [Head]",
        "NPC L ForearmTwist1 [LLt1]",
        "NPC R UpperarmTwist2 [RUt2]",
        "NPC Spine2 [Spn2]",
    ];
    let plan = plan("WalkForward", &driven, true);
    let coverage = admit_clip(&plan, &emily_palette(), &required_all(), false, false, 0.05)
        .expect("every required role has a driven bone");
    assert_eq!(coverage.roles_missing, Vec::<&str>::new());
    // Either family answers for the role, so the CME bones count with the NPC
    // ones they mirror.
    let hands = coverage.role_bones.get("hands").expect("the hands role");
    assert!(hands.contains(&"NPC L Hand [LHnd]".to_owned()));
    assert!(hands.contains(&"CME R Hand [RHnd]".to_owned()));
}

#[test]
fn a_pelvis_driven_clip_without_a_spine_bone_is_refused_as_a_rigid_torso() {
    let driven = [
        "NPC Pelvis [Pelv]",
        "NPC L Thigh [LThg]",
        "NPC R Thigh [RThg]",
        "NPC L Calf [LClf]",
        "NPC R Calf [RClf]",
        "NPC L Foot [Lft ]",
        "NPC R Foot [Rft ]",
    ];
    let plan = plan("Idle", &driven, true);
    let refused = admit_clip(&plan, &emily_palette(), &[], false, false, 0.05)
        .expect_err("the hips move and nothing above them does");
    assert_eq!(refused.code(), "export_actor_clip_torso_rigid");
    let message = format!("{refused}");
    for expected in [
        "\"Idle\"",
        "NPC Pelvis [Pelv]",
        "no spine bone",
        "allow_rigid_torso",
    ] {
        assert!(
            message.contains(expected),
            "the refusal names {expected:?}: {message}"
        );
    }
    // The declared pose is admitted, and the row says it was.
    let coverage = admit_clip(&plan, &emily_palette(), &[], false, true, 0.05)
        .expect("a rigid torso is a pose a caller may declare");
    let row = clip_value(0, &plan, &coverage);
    assert_eq!(row["rigid_torso_admitted"], true);
    assert!(
        row["roles_missing"]
            .as_array()
            .is_some_and(|roles| { roles.iter().any(|role| role.as_str() == Some("spine")) })
    );
}

#[test]
fn a_clip_that_never_drives_the_pelvis_is_not_a_rigid_torso() {
    let driven = ["NPC L Hand [LHnd]", "NPC R Hand [RHnd]"];
    let plan = plan("Wave", &driven, true);
    let coverage = admit_clip(&plan, &emily_palette(), &[], false, false, 0.05)
        .expect("a hand-only clip moves no hips, so no torso is carried rigidly");
    assert!(coverage.roles_missing.contains(&"spine"));
}

#[test]
fn the_report_row_keeps_its_keys_and_adds_the_coverage() {
    let palette = emily_palette();
    let comparable = plan("Idle", &EMILY_DRIVEN, true);
    let coverage = clip_coverage(&comparable, &palette);
    let row = clip_value(0, &comparable, &coverage);
    for key in [
        "index",
        "name",
        "duration",
        "samples",
        "channels",
        "mapped_bones",
        "palette_bones",
        "roles_covered",
        "roles_missing",
        "role_bones",
        "untouched_bones",
        "root_bones",
        "moving_channels",
        "unit_scale",
        "max_motion_degrees",
        "max_motion_translation",
        "max_rest_rotation_degrees",
        "max_rest_translation_residual",
        "rest_pose_comparable",
        "rest_pose_outlier",
        "max_frame_delta_degrees",
        "max_swing_degrees",
        "frame_outlier",
        "roll_normalization_degrees",
    ] {
        assert!(row.get(key).is_some(), "the clip row keeps `{key}`");
    }
    assert!(
        row.get("rest_incompatible_admitted").is_none(),
        "a comparable clip is not marked admitted"
    );
    let admitted = plan("WalkForward", &EMILY_DRIVEN, false);
    let row = clip_value(1, &admitted, &clip_coverage(&admitted, &palette));
    assert_eq!(row["rest_incompatible_admitted"], true);
}

#[test]
fn a_required_role_the_vocabulary_does_not_know_is_a_usage_refusal() {
    // `validate` runs before any file is read, so a dummy input path is enough.
    let params = ExportActorParams {
        nif: "absent.nif".into(),
        body_shape: "Body".into(),
        sources: Vec::new(),
        body: "absent.obj".into(),
        layers: Vec::new(),
        skeleton: None,
        clips: Some("clips.glb".into()),
        clip_names: vec!["WalkForward".into()],
        require_clip_roles: vec!["arms".into()],
        allow_incompatible_rest: false,
        allow_rigid_torso: false,
        units_per_metre: None,
        rest_translation_tolerance: None,
        rest_scale_tolerance: None,
        rest_frame_tolerance_degrees: None,
        reach_radius: None,
        layer_reach_radius: None,
        smooth_iterations: None,
        source_reach_tolerance: None,
        extremity_bones: Vec::new(),
        out: "out.glb".into(),
        report: None,
    };
    let refused = run(&params).expect_err("an unknown role name is a usage error");
    assert_eq!(refused.code(), "invalid_value");
    let message = format!("{refused}");
    assert!(
        message.contains("role \"arms\" is not a standard rig role"),
        "the refusal teaches the vocabulary: {message}"
    );
    assert!(
        message.contains(CLIP_ROLES.join(", ").as_str()),
        "the refusal lists the valid roles: {message}"
    );
}
