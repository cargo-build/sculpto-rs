"""Restore source corner UVs on a fitted or transported mesh.

Usage:
  python3 tools/merge_uv.py --fitted <fitted.obj> --source <aligned garment.obj> \
      --out <garment-uv.obj>

The sculpto CLI keeps vertex indices and triangle corner order stable through
`garment aura` and `garment follow`: `read_obj` fan-triangulates polygons
around their first corner, and these operations rewrite positions only.
The CLI writes OBJs without `vt` data, so
the fitted mesh loses the garment's UVs. This tool rebuilds them by repeating
the same fan triangulation on the source and looking each fitted triangle up
by its vertex triple, so the bake can emit `vt` data the combined GLB turns
into a textured primitive.
"""

import argparse


def fan_triangles(corners):
    """Same fan rule as the CLI's `read_obj`."""
    return [(corners[0], corners[i], corners[i + 1]) for i in range(1, len(corners) - 1)]


def parse_corner(token, path):
    parts = token.split("/")
    try:
        vertex = int(parts[0]) - 1
    except ValueError:
        raise SystemExit(f"{path}: face corner `{token}` is not a vertex index")
    if vertex < 0:
        raise SystemExit(f"{path}: face corner `{token}` is not 1-based")
    uv = None
    if len(parts) > 1 and parts[1]:
        uv = int(parts[1]) - 1
        if uv < 0:
            raise SystemExit(f"{path}: texture index `{parts[1]}` is not 1-based")
    return vertex, uv


def read_source(path):
    """Positions, texture coordinates, per-triangle corner UVs and per-vertex UVs."""
    positions = 0
    uvs = []
    corner_map = {}
    vertex_uv = {}
    with open(path, encoding="utf-8") as sink:
        for line in sink:
            if line.startswith("v "):
                positions += 1
            elif line.startswith("vt "):
                uvs.append(tuple(float(value) for value in line.split()[1:]))
            elif line.startswith("f "):
                corners = [parse_corner(token, path) for token in line.split()[1:]]
                if any(uv is None for _, uv in corners):
                    raise SystemExit(f"{path}: face without texture indices: {line.strip()}")
                for vertex, uv in corners:
                    vertex_uv.setdefault(vertex, uv)
                for triangle in fan_triangles(corners):
                    key = tuple(vertex for vertex, _ in triangle)
                    corner_map.setdefault(key, tuple(uv for _, uv in triangle))
    if positions == 0:
        raise SystemExit(f"{path}: no vertex lines")
    if not uvs:
        raise SystemExit(f"{path}: no `vt` lines; nothing to merge")
    if not corner_map:
        raise SystemExit(f"{path}: no triangles")
    return positions, uvs, corner_map, vertex_uv


def read_fitted(path):
    positions = []
    triangles = []
    with open(path, encoding="utf-8") as sink:
        for line in sink:
            if line.startswith("v "):
                x, y, z = line.split()[1:4]
                positions.append((float(x), float(y), float(z)))
            elif line.startswith("f "):
                corners = [parse_corner(token, path)[0] for token in line.split()[1:]]
                triangles.extend(fan_triangles(corners))
    if not positions:
        raise SystemExit(f"{path}: no vertex lines")
    if not triangles:
        raise SystemExit(f"{path}: no triangles")
    return positions, triangles


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fitted", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    source_verts, source_uvs, corner_map, vertex_uv = read_source(args.source)
    positions, triangles = read_fitted(args.fitted)

    kept = []
    matched = 0
    unmatched = 0
    for triangle in triangles:
        uvs = corner_map.get(triangle)
        if uvs is not None:
            matched += 1
            kept.append((triangle, uvs))
            continue
        reversed_triangle = (triangle[0], triangle[2], triangle[1])
        uvs = corner_map.get(reversed_triangle)
        if uvs is not None:
            matched += 1
            kept.append((triangle, (uvs[0], uvs[2], uvs[1])))
            continue
        unmatched += 1
        fallback = []
        for vertex in triangle:
            uv = vertex_uv.get(vertex)
            if uv is None:
                raise SystemExit(f"{args.fitted}: vertex {vertex + 1} has no source UV; the fitted mesh is not derived from `{args.source}`")
            fallback.append(uv)
        kept.append((triangle, tuple(fallback)))

    used = sorted({vertex for triangle, _ in kept for vertex in triangle})
    if used and used[-1] >= len(positions):
        raise SystemExit(f"{args.fitted}: vertex index out of range")
    remap = {vertex: index for index, vertex in enumerate(used)}

    uv_index = {}
    uv_order = []
    # Deduplicate on the rounded coordinates so the OBJ stays compact while
    # the bake's importer still sees one UV island per collapsed value.
    corner_uv = []
    for _, uvs in kept:
        row = []
        for uv in uvs:
            key = tuple(round(value, 6) for value in source_uvs[uv])
            index = uv_index.get(key)
            if index is None:
                index = len(uv_order)
                uv_index[key] = index
                uv_order.append(source_uvs[uv])
            row.append(index)
        corner_uv.append(row)

    with open(args.out, "w", encoding="utf-8") as sink:
        for vertex in used:
            x, y, z = positions[vertex]
            sink.write(f"v {x:.6f} {y:.6f} {z:.6f}\n")
        for uv in uv_order:
            sink.write("vt " + " ".join(f"{value:.6f}" for value in uv) + "\n")
        for (triangle, _), uvs in zip(kept, corner_uv):
            sink.write(
                "f "
                + " ".join(
                    f"{remap[vertex] + 1}/{uv + 1}" for vertex, uv in zip(triangle, uvs)
                )
                + "\n"
            )
    print(
        f"merge_uv: triangles={len(triangles)} matched={matched} unmatched={unmatched} "
        f"vt={len(uv_order)} verts={len(used)}"
    )


if __name__ == "__main__":
    main()
