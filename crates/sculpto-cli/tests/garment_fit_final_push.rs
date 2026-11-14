//! `garment fit` can request the opt-in final push-out stage. @req DECL0013
//!
//! The flags are argv-only in the sense `garment push` is: no command id is
//! added to the registry, and `outfit.fit` gains the same two optional
//! parameters so the JSON surface cannot drift from the flags.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::{Value, json};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

/// One single-shape NIF; `z` places the quad in the mesh frame.
fn quad_nif(path: &Path, name: &str, z: f32) {
    let asset = MeshAsset {
        name: name.into(),
        positions: vec![
            [-1.0, -1.0, z],
            [1.0, -1.0, z],
            [1.0, 1.0, z],
            [-1.0, 1.0, z],
        ],
        triangle_partitions: vec![PartitionId(0); 2],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        ..MeshAsset::default()
    };
    std::fs::write(path, write_se_shape(&asset).expect("synthetic NIF")).expect("write NIF");
}

fn body_obj(path: &Path) {
    std::fs::write(
        path,
        "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3\nf 1 3 4\n",
    )
    .expect("body OBJ");
}

struct Scene {
    root: PathBuf,
}

impl Scene {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "sculpto-final-push-fit-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("fixture directory");
        quad_nif(&root.join("garment.nif"), "Garment", 0.5);
        quad_nif(&root.join("body.nif"), "Body", 0.0);
        body_obj(&root.join("body.obj"));
        std::fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&json!({
                "outfit": "Final push garment",
                "nif": "garment.nif",
                "garment_shapes": [{"name": "Garment"}],
            }))
            .expect("manifest JSON"),
        )
        .expect("manifest");
        Self { root }
    }

    fn path(&self, name: &str) -> String {
        self.root.join(name).display().to_string()
    }

    /// The shared argument list of a manifest fit on the synthetic scene.
    fn fit_args(&self, out: &str) -> Vec<String> {
        vec![
            "garment".into(),
            "fit".into(),
            "--nif".into(),
            self.path("garment.nif"),
            "--shape".into(),
            "Garment".into(),
            "--body-shape".into(),
            "Body".into(),
            "--body-nif".into(),
            self.path("body.nif"),
            "--body".into(),
            self.path("body.obj"),
            "--manifest".into(),
            self.path("manifest.json"),
            "--out".into(),
            out.into(),
        ]
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove fixture directory");
    }
}

fn text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_fit_prints_the_stage_report_only_when_the_flags_are_given() {
    let scene = Scene::new("report");
    let plain = scene.fit_args(&scene.path("plain.obj"));
    let output = sculpto().args(&plain).output().expect("garment fit");
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stderr}");
    assert!(
        !stdout.contains("final push:"),
        "the stage must stay off by default: {stdout}"
    );

    let mut asked = scene.fit_args(&scene.path("pushed.obj"));
    asked.extend(["--final-push".into(), "2".into()]);
    asked.extend(["--final-push-tolerance".into(), "0.05".into()]);
    let output = sculpto().args(&asked).output().expect("garment fit");
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stdout.contains("final push: visible violations 0 (max 0.000000) at tolerance 0.050000 -> 0 (max 0.000000) after 0 of 2 passes; held 0"),
        "the stage report must name the caller's budget and tolerance: {stdout}"
    );
}

#[test]
fn the_registered_command_accepts_the_same_two_parameters() {
    let scene = Scene::new("run");
    let input = json!({
        "nif": scene.path("garment.nif"),
        "shape": "Garment",
        "body_shape": "Body",
        "body_nif": scene.path("body.nif"),
        "body": scene.path("body.obj"),
        "manifest": scene.path("manifest.json"),
        "out": scene.path("run.obj"),
        "final_push": 2,
        "final_push_tolerance": 0.05,
    });
    let output = sculpto()
        .args(["run", "--cmd", "outfit.fit", "--input", &input.to_string()])
        .output()
        .expect("run outfit.fit");
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stderr}");
    let value: Value = serde_json::from_str(&stdout).expect("one JSON envelope");
    assert_eq!(value["ok"], true, "{stdout}");
    let push = &value["result"]["regions"]["ordinary"]["final_push"];
    assert_eq!(push["budget"], 2, "{stdout}");
    assert_eq!(push["tolerance"], 0.05, "{stdout}");
}

#[test]
fn half_a_request_and_an_out_of_range_budget_are_refused_by_name() {
    let scene = Scene::new("refusals");

    let mut only_budget = scene.fit_args(&scene.path("no-tolerance.obj"));
    only_budget.extend(["--final-push".into(), "2".into()]);
    let output = sculpto().args(&only_budget).output().expect("garment fit");
    let (_, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(
        stderr.contains("`final_push_tolerance`: required when `final_push` is given"),
        "{stderr}"
    );

    let mut only_tolerance = scene.fit_args(&scene.path("no-budget.obj"));
    only_tolerance.extend(["--final-push-tolerance".into(), "0.05".into()]);
    let output = sculpto()
        .args(&only_tolerance)
        .output()
        .expect("garment fit");
    let (_, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(
        stderr.contains("`final_push`: required when `final_push_tolerance` is given"),
        "{stderr}"
    );

    let mut over_budget = scene.fit_args(&scene.path("over.obj"));
    over_budget.extend(["--final-push".into(), "11".into()]);
    over_budget.extend(["--final-push-tolerance".into(), "0.05".into()]);
    let output = sculpto().args(&over_budget).output().expect("garment fit");
    let (_, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(stderr.contains("outside 1..=10"), "{stderr}");

    let mut unparsable = scene.fit_args(&scene.path("nan.obj"));
    unparsable.extend(["--final-push".into(), "many".into()]);
    let output = sculpto().args(&unparsable).output().expect("garment fit");
    let (_, stderr) = text(&output);
    assert!(!output.status.success());
    assert!(stderr.contains("invalid --final-push"), "{stderr}");
}

#[test]
fn the_usage_lists_both_flags_under_the_fit_options() {
    let output = sculpto().output().expect("sculpto usage");
    let (stdout, stderr) = text(&output);
    let usage = format!("{stdout}{stderr}");
    let options = usage
        .split("garment fit options:")
        .nth(1)
        .expect("a garment fit options block");
    let block = options.split("garment follow options:").next().unwrap();
    assert!(block.contains("--final-push <n>"), "{block}");
    assert!(block.contains("--final-push-tolerance <units>"), "{block}");
}
