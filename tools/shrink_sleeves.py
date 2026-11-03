#!/usr/bin/env python3
"""Collapse a garment's relief sleeves inward so a generated tube can replace them.

Cutting the flat sleeve band leaves ragged boundary shards (two box cuts were
tried and both produced spikes). Pulling those vertices toward the shoulder and
the arm axis instead hides the sheet inside the sleeve tube: no boundary, no
shards.

    shrink_sleeves.py mesh=<in.obj> out=<out.obj> shoulder_x=<units> [pull=0.12]
"""

from __future__ import annotations

import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("mesh", "out", "shoulder_x"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
LIMIT = float(cfg["shoulder_x"])
PULL = float(cfg.get("pull", "0.12"))

moved = 0
with open(cfg["mesh"], encoding="utf-8") as source, open(cfg["out"], "w", encoding="utf-8") as sink:
    for line in source:
        if not line.startswith("v "):
            sink.write(line)
            continue
        _, x, y, z = line.split()[:4]
        px, py, pz = float(x), float(y), float(z)
        if abs(px) > LIMIT:
            # keep a thin sliver at the shoulder and fold the sleeve onto the arm
            px = (LIMIT + (abs(px) - LIMIT) * PULL) * (1 if px > 0 else -1)
            pz = pz * (1.0 - 0.6 * min((abs(float(x)) - LIMIT) / 20.0, 1.0))
            moved += 1
        sink.write(f"v {px:.6f} {py:.6f} {pz:.6f}\n")
print(f"shrunk {moved} sleeve vertices beyond |x| > {LIMIT} (pull {PULL})")