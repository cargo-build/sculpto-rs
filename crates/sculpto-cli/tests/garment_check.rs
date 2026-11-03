use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn requested_self_check_is_an_exit_gate() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("sculpto-self-check-{}-{stamp}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, "v 10 0 0\nv 11 0 0\nv 10 1 0\nf 1 2 3\n").unwrap();
    fs::write(&garment, "v 0 0 0\nv 1 0 0\nv 0 0 1\nv 0.2 -0.1 0.2\nv 0.2 0.1 0.2\nv 0.8 0.1 0.2\nf 1 2 3\nf 4 5 6\n").unwrap();
    let run = |check_self| {
        Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["garment", "check", "--body"])
            .arg(&body)
            .arg("--garment")
            .arg(&garment)
            .args(["--self", check_self])
            .output()
            .unwrap()
    };
    let body_only = run("0");
    let full = run("1");
    fs::remove_dir_all(&directory).unwrap();
    assert!(body_only.status.success());
    assert!(!full.status.success());
    assert!(
        String::from_utf8(full.stdout)
            .unwrap()
            .contains("body pairs 0, self pairs 1")
    );
    assert!(
        String::from_utf8(full.stderr)
            .unwrap()
            .contains("nonadjacent garment triangle pairs intersect")
    );
}
