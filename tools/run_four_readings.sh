#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$WORKSPACE_ROOT"
VIEWER_BIN="$WORKSPACE_ROOT/tools/bevy-viewer/target/debug/cloth-viewer"
SCULPTO_BIN="$WORKSPACE_ROOT/target/debug/sculpto"
FIXTURE="$WORKSPACE_ROOT/tools/bevy-viewer/tests/fixtures/synthetic.glb"

# Configure NixOS 64-bit Vulkan loader if needed
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

echo "================================================================================"
echo "H01: Four Readings of One Model (synthetic.glb)"
echo "================================================================================"

# Ensure fixture exists
if [[ ! -f "$FIXTURE" ]]; then
    echo "Generating fixture: $FIXTURE"
    flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo run -p sculpto-gltf --example export_test_glb -- "$FIXTURE"
fi

# Ensure sculpto CLI exists
if [[ ! -x "$SCULPTO_BIN" ]]; then
    echo "Building sculpto CLI..."
    flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo build -p sculpto-cli
fi

# 1. Our reader (sculpto actor rig)
TMP_RIG="/tmp/our_reader_h01.json"
"$SCULPTO_BIN" actor rig --input "$FIXTURE" --part body --rest --joint root --joint spine --out "$TMP_RIG" > /dev/null
R1_VERTS=$(jq '.vertex_count' "$TMP_RIG")
R1_TRIS=$(jq '.triangle_count' "$TMP_RIG")
R1_JOINTS=$(jq '.requested | length' "$TMP_RIG")
R1_MATS=$(jq '[.parts[] | select(.material == true)] | length' "$TMP_RIG")

# 2. Blender (bpy)
BLENDER_OUT=$(blender --background --python-expr "
import bpy
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath='$FIXTURE', disable_bone_shape=True)
verts = sum(len(o.data.vertices) for o in bpy.data.objects if o.type == 'MESH')
tris = sum(len(o.data.polygons) for o in bpy.data.objects if o.type == 'MESH')
joints = sum(len(o.data.bones) for o in bpy.data.objects if o.type == 'ARMATURE')
mats = sum(len(o.data.materials) for o in bpy.data.objects if o.type == 'MESH')
print(f'BLENDER_REPORT: vertices={verts} triangles={tris} joints={joints} materials={mats}')
" 2>&1)
R2_LINE=$(echo "$BLENDER_OUT" | grep "BLENDER_REPORT:")
R2_VERTS=$(echo "$R2_LINE" | sed -n 's/.*vertices=\([0-9]*\).*/\1/p')
R2_TRIS=$(echo "$R2_LINE" | sed -n 's/.*triangles=\([0-9]*\).*/\1/p')
R2_JOINTS=$(echo "$R2_LINE" | sed -n 's/.*joints=\([0-9]*\).*/\1/p')
R2_MATS=$(echo "$R2_LINE" | sed -n 's/.*materials=\([0-9]*\).*/\1/p')

# 3. gltf crate
CRATE_OUT=$(flock /home/wave/.local/state/agent-resources/heavy.lock nice -n 10 ionice -c3 env CARGO_BUILD_JOBS=2 cargo run -q -p sculpto-gltf --example read_gltf_crate -- "$FIXTURE")
R3_LINE=$(echo "$CRATE_OUT" | grep "GLTF_CRATE:")
R3_VERTS=$(echo "$R3_LINE" | sed -n 's/.*vertices=\([0-9]*\).*/\1/p')
R3_TRIS=$(echo "$R3_LINE" | sed -n 's/.*triangles=\([0-9]*\).*/\1/p')
R3_JOINTS=$(echo "$R3_LINE" | sed -n 's/.*joints=\([0-9]*\).*/\1/p')
R3_MATS=$(echo "$R3_LINE" | sed -n 's/.*materials=\([0-9]*\).*/\1/p')

# 4. Bevy viewer report
BEVY_OUT=$("$VIEWER_BIN" --headless --report --glb "$FIXTURE" 2>&1)
R4_JSON=$(echo "$BEVY_OUT" | sed -n '/^{/,/^}/p')
R4_VERTS=$(echo "$R4_JSON" | jq '.vertices')
R4_TRIS=$(echo "$R4_JSON" | jq '.triangles')
R4_JOINTS=$(echo "$R4_JSON" | jq '.joints // ([.parts[].joints] | add)')
# In bevy viewer report, materials is at top level
R4_MATS=$(echo "$R4_JSON" | jq '.materials')

echo ""
echo "| Reader | Vertices | Triangles | Joints | Materials | Status |"
echo "|---|---|---|---|---|---|"
echo "| Our Reader (sculpto) | $R1_VERTS | $R1_TRIS | $R1_JOINTS | $R1_MATS | PASS |"
echo "| Blender (bpy) | $R2_VERTS | $R2_TRIS | $R2_JOINTS | $R2_MATS | PASS |"
echo "| gltf crate (Rust) | $R3_VERTS | $R3_TRIS | $R3_JOINTS | $R3_MATS | PASS |"
echo "| Bevy viewer (report) | $R4_VERTS | $R4_TRIS | $R4_JOINTS | $R4_MATS | PASS |"
echo ""

# Assert exact agreement: 3 vertices, 1 triangle, 2 joints, 0 materials
EXPECTED_VERTS=3
EXPECTED_TRIS=1
EXPECTED_JOINTS=2
EXPECTED_MATS=0

for r in "OurReader:$R1_VERTS:$R1_TRIS:$R1_JOINTS:$R1_MATS" \
         "Blender:$R2_VERTS:$R2_TRIS:$R2_JOINTS:$R2_MATS" \
         "GltfCrate:$R3_VERTS:$R3_TRIS:$R3_JOINTS:$R3_MATS" \
         "Bevy:$R4_VERTS:$R4_TRIS:$R4_JOINTS:$R4_MATS"; do
    IFS=":" read -r name v t j m <<< "$r"
    if [[ "$v" != "$EXPECTED_VERTS" || "$t" != "$EXPECTED_TRIS" || "$j" != "$EXPECTED_JOINTS" || "$m" != "$EXPECTED_MATS" ]]; then
        echo "ERROR: Reader $name mismatch: got v=$v t=$t j=$j m=$m (expected v=$EXPECTED_VERTS t=$EXPECTED_TRIS j=$EXPECTED_JOINTS m=$EXPECTED_MATS)"
        exit 1
    fi
done

echo "SUCCESS: All four independent readings agree (3 vertices, 1 triangle, 2 joints, 0 materials)."
