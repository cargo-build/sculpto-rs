"""Silhouette IoU between two figure renders.

Extracts each render's alpha channel (film_transparent renders), trims to the
figure bbox, resizes to a fixed grid and reports intersection-over-union of
the figure pixels plus per-band width agreement. Pose-matched pairs (same pose
on both sides) measure fit fidelity; pose-mismatched pairs measure total
difference.

Usage:
    python3 tools/silhouette_iou.py <a.png> <b.png> [bands]
"""

import subprocess
import sys


def silhouette(path, w=200, h=300):
    """Figure mask from the render's alpha channel (film_transparent renders)."""
    trimmed = subprocess.run(
        ["magick", path, "-alpha", "extract", "-threshold", "50%", "-format", "%@", "info:"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    tw, th = (int(x) for x in trimmed.split("+")[0].split("x")[:2])
    raw = subprocess.run(
        ["magick", path, "-alpha", "extract", "-threshold", "50%",
         "-crop", f"{tw}x{th}+{trimmed.split('+', 1)[1]}", "+repage",
         "-resize", f"{w}x{h}!", "-depth", "8", "gray:-"],
        capture_output=True, check=True,
    ).stdout
    return raw, w, h


def main():
    a_path, b_path = sys.argv[1], sys.argv[2]
    bands = int(sys.argv[3]) if len(sys.argv) > 3 else 10
    a, w, h = silhouette(a_path)
    b, _, _ = silhouette(b_path)
    inter = union = 0
    aw = [0] * bands
    bw = [0] * bands
    for i in range(w * h):
        av, bv = a[i] > 127, b[i] > 127
        inter += av and bv
        union += av or bv
        band = min(i // w * bands // h, bands - 1)
        aw[band] += av
        bw[band] += bv
    iou = inter / union if union else 0.0
    profile = [min(aw[i], bw[i]) / max(aw[i], bw[i], 1) for i in range(bands)]
    worst = min(range(bands), key=lambda i: profile[i])
    print(f"iou {iou:.3f}  bands {bands} (top->{bands//2}->bottom)")
    print("  band agreement: " + " ".join(f"{p:.2f}" for p in profile))
    print(f"  worst band {worst} ({profile[worst]:.2f})")


main()
