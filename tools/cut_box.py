#!/usr/bin/env python3
"""Drop faces whose centroid falls inside an axis-aligned box.

Catalogue pieces carry small parts a deliverable must not show (a dangling
strap on the trousers). Cutting by a local box keeps the garment intact while
removing the accessory, and the vertex list stays unchanged for the render swap.

    cut_box.py mesh=<in.obj> out=<out.obj> box=<x0,x1,y0,y1,z0,z1>
"""

from __future__ import annotations

import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("mesh", "out", "box"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
x0, x1, y0, y1, z0, z1 = (float(v) for v in cfg["box"].split(","))

coords = []
lines = []
for line in open(cfg["mesh"], encoding="utf-8"):
    lines.append(line)
    if line.startswith("v "):
        _, x, y, z = line.split()[:4]
        coords.append((float(x), float(y), float(z)))

kept = dropped = 0
with open(cfg["out"], "w", encoding="utf-8") as sink:
    for line in lines:
        if line.startswith("f "):
            index = [int(t.split("/")[0]) - 1 for t in line.split()[1:]]
            cx = sum(coords[i][0] for i in index) / len(index)
            cy = sum(coords[i][1] for i in index) / len(index)
            cz = sum(coords[i][2] for i in index) / len(index)
            if x0 <= cx <= x1 and y0 <= cy <= y1 and z0 <= cz <= z1:
                dropped += 1
                continue
            kept += 1
        sink.write(line)
print(f"cut box: kept {kept} faces, dropped {dropped}")