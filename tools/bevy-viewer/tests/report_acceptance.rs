use std::path::{Path, PathBuf};
use std::process::Command;

fn viewer_binary() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let debug_bin = manifest_dir.join("target/debug/cloth-viewer");
    if debug_bin.exists() {
        return debug_bin;
    }
    let workspace_bin = manifest_dir.join("../../target/debug/cloth-viewer");
    if workspace_bin.exists() {
        return workspace_bin;
    }
    debug_bin
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
        if let Some(vulkan_loader) = find_vulkan_loader() {
            cmd.env("LD_LIBRARY_PATH", format!("{opengl_lib}:{vulkan_loader}"));
        } else {
            cmd.env("LD_LIBRARY_PATH", opengl_lib);
        }
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

#[test]
fn report_acceptance_on_synthetic_glb() {
    let bin = viewer_binary();
    if !bin.exists() {
        eprintln!("SKIP: cloth-viewer binary not found at {}", bin.display());
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/synthetic.glb");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());

    let mut cmd = Command::new(&bin);
    cmd.args(["--headless", "--report", "--glb"]).arg(&fixture);
    vulkan_env(&mut cmd);

    let output = cmd.output().expect("execute cloth-viewer");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if stderr.contains("Unable to find a GPU") || stdout.contains("Unable to find a GPU") {
            println!("SKIP: Host lacks GPU / Vulkan capability");
            return;
        }
        panic!(
            "cloth-viewer failed (status={:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
    }

    let json_start = stdout.find('{').expect("JSON object in stdout");
    let json_str = &stdout[json_start..];
    let val: serde_json::Value = serde_json::from_str(json_str.trim()).expect("valid JSON report");

    assert_eq!(val["profile"], "bevy");
    assert_eq!(val["bevy_version"], "0.20.1");
    assert_eq!(val["morph_limit"], 256);
    assert_eq!(val["morph_limit_verified"], true);
    assert_eq!(
        val["nodes"], 3,
        "synthetic mesh has 1 mesh node + 2 joint nodes"
    );
    assert_eq!(val["meshes"], 1);
    assert_eq!(val["vertices"], 3, "two-bone triangle has 3 vertices");
    assert_eq!(val["triangles"], 1, "two-bone triangle has 1 triangle");
    assert_eq!(val["materials"], 0);
}

#[test]
fn report_acceptance_on_tier1_glb() {
    let bin = viewer_binary();
    if !bin.exists() {
        eprintln!("SKIP: cloth-viewer binary not found at {}", bin.display());
        return;
    }
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tier1.glb");
    assert!(fixture.exists(), "fixture exists: {}", fixture.display());

    let mut cmd = Command::new(&bin);
    cmd.args(["--headless", "--report", "--glb"]).arg(&fixture);
    vulkan_env(&mut cmd);

    let output = cmd.output().expect("execute cloth-viewer");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        if stderr.contains("Unable to find a GPU") || stdout.contains("Unable to find a GPU") {
            println!("SKIP: Host lacks GPU / Vulkan capability");
            return;
        }
        panic!(
            "cloth-viewer failed (status={:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status
        );
    }

    let json_start = stdout.find('{').expect("JSON object in stdout");
    let json_str = &stdout[json_start..];
    let val: serde_json::Value = serde_json::from_str(json_str.trim()).expect("valid JSON report");

    assert_eq!(val["profile"], "bevy");
    assert_eq!(val["bevy_version"], "0.20.1");
    assert_eq!(val["morph_limit"], 256);
    assert_eq!(val["morph_limit_verified"], true);
    assert_eq!(val["nodes"], 1, "Tier-1 quad has 1 mesh node");
    assert_eq!(val["meshes"], 1);
    assert_eq!(val["vertices"], 4, "Tier-1 quad has 4 vertices");
    assert_eq!(val["triangles"], 2, "Tier-1 quad has 2 triangles");
    assert_eq!(val["materials"], 0);
}

#[test]
fn broken_export_fails_with_named_mismatch() {
    let bin = viewer_binary();
    if !bin.exists() {
        return;
    }
    let mut cmd = Command::new(&bin);
    cmd.args([
        "--headless",
        "--report",
        "--glb",
        "/nonexistent/path/broken.glb",
    ]);
    vulkan_env(&mut cmd);

    let output = cmd.output().expect("execute cloth-viewer");
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stderr.contains("Unable to find a GPU") {
        println!("SKIP: Host lacks GPU / Vulkan capability");
        return;
    }

    assert!(!output.status.success(), "broken export must fail");
    assert!(
        stderr.contains("REPORT_ERROR: failed to load glTF asset") || stderr.contains("broken.glb"),
        "error message must name the asset failure: {stderr}"
    );
}
