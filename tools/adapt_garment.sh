#!/usr/bin/env bash
# One-command garment adaptation: align -> fit -> bake -> GLB.
#
#   tools/adapt_garment.sh \
#       --garment <garment.obj> --blend <actor character.blend> \
#       --class robe --clip Combat_Idle --out <dir> [--settle 24] \
#       [--frame-end 140] [--sample 4] [--sculpto <binary>] [--blender blender] \
#       [--actor-glb <character.glb> --material-blend <garment.blend> \
#        --material-object <material name>]
#
# This internal fitting stage requires a prepared single-surface OBJ with UVs
# in the display frame. Raw intake/preparation belongs to the outfit workflow
# (DECL0013); an unresolved intake verdict stops before fit or simulation.
# Steps,
# each logged with its wall time:
#   1. export the actor's rest body (Blender)                -> <out>/actor-body.obj
#   2. place the garment in the actor frame (class aware)    -> <out>/garment-actor.obj
#   3. corset-aura fit + body/self check (sculpto)           -> <out>/garment-fitted.obj
#   4. re-attach the source UVs to the fitted mesh           -> <out>/garment-uv.obj
#   5. XPBD bake over the clip (Blender)                     -> <out>/bake/
#   6. GLB export with morph animation (Blender)             -> <out>/<clip>.glb
#
# Step 4 runs only when the placed garment carries `vt` lines. Step 6 becomes
# the combined actor + cloth asset (`<clip>-combined.glb`) when the material
# flags are given: that needs the accepted actor GLB, so `--actor-glb` is
# required with them, and the garment material rides on every cloth shell.
# Prints a JSON summary with per-step wall time, fit numbers and the bake's
# qualification block.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
GARMENT=""; BLEND=""; CLASS="robe"; CLIP="Combat_Idle"; OUT=""
ACTOR_GLB=""; MATERIAL_BLEND=""; MATERIAL_OBJECT=""
SETTLE=24; FRAME_END=140; SAMPLE=4
SCULPTO=${SCULPTO:-$ROOT/target/debug/sculpto}
BLENDER=${BLENDER:-blender}

while [ $# -gt 0 ]; do
    case "$1" in
        --garment) GARMENT=$2; shift 2 ;;
        --blend) BLEND=$2; shift 2 ;;
        --class) CLASS=$2; shift 2 ;;
        --clip) CLIP=$2; shift 2 ;;
        --out) OUT=$2; shift 2 ;;
        --actor-glb) ACTOR_GLB=$2; shift 2 ;;
        --material-blend) MATERIAL_BLEND=$2; shift 2 ;;
        --material-object) MATERIAL_OBJECT=$2; shift 2 ;;
        --settle) SETTLE=$2; shift 2 ;;
        --frame-end) FRAME_END=$2; shift 2 ;;
        --sample) SAMPLE=$2; shift 2 ;;
        --sculpto) SCULPTO=$2; shift 2 ;;
        --blender) BLENDER=$2; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

for required in GARMENT BLEND OUT; do
    if [ -z "${!required}" ]; then
        echo "missing --$(echo "$required" | tr 'A-Z_' 'a-z-')" >&2
        exit 2
    fi
done
if [ -n "$MATERIAL_BLEND" ] || [ -n "$MATERIAL_OBJECT" ]; then
    for required in MATERIAL_BLEND MATERIAL_OBJECT ACTOR_GLB; do
        if [ -z "${!required}" ]; then
            echo "material passthrough needs --actor-glb, --material-blend and --material-object" >&2
            exit 2
        fi
    done
fi
mkdir -p "$OUT"

stamp() { date +%s.%N; }
elapsed() { python3 -c "print(round($2 - $1, 2))"; }

t0=$(stamp)
"$BLENDER" --background "$BLEND" --python "$ROOT/tools/export_body_obj.py" -- \
    mesh=BaseShape out="$OUT/actor-body.obj" >"$OUT/export-body.log" 2>&1
t1=$(stamp)

# `--class auto` defers classification to the aura; a placement rule still has
# to be chosen, and an unclassified garment uses the full-height rule.
ALIGN_CLASS=$CLASS
if [ "$ALIGN_CLASS" = auto ]; then ALIGN_CLASS=unknown; fi
python3 "$ROOT/tools/align_garment.py" --garment "$GARMENT" --body "$OUT/actor-body.obj" \
    --class "$ALIGN_CLASS" --out "$OUT/garment-actor.obj" | tee "$OUT/align.log"
t2=$(stamp)

"$SCULPTO" garment intake --body "$OUT/actor-body.obj" \
    --garment "$OUT/garment-actor.obj" --require-fit 1 >"$OUT/intake.log" 2>&1 || {
    cat "$OUT/intake.log" >&2
    exit 1
}

"$SCULPTO" garment aura --body "$OUT/actor-body.obj" \
    --garment "$OUT/garment-actor.obj" --out "$OUT/garment-fitted.obj" --class "$CLASS" >"$OUT/fit.log" 2>&1 || {
    cat "$OUT/fit.log" >&2
    exit 1
}
if ! check_log=$("$SCULPTO" garment check --body "$OUT/actor-body.obj" \
    --garment "$OUT/garment-fitted.obj" --self 1 2>&1); then
    echo "$check_log" >>"$OUT/fit.log"
    echo "$check_log" >&2
    exit 1
fi
echo "$check_log" >>"$OUT/fit.log"
t3=$(stamp)

# The sculpto CLI writes positions and triangles only, so a textured garment
# loses its UVs at the fit; merge_uv rebuilds them from the placed copy.
BAKE_GARMENT="$OUT/garment-fitted.obj"
if rg -q '^vt ' "$OUT/garment-actor.obj"; then
    python3 "$ROOT/tools/merge_uv.py" --fitted "$OUT/garment-fitted.obj" \
        --source "$OUT/garment-actor.obj" --out "$OUT/garment-uv.obj" | tee "$OUT/uv.log"
    BAKE_GARMENT="$OUT/garment-uv.obj"
fi
t4=$(stamp)

"$BLENDER" --background "$BLEND" --python "$ROOT/tools/cloth_bake.py" -- \
    garment="$BAKE_GARMENT" out="$OUT/bake" clip="$CLIP" \
    settle="$SETTLE" frame_end="$FRAME_END" sample="$SAMPLE" >"$OUT/bake.log" 2>&1
t5=$(stamp)

if [ -n "$MATERIAL_BLEND" ]; then
    GLB="$OUT/$CLIP-combined.glb"
    "$BLENDER" --background --factory-startup --python "$ROOT/tools/export_combined_glb.py" -- \
        actor="$ACTOR_GLB" bake="$OUT/bake" out="$GLB" \
        material_blend="$MATERIAL_BLEND" material_object="$MATERIAL_OBJECT" >"$OUT/glb.log" 2>&1
else
    GLB="$OUT/$CLIP.glb"
    "$BLENDER" --background --factory-startup --python "$ROOT/tools/export_cloth_glb.py" -- \
        bake="$OUT/bake" out="$GLB" >"$OUT/glb.log" 2>&1
fi
t6=$(stamp)

python3 - "$OUT" "$CLIP" "$GARMENT" "$BLEND" "$CLASS" "$GLB" "$BAKE_GARMENT" \
    "$(elapsed "$t0" "$t1")" "$(elapsed "$t1" "$t2")" "$(elapsed "$t2" "$t3")" \
    "$(elapsed "$t3" "$t4")" "$(elapsed "$t4" "$t5")" "$(elapsed "$t5" "$t6")" \
    "$check_log" <<'PY'
import json, os, sys
(out, clip, garment, blend, garment_class, glb, bake_garment,
 t_body, t_align, t_fit, t_uv, t_bake, t_glb, check) = sys.argv[1:]
summary = {
    "garment": garment,
    "blend": blend,
    "class": garment_class,
    "clip": clip,
    "check": check,
    "seconds": {"export_body": float(t_body), "align": float(t_align),
                "fit": float(t_fit), "uv": float(t_uv),
                "bake": float(t_bake), "glb": float(t_glb)},
    "glb": glb,
    "glb_bytes": os.path.getsize(glb),
    "bake_garment": os.path.basename(bake_garment),
}
bake_report = os.path.join(out, "bake", "report.json")
if os.path.exists(bake_report):
    report = json.load(open(bake_report))
    summary["bake"] = {"qualify": report.get("qualify"), "shells": report.get("shells"),
                       "pin": report.get("pin"), "sim_seconds": report.get("sim_seconds")}
json.dump(summary, open(os.path.join(out, "adapt.json"), "w"), indent=1)
print("ADAPT " + json.dumps(summary))
PY
