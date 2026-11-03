#[path = "../../sculpto-mesh/tests/common/mod.rs"]
mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn run(input: &Path, output: &Path, report: Option<&Path>, pose: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command
        .args(["actor", "sample", "--input"])
        .arg(input)
        .args(["--part", "Body", "--out"])
        .arg(output)
        .args(pose);
    if let Some(path) = report {
        command.arg("--report").arg(path);
    }
    command.output().unwrap()
}

#[test]
fn actor_sample_preserves_y_up_and_reports_source_identity_and_errors() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sculpto-actor-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("actor.glb");
    let output = root.join("actor.obj");
    let report = root.join("actor.json");
    let mut fixture = common::Fixture::triangle();
    fixture.skin();
    fixture.channel(0, "translation", &[0., 1.], &[20., 0., 0., 30., 0., 0.]);
    fs::write(&input, fixture.bytes()).unwrap();
    let result = run(&input, &output, Some(&report), &["--rest"]);
    assert!(result.status.success(), "{result:?}");
    let obj = fs::read_to_string(&output).unwrap();
    assert!(obj.contains("v 11 2 2.25\n"), "{obj}");
    assert!(obj.contains("g node_2_mesh_0_primitive_0\nf 1 2 3\n"));
    let metadata: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(metadata["pose"]["kind"], "rest");
    assert_eq!(metadata["parts"][0]["name"], "Body");
    assert_eq!(metadata["parts"][0]["node"], 2);
    assert_eq!(metadata["parts"][0]["vertex_count"], 3);
    let result = run(&input, &output, None, &["--clip", "Run", "--time", "0"]);
    assert!(result.status.success(), "{result:?}");
    assert!(
        fs::read_to_string(&output)
            .unwrap()
            .contains("v 21 2 2.25\n")
    );
    let absent = root.join("absent.obj");
    [
        vec!["--clip", "missing", "--time", "0"],
        vec!["--clip", "Run", "--time", "2"],
        vec!["--rest", "--part", "missing"],
    ]
    .into_iter()
    .for_each(|pose| {
        assert!(!run(&input, &absent, None, &pose).status.success());
        assert!(!absent.exists());
    });
    let invalid = root.join("invalid.glb");
    fs::write(&invalid, b"invalid").unwrap();
    assert!(!run(&invalid, &absent, None, &["--rest"]).status.success());
    assert!(!absent.exists());
    [
        vec![],
        vec!["--rest", "--clip", "Run", "--time", "0"],
        vec!["--clip", "Run", "--time", "NaN"],
        vec!["--rest", "--rest"],
    ]
    .into_iter()
    .for_each(|pose| {
        assert_eq!(run(&input, &absent, None, &pose).status.code(), Some(2));
        assert!(!absent.exists());
    });
    #[cfg(target_os = "linux")]
    {
        let result = run(&input, Path::new("/dev/full"), None, &["--rest"]);
        assert!(!result.status.success());
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("/dev/full")
        );
        let result = run(&input, &output, Some(Path::new("/dev/full")), &["--rest"]);
        assert!(!result.status.success());
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("/dev/full")
        );
    }
    fs::remove_dir_all(root).unwrap();
}
