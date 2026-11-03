"""Island-aware garment prep for Tripo whole-figure meshes.

Tripo exports are fragmented: torso, arm segments, legs and hanging panels are
separate connected islands, sometimes with bent elbows and debris. A plain
radial fit mangles the horizontal arm islands into strips, so this tool:

1. splits the mesh into connected islands (cached next to the raw file);
2. drops debris faces (<100 verts) but keeps their vertices (order preserved
   for vertex-swap rendering);
3. rigidly maps each arm island onto the body's arm (shoulder->elbow->hand
   landmarks, rotation + uniform scale in the vertical plane);
4. rescales everything from the raw frame (y-up, 0..~0.65) into the body frame
   (y-up, feet at the body's y-min);
5. writes a fit part (torso/limbs/arm islands), a pass part (hanging panels,
   excluded from the radial fit) and a reposed whole mesh (no fit) used as the
   pose-normalized concept render.

Usage:
    python3 tools/shell_prep.py raw=<obj> body=<obj> stem=<prefix>

Writes <stem>-fitpart.obj, <stem>-passpart.obj, <stem>-reposed.obj.
"""

import json
import math
import os
import sys
from collections import defaultdict

argv = sys.argv[1:]
cfg = dict(a.split("=", 1) for a in argv if "=" in a)
for key in ("raw", "body", "stem"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")

ARM_FAR = 0.30  # island reaches |x| beyond this (raw units) -> candidate arm
ARM_LOW = 0.25  # island must start above this normalized height
DEBRIS = 100
# Optional `column=<centre>,<half>` (raw x): keep only islands whose mean |x|
# sits near the centre. Wardrobe-set GLBs are multi-view catalogues; the centre
# column is a full outfit stacked by body height.
column = None
if "column" in cfg:
    centre, half = (float(v) for v in cfg["column"].split(","))
    column = (centre, half)
# Optional `piece=<vertex count>[,<count>]`: keep only the named islands
# (cut-out workflow; several counts = one garment made of separate islands).
piece = [int(v) for v in cfg["piece"].split(",")] if "piece" in cfg else None


def read_obj(path):
    verts, faces = [], []
    with open(path) as fh:
        for line in fh:
            if line.startswith("v "):
                t = line.split()
                verts.append((float(t[1]), float(t[2]), float(t[3])))
            elif line.startswith("f "):
                faces.append([int(tok.split("/")[0]) - 1 for tok in line.split()[1:]])
    return verts, faces


def island_labels(verts, faces, cache_path):
    """Vertex -> island id, via inline union-find with a json cache."""
    if os.path.exists(cache_path):
        with open(cache_path) as fh:
            return json.load(fh)
    n = len(verts)
    parent = list(range(n))
    for f in faces:
        a = f[0]
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        for i in f[1:]:
            b = i
            while parent[b] != b:
                parent[b] = parent[parent[b]]
                b = parent[b]
            if b != a:
                parent[b] = a
    labels = list(range(n))
    for i in range(n):
        r = i
        while parent[r] != r:
            r = parent[r]
        j = i
        while labels[j] != r:
            labels[j], j = r, parent[j]
    with open(cache_path, "w") as fh:
        json.dump(labels, fh)
    return labels


def body_landmarks(verts):
    """Shoulder/elbow/hand landmarks per side; arm threshold from the |x| gap."""
    ys = [v[1] for v in verts]
    lo, hi = min(ys), max(ys)
    height = hi - lo
    window = [v for v in verts if lo + 0.35 * height < v[1] < lo + 0.97 * height]
    xs = sorted(abs(v[0]) for v in window)
    bulk = xs[int(len(xs) * 0.70)]
    far = xs[-1]
    thr = 0.5 * (bulk + far)
    marks = {}
    for side in (1, -1):
        cand = [v for v in window if side * v[0] > thr]
        s = max(cand, key=lambda v: v[1])
        w = max(cand, key=lambda v: side * v[0])
        m = tuple(s[i] + 0.45 * (w[i] - s[i]) for i in range(3))
        angle = math.degrees(math.atan2(side * (w[0] - s[0]), s[1] - w[1]))
        marks[side] = {"S": s, "M": m, "W": w}
        print(
            f"  body {'R' if side > 0 else 'L'}: thr={thr:.1f} shoulder=({s[0]:.1f},{s[1]:.1f}) "
            f"wrist=({w[0]:.1f},{w[1]:.1f}) angle={angle:.1f}deg len={math.dist(s, w):.1f} n={len(cand)}"
        )
    return marks


def end_points(verts, idxs, side):
    """Proximal (shoulder) / distal (hand) end centroids of an arm island."""
    lo = min(verts[i][0] for i in idxs)
    hi = max(verts[i][0] for i in idxs)
    span = max(hi - lo, 1e-9)

    def rel(i):
        r = (verts[i][0] - lo) / span
        return 1.0 - r if side < 0 else r

    prox = [i for i in idxs if rel(i) < 0.10]
    distal = [i for i in idxs if rel(i) > 0.90]
    p = [sum(verts[i][k] for i in prox) / len(prox) for k in range(3)]
    d = [sum(verts[i][k] for i in distal) / len(distal) for k in range(3)]
    return p, d


def main():
    import time

    t0 = time.time()
    raw_verts, raw_faces = read_obj(cfg["raw"])
    body_verts, _ = read_obj(cfg["body"])
    print(f"{cfg['raw']}: {len(raw_verts)} verts, {len(raw_faces)} faces [{time.time()-t0:.0f}s read]")
    marks = body_landmarks(body_verts)

    bys = min(v[1] for v in body_verts)
    body_h = max(v[1] for v in body_verts) - bys
    raw_ys = [v[1] for v in raw_verts]
    rlo, rhi = min(raw_ys), max(raw_ys)
    raw_h = rhi - rlo
    scale = body_h / raw_h
    print(f"  global scale {scale:.2f} (raw {raw_h:.2f} -> body {body_h:.1f}, feet {bys:.1f})")

    def gxf(v):
        return (v[0] * scale, (v[1] - rlo) * scale + bys, v[2] * scale)

    cache = cfg["raw"] + ".islands.json"
    t1 = time.time()
    labels = island_labels(raw_verts, raw_faces, cache)
    print(f"  islands labeled [{time.time()-t1:.0f}s]")
    groups = defaultdict(list)
    for i, lab in enumerate(labels):
        groups[lab].append(i)

    classified = []
    for idxs in groups.values():
        xs = [raw_verts[i][0] for i in idxs]
        ys = [raw_verts[i][1] for i in idxs]
        reach = max(abs(x) for x in xs)
        y0 = (min(ys) - rlo) / raw_h
        if len(idxs) < DEBRIS:
            kind = "debris"
        elif reach > ARM_FAR and y0 > ARM_LOW:
            kind = "arm"
        elif reach > ARM_FAR:
            kind = "hang"
        else:
            kind = "core"
        classified.append((kind, idxs))
        print(
            f"  island n={len(idxs):7d} x[{min(xs):+.2f},{max(xs):+.2f}] "
            f"y[{min(ys):+.2f},{max(ys):+.2f}] -> {kind}"
        )

    if column is not None:
        centre, half = column
        kept = []
        dropped = 0
        for kind, idxs in classified:
            mean_x = sum(raw_verts[i][0] for i in idxs) / len(idxs)
            if kind != "debris" and abs(mean_x - centre) > half:
                dropped += 1
                kept.append(("drop", idxs))
            else:
                kept.append((kind, idxs))
        classified = kept
        print(f"  column {centre:+.2f}+-{half:.2f}: {dropped} islands dropped")

    if piece is not None:
        kept = []
        found = set()
        for kind, idxs in classified:
            if len(idxs) in piece:
                kept.append((kind, idxs))
                found.add(len(idxs))
                print(f"  piece {len(idxs)}: kept island y[{min(raw_verts[i][1] for i in idxs):.2f},{max(raw_verts[i][1] for i in idxs):.2f}]")
            else:
                kept.append(("drop", idxs))
        missing = set(piece) - found
        if missing:
            raise SystemExit(f"no islands with {sorted(missing)} vertices")
        classified = kept

    out = [None] * len(raw_verts)
    for kind, idxs in classified:
        if kind == "debris":
            for i in idxs:
                out[i] = gxf(raw_verts[i])

    for side in (1, -1):
        arms = [c for c in classified if c[0] == "arm" and sum(raw_verts[i][0] for i in c[1]) / len(c[1]) * side > 0]
        arms.sort(key=lambda c: -max(raw_verts[i][1] for i in c[1]))
        m = marks[side]
        chain = [(m["S"], m["M"]), (m["M"], m["W"])]
        for j, (_, idxs) in enumerate(arms):
            prox_t, dist_t = chain[j] if j < len(chain) else chain[-1]
            p, d = end_points(raw_verts, idxs, side)
            pt, dt = gxf(p), gxf(d)
            vx, vy = dt[0] - pt[0], dt[1] - pt[1]
            tx, ty = dist_t[0] - prox_t[0], dist_t[1] - prox_t[1]
            # rotation + translation only: the garment keeps its modeled sleeve
            # radius; landmark segments set direction and anchor, not scale.
            s = 1.0
            ang = math.atan2(ty, tx) - math.atan2(vy, vx)
            ca, sa = math.cos(ang), math.sin(ang)
            # Bend, not rigid swing: islands may merge arm and torso (e.g. the
            # mage robe), so weight the rotation by distance beyond the
            # shoulder anchor; torso-side vertices stay in place.
            anchor = prox_t
            band = max(0.30 * math.hypot(tx, ty), 1e-9)
            for i in idxs:
                v = gxf(raw_verts[i])
                rx, ry = v[0] - anchor[0], v[1] - anchor[1]
                w = min(max((abs(v[0]) - abs(anchor[0])) / band, 0.0), 1.0)
                a = ang * w
                c, sn = math.cos(a), math.sin(a)
                out[i] = (
                    anchor[0] + (c * rx - sn * ry) * s,
                    anchor[1] + (sn * rx + c * ry) * s,
                    anchor[2] + (v[2] - anchor[2]) * s,
                )
            print(
                f"  mapped {'R' if side > 0 else 'L'} arm piece {j}: {len(idxs)} verts, "
                f"len {math.hypot(vx, vy):.1f}->{math.hypot(tx, ty):.1f}, "
                f"scale {s:.2f}, rot {math.degrees(ang):.1f}deg"
            )

    for kind, idxs in classified:
        if kind in ("arm", "debris"):
            continue
        for i in idxs:
            out[i] = gxf(raw_verts[i])

    kind_of = {}
    for kind, idxs in classified:
        for i in idxs:
            kind_of[i] = kind
    fit_faces = [f for f in raw_faces if kind_of[f[0]] in ("core", "arm")]
    # hang + debris: excluded from the radial fit; debris faces stay so every
    # vertex remains face-referenced (Blender's OBJ import drops free verts).
    pass_faces = [f for f in raw_faces if kind_of[f[0]] in ("hang", "debris")]
    dropped = len(raw_faces) - len(fit_faces) - len(pass_faces)

    def write(path, faces):
        with open(path, "w") as fh:
            fh.write(f"# sculpto shell_prep from {cfg['raw']}\n")
            for v in out:
                fh.write(f"v {v[0]:.6f} {v[1]:.6f} {v[2]:.6f}\n")
            for f in faces:
                fh.write("f " + " ".join(str(i + 1) for i in f) + "\n")

    stem = cfg["stem"]
    write(f"{stem}-fitpart.obj", fit_faces)
    write(f"{stem}-passpart.obj", pass_faces)
    write(f"{stem}-reposed.obj", fit_faces + pass_faces)
    print(
        f"  wrote {stem}-fitpart/passpart/reposed.obj: "
        f"fit {len(fit_faces)} faces, pass {len(pass_faces)} faces, dropped {dropped}"
    )


main()
