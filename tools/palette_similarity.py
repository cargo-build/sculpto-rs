"""Reproducible palette-similarity proxy between a concept sheet and a render.

No agent on this machine can see images, but palette agreement is measurable:
drop background-like pixels (transparent alpha on our renders, the parchment
beige on concept sheets), reduce both images to their dominant colours, and
report per-colour coverage differences. This is a proxy, not a visual audit;
it exists so the claim "renders carry the concept's colours" can be checked
by anyone with ImageMagick.

Usage:
    python3 tools/palette_similarity.py <concept.png> <render.png> [k]
"""

import json
import subprocess
import sys


def dominant(path: str, k: int, background: bool, crop_front: bool = False) -> list[tuple[tuple[int, int, int], float]]:
    args = ["magick", path]
    if crop_front:
        # concept sheets hold front/side/back/prop columns; keep the front view
        args += ["-gravity", "west", "-crop", "34%x100%+0+0", "+repage"]
    args += [
        "-alpha", "on", "-background", "none", "-alpha", "remove",
    ]
    if background:
        # concept sheets sit on a warm parchment; treat near-beige as background
        args += ["-fuzz", "12%", "-transparent", "rgb(214,203,178)"]
    else:
        # studio renders carry an opaque backdrop: strip its corner colour
        corner = subprocess.run(
            ["magick", path, "-format", "%[pixel:p{2,2}]", "info:"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        rgb = corner.split("(")[1].split(")")[0].split(",")[:3]
        args += ["-fuzz", "8%", "-transparent", f"rgb({rgb[0]},{rgb[1]},{rgb[2]})"]
    args += ["-alpha", "on", "-background", "none", "-alpha", "remove"]
    args += ["-resize", "120x", "-colors", str(k), "-format", "%c", "histogram:info:-"]
    out = subprocess.run(args, capture_output=True, text=True, check=True).stdout
    entries = []
    for line in out.splitlines():
        line = line.strip()
        if not line or ":" not in line:
            continue
        count, rest = line.split(":", 1)
        colour = rest.strip().split("#")[1].split()[0] if "#" in rest else None
        if colour is None:
            continue
        alpha = 1.0
        if "(" in rest and "," in rest:
            fields = rest.split("(")[1].split(")")[0].split(",")
            if len(fields) >= 4:
                try:
                    alpha = float(fields[3])
                except ValueError:
                    alpha = 1.0
        if alpha < 0.1:
            continue
        entries.append((colour, int(count)))
    total = sum(count for _, count in entries) or 1
    palette = []
    for colour, count in entries:
        rgb = (int(colour[0:2], 16), int(colour[2:4], 16), int(colour[4:6], 16))
        # skip what ImageMagick used to represent transparency
        if rgb == (0, 0, 0):
            continue
        palette.append((rgb, count / total))
    palette.sort(key=lambda item: -item[1])
    return palette[:k]


def main() -> None:
    concept_path, render_path = sys.argv[1], sys.argv[2]
    k = int(sys.argv[3]) if len(sys.argv) > 3 else 6
    crop_front = len(sys.argv) > 4 and sys.argv[4] == "front"
    concept = dominant(concept_path, k, background=True, crop_front=crop_front)
    render = dominant(render_path, k, background=False)

    def nearest(colour):
        best = min(render, key=lambda item: sum((a - b) ** 2 for a, b in zip(item[0], colour)))
        return best

    worst = 0.0
    print("concept -> nearest render colour (fraction gap)")
    for colour, fraction in concept:
        match, render_fraction = nearest(colour)
        gap = abs(fraction - render_fraction)
        worst = max(worst, gap)
        print(
            f"  #{colour[0]:02x}{colour[1]:02x}{colour[2]:02x} {fraction:5.1%}"
            f" -> #{match[0]:02x}{match[1]:02x}{match[2]:02x} {render_fraction:5.1%}"
            f"  |d|={gap:5.1%}"
        )
    print(f"render palette: {json.dumps([[f'#{c[0]:02x}{c[1]:02x}{c[2]:02x}', round(f, 3)] for c, f in render])}")
    print(f"largest single-colour fraction gap: {worst:.1%}")


main()