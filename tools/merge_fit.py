#!/usr/bin/env python3
"""Merge a radial fit with the pre-fit arms.

`garment aura` presses every vertex onto a radial envelope around the vertical
body axis, which collapses T-pose sleeves into stubs. This keeps the fitted
torso and restores the authored positions outside |x| > limit, so the sleeves
stay along the arms.

    merge_fit.py before=<pre.obj> after=<fitted.obj> out=<merged.obj> [limit=30]
"""

from __future__ import annotations

import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("before", "after", "out"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
LIMIT = float(cfg.get("limit", "30"))


def is_vertex(line: str) -> bool:
    return line.startswith("v ")


def coords(line: str) -> tuple[float, float, float]:
    _, x, y, z = line.split()[:4]
    return float(x), float(y), float(z)


with open(cfg["before"], encoding="utf-8") as handle:
    before = handle.readlines()
with open(cfg["after"], encoding="utf-8") as handle:
    after = handle.readlines()

before_vertices = [line for line in before if is_vertex(line)]
after_vertices = [line for line in after if is_vertex(line)]
assert len(before_vertices) == len(after_vertices), "vertex counts differ"
chosen = []
kept = restored = blended = 0
BLEND = float(cfg.get("blend", "6"))
for old, new in zip(before_vertices, after_vertices):
    ox, oy, oz = coords(old)
    nx, ny, nz = coords(new)
    # gradual hand-over from the fitted torso to the authored sleeve: a hard
    # cut at |x| = LIMIT leaves a visible seam ring at the shoulder
    weight = min(max((abs(ox) - LIMIT) / BLEND, 0.0), 1.0)
    if weight >= 1.0:
        chosen.append(old)
        restored += 1
    elif weight <= 0.0:
        chosen.append(new)
        kept += 1
    else:
        mx = nx + (ox - nx) * weight
        my = ny + (oy - ny) * weight
        mz = nz + (oz - nz) * weight
        chosen.append(f"v {mx:.6f} {my:.6f} {mz:.6f}\n")
        blended += 1
out = []
index = 0
for line in after:
    if is_vertex(line):
        out.append(chosen[index])
        index += 1
    else:
        out.append(line)
with open(cfg["out"], "w", encoding="utf-8") as sink:
    sink.writelines(out)
print(f"torso kept {kept}, arms restored {restored}, blended {blended} (|x| > {LIMIT}, blend {BLEND})")