use sculpto_engine::{ErrorKind, catalogue, dispatch};
use serde_json::json;

fn input() -> serde_json::Value {
    json!({ "positions": [[0.,0.,0.], [1.,0.,0.], [0.,1.,0.]], "triangles": [[0,1,2]],
        "anchors": [{"vertex": 0, "target": [0.,0.,0.25]}], "collar_radius": 2., "max_shift": 0.5 })
}

#[test]
fn registry_exposes_and_executes_a_pure_seam_operation() {
    let spec = catalogue()
        .iter()
        .find(|s| s.id.as_str() == "component.seam_fit")
        .unwrap();
    assert!(!spec.mutates);
    assert!(spec.implemented());
    assert_eq!(spec.input_schema()["additionalProperties"], false);
    let out = dispatch("component.seam_fit", &input()).unwrap();
    assert_eq!(out["positions"][0], json!([0., 0., 0.25]));
    assert_eq!(out["changed_vertices"], json!([0, 1, 2]));
}

#[test]
fn required_lengths_unknown_keys_and_motion_refusals_are_classified() {
    let mut p = input();
    p.as_object_mut().unwrap().remove("collar_radius");
    assert_eq!(
        dispatch("component.seam_fit", &p).unwrap_err().kind(),
        ErrorKind::InvalidParams
    );
    let mut p = input();
    p["anchors"][0]["typo"] = json!(1);
    assert_eq!(
        dispatch("component.seam_fit", &p).unwrap_err().kind(),
        ErrorKind::InvalidParams
    );
    let mut p = input();
    p["max_shift"] = json!(0.1);
    let error = dispatch("component.seam_fit", &p).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "component_seam_shift_limit");
}
