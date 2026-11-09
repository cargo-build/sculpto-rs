#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
VIEWER_DIR="$WORKSPACE_ROOT/tools/bevy-viewer"
VIEWER_BIN="$VIEWER_DIR/target/debug/cloth-viewer"

# Set up Vulkan environment for NixOS if not already configured
if [[ "${LD_LIBRARY_PATH:-}" != *"/run/opengl-driver/lib"* ]]; then
    VULKAN_LOADER=""
    for f in /nix/store/*vulkan-loader-1.4.363*/lib/libvulkan.so.1 /nix/store/*vulkan-loader*/lib/libvulkan.so.1; do
        if [[ -f "$f" ]] && readelf -h "$f" 2>/dev/null | grep -q "ELF64"; then
            VULKAN_LOADER="$f"
            break
        fi
    done
    if [[ -n "$VULKAN_LOADER" ]]; then
        export LD_LIBRARY_PATH="/run/opengl-driver/lib:$(dirname "$VULKAN_LOADER"):${LD_LIBRARY_PATH:-}"
    else
        export LD_LIBRARY_PATH="/run/opengl-driver/lib:${LD_LIBRARY_PATH:-}"
    fi
fi
if [[ -z "${VK_ICD_FILENAMES:-}" && -f "/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json" ]]; then
    export VK_ICD_FILENAMES="/run/opengl-driver/share/vulkan/icd.d/nvidia_icd.json"
fi
export WGPU_BACKEND="${WGPU_BACKEND:-vulkan}"

# Build viewer if binary does not exist
if [[ ! -x "$VIEWER_BIN" ]]; then
    echo "Building cloth-viewer..."
    flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo build --manifest-path "$VIEWER_DIR/Cargo.toml"
fi

FIXTURE_SYNTHETIC="$VIEWER_DIR/tests/fixtures/synthetic.glb"
FIXTURE_TIER1="$VIEWER_DIR/tests/fixtures/tier1.glb"

if [[ ! -f "$FIXTURE_SYNTHETIC" || ! -f "$FIXTURE_TIER1" ]]; then
    echo "Generating fixtures via sculpto-gltf example..."
    flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo run -p sculpto-gltf --example export_test_glb -- "$FIXTURE_SYNTHETIC"
    flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo run -p sculpto-gltf --example export_test_glb -- --tier1 "$FIXTURE_TIER1"
fi

echo "=== Test 1: Synthetic skinned GLB ==="
set +e
SYNTH_OUT=$("$VIEWER_BIN" --headless --report --glb "$FIXTURE_SYNTHETIC" 2>&1)
SYNTH_STATUS=$?
set -e

if [[ $SYNTH_STATUS -ne 0 ]]; then
    if echo "$SYNTH_OUT" | grep -q "Unable to find a GPU"; then
        echo "SKIP: Host lacks Vulkan/GPU capability"
        exit 0
    fi
    echo "FAIL: cloth-viewer failed with status $SYNTH_STATUS"
    echo "$SYNTH_OUT"
    exit 1
fi

echo "$SYNTH_OUT"

# Verify synthetic counts
echo "$SYNTH_OUT" | grep -q '"nodes": 3' || { echo "FAIL: expected nodes: 3"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"meshes": 1' || { echo "FAIL: expected meshes: 1"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"vertices": 3' || { echo "FAIL: expected vertices: 3"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"triangles": 1' || { echo "FAIL: expected triangles: 1"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"materials": 0' || { echo "FAIL: expected materials: 0"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"morph_limit": 256' || { echo "FAIL: expected morph_limit: 256"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"morph_limit_verified": true' || { echo "FAIL: expected morph_limit_verified: true"; exit 1; }
echo "$SYNTH_OUT" | grep -q '"bevy_version": "0.20.1"' || { echo "FAIL: expected bevy_version: 0.20.1"; exit 1; }

echo "=== Test 2: Tier-1 fixture GLB ==="
TIER1_OUT=$("$VIEWER_BIN" --headless --report --glb "$FIXTURE_TIER1" 2>&1)
echo "$TIER1_OUT"

echo "$TIER1_OUT" | grep -q '"nodes": 1' || { echo "FAIL: expected nodes: 1"; exit 1; }
echo "$TIER1_OUT" | grep -q '"meshes": 1' || { echo "FAIL: expected meshes: 1"; exit 1; }
echo "$TIER1_OUT" | grep -q '"vertices": 4' || { echo "FAIL: expected vertices: 4"; exit 1; }
echo "$TIER1_OUT" | grep -q '"triangles": 2' || { echo "FAIL: expected triangles: 2"; exit 1; }
echo "$TIER1_OUT" | grep -q '"materials": 0' || { echo "FAIL: expected materials: 0"; exit 1; }

echo "=== Test 3: Broken export negative test ==="
set +e
BROKEN_OUT=$("$VIEWER_BIN" --headless --report --glb "/nonexistent/path/broken.glb" 2>&1)
BROKEN_STATUS=$?
set -e

if [[ $BROKEN_STATUS -eq 0 ]]; then
    echo "FAIL: expected non-zero exit code on broken GLB"
    exit 1
fi
echo "$BROKEN_OUT" | grep -q "REPORT_ERROR" || { echo "FAIL: expected REPORT_ERROR in stderr"; exit 1; }

echo "ALL TESTS PASSED: Bevy consumer acceptance verified."
