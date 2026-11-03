"""Stitch generated sleeve tubes into a garment's sleeve openings.

The wardrobe sleeve is a single-sided relief sheet, so radial inflation can only
bend it (audit 7). This bridges the tube's end rings to the garment's opening
boundary loops, so the sleeve both wraps the arm and shares the garment's seam.

Usage:
    blender --background --factory-startup --python tools/stitch_sleeves.py -- \
        garment=<garment.obj> tube=<tube.obj> out=<out.obj> [max_ring=40]
"""

import sys

import bmesh
import bpy

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
cfg = dict(arg.split("=", 1) for arg in argv if "=" in arg)
for key in ("garment", "tube", "out"):
    if key not in cfg:
        raise SystemExit(f"missing `{key}=...`")
MAX_RING = int(cfg.get("max_ring", "40"))

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)

bpy.ops.wm.obj_import(filepath=cfg["garment"])
garment = [o for o in bpy.data.objects if o.type == "MESH"][0]
bpy.ops.wm.obj_import(filepath=cfg["tube"])
tube = [o for o in bpy.data.objects if o.type == "MESH" and o is not garment][0]

# One mesh: bridge_loops works inside a single bmesh.
bpy.ops.object.select_all(action="DESELECT")
garment.select_set(True)
tube.select_set(True)
bpy.context.view_layer.objects.active = garment
bpy.ops.object.join()
mesh = bpy.context.active_object

bm = bmesh.new()
bm.from_mesh(mesh.data)
bm.verts.ensure_lookup_table()
bm.edges.ensure_lookup_table()


def loops_of_boundary(bmesh_data):
    """Group boundary edges into vertex loops."""
    boundary = [e for e in bmesh_data.edges if len(e.link_faces) == 1]
    parent = {}

    def find(a):
        parent.setdefault(a, a)
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[ra] = rb

    for edge in boundary:
        union(edge.verts[0].index, edge.verts[1].index)
    groups = {}
    for edge in boundary:
        groups.setdefault(find(edge.verts[0].index), []).append(edge)
    return list(groups.values())


def centroid(loop):
    verts = {v for edge in loop for v in edge.verts}
    x = sum(v.co.x for v in verts) / len(verts)
    y = sum(v.co.y for v in verts) / len(verts)
    z = sum(v.co.z for v in verts) / len(verts)
    return (x, y, z)


loops = loops_of_boundary(bm)
# Only the generated tube rings count as "rings": they all have the same edge
# count (16) and sit in the arm region. Small garment tears must not be bridged.
ring_size = 16


def in_arm_region(loop, threshold=20.0):
    return abs(centroid(loop)[0]) > threshold


small = [loop for loop in loops if len(loop) == ring_size and in_arm_region(loop)]
large = [loop for loop in loops if len(loop) > 100 and in_arm_region(loop)]
print(f"  tube rings (<= {MAX_RING} edges): {[len(l) for l in small]}")
print(f"  garment openings: {sorted((len(l) for l in large), reverse=True)[:4]}")


bridged = 0
for ring in small:
    ring_centre = centroid(ring)
    # nearest garment opening on the same side of the body
    candidates = [loop for loop in large if centroid(loop)[0] * ring_centre[0] > 0]
    if not candidates:
        continue
    opening = min(candidates, key=lambda loop: abs(centroid(loop)[0] - ring_centre[0]))
    try:
        bmesh.ops.bridge_loops(bm, edges=ring + opening)
        bridged += 1
        print(
            f"  bridged ring ({len(ring)} edges) to opening ({len(opening)} edges) "
            f"at x={ring_centre[0]:.1f}"
        )
    except RuntimeError as error:
        print(f"  bridge failed at x={ring_centre[0]:.1f}: {error}")

bm.to_mesh(mesh.data)
bm.free()
mesh.data.update()
print(f"bridged {bridged} sleeve rings")

# write by hand so nothing else in the scene leaks into the file
with open(cfg["out"], "w", encoding="utf-8") as sink:
    for vertex in mesh.data.vertices:
        co = mesh.matrix_world @ vertex.co
        sink.write(f"v {co.x:.6f} {co.y:.6f} {co.z:.6f}\n")
    for polygon in mesh.data.polygons:
        sink.write("f " + " ".join(str(i + 1) for i in polygon.vertices) + "\n")
print(f"wrote {cfg['out']}: {len(mesh.data.vertices)} verts, {len(mesh.data.polygons)} faces")