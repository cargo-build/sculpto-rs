use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn uv_report_is_written_on_geometric_refusal_and_invalid_data_is_explicit() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sculpto-uv-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("uv.json");
    let report = root.join("report.json");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["garment", "uv-check", "--uv"])
            .arg(&input)
            .arg("--report")
            .arg(&report)
            .output()
            .unwrap()
    };
    fs::write(
        &input,
        r#"{"triangles":[[[0,0],[1,0],[0,1]],[[0,0],[1,0],[0,1]],[[0,0],[0,0],[0,0]]]}"#,
    )
    .unwrap();
    assert!(!run().status.success());
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(value["overlap_pairs"], serde_json::json!([[0, 1]]));
    assert_eq!(value["degenerate_triangles"], serde_json::json!([2]));
    assert_eq!(value["verdict"]["passed"], false);
    let original = fs::read(&report).unwrap();
    assert!(!run().status.success());
    assert_eq!(fs::read(&report).unwrap(), original);
    fs::remove_file(&report).unwrap();
    fs::write(&input, r#"{"triangles":[[[0,0],[1,0],[0,1]]]}"#).unwrap();
    assert!(run().status.success());
    fs::remove_file(&report).unwrap();
    [
        r#"{"triangles":[[[0,0],[1,0],[0,1e100]]]}"#,
        r#"{"triangles":[[[0,0],[1,0],[0,null]]]}"#,
        "not json",
    ]
    .into_iter()
    .for_each(|bad| {
        fs::write(&input, bad).unwrap();
        let result = run();
        assert!(!result.status.success(), "{bad}");
        assert!(!result.stderr.is_empty());
        assert!(!report.exists());
    });
    fs::remove_dir_all(root).unwrap();
}
