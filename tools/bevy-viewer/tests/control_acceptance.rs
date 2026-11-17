//! Acceptance for the agent control socket against the real binary.
//!
//! The socket is the one contract an agent depends on, so it is exercised end
//! to end: the digest must be the hash of the bytes on disk, a malformed line
//! must come back as a named error, a capture must write a PNG, and `quit` must
//! end the process. GPU-dependent, so a host without Vulkan skips like the
//! report acceptance does.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn viewer_binary() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let debug_bin = manifest_dir.join("target/debug/cloth-viewer");
    if debug_bin.exists() {
        return debug_bin;
    }
    manifest_dir.join("../../target/debug/cloth-viewer")
}

fn find_vulkan_loader() -> Option<String> {
    let entries = std::fs::read_dir("/nix/store").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.contains("vulkan-loader") && !name_str.ends_with(".drv") {
            let lib = entry.path().join("lib/libvulkan.so.1");
            if lib.exists() {
                return Some(entry.path().join("lib").to_string_lossy().to_string());
            }
        }
    }
    None
}

fn vulkan_env(cmd: &mut Command) {
    if std::env::var_os("LD_LIBRARY_PATH").is_none() {
        let opengl_lib = "/run/opengl-driver/lib";
        match find_vulkan_loader() {
            Some(loader) => cmd.env("LD_LIBRARY_PATH", format!("{opengl_lib}:{loader}")),
            None => cmd.env("LD_LIBRARY_PATH", opengl_lib),
        };
    }
    if std::env::var_os("VK_ICD_FILENAMES").is_none() {
        let icd = "/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json";
        if Path::new(icd).exists() {
            cmd.env("VK_ICD_FILENAMES", icd);
        }
    }
    if std::env::var_os("WGPU_BACKEND").is_none() {
        cmd.env("WGPU_BACKEND", "vulkan");
    }
}

/// Ask the socket one question and read its one-line answer.
fn ask(path: &Path, request: &str) -> serde_json::Value {
    let mut stream = UnixStream::connect(path).expect("connect to the control socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    stream
        .write_all(format!("{request}\n").as_bytes())
        .expect("write request");
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .expect("read reply");
    serde_json::from_str(line.trim()).unwrap_or_else(|_| panic!("reply is JSON: {line:?}"))
}

/// A PNG is evidence only once its `IEND` chunk is on disk. The capture answer
/// is queued a frame ahead of the write, so a caller that treats the reply — or
/// mere file existence — as completion can read a zero-byte or truncated file.
fn complete_png(path: &Path) -> Option<u64> {
    const IEND: &[u8] = b"\x00\x00\x00\x00IEND\xaeB`\x82";
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() <= IEND.len() || !bytes.ends_with(IEND) {
        return None;
    }
    Some(bytes.len() as u64)
}

/// Poll until a complete PNG exists, or give up after `limit`.
fn await_complete_png(path: &Path, limit: Duration) -> Option<u64> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(size) = complete_png(path) {
            return Some(size);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    // sha256sum is on every host this suite runs on; the point of the check is
    // that the viewer's own hash equals an external one, so do not reuse the
    // viewer's implementation.
    use std::io::Write as _;
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("run sha256sum");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(bytes)
        .expect("pipe bytes");
    let output = child.wait_with_output().expect("sha256sum output");
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

struct Viewer {
    child: std::process::Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Viewer {
    // The child is moved into `Viewer`, whose `Drop` kills and waits for it on
    // every path, including a panic in a test assertion.
    #[allow(clippy::zombie_processes)]
    fn start(fixture: &Path, tag: &str) -> Option<Viewer> {
        Self::start_mode(fixture, tag, "--glb")
    }

    #[allow(clippy::zombie_processes)]
    fn start_mode(fixture: &Path, tag: &str, mode: &str) -> Option<Viewer> {
        let bin = viewer_binary();
        if !bin.exists() {
            eprintln!("SKIP: cloth-viewer binary not found at {}", bin.display());
            return None;
        }
        let dir =
            std::env::temp_dir().join(format!("cloth-viewer-control-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("work dir");
        let socket = dir.join("viewer.sock");
        let mut cmd = Command::new(&bin);
        cmd.args(["--headless", mode])
            .arg(fixture)
            .arg("--control")
            .arg(&socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        vulkan_env(&mut cmd);
        let mut child = cmd.spawn().expect("start cloth-viewer");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if socket.exists() {
                return Some(Viewer { child, socket, dir });
            }
            if let Some(status) = child.try_wait().expect("poll viewer") {
                panic!("viewer exited before the socket appeared: {status:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        panic!("control socket did not appear at {}", socket.display());
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn control_socket_answers_identity_errors_capture_and_quit() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic.glb");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());
    let bytes = std::fs::read(&fixture).expect("read fixture");
    let digest = sha256_hex(&bytes);
    let Some(mut viewer) = Viewer::start(&fixture, "state") else {
        return;
    };

    let state = ask(&viewer.socket, r#"{"cmd":"state"}"#);
    assert_eq!(state["ok"], true, "state answers: {state}");
    assert_eq!(state["protocol"], 1);
    assert_eq!(
        state["asset"]["sha256"],
        digest.as_str(),
        "digest is the file's"
    );
    assert_eq!(state["asset"]["present"], true);
    assert_eq!(state["asset"]["file"], "synthetic.glb");
    assert_eq!(state["clip"]["count"], 0, "synthetic.glb carries no clips");
    assert_eq!(state["joint_count"], 0, "no mixamo/NPC-named joints here");

    let malformed = ask(&viewer.socket, "{ not json");
    assert_eq!(malformed["ok"], false);
    assert!(
        malformed["error"]
            .as_str()
            .unwrap_or_default()
            .starts_with("malformed JSON:"),
        "{malformed}"
    );

    let unknown = ask(&viewer.socket, r#"{"cmd":"teleport"}"#);
    assert_eq!(unknown["ok"], false);
    assert!(unknown["error"]
        .as_str()
        .unwrap_or_default()
        .contains("unknown command"));
    assert!(unknown["error"]
        .as_str()
        .unwrap_or_default()
        .contains("help"));

    // A clip index must be refused when the file carries none, instead of
    // playing a phantom graph node.
    let clip = ask(&viewer.socket, r#"{"cmd":"clip","index":1}"#);
    assert_eq!(clip["ok"], false);
    assert!(
        clip["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no animation clips"),
        "{clip}"
    );

    let shot = viewer.dir.join("capture/synthetic.png");
    let capture = ask(
        &viewer.socket,
        &format!(
            r#"{{"cmd":"capture","path":{:?}}}"#,
            shot.display().to_string()
        ),
    );
    assert_eq!(capture["ok"], true, "{capture}");
    let written = await_complete_png(&shot, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("capture PNG completes: {}", shot.display()));
    assert!(written > 1000, "capture PNG has pixels: {written} bytes");

    let quit = ask(&viewer.socket, r#"{"cmd":"quit"}"#);
    assert_eq!(quit["ok"], true, "{quit}");
    let status = viewer.child.wait().expect("viewer exits after quit");
    assert!(status.success(), "clean exit after quit: {status:?}");
}

/// The defect that stalled the Emily knee session: a headless control run used
/// to fire its one-shot capture and exit at frame 301 (about five seconds),
/// dropping the caller's third request. The session must outlive that interval.
#[test]
fn control_session_outlives_the_one_shot_exit() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic.glb");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());
    let Some(mut viewer) = Viewer::start(&fixture, "lifetime") else {
        return;
    };
    // The one-shot path exits after 301 frames at 60 Hz (~5.0 s). Staying
    // answerable past 6 s is the property under test.
    let started = Instant::now();
    let mut answered_at = Duration::ZERO;
    while started.elapsed() < Duration::from_secs(8) {
        let state = ask(&viewer.socket, r#"{"cmd":"state"}"#);
        assert_eq!(state["ok"], true, "state still answers: {state}");
        answered_at = started.elapsed();
        if answered_at >= Duration::from_secs(6) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(
        answered_at >= Duration::from_secs(6),
        "control answered for {answered_at:?}, which is not past the 5 s exit"
    );

    // A capture taken after that interval must still arrive and complete: this
    // was the third request the old build could not serve.
    let shot = viewer.dir.join("late/tail.png");
    let reply = ask(
        &viewer.socket,
        &format!(
            r#"{{"cmd":"capture","path":{:?}}}"#,
            shot.display().to_string()
        ),
    );
    assert_eq!(reply["ok"], true, "{reply}");
    let written = await_complete_png(&shot, Duration::from_secs(20))
        .unwrap_or_else(|| panic!("late capture completes: {}", shot.display()));
    assert!(written > 1000, "late capture has pixels: {written} bytes");

    let quit = ask(&viewer.socket, r#"{"cmd":"quit"}"#);
    assert_eq!(quit["ok"], true, "{quit}");
    let status = viewer.child.wait().expect("clean exit");
    assert!(status.success(), "clean exit after quit: {status:?}");
}

/// Without `--control` the one-shot path must stay exactly as it was: capture,
/// then exit on its own. This is the behaviour control mode is exempted from.
#[test]
fn one_shot_capture_still_exits_without_control() {
    let bin = viewer_binary();
    if !bin.exists() {
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic.glb");
    let dir = std::env::temp_dir().join(format!("cloth-viewer-oneshot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir");
    let shot = dir.join("oneshot.png");
    let mut cmd = Command::new(&bin);
    cmd.args(["--headless", "--glb"]).arg(&fixture);
    cmd.env("SCULPTO_VIEWER_PNG", &shot);
    vulkan_env(&mut cmd);
    let output = cmd.output().expect("run cloth-viewer");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Unable to find a GPU") {
        println!("SKIP: Host lacks GPU / Vulkan capability");
        return;
    }
    assert!(output.status.success(), "one-shot run exits 0: {stderr}");
    let written = await_complete_png(&shot, Duration::from_secs(10))
        .unwrap_or_else(|| panic!("one-shot PNG completes: {}", shot.display()));
    assert!(written > 1000, "one-shot PNG has pixels: {written} bytes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_asset_is_named_at_startup() {
    let bin = viewer_binary();
    if !bin.exists() {
        return;
    }
    let mut cmd = Command::new(&bin);
    cmd.args([
        "--headless",
        "--glb",
        "/nonexistent/posemy/reference.glb",
        "--control",
        "/tmp/cloth-viewer-control-missing.sock",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    vulkan_env(&mut cmd);
    let output = cmd.output().expect("run cloth-viewer");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Unable to find a GPU") {
        println!("SKIP: Host lacks GPU / Vulkan capability");
        return;
    }
    assert_eq!(
        output.status.code(),
        Some(2),
        "missing asset exits 2: {stderr}"
    );
    assert!(
        stderr.contains("asset not found: /nonexistent/posemy/reference.glb"),
        "names the missing file: {stderr}"
    );
}

#[test]
fn control_is_refused_with_report() {
    let bin = viewer_binary();
    if !bin.exists() {
        return;
    }
    let output = Command::new(&bin)
        .args([
            "--headless",
            "--report",
            "--glb",
            "tests/fixtures/synthetic.glb",
            "--control",
            "/tmp/cloth-viewer-control-report.sock",
        ])
        .output()
        .expect("run cloth-viewer");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("--control needs a running viewer"),
        "explains the refusal: {stderr}"
    );
    assert!(
        !Path::new("/tmp/cloth-viewer-control-report.sock").exists(),
        "no socket is left behind"
    );
}

fn await_bank_slot(viewer: &Viewer, slot: usize) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = ask(&viewer.socket, r#"{"cmd":"state"}"#);
        if state["asset"]["loaded"] == true
            && state["bank"]["current"] == slot
            && state["bank"]["requested"].is_null()
            && state["joint_count"] == 65
        {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "slot {slot} did not load: {state}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Private Mixamo bank: every slot, actual motion, opacity, camera and exact rest return.
#[test]
fn supplied_bank_switches_all_slots_and_returns_to_exact_rest() {
    let Some(path) = std::env::var_os("SCULPTO_VIEWER_BANK_FIXTURE") else {
        eprintln!("SKIP: SCULPTO_VIEWER_BANK_FIXTURE is not supplied");
        return;
    };
    let Some(viewer) = Viewer::start_mode(Path::new(&path), "bank", "--bank") else {
        return;
    };
    let first = await_bank_slot(&viewer, 0);
    let slots = first["bank"]["slots"].as_array().expect("bank slots");
    assert_eq!(slots.len(), 10);
    assert!(first["clip"]["index"].is_null());
    assert_eq!(
        ask(&viewer.socket, r#"{"cmd":"slot","index":10}"#)["ok"],
        false
    );
    assert_eq!(
        ask(&viewer.socket, r#"{"cmd":"clip","index":0}"#)["ok"],
        false
    );
    let camera = first["camera"].clone();
    let mut evidence = Vec::new();
    for index in 0..10 {
        assert_eq!(
            ask(
                &viewer.socket,
                &format!(r#"{{"cmd":"slot","index":{index}}}"#)
            )["ok"],
            true
        );
        let state = await_bank_slot(&viewer, index);
        assert_eq!(state["camera"], camera, "camera changed at slot {index}");
        assert_eq!(state["clip"]["index"].is_null(), index < 2);
        let materials = state["materials"].as_array().expect("actor materials");
        assert_eq!(materials.len(), 3, "complete dressed actor at slot {index}");
        for material in materials {
            let alpha = material["alpha"].as_f64().expect("alpha");
            assert!((alpha - 0.85).abs() < 1e-6, "slot{index}: {material}");
        }
        if index >= 2 {
            std::thread::sleep(Duration::from_millis(300));
            let later = ask(&viewer.socket, r#"{"cmd":"state"}"#);
            assert_ne!(state["joints"], later["joints"], "slot {index} is frozen");
            let hips = |state: &serde_json::Value| {
                state["joints"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|joint| joint["name"] == "mixamorig:Hips")
                    .unwrap()["position"]
                    .clone()
            };
            assert_eq!(hips(&state), hips(&later), "slot {index} root travelled");
        }
        evidence.push(serde_json::json!({"slot": index, "clip": state["clip"],
            "asset": state["asset"], "materials": state["materials"]}));
    }
    assert_eq!(
        ask(&viewer.socket, r#"{"cmd":"slot","index":0}"#)["ok"],
        true
    );
    let returned = await_bank_slot(&viewer, 0);
    assert_eq!(returned["asset"]["sha256"], first["asset"]["sha256"]);
    let by_name =
        |state: &serde_json::Value| -> std::collections::BTreeMap<String, serde_json::Value> {
            state["joints"]
                .as_array()
                .unwrap()
                .iter()
                .map(|joint| {
                    (
                        joint["name"].as_str().unwrap().to_owned(),
                        joint["position"].clone(),
                    )
                })
                .collect()
        };
    assert_eq!(
        by_name(&returned),
        by_name(&first),
        "A-rest transforms changed after motion"
    );
    if let Some(path) = std::env::var_os("SCULPTO_VIEWER_BANK_EVIDENCE") {
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).expect("bank evidence");
    }
}
