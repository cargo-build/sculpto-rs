#!/usr/bin/env bash
# Headless Unity glTF import probe for sculpto GLB assets.
#
# Creates a throwaway project under /tmp, adds glTFast 6.14.0, copies input
# GLBs, runs ImportProbe.Run and prints the JSON report. Requires Unity
# 6000.6.4f1 from the Hub and the NixOS launcher facts recorded in README.md.
set -euo pipefail

PROBE_DIR=${PROBE_DIR:-/tmp/sculpto-classify/cloth/unity-probe}
EDITOR=${EDITOR:-$HOME/Unity/Hub/Editor/6000.6.4f1/Editor/Unity}
HUB_FHS=${HUB_FHS:-/nix/store/46z4ihvk6g5hxx6426ihrr99vv74lc16-unityhub-fhs-env-3.19.5/bin/unityhub-fhs-env}
NCURSES=${NCURSES:-$HOME/.local/state/unity-mcp/runtime/ncurses-64/lib}

mkdir -p "$PROBE_DIR/Assets/Editor" "$PROBE_DIR/Assets/Garments"
if [ ! -d "$PROBE_DIR/Packages" ]; then
    "$HUB_FHS" "$EDITOR" -batchmode -nographics -quit -createProject "$PROBE_DIR" -logFile /tmp/unity-create.log
    python3 - "$PROBE_DIR/Packages/manifest.json" <<'PY'
import json, sys
path = sys.argv[1]
doc = json.load(open(path))
doc['dependencies']['com.unity.cloud.gltfast'] = '6.14.0'
json.dump(doc, open(path, 'w'), indent=2)
PY
fi

cp "$(dirname "$0")/ImportProbe.cs" "$(dirname "$0")/MiniJson.cs" "$PROBE_DIR/Assets/Editor/"

# If explicit files passed as arguments, copy them
if [ "$#" -gt 0 ]; then
    for f in "$@"; do
        cp "$f" "$PROBE_DIR/Assets/Garments/$(basename "$f")"
    done
fi

# If legacy env vars are set, copy them
if [ -n "${ACTOR_GLB:-}" ]; then
    cp "$ACTOR_GLB" "$PROBE_DIR/Assets/Garments/character.glb"
fi
if [ -n "${CLOTH_GLB:-}" ]; then
    cp "$CLOTH_GLB" "$PROBE_DIR/Assets/Garments/C1-cloth-idle.glb"
fi

# If no garments exist yet, generate sculpto-gltf fixtures
if [ -z "$(ls -A "$PROBE_DIR/Assets/Garments")" ]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
    cargo run -p sculpto-gltf --example export_test_glb -- --tier1 "$PROBE_DIR/Assets/Garments/tier1.glb"
    cargo run -p sculpto-gltf --example export_test_glb -- --synthetic "$PROBE_DIR/Assets/Garments/synthetic.glb"
    cargo run -p sculpto-gltf --example export_test_glb -- --morph "$PROBE_DIR/Assets/Garments/morph.glb"
fi

"$HUB_FHS" env LD_LIBRARY_PATH="$NCURSES" "$EDITOR" -batchmode -nographics -quit \
    -projectPath "$PROBE_DIR" -executeMethod ImportProbe.Run \
    -logFile "$PROBE_DIR/Editor.log" >/dev/null 2>&1 || true

grep -o 'IMPORT_PROBE .*' "$PROBE_DIR/Editor.log" | tail -1
cp "$PROBE_DIR/Editor.log" "$PROBE_DIR/script-log.txt" 2>/dev/null || true
