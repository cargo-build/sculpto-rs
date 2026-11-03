#!/usr/bin/env bash
# Headless Unity glTF import probe for the sculpto cloth assets.
#
# Creates a throwaway project under /tmp, adds glTFast, copies the actor and
# cloth GLBs, runs ImportProbe.Run and prints the JSON report. Requires Unity
# 6000.6.4f1 from the Hub and the NixOS launcher facts recorded in README.md.
set -euo pipefail

PROBE_DIR=${PROBE_DIR:-/tmp/sculpto-classify/cloth/unity-probe}
ACTOR_GLB=${ACTOR_GLB:?set ACTOR_GLB to the actor GLB path}
CLOTH_GLB=${CLOTH_GLB:?set CLOTH_GLB to the cloth GLB path}
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
cp "$ACTOR_GLB" "$PROBE_DIR/Assets/Garments/character.glb"
cp "$CLOTH_GLB" "$PROBE_DIR/Assets/Garments/C1-cloth-idle.glb"

"$HUB_FHS" env LD_LIBRARY_PATH="$NCURSES" "$EDITOR" -batchmode -nographics -quit \
    -projectPath "$PROBE_DIR" -executeMethod ImportProbe.Run \
    -logFile "$PROBE_DIR/Editor.log" >/dev/null 2>&1 || true
grep -o 'IMPORT_PROBE .*' "$PROBE_DIR/Editor.log" | tail -1
cp "$PROBE_DIR/Editor.log" "$PROBE_DIR/script-log.txt" 2>/dev/null || true
