#!/usr/bin/env python3
"""Place a catalogue garment piece rigidly onto a body.

Wardrobe GLBs are view catalogues: each garment sits beside the body, not on
it. The radial press fit collapses loose pieces (a tunic becomes a chest band),
so this tool keeps the piece's authored shape and only scales/translates it:
the vertical span maps onto a body landmark band and the horizontal centre is
aligned. Vertex order is untouched, so the renderer's keep mask still matches.

    align_piece.py mesh=<reposed.obj> piece=<kept.obj> body=<body.obj>
                   out=<aligned.obj> [top=<frac>] [bottom=<frac>]
"""

from __future__ import annotations

import sys

argv = sys.argv[1:]
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("mesh", "piece", "body", "out"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")

# Body-frame fractions of height, measured down from the top of the head.
TOP = float(cfg.get("top", "0.14"))
BOTTOM = float(cfg.get("bottom", "0.62"))
# width: scale by arm span (T-pose garments), height: scale by the landmark band
MODE = cfg.get("mode", "height")


def vertices(path: str) -> list[tuple[float, float, float]]:
    out = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("v "):
                _, x, y, z = line.split()[:4]
                out.append((float(x), float(y), float(z)))
    return out


def bbox(points):
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    zs = [p[2] for p in points]
    return (min(xs), max(xs)), (min(ys), max(ys)), (min(zs), max(zs))


mesh = vertices(cfg["mesh"])
piece_points = vertices(cfg["piece"])
body_points = vertices(cfg["body"])

# Only vertices the piece actually uses take part in the placement bbox.
used = set()
with open(cfg["piece"], encoding="utf-8") as handle:
    for line in handle:
        if line.startswith("f "):
            for token in line.split()[1:]:
                used.add(int(token.split("/")[0]) - 1)
if not used:
    raise SystemExit("piece has no faces")
piece_pts = [mesh[i] for i in used]

(px0, px1), (py0, py1), (pz0, pz1) = bbox(piece_pts)
(bx0, bx1), (by0, by1), (bz0, bz1) = bbox(body_points)
body_h = by1 - by0
target_top = by1 - TOP * body_h
target_bottom = by1 - BOTTOM * body_h
if MODE == "width":
    # T-pose garment on a T-pose body: the arm span is the stable anchor, the
    # vertical extent follows the garment's own proportions. `target_width`
    # pins the span explicitly because the body bbox reflects its A-pose.
    span = float(cfg.get("target_width", "")) if cfg.get("target_width") else (bx1 - bx0)
    scale = span / (px1 - px0)
else:
    scale = (target_top - target_bottom) / (py1 - py0)
centre_x = (bx0 + bx1) / 2 - (px0 + px1) / 2 * scale
centre_z = (bz0 + bz1) / 2 - (pz0 + pz1) / 2 * scale
# width mode anchors the garment's top at the shoulder landmark; height mode
# stretches the band, so its bottom defines the offset.
shift_y = (target_top - py1 * scale) if MODE == "width" else (target_bottom - py0 * scale)

print(
    f"piece y[{py0:.2f},{py1:.2f}] -> body band [{target_bottom:.2f},{target_top:.2f}] "
    f"scale {scale:.4f}, body height {body_h:.2f}"
)

with open(cfg["mesh"], encoding="utf-8") as source, open(cfg["out"], "w", encoding="utf-8") as sink:
    for line in source:
        if not line.startswith("v "):
            sink.write(line)
            continue
        _, x, y, z = line.split()[:4]
        nx = float(x) * scale + centre_x
        ny = float(y) * scale + shift_y
        nz = float(z) * scale + centre_z
        sink.write(f"v {nx:.6f} {ny:.6f} {nz:.6f}\n")

print(f"wrote {cfg['out']}")