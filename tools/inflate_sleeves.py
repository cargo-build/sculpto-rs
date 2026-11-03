#!/usr/bin/env python3
"""Inflate a garment's relief sleeves onto a cylinder around the arm axis.

Replacing the sleeve with a separate tube never matches the tunic's seams (audit
7). This keeps the garment's own sleeve surface and pushes its arm-band vertices
out onto a cylinder of radius (arm radius + clearance) around the measured
shoulder -> wrist axis, so the sheet wraps the arm and keeps every seam.

    inflate_sleeves.py mesh=<garment.obj> body=<posed_body.obj> out=<out.obj>
                       [clearance=2.5]
"""

from __future__ import annotations

import math
import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("mesh", "body", "out"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
CLEARANCE = float(cfg.get("clearance", "2.5"))


def load(path):
    out = []
    for line in open(path, encoding="utf-8"):
        if line.startswith("v "):
            _, x, y, z = line.split()[:4]
            out.append((float(x), float(y), float(z)))
    return out


body = load(cfg["body"])
xs = [p[0] for p in body]
zs = [p[2] for p in body]
centre_x = (min(xs) + max(xs)) / 2
span = max(xs) - min(xs)
top = min(zs) + 0.80 * (max(zs) - min(zs))

axes = {}
for side in (1, -1):
    arm = [p for p in body if p[0] * side > centre_x * side + 0.17 * span / 2]
    wrist = max(arm, key=lambda p: p[0] * side)
    band = [p for p in arm if abs(p[0] - wrist[0] * 0.45) < 4.0] or arm[:50]
    shoulder = (
        sum(p[0] for p in band) / len(band) * 0.5 + wrist[0] * 0.5,
        sum(p[1] for p in band) / len(band),
        sum(p[2] for p in band) / len(band),
    )
    axis = (wrist[0] - shoulder[0], wrist[1] - shoulder[1], wrist[2] - shoulder[2])
    length = math.sqrt(sum(c * c for c in axis))
    axes[side] = (shoulder, tuple(c / length for c in axis), length)

# arm surface radius per side, measured from the body in a narrow band
radii = {}
for side, (shoulder, direction, length) in axes.items():
    near = []
    for point in body:
        rel = sum((point[i] - shoulder[i]) * direction[i] for i in range(3))
        if rel < 0 or rel > length:
            continue
        radial = math.sqrt(
            sum((point[i] - shoulder[i] - direction[i] * rel) ** 2 for i in range(3))
        )
        if radial < 0.06 * span:
            near.append(radial)
    radii[side] = max(near) if near else 5.0
    print(f"arm {side}: surface radius {radii[side]:.2f} (+{CLEARANCE})")

moved = 0
with open(cfg["mesh"], encoding="utf-8") as source, open(cfg["out"], "w", encoding="utf-8") as sink:
    for line in source:
        if not line.startswith("v "):
            sink.write(line)
            continue
        _, x, y, z = line.split()[:4]
        px, py, pz = float(x), float(y), float(z)
        side = 1 if px > centre_x else -1
        shoulder, direction, length = axes[side]
        rel = sum((point - shoulder[i]) * direction[i] for i, point in enumerate((px, py, pz)))
        if 0.0 <= rel <= length:
            radial_vec = [ (px, py, pz)[i] - shoulder[i] - direction[i] * rel for i in range(3) ]
            radial = math.sqrt(sum(c * c for c in radial_vec))
            target = radii[side] + CLEARANCE
            if radial > 1e-4 and radial < target:
                scale = target / radial
                px = shoulder[0] + direction[0] * rel + radial_vec[0] * scale
                py = shoulder[1] + direction[1] * rel + radial_vec[1] * scale
                pz = shoulder[2] + direction[2] * rel + radial_vec[2] * scale
                moved += 1
        sink.write(f"v {px:.6f} {py:.6f} {pz:.6f}\n")
print(f"inflated {moved} sleeve vertices onto the arm cylinder")