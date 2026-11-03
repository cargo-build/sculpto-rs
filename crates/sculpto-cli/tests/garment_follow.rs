use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn follow_preserves_the_obj_frame_and_render_order() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sculpto-follow-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let reference = root.join("reference.obj");
    let target = root.join("target.obj");
    let render = root.join("render.obj");
    let output = root.join("output.obj");
    fs::write(&reference, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
    fs::write(&target, "v 2 3 4\nv 3 3 4\nv 2 3 5\nf 1 2 3\n").unwrap();
    fs::write(
        &render,
        "v 0.2 0.3 0.1\nv 0.3 0.3 0.1\nv 0.2 0.4 0.1\nf 1 2 3\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["garment", "follow", "--reference"])
        .arg(reference)
        .arg("--target")
        .arg(target)
        .arg("--garment")
        .arg(render)
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let text = fs::read_to_string(output).unwrap();
    fs::remove_dir_all(root).unwrap();
    let positions: Vec<Vec<f32>> = text
        .lines()
        .filter_map(|line| line.strip_prefix("v "))
        .map(|line| {
            line.split_whitespace()
                .map(|value| value.parse().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(positions.len(), 3);
    positions
        .iter()
        .zip([[2.2, 2.9, 4.3], [2.3, 2.9, 4.3], [2.2, 2.9, 4.4]])
        .for_each(|(actual, expected)| {
            actual
                .iter()
                .zip(expected)
                .for_each(|(a, b)| assert!((a - b).abs() < 1e-6))
        });
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("f "))
            .collect::<Vec<_>>(),
        ["f 1 2 3"]
    );
}

fn follow(root: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["garment", "follow", "--reference"])
        .arg(root.join("control.obj"))
        .arg("--target")
        .arg(root.join("control.obj"))
        .arg("--garment")
        .arg(root.join("render.obj"))
        .arg("--out")
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn polygon_transport_rejects_invalid_indices_and_reports_write_errors() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("sculpto-follow-io-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let mesh = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n";
    fs::write(root.join("control.obj"), mesh).unwrap();
    fs::write(root.join("render.obj"), mesh).unwrap();
    let output = root.join("result.obj");
    assert!(follow(&root, &output).status.success());
    let text = fs::read_to_string(&output).unwrap();
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("f "))
            .collect::<Vec<_>>(),
        ["f 1 2 3", "f 1 3 4"]
    );
    #[cfg(target_os = "linux")]
    {
        let failed = follow(&root, Path::new("/dev/full"));
        assert!(!failed.status.success());
        assert!(
            String::from_utf8(failed.stderr)
                .unwrap()
                .contains("/dev/full")
        );
    }
    fs::write(
        root.join("render.obj"),
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 99\n",
    )
    .unwrap();
    let invalid = follow(&root, &output);
    fs::remove_dir_all(root).unwrap();
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("face references vertex 99")
    );
}
