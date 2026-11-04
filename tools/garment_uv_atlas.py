"""Native diagnosed-chart repair; the pinned Rust core owns UV qualification."""

import hashlib
import json
import subprocess

import bpy
import numpy as np


def coordinates(mesh):
    return np.asarray([tuple(loop.uv) for loop in mesh.uv_layers.active.data], dtype=np.float32)


def digest(uv):
    return hashlib.sha256(np.ascontiguousarray(uv).tobytes()).hexdigest()


def edge_ratios(uv):
    corners = uv.reshape(-1, 3, 2).astype(np.float64)
    lengths = np.linalg.norm(corners - np.roll(corners, 1, axis=1), axis=2)
    return lengths / lengths.sum(axis=1)[:, None]


def check(mesh, binary, output, stage):
    uv = coordinates(mesh)
    # The array also preserves invalid native values if JSON cannot encode them.
    np.save(output / ("uv-" + stage + ".npy"), uv, allow_pickle=False)
    path = output / ("uv-" + stage + ".json")
    path.write_text(json.dumps({"triangles": uv.reshape(-1, 3, 2).tolist()}, allow_nan=False) + "\n")
    report_path = output / ("uv-" + stage + "-report.json")
    result = subprocess.run([str(binary), "garment", "uv-check", "--uv", str(path), "--report", str(report_path)],
                            capture_output=True, text=True, check=False)
    if not report_path.is_file():
        raise RuntimeError(f"UV qualifier produced no report: {result.stderr.strip()}")
    report = json.loads(report_path.read_text())
    if result.returncode != (0 if report["verdict"]["passed"] else 1):
        raise RuntimeError(f"UV qualifier exit/report disagreement: {result.returncode}: {result.stderr.strip()}")
    return report


def adjacency(mesh, uv, excluded):
    owners = {}
    pairs = set()
    for face in mesh.polygons:
        if face.index in excluded:
            continue
        corners = list(face.loop_indices)
        for first, second in zip(corners, corners[1:] + corners[:1]):
            a, b = mesh.loops[first].vertex_index, mesh.loops[second].vertex_index
            key = (a, b, tuple(uv[first]), tuple(uv[second])) if a < b else (b, a, tuple(uv[second]), tuple(uv[first]))
            if key in owners:
                pairs.add(tuple(sorted((owners[key], face.index))))
            else:
                owners[key] = face.index
    return pairs


def isolate_faces(target, uv, affected):
    mesh = target.data
    world = np.asarray([target.matrix_world @ vertex.co for vertex in mesh.vertices], dtype=np.float64)
    triangles = np.asarray([tuple(face.vertices) for face in mesh.polygons], dtype=np.int32)
    points = world[triangles]
    area = np.linalg.norm(np.cross(points[:, 1] - points[:, 0], points[:, 2] - points[:, 0]), axis=1) / 2
    corners = uv.reshape(-1, 3, 2).astype(np.float64)
    a, b = corners[:, 1] - corners[:, 0], corners[:, 2] - corners[:, 0]
    uv_area = np.abs(a[:, 0] * b[:, 1] - a[:, 1] * b[:, 0]) / 2
    valid = (uv_area > 0) & (area > 0)
    if np.any(area <= 0):
        raise ValueError("cannot derive triangle charts from collapsed geometry")
    if valid.any():
        density = float(np.sqrt(uv_area[valid].sum() / area[valid].sum()))
        basis = "measured valid UV/world area ratio"
    else:
        density = float(1 / np.sqrt(area.sum()))
        basis = "unit total area from geometry; every input UV triangle is collapsed"
    charts = []
    for face in affected:
        p = points[face]
        edge = p[1] - p[0]
        length = np.linalg.norm(edge)
        delta = p[2] - p[0]
        charts.append(np.asarray([[0, 0], [length, 0],
            [np.dot(delta, edge) / length, np.linalg.norm(np.cross(edge, delta)) / length]]) * density)
    stride = 1 + max(float(np.ptp(chart, axis=0).max()) for chart in charts)
    for index, (face, chart) in enumerate(zip(affected, charts)):
        # Distinct corner coordinates break UV adjacency without physical cuts.
        uv[3 * face:3 * face + 3] = chart + (2 + index * stride, 2)
    mesh.uv_layers.active.data.foreach_set("uv", uv.ravel())
    return density, basis


def pack_once(target):
    bpy.ops.object.select_all(action="DESELECT")
    target.select_set(True)
    bpy.context.view_layer.objects.active = target
    bpy.context.scene.tool_settings.use_uv_select_sync = False
    bpy.ops.object.mode_set(mode="EDIT")
    try:
        bpy.ops.mesh.select_all(action="SELECT")
        bpy.ops.uv.select_all(action="SELECT")
        bpy.ops.uv.pack_islands(udim_source="ACTIVE_UDIM", rotate=True, rotate_method="CARDINAL",
            scale=True, merge_overlap=False, margin_method="FRACTION", margin=0.02,
            pin=False, shape_method="CONCAVE")
    finally:
        bpy.ops.object.mode_set(mode="OBJECT")


def qualify_atlas(target, binary, output, record):
    mesh = target.data
    before = coordinates(mesh)
    record.update(operation="retain", before_sha256=digest(before))
    record["before"] = check(mesh, binary, output, "before")
    if record["before"]["verdict"]["passed"]:
        record.update(after_sha256=digest(before), affected_faces=[], after=record["before"])
        return
    affected = sorted(set(record["before"]["degenerate_triangles"]) |
                      {face for pair in record["before"]["overlap_pairs"] for face in pair})
    if not affected:
        raise ValueError("UV atlas bounds failed; no overlap/collapse chart repair applies")
    excluded = set(affected)
    edges = adjacency(mesh, before, excluded)
    record.update(operation="isolate diagnosed triangle charts; native pack once", affected_faces=affected,
                  pack_settings=dict(udim_source="ACTIVE_UDIM", rotate=True, rotate_method="CARDINAL", margin=0.02,
                      margin_method="FRACTION", scale=True, merge_overlap=False, pin=False, shape_method="CONCAVE"))
    density, basis = isolate_faces(target, before.copy(), affected)
    record.update(triangle_chart_density=density, triangle_chart_density_basis=basis)
    unaffected = np.asarray([face.index not in excluded for face in mesh.polygons])
    isolated = coordinates(mesh).reshape(-1, 3, 2)
    record["unaffected_corners_before_pack_preserved"] = bool(np.array_equal(
        isolated[unaffected], before.reshape(-1, 3, 2)[unaffected]))
    if not record["unaffected_corners_before_pack_preserved"]:
        raise ValueError("triangle-chart isolation changed unaffected UV interiors")
    pack_once(target)
    after = coordinates(mesh)
    record["after_sha256"] = digest(after)
    record["after"] = check(mesh, binary, output, "after")
    record["unaffected_adjacency_preserved"] = adjacency(mesh, after, excluded) == edges
    if unaffected.any():
        record["unaffected_edge_ratio_max_error"] = float(np.abs(
            edge_ratios(before.reshape(-1, 3, 2)[unaffected].reshape(-1, 2)) -
            edge_ratios(after.reshape(-1, 3, 2)[unaffected].reshape(-1, 2))).max())
    else:
        record["unaffected_edge_ratio_max_error"] = 0.0
    if not record["unaffected_adjacency_preserved"]:
        raise ValueError("native packing changed unaffected chart interiors")
    if not record["after"]["verdict"]["passed"]:
        raise ValueError("UV atlas failed complete qualification after one native repack")
