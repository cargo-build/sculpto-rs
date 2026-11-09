//! The two GLB repairs through the command surface, in the shape the tooling
//! calls them: `sculpto run --cmd <id> --input '{"glb": …}'`.

#[path = "../../sculpto-gltf/tests/common/mod.rs"]
mod fixtures;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

struct Work {
    root: PathBuf,
}

impl Work {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("a clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sculpto-gltf-{}-{stamp}", std::process::id()));
        fs::create_dir(&root).expect("a work directory");
        Self { root }
    }

    fn glb(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.glb(name);
        fs::write(&path, bytes).expect("write the fixture");
        path
    }

    /// The tooling's invocation: one command id, one JSON payload.
    fn run(&self, command: &str, glb: &Path) -> (Output, Value) {
        let payload = json!({ "glb": glb }).to_string();
        let output = Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["run", "--cmd", command, "--input", &payload])
            .output()
            .expect("run sculpto");
        let envelope: Value =
            serde_json::from_slice(&output.stdout).expect("one JSON envelope on stdout");
        (output, envelope)
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove the work directory");
    }
}

#[test]
fn the_registry_repairs_a_written_glb_in_place() {
    let work = Work::new();
    let path = work.write("cloth.glb", &fixtures::defective());

    let (output, envelope) = work.run("gltf.reset_weights", &path);
    assert!(output.status.success(), "{envelope}");
    assert_eq!(envelope["ok"], true);
    assert_eq!(envelope["result"]["meshes"], 1);
    assert_eq!(
        envelope["result"]["bytes"].as_u64(),
        Some(fs::metadata(&path).expect("the file").len())
    );
    let bytes = fs::read(&path).expect("the repaired file");
    assert_eq!(
        fixtures::json_chunk(&bytes)["meshes"][0]["weights"],
        json!([0.0, 0.0])
    );

    let (output, envelope) = work.run("gltf.clamp_texcoord", &path);
    assert!(output.status.success(), "{envelope}");
    assert_eq!(envelope["result"]["texcoords"], 2);
    let bytes = fs::read(&path).expect("the repaired file");
    gltf::Gltf::from_slice(&bytes).expect("the repaired GLB loads");
    let material = fixtures::json_chunk(&bytes)["materials"][0].clone();
    assert_eq!(
        material["pbrMetallicRoughness"]["baseColorTexture"]["texCoord"],
        0
    );
    assert_eq!(material["normalTexture"]["texCoord"], 0);

    // A second pass is a no-op on the bytes, so the tooling can re-run safely.
    let before = fs::read(&path).expect("the repaired file");
    let (_, envelope) = work.run("gltf.reset_weights", &path);
    assert_eq!(envelope["result"]["meshes"], 0, "{envelope}");
    let (_, envelope) = work.run("gltf.clamp_texcoord", &path);
    assert_eq!(envelope["result"]["texcoords"], 0, "{envelope}");
    assert_eq!(fs::read(&path).expect("the repaired file"), before);
}

#[test]
fn a_missing_file_is_an_environment_failure() {
    let work = Work::new();
    let (output, envelope) = work.run("gltf.reset_weights", &work.glb("absent.glb"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["kind"], "io");
    assert_eq!(envelope["error"]["code"], "io_error");
}

#[test]
fn a_file_that_is_not_a_glb_is_an_invalid_request() {
    let work = Work::new();
    let path = work.write("notes.txt", b"not a glb, just some ordinary bytes");
    let (output, envelope) = work.run("gltf.clamp_texcoord", &path);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(envelope["error"]["kind"], "invalid_request");
    assert_eq!(envelope["error"]["code"], "glb_not_glb");
    assert_eq!(
        fs::read(&path).expect("the untouched file"),
        b"not a glb, just some ordinary bytes"
    );
}
