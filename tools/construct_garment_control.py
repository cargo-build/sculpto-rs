"""Reconstruct a source-guided control surface from a thick garment shell (@req DECL0013).

Blender: --background --factory-startup --python-exit-code 1
--python tools/construct_garment_control.py -- recipe.json new-output-directory

The adapter reads a digest-pinned `.blend` object and a digest-pinned control
profile, asks `sculpto garment control-exterior|control-repair|control-check` for
every geometric decision, and performs only the native mesh operators: triangle
fill, edge collapse, triangulation and the decimation ratio the plan declares.
Every removed, filled and collapsed region is reported in `receipt.json`.

Frames: the profile guide and aperture anchors are source-local. The CLI speaks
the Sculpto OBJ display frame, where display `(x, y, z)` is source `(x, -z, y)`.
The receipt records the source object's matrix separately; no axis conversion is
implicit and the original object is never modified.
"""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

import bmesh
import bpy
from mathutils import Matrix, Vector
from mathutils.bvhtree import BVHTree
import numpy as np


FRAMES = ("source-local-z-up", "sculpto-display-y-up")
# Rendering is a viewing aid, never fit evidence (garment-authoring § 4).
RENDER_NOTE = "matched views for review; topology and contacts come from control-check"


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_json(path):
    return json.loads(Path(path).read_text())


def run_cli(binary, arguments):
    result = subprocess.run(
        [str(binary), *arguments], capture_output=True, text=True, check=False
    )
    output = (result.stdout + result.stderr).strip()
    if result.returncode != 0:
        raise RuntimeError(f"sculpto {' '.join(arguments)} failed:\n{output}")
    print(" ".join(["sculpto", *arguments]), flush=True)
    print(output, flush=True)
    return output


def write_obj(path, positions, triangles):
    """Source-local positions to the Sculpto display frame `(x, z, -y)`."""
    lines = ["# sculpto source-guided control"]
    lines.extend(
        f"v {point[0]:.9f} {point[2]:.9f} {-point[1]:.9f}" for point in positions
    )
    lines.extend(f"f {a + 1} {b + 1} {c + 1}" for a, b, c in triangles)
    Path(path).write_text("\n".join(lines) + "\n")


def read_obj(path):
    """Sculpto display OBJ to source-local positions."""
    positions = []
    triangles = []
    for line in Path(path).read_text().splitlines():
        if line.startswith("v "):
            _, x, y, z = line.split()[:4]
            positions.append((float(x), -float(z), float(y)))
        elif line.startswith("f "):
            corners = [int(corner.split("/")[0]) - 1 for corner in line.split()[1:]]
            if len(corners) < 3:
                raise ValueError(f"{path}: face has fewer than three corners")
            triangles.extend(
                [corners[0], a, b] for a, b in zip(corners[1:], corners[2:])
            )
    if not positions:
        raise ValueError(f"{path}: no vertex lines")
    if triangles and max(max(triangle) for triangle in triangles) >= len(positions):
        raise ValueError(f"{path}: face references a vertex that does not exist")
    return positions, triangles


def source_object(source, name):
    with bpy.data.libraries.load(str(source), link=False) as (available, wanted):
        if name not in available.objects:
            raise ValueError(f"{source} has no object `{name}`")
        wanted.objects = [name]
    obj = wanted.objects[0]
    if obj is None or obj.type != "MESH":
        raise ValueError(f"`{name}` is not a mesh object")
    bpy.context.collection.objects.link(obj)
    bpy.context.view_layer.update()
    return obj


def triangulated(obj):
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.verts.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    bm.verts.index_update()
    positions = [tuple(vertex.co) for vertex in bm.verts]
    triangles = [[vertex.index for vertex in face.verts] for face in bm.faces]
    bm.free()
    return positions, triangles


def control_mesh(name):
    mesh = bpy.data.meshes.new(name)
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    return obj


def set_geometry(obj, positions, triangles):
    mesh = obj.data
    mesh.clear_geometry()
    mesh.from_pydata([tuple(point) for point in positions], [], triangles)
    mesh.update()


def loop_edges(bm, vertices):
    bm.verts.ensure_lookup_table()
    edges = []
    pairs = list(zip(vertices, vertices[1:] + vertices[:1]))
    for first, second in pairs:
        start, end = bm.verts[first], bm.verts[second]
        edge = next(
            (candidate for candidate in start.link_edges if candidate.other_vert(start) is end),
            None,
        )
        if edge is None:
            raise RuntimeError(f"loop edge {first}-{second} is not a native control edge")
        edges.append(edge)
    return edges


def edge_direction(face, edge):
    """The edge endpoints in the order this face traverses them."""
    for loop in face.loops:
        if loop.edge is edge:
            return loop.vert, loop.link_loop_next.vert
    raise RuntimeError("face does not traverse the requested edge")


def orient_new_faces(new_faces):
    """Propagate the kept surface's winding through the entire generated patch."""
    created = [face for face in new_faces if face.is_valid]
    created_set = set(created)
    pending = [
        (face, other, edge)
        for face in created
        for edge in face.edges
        for other in edge.link_faces
        if other not in created_set
    ]
    visited = set()
    flipped = 0
    while pending:
        face, other, shared = pending.pop()
        if face in visited:
            continue
        mine = edge_direction(face, shared)
        theirs = edge_direction(other, shared)
        if mine[0] is theirs[0] and mine[1] is theirs[1]:
            face.normal_flip()
            flipped += 1
        visited.add(face)
        for edge in face.edges:
            pending.extend(
                (neighbor, face, edge)
                for neighbor in edge.link_faces
                if neighbor in created_set and neighbor not in visited
            )
    if len(visited) != len(created):
        raise RuntimeError("generated patch has faces disconnected from the kept surface")
    return flipped


CLOSURES = ("triangulated", "boundary")


def element_snapshot(bm):
    """Identity sets of every live element, for exact rollback comparison."""
    bm.verts.ensure_lookup_table()
    bm.edges.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    return {"verts": set(bm.verts), "edges": set(bm.edges), "faces": set(bm.faces)}


def new_elements(bm, before):
    """Elements that did not exist before the attempt, including partial mutation."""
    bm.verts.ensure_lookup_table()
    bm.edges.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    return (
        [face for face in bm.faces if face not in before["faces"]],
        [edge for edge in bm.edges if edge not in before["edges"]],
        [vertex for vertex in bm.verts if vertex not in before["verts"]],
    )


def rollback_patch(bm, before, loop_edges):
    """Remove only what the attempt created; pre-existing geometry must survive."""
    stale = [face for face in before["faces"] if not face.is_valid]
    if stale:
        raise RuntimeError(f"rollback would lose {len(stale)} pre-existing face(s)")
    created, _, _ = new_elements(bm, before)
    if created:
        # FACES_ONLY leaves pre-existing boundary edges and vertices in place.
        bmesh.ops.delete(bm, geom=created, context="FACES_ONLY")
    bm.edges.ensure_lookup_table()
    orphan_edges = [edge for edge in bm.edges if edge not in before["edges"] and not edge.link_faces]
    if orphan_edges:
        bmesh.ops.delete(bm, geom=orphan_edges, context="EDGES")
    bm.verts.ensure_lookup_table()
    orphan_verts = [vertex for vertex in bm.verts if vertex not in before["verts"] and not vertex.link_edges]
    if orphan_verts:
        bmesh.ops.delete(bm, geom=orphan_verts, context="VERTS")
    bm.faces.ensure_lookup_table()
    bm.edges.ensure_lookup_table()
    bm.verts.ensure_lookup_table()
    surviving = {
        "faces": sum(1 for face in before["faces"] if face.is_valid),
        "edges": sum(1 for edge in before["edges"] if edge.is_valid),
        "verts": sum(1 for vertex in before["verts"] if vertex.is_valid),
    }
    expected = {
        "faces": len(before["faces"]),
        "edges": len(before["edges"]),
        "verts": len(before["verts"]),
    }
    if surviving != expected:
        raise RuntimeError(f"rollback lost pre-existing geometry: {surviving} != {expected}")
    if (len(bm.faces), len(bm.edges), len(bm.verts)) != (
        expected["faces"], expected["edges"], expected["verts"]
    ):
        raise RuntimeError(
            "rollback left generated geometry: "
            f"{len(bm.faces)} faces / {len(bm.edges)} edges / {len(bm.verts)} vertices, expected "
            f"{expected['faces']} / {expected['edges']} / {expected['verts']}"
        )
    open_loop = [edge for edge in loop_edges if edge.is_valid and len(edge.link_faces) != 1]
    if open_loop:
        raise RuntimeError(f"rollback changed {len(open_loop)} loop edge(s)")


def verify_patch(bm, loop_edges, created, loop_vertices):
    """Complete patch topology: manifold, closed loop, consistent winding."""
    loop_edges_set = set(loop_edges)
    if not created:
        raise RuntimeError("the operator created no face")
    for face in created:
        if not face.is_valid:
            raise RuntimeError("the operator returned a face that is no longer valid")
        if len(face.verts) != 3:
            raise RuntimeError(f"generated face has {len(face.verts)} corners, not 3")
        if face.calc_area() <= 0.0:
            raise RuntimeError("generated face has no area")
    bm.edges.ensure_lookup_table()
    for edge in bm.edges:
        faces = len(edge.link_faces)
        if faces > 2:
            raise RuntimeError(
                "generated patch makes an edge non-manifold "
                f"({faces} faces at {[round(value, 6) for value in (edge.verts[0].co + edge.verts[1].co) / 2]})"
            )
        if faces == 2:
            directions = [edge_direction(face, edge) for face in edge.link_faces]
            if directions[0][0] is directions[1][0] and directions[0][1] is directions[1][1]:
                raise RuntimeError("generated patch reverses face winding")
    open_loop = [edge for edge in loop_edges_set if edge.is_valid and len(edge.link_faces) != 2]
    if open_loop:
        raise RuntimeError(
            f"generated patch left {len(open_loop)} of {len(loop_vertices)} loop edge(s) open"
        )


def boundary_patch(bm, edges):
    """One n-gon over the loop edges only, then triangulate it."""
    result = bmesh.ops.holes_fill(bm, edges=edges, sides=len(edges))
    patch = [face for face in result.get("faces", []) if isinstance(face, bmesh.types.BMFace)]
    if not patch:
        raise RuntimeError("holes_fill created no face")
    bmesh.ops.triangulate(bm, faces=[face for face in patch if face.is_valid])


def chord_patch(bm, edges):
    """Free-chord triangle fill over the loop edges."""
    result = bmesh.ops.triangle_fill(bm, use_beauty=True, edges=edges)
    patch = [face for face in result.get("geom", []) if isinstance(face, bmesh.types.BMFace)]
    if not patch:
        raise RuntimeError("triangle_fill created no face")


def apply_patch(bm, kind, edges):
    if kind == "boundary":
        boundary_patch(bm, edges)
    elif kind == "triangulated":
        chord_patch(bm, edges)
    else:
        raise RuntimeError(f"unknown closure kind `{kind}`")


def fill_loop(bm, vertices, report, closure):
    """Close one loop as a native transaction: capture, verify, or roll back.

    The core plans the patch kind from topology: a loop that straddles an existing
    edge is closed with a boundary patch, because a free chord would reuse that
    edge. Both operators are complementary in practice, so the other one is tried
    once when verification rejects the planned patch. Every created face is captured
    from the mesh itself (not from the operator return value), so a partial mutation
    or a triangulated n-gon is owned completely; a rejected attempt is rolled back to
    the exact pre-existing geometry, and a double failure is an explicit error.
    """
    if closure not in CLOSURES:
        raise RuntimeError(
            f"plan action has closure `{closure}`, expected one of {CLOSURES}"
        )
    edges = loop_edges(bm, vertices)
    order = ["boundary", "triangulated"] if closure == "boundary" else ["triangulated", "boundary"]
    attempts = []
    for kind in order:
        before = element_snapshot(bm)
        failure = None
        try:
            apply_patch(bm, kind, edges)
        except Exception as error:  # noqa: BLE001 - partial mutation may exist
            failure = f"{type(error).__name__}: {error}"
        created, created_edges, created_verts = new_elements(bm, before)
        flipped = 0
        if failure is None:
            try:
                flipped = orient_new_faces(created)
                verify_patch(bm, edges, created, vertices)
            except Exception as error:  # noqa: BLE001
                failure = f"{type(error).__name__}: {error}"
        if failure is not None:
            attempts.append(
                {
                    "closure": kind,
                    "error": failure,
                    "partial_faces": len(created),
                    "partial_edges": len(created_edges),
                }
            )
            rollback_patch(bm, before, edges)
            continue
        report.append(
            {
                "operator": "holes_fill+triangulate" if kind == "boundary" else "triangle_fill",
                "closure": kind,
                "planned": closure,
                "fallback": bool(attempts),
                "edges": len(edges),
                "faces": len(created),
                "new_edges": len(created_edges),
                "new_vertices": len(created_verts),
                "flipped": flipped,
                "rejected": attempts,
            }
        )
        return
    raise RuntimeError(
        f"loop closure failed for {len(edges)} edges after {len(attempts)} verified "
        f"attempts: {attempts}"
    )


def tidy(bm):
    """Triangulate, drop degenerate edges and unreferenced vertices, then report."""
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bmesh.ops.dissolve_degenerate(bm, dist=1e-5, edges=bm.edges[:])
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    loose = [vertex for vertex in bm.verts if not vertex.link_faces]
    if loose:
        bmesh.ops.delete(bm, geom=loose, context="VERTS")
    bm.verts.ensure_lookup_table()
    bm.faces.ensure_lookup_table()
    bm.verts.index_update()


def mesh_arrays(bm):
    return (
        [tuple(vertex.co) for vertex in bm.verts],
        [[vertex.index for vertex in face.verts] for face in bm.faces],
    )


def boundary_loops(bm):
    bmesh.ops.triangulate(bm, faces=bm.faces[:])
    bm.verts.ensure_lookup_table()
    edges = [edge for edge in bm.edges if edge.is_boundary]
    visited = set()
    loops = []
    for edge in edges:
        if edge in visited:
            continue
        loop = [edge]
        visited.add(edge)
        current = edge.verts[1]
        start = edge.verts[0]
        closed = False
        while True:
            following = next(
                (
                    candidate
                    for candidate in current.link_edges
                    if candidate.is_boundary and candidate not in visited
                ),
                None,
            )
            if following is None:
                break
            loop.append(following)
            visited.add(following)
            current = following.other_vert(current)
            if current is start:
                closed = True
                break
        loops.append({"edges": len(loop), "closed": closed})
    loops.sort(key=lambda item: item["edges"], reverse=True)
    return len(edges), loops


def stage_record(path, vertices, triangles):
    """Identity of one native stage: file digest plus counts."""
    return {
        "file": Path(path).name,
        "sha256": sha256(path),
        "vertices": int(vertices),
        "triangles": int(triangles),
    }


def save_npz(path, positions, triangles):
    display = np.array(
        [[point[0], point[2], -point[1]] for point in positions], dtype=np.float32
    )
    ordered = np.array(triangles, dtype=np.int32)
    np.savez_compressed(path, vertices=display, triangles=ordered)
    return {
        "vertices": int(len(display)),
        "triangles": int(len(ordered)),
        "npz_sha256": sha256(path),
    }


def deviation_tree(positions, triangles):
    return BVHTree.FromPolygons(
        [Vector(point) for point in positions], [tuple(face) for face in triangles]
    )


def deviation(points, tree):
    distances = np.array(
        [tree.find_nearest(Vector(point))[3] for point in points], dtype=np.float64
    )
    if not len(distances):
        return {"points": 0}
    return {
        "points": int(len(distances)),
        "mean": float(distances.mean()),
        "p95": float(np.percentile(distances, 95)),
        "max": float(distances.max()),
    }


def reference_comparison(reference, controls):
    """Compare against a frozen control in the common Sculpto display frame."""
    with np.load(reference) as archive:
        vertices = np.asarray(archive["vertices"], dtype=np.float32)
        triangles = np.asarray(archive["triangles"], dtype=np.int32)
    control_vertices = np.array(
        [[point[0], point[2], -point[1]] for point in controls[0]], dtype=np.float32
    )
    control_triangles = np.asarray(controls[1], dtype=np.int32)
    frozen = deviation_tree(vertices, triangles)
    candidate = deviation_tree(control_vertices, control_triangles)
    return {
        "reference": {"vertices": int(len(vertices)), "triangles": int(len(triangles))},
        "ordered_topology_matches": bool(
            len(triangles) == len(control_triangles)
            and np.array_equal(triangles, control_triangles)
        ),
        "reference_to_control": deviation(vertices, candidate),
        "control_to_reference": deviation(control_vertices, frozen),
    }


def render_views(source, control, matrix, out_dir, label):
    """Matched orthographic views of source and control in the source world frame."""
    control.matrix_world = matrix.copy()
    scene = bpy.context.scene
    points = [
        matrix @ vertex.co for vertex in source.data.vertices
    ] + [control.matrix_world @ vertex.co for vertex in control.data.vertices]
    low = Vector((min(point.x for point in points), min(point.y for point in points), min(point.z for point in points)))
    high = Vector((max(point.x for point in points), max(point.y for point in points), max(point.z for point in points)))
    center = (low + high) / 2.0
    scale = 1.0 / max(high - low)
    for candidate in (source, control):
        candidate.matrix_world = (
            Matrix.Scale(scale, 4) @ Matrix.Translation(-center) @ candidate.matrix_world
        )
    scene.render.engine = "BLENDER_WORKBENCH"
    scene.render.resolution_x = 1100
    scene.render.resolution_y = 750
    scene.display.shading.light = "STUDIO"
    scene.display.shading.color_type = "SINGLE"
    scene.display.shading.single_color = (0.55, 0.65, 0.55)
    scene.display.shading.show_shadows = True
    scene.display.shading.show_cavity = True
    scene.display.shading.background_type = "WORLD"
    scene.world = bpy.data.worlds.new("ControlReview")
    scene.world.color = (0.08, 0.08, 0.08)
    camera = bpy.data.objects.new("Camera", bpy.data.cameras.new("Camera"))
    scene.collection.objects.link(camera)
    scene.camera = camera
    camera.data.type = "ORTHO"
    camera.data.ortho_scale = 1.3 * max(
        1.0, scene.render.resolution_x / scene.render.resolution_y
    )
    written = []
    for view, position in (("front", (0.0, -3.0, 0.0)), ("side", (3.0, 0.0, 0.0))):
        camera.location = Vector(position)
        camera.rotation_euler = (-camera.location).to_track_quat("-Z", "Y").to_euler()
        for name, candidate in (("source", source), ("control", control)):
            source.hide_render = candidate is not source
            control.hide_render = candidate is not control
            scene.render.filepath = str(out_dir / f"{label}-{view}-{name}.png")
            bpy.ops.render.render(write_still=True)
            written.append(scene.render.filepath)
    return written


def recipe_inputs(recipe):
    source = Path(recipe["source"])
    profile = Path(recipe["profile"])
    binary = Path(recipe["sculpto"])
    if sha256(source) != recipe["source_sha256"]:
        raise ValueError(f"{source} digest does not match the recipe")
    if sha256(profile) != recipe["profile_sha256"]:
        raise ValueError(f"{profile} digest does not match the recipe")
    if not binary.is_file():
        raise ValueError(f"sculpto binary {binary} does not exist")
    declared = read_json(profile)
    if declared.get("frame") not in FRAMES:
        raise ValueError(f"profile frame must be one of {FRAMES}")
    return source, profile, binary, declared


def main(recipe_path, out_dir):
    started = time.monotonic()
    recipe = read_json(recipe_path)
    source_path, profile_path, binary, declared = recipe_inputs(recipe)
    output = Path(out_dir)
    if output.exists():
        raise FileExistsError(f"{output} already exists; exclusive output only")
    output.mkdir(parents=True, exist_ok=False)
    receipt = {
        "recipe": str(Path(recipe_path).resolve()),
        "source": str(source_path.resolve()),
        "source_sha256": recipe["source_sha256"],
        "profile": str(profile_path.resolve()),
        "profile_sha256": recipe["profile_sha256"],
        "object": recipe["object"],
        "frame": {
            "guide": declared["frame"],
            "source_local": "blender-source-local-z-up",
            "obj": "sculpto-display-y-up",
            "npz": "sculpto-display-y-up",
        },
        "renders_note": RENDER_NOTE,
    }
    try:
        bpy.ops.wm.read_factory_settings(use_empty=True)
        source = source_object(source_path, recipe["object"])
        receipt["source_matrix_world"] = [list(row) for row in source.matrix_world]
        receipt["source_dimensions"] = list(source.dimensions)
        positions, triangles = triangulated(source)
        receipt["source_geometry"] = {
            "vertices": len(positions),
            "triangles": len(triangles),
            "uv_layers": [
                {
                    "name": layer.name,
                    "sha256": hashlib.sha256(
                        np.asarray([uv.uv[:] for uv in layer.data], dtype=np.float64).tobytes()
                    ).hexdigest(),
                }
                for layer in source.data.uv_layers
            ],
            "materials": [
                {
                    "name": material.name,
                    "nodes": sorted(
                        node.type
                        for node in getattr(material, "node_tree", None).nodes
                    )
                    if material.use_nodes
                    else [],
                }
                for material in source.data.materials
            ],
            "unchanged": True,
        }
        source_obj = output / "source.obj"
        write_obj(source_obj, positions, triangles)

        exterior_plan_path = output / "exterior-plan.json"
        run_cli(
            binary,
            [
                "garment",
                "control-exterior",
                "--source",
                str(source_obj),
                "--profile",
                str(profile_path),
                "--out",
                str(output / "selected.obj"),
                "--plan",
                str(exterior_plan_path),
            ],
        )
        plan = read_json(exterior_plan_path)
        receipt["stages"] = {}
        receipt["stages"]["selected"] = stage_record(
            output / "selected.obj", plan["vertices"], plan["triangles"]
        )
        receipt["lineage"] = {
            "plan": exterior_plan_path.name,
            "plan_sha256": sha256(exterior_plan_path),
            "vertex_sources": len(plan["vertex_sources"]),
            "selected_faces": len(plan["selected_faces"]),
            "removed": {
                reason: len(faces) for reason, faces in plan["removed"].items()
            },
            "note": "full arrays are in the plan file; QEM stages have no original faces",
        }
        receipt["exterior"] = plan["composition"]
        receipt["apertures"] = plan["apertures"]["matched"]
        receipt["incidental"] = plan["incidental"]
        receipt["reduction_plan"] = plan["reduction"]

        control = control_mesh("SculptoControl")
        selected = read_obj(output / "selected.obj")
        set_geometry(control, *selected)
        if len(selected[0]) != plan["vertices"] or len(selected[1]) != plan["triangles"]:
            raise RuntimeError(
                "the selected OBJ does not match its plan; vertex order is not preserved"
            )

        bm = bmesh.new()
        bm.from_mesh(control.data)
        fills = []
        for action in plan["actions"]:
            if action["kind"] != "fill":
                raise RuntimeError(
                    "the source stage may not collapse geometry; the plan asked for "
                    f"{action['kind']} on loop {action['loop']}"
                )
            fill_loop(bm, action["vertices"], fills, action["closure"])
        tidy(bm)
        high_positions, high_triangles = mesh_arrays(bm)
        high_edges, high_loops = boundary_loops(bm)
        bm.to_mesh(control.data)
        bm.free()
        receipt["high_control"] = {
            "vertices": len(high_positions),
            "triangles": len(high_triangles),
            "boundary_edges": high_edges,
            "boundary_loops": high_loops,
            "fills": fills,
        }
        save_npz(output / "high_control.npz", high_positions, high_triangles)
        write_obj(output / "high_control.obj", high_positions, high_triangles)
        receipt["stages"]["high_control"] = stage_record(
            output / "high_control.obj", len(high_positions), len(high_triangles)
        )

        ratio = float(plan["reduction"]["ratio"])
        if ratio < 1.0:
            bpy.context.view_layer.objects.active = control
            control.select_set(True)
            modifier = control.modifiers.new("Decimate", "DECIMATE")
            modifier.decimate_type = "COLLAPSE"
            modifier.ratio = ratio
            bpy.ops.object.modifier_apply(modifier=modifier.name)
            control.select_set(False)
        decimated_positions, decimated_triangles = triangulated(control)
        write_obj(output / "decimated.obj", decimated_positions, decimated_triangles)
        # QEM output is a new surface, not the source faces: record the operation
        # and both stage geometries instead of inventing lineage.
        receipt["stages"]["decimated"] = stage_record(
            output / "decimated.obj", len(decimated_positions), len(decimated_triangles)
        )
        receipt["decimation"] = {
            "operator": "bpy Decimate COLLAPSE",
            "ratio": ratio,
            "target_triangles": plan["reduction"]["target_triangles"],
            "before_triangles": len(high_triangles),
            "after_triangles": len(decimated_triangles),
            "before_sha256": sha256(output / "high_control.obj"),
            "after_sha256": sha256(output / "decimated.obj"),
            "lineage": "not original faces: stage-local generated surface",
        }

        repair_plan_path = output / "repair-plan.json"
        run_cli(
            binary,
            [
                "garment",
                "control-repair",
                "--control",
                str(output / "decimated.obj"),
                "--profile",
                str(profile_path),
                "--out",
                str(output / "repaired.obj"),
                "--plan",
                str(repair_plan_path),
            ],
        )
        repair = read_json(repair_plan_path)
        repaired = read_obj(output / "repaired.obj")
        set_geometry(control, *repaired)
        if len(repaired[0]) != repair["vertices"] or len(repaired[1]) != repair["triangles"]:
            raise RuntimeError("the repaired OBJ does not match its plan")
        bm = bmesh.new()
        bm.from_mesh(control.data)
        bm.verts.ensure_lookup_table()
        # Resolve every planned element by identity before the first mutation, so a
        # collapse cannot make a later planned index name another vertex.
        resolved = []
        for action in repair["actions"]:
            if action["kind"] == "collapse":
                first, second = (
                    bm.verts[action["edge"][0]],
                    bm.verts[action["edge"][1]],
                )
                edge = next(
                    (candidate for candidate in first.link_edges if candidate.other_vert(first) is second),
                    None,
                )
                if edge is None:
                    raise RuntimeError(
                        f"planned collapse edge {action['edge']} is not a native edge"
                    )
                resolved.append((action, edge))
            else:
                vertices = [bm.verts[index] for index in action["vertices"]]
                resolved.append((action, vertices))
        actions = []
        for action, element in resolved:
            if action["kind"] == "collapse":
                bmesh.ops.collapse(bm, edges=[element])
                actions.append(
                    {"operator": "collapse", "loop": action["loop"], "edge": action["edge"]}
                )
            else:
                indices = [vertex.index for vertex in element]
                fills = []
                fill_loop(bm, indices, fills, action["closure"])
                actions.append({"operator": "fill", "loop": action["loop"], **fills[0]})
        tidy(bm)
        reduced_positions, reduced_triangles = mesh_arrays(bm)
        bm.to_mesh(control.data)
        bm.free()
        receipt["repair"] = {
            "tip_removed": repair["tip_removed"],
            "tip_vertices": repair["tip_vertices"],
            "tip_triangles": repair["tip_triangles"],
            "actions": actions,
        }
        write_obj(output / "pre_separation.obj", reduced_positions, reduced_triangles)
        receipt["stages"]["repaired"] = stage_record(
            output / "repaired.obj", repair["vertices"], repair["triangles"]
        )
        receipt["stages"]["pre_separation"] = stage_record(
            output / "pre_separation.obj", len(reduced_positions), len(reduced_triangles)
        )
        separation_report_path = output / "separation.json"
        run_cli(
            binary,
            [
                "garment",
                "control-separate",
                "--control",
                str(output / "pre_separation.obj"),
                "--profile",
                str(profile_path),
                "--out",
                str(output / "reduced_control.obj"),
                "--report",
                str(separation_report_path),
            ],
        )
        receipt["separation"] = read_json(separation_report_path)
        # The validated OBJ is the authoritative final array: read it back.
        reduced_positions, reduced_triangles = read_obj(output / "reduced_control.obj")
        set_geometry(control, reduced_positions, reduced_triangles)
        receipt["stages"]["reduced_control"] = stage_record(
            output / "reduced_control.obj", len(reduced_positions), len(reduced_triangles)
        )
        receipt["reduced_control"] = save_npz(
            output / "reduced_control.npz", reduced_positions, reduced_triangles
        )

        if "reference" in recipe:
            reference = Path(recipe["reference"])
            if sha256(reference) != recipe["reference_sha256"]:
                raise ValueError(f"{reference} digest does not match the recipe")
            receipt["reference_comparison"] = reference_comparison(
                reference, (reduced_positions, reduced_triangles)
            )
        # Matched views are a review aid for accepted and rejected controls alike.
        receipt["renders"] = render_views(
            source, control, Matrix(receipt["source_matrix_world"]), output, recipe["object"]
        )
        report_path = output / "validation.json"
        try:
            run_cli(
                binary,
                [
                    "garment",
                    "control-check",
                    "--control",
                    str(output / "reduced_control.obj"),
                    "--source",
                    str(source_obj),
                    "--profile",
                    str(profile_path),
                    "--report",
                    str(report_path),
                ],
            )
        finally:
            if report_path.exists():
                receipt["validation"] = read_json(report_path)
        receipt["reduced_obj_sha256"] = sha256(output / "reduced_control.obj")
        receipt["status"] = "constructed"
    except Exception as error:
        receipt.update(status="failed; diagnostics retained", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        receipt["timings"] = {"seconds": time.monotonic() - started}
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print("CONTROL", json.dumps(receipt["reduced_control"]), flush=True)


if __name__ == "__main__":
    arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(arguments) != 2:
        raise SystemExit("expected recipe.json new-output-directory")
    main(*arguments)
