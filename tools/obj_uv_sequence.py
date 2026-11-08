"""Write a simulated OBJ frame and keep the garment's loop UVs.

The legacy bake moves vertices without changing topology or UVs, so every
sampled frame repeats one `vt` block keyed on the rounded loop UVs. Without
that block the combined GLB loses `TEXCOORD_0` and the cloth samples a single
texel, which was the white-cloak defect.
"""


def write_obj(path, coords, polygons, uv_layer):
    """Write positions, faces and the garment's loop UVs."""
    uv_rows = []
    uv_index = {}
    corners = []
    if uv_layer is not None:
        loop_uvs = [datum.uv for datum in uv_layer.data]
        for polygon in polygons:
            row = []
            for loop in range(polygon.loop_start, polygon.loop_start + polygon.loop_total):
                key = (round(loop_uvs[loop][0], 6), round(loop_uvs[loop][1], 6))
                index = uv_index.get(key)
                if index is None:
                    index = len(uv_rows)
                    uv_index[key] = index
                    uv_rows.append(key)
                row.append(index)
            corners.append(row)
    with open(path, "w", encoding="utf-8") as sink:
        for co in coords:
            sink.write(f"v {co.x:.6f} {co.y:.6f} {co.z:.6f}\n")
        for u, v in uv_rows:
            sink.write(f"vt {u:.6f} {v:.6f}\n")
        for index, polygon in enumerate(polygons):
            if uv_layer is None:
                sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")
            else:
                sink.write(
                    "f "
                    + " ".join(
                        f"{vertex + 1}/{corner + 1}"
                        for vertex, corner in zip(polygon.vertices, corners[index])
                    )
                    + "\n"
                )
