"""Place a garment OBJ into the actor's display frame, class aware.

Usage:
  python3 tools/align_garment.py --garment <garment.obj> --body <actor-body.obj> \
      --class <kind> --out <garment-actor.obj>

Both inputs and the output are sculpto display-frame OBJs (Y-up: `y` is
height, `z` is depth). Only `v` lines are rewritten; every other line —
`vt`, `f` with corner UVs, `usemtl`, groups — is copied verbatim so the
garment's UVs survive into the fit and are merged back after trimming.

Two placements, because one bbox rule cannot serve both shapes:

- `upper-body` (a tunic, shirt or bodice) has no foot-level extent to anchor
  on. It is scaled by the ratio of shoulder-band width to the garment's top
  band width, centred on the torso's mid-band centroid, and hung so the
  garment's top edge sits at the neck base.
- `trousers` anchor at the hip: scaled by the hip-band width to the garment's
  waistband width, centred on the hip centroid, waistband at 0.60 × body
  height. A height-ratio scale would stretch them into a full-body sheath
  (measured on the HC5 candidate: scale 7.49, y span 0.166–1.609 m).
- `footwear` anchors on the foot: sole length (z extent of the lowest body
  band) sets the scale against the boot's own sole extent, centred on the
  foot centroid, sole resting at the body's lowest point.
- everything else spans the body's height (`robe`, `dress`, `skirt`, `cape`,
  `composite`, `unknown`): uniform scale by the height ratio, feet aligned,
  centred on the body bbox.
"""

import argparse

KINDS = (
    "upper-body",
    "trousers",
    "skirt",
    "dress",
    "robe",
    "footwear",
    "cape",
    "accessory",
    "composite",
    "unknown",
)
HEIGHT_PLACED = ("skirt", "dress", "robe", "cape", "composite", "unknown")

# Placement constants, fractions of body height.
SHOULDER_BAND = 0.15
TORSO_BAND = (0.60, 0.85)
COLLAR = 0.88
HIP_BAND = (0.50, 0.62)
WAIST = 0.60
ANKLE = 0.06
FOOT_BAND = 0.10


def load_vertices(path):
    """Positions in file order plus the raw lines (indices match positions)."""
    lines = open(path, encoding="utf-8").read().splitlines(keepends=True)
    verts = []
    for line in lines:
        if line.startswith("v "):
            _, x, y, z = line.split()[:4]
            verts.append((float(x), float(y), float(z)))
    if not verts:
        raise SystemExit(f"{path}: no vertex lines")
    return verts, lines


def height_placement(body, garment):
    body_y = [v[1] for v in body]
    garment_y = [v[1] for v in garment]
    scale = (max(body_y) - min(body_y)) / (max(garment_y) - min(garment_y))
    body_cx = (min(v[0] for v in body) + max(v[0] for v in body)) / 2
    body_cz = (min(v[2] for v in body) + max(v[2] for v in body)) / 2
    garment_cx = (min(v[0] for v in garment) + max(v[0] for v in garment)) / 2
    garment_cz = (min(v[2] for v in garment) + max(v[2] for v in garment)) / 2
    base_y = min(body_y)
    garment_min_y = min(garment_y)

    def place(x, y, z):
        return (
            body_cx + (x - garment_cx) * scale,
            base_y + (y - garment_min_y) * scale,
            body_cz + (z - garment_cz) * scale,
        )

    return place, scale, base_y


def upper_body_placement(body, garment):
    """Shoulder width sets the scale; the collar lands at the neck base."""
    body_y = [v[1] for v in body]
    min_y, max_y = min(body_y), max(body_y)
    height = max_y - min_y
    shoulder = [v for v in body if v[1] >= max_y - SHOULDER_BAND * height]
    w_body = max(v[0] for v in shoulder) - min(v[0] for v in shoulder)
    torso = [v for v in body if min_y + TORSO_BAND[0] * height <= v[1] <= min_y + TORSO_BAND[1] * height]
    if not torso:
        raise SystemExit("body has no vertices in the torso band; cannot place an upper body")
    cx = sum(v[0] for v in torso) / len(torso)
    cz = sum(v[2] for v in torso) / len(torso)

    garment_y = [v[1] for v in garment]
    garment_max = max(garment_y)
    span = garment_max - min(garment_y)
    top = [v for v in garment if v[1] >= garment_max - SHOULDER_BAND * span]
    w_garment = max(v[0] for v in top) - min(v[0] for v in top)
    if w_garment <= 0.0:
        raise SystemExit("garment top band has no width; cannot scale an upper body")
    gcx = sum(v[0] for v in top) / len(top)
    gcz = sum(v[2] for v in top) / len(top)
    scale = w_body / w_garment
    collar_y = min_y + COLLAR * height

    def place(x, y, z):
        return (cx + (x - gcx) * scale, collar_y - (garment_max - y) * scale, cz + (z - gcz) * scale)

    return place, scale, collar_y


def trousers_placement(body, garment):
    """Leg length sets the scale; the waistband lands at the waist line.

    Width is left to the aura's radial profile: scaling by hip width instead
    stretched the HC5 candidate's hem 16 cm under the floor because its
    waist-to-length proportions differ from the body's.
    """
    body_y = [v[1] for v in body]
    min_y, max_y = min(body_y), max(body_y)
    height = max_y - min_y
    hip = [v for v in body if min_y + HIP_BAND[0] * height <= v[1] <= min_y + HIP_BAND[1] * height]
    if not hip:
        raise SystemExit("body has no vertices in the hip band; cannot place trousers")
    cx = sum(v[0] for v in hip) / len(hip)
    cz = sum(v[2] for v in hip) / len(hip)

    garment_y = [v[1] for v in garment]
    garment_max, garment_min = max(garment_y), min(garment_y)
    span = garment_max - garment_min
    if span <= 0.0:
        raise SystemExit("garment has no height span; cannot scale trousers")
    gcx = sum(v[0] for v in garment) / len(garment)
    gcz = sum(v[2] for v in garment) / len(garment)
    waist_y = min_y + WAIST * height
    ankle_y = min_y + ANKLE * height
    scale = (waist_y - ankle_y) / span

    def place(x, y, z):
        return (cx + (x - gcx) * scale, waist_y - (garment_max - y) * scale, cz + (z - gcz) * scale)

    return place, scale, waist_y


def footwear_placement(body, garment, foot_side):
    """Sole length sets the scale; one boot rests on the chosen foot."""
    body_y = [v[1] for v in body]
    min_y, max_y = min(body_y), max(body_y)
    foot = [v for v in body if v[1] <= min_y + FOOT_BAND * (max_y - min_y)]
    if not foot:
        raise SystemExit("body has no vertices in the foot band; cannot place footwear")
    split = sum(v[0] for v in foot) / len(foot)
    side = [v for v in foot if (v[0] < split) == (foot_side == "left")]
    if not side:
        raise SystemExit(f"body has no `{foot_side}` foot in the band; cannot place footwear")
    cx = sum(v[0] for v in side) / len(side)
    cz = sum(v[2] for v in side) / len(side)
    body_sole_z = max(v[2] for v in side) - min(v[2] for v in side)

    garment_y = [v[1] for v in garment]
    garment_min = min(garment_y)
    span = max(garment_y) - garment_min
    sole = [v for v in garment if v[1] <= garment_min + FOOT_BAND * span]
    if not sole:
        raise SystemExit("garment has no sole band; cannot place footwear")
    garment_sole_z = max(v[2] for v in sole) - min(v[2] for v in sole)
    if garment_sole_z <= 0.0:
        raise SystemExit("garment sole band has no length; cannot scale footwear")
    gcx = sum(v[0] for v in sole) / len(sole)
    gcz = sum(v[2] for v in sole) / len(sole)
    scale = body_sole_z / garment_sole_z

    def place(x, y, z):
        return (cx + (x - gcx) * scale, min_y + (y - garment_min) * scale, cz + (z - gcz) * scale)

    return place, scale, min_y


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--garment", required=True)
    parser.add_argument("--body", required=True)
    parser.add_argument("--class", dest="kind", required=True, choices=KINDS)
    parser.add_argument("--out", required=True)
    parser.add_argument("--foot", choices=("left", "right"), default="left",
                        help="which foot a single footwear piece anchors on")
    args = parser.parse_args()

    body, _ = load_vertices(args.body)
    garment, lines = load_vertices(args.garment)
    if args.kind == "upper-body":
        place, scale, anchor = upper_body_placement(body, garment)
    elif args.kind == "trousers":
        place, scale, anchor = trousers_placement(body, garment)
    elif args.kind == "footwear":
        place, scale, anchor = footwear_placement(body, garment, args.foot)
    elif args.kind in HEIGHT_PLACED:
        place, scale, anchor = height_placement(body, garment)
    else:
        raise SystemExit(f"class `{args.kind}` has no placement rule")

    index = 0
    with open(args.out, "w", encoding="utf-8") as sink:
        for line in lines:
            if line.startswith("v "):
                x, y, z = place(*garment[index])
                index += 1
                sink.write(f"v {x:.6f} {y:.6f} {z:.6f}\n")
            else:
                sink.write(line)
    label = {"upper-body": "collar_y", "trousers": "waist_y", "footwear": "sole_y"}.get(args.kind, "base_y")
    suffix = f" foot={args.foot}" if args.kind == "footwear" else ""
    print(f"align: class={args.kind} scale={scale:.4f} {label}={anchor:.4f} verts={len(garment)}{suffix}")


if __name__ == "__main__":
    main()