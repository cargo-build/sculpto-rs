#!/usr/bin/env python3
"""Pull a garment's collar toward the neck axis.

A catalogue tunic is authored with a wide boat neckline. Scaling the top band
toward the vertical neck axis turns it into the high round collar reference art
shows, while the shoulders and hem keep their shape.

    pinch_collar.py mesh=<in.obj> out=<out.obj> y=<collar height> [depth=<band>]
                    [factor=<0..1>] [cx=0] [cz=0]
"""

from __future__ import annotations

import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("mesh", "out", "y"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
Y = float(cfg["y"])
DEPTH = float(cfg.get("depth", "8"))
FACTOR = float(cfg.get("factor", "0.55"))
CX = float(cfg.get("cx", "0"))
CZ = float(cfg.get("cz", "0"))

moved = 0
with open(cfg["mesh"], encoding="utf-8") as source, open(cfg["out"], "w", encoding="utf-8") as sink:
    for line in source:
        if not line.startswith("v "):
            sink.write(line)
            continue
        _, x, y, z = line.split()[:4]
        px, py, pz = float(x), float(y), float(z)
        if py > Y:
            ramp = min((py - Y) / DEPTH, 1.0)
            scale = 1.0 - FACTOR * ramp
            px = CX + (px - CX) * scale
            pz = CZ + (pz - CZ) * scale
            moved += 1
        sink.write(f"v {px:.6f} {py:.6f} {pz:.6f}\n")
print(f"collar pinch: moved {moved} vertices above y={Y} (factor {FACTOR}, depth {DEPTH})")